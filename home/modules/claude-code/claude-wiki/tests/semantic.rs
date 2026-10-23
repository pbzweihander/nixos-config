use serde_json::{Value, json};
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
        self.respond_bytes(args, input, format!("{reply}\n").into_bytes())
    }

    fn respond_bytes(&self, args: &[&str], input: &str, reply: Vec<u8>) -> (Output, Value) {
        let listener = self.listener();
        let server = thread::spawn(move || {
            let (mut stream, _) = listener.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut request = String::new();
            BufReader::new(&mut stream).read_line(&mut request).unwrap();
            // Oversized replies can be rejected before the peer finishes writing.
            let _ = stream.write_all(&reply);
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
            {"path":"knowledge/foo","score":1,"heading":"Recovery","start_line":7,"end_line":8},
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
            "pages":[{"path":cwd.join("pages/knowledge/foo.md"), "rel":"pages/knowledge/foo.md", "title":"foo cache notes", "type":"knowledge", "section":{"heading":"Recovery","start":7,"end":8}}],
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
            let (mut stream, _) = listener.accept().unwrap();
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
                "---\ntitle: Hidden {name}\nstatus: open\nproject: target\ntags: [unique]\ncreated: 2026-09-20\n---\n{body}\n"
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
            {"path":"followups/match","score":0.6,"heading":"Recovery","start_line":7,"end_line":8},
            {"path":"followups/blocked","score":0.5,"heading":"Hidden section","start_line":1,"end_line":9}
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
            json!({"heading":"Recovery","start":7,"end":8})
        );
        let blocked = hits.last().unwrap();
        assert_eq!(blocked["title"], "[BLOCKED: secret]");
        assert_eq!(blocked["snippet"], "");
        assert!(blocked["section"].is_null());
        assert!(!output.contains("Hidden blocked"));
        assert!(!output.contains("Hidden section"));
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
    let (output, request) = wiki.respond(
        &["search", "unmatchedmeaning", "anotherquery", "-n", "1"],
        "",
        json!({"ok": true, "tier": "gpu", "results": [
            {"path": "knowledge/deleted", "score": 2.0},
            {"path": "knowledge/foo", "score": 1.23, "heading": "What kills it", "start_line": 27, "end_line": 36},
            {"path": "knowledge/bar", "score": 0.9}
        ]}),
    );
    assert_eq!(
        request,
        json!({"op": "dense", "query": "unmatchedmeaning anotherquery", "n": 40})
    );
    let lines: Vec<_> = output.lines().collect();
    assert_eq!(
        lines[0],
        format!(
            "{} (meaning)",
            wiki.0.join("wiki/pages/knowledge/foo.md").display()
        )
    );
    assert!(lines[1].starts_with("  foo cache notes  (knowledge; updated "));
    assert!(lines[1].ends_with("; project=demo; tags=cache)"));
    assert_eq!(lines[2], "  § What kills it (lines 27-36)");
    assert_eq!(lines.len(), 3);
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
                let (mut stream, _) = listener.accept().unwrap();
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
            "claude-wiki: pages that may already cover this: {} (foo cache notes). Read one if it bears on the request; otherwise ignore this note and do not reply to it.",
            wiki.0.join("wiki/pages/knowledge/foo.md").display()
        )
    );
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
    let db = rusqlite::Connection::open(wiki.0.join("wiki/usage.sqlite")).unwrap();
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
        assert!(run(wiki.command(&["skill-pages", "claude-wiki"]), "").is_empty());
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
    let db = rusqlite::Connection::open(wiki.0.join("wiki/usage.sqlite")).unwrap();
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
            .open(wiki.0.join("wiki/.lock"))
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
            .open(wiki.0.join("wiki/.lock"))
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
