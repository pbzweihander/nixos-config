use serde_json::{json, Value};
use std::{
    fs,
    io::{BufRead, BufReader, Write},
    os::unix::net::UnixListener,
    path::PathBuf,
    process::{Command, Stdio},
    sync::{
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

struct Wiki(PathBuf);

impl Wiki {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let dir = std::env::temp_dir().join(format!(
            "cw-cli-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(dir.join("wiki/pages/knowledge")).unwrap();
        for name in ["foo", "bar"] {
            fs::write(
                dir.join(format!("wiki/pages/knowledge/{name}.md")),
                format!("---\ntitle: {name} cache notes\ntags: [cache]\nproject: demo\ncreated: 2026-09-20\n---\nRecovery details.\n"),
            ).unwrap();
        }
        let wiki = Self(dir);
        run(wiki.command(&["list"]), "");
        wiki
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_claude-wiki"));
        command
            .args(args)
            .env("CLAUDE_WIKI_DIR", self.0.join("wiki"))
            .env("CLAUDE_WIKI_SOCKET", self.0.join("embed.sock"))
            .env("CLAUDE_WIKI_SEMANTIC_TIMEOUT_MS", "1000")
            .env("CLAUDE_WIKI_REMIND_INTERVAL", "15")
            .env("GIT_CONFIG_NOSYSTEM", "1")
            .env("GIT_CONFIG_GLOBAL", "/dev/null");
        command
    }

    fn listener(&self) -> UnixListener {
        UnixListener::bind(self.0.join("embed.sock")).unwrap()
    }

    fn respond(&self, args: &[&str], input: &str, reply: Value) -> (String, Value) {
        let listener = self.listener();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut request = String::new();
            BufReader::new(&mut stream).read_line(&mut request).unwrap();
            writeln!(stream, "{reply}").unwrap();
            serde_json::from_str(&request).unwrap()
        });
        let output = run(self.command(args), input);
        (output, server.join().unwrap())
    }
}

impl Drop for Wiki {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn run(mut command: Command, input: &str) -> String {
    let mut child = command
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    child
        .stdin
        .take()
        .unwrap()
        .write_all(input.as_bytes())
        .unwrap();
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success(), "{output:?}");
    assert!(output.stderr.is_empty(), "{output:?}");
    String::from_utf8(output.stdout).unwrap()
}

fn assert_no_connection(listener: &UnixListener) {
    listener.set_nonblocking(true).unwrap();
    assert_eq!(
        listener.accept().unwrap_err().kind(),
        std::io::ErrorKind::WouldBlock
    );
}

#[test]
fn keyword_hits_filters_and_opt_out_skip_daemon() {
    let wiki = Wiki::new();
    let listener = wiki.listener();
    let output = run(wiki.command(&["search", "cache"]), "");
    assert!(output.contains("foo cache notes"));
    assert!(!output.contains("semantic matches"));
    assert_no_connection(&listener);
    for flags in [
        vec!["--type", "knowledge"],
        vec!["--status", "open"],
        vec!["--project", "demo"],
        vec!["--tag", "cache"],
        vec!["--no-semantic"],
    ] {
        let mut args = vec!["search", "unmatchedmeaning"];
        args.extend(flags);
        let output = run(wiki.command(&args), "");
        assert!(output.starts_with("no pages match"));
        assert_eq!(output.lines().count(), 1);
        assert_no_connection(&listener);
    }
}

#[test]
fn semantic_search_renders_pages_and_supplied_section() {
    let wiki = Wiki::new();
    let (output, request) = wiki.respond(
        &["search", "unmatchedmeaning", "anotherquery", "-n", "3"],
        "",
        json!({"ok": true, "tier": "gpu", "results": [
            {"path": "knowledge/deleted", "score": 2.0},
            {"path": "knowledge/foo", "score": 1.23, "heading": "What kills it", "start_line": 27, "end_line": 36},
            {"path": "knowledge/bar", "score": 0.9}
        ]}),
    );
    assert_eq!(
        request,
        json!({"op": "search", "query": "unmatchedmeaning anotherquery", "n": 3})
    );
    let lines: Vec<_> = output.lines().collect();
    assert!(lines[0].starts_with("no pages match"));
    assert_eq!(lines[1], "semantic matches (meaning, not keywords):");
    assert_eq!(
        lines[2],
        wiki.0.join("wiki/pages/knowledge/foo.md").to_str().unwrap()
    );
    assert!(lines[3].starts_with("  foo cache notes  (knowledge; updated "));
    assert!(lines[3].ends_with("; project=demo; tags=cache)"));
    assert_eq!(lines[4], "  § What kills it (lines 27-36)");
    assert_eq!(
        lines[5],
        wiki.0.join("wiki/pages/knowledge/bar.md").to_str().unwrap()
    );
    assert!(lines[6].starts_with("  bar cache notes  (knowledge; updated "));
    assert_eq!(lines.len(), 7);
}

#[test]
fn reminder_sends_raw_korean_and_keeps_nudge() {
    let wiki = Wiki::new();
    let prompt = "  캐시가 왜 사라져요?\n다시 시작하면 그래요.";
    let (output, request) = wiki.respond(
        &["remind", "--interval", "1"],
        &json!({"session_id": "korean", "prompt": prompt}).to_string(),
        json!({"ok": true, "tier": "cpu", "results": [{"path": "knowledge/foo", "score": 1.0}]}),
    );
    assert_eq!(request, json!({"op": "related", "prompt": prompt, "n": 3}));
    assert_eq!(output.lines().next().unwrap(), "claude-wiki: pages that may already cover this: knowledge/foo. Read one with `claude-wiki search <terms>` or the Read tool if it is relevant; otherwise ignore this note and do not reply to it.");
    assert!(output.contains("claude-wiki: 1 prompts since"));
}

#[test]
fn wrapper_and_empty_socket_skip_daemon_without_disabling_nudge() {
    let wiki = Wiki::new();
    let listener = wiki.listener();
    let wrapper = json!({"prompt": " \n\t<command-name>/config</command-name> cache recovery"});
    assert!(run(wiki.command(&["remind"]), &wrapper.to_string()).is_empty());
    assert_no_connection(&listener);
    let mut command = wiki.command(&["remind", "--interval", "1"]);
    command.env("CLAUDE_WIKI_SOCKET", "");
    let output = run(command, &json!({"prompt": "cache recovery"}).to_string());
    assert!(output.starts_with("claude-wiki: 2 prompts since"));
    assert_eq!(output.lines().count(), 1);
    assert_no_connection(&listener);
}

#[test]
fn empty_semantic_answer_adds_no_output() {
    for args in [vec!["search", "unmatchedmeaning"], vec!["remind"]] {
        let wiki = Wiki::new();
        let (output, _) = wiki.respond(
            &args,
            &json!({"prompt": "cache recovery"}).to_string(),
            json!({"ok": true, "tier": "cpu", "results": []}),
        );
        if args[0] == "search" {
            assert!(output.starts_with("no pages match"));
            assert_eq!(output.lines().count(), 1);
        } else {
            assert!(output.is_empty());
        }
    }
}

#[test]
fn runtime_directory_socket_is_the_default() {
    let wiki = Wiki::new();
    fs::create_dir(wiki.0.join("claude-wiki")).unwrap();
    let listener = UnixListener::bind(wiki.0.join("claude-wiki/embed.sock")).unwrap();
    let server = thread::spawn(move || {
        let (mut stream, _) = listener.accept().unwrap();
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut request = String::new();
        BufReader::new(&mut stream).read_line(&mut request).unwrap();
        writeln!(stream, "{}", json!({"ok": true, "results": []})).unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&request).unwrap()["op"],
            "search"
        );
    });
    let mut command = wiki.command(&["search", "unmatchedmeaning"]);
    command
        .env_remove("CLAUDE_WIKI_SOCKET")
        .env("XDG_RUNTIME_DIR", &wiki.0);
    assert_eq!(run(command, "").lines().count(), 1);
    server.join().unwrap();
}

#[test]
fn timeout_override_keeps_stalled_daemon_from_delaying_nudge() {
    let wiki = Wiki::new();
    let listener = wiki.listener();
    let (release, wait) = mpsc::channel();
    let server = thread::spawn(move || {
        let (_stream, _) = listener.accept().unwrap();
        let _ = wait.recv_timeout(Duration::from_secs(2));
    });
    let mut command = wiki.command(&["remind", "--interval", "1"]);
    command.env("CLAUDE_WIKI_SEMANTIC_TIMEOUT_MS", "40");
    let start = Instant::now();
    let output = run(command, &json!({"prompt": "cache recovery"}).to_string());
    let elapsed = start.elapsed();
    let _ = release.send(());
    server.join().unwrap();
    assert!(elapsed < Duration::from_millis(300), "took {elapsed:?}");
    assert!(output.starts_with("claude-wiki: 1 prompts since"));
    assert_eq!(output.lines().count(), 1);
}
