import contextlib
import fcntl
import io
import json
import os
from pathlib import Path
import subprocess
import sys
import tempfile
import time
import unittest
from unittest.mock import patch


PACKAGE = Path(__file__).resolve().parents[1]
sys.path.insert(0, str(PACKAGE))

from wiki_curator.gitutil import changes, git, status
from wiki_curator.preservation import preserved, tokens
from wiki_curator.runner import DIFF_LIMIT, DIFF_TRUNCATED, read_state, run, save_rejection_diff
from wiki_curator.selection import DAY, select_tasks


A = "pages/knowledge/a.md"
B = "pages/knowledge/b.md"
REF = "pages/knowledge/reference.md"
OLD = "Measured `probe --fast` takes 45s. [[knowledge/missing]]\n"
NEW = "Measured `probe --fast` takes 45s. [[knowledge/reference]]\n"


def page(body, *, created="2020-01-01", extra=""):
    return f"---\ntitle: Example\ntags: [nix]\ncreated: {created}\n{extra}---\n\n{body}"


def justification(**overrides):
    return {"summary": "repair a dangling reference", "dropped": [
        {"token": "[[knowledge/missing]]", "reason": "corrected the dangling target"}],
        "redirects": {}, **overrides}


class RepoTest(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)
        self.wiki = self.root / "wiki"
        self.wiki.mkdir()
        self.cache = self.root / "cache/wiki-curator"
        self.bin = self.root / "bin"
        self.bin.mkdir()
        self.fixture_path = self.root / "fixture.json"
        self.calls_path = self.root / "calls.jsonl"
        fake = Path(__file__).with_name("fake_cli.py").read_text()
        for name in ("claude", "codex", "claude-wiki"):
            path = self.bin / name
            path.write_text(f"#!{sys.executable}\n" + fake)
            path.chmod(0o755)
        self.env = {
            **os.environ, "HOME": str(self.root), "XDG_CONFIG_HOME": str(self.root / "config"),
            "XDG_CACHE_HOME": str(self.root / "cache"), "GIT_CONFIG_NOSYSTEM": "1",
            "GIT_CONFIG_GLOBAL": os.devnull, "GIT_AUTHOR_NAME": "Curator Test",
            "GIT_AUTHOR_EMAIL": "curator@example.invalid", "GIT_COMMITTER_NAME": "Curator Test",
            "GIT_COMMITTER_EMAIL": "curator@example.invalid", "PATH": f"{self.bin}:{os.environ['PATH']}",
            "CURATOR_FIXTURE": str(self.fixture_path), "CURATOR_CALLS": str(self.calls_path),
            "CURATOR_MAIN": str(self.wiki), "CLAUDE_WIKI_DIR": str(self.wiki),
            "PYTHONPATH": str(PACKAGE), "PYTHONDONTWRITEBYTECODE": "1",
        }
        for key in ("WIKI_CURATOR_MODEL", "WIKI_CURATOR_MAX_TASKS", "GIT_DIR", "GIT_WORK_TREE", "GIT_INDEX_FILE"):
            self.env.pop(key, None)
        self.environment = patch.dict(os.environ, self.env, clear=True)
        self.environment.start()
        self.addCleanup(self.environment.stop)
        git(self.wiki, "init", "-b", "main")
        git(self.wiki, "config", "commit.gpgsign", "false")
        self.write(".gitignore", ".lock\nindex.sqlite*\nusage.sqlite*\nsessions.sqlite*\nvectors-*.npz\n")
        self.write(A, page(OLD))
        self.write(REF, page("Reference for the probe.\n"))
        self.write("notes.txt", "original notes\n")
        self.commit("initial")
        self.fixture = {"lint": {"broken-links": [{"path": A, "detail": "knowledge/missing"}]},
                        "writes": {A: page(NEW)}, "justification": justification()}
        self.save_fixture()

    def write(self, path, text):
        full = self.wiki / path
        full.parent.mkdir(parents=True, exist_ok=True)
        full.write_text(text)

    def age(self):
        for path in self.wiki.rglob("*.md"):
            os.utime(path, (1, 1))

    def commit(self, message="fixture"):
        git(self.wiki, "add", "-A")
        git(self.wiki, "commit", "-m", message)
        self.age()
        return git(self.wiki, "rev-parse", "HEAD").strip()

    def save_fixture(self):
        self.fixture_path.write_text(json.dumps(self.fixture))

    def calls(self, name=None):
        values = [json.loads(line) for line in self.calls_path.read_text().splitlines()] if self.calls_path.exists() else []
        return [v for v in values if name is None or v["name"] == name]

    def execute(self, **kwargs):
        self.save_fixture()
        with contextlib.redirect_stdout(io.StringIO()):
            code = run(self.wiki, self.cache, **kwargs)
        report = json.loads(sorted((self.cache / "runs").glob("*.json"))[-1].read_text())
        self.assertFalse((self.cache / "worktree").exists())
        self.assertEqual(git(self.wiki, "worktree", "list", "--porcelain").count("worktree "), 1)
        return code, report

    def result(self, **kwargs):
        code, report = self.execute(**kwargs)
        self.assertEqual(code, 0, report)
        self.assertEqual(len(report["tasks"]), 1, report)
        return report["tasks"][0]


class SelectionTests(RepoTest):
    def test_not_duplicates_suppresses_only_the_pair_until_expiry(self):
        self.write(B, page("Related but distinct facts.\n"))
        self.commit()
        lint = {"duplicates": [{"a": B, "b": A, "score": .99}],
                "missing-queries": [{"path": A, "detail": "missing"}]}
        now = time.time()
        cached = {f"{A}|{B}": now + 90 * DAY}
        tasks = select_tasks(self.wiki, lint, {}, now, not_duplicates=cached)
        self.assertEqual([t.type for t in tasks], ["missing-queries"])
        self.assertEqual(tasks[0].primary, [A])
        for deadline in (now - 1, now):
            tasks = select_tasks(self.wiki, lint, {}, now, not_duplicates={f"{A}|{B}": deadline})
            self.assertEqual([t.type for t in tasks], ["duplicates"])
        tasks = select_tasks(self.wiki, lint, {}, now, not_duplicates={f"{A}|{REF}": now + DAY})
        self.assertEqual([t.type for t in tasks], ["duplicates"])

    def test_duplicate_snapshot_exclusions_apply_to_either_page_and_only_filter(self):
        dated = "pages/knowledge/design-2020-02-01-round.md"
        for path in (dated, B):
            self.write(path, page("A separate snapshot.\n"))
        self.commit()
        for snapshot_page, listed in ((dated, False), (dated, True), (B, True)):
            for a, b in ((A, snapshot_page), (snapshot_page, A)):
                with self.subTest(a=a, b=b, listed=listed):
                    lint = {"duplicates": [{"a": a, "b": b, "score": .99}]}
                    if listed:
                        lint["snapshots"] = [{"path": snapshot_page, "detail": "dated"}]
                    self.assertEqual(select_tasks(self.wiki, lint, {}, time.time(), only=["duplicates"]), [])
                    tasks = select_tasks(self.wiki, lint, {}, time.time())
                    self.assertEqual([t.type for t in tasks], ["snapshots"] if listed and snapshot_page == dated else [])

    def test_priority_dedup_groups_and_limits(self):
        for name in ("dup1", "dup2", "log", "2020-02-01-snapshot", *[f"query{i}" for i in range(7)],
                     *[f"tag{i}" for i in range(24)]):
            self.write(f"pages/knowledge/{name}.md", page("Text.\n"))
        self.commit()
        lint = {
            "broken-links": [{"path": A, "detail": "one"}, {"path": A, "detail": "two"}],
            "duplicates": [{"a": "pages/knowledge/dup1.md", "b": "pages/knowledge/dup2.md", "score": .95}],
            "snapshots": [{"path": "pages/knowledge/2020-02-01-snapshot.md", "detail": "dated"}],
            "log-growth": [{"path": "pages/knowledge/log.md", "detail": "timeline"}],
            "missing-queries": [{"path": f"pages/knowledge/query{i}.md", "detail": "missing"} for i in range(7)],
            "singleton-tags": [{"path": f"pages/knowledge/tag{i}.md", "tag": f"tag{i}"} for i in range(24)],
            "orphans": [{"path": REF}], "stale-followups": [{"path": REF}], "unread": [{"path": REF}],
        }
        tasks = select_tasks(self.wiki, lint, {}, time.time(), max_tasks=10)
        self.assertEqual([t.type for t in tasks], ["broken-links", "duplicates", "snapshots", "log-growth", "missing-queries", "singleton-tags"])
        self.assertEqual(len(tasks[4].primary), 5)
        self.assertEqual(len(tasks[5].details["items"]), 20)
        self.assertEqual(tasks[2].details["destination"], "pages/history/2020-02-01-snapshot.md")
        self.assertEqual(len(select_tasks(self.wiki, lint, {}, time.time())), 5)
        self.assertEqual([t.type for t in select_tasks(self.wiki, lint, {}, time.time(), only=["log-growth"])], ["log-growth"])

    def test_projects_dirty_recent_cooldown_and_rename_source_are_skipped(self):
        paths = ["pages/projects/proj.md", "pages/knowledge/dirty.md", "pages/knowledge/recent.md",
                 "pages/knowledge/cooling.md", "pages/knowledge/rename.md", "pages/knowledge/ignored.md"]
        for path in paths:
            self.write(path, page("Text.\n"))
        self.commit()
        self.write(paths[1], page("Dirty edit\n"))
        os.utime(self.wiki / paths[2], None)
        git(self.wiki, "mv", paths[4], "pages/knowledge/moved.md")
        lint = {"broken-links": [{"path": path, "detail": "missing"} for path in [*paths, A]]}
        tasks = select_tasks(self.wiki, lint, {paths[3]: time.time() + DAY, paths[5]: time.time() + DAY}, time.time())
        self.assertEqual([t.primary for t in tasks], [[A]])
        self.assertIn(paths[4], status(self.wiki))

    def test_duplicate_eligibility_keeper_and_backlink_skips(self):
        self.write(B, page("Same fact.\n", created="2021-01-01"))
        self.write("pages/followups/open1.md", page("Work.\n", extra="status: open\n"))
        self.write("pages/followups/open2.md", page("Work.\n", extra="status: 'open'\n"))
        self.write("pages/followups/done.md", page("Work.\n", extra="status: done\n"))
        self.commit()
        lint = {"duplicates": [
            {"a": B, "b": A, "score": .99}, {"a": A, "b": B, "score": .98},
            {"a": A, "b": REF, "score": .89},
            {"a": A, "b": "pages/followups/open1.md", "score": .99},
            {"a": "pages/followups/open1.md", "b": "pages/followups/done.md", "score": .99},
            {"a": "pages/followups/open1.md", "b": "pages/followups/open2.md", "score": .99},
        ]}
        tasks = select_tasks(self.wiki, lint, {}, time.time())
        self.assertEqual(len(tasks), 2)
        self.assertEqual(next(t for t in tasks if A in t.primary).details["keep"], A)
        self.write("pages/projects/proj.md", page("[[knowledge/b]]\n"))
        self.commit()
        tasks = select_tasks(self.wiki, lint, {}, time.time())
        self.assertEqual(len(tasks), 1)
        self.assertTrue(tasks[0].primary[0].startswith("pages/followups/"))

    def test_snapshot_backlinks_and_date_in_middle(self):
        source = "pages/knowledge/service-2020-02-01-state.md"
        self.write(source, page("Snapshot.\n"))
        self.write(B, page("[[knowledge/service-2020-02-01-state|snapshot]]\n"))
        self.commit()
        lint = {"snapshots": [{"path": source, "detail": "dated"}]}
        tasks = select_tasks(self.wiki, lint, {}, time.time())
        self.assertIn(B, tasks[0].pages)
        self.assertEqual(tasks[0].details["destination"], "pages/history/2020-02-01-service-state.md")
        self.write(B, page("Dirty backlink.\n"))
        self.assertEqual(select_tasks(self.wiki, lint, {}, time.time()), [])


class StateTests(RepoTest):
    def test_not_duplicates_defaults_for_missing_and_legacy_state(self):
        self.assertEqual(read_state(self.cache), {"cooldowns": {}, "not_duplicates": {}, "dismissed_diffs": []})
        self.cache.mkdir(parents=True)
        cooldowns = {A: time.time() + DAY}
        (self.cache / "state.json").write_text(json.dumps({"cooldowns": cooldowns}))
        self.assertEqual(read_state(self.cache), {"cooldowns": cooldowns, "not_duplicates": {}, "dismissed_diffs": []})
        state = {"cooldowns": cooldowns, "not_duplicates": {f"{A}|{B}": time.time() + 90 * DAY}, "dismissed_diffs": []}
        (self.cache / "state.json").write_text(json.dumps(state))
        self.assertEqual(read_state(self.cache), state)

    def test_invalid_not_duplicates_fails_closed(self):
        self.cache.mkdir(parents=True)
        invalid = [None, [], "bad", {A: 1}, {f"{B}|{A}": 1}, {f"{A}|{A}": 1},
                   {f"{A}|{B}|{REF}": 1}, {f"{A}|pages/projects/proj.md": 1},
                   {"../outside.md|pages/knowledge/a.md": 1}, {f"{A}|": 1}]
        invalid += [{f"{A}|{B}": value} for value in (True, False, None, "123", {}, [], float("nan"), float("inf"))]
        for value in invalid:
            with self.subTest(value=value):
                text = json.dumps({"cooldowns": {}, "not_duplicates": value})
                (self.cache / "state.json").write_text(text)
                with self.assertRaises(ValueError):
                    read_state(self.cache)
                self.assertEqual((self.cache / "state.json").read_text(), text)
        code, _ = self.execute()
        self.assertEqual(code, 1)
        self.assertFalse(self.calls("claude"))


class PreservationTests(RepoTest):
    def check_change(self, old, new, explanation=None):
        self.write(A, page(old))
        parent = self.commit()
        self.write(A, page(new))
        commit = self.commit()
        return preserved(self.wiki, parent, commit, explanation or justification(dropped=[]))

    def test_number_and_code_loss_including_fences(self):
        old = "`probe --fast` needs 45s, 0.3%, 2.1.x and v1.6.0.\n```sh\necho `uname` 123ms\n```\n"
        found = tokens(page(old))
        self.assertTrue({"`probe --fast`", "45s", "0.3%", "2.1.x", "v1.6.0", "`uname`", "123ms"} <= found)
        self.assertEqual(self.check_change(old, old.replace("45s", "46s")), ["45s"])
        self.assertEqual(self.check_change(old, old.replace("`probe --fast`", "the probe")), ["`probe --fast`"])
        self.assertEqual(self.check_change(old, old.replace("123ms", "quickly")), ["123ms"])

    def test_dropped_exception_and_frontmatter_exclusion(self):
        self.assertEqual(self.check_change("Takes 45s.\n", "Takes a while.\n",
                         justification(dropped=[{"token": "45s", "reason": "obsolete measurement; reviewer must verify"}])), [])
        self.assertNotIn("2020", tokens(page("Text.\n")))
        self.assertEqual(self.check_change("https://example.invalid/path\n", "no URL\n"), ["https://example.invalid/path"])

    def test_rename_detection_normalizes_link_targets(self):
        self.write(A, page("See [[knowledge/b|the page]].\n"))
        self.write(B, page("The exact same facts.\n" * 10))
        parent = self.commit()
        target = "pages/history/2020-01-01-b.md"
        (self.wiki / target).parent.mkdir(parents=True)
        git(self.wiki, "mv", B, target)
        self.write(A, page("See [[history/2020-01-01-b|the page]].\n"))
        commit = self.commit()
        self.assertTrue(any(flag.startswith("R") for flag, _, _ in changes(self.wiki, parent, commit)))
        self.assertEqual(preserved(self.wiki, parent, commit, justification(dropped=[])), [])

    def test_numbers_in_link_targets_are_not_tokens(self):
        found = tokens(page("See [[knowledge/b-2020-01-02|the 45s page]].\n"))
        self.assertIn("45s", found)
        self.assertFalse({"2020", "01", "02"} & found)
        self.write(A, page("See [[knowledge/b-2020-01-02]].\n"))
        self.write("pages/knowledge/b-2020-01-02.md", page("The exact same facts.\n" * 10))
        parent = self.commit()
        target = "pages/history/2020-01-02-b.md"
        (self.wiki / target).parent.mkdir(parents=True)
        git(self.wiki, "mv", "pages/knowledge/b-2020-01-02.md", target)
        self.write(A, page("See [[history/2020-01-02-b]].\n"))
        commit = self.commit()
        self.assertEqual(preserved(self.wiki, parent, commit, justification(dropped=[])), [])

    def test_redirects_for_merge_and_union_of_changed_pages(self):
        self.write(A, page("Measured `first` takes 45s. [[knowledge/b]]\n"))
        self.write(B, page("Measured `second` takes 75s.\n"))
        parent = self.commit()
        self.write(A, page("Measured `first` takes 45s; measured `second` takes 75s. [[knowledge/a]]\n"))
        git(self.wiki, "rm", B)
        commit = self.commit()
        self.assertEqual(preserved(self.wiki, parent, commit, justification(dropped=[], redirects={B: A})), [])
        self.assertIn("[[knowledge/b]]", preserved(self.wiki, parent, commit, justification(dropped=[])))

    def test_cycles_reject(self):
        parent = git(self.wiki, "rev-parse", "HEAD").strip()
        self.write(A, page(NEW))
        commit = self.commit()
        with self.assertRaisesRegex(ValueError, "cyclic"):
            preserved(self.wiki, parent, commit, justification(redirects={A: B, B: A}))


class RunTests(RepoTest):
    def duplicate_fixture(self):
        self.write(A, page("Measured `probe --fast` takes 45s.\n"))
        self.write(B, page("The `cache` holds 12MiB.\n", created="2021-01-01"))
        self.write(REF, page("See [[knowledge/b|cache notes]].\n"))
        self.commit()
        self.fixture = {
            "lint": {"duplicates": [{"a": A, "b": B, "score": .96}]},
            "writes": {A: page("Measured `probe --fast` takes 45s. The `cache` holds 12MiB.\n"),
                       REF: page("See [[knowledge/a|cache notes]].\n")},
            "deletes": [B], "justification": justification(dropped=[], redirects={B: A}),
        }

    def test_duplicate_abstention_records_reason_and_suppresses_pair_for_90_days(self):
        self.duplicate_fixture()
        self.fixture["writes"] = {}
        self.fixture["deletes"] = []
        reason = "not duplicates: separate review rounds with different findings"
        self.fixture["justification"] = justification(summary=reason, dropped=[])
        self.fixture["lint"]["duplicates"][0].update(a=B, b=A)
        before = git(self.wiki, "rev-parse", "HEAD").strip()
        result = self.result()
        self.assertEqual(result["outcome"], "no-op", result)
        self.assertEqual(result["reason"], reason)
        self.assertNotIn("commit", result)
        self.assertNotIn("diff", result)
        self.assertFalse(self.calls("codex"))
        self.assertEqual(git(self.wiki, "rev-parse", "HEAD").strip(), before)
        self.assertEqual(status(self.wiki), {})
        state = read_state(self.cache)
        self.assertEqual(state["cooldowns"], {})
        self.assertEqual(list(state["not_duplicates"]), [f"{A}|{B}"])
        self.assertAlmostEqual(state["not_duplicates"][f"{A}|{B}"] - time.time(), 90 * DAY, delta=10)
        self.assertIn(reason, (self.cache / "latest.md").read_text())
        prompt = self.calls("claude")[0]["prompt"]
        self.assertIn("not duplicates: <reason>", prompt)
        self.assertIn("change no page", prompt)
        code, report = self.execute()
        self.assertEqual(code, 0, report)
        self.assertEqual(report["tasks"], [])
        self.assertEqual(len(self.calls("claude")), 1)

    def test_duplicate_noop_suppression_does_not_require_full_justification(self):
        self.duplicate_fixture()
        self.fixture["writes"] = {}
        self.fixture["deletes"] = []
        for explanation in (None, {"summary": 123}, {"summary": "not duplicates: separate systems"}):
            with self.subTest(explanation=explanation):
                (self.cache / "state.json").unlink(missing_ok=True)
                if explanation is None:
                    self.fixture.pop("justification", None)
                else:
                    self.fixture["justification"] = explanation
                result = self.result()
                self.assertEqual(result["outcome"], "no-op")
                if explanation and isinstance(explanation["summary"], str):
                    self.assertEqual(result["reason"], explanation["summary"])
                else:
                    self.assertNotIn("reason", result)
                self.assertIn(f"{A}|{B}", read_state(self.cache)["not_duplicates"])

    def test_rejected_scope_diff_includes_edits_deletions_and_new_binary_pages(self):
        self.fixture["writes"].update({B: page("A new page: 안녕하세요.\n"),
                                       "pages/knowledge/binary.md": "binary\x00payload",
                                       "outside.txt": "outside edits are not archived"})
        self.fixture["deletes"] = [REF]
        self.write("notes.txt", "unrelated staged work\n")
        git(self.wiki, "add", "notes.txt")
        self.write("notes.txt", "unrelated unstaged work\n")
        before_head = git(self.wiki, "rev-parse", "HEAD")
        before_index = git(self.wiki, "diff", "--cached", "--binary")
        before_status = status(self.wiki)
        code, report = self.execute()
        self.assertEqual(code, 0, report)
        result = report["tasks"][0]
        self.assertEqual(result["outcome"], "rejected", result)
        self.assertNotIn("commit", result)
        path = Path(result["diff"])
        self.assertEqual(path, self.cache / "runs" / f"{report['timestamp']}-1.diff")
        diff = path.read_text()
        self.assertIn("+Measured `probe --fast` takes 45s. [[knowledge/reference]]", diff)
        self.assertIn("new file mode", diff)
        self.assertIn("+A new page: 안녕하세요.", diff)
        self.assertIn(f"a/{REF}", diff)
        self.assertIn("-Reference for the probe.", diff)
        self.assertIn("GIT binary patch", diff)
        self.assertNotIn("outside.txt", diff)
        self.assertNotIn(".curator", diff)
        self.assertNotIn("notes.txt", diff)
        git(self.wiki, "apply", "--check", str(path))
        self.assertEqual(git(self.wiki, "rev-parse", "HEAD"), before_head)
        self.assertEqual(git(self.wiki, "diff", "--cached", "--binary"), before_index)
        self.assertEqual(status(self.wiki), before_status)
        self.assertEqual((self.wiki / "notes.txt").read_text(), "unrelated unstaged work\n")
        self.assertIn(str(path), next(line for line in (self.cache / "latest.md").read_text().splitlines()
                                      if line.startswith("- rejected:")))

    def test_diff_saved_for_check_and_claude_failure_with_page_changes(self):
        for failure in ({"check_exit": 2}, {"claude_exit": 1}):
            with self.subTest(failure=failure):
                self.fixture = {"lint": self.fixture["lint"], "writes": {A: page(NEW)},
                                "justification": justification(), **failure}
                (self.cache / "state.json").unlink(missing_ok=True)
                result = self.result()
                self.assertEqual(result["outcome"], "rejected")
                self.assertNotIn("commit", result)
                self.assertIn("+Measured", Path(result["diff"]).read_text())
                self.assertFalse(self.calls("codex"))
        (self.cache / "state.json").unlink()
        self.fixture["writes"] = {}
        result = self.result()
        self.assertEqual(result["outcome"], "rejected")
        self.assertNotIn("diff", result)

    def test_rejection_diff_is_capped_in_bytes_with_a_marker(self):
        self.fixture["writes"][A] = page("가" * DIFF_LIMIT)
        self.fixture["claude_exit"] = 1
        result = self.result()
        diff = Path(result["diff"]).read_bytes()
        self.assertEqual(len(diff), DIFF_LIMIT)
        self.assertTrue(diff.startswith(b"diff --git "))
        self.assertTrue(diff.endswith(DIFF_TRUNCATED))

    def test_rejection_diff_preserves_non_utf8_text_and_ignored_new_files(self):
        parent = git(self.wiki, "rev-parse", "HEAD").strip()
        worktree = self.root / "evidence-worktree"
        git(self.wiki, "worktree", "add", "--detach", str(worktree), parent)
        try:
            (worktree / A).write_bytes(b"non-UTF8 text: \xff\n")
            (worktree / ".gitignore").write_text("pages/knowledge/new.md\n")
            (worktree / "pages/knowledge/new.md").write_text("Ignored new page\n")
            destination = self.root / "evidence.diff"
            self.assertEqual(save_rejection_diff(worktree, parent, destination), str(destination))
            diff = destination.read_bytes()
            self.assertIn(b"+non-UTF8 text: \xff", diff)
            self.assertIn(b"+Ignored new page", diff)
            self.assertNotIn(b".gitignore", diff)
            git(self.wiki, "apply", "--check", str(destination))
        finally:
            git(self.wiki, "worktree", "remove", "--force", str(worktree))

    def test_merge_preserves_facts_and_updates_backlinks(self):
        self.duplicate_fixture()
        result = self.result()
        self.assertEqual(result["outcome"], "merged", result)
        self.assertFalse((self.wiki / B).exists())
        self.assertIn("[[knowledge/a|cache notes]]", (self.wiki / REF).read_text())
        self.assertIn("12MiB", (self.wiki / A).read_text())
        review = self.calls("codex")[0]["prompt"]
        self.assertIn('"old_path": "pages/knowledge/b.md"', review)
        self.assertIn('"new_path": null', review)

    def test_move_snapshot_keeps_body_and_updates_links(self):
        source = "pages/knowledge/2020-01-01-state.md"
        destination = "pages/history/2020-01-01-state.md"
        self.write(source, page("Measured `probe` uses 45s.\n"))
        self.write(REF, page("See [[knowledge/2020-01-01-state]].\n"))
        self.commit()
        self.fixture = {
            "lint": {"snapshots": [{"path": source, "detail": "dated"}]},
            "moves": {source: destination},
            "writes": {REF: page("See [[history/2020-01-01-state]].\n")},
            "justification": justification(dropped=[], redirects={source: destination}),
        }
        result = self.result()
        self.assertEqual(result["outcome"], "merged", result)
        self.assertFalse((self.wiki / source).exists())
        self.assertEqual((self.wiki / destination).read_text(), page("Measured `probe` uses 45s.\n"))

    def test_log_growth_moves_timeline_with_bidirectional_links(self):
        destination = time.strftime("pages/history/%Y-%m-%d-a-curation.md", time.gmtime())
        self.write(A, page("Measured `probe` now uses 45s.\nOld run measured 75s.\n"))
        self.commit()
        self.fixture = {
            "lint": {"log-growth": [{"path": A, "detail": "timeline"}]},
            "writes": {A: page(f"Measured `probe` now uses 45s. [[{destination[6:-3]}]]\n"),
                       destination: page("Old run measured 75s. [[knowledge/a]]\n")},
            "justification": justification(dropped=[]),
        }
        result = self.result()
        self.assertEqual(result["outcome"], "merged", result)

    def test_new_backlink_during_review_defers_merge(self):
        self.duplicate_fixture()
        self.fixture["move_main"] = {"writes": {"pages/projects/project.md": page("See [[knowledge/b]].\n")}}
        result = self.result()
        self.assertEqual(result["outcome"], "deferred", result)
        self.assertIn("new backlink", result["reason"])
        self.assertTrue((self.wiki / B).exists())

    def test_missing_backlink_update_and_redirect_fabrication_reject(self):
        self.duplicate_fixture()
        self.fixture["writes"].pop(REF)
        result = self.result()
        self.assertEqual(result["outcome"], "rejected", result)
        self.assertFalse(self.calls("codex"))
        (self.cache / "state.json").unlink()
        self.fixture["writes"][REF] = page("See [[knowledge/a|cache notes]].\n")
        self.fixture["justification"]["redirects"] = {B: REF}
        self.assertEqual(self.result()["outcome"], "rejected")

    def test_ignored_outside_edit_and_review_mutations_reject(self):
        self.write(".gitignore", (self.wiki / ".gitignore").read_text() + "ignored.txt\n")
        self.commit()
        self.fixture["writes"]["ignored.txt"] = "out of scope"
        self.assertEqual(self.result()["outcome"], "rejected")
        self.assertFalse(self.calls("codex"))
        (self.cache / "state.json").unlink()
        self.fixture["writes"].pop("ignored.txt")
        self.fixture["review_edits"] = {"writes": {A: page("Unreviewed edit.\n")}}
        self.assertEqual(self.result()["outcome"], "rejected")
        self.assertEqual((self.wiki / A).read_text(), page(OLD))

    def test_sigterm_cleans_worktree_and_records_failed_run(self):
        self.fixture["wait_for"] = str(self.root / "never-created")
        self.save_fixture()
        process = subprocess.Popen([sys.executable, "-m", "wiki_curator", "run"],
                                   stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        try:
            deadline = time.monotonic() + 10
            while not self.calls("claude") and time.monotonic() < deadline:
                time.sleep(.02)
            self.assertTrue(self.calls("claude"))
            process.terminate()
            stdout, stderr = process.communicate(timeout=10)
            self.assertEqual(process.returncode, 1, (stdout, stderr))
            self.assertFalse((self.cache / "worktree").exists())
            self.assertIn("interrupted", (self.cache / "latest.md").read_text())
        finally:
            if process.poll() is None:
                process.kill()
                process.communicate()

    def test_accepted_on_moved_main_preserves_unrelated_dirty_file(self):
        self.write("notes.txt", "user's unfinished edit\n")
        self.fixture["move_main"] = {"writes": {"pages/history/2020-01-01-other.md": page("Other session's work.\n")}}
        self.fixture["stage_justification"] = True
        os.environ["WIKI_CURATOR_MODEL"] = "fixture-model"
        result = self.result()
        self.assertEqual(result["outcome"], "merged", result)
        self.assertEqual((self.wiki / A).read_text(), page(NEW))
        self.assertEqual((self.wiki / "notes.txt").read_text(), "user's unfinished edit\n")
        self.assertEqual(status(self.wiki), {"notes.txt": " M"})
        self.assertNotEqual(result["commit"], result["landed_commit"])
        log = git(self.wiki, "log", "-1", "--format=%B")
        self.assertIn("curator: broken-links:", log)
        self.assertIn("Curator-Run:", log)
        self.assertIn(f"cherry picked from commit {result['commit']}", log)
        self.assertNotIn(".curator", git(self.wiki, "ls-tree", "-r", "--name-only", "HEAD"))
        claude = self.calls("claude")[0]
        self.assertEqual(claude["cwd"], str(self.cache / "worktree"))
        self.assertEqual(claude["wiki"], claude["cwd"])
        self.assertEqual(claude["socket"], "")
        self.assertIn("fixture-model", claude["args"])
        self.assertEqual(claude["args"][claude["args"].index("--setting-sources") + 1], "")
        self.assertIn("--no-session-persistence", claude["args"])
        self.assertIn("Bash(git mv:*)", claude["args"])
        codex = self.calls("codex")[0]
        self.assertEqual(codex["args"][:7], ["exec", "--skip-git-repo-check", "-s", "read-only", "--color", "never", "-o"])
        self.assertIn("Measured `probe --fast` takes 45s.", codex["prompt"])
        self.assertIn("reported, unverified", codex["prompt"])

    def test_preservation_failure_rejects_before_codex(self):
        self.fixture["writes"][A] = page(NEW.replace("45s", "46s"))
        result = self.result()
        self.assertEqual(result["outcome"], "rejected")
        self.assertEqual(result["gates"]["preservation"]["missing_tokens"], ["45s"])
        self.assertFalse(self.calls("codex"))
        self.assertTrue(result["branch"].startswith("curator/rejected/"))
        self.assertNotIn("diff", result)
        self.assertEqual(git(self.wiki, "rev-parse", result["branch"]).strip(), result["commit"])
        cooldown = json.loads((self.cache / "state.json").read_text())["cooldowns"][A]
        self.assertAlmostEqual(cooldown - time.time(), 14 * DAY, delta=10)
        self.assertEqual((self.wiki / A).read_text(), page(OLD))

    def test_codex_reject_invalid_nonzero_missing_and_timeout(self):
        cases = [
            {"review": '{"accept": false, "problems": ["Lost the machine qualification"]}'},
            {"review": "not JSON"}, {"codex_exit": 3}, {"no_review_file": True},
            {"review": '{"accept": "true", "problems": []}'},
            {"review": '{"accept": true, "problems": ["unsure"]}'}, {"codex_sleep": 2},
            {"review": '{"accept": false, "accept": true, "problems": []}'},
        ]
        for case in cases:
            with self.subTest(case=case):
                self.fixture = {"lint": self.fixture["lint"], "writes": {A: page(NEW)}, "justification": justification(), **case}
                (self.cache / "state.json").unlink(missing_ok=True)
                with patch("wiki_curator.runner.CODEX_TIMEOUT", .15 if "codex_sleep" in case else 10):
                    result = self.result()
                self.assertEqual(result["outcome"], "rejected", result)
                self.assertTrue(result["branch"].startswith("curator/rejected/"))
                self.assertTrue(result["codex_problems"])
        self.assertEqual((self.wiki / A).read_text(), page(OLD))

    def test_conflict_defers_and_restores_main_and_dirty_file(self):
        self.write("notes.txt", "unfinished\n")
        self.fixture["move_main"] = {"writes": {A: page("Another session replaced this entire line.\n")}}
        self.fixture["backdate_main"] = True
        result = self.result()
        self.assertEqual(result["outcome"], "deferred", result)
        self.assertIn("cherry-pick failed", result["reason"])
        self.assertEqual(git(self.wiki, "log", "-1", "--format=%s").strip(), "concurrent session")
        self.assertEqual((self.wiki / A).read_text(), page("Another session replaced this entire line.\n"))
        self.assertEqual((self.wiki / "notes.txt").read_text(), "unfinished\n")
        self.assertEqual(status(self.wiki), {"notes.txt": " M"})
        self.assertFalse((self.wiki / ".git/CHERRY_PICK_HEAD").exists())
        self.assertAlmostEqual(json.loads((self.cache / "state.json").read_text())["cooldowns"][A] - time.time(), DAY, delta=10)

    def test_staged_unrelated_changes_defer_without_touching_index(self):
        self.write("notes.txt", "staged\n")
        git(self.wiki, "add", "notes.txt")
        before = git(self.wiki, "diff", "--cached")
        result = self.result()
        self.assertEqual(result["outcome"], "deferred", result)
        self.assertEqual(git(self.wiki, "diff", "--cached"), before)
        self.assertEqual((self.wiki / "notes.txt").read_text(), "staged\n")

    def test_lock_timeout_defers(self):
        with (self.wiki / ".lock").open("a") as lock:
            fcntl.flock(lock, fcntl.LOCK_EX)
            with patch("wiki_curator.runner.LOCK_TIMEOUT", .05):
                result = self.result()
        self.assertEqual(result["outcome"], "deferred", result)
        self.assertIn("lock busy", result["reason"])

    def test_preflight_failures_do_not_review(self):
        cases = [
            {"check_exit": 2}, {"check_exit": 42}, {"justification": {"summary": "invalid"}},
            {"writes": {A: page(NEW), "outside.txt": "unexpected"}},
            {"writes": {A: page(NEW), "pages/projects/proj.md": page("Forbidden")}},
            {"writes": {A: page(NEW), REF: page("Unrelated edit")}},
            {"claude_exit": 1}, {"claude_sleep": 2},
        ]
        for case in cases:
            with self.subTest(case=case):
                self.fixture = {"lint": self.fixture["lint"], "writes": {A: page(NEW)}, "justification": justification(), **case}
                (self.cache / "state.json").unlink(missing_ok=True)
                with patch("wiki_curator.runner.CLAUDE_TIMEOUT", .15 if "claude_sleep" in case else 10):
                    result = self.result()
                self.assertEqual(result["outcome"], "rejected", result)
                self.assertFalse(self.calls("codex"))
                self.assertNotIn("commit", result)

    def test_missing_justification_and_noop(self):
        self.fixture.pop("justification")
        self.assertEqual(self.result()["outcome"], "rejected")
        (self.cache / "state.json").unlink()
        self.fixture["writes"] = {}
        self.assertEqual(self.result()["outcome"], "no-op")
        self.assertEqual(read_state(self.cache)["not_duplicates"], {})

    def test_query_probe_requires_zero(self):
        self.fixture["lint"] = {"missing-queries": [{"path": A, "detail": "missing"}]}
        self.fixture["writes"][A] = page(OLD, extra='queries: ["Why does the probe take time?", "How long does the probe take?"]\n')
        self.fixture["justification"] = justification(dropped=[])
        self.fixture["check_exit"] = 1
        result = self.result()
        self.assertEqual(result["outcome"], "rejected")
        self.assertIn("--probe", self.calls("claude-wiki")[-1]["args"])
        (self.cache / "state.json").unlink()
        self.fixture["check_exit"] = 0
        self.assertEqual(self.result()["outcome"], "merged")

    def test_cleanup_on_task_exception_and_run_failure(self):
        with patch("wiki_curator.runner.perform_task", side_effect=RuntimeError("injected task failure")):
            self.assertEqual(self.result()["outcome"], "error")
        self.fixture["lint_exit"] = 1
        code, report = self.execute()
        self.assertEqual(code, 1)
        self.assertIn("error", report)

    def test_stale_worktree_removed_before_next_run(self):
        self.cache.mkdir(parents=True)
        git(self.wiki, "worktree", "add", "--detach", str(self.cache / "worktree"), "main")
        (self.cache / "worktree/leftover").write_text("stale attempt")
        self.assertEqual(self.result()["outcome"], "merged")

    def test_corrupt_state_fails_closed(self):
        self.cache.mkdir(parents=True)
        (self.cache / "state.json").write_text("{invalid")
        code, report = self.execute()
        self.assertEqual(code, 1)
        self.assertFalse(self.calls("claude"))
        self.assertEqual((self.cache / "state.json").read_text(), "{invalid")

    def test_second_concurrent_run_exits_immediately(self):
        release = self.root / "release"
        self.fixture["wait_for"] = str(release)
        self.save_fixture()
        first = subprocess.Popen([sys.executable, "-m", "wiki_curator", "run"], env=os.environ.copy(),
                                 stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True)
        try:
            deadline = time.monotonic() + 10
            while not self.calls("claude") and time.monotonic() < deadline:
                time.sleep(.02)
            self.assertTrue(self.calls("claude"))
            start = time.monotonic()
            second = subprocess.run([sys.executable, "-m", "wiki_curator", "run"],
                                    capture_output=True, text=True, timeout=3)
            self.assertEqual(second.returncode, 0, second.stderr)
            self.assertIn("already running", second.stdout)
            self.assertLess(time.monotonic() - start, 3)
        finally:
            release.touch()
            stdout, stderr = first.communicate(timeout=15)
        self.assertEqual(first.returncode, 0, (stdout, stderr))
        self.assertEqual(len(list((self.cache / "runs").glob("*.json"))), 1)
        self.assertFalse((self.cache / "worktree").exists())

    def test_cli_env_defaults_only_status_and_debug_preservation(self):
        os.environ["WIKI_CURATOR_MAX_TASKS"] = "0"
        response = subprocess.run([sys.executable, "-m", "wiki_curator", "run", "--only", "broken-links"],
                                  capture_output=True, text=True, timeout=10)
        self.assertEqual(response.returncode, 0, response.stderr)
        self.assertIn("No eligible tasks", response.stdout)
        self.assertFalse(self.calls("claude"))
        self.fixture["review"] = '{"accept": false, "problems": ["uncertain"]}'
        result = self.result()
        response = subprocess.run([sys.executable, "-m", "wiki_curator", "status"], capture_output=True, text=True)
        self.assertEqual(response.returncode, 0, response.stderr)
        self.assertIn(result["branch"], response.stdout)
        explanation = self.root / "justification.json"
        explanation.write_text(json.dumps(justification()))
        response = subprocess.run([sys.executable, "-m", "wiki_curator", "check-preservation",
                                   result["parent_commit"], result["commit"], "--justification", str(explanation)],
                                  cwd=self.wiki, capture_output=True, text=True)
        self.assertEqual(response.returncode, 0, response.stderr)
        self.assertEqual(json.loads(response.stdout), {"missing_tokens": []})


if __name__ == "__main__":
    unittest.main()
