use serde::{Deserialize, Serialize};
use std::{
    env,
    io::{Read, Write},
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

#[derive(Serialize)]
#[serde(tag = "op", rename_all = "lowercase")]
enum Request {
    Related { prompt: String, n: i64 },
    Search { query: String, n: i64 },
}

#[derive(Deserialize)]
struct Response {
    ok: bool,
    results: Option<Vec<WireHit>>,
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
    match env::var_os("CLAUDE_WIKI_SOCKET") {
        Some(path) => (!path.is_empty()).then(|| PathBuf::from(path)),
        None => env::var_os("XDG_RUNTIME_DIR")
            .filter(|dir| !dir.is_empty())
            .map(|dir| PathBuf::from(dir).join("claude-wiki/embed.sock")),
    }
}

fn remaining(deadline: Instant) -> Option<Duration> {
    deadline
        .checked_duration_since(Instant::now())
        .filter(|left| !left.is_zero())
}

fn exchange(path: &Path, request: Request, deadline: Instant) -> Option<Vec<Hit>> {
    let mut bytes = serde_json::to_vec(&request).ok()?;
    bytes.push(b'\n');
    if bytes.len() > MAX_MESSAGE_BYTES {
        return None;
    }
    remaining(deadline)?;
    let mut stream = UnixStream::connect(path).ok()?;
    let mut pending = bytes.as_slice();
    while !pending.is_empty() {
        stream.set_write_timeout(Some(remaining(deadline)?)).ok()?;
        let written = stream.write(pending).ok()?;
        if written == 0 {
            return None;
        }
        pending = &pending[written..];
    }

    bytes.clear();
    let mut buffer = [0; 4096];
    loop {
        // A peer trickling bytes must not restart the deadline with each read.
        stream.set_read_timeout(Some(remaining(deadline)?)).ok()?;
        let read = stream.read(&mut buffer).ok()?;
        if read == 0 {
            return None;
        }
        let newline = buffer[..read].iter().position(|&byte| byte == b'\n');
        bytes.extend_from_slice(&buffer[..newline.unwrap_or(read)]);
        // Bound memory even if a broken peer never terminates its response line.
        if bytes.len() > MAX_MESSAGE_BYTES {
            return None;
        }
        if newline.is_some() {
            let response: Response = serde_json::from_slice(&bytes).ok()?;
            return if response.ok {
                response
                    .results?
                    .into_iter()
                    .map(WireHit::into_hit)
                    .collect()
            } else {
                None
            };
        }
    }
}

fn request_at(path: PathBuf, request: Request, timeout: Duration) -> Vec<Hit> {
    let Some(deadline) = Instant::now().checked_add(timeout) else {
        return vec![];
    };
    if remaining(deadline).is_none() {
        return vec![];
    }
    let (send, receive) = mpsc::channel();
    // UnixStream has no connect timeout. Keep the whole exchange off the caller
    // so even a full listen backlog cannot hold the hook beyond its deadline.
    if thread::Builder::new()
        .spawn(move || {
            let _ = send.send(exchange(&path, request, deadline).unwrap_or_default());
        })
        .is_err()
    {
        return vec![];
    }
    receive
        .recv_timeout(deadline.saturating_duration_since(Instant::now()))
        .unwrap_or_default()
}

fn request(request: Request) -> Vec<Hit> {
    let Some(path) = socket_path() else {
        return vec![];
    };
    let timeout = env::var("CLAUDE_WIKI_SEMANTIC_TIMEOUT_MS")
        .ok()
        .and_then(|value| value.parse().ok())
        .unwrap_or(DEFAULT_TIMEOUT_MS);
    request_at(path, request, Duration::from_millis(timeout))
}

pub fn related(prompt: &str) -> Vec<Hit> {
    request(Request::Related {
        prompt: prompt.to_owned(),
        n: 3,
    })
}

pub fn search(query: &str, n: i64) -> Vec<Hit> {
    if n == 0 {
        return vec![];
    }
    request(Request::Search {
        query: query.to_owned(),
        // SQLite accepts a negative limit as unlimited; the daemon caps at 1000.
        n: if n < 0 { 1000 } else { n.min(1000) },
    })
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
            serde_json::to_value(Request::Search {
                query: "exact error".into(),
                n: 10
            })
            .unwrap(),
            serde_json::json!({"op": "search", "query": "exact error", "n": 10})
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
            Request::Search {
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
