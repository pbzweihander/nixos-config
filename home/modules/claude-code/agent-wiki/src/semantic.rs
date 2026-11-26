use serde::{Deserialize, Serialize, de::DeserializeOwned};
use std::{
    env,
    io::{self, Read, Write},
    os::unix::net::UnixStream,
    path::{Path, PathBuf},
    sync::mpsc,
    thread,
    time::{Duration, Instant},
};

const DEFAULT_TIMEOUT_MS: u64 = 300;
const MAX_MESSAGE_BYTES: usize = 1024 * 1024;

#[derive(Debug, PartialEq)]
pub struct Hit {
    pub path: String,
    pub section: Option<(String, i64, i64)>,
}

pub enum DenseStatus {
    Disabled,
    Ok,
    Unavailable(String),
}

impl DenseStatus {
    pub fn name(&self) -> &'static str {
        match self {
            Self::Disabled => "disabled",
            Self::Ok => "ok",
            Self::Unavailable(_) => "unavailable",
        }
    }
}

/// A daemon hit checked against the local index, shared by `search` and `related`.
#[derive(Debug, PartialEq)]
pub struct Resolved {
    /// Index path (`pages/<type>/<slug>.md`) of a well-formed daemon page id.
    pub rel: String,
    /// The daemon's section, kept only while the local index still has that exact
    /// section for the page.
    pub section: Option<(String, i64, i64)>,
}

/// Resolves a daemon hit to an index path and a current section. `None` for a
/// malformed id, so a corrupt daemon cannot name a file outside `pages/`.
///
/// The daemon reads sections from this index, but its snapshot can lag behind an
/// edit: a section that no longer exists (renamed heading, moved or removed lines)
/// is dropped while the page itself stays a hit. Visibility rules (blocked pages,
/// filters, history) stay with each caller.
pub fn resolve(db: &rusqlite::Connection, hit: Hit) -> rusqlite::Result<Option<Resolved>> {
    let Some(rel) = crate::pages::link_path(&hit.path) else {
        return Ok(None);
    };
    let section = match hit.section {
        Some((heading, start, end)) => {
            let current: bool = db.query_row(
                "select exists(select 1 from sections join files on files.id = sections.file_id
                 where files.path = ? and sections.heading = ?
                   and sections.start_line = ? and sections.end_line = ?)",
                rusqlite::params![rel, heading, start, end],
                |r| r.get(0),
            )?;
            current.then_some((heading, start, end))
        }
        None => None,
    };
    Ok(Some(Resolved { rel, section }))
}

#[derive(Debug, Serialize)]
pub struct Section {
    pub heading: String,
    pub start: i64,
    pub end: i64,
}

impl From<(String, i64, i64)> for Section {
    fn from((heading, start, end): (String, i64, i64)) -> Self {
        Self {
            heading,
            start,
            end,
        }
    }
}

#[derive(Serialize)]
#[serde(tag = "op", rename_all = "lowercase")]
enum Request {
    Related {
        prompt: String,
        n: i64,
    },
    Dense {
        query: String,
        n: i64,
    },
    Similar {
        text: String,
        exclude: String,
        n: i64,
        min: f64,
    },
    Pairs {
        min: f64,
        types: Vec<String>,
    },
}

#[derive(Deserialize)]
struct Response<T> {
    ok: bool,
    results: Option<Vec<T>>,
    error: Option<serde_json::Value>,
}

#[derive(Deserialize)]
struct WireHit {
    path: String,
    #[serde(rename = "score")]
    _score: f64,
    heading: Option<String>,
    start_line: Option<i64>,
    end_line: Option<i64>,
}

impl WireHit {
    fn into_hit(self) -> Option<Hit> {
        let section = match (self.heading, self.start_line, self.end_line) {
            (None, None, None) => None,
            (Some(heading), Some(start), Some(end)) if start > 0 && end >= start => {
                Some((heading, start, end))
            }
            _ => return None,
        };
        Some(Hit {
            path: self.path,
            section,
        })
    }
}

fn socket_path() -> Option<PathBuf> {
    match crate::compat::var_os("AGENT_WIKI_SOCKET") {
        Some(path) => (!path.is_empty()).then(|| PathBuf::from(path)),
        None => env::var_os("XDG_RUNTIME_DIR")
            .filter(|dir| !dir.is_empty())
            .map(|dir| crate::compat::directory(&PathBuf::from(dir)).join("embed.sock")),
    }
}

fn timeout_reason(timeout: Duration) -> String {
    format!("timed out after {} ms", timeout.as_millis())
}

fn io_reason(error: io::Error, timeout: Duration) -> String {
    match error.kind() {
        io::ErrorKind::TimedOut | io::ErrorKind::WouldBlock => timeout_reason(timeout),
        _ => format!("socket I/O error: {error}"),
    }
}

fn remaining(deadline: Instant, timeout: Duration) -> Result<Duration, String> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|left| !left.is_zero())
        .ok_or_else(|| timeout_reason(timeout))
}

fn exchange<T: DeserializeOwned>(
    path: &Path,
    request: Request,
    deadline: Instant,
    timeout: Duration,
) -> Result<Vec<T>, String> {
    let mut bytes = serde_json::to_vec(&request).map_err(|_| "invalid request")?;
    bytes.push(b'\n');
    if bytes.len() > MAX_MESSAGE_BYTES {
        return Err("oversized request".into());
    }
    remaining(deadline, timeout)?;
    let mut stream =
        UnixStream::connect(path).map_err(|_| format!("cannot connect to {}", path.display()))?;
    let mut pending = bytes.as_slice();
    while !pending.is_empty() {
        stream
            .set_write_timeout(Some(remaining(deadline, timeout)?))
            .map_err(|error| io_reason(error, timeout))?;
        let written = stream
            .write(pending)
            .map_err(|error| io_reason(error, timeout))?;
        if written == 0 {
            return Err("socket closed while writing request".into());
        }
        pending = &pending[written..];
    }

    bytes.clear();
    let mut buffer = [0; 4096];
    loop {
        // A peer trickling bytes must not restart the deadline with each read.
        stream
            .set_read_timeout(Some(remaining(deadline, timeout)?))
            .map_err(|error| io_reason(error, timeout))?;
        let read = stream
            .read(&mut buffer)
            .map_err(|error| io_reason(error, timeout))?;
        if read == 0 {
            return Err("invalid response (incomplete line)".into());
        }
        let newline = buffer[..read].iter().position(|&byte| byte == b'\n');
        bytes.extend_from_slice(&buffer[..newline.unwrap_or(read)]);
        // Bound memory even if a broken peer never terminates its response line.
        if bytes.len() > MAX_MESSAGE_BYTES {
            return Err("oversized response".into());
        }
        if newline.is_some() {
            let response: Response<T> =
                serde_json::from_slice(&bytes).map_err(|_| "invalid response")?;
            return if response.ok {
                response.results.ok_or_else(|| "invalid response".into())
            } else {
                let reason = response.error.as_ref().and_then(|error| error.as_str());
                Err(reason.map_or_else(
                    || "daemon error".into(),
                    |reason| {
                        let reason: String = reason
                            .chars()
                            .take(200)
                            .map(|c| if c.is_control() { ' ' } else { c })
                            .collect();
                        format!("daemon error: {reason}")
                    },
                ))
            };
        }
    }
}

fn exchange_at<T: DeserializeOwned + Send + 'static>(
    path: PathBuf,
    request: Request,
    timeout: Duration,
) -> Result<Vec<T>, String> {
    let deadline = Instant::now()
        .checked_add(timeout)
        .ok_or("invalid timeout")?;
    remaining(deadline, timeout)?;
    let (send, receive) = mpsc::channel();
    // UnixStream has no connect timeout. Keep the whole exchange off the caller
    // so even a full listen backlog cannot hold the hook beyond its deadline.
    thread::Builder::new()
        .spawn(move || {
            let _ = send.send(exchange(&path, request, deadline, timeout));
        })
        .map_err(|_| "could not start request worker")?;
    receive
        .recv_timeout(deadline.saturating_duration_since(Instant::now()))
        .map_err(|error| match error {
            mpsc::RecvTimeoutError::Timeout => timeout_reason(timeout),
            mpsc::RecvTimeoutError::Disconnected => "request worker stopped".into(),
        })?
}

fn request_at_with_status(
    path: PathBuf,
    request: Request,
    timeout: Duration,
) -> Result<Vec<Hit>, String> {
    exchange_at::<WireHit>(path, request, timeout)?
        .into_iter()
        .map(WireHit::into_hit)
        .collect::<Option<Vec<_>>>()
        .ok_or_else(|| "invalid response".into())
}

fn request_at(path: PathBuf, request: Request, timeout: Duration) -> Vec<Hit> {
    request_at_with_status(path, request, timeout).unwrap_or_default()
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Similar {
    pub path: String,
    pub score: f64,
}

#[derive(Debug, Deserialize, Serialize)]
pub struct Pair {
    pub a: String,
    pub b: String,
    pub score: f64,
}

fn batch<T: DeserializeOwned + Send + 'static>(request: Request) -> Option<Vec<T>> {
    let timeout = crate::compat::var("AGENT_WIKI_SEMANTIC_BATCH_TIMEOUT_MS")
        .ok()
        .and_then(|s| s.parse().ok())
        .unwrap_or(5000);
    exchange_at(socket_path()?, request, Duration::from_millis(timeout)).ok()
}

pub fn similar(text: &str, exclude: &str, min: f64) -> Vec<Similar> {
    batch(Request::Similar {
        text: text.into(),
        exclude: exclude.into(),
        n: 3,
        min,
    })
    .unwrap_or_default()
}

pub fn pairs(min: f64) -> Option<Vec<Pair>> {
    batch(Request::Pairs {
        min,
        types: vec!["knowledge".into(), "followups".into()],
    })
}

fn request_timeout() -> Duration {
    let timeout = crate::compat::var("AGENT_WIKI_SEMANTIC_TIMEOUT_MS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(DEFAULT_TIMEOUT_MS);
    Duration::from_millis(timeout)
}

fn request(request: Request) -> Vec<Hit> {
    let Some(path) = socket_path() else {
        return vec![];
    };
    request_at(path, request, request_timeout())
}

pub fn related(prompt: &str) -> Vec<Hit> {
    request(Request::Related {
        prompt: prompt.to_owned(),
        n: 10,
    })
}

pub fn related_with_timeout(prompt: &str, timeout: Duration) -> Vec<Hit> {
    let Some(path) = socket_path() else {
        return vec![];
    };
    request_at(
        path,
        Request::Related {
            prompt: prompt.to_owned(),
            n: 10,
        },
        timeout,
    )
}

pub fn dense_with_status(query: &str, n: i64) -> (Vec<Hit>, DenseStatus) {
    if n == 0 {
        return (vec![], DenseStatus::Disabled);
    }
    let Some(path) = socket_path() else {
        return (vec![], DenseStatus::Disabled);
    };
    match request_at_with_status(
        path,
        Request::Dense {
            query: query.to_owned(),
            n: n.clamp(1, 100),
        },
        request_timeout(),
    ) {
        Ok(hits) => (hits, DenseStatus::Ok),
        Err(reason) => (vec![], DenseStatus::Unavailable(reason)),
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::{
        fs,
        io::{BufRead, BufReader},
        os::unix::net::UnixListener,
        sync::atomic::{AtomicU64, Ordering},
    };

    struct SocketDir(PathBuf);

    impl SocketDir {
        fn new() -> Self {
            static NEXT: AtomicU64 = AtomicU64::new(0);
            let path = env::temp_dir().join(format!(
                "cw-sem-{}-{}",
                std::process::id(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ));
            fs::create_dir(&path).unwrap();
            Self(path)
        }

        fn socket(&self) -> PathBuf {
            self.0.join("embed.sock")
        }
    }

    impl Drop for SocketDir {
        fn drop(&mut self) {
            let _ = fs::remove_dir_all(&self.0);
        }
    }

    fn related_request() -> Request {
        Request::Related {
            prompt: "캐시가 왜 사라져요?\n\"다시 시작\"".into(),
            n: 3,
        }
    }

    fn with_server(
        request: Request,
        handler: impl FnOnce(UnixStream) + Send + 'static,
    ) -> Vec<Hit> {
        let dir = SocketDir::new();
        let listener = UnixListener::bind(dir.socket()).unwrap();
        let server = thread::spawn(move || {
            let (stream, _) = listener.accept().unwrap();
            handler(stream);
        });
        let results = request_at(dir.socket(), request, Duration::from_secs(1));
        server.join().unwrap();
        results
    }

    fn read_request(stream: &mut UnixStream) -> serde_json::Value {
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut line = String::new();
        BufReader::new(stream).read_line(&mut line).unwrap();
        assert!(line.ends_with('\n'));
        serde_json::from_str(&line).unwrap()
    }

    fn reply(bytes: &'static [u8]) -> Vec<Hit> {
        with_server(related_request(), move |mut stream| {
            read_request(&mut stream);
            stream.write_all(bytes).unwrap();
        })
    }

    #[test]
    fn request_encoding() {
        assert_eq!(
            serde_json::to_value(related_request()).unwrap(),
            serde_json::json!({"op": "related", "prompt": "캐시가 왜 사라져요?\n\"다시 시작\"", "n": 3})
        );
        assert_eq!(
            serde_json::to_value(Request::Dense {
                query: "exact error".into(),
                n: 10
            })
            .unwrap(),
            serde_json::json!({"op": "dense", "query": "exact error", "n": 10})
        );
    }

    #[test]
    fn response_with_and_without_sections() {
        let hits = with_server(related_request(), |mut stream| {
            assert_eq!(
                read_request(&mut stream),
                serde_json::to_value(related_request()).unwrap()
            );
            // Splitting a reply across reads is valid; EOF before the newline is not.
            stream
                .write_all(br#"{"ok":true,"tier":"gpu","results":["#)
                .unwrap();
            stream.write_all(b"{\"path\":\"knowledge/foo\",\"score\":1.23,\"heading\":\"What kills it\",\"start_line\":27,\"end_line\":36},{\"path\":\"knowledge/bar\",\"score\":0.9}]}\n").unwrap();
        });
        assert_eq!(
            hits,
            vec![
                Hit {
                    path: "knowledge/foo".into(),
                    section: Some(("What kills it".into(), 27, 36))
                },
                Hit {
                    path: "knowledge/bar".into(),
                    section: None
                },
            ]
        );
        assert!(reply(b"{\"ok\":true,\"tier\":\"cpu\",\"results\":[]}\n").is_empty());
    }

    #[test]
    fn resolve_validates_ids_and_drops_stale_sections() -> anyhow::Result<()> {
        let mut wiki = crate::usage::tests::Wiki::new()?;
        wiki.page(
            "pages/knowledge/current.md",
            "---\ntitle: Current\ncreated: 2026-11-27\n---\n# Kept\nline\n# Other\nline\n",
        )?;
        wiki.page(
            "pages/knowledge/other.md",
            "---\ntitle: Other\ncreated: 2026-11-27\n---\n# Only here\nline\n",
        )?;
        let hit = |path: &str, section: Option<(&str, i64, i64)>| Hit {
            path: path.into(),
            section: section.map(|(h, s, e)| (h.to_owned(), s, e)),
        };
        let resolved = |path, section| resolve(&wiki.index, hit(path, section)).unwrap();
        assert_eq!(
            resolved("knowledge/current", Some(("Kept", 5, 6))),
            Some(Resolved {
                rel: "pages/knowledge/current.md".into(),
                section: Some(("Kept".into(), 5, 6)),
            })
        );
        for stale in [
            ("Removed heading", 5, 6),
            ("Kept", 900, 999),
            ("Kept", 5, 7),
            ("Other", 5, 6),
        ] {
            assert_eq!(
                resolved("knowledge/current", Some(stale)),
                Some(Resolved {
                    rel: "pages/knowledge/current.md".into(),
                    section: None,
                }),
                "{stale:?}"
            );
        }
        // Another page's section never applies to this one.
        assert_eq!(
            resolved("knowledge/current", Some(("Only here", 5, 6)))
                .unwrap()
                .section,
            None
        );
        for malformed in [
            "../outside",
            "knowledge/../x",
            "secret/x",
            "/etc/passwd",
            "",
        ] {
            assert_eq!(resolved(malformed, None), None, "{malformed}");
        }
        // A well-formed id for a deleted page still resolves: callers check rows.
        assert_eq!(
            resolved("knowledge/deleted", Some(("Kept", 5, 6))),
            Some(Resolved {
                rel: "pages/knowledge/deleted.md".into(),
                section: None,
            })
        );
        Ok(())
    }

    #[test]
    fn missing_socket() {
        let dir = SocketDir::new();
        assert!(request_at(dir.socket(), related_request(), Duration::from_millis(50)).is_empty());
    }

    #[test]
    fn refused_connection() {
        let dir = SocketDir::new();
        drop(UnixListener::bind(dir.socket()).unwrap());
        assert!(request_at(dir.socket(), related_request(), Duration::from_millis(50)).is_empty());
    }

    #[test]
    fn truncated_line() {
        assert!(reply(b"{\"ok\":true,\"results\":[").is_empty());
        assert!(
            reply(b"{\"ok\":true,\"results\":[{\"path\":\"knowledge/foo\",\"score\":1}]}")
                .is_empty()
        );
    }

    #[test]
    fn invalid_json() {
        assert!(reply(b"not json\n").is_empty());
        assert!(reply(b"{\"ok\":true,\"results\":\"bad type\"}\n").is_empty());
        assert!(reply(b"{\"ok\":true,\"results\":[{\"path\":\"knowledge/foo\",\"score\":1,\"heading\":\"incomplete\"}]}\n").is_empty());
    }

    #[test]
    fn daemon_error() {
        assert!(reply(b"{\"ok\":false,\"error\":\"warming up\"}\n").is_empty());
        assert!(
            reply(b"{\"ok\":false,\"results\":[{\"path\":\"knowledge/foo\",\"score\":1}]}\n")
                .is_empty()
        );
    }

    fn stalled_peer(request: Request, read: bool) {
        let dir = SocketDir::new();
        let listener = UnixListener::bind(dir.socket()).unwrap();
        let (release, wait) = mpsc::channel();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            if read {
                read_request(&mut stream);
            }
            let _ = wait.recv_timeout(Duration::from_secs(2));
        });
        let start = Instant::now();
        let hits = request_at(dir.socket(), request, Duration::from_millis(75));
        let elapsed = start.elapsed();
        let _ = release.send(());
        server.join().unwrap();
        assert!(hits.is_empty());
        assert!(elapsed < Duration::from_millis(300), "took {elapsed:?}");
    }

    #[test]
    fn accept_then_stall() {
        stalled_peer(related_request(), true);
    }

    #[test]
    fn write_stall() {
        stalled_peer(
            Request::Dense {
                query: "x".repeat(900_000),
                n: 10,
            },
            false,
        );
    }

    #[test]
    fn trickling_reply_has_one_total_deadline() {
        let dir = SocketDir::new();
        let listener = UnixListener::bind(dir.socket()).unwrap();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            read_request(&mut stream);
            for byte in b"{\"ok\":true,\"results\":[]}\n" {
                if stream.write_all(&[*byte]).is_err() {
                    break;
                }
                thread::sleep(Duration::from_millis(20));
            }
        });
        let start = Instant::now();
        assert!(request_at(dir.socket(), related_request(), Duration::from_millis(75)).is_empty());
        assert!(start.elapsed() < Duration::from_millis(300));
        server.join().unwrap();
    }
}
