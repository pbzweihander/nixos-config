use serde_json::{Value, json};
use std::{
    fs,
    io::{BufRead, BufReader, Write},
    os::unix::net::{UnixListener, UnixStream},
    path::PathBuf,
    process::{Command, Output, Stdio},
    sync::{
        atomic::{AtomicU64, Ordering},
        mpsc,
    },
    thread,
    time::{Duration, Instant},
};

fn accept(listener: &UnixListener) -> (UnixStream, std::os::unix::net::SocketAddr) {
    listener.set_nonblocking(true).unwrap();
    let deadline = Instant::now() + Duration::from_secs(3);
    loop {
        match listener.accept() {
            Ok((stream, address)) => {
                stream
                    .set_read_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                stream
                    .set_write_timeout(Some(Duration::from_secs(2)))
                    .unwrap();
                return (stream, address);
            }
            Err(e) if e.kind() == std::io::ErrorKind::WouldBlock && Instant::now() < deadline => {
                thread::sleep(Duration::from_millis(5))
            }
            Err(e) => panic!("fake daemon accept failed or timed out: {e}"),
        }
    }
}

struct Wiki(PathBuf);

#[test]
fn new_settings_override_legacy_settings_including_clap_and_empty_socket() {
    let wiki = Wiki::new();
    let mut command = wiki.command(&["search", "--json", "cache"]);
    command
        .env("CLAUDE_WIKI_DIR", wiki.0.join("unused"))
        .env("AGENT_WIKI_DIR", wiki.0.join("wiki"))
        .env("AGENT_WIKI_SOCKET", "");
    let value: Value = serde_json::from_str(&run(command, "")).unwrap();
    assert!(!value["hits"].as_array().unwrap().is_empty());
    assert!(!wiki.0.join("unused").exists());
    assert_eq!(value["dense"], "disabled");

    for (subcommand, suffix) in [
        ("context", "CONTEXT_BUDGET"),
        ("context", "OVERVIEW_BUDGET"),
        ("remind", "REMIND_INTERVAL"),
        ("remind", "REMIND_FAILURES"),
        ("recall", "RECALL_MIN_WINDOWS"),
        ("recall", "RECALL_MAX_SKILL_PAGES"),
        ("recall", "RECALL_MAX_PAGES"),
    ] {
        let old = format!("CLAUDE_WIKI_{suffix}");
        let new = format!("AGENT_WIKI_{suffix}");
        let mut command = wiki.command(&[subcommand]);
        command.env_remove(&new).env(&old, "invalid");
        let output = run_output(command, "{}");
        assert!(
            String::from_utf8_lossy(&output.stderr).contains("invalid"),
            "{suffix}"
        );
        let mut command = wiki.command(&[subcommand]);
        command
            .env(&new, "10000")
            .env(&old, "invalid")
            .env("AGENT_WIKI_SOCKET", "");
        let output = run_output(command, "{}");
        assert!(output.stderr.is_empty(), "{suffix}: {output:?}");
    }
}

#[test]
fn default_wiki_directory_uses_existing_legacy_data_until_migrated() {
    for legacy in [true, false] {
        let wiki = Wiki::new();
        let data = wiki.0.join("data");
        fs::create_dir(&data).unwrap();
        let old = data.join("claude-wiki");
        let new = data.join("agent-wiki");
        fs::rename(wiki.0.join("wiki"), if legacy { &old } else { &new }).unwrap();
        let mut command = wiki.command(&["list"]);
        command
            .env_remove("CLAUDE_WIKI_DIR")
            .env_remove("AGENT_WIKI_DIR")
            .env("XDG_DATA_HOME", &data)
            .env("XDG_STATE_HOME", wiki.0.join("default-state"));
        assert!(run(command, "").contains("foo cache notes"));
        assert!(!(if legacy { &new } else { &old }).exists());
        assert!(
            wiki.0
                .join("default-state")
                .join("agent-wiki")
                .join("index.sqlite")
                .exists()
        );
        // An overridden data home without a state home must stay isolated too.
        let mut command = wiki.command(&["list"]);
        command
            .env_remove("CLAUDE_WIKI_DIR")
            .env_remove("AGENT_WIKI_DIR")
            .env("XDG_DATA_HOME", &data)
            .env_remove("XDG_STATE_HOME")
            .env_remove("AGENT_WIKI_STATE_DIR")
            .env_remove("CLAUDE_WIKI_STATE_DIR");
        assert!(run(command, "").contains("foo cache notes"));
        assert!(
            data.join(if legacy {
                "claude-wiki-state"
            } else {
                "agent-wiki-state"
            })
            .join("index.sqlite")
            .exists()
        );
    }
}

#[test]
fn default_root_aliases_share_one_state_directory() {
    let wiki = Wiki::new();
    let data = wiki.0.join("data");
    fs::create_dir(&data).unwrap();
    let root = data.join("agent-wiki");
    fs::rename(wiki.0.join("wiki"), &root).unwrap();
    let alias = wiki.0.join("alias");
    std::os::unix::fs::symlink(&root, &alias).unwrap();
    for path in [".", alias.to_str().unwrap()] {
        let mut command = wiki.command(&["--dir", path, "list"]);
        command
            .current_dir(&root)
            .env("XDG_DATA_HOME", &data)
            .env("XDG_STATE_HOME", wiki.0.join("xdg-state"));
        assert!(run(command, "").contains("foo cache notes"));
        assert!(wiki.0.join("xdg-state/agent-wiki/index.sqlite").exists());
        assert!(!data.join("agent-wiki-state").exists());
    }
}

#[test]
fn large_failure_payload_is_processed_and_over_limit_input_is_quiet() {
    let wiki = Wiki::new();
    let mut input = json!({"session_id":"large", "hook_event_name":"PostToolUseFailure", "tool_name":"Bash", "tool_input":{"command":"cargo test"}, "error":"x".repeat(70_000)});
    let output = run_output(wiki.command(&["remind", "--event"]), &input.to_string());
    assert!(
        output.status.success() && output.stderr.is_empty(),
        "{output:?}"
    );
    let db = rusqlite::Connection::open(wiki.0.join("wiki-state/state.sqlite")).unwrap();
    assert_eq!(
        db.query_row(
            "select failures from remind_event_state where session_id='large'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        1
    );
    input["error"] = json!("x".repeat(1024 * 1024));
    // Feed through a regular file so an early reader close cannot make the test
    // writer fail with BrokenPipe before checking the hook's quiet fail-open.
    let payload = wiki.0.join("oversized.json");
    fs::write(&payload, input.to_string()).unwrap();
    for args in [
        ["remind", "--event"],
        ["recall", "--hook"],
        ["remind", "--hook"],
    ] {
        let output = wiki
            .command(&args)
            .stdin(fs::File::open(&payload).unwrap())
            .output()
            .unwrap();
        assert!(
            output.status.success() && output.stdout.is_empty() && output.stderr.is_empty(),
            "{output:?}"
        );
    }
}

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
        assert!(
            Command::new("git")
                .arg("-C")
                .arg(wiki.0.join("wiki"))
                .args(["add", "pages"])
                .status()
                .unwrap()
                .success()
        );
        assert!(
            Command::new("git")
                .arg("-C")
                .arg(wiki.0.join("wiki"))
                .args([
                    "-c",
                    "user.name=test",
                    "-c",
                    "user.email=test@example.com",
                    "-c",
                    "commit.gpgsign=false",
                    "commit",
                    "-qm",
                    "fixture"
                ])
                .status()
                .unwrap()
                .success()
        );
        wiki
    }

    fn command(&self, args: &[&str]) -> Command {
        let mut command = Command::new(env!("CARGO_BIN_EXE_agent-wiki"));
        crate::hermetic(&mut command)
            .args(args)
            .env("CLAUDE_WIKI_DIR", self.0.join("wiki"))
            .env("CLAUDE_WIKI_SESSIONS_DIRS", self.0.join("transcripts"))
            .env(
                "CLAUDE_WIKI_CODEX_SESSIONS_DIRS",
                self.0.join("codex-transcripts"),
            )
            .env(
                "CLAUDE_WIKI_ANTIGRAVITY_SESSIONS_DIRS",
                self.0.join("antigravity-transcripts"),
            )
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
        self.respond_bytes(args, input, format!("{reply}\n").into_bytes())
    }

    fn respond_bytes(&self, args: &[&str], input: &str, reply: Vec<u8>) -> (Output, Value) {
        self.respond_command(self.command(args), input, reply)
    }

    fn respond_command(&self, command: Command, input: &str, reply: Vec<u8>) -> (Output, Value) {
        // Each exchange binds afresh; a socket file left by an earlier one is stale.
        let _ = fs::remove_file(self.0.join("embed.sock"));
        let listener = self.listener();
        let server = thread::spawn(move || {
            let (mut stream, _) = accept(&listener);
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut request = String::new();
            BufReader::new(&mut stream).read_line(&mut request).unwrap();
            // Oversized replies can be rejected before the peer finishes writing.
            let _ = stream.write_all(&reply);
            serde_json::from_str(&request).unwrap()
        });
        let output = run_output(command, input);
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

fn assert_dense_unavailable(output: Output, expected: &str, json: bool, reason: &str) {
    assert!(output.status.success(), "{output:?}");
    if json {
        assert!(output.stderr.is_empty(), "{output:?}");
        let mut expected: Value = serde_json::from_str(expected).unwrap();
        expected["dense"] = "unavailable".into();
        expected["dense_error"] = reason.into();
        assert_eq!(
            serde_json::from_slice::<Value>(&output.stdout).unwrap(),
            expected
        );
    } else {
        assert_eq!(output.stdout, expected.as_bytes());
        assert_eq!(
            String::from_utf8(output.stderr).unwrap(),
            format!("note: dense retrieval unavailable ({reason}); results are BM25 only\n")
        );
    }
}

#[test]
fn related_json_requests_ten_filters_and_logs_selected_pages() {
    let wiki = Wiki::new();
    for (path, text) in [
        (
            "history/past",
            "---\ntitle: Past\ncreated: 2020-01-01\n---\npast",
        ),
        (
            "knowledge/blocked",
            "---\ntitle: Hidden\ncreated: 2020-01-01\n---\nghp_AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA",
        ),
        (
            "projects/wiki",
            "---\ntitle: Overview\ncreated: 2020-01-01\n---\noverview",
        ),
    ] {
        fs::write(wiki.0.join(format!("wiki/pages/{path}.md")), text).unwrap();
    }
    let cwd = wiki.0.join("wiki");
    let (output, request) = wiki.respond(
        &[
            "related",
            "--json",
            "-n",
            "1",
            "--cwd",
            cwd.to_str().unwrap(),
            "--session",
            "s",
            "--source",
            "mod-agent",
        ],
        "Why does the cache disappear?",
        json!({"ok":true,"results":[
            {"path":"history/past","score":1}, {"path":"knowledge/blocked","score":1},
            {"path":"knowledge/deleted","score":1},
            {"path":"knowledge/foo","score":1,"heading":"","start_line":1,"end_line":7},
            {"path":"knowledge/bar","score":1}
        ]}),
    );
    assert_eq!(
        request,
        json!({"op":"related","prompt":"Why does the cache disappear?","n":10})
    );
    assert_eq!(
        serde_json::from_str::<Value>(&output).unwrap(),
        json!({
            "pages":[{"path":cwd.join("pages/knowledge/foo.md"), "rel":"pages/knowledge/foo.md", "title":"foo cache notes", "type":"knowledge", "section":{"heading":"","start":1,"end":7}}],
            "overview":cwd.join("pages/projects/wiki.md")
        })
    );
    let output = run(
        wiki.command(&["injections", "--json", "--session", "s"]),
        "",
    );
    let report: Value = serde_json::from_str(&output).unwrap();
    let pages = report["pages"].as_array().unwrap();
    assert_eq!(pages.len(), 1);
    assert_eq!(pages[0]["rel"], "pages/knowledge/foo.md");
    assert_eq!(pages[0]["sources"], json!(["mod-agent"]));
    assert!(pages[0]["at"].as_i64().unwrap() > 0);
    assert_eq!(
        run(wiki.command(&["injections", "--session", "s"]), ""),
        format!(
            "{} (foo cache notes) [mod-agent]\n",
            cwd.join("pages/knowledge/foo.md").display()
        )
    );
    fs::remove_file(wiki.0.join("embed.sock")).unwrap();
    let (output, _) = wiki.respond(
        &["related", "--cwd", cwd.to_str().unwrap()],
        "cache",
        json!({"ok":true,"results":[{"path":"knowledge/bar","score":1}]}),
    );
    assert_eq!(
        output,
        format!(
            "{} (bar cache notes)\noverview: {}\n",
            cwd.join("pages/knowledge/bar.md").display(),
            cwd.join("pages/projects/wiki.md").display()
        )
    );
}

#[test]
fn related_timeout_is_separate_from_hook_timeout_and_can_be_overridden() {
    for timeout in [None, Some("75")] {
        let wiki = Wiki::new();
        let listener = wiki.listener();
        let (release, wait) = mpsc::channel();
        let server = thread::spawn(move || {
            let (mut stream, _) = accept(&listener);
            let mut request = String::new();
            BufReader::new(&mut stream).read_line(&mut request).unwrap();
            let _ = wait.recv_timeout(Duration::from_millis(400));
            let _ = writeln!(
                stream,
                "{}",
                json!({"ok":true,"results":[{"path":"knowledge/foo","score":1}]})
            );
        });
        let mut command = wiki.command(&["related", "--json", "--cwd", "/"]);
        command.env("CLAUDE_WIKI_SEMANTIC_TIMEOUT_MS", "1");
        if let Some(timeout) = timeout {
            command.args(["--timeout-ms", timeout]);
        }
        let start = Instant::now();
        let output = run(command, "cache");
        let _ = release.send(());
        server.join().unwrap();
        let report: Value = serde_json::from_str(&output).unwrap();
        assert_eq!(
            report["pages"].as_array().unwrap().len(),
            usize::from(timeout.is_none())
        );
        assert!(start.elapsed() < Duration::from_secs(2));
    }
}

#[test]
fn json_search_fuses_keyword_and_dense_results() {
    let wiki = Wiki::new();
    fs::write(
        wiki.0.join("wiki/pages/knowledge/lexical.md"),
        "---\ntitle: Other notes\ncreated: 2026-09-20\n---\nfoo\n",
    )
    .unwrap();
    let (output, request) = wiki.respond(
        &["search", "foo", "--json"],
        "",
        json!({"ok":true,"results":[
            {"path":"knowledge/bar","score":0.9},
            {"path":"knowledge/foo","score":0.1},
            {"path":"knowledge/foo","score":0.05},
            {"path":"knowledge/missing","score":0.01}
        ]}),
    );
    assert_eq!(request, json!({"op":"dense","query":"foo","n":40}));
    let report: Value = serde_json::from_str(&output).unwrap();
    assert_eq!(report["dense"], "ok");
    assert_eq!(report["lexical"], true);
    assert!(report.get("dense_error").is_none());
    let hits = report["hits"].as_array().unwrap();
    assert_eq!(hits.len(), 3);
    assert_eq!(
        hits.iter()
            .map(|hit| hit["via"].as_str().unwrap())
            .collect::<Vec<_>>(),
        ["both", "bm25", "dense"]
    );
    assert_eq!(hits[0]["rel"], "pages/knowledge/foo.md");
    assert_eq!(hits[0]["semantic"], false);
    assert_eq!(hits[0]["tags"], json!(["cache"]));
    assert!(hits[0]["snippet"].as_str().unwrap().contains("[foo]"));
    assert_eq!(hits[2]["semantic"], true);
    assert_eq!(hits[2]["snippet"], "");
}

#[test]
fn dense_only_rows_apply_each_filter_and_redact_blocked_pages() {
    for flags in [
        vec![],
        vec!["--type", "followups"],
        vec!["--status", "open"],
        vec!["--project", "target"],
        vec!["--tag", "unique"],
        vec![
            "--type",
            "followups",
            "--status",
            "open",
            "--project",
            "target",
            "--tag",
            "unique",
        ],
    ] {
        let wiki = Wiki::new();
        for (name, body) in [
            ("match", "Recovery"),
            ("blocked", "ghp_AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA"),
        ] {
            fs::write(wiki.0.join(format!("wiki/pages/followups/{name}.md")), format!(
                "---\ntitle: Hidden {name}\nstatus: open\nproject: target\ntags: [unique]\ncreated: 2026-09-20\n---\n# Recovery\n{body}\n"
            )).unwrap();
        }
        // A substring tag must not match the whole-tag filter.
        fs::write(wiki.0.join("wiki/pages/knowledge/bar.md"),
            "---\ntitle: bar\nproject: demo\ntags: [nonunique]\ncreated: 2026-09-20\n---\nOther details.\n").unwrap();
        let mut args = vec!["search", "nomatch", "--json"];
        args.extend(&flags);
        let (output, request) = wiki.respond(&args, "", json!({"ok":true,"results":[
            {"path":"knowledge/foo","score":0.9},
            {"path":"knowledge/bar","score":0.8},
            {"path":"followups/missing","score":0.7},
            {"path":"followups/match","score":0.6,"heading":"Recovery","start_line":8,"end_line":9},
            {"path":"followups/blocked","score":0.5,"heading":"Recovery","start_line":8,"end_line":9}
        ]}));
        assert_eq!(request, json!({"op":"dense","query":"nomatch","n":40}));
        let report: Value = serde_json::from_str(&output).unwrap();
        let hits = report["hits"].as_array().unwrap();
        assert_eq!(hits.len(), if flags.is_empty() { 4 } else { 2 }, "{output}");
        assert!(
            hits.iter()
                .all(|hit| hit["via"] == "dense" && hit["semantic"] == true)
        );
        let matched = &hits[hits.len() - 2];
        assert_eq!(matched["rel"], "pages/followups/match.md");
        assert_eq!(
            matched["section"],
            json!({"heading":"Recovery","start":8,"end":9})
        );
        let blocked = hits.last().unwrap();
        assert_eq!(blocked["title"], "[BLOCKED: secret]");
        assert_eq!(blocked["snippet"], "");
        assert!(blocked["section"].is_null());
        assert_eq!(blocked["status"], "");
        assert_eq!(blocked["project"], "");
        assert_eq!(blocked["tags"], json!([]));
        assert!(!output.contains("Hidden blocked"));
    }
}

#[test]
fn probe_uses_dense_ranks_even_when_keywords_match() {
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
            "fused: rank 2, bm25: not in top 40, dense: rank 2",
        ),
        (
            "캐시가 왜 사라져요?",
            vec!["knowledge/bar"],
            "3",
            true,
            "dense: not in top 40",
        ),
        (
            "캐시가 왜 사라져요?",
            vec![],
            "3",
            true,
            "dense: no results",
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
        assert_eq!(request, json!({"op": "dense", "query": query, "n": 40}));
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
fn probe_accepts_fused_bm25_or_dense_independently() {
    for method in ["fused", "bm25", "dense"] {
        let wiki = Wiki::new();
        fs::write(
            wiki.0.join("wiki/pages/history/past.md"),
            "---\ntitle: Past\ncreated: 2026-09-20\n---\nOld details.\n",
        )
        .unwrap();
        let baseline: Value = serde_json::from_str(&run(
            wiki.command(&["search", "cache", "--no-semantic", "--json"]),
            "",
        ))
        .unwrap();
        let first = baseline["hits"][0]["rel"].as_str().unwrap();
        let second = baseline["hits"][1]["rel"].as_str().unwrap();
        let (target, paths) = match method {
            "fused" => (second, vec!["pages/history/past.md", second]),
            "bm25" => (first, vec![second]),
            _ => ("pages/history/past.md", vec!["pages/history/past.md"]),
        };
        let path = wiki.0.join("wiki").join(target);
        let original = fs::read_to_string(&path).unwrap();
        fs::write(
            &path,
            original.replacen("---\n", "---\nqueries: [cache]\n", 1),
        )
        .unwrap();
        let results: Vec<_> = paths.iter().map(|path| json!({
            "path": path.strip_prefix("pages/").unwrap().strip_suffix(".md").unwrap(), "score": 0.1
        })).collect();
        let reply = json!({"ok":true,"results":results});
        let (output, _) =
            wiki.respond(&["search", "cache", "--json", "-n", "1"], "", reply.clone());
        let report: Value = serde_json::from_str(&output).unwrap();
        assert_eq!(report["hits"][0]["rel"] == target, method == "fused");
        fs::remove_file(wiki.0.join("embed.sock")).unwrap();
        let (output, request) =
            wiki.respond_output(&["check", "--probe", "--probe-top", "1", target], "", reply);
        assert_eq!(request, json!({"op":"dense","query":"cache","n":40}));
        assert!(output.status.success(), "{method}: {output:?}");
        assert!(
            String::from_utf8(output.stderr)
                .unwrap()
                .contains("1 queries, 0 failures")
        );
    }
}

#[test]
fn probe_reports_disabled_and_unavailable_dense_requests() {
    for state in ["disabled", "down", "error"] {
        let wiki = Wiki::new();
        fs::write(
            wiki.0.join("wiki/pages/knowledge/foo.md"),
            "---\ntitle: foo\ncreated: 2026-09-29\nqueries: [unmatchedmeaning]\n---\nRecovery details.\n",
        )
        .unwrap();
        let args = ["check", "--probe", "knowledge/foo"];
        let (output, diagnostic) = match state {
            "disabled" => {
                let mut command = wiki.command(&args);
                command.env("CLAUDE_WIKI_SOCKET", "");
                (run_output(command, ""), "disabled".to_owned())
            }
            "down" => (
                run_output(wiki.command(&args), ""),
                format!(
                    "unavailable (cannot connect to {})",
                    wiki.0.join("embed.sock").display()
                ),
            ),
            _ => (
                wiki.respond_output(&args, "", json!({"ok": false, "error": "warming up"}))
                    .0,
                "unavailable (daemon error: warming up)".to_owned(),
            ),
        };
        assert_eq!(output.status.code(), Some(1));
        assert_eq!(
            String::from_utf8(output.stderr).unwrap(),
            format!(
                "warning: pages/knowledge/foo.md: query \"unmatchedmeaning\" does not find this page (fused: absent, bm25: not in top 40, dense: {diagnostic})\nprobe: 1 pages probed, 1 queries, 1 failures\n"
            )
        );
    }
}

#[test]
fn search_uses_forty_candidates_and_preserves_large_bm25_fallbacks() {
    let wiki = Wiki::new();
    for i in 0..60 {
        fs::write(
            wiki.0.join(format!("wiki/pages/knowledge/rank-{i}.md")),
            format!("---\ntitle: ranktoken {i}\ncreated: 2026-09-20\n---\nRanking details.\n"),
        )
        .unwrap();
    }
    let report: Value = serde_json::from_str(&run(
        wiki.command(&["search", "ranktoken", "--no-semantic", "--json", "-n", "60"]),
        "",
    ))
    .unwrap();
    let paths: Vec<_> = report["hits"]
        .as_array()
        .unwrap()
        .iter()
        .map(|hit| hit["rel"].as_str().unwrap())
        .collect();
    assert_eq!(paths.len(), 60);
    for n in ["50", "-1"] {
        let mut args = wiki.command(&["search", "ranktoken", "--json"]);
        args.arg(format!("-n{n}"));
        let fallback: Value = serde_json::from_str(&run(args, "")).unwrap();
        assert_eq!(
            fallback["hits"].as_array().unwrap().len(),
            if n == "50" { 50 } else { 60 }
        );
    }
    let path = paths[39]
        .strip_prefix("pages/")
        .unwrap()
        .strip_suffix(".md")
        .unwrap();
    let (output, request) = wiki.respond(
        &["search", "ranktoken", "--json", "-n", "1"],
        "",
        json!({"ok":true,"results":[{"path":path,"score":0.01}]}),
    );
    assert_eq!(request["n"], 40);
    let fused: Value = serde_json::from_str(&output).unwrap();
    assert_eq!(fused["hits"][0]["rel"], paths[39]);
    assert_eq!(fused["hits"][0]["via"], "both");
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
    let output = run(
        wiki.command(&["search", "ranktoken", "-n", "12", "--no-semantic"]),
        "",
    );
    let paths: Vec<_> = output
        .lines()
        .filter(|line| line.starts_with('/'))
        .collect();
    assert_eq!(paths.len(), 12);
    for (rank, top, failure, diagnostic) in [
        (4, "3", true, "bm25: rank 4"),
        (4, "4", false, ""),
        (11, "3", true, "bm25: rank 11"),
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
        assert!(
            String::from_utf8(output.stderr)
                .unwrap()
                .contains(diagnostic)
        );
        fs::write(path, original).unwrap();
    }
    let path = wiki.0.join("wiki/pages/knowledge/foo.md");
    fs::write(&path, "---\ntitle: foo\ncreated: 2026-09-29\nqueries: [questiononlytoken]\n---\n# Recovery\nDetails.\n").unwrap();
    let output = run_output(wiki.command(&["check", "--probe", "knowledge/foo"]), "");
    assert_eq!(output.status.code(), Some(1));
    let db = rusqlite::Connection::open(wiki.0.join("wiki-state/index.sqlite")).unwrap();
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
fn opt_out_empty_or_unset_socket_skip_daemon() {
    let wiki = Wiki::new();
    let listener = wiki.listener();
    for json in [false, true] {
        for query in ["cache", "unmatchedmeaning"] {
            let mut args = vec!["search", query];
            if json {
                args.push("--json");
            }
            let mut command = wiki.command(&args);
            command.arg("--no-semantic");
            let expected = run(command, "");
            if json {
                let report: Value = serde_json::from_str(&expected).unwrap();
                assert_eq!(report["dense"], "disabled");
                assert!(report.get("dense_error").is_none());
            }
            for disabled in ["empty socket", "unset runtime", "empty runtime"] {
                let mut command = wiki.command(&args);
                match disabled {
                    "empty socket" => command.env("CLAUDE_WIKI_SOCKET", ""),
                    "unset runtime" => command
                        .env_remove("CLAUDE_WIKI_SOCKET")
                        .env_remove("XDG_RUNTIME_DIR"),
                    _ => command
                        .env_remove("CLAUDE_WIKI_SOCKET")
                        .env("XDG_RUNTIME_DIR", ""),
                };
                assert_eq!(run(command, ""), expected);
            }
            assert_no_connection(&listener);
        }
    }
}

#[test]
fn dense_search_renders_pages_and_supplied_section() {
    let wiki = Wiki::new();
    // The daemon reads sections from this index; give foo a real heading section.
    fs::write(
        wiki.0.join("wiki/pages/knowledge/foo.md"),
        "---\ntitle: foo cache notes\ntags: [cache]\nproject: demo\ncreated: 2026-09-20\n---\n# What kills it\nRecovery details.\n",
    )
    .unwrap();
    let (output, request) = wiki.respond_output(
        &["search", "unmatchedmeaning", "anotherquery", "-n", "1"],
        "",
        json!({"ok": true, "tier": "gpu", "results": [
            {"path": "knowledge/deleted", "score": 2.0},
            {"path": "knowledge/foo", "score": 1.23, "heading": "What kills it", "start_line": 7, "end_line": 8},
            {"path": "knowledge/bar", "score": 0.9}
        ]}),
    );
    assert_eq!(
        request,
        json!({"op": "dense", "query": "unmatchedmeaning anotherquery", "n": 40})
    );
    assert!(output.status.success(), "{output:?}");
    assert_eq!(
        String::from_utf8(output.stderr).unwrap(),
        "note: no keyword match; these results match by meaning only. If none fits, try agent-wiki sessions <terms>.\n"
    );
    let stdout = String::from_utf8(output.stdout).unwrap();
    let lines: Vec<_> = stdout.lines().collect();
    assert_eq!(
        lines[0],
        format!(
            "{} (meaning)",
            wiki.0.join("wiki/pages/knowledge/foo.md").display()
        )
    );
    assert!(lines[1].starts_with("  foo cache notes  (knowledge; updated "));
    assert!(lines[1].ends_with("; project=demo; tags=cache)"));
    assert_eq!(lines[2], "  § What kills it (lines 7-8)");
    assert_eq!(lines.len(), 3);
}

#[test]
fn stale_daemon_sections_are_dropped_but_pages_kept() {
    let wiki = Wiki::new();
    fs::write(
        wiki.0.join("wiki/pages/knowledge/foo.md"),
        "---\ntitle: foo cache notes\ntags: [cache]\nproject: demo\ncreated: 2026-09-20\n---\n# Current\nRecovery details.\n",
    )
    .unwrap();
    // A snapshot older than the page: a removed heading, lines past the end, and
    // a section moved by an edit. A malformed id is never resolved to a file.
    for (heading, start, end) in [
        ("Removed heading", 7, 8),
        ("Current", 900, 999),
        ("Current", 6, 8),
    ] {
        let reply = json!({"ok": true, "results": [
            {"path": "../../outside", "score": 3.0},
            {"path": "knowledge/foo", "score": 1.0, "heading": heading, "start_line": start, "end_line": end}
        ]});
        let (output, _) =
            wiki.respond(&["search", "unmatchedmeaning", "--json"], "", reply.clone());
        let report: Value = serde_json::from_str(&output).unwrap();
        let hits = report["hits"].as_array().unwrap();
        assert_eq!(hits.len(), 1, "{output}");
        assert_eq!(hits[0]["rel"], "pages/knowledge/foo.md");
        assert!(hits[0]["section"].is_null(), "{output}");
        fs::remove_file(wiki.0.join("embed.sock")).unwrap();

        let (output, _) = wiki.respond_output(&["search", "unmatchedmeaning"], "", reply.clone());
        assert!(output.status.success(), "{output:?}");
        let text = String::from_utf8(output.stdout).unwrap();
        assert!(text.contains("foo cache notes"), "{text}");
        assert!(!text.contains('§'), "{text}");
        assert!(!text.contains(heading), "{text}");
        fs::remove_file(wiki.0.join("embed.sock")).unwrap();

        let (output, _) = wiki.respond(&["related", "--json", "--cwd", "/"], "cache", reply);
        let report: Value = serde_json::from_str(&output).unwrap();
        assert_eq!(report["pages"].as_array().unwrap().len(), 1, "{output}");
        assert_eq!(report["pages"][0]["rel"], "pages/knowledge/foo.md");
        assert!(report["pages"][0]["section"].is_null(), "{output}");
        fs::remove_file(wiki.0.join("embed.sock")).unwrap();
    }
}

#[test]
fn blocked_json_hits_hide_status_project_and_tags_in_every_mode() {
    let wiki = Wiki::new();
    fs::create_dir_all(wiki.0.join("wiki/pages/followups")).unwrap();
    // The body blocks the page; its metadata must not escape through JSON.
    fs::write(
        wiki.0.join("wiki/pages/followups/leaky.md"),
        "---\ntitle: Leaky title\nstatus: open\nproject: leakyproject\ntags: [leakytag, secondleak]\ncreated: 2026-09-20\n---\nmarker ghp_AAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAAA\n",
    )
    .unwrap();
    let check = |output: &str, via: &str| {
        let report: Value = serde_json::from_str(output).unwrap();
        let hits = report["hits"].as_array().unwrap();
        let hit = hits
            .iter()
            .find(|hit| hit["rel"] == "pages/followups/leaky.md")
            .unwrap_or_else(|| panic!("{output}"));
        assert_eq!(hit["via"], via, "{output}");
        assert_eq!(hit["title"], "[BLOCKED: secret]");
        assert_eq!(hit["status"], "");
        assert_eq!(hit["project"], "");
        assert_eq!(hit["tags"], json!([]));
        assert_eq!(hit["snippet"], "");
        assert!(hit["section"].is_null());
        for hidden in [
            "Leaky title",
            "leakyproject",
            "leakytag",
            "secondleak",
            "ghp_",
        ] {
            assert!(!output.contains(hidden), "{hidden}: {output}");
        }
    };
    // BM25 only.
    check(
        &run(
            wiki.command(&["search", "marker", "--json", "--no-semantic"]),
            "",
        ),
        "bm25",
    );
    let dense = json!({"ok": true, "results": [
        {"path": "followups/leaky", "score": 1.0, "heading": "", "start_line": 1, "end_line": 8}
    ]});
    // Dense only: no keyword matches.
    let (output, _) = wiki.respond(&["search", "unmatchedmeaning", "--json"], "", dense.clone());
    check(&output, "dense");
    fs::remove_file(wiki.0.join("embed.sock")).unwrap();
    // Fused: both retrievers return the page.
    let (output, _) = wiki.respond(&["search", "marker", "--json"], "", dense);
    check(&output, "both");
}

#[test]
fn related_returns_promptly_when_stdin_stays_open() {
    let wiki = Wiki::new();
    // No daemon listens: only the stdin wait could hold the command.
    let mut child = wiki
        .command(&["related", "--json", "--timeout-ms", "20", "--cwd", "/"])
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let mut stdin = child.stdin.take().unwrap();
    stdin.write_all(b"cache ").unwrap();
    stdin.flush().unwrap();
    let start = Instant::now();
    let deadline = start + Duration::from_secs(3);
    let status = loop {
        if let Some(status) = child.try_wait().unwrap() {
            break status;
        }
        if Instant::now() > deadline {
            let _ = child.kill();
            panic!("related still waits for stdin after {:?}", start.elapsed());
        }
        thread::sleep(Duration::from_millis(5));
    };
    // The writer is still open: the command finished on its stdin deadline.
    let elapsed = start.elapsed();
    drop(stdin);
    let output = child.wait_with_output().unwrap();
    assert!(status.success(), "{output:?}");
    assert!(elapsed < Duration::from_millis(1500), "{elapsed:?}");
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(report, json!({"pages": [], "overview": null}));
}

#[test]
fn daemon_down_error_and_timeout_preserve_bm25_output() {
    for json in [false, true] {
        for query in ["cache", "unmatchedmeaning"] {
            let wiki = Wiki::new();
            let mut args = vec!["search", query];
            if json {
                args.push("--json");
            }
            let mut command = wiki.command(&args);
            command.arg("--no-semantic");
            let expected = run(command, "");
            let connection_error =
                format!("cannot connect to {}", wiki.0.join("embed.sock").display());
            assert_dense_unavailable(
                run_output(wiki.command(&args), ""),
                &expected,
                json,
                &connection_error,
            );
            let (output, request) =
                wiki.respond_output(&args, "", json!({"ok":false,"error":"unknown operation"}));
            assert_eq!(request["op"], "dense");
            assert_dense_unavailable(output, &expected, json, "daemon error: unknown operation");
            assert_dense_unavailable(
                run_output(wiki.command(&args), ""),
                &expected,
                json,
                &connection_error,
            );
            fs::remove_file(wiki.0.join("embed.sock")).unwrap();
            let listener = wiki.listener();
            let (release, wait) = mpsc::channel();
            let server = thread::spawn(move || {
                let (mut stream, _) = accept(&listener);
                let mut request = String::new();
                BufReader::new(&mut stream).read_line(&mut request).unwrap();
                assert_eq!(
                    serde_json::from_str::<Value>(&request).unwrap()["op"],
                    "dense"
                );
                let _ = wait.recv_timeout(Duration::from_secs(2));
            });
            let mut command = wiki.command(&args);
            command.env("CLAUDE_WIKI_SEMANTIC_TIMEOUT_MS", "50");
            let start = Instant::now();
            let output = run_output(command, "");
            let elapsed = start.elapsed();
            let _ = release.send(());
            server.join().unwrap();
            assert_dense_unavailable(output, &expected, json, "timed out after 50 ms");
            assert!(elapsed < Duration::from_secs(1));
        }
    }
}

#[test]
fn empty_dense_answer_is_ok_and_preserves_bm25_output() {
    for json in [false, true] {
        for query in ["cache", "unmatchedmeaning"] {
            let wiki = Wiki::new();
            let mut args = vec!["search", query];
            if json {
                args.push("--json");
            }
            let mut command = wiki.command(&args);
            command.arg("--no-semantic");
            let expected = run(command, "");
            let (output, _) = wiki.respond(&args, "", json!({"ok":true,"results":[]}));
            if json {
                let mut expected: Value = serde_json::from_str(&expected).unwrap();
                expected["dense"] = "ok".into();
                assert_eq!(serde_json::from_str::<Value>(&output).unwrap(), expected);
            } else {
                assert_eq!(output, expected);
            }
        }
    }
}

#[test]
fn invalid_or_oversized_dense_responses_report_unavailable() {
    for json in [false, true] {
        for (reply, reason) in [
            (b"not json\n".to_vec(), "invalid response"),
            (b"{\"ok\":true,\"results\":\"bad type\"}\n".to_vec(), "invalid response"),
            (b"{\"ok\":true}\n".to_vec(), "invalid response"),
            (b"{\"ok\":true,\"results\":[{\"path\":\"knowledge/foo\",\"score\":1,\"heading\":\"incomplete\"}]}\n".to_vec(), "invalid response"),
            (b"{\"ok\":true,\"results\":[]}".to_vec(), "invalid response (incomplete line)"),
            (vec![b'x'; 1024 * 1024 + 1], "oversized response"),
            (b"{\"ok\":false}\n".to_vec(), "daemon error"),
        ] {
            let wiki = Wiki::new();
            let mut args = vec!["search", "cache"];
            if json {
                args.push("--json");
            }
            let mut command = wiki.command(&args);
            command.arg("--no-semantic");
            let expected = run(command, "");
            let (output, _) = wiki.respond_bytes(&args, "", reply);
            assert_dense_unavailable(output, &expected, json, reason);
        }
    }
}

#[test]
fn daemon_error_is_bounded_and_stays_on_one_line() {
    for json in [false, true] {
        let wiki = Wiki::new();
        let mut args = vec!["search", "cache"];
        if json {
            args.push("--json");
        }
        let mut command = wiki.command(&args);
        command.arg("--no-semantic");
        let expected = run(command, "");
        let (output, _) = wiki.respond_output(
            &args,
            "",
            json!({"ok":false,"error":format!("warming\nup\r\t{}", "캐".repeat(250))}),
        );
        assert_dense_unavailable(
            output,
            &expected,
            json,
            &format!("daemon error: warming up  {}", "캐".repeat(188)),
        );
    }
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
    assert_eq!(request, json!({"op": "related", "prompt": prompt, "n": 10}));
    assert_eq!(
        output.lines().next().unwrap(),
        format!(
            "agent-wiki: pages that may already cover this: {} (foo cache notes). Read one if it bears on the request; otherwise ignore this note and do not reply to it.",
            wiki.0.join("wiki/pages/knowledge/foo.md").display()
        )
    );
    assert!(output.contains("agent-wiki: 1 prompts since"));
    let recall = run(wiki.command(&["recall"]), &recall_hook);
    assert!(
        recall.is_empty(),
        "reminder suggestions must be seen by recall: {recall}"
    );
}

#[test]
fn reminder_with_curated_summary_reuses_the_final_period() {
    let wiki = Wiki::new();
    let path = wiki.0.join("wiki/pages/knowledge/foo.md");
    fs::write(
        &path,
        "---\ntitle: Cache recovery\ncreated: 2026-09-20\nsummary: 캐시를 다시 만든다.\n---\n본문\n",
    )
    .unwrap();
    let (output, _) = wiki.respond(
        &["remind"],
        &json!({"session_id": "summary", "prompt": "캐시 복구"}).to_string(),
        json!({"ok":true,"results":[{"path":"knowledge/foo","score":1.0}]}),
    );
    assert_eq!(
        output,
        format!(
            "agent-wiki: pages that may already cover this: {} (Cache recovery) — 캐시를 다시 만든다. Read one if it bears on the request; otherwise ignore this note and do not reply to it.\n",
            path.display()
        ),
    );
}

#[test]
fn orca_bot_sessions_keep_suggestions_but_skip_the_interval_nudge() {
    let wiki = Wiki::new();
    let bot = wiki.0.join(".orca/bots/bot-1");
    fs::create_dir_all(&bot).unwrap();
    let reply =
        json!({"ok": true, "tier": "cpu", "results": [{"path": "knowledge/foo", "score": 1.0}]});
    let suggestion = format!(
        "agent-wiki: pages that may already cover this: {}",
        wiki.0.join("wiki/pages/knowledge/foo.md").display()
    );
    for (session, by_env) in [("bot-env", true), ("bot-cwd", false)] {
        let mut command = wiki.command(&["remind", "--interval", "1"]);
        command.env("HOME", &wiki.0);
        let mut hook = json!({"session_id": session, "prompt": "cache"});
        if by_env {
            command.env("ORCA_BOT_HOME", &bot);
        } else {
            command.env_remove("ORCA_BOT_HOME");
            hook["cwd"] = json!(bot);
        }
        let (output, _) = wiki.respond_command(
            command,
            &hook.to_string(),
            format!("{reply}\n").into_bytes(),
        );
        assert!(output.status.success(), "{output:?}");
        let text = String::from_utf8(output.stdout).unwrap();
        assert!(text.contains(&suggestion), "{text}");
        assert!(!text.contains("prompts since"), "{text}");
    }
    // The same prompt outside a bot home still gets the nudge.
    let mut command = wiki.command(&["remind", "--interval", "1"]);
    command.env("HOME", &wiki.0).env_remove("ORCA_BOT_HOME");
    let (output, _) = wiki.respond_command(
        command,
        &json!({"session_id": "plain", "prompt": "cache", "cwd": wiki.0}).to_string(),
        format!("{reply}\n").into_bytes(),
    );
    let text = String::from_utf8(output.stdout).unwrap();
    assert!(text.contains(&suggestion), "{text}");
    assert!(text.contains("agent-wiki: 1 prompts since"), "{text}");
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
    assert!(output.starts_with("agent-wiki: 2 prompts since"));
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
    fs::create_dir(wiki.0.join("agent-wiki")).unwrap();
    let listener = UnixListener::bind(wiki.0.join("agent-wiki/embed.sock")).unwrap();
    let server = thread::spawn(move || {
        let (mut stream, _) = accept(&listener);
        stream
            .set_read_timeout(Some(Duration::from_secs(2)))
            .unwrap();
        let mut request = String::new();
        BufReader::new(&mut stream).read_line(&mut request).unwrap();
        writeln!(stream, "{}", json!({"ok": true, "results": []})).unwrap();
        assert_eq!(
            serde_json::from_str::<Value>(&request).unwrap()["op"],
            "dense"
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
        let (_stream, _) = accept(&listener);
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
    assert!(output.starts_with("agent-wiki: 1 prompts since"));
    assert_eq!(output.lines().count(), 1);
}

#[test]
fn reminder_and_recall_log_only_emitted_pages_and_share_seen_state() {
    let wiki = Wiki::new();
    for name in ["foo", "bar"] {
        let path = wiki.0.join(format!("wiki/pages/knowledge/{name}.md"));
        fs::write(
            &path,
            fs::read_to_string(&path)
                .unwrap()
                .replace("project: demo\n", ""),
        )
        .unwrap();
    }
    let hook = json!({"session_id":"shared", "hook_event_name":"PreToolUse", "tool_name":"Skill", "tool_input":{"skill":"cache"}}).to_string();
    let recall = run(wiki.command(&["recall"]), &hook);
    assert!(recall.contains("foo cache notes"));
    let (output, _) = wiki.respond(&["remind"], &json!({"session_id":"shared", "prompt":"cache"}).to_string(),
        json!({"ok":true, "results":[{"path":"knowledge/foo", "score":1.0}, {"path":"knowledge/bar", "score":0.9}]}));
    assert!(output.is_empty());
    let db = rusqlite::Connection::open(wiki.0.join("wiki-state/usage.sqlite")).unwrap();
    assert_eq!(
        db.query_row(
            "select count(*) from injections where source='recall-skill' and session_id='shared'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        2
    );
    assert_eq!(
        db.query_row(
            "select count(*) from injections where source='remind'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        0
    );
    let output = run_output(wiki.command(&["sync", "--rebuild"]), "");
    assert!(matches!(output.status.code(), Some(0 | 1)));
    assert_eq!(
        db.query_row("select count(*) from injections", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        2
    );
}

#[test]
fn skill_pages_and_recall_dedupe_in_both_directions_and_log_only_returned_pages() {
    for recall_first in [false, true] {
        let wiki = Wiki::new();
        for name in ["foo", "bar"] {
            let path = wiki.0.join(format!("wiki/pages/knowledge/{name}.md"));
            fs::write(
                &path,
                fs::read_to_string(&path)
                    .unwrap()
                    .replace("project: demo\n", ""),
            )
            .unwrap();
        }
        let hook = json!({"session_id":"shared", "hook_event_name":"PreToolUse", "tool_name":"Skill", "tool_input":{"skill":"cache"}}).to_string();
        let skill_pages = || {
            serde_json::from_str::<Value>(&run(
                wiki.command(&[
                    "skill-pages",
                    "plugin:cache",
                    "--json",
                    "-n",
                    "1",
                    "--cwd",
                    "/",
                    "--session",
                    "shared",
                    "--source",
                    "mod-skill",
                ]),
                "",
            ))
            .unwrap()
        };
        let recall = || run(wiki.command(&["recall", "--max-skill-pages", "1"]), &hook);
        let (report, recalled) = if recall_first {
            let recalled = recall();
            (skill_pages(), recalled)
        } else {
            let report = skill_pages();
            (report, recall())
        };
        let pages = report["pages"].as_array().unwrap();
        assert_eq!(pages.len(), 1);
        let title = pages[0]["title"].as_str().unwrap();
        assert!(!recalled.contains(title));
        assert!(recalled.contains(if title.starts_with("foo") {
            "bar cache notes"
        } else {
            "foo cache notes"
        }));
        assert_eq!(skill_pages(), json!({"pages": []}));
        assert!(recall().is_empty());
        let injections: Value = serde_json::from_str(&run(
            wiki.command(&["injections", "--session", "shared", "--json"]),
            "",
        ))
        .unwrap();
        let rows = injections["pages"].as_array().unwrap();
        assert_eq!(rows.len(), 2);
        let mut sources: Vec<_> = rows
            .iter()
            .map(|row| {
                assert_eq!(row["sources"].as_array().unwrap().len(), 1);
                row["sources"][0].as_str().unwrap()
            })
            .collect();
        sources.sort();
        assert_eq!(sources, ["mod-skill", "recall-skill"]);
        // Without a session, existing seen state does not suppress text output.
        let text = run(wiki.command(&["skill-pages", "cache", "--cwd", "/"]), "");
        for name in ["foo", "bar"] {
            assert!(text.contains(&format!(
                "{} ({name} cache notes)\n",
                wiki.0
                    .join(format!("wiki/pages/knowledge/{name}.md"))
                    .display()
            )));
        }
        assert!(run(wiki.command(&["skill-pages", "agent-wiki"]), "").is_empty());
    }
}

#[test]
fn skill_pages_and_remind_share_seen_pages_in_both_directions() {
    let wiki = Wiki::new();
    for name in ["foo", "bar"] {
        let path = wiki.0.join(format!("wiki/pages/knowledge/{name}.md"));
        fs::write(
            &path,
            fs::read_to_string(&path)
                .unwrap()
                .replace("project: demo\n", ""),
        )
        .unwrap();
    }
    let listener = wiki.listener();
    let output = run(
        wiki.command(&[
            "skill-pages",
            "cache",
            "--json",
            "--session",
            "mod-first",
            "--cwd",
            "/",
        ]),
        "",
    );
    assert_eq!(
        serde_json::from_str::<Value>(&output).unwrap()["pages"]
            .as_array()
            .unwrap()
            .len(),
        2
    );
    assert_no_connection(&listener);
    drop(listener);
    fs::remove_file(wiki.0.join("embed.sock")).unwrap();
    let reply = json!({"ok":true, "results":[{"path":"knowledge/foo", "score":1}, {"path":"knowledge/bar", "score":1}]});
    let (output, _) = wiki.respond(
        &["remind"],
        &json!({"session_id":"mod-first", "prompt":"cache"}).to_string(),
        reply.clone(),
    );
    assert!(output.is_empty());
    fs::remove_file(wiki.0.join("embed.sock")).unwrap();
    let (output, _) = wiki.respond(
        &["remind"],
        &json!({"session_id":"remind-first", "prompt":"cache"}).to_string(),
        reply,
    );
    assert!(output.contains("foo cache notes") && output.contains("bar cache notes"));
    let output = run(
        wiki.command(&[
            "skill-pages",
            "cache",
            "--json",
            "--session",
            "remind-first",
            "--cwd",
            "/",
        ]),
        "",
    );
    assert_eq!(
        serde_json::from_str::<Value>(&output).unwrap(),
        json!({"pages": []})
    );
}

#[test]
fn context_logs_only_overview_and_retained_project_lists_and_bounds_stdin() {
    let wiki = Wiki::new();
    for (path, text) in [
        (
            "projects/wiki",
            "---\ntitle: Overview\ncreated: 2020-01-01\n---\nProject body.",
        ),
        (
            "followups/task",
            "---\ntitle: Task\ncreated: 2020-01-01\nproject: wiki\nstatus: open\n---\nTask body.",
        ),
        (
            "knowledge/project",
            "---\ntitle: Project\ncreated: 2020-01-01\nproject: wiki\n---\nFacts.",
        ),
    ] {
        fs::write(wiki.0.join(format!("wiki/pages/{path}.md")), text).unwrap();
    }
    let mut command = wiki.command(&["context"]);
    command.current_dir(wiki.0.join("wiki"));
    let output = run(
        command,
        &json!({"session_id":"context-session"}).to_string(),
    );
    assert!(output.contains("Recently updated:"));
    let db = rusqlite::Connection::open(wiki.0.join("wiki-state/usage.sqlite")).unwrap();
    let paths: Vec<String> = db
        .prepare("select path from injections order by path")
        .unwrap()
        .query_map([], |r| r.get(0))
        .unwrap()
        .collect::<rusqlite::Result<_>>()
        .unwrap();
    assert_eq!(
        paths,
        [
            "pages/followups/task.md",
            "pages/knowledge/project.md",
            "pages/projects/wiki.md"
        ]
    );
    let mut command = wiki.command(&["context", "--budget", "1"]);
    command.current_dir(wiki.0.join("wiki"));
    let output = run_output(command, &json!({"session_id":"trimmed"}).to_string());
    assert!(output.status.success());
    assert_eq!(
        db.query_row(
            "select count(*) from injections where session_id='trimmed'",
            [],
            |r| r.get::<_, i64>(0)
        )
        .unwrap(),
        1
    );
    let mut child = wiki
        .command(&["context"])
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    let stdin = child.stdin.take().unwrap();
    let start = Instant::now();
    loop {
        if let Some(status) = child.try_wait().unwrap() {
            assert!(status.success());
            break;
        }
        if start.elapsed() > Duration::from_secs(2) {
            child.kill().unwrap();
            panic!("context blocked on stdin");
        }
        thread::sleep(Duration::from_millis(10));
    }
    assert!(start.elapsed() < Duration::from_millis(800));
    drop(stdin);
    assert_eq!(
        db.query_row("select count(*) from injections", [], |r| r
            .get::<_, i64>(0))
            .unwrap(),
        4
    );
}

#[test]
fn check_warns_about_non_korean_prose_for_every_page_type() {
    let wiki = Wiki::new();
    let english = "The cache stores build results so later runs can reuse them. ".repeat(10);
    let korean = "캐시는 빌드 결과를 저장하므로 다음 실행에서 다시 사용할 수 있습니다. ".repeat(12);
    for kind in ["knowledge", "projects", "followups", "history"] {
        let target = format!("{kind}/language");
        let path = wiki.0.join(format!("wiki/pages/{target}.md"));
        for (body, code) in [(&english, 1), (&korean, 0)] {
            fs::write(
                &path,
                format!("---\ntitle: 캐시\ncreated: 2026-09-29\nstatus: open\n---\n{body}"),
            )
            .unwrap();
            let output = run_output(wiki.command(&["check", &target]), "");
            assert_eq!(output.status.code(), Some(code), "{kind}: {output:?}");
            assert_eq!(
                String::from_utf8(output.stderr).unwrap(),
                if code == 1 {
                    format!(
                        "warning: pages/{target}.md: prose is 0% Hangul; wiki pages are written in Korean (identifiers, commands and quoted output stay verbatim)\n"
                    )
                } else {
                    String::new()
                }
            );
        }
    }
    for (text, problem) in [
        (english.clone(), "has no frontmatter"),
        (
            format!("---\ntitle: Cache\ncreated: 2026-09-29\n---\n{english}\n<!-- hidden -->"),
            "blocked: pages/knowledge/foo.md: injection:",
        ),
    ] {
        fs::write(wiki.0.join("wiki/pages/knowledge/foo.md"), text).unwrap();
        let output = run_output(wiki.command(&["check", "knowledge/foo"]), "");
        assert_eq!(output.status.code(), Some(2), "{output:?}");
        assert!(String::from_utf8(output.stderr).unwrap().contains(problem));
    }
}

#[test]
fn lint_non_korean_category_has_usage_json_and_text_limits() {
    let wiki = Wiki::new();
    let english = "The cache stores build results so later runs can reuse them. ".repeat(10);
    let mut paths = vec![];
    for i in 0..21 {
        let kind = ["knowledge", "projects", "followups", "history"][i % 4];
        let path = format!("pages/{kind}/language-{i:02}.md");
        fs::write(
            wiki.0.join("wiki").join(&path),
            format!("---\ntitle: Cache\ncreated: 2026-09-29\nstatus: done\n---\n{english}"),
        )
        .unwrap();
        paths.push(path);
    }
    paths.sort();
    for (name, body) in [
        ("blocked", format!("{english}\n<!-- hidden -->")),
        (
            "korean",
            "캐시가 사라지면 저장 경로와 실행 환경을 먼저 확인합니다. ".repeat(12),
        ),
    ] {
        fs::write(
            wiki.0.join(format!("wiki/pages/knowledge/{name}.md")),
            format!("---\ntitle: Cache\ncreated: 2026-09-29\n---\n{body}"),
        )
        .unwrap();
    }
    for (args, limit) in [(vec!["lint"], 20), (vec!["lint", "--all"], 21)] {
        let output = run_output(wiki.command(&args), "");
        assert!(output.status.success(), "{output:?}");
        let text = String::from_utf8(output.stdout).unwrap();
        let (before, after) = text.split_once("non-korean (21)\n").unwrap();
        assert!(before.contains("missing-queries ("));
        assert!(!before.contains("orphans ("));
        let category = after.split_once("orphans (").unwrap().0;
        let expected = paths
            .iter()
            .take(limit)
            .map(|path| format!("  {path}: prose is 0% Hangul\n"))
            .collect::<String>();
        assert_eq!(
            category,
            if limit == 20 {
                format!("{expected}  … and 1 more\n")
            } else {
                expected
            }
        );
    }
    let output = run_output(wiki.command(&["lint", "--json"]), "");
    assert!(output.status.success(), "{output:?}");
    let report: Value = serde_json::from_slice(&output.stdout).unwrap();
    let expected: Vec<_> = paths
        .iter()
        .map(|path| {
            json!({
                "detail": "prose is 0% Hangul",
                "path": path,
                "usage": {
                    "importance": 0.0,
                    "injected_unread": 0,
                    "last_read": null,
                    "read_sessions": 0,
                    "reads": 0
                }
            })
        })
        .collect();
    assert_eq!(report["non-korean"], json!(expected));
}

#[test]
fn new_page_duplicate_notes_and_lint_pairs_use_batch_protocol() {
    let wiki = Wiki::new();
    let text = "---\ntitle: New cache\ncreated: 2020-01-01\n---\nCache.";
    fs::write(wiki.0.join("wiki/pages/knowledge/new.md"), text).unwrap();
    let (output, request) = wiki.respond_output(
        &["check", "knowledge/new"],
        "",
        json!({"ok":true,"results":[{"path":"knowledge/foo", "score":0.91}]}),
    );
    assert!(output.status.success());
    assert_eq!(
        request,
        json!({"op":"similar", "text":text, "exclude":"pages/knowledge/new.md", "n":3, "min":0.85})
    );
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("(0.91, foo cache notes)")
    );
    fs::remove_file(wiki.0.join("embed.sock")).unwrap();
    fs::write(
        wiki.0.join("wiki/pages/followups/closed.md"),
        "---\ntitle: Closed\ncreated: 2020-01-01\nstatus: done\n---\nCache.",
    )
    .unwrap();
    let (output, request) = wiki.respond(
        &["lint", "--json"],
        "",
        json!({"ok":true,"results":[
        {"a":"knowledge/foo", "b":"knowledge/bar", "score":0.9},
        {"a":"knowledge/foo", "b":"followups/closed", "score":0.99}]}),
    );
    assert_eq!(
        request,
        json!({"op":"pairs", "min":0.88, "types":["knowledge","followups"]})
    );
    let report: Value = serde_json::from_str(&output).unwrap();
    assert_eq!(report["duplicates"].as_array().unwrap().len(), 1);
}

#[test]
fn shrink_warning_commits_and_reports_exit_one() {
    let wiki = Wiki::new();
    let path = wiki.0.join("wiki/pages/knowledge/foo.md");
    fs::write(
        &path,
        format!(
            "---\ntitle: foo\ncreated: 2020-01-01\nqueries: [foo]\n---\n{}",
            "x".repeat(5000)
        ),
    )
    .unwrap();
    let git = |args: &[&str]| {
        assert!(
            Command::new("git")
                .arg("-C")
                .arg(wiki.0.join("wiki"))
                .args(args)
                .status()
                .unwrap()
                .success()
        );
    };
    git(&["add", "pages/knowledge/foo.md"]);
    git(&[
        "-c",
        "user.name=test",
        "-c",
        "user.email=test@example.com",
        "commit",
        "-qm",
        "long page",
    ]);
    fs::write(
        &path,
        "---\ntitle: foo\ncreated: 2020-01-01\nqueries: [foo]\n---\nShort.",
    )
    .unwrap();
    let mut command = wiki.command(&["sync", "knowledge/foo"]);
    command
        .env("GIT_AUTHOR_NAME", "test")
        .env("GIT_AUTHOR_EMAIL", "test@example.com")
        .env("GIT_COMMITTER_NAME", "test")
        .env("GIT_COMMITTER_EMAIL", "test@example.com");
    let output = run_output(command, "");
    assert_eq!(output.status.code(), Some(1));
    assert!(
        String::from_utf8(output.stderr)
            .unwrap()
            .contains("shrank from 5054 to 60 chars")
    );
    assert!(
        String::from_utf8(output.stdout)
            .unwrap()
            .contains("committed:")
    );
}

#[test]
fn context_waits_for_a_busy_lock_and_then_prints_its_summary() {
    let wiki = Wiki::new();
    let mut lock = fd_lock::RwLock::new(
        fs::OpenOptions::new()
            .write(true)
            .open(wiki.0.join("wiki-state/wiki.lock"))
            .unwrap(),
    );
    let guard = lock.write().unwrap();
    let mut child = wiki
        .command(&["context"])
        .env_remove("CLAUDE_WIKI_HOOK_LOCK_MS")
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .unwrap();
    thread::sleep(Duration::from_millis(150));
    assert!(
        child.try_wait().unwrap().is_none(),
        "context skipped a busy lock"
    );
    drop(guard);
    let output = child.wait_with_output().unwrap();
    assert!(output.status.success(), "{output:?}");
    assert!(output.stderr.is_empty(), "{output:?}");
    assert!(
        String::from_utf8(output.stdout)
            .unwrap()
            .starts_with("Shared wiki:")
    );
}

#[test]
fn hooks_respect_lock_deadline_and_non_hooks_still_wait() {
    let wiki = Wiki::new();
    let mut lock = fd_lock::RwLock::new(
        fs::OpenOptions::new()
            .write(true)
            .open(wiki.0.join("wiki-state/wiki.lock"))
            .unwrap(),
    );
    let guard = lock.write().unwrap();
    for hook in ["context", "remind", "recall"] {
        let mut command = wiki.command(&[hook]);
        command.env("CLAUDE_WIKI_HOOK_LOCK_MS", "100");
        let started = Instant::now();
        let output = run_output(command, "");
        assert!(started.elapsed() >= Duration::from_millis(100));
        assert!(started.elapsed() < Duration::from_secs(1));
        assert!(output.status.success(), "{output:?}");
        assert!(
            output.stdout.is_empty() && output.stderr.is_empty(),
            "{output:?}"
        );
    }
    let mut children: Vec<_> = [
        vec!["list"],
        vec!["search", "foo", "--json"],
        vec!["related", "--json", "--cwd", "/"],
        vec!["injections", "--session", "missing", "--json"],
        vec!["skill-pages", "cache", "--json", "--cwd", "/"],
    ]
    .into_iter()
    .map(|args| {
        let child = wiki
            .command(&args)
            .env("CLAUDE_WIKI_HOOK_LOCK_MS", "0")
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::piped())
            .spawn()
            .unwrap();
        (args, child)
    })
    .collect();
    thread::sleep(Duration::from_millis(150));
    for (args, child) in &mut children {
        assert!(child.try_wait().unwrap().is_none(), "{args:?}");
    }
    drop(guard);
    for (args, child) in children {
        let output = child.wait_with_output().unwrap();
        assert!(output.status.success(), "{args:?}: {output:?}");
        let text = String::from_utf8(output.stdout).unwrap();
        if args[0] == "list" {
            assert!(text.contains("foo cache notes"));
        } else {
            serde_json::from_str::<Value>(&text).unwrap();
        }
    }
}

#[test]
fn event_reminders_bypass_wiki_locks_and_fail_open_on_sqlite_contention() {
    let wiki = Wiki::new();
    let input = json!({"session_id":"event-lock", "hook_event_name":"PreCompact"}).to_string();
    let mut lock = fd_lock::RwLock::new(
        fs::OpenOptions::new()
            .write(true)
            .open(wiki.0.join("wiki-state/wiki.lock"))
            .unwrap(),
    );
    let guard = lock.write().unwrap();
    let started = Instant::now();
    let output = run_output(wiki.command(&["remind", "--event"]), &input);
    assert!(started.elapsed() < Duration::from_secs(1));
    assert!(
        output.status.success()
            && String::from_utf8_lossy(&output.stdout).contains("context compaction")
    );
    drop(guard);

    let input = input.replace("event-lock", "event-busy");
    let db = rusqlite::Connection::open(wiki.0.join("wiki-state/state.sqlite")).unwrap();
    db.execute_batch("begin exclusive").unwrap();
    let started = Instant::now();
    let output = run_output(wiki.command(&["remind", "--event"]), &input);
    assert!(started.elapsed() < Duration::from_secs(1));
    assert!(output.status.success() && output.stdout.is_empty());
    db.execute_batch("rollback").unwrap();
    assert!(run(wiki.command(&["remind", "--event"]), &input).contains("context compaction"));
}

#[test]
fn event_reminders_do_not_refresh_pages_or_connect_to_semantic_daemon() {
    let wiki = Wiki::new();
    let listener = wiki.listener();
    fs::write(
        wiki.0.join("wiki/pages/knowledge/new.md"),
        "---\ntitle: new\n---\nbody",
    )
    .unwrap();
    let input = json!({
        "session_id":"event-fast", "hook_event_name":"PostToolUse", "tool_name":"Bash",
        "tool_input":{"command":"git commit -m fix"}, "tool_response":{"interrupted":false},
    })
    .to_string();
    assert!(run(wiki.command(&["remind", "--event"]), &input).contains("you just committed"));
    assert_no_connection(&listener);
    let db = rusqlite::Connection::open(wiki.0.join("wiki-state/index.sqlite")).unwrap();
    let count: i64 = db
        .query_row(
            "select count(*) from files where path='pages/knowledge/new.md'",
            [],
            |r| r.get(0),
        )
        .unwrap();
    assert_eq!(count, 0);
}

#[test]
fn plain_check_uses_one_head_listing_and_no_per_page_git_show() {
    let wiki = Wiki::new();
    for i in 0..40 {
        fs::write(
            wiki.0.join(format!("wiki/pages/knowledge/check-{i}.md")),
            "---\ntitle: Checked\ncreated: 2020-01-01\n---\nFacts.",
        )
        .unwrap();
    }
    for args in [
        vec!["add", "pages"],
        vec![
            "-c",
            "user.name=test",
            "-c",
            "user.email=test@example.com",
            "commit",
            "-qm",
            "check fixture",
        ],
    ] {
        assert!(
            Command::new("git")
                .arg("-C")
                .arg(wiki.0.join("wiki"))
                .args(args)
                .status()
                .unwrap()
                .success()
        );
    }
    let listener = wiki.listener();
    let trace = wiki.0.join("git-trace.jsonl");
    let mut command = wiki.command(&["check"]);
    command.env("GIT_TRACE2_EVENT", &trace);
    run(command, "");
    assert_no_connection(&listener);
    let events: Vec<Value> = fs::read_to_string(trace)
        .unwrap()
        .lines()
        .map(|line| serde_json::from_str(line).unwrap())
        .collect();
    let calls: Vec<_> = events
        .iter()
        .filter(|event| event["event"] == "start")
        .collect();
    assert_eq!(calls.len(), 1, "{calls:?}");
    let args = calls[0]["argv"].as_array().unwrap();
    assert!(args.iter().any(|arg| arg == "ls-tree"));
    assert!(!args.iter().any(|arg| arg == "show"));
}

/// Every path under `dir` with its modification time, to prove a command wrote nothing.
fn tree(dir: &std::path::Path) -> Vec<(PathBuf, Option<std::time::SystemTime>)> {
    let mut entries = vec![];
    let mut pending = vec![dir.to_path_buf()];
    while let Some(path) = pending.pop() {
        let Ok(meta) = fs::symlink_metadata(&path) else {
            continue;
        };
        if meta.is_dir() {
            pending.extend(fs::read_dir(&path).unwrap().map(|e| e.unwrap().path()));
        }
        entries.push((path, meta.modified().ok()));
    }
    entries.sort();
    entries
}

#[test]
fn no_refresh_reads_the_existing_index_without_writing() {
    let wiki = Wiki::new();
    let state = wiki.0.join("wiki-state");
    assert!(state.join("index.sqlite").is_file());
    // A lock file is never needed: remove it so a recreated one would show.
    fs::remove_file(state.join("wiki.lock")).unwrap();
    fs::write(
        wiki.0.join("wiki/pages/knowledge/fresh.md"),
        "---\ntitle: fresh zirconium notes\ncreated: 2026-11-27\n---\nzirconium\n",
    )
    .unwrap();
    let before = tree(&wiki.0);
    let search = run(
        wiki.command(&["search", "--no-refresh", "--no-semantic", "cache"]),
        "",
    );
    assert!(search.contains("foo cache notes"), "{search}");
    let fresh = run(
        wiki.command(&["search", "--no-refresh", "--no-semantic", "zirconium"]),
        "",
    );
    assert!(fresh.starts_with("no pages match"), "{fresh}");
    let json: Value = serde_json::from_str(&run(
        wiki.command(&["search", "--no-refresh", "--no-semantic", "--json", "cache"]),
        "",
    ))
    .unwrap();
    assert_eq!(json["hits"].as_array().unwrap().len(), 2, "{json}");
    let list = run(wiki.command(&["list", "--no-refresh"]), "");
    assert!(list.contains("bar cache notes"), "{list}");
    assert!(!list.contains("zirconium"), "{list}");
    assert_eq!(tree(&wiki.0), before);
    for name in ["wiki.lock", "migration.lock"] {
        assert!(!state.join(name).exists());
    }
    assert!(!wiki.0.join("wiki/.lock").exists());
    // An ordinary command still refreshes and finds the new page.
    let refreshed = run(wiki.command(&["search", "--no-semantic", "zirconium"]), "");
    assert!(refreshed.contains("fresh zirconium notes"), "{refreshed}");
}

#[test]
fn no_refresh_without_an_index_creates_nothing() {
    static NEXT: AtomicU64 = AtomicU64::new(0);
    let dir = std::env::temp_dir().join(format!(
        "cw-no-refresh-{}-{}",
        std::process::id(),
        NEXT.fetch_add(1, Ordering::Relaxed)
    ));
    fs::create_dir_all(dir.join("wiki/pages/knowledge")).unwrap();
    fs::write(
        dir.join("wiki/pages/knowledge/page.md"),
        "---\ntitle: unindexed\ncreated: 2026-11-27\n---\nbody\n",
    )
    .unwrap();
    let wiki = Wiki(dir);
    let before = tree(&wiki.0);
    for (args, stdout) in [
        (vec!["search", "--no-refresh", "unindexed"], ""),
        (vec!["list", "--no-refresh"], ""),
        (
            vec!["search", "--no-refresh", "--json", "unindexed"],
            "{\"dense\":\"disabled\",\"hits\":[],\"lexical\":false}\n",
        ),
    ] {
        let output = run_output(wiki.command(&args), "");
        assert!(output.status.success(), "{output:?}");
        assert_eq!(String::from_utf8(output.stdout).unwrap(), stdout);
        assert!(
            String::from_utf8(output.stderr)
                .unwrap()
                .contains("no index yet"),
        );
    }
    // A root that does not exist at all stays absent too.
    let mut missing = wiki.command(&["list", "--no-refresh"]);
    missing.env("CLAUDE_WIKI_DIR", wiki.0.join("absent"));
    assert!(run_output(missing, "").status.success());
    assert_eq!(tree(&wiki.0), before);
}

#[test]
fn no_refresh_on_wal_indexes_reads_committed_frames_without_new_files() {
    use std::os::unix::fs::PermissionsExt;
    let wiki = Wiki::new();
    let state = wiki.0.join("wiki-state");
    let index = state.join("index.sqlite");
    let names = |dir: &std::path::Path| -> Vec<String> {
        let mut names: Vec<_> = fs::read_dir(dir)
            .unwrap()
            .map(|e| e.unwrap().file_name().to_string_lossy().into_owned())
            .collect();
        names.sort();
        names
    };
    let lookups = |expect: &str| {
        let search = run(
            wiki.command(&[
                "search",
                "--no-refresh",
                "--no-semantic",
                "cache",
                "zirconium",
            ]),
            "",
        );
        assert!(search.contains(expect), "{search}");
        let list = run(wiki.command(&["list", "--no-refresh"]), "");
        assert!(list.contains(expect), "{list}");
    };
    // 1. A clean WAL-mode index: the header says WAL, but no sidecars exist.
    rusqlite::Connection::open(&index)
        .unwrap()
        .pragma_update(None, "journal_mode", "wal")
        .unwrap();
    assert!(!names(&state).iter().any(|n| n.starts_with("index.sqlite-")));
    for mode in [0o700, 0o500] {
        fs::set_permissions(&state, fs::Permissions::from_mode(mode)).unwrap();
        let before = (tree(&wiki.0), tree(&state));
        lookups("foo cache notes");
        assert_eq!((tree(&wiki.0), tree(&state)), before);
    }
    fs::set_permissions(&state, fs::Permissions::from_mode(0o700)).unwrap();
    // 2. Committed pages that live only in the -wal: an open reader keeps the
    // refresh below from checkpointing on close.
    let holder = rusqlite::Connection::open(&index).unwrap();
    holder
        .query_row("select count(*) from files", [], |r| r.get::<_, i64>(0))
        .unwrap();
    fs::write(
        wiki.0.join("wiki/pages/knowledge/fresh.md"),
        "---\ntitle: fresh zirconium notes\ncreated: 2026-11-27\n---\nzirconium\n",
    )
    .unwrap();
    run(wiki.command(&["list"]), "");
    assert!(fs::metadata(state.join("index.sqlite-wal")).unwrap().len() > 0);
    for mode in [0o700, 0o500] {
        fs::set_permissions(&state, fs::Permissions::from_mode(mode)).unwrap();
        let before = (tree(&wiki.0), tree(&state));
        lookups("fresh zirconium notes");
        assert_eq!((tree(&wiki.0), tree(&state)), before);
    }
    fs::set_permissions(&state, fs::Permissions::from_mode(0o700)).unwrap();
    drop(holder);
}
