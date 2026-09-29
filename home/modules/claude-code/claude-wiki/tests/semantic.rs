use serde_json::{json, Value};
use std::{
    fs,
    io::{BufRead, BufReader, Write},
    os::unix::net::UnixListener,
    path::PathBuf,
    process::{Command, Output, Stdio},
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
            .env("CLAUDE_WIKI_SKILL_DIRS", self.0.join("skills"))
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
        let (output, request) = self.respond_output(args, input, reply);
        assert!(output.status.success(), "{output:?}");
        assert!(output.stderr.is_empty(), "{output:?}");
        (String::from_utf8(output.stdout).unwrap(), request)
    }

    fn respond_output(&self, args: &[&str], input: &str, reply: Value) -> (Output, Value) {
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
        let output = run_output(self.command(args), input);
        (output, server.join().unwrap())
    }
}

impl Drop for Wiki {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn run_output(mut command: Command, input: &str) -> Output {
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
    child.wait_with_output().unwrap()
}

fn run(command: Command, input: &str) -> String {
    let output = run_output(command, input);
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
fn probe_uses_semantic_ranks_even_when_keywords_match() {
    for (query, paths, top, failure, diagnostic) in [
        (
            "캐시가 왜 사라져요?",
            vec!["knowledge/bar", "knowledge/foo"],
            "3",
            false,
            "",
        ),
        (
            "캐시가 왜 사라져요?",
            vec!["knowledge/bar", "knowledge/foo"],
            "1",
            true,
            "semantic: rank 2",
        ),
        (
            "캐시가 왜 사라져요?",
            vec!["knowledge/bar"],
            "3",
            true,
            "semantic: not in top 10",
        ),
        (
            "캐시가 왜 사라져요?",
            vec![],
            "3",
            true,
            "semantic: unavailable",
        ),
        ("foo", vec![], "1", false, ""),
    ] {
        let wiki = Wiki::new();
        fs::write(wiki.0.join("wiki/pages/knowledge/foo.md"), format!(
            "---\ntitle: foo cache notes\ncreated: 2026-09-29\nqueries: [{query}]\n---\nRecovery details.\n"
        )).unwrap();
        let results: Vec<_> = paths
            .iter()
            .map(|path| json!({"path": path, "score": 1.0}))
            .collect();
        let (output, request) = wiki.respond_output(
            &["check", "--probe", "--probe-top", top],
            "",
            json!({"ok": true, "results": results}),
        );
        assert_eq!(request, json!({"op": "search", "query": query, "n": 10}));
        assert_eq!(output.status.code(), Some(i32::from(failure)), "{output:?}");
        let stderr = String::from_utf8(output.stderr).unwrap();
        assert!(stderr.contains(diagnostic), "{stderr}");
        assert!(
            stderr.contains(&format!(
                "1 pages probed, 1 queries, {} failures",
                i32::from(failure)
            )),
            "{stderr}"
        );
        assert_eq!(stderr.lines().count(), if failure { 2 } else { 1 });
    }
}

#[test]
fn probe_bm25_matches_search_ranks_and_never_indexes_questions() {
    let wiki = Wiki::new();
    for i in 0..12 {
        fs::write(
            wiki.0.join(format!("wiki/pages/knowledge/rank-{i}.md")),
            format!("---\ntitle: ranktoken {i}\ncreated: 2026-09-29\n---\nRanking details.\n"),
        )
        .unwrap();
    }
    let output = run(wiki.command(&["search", "ranktoken", "-n", "12"]), "");
    let paths: Vec<_> = output
        .lines()
        .filter(|line| line.starts_with('/'))
        .collect();
    assert_eq!(paths.len(), 12);
    for (rank, top, failure, diagnostic) in [
        (4, "3", true, "bm25: rank 4"),
        (4, "4", false, ""),
        (11, "3", true, "bm25: not in top 10"),
        (11, "11", false, ""),
    ] {
        let path = paths[rank - 1];
        let original = fs::read_to_string(path).unwrap();
        fs::write(
            path,
            original.replacen("---\n", "---\nqueries: [ranktoken]\n", 1),
        )
        .unwrap();
        let output = run_output(
            wiki.command(&["check", "--probe", "--probe-top", top, path]),
            "",
        );
        assert_eq!(output.status.code(), Some(i32::from(failure)), "{output:?}");
        assert!(String::from_utf8(output.stderr)
            .unwrap()
            .contains(diagnostic));
        fs::write(path, original).unwrap();
    }
    let path = wiki.0.join("wiki/pages/knowledge/foo.md");
    fs::write(&path, "---\ntitle: foo\ncreated: 2026-09-29\nqueries: [questiononlytoken]\n---\n# Recovery\nDetails.\n").unwrap();
    let output = run_output(wiki.command(&["check", "--probe", "knowledge/foo"]), "");
    assert_eq!(output.status.code(), Some(1));
    let db = rusqlite::Connection::open(wiki.0.join("wiki/index.sqlite")).unwrap();
    for table in ["fts", "sections_fts"] {
        let matches: i64 = db
            .query_row(
                &format!("select count(*) from {table} where {table} match 'questiononlytoken'"),
                [],
                |r| r.get(0),
            )
            .unwrap();
        assert_eq!(matches, 0, "{table}");
    }
    let listener = wiki.listener();
    run(wiki.command(&["check", "knowledge/foo"]), "");
    assert_no_connection(&listener);
    fs::write(&path, "---\ntitle: foo\ncreated: 2026-09-29\nqueries: [questiononlytoken]\n---\nghp_AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA\n").unwrap();
    let output = run_output(wiki.command(&["check", "--probe", "knowledge/foo"]), "");
    assert_eq!(output.status.code(), Some(2));
    assert_no_connection(&listener);
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
    fs::write(
        wiki.0.join("wiki/pages/knowledge/foo.md"),
        "---\ntitle: foo cache notes\ncreated: 2026-09-20\n---\nerror building bubblewrap command: cannot establish app-server socket mount isolation\n",
    ).unwrap();
    let recall_hook = json!({"session_id": "korean", "hook_event_name": "PostToolUseFailure", "tool_name": "Bash", "error": "error building bubblewrap command: cannot establish app-server socket mount isolation"}).to_string();
    let explanation: Value =
        serde_json::from_str(&run(wiki.command(&["recall", "--explain"]), &recall_hook)).unwrap();
    assert_eq!(explanation["selected"], json!(["pages/knowledge/foo.md"]));
    let prompt = "  캐시가 왜 사라져요?\n다시 시작하면 그래요.";
    let (output, request) = wiki.respond(
        &["remind", "--interval", "1"],
        &json!({"session_id": "korean", "prompt": prompt}).to_string(),
        json!({"ok": true, "tier": "cpu", "results": [{"path": "knowledge/foo", "score": 1.0}]}),
    );
    assert_eq!(request, json!({"op": "related", "prompt": prompt, "n": 3}));
    assert_eq!(output.lines().next().unwrap(), "claude-wiki: pages that may already cover this: knowledge/foo. Read one with `claude-wiki search <terms>` or the Read tool if it is relevant; otherwise ignore this note and do not reply to it.");
    assert!(output.contains("claude-wiki: 1 prompts since"));
    let recall = run(wiki.command(&["recall"]), &recall_hook);
    assert!(
        recall.is_empty(),
        "reminder suggestions must be seen by recall: {recall}"
    );
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
