from datetime import datetime, timedelta, timezone
import fcntl
import json
import os
import subprocess
import sys
import time
from unittest.mock import patch

from test_curator import A, B, DAY, NEW, OLD, REF, RepoTest, page
from wiki_curator.gitutil import git, status
from wiki_curator.review import DIFF_LIMIT, DIFF_TRUNCATED, land_branch
from wiki_curator.runner import read_state, write_json


class ReviewTests(RepoTest):
    def timestamp(self, days=0):
        return (datetime.now(timezone.utc) - timedelta(days=days)).strftime("%Y%m%dT%H%M%S.%fZ")

    def cli(self, *args, code=0, binary=False):
        result = subprocess.run([sys.executable, "-m", "wiki_curator", *args],
                                capture_output=True, text=not binary, timeout=10)
        self.assertEqual(result.returncode, code, (result.stdout, result.stderr))
        return result.stdout

    def report(self, timestamp, *tasks):
        (self.cache / "runs").mkdir(parents=True, exist_ok=True)
        write_json(self.cache / "runs" / f"{timestamp}.json", {"tasks": list(tasks)})

    def diff(self, timestamp=None, number=1, content=b"rejected patch\n"):
        timestamp = timestamp or self.timestamp()
        path = self.cache / "runs" / f"{timestamp}-{number}.diff"
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(content)
        return f"diff:{path.stem}", path

    def candidate(self, *, outcome="rejected", commits=1, edits=None, deleted=(), timestamp=None):
        timestamp = timestamp or self.timestamp()
        branch = f"curator/{outcome}/{timestamp}-1"
        worktree = self.root / "candidate"
        git(self.wiki, "worktree", "add", "--detach", str(worktree), "main")
        try:
            for number in range(commits):
                for path, text in (edits or {A: page(NEW)}).items():
                    full = worktree / path
                    full.parent.mkdir(parents=True, exist_ok=True)
                    full.write_text(text + (f"\nCommit {number}\n" if commits > 1 else ""))
                for path in deleted:
                    (worktree / path).unlink(missing_ok=True)
                git(worktree, "add", "-A")
                git(worktree, "commit", "-m", f"candidate {number}")
            commit = git(worktree, "rev-parse", "HEAD").strip()
            git(self.wiki, "branch", branch, commit)
        finally:
            git(self.wiki, "worktree", "remove", "--force", str(worktree))
        return branch, commit

    def test_review_joins_reports_lists_all_branches_and_ignores_legacy_diffs(self):
        self.fixture["review"] = '{"accept": false, "problems": ["Lost qualification", "Check the number"]}'
        result = self.result()
        branch = result["branch"]
        deferred, _ = self.candidate(outcome="deferred", timestamp=self.timestamp(45))
        self.report(deferred.rsplit("/", 1)[1].rsplit("-", 1)[0],
                    {"type": "snapshots", "pages": [B], "reason": "main index has staged changes"})
        missing, missing_commit = self.candidate(timestamp=self.timestamp(1))
        self.diff()
        self.diff(self.timestamp(31))
        dismissed_id, _ = self.diff()
        state = read_state(self.cache)
        state["dismissed_diffs"] = [dismissed_id]
        write_json(self.cache / "state.json", state)
        items = json.loads(self.cli("review", "--json"))["items"]
        self.assertEqual([i["id"] for i in items], [branch, missing, deferred])
        self.assertEqual(set(items[0]), {"id", "kind", "branch", "run", "task", "type", "pages",
                                       "outcome", "reason", "codex_problems", "adjudicate", "summary", "commit"})
        self.assertTrue(all(i["kind"] == "branch" and "diff" not in i for i in items))
        self.assertEqual(items[0]["codex_problems"], ["Lost qualification", "Check the number"])
        self.assertEqual(items[0]["commit"], result["commit"])
        self.assertEqual(items[0]["summary"], result["justification"]["summary"])
        self.assertEqual(items[1]["commit"], missing_commit)
        for key in ("type", "reason", "summary"):
            self.assertIsNone(items[1][key])
        self.assertEqual(items[1]["pages"], [])
        self.assertEqual(items[2]["outcome"], "deferred")
        text = self.cli("review")
        self.assertIn("  Lost qualification\n  Check the number\n", text)
        self.assertEqual(len([line for line in text.splitlines() if not line.startswith("  ")]), 3)
        self.assertEqual(len(list((self.cache / "runs").glob("*.diff"))), 3)

    def test_review_missing_corrupt_and_malformed_reports(self):
        branch, _ = self.candidate()
        timestamp = branch.split("/")[-1].rsplit("-", 1)[0]
        self.report(timestamp, {})
        report_path = self.cache / "runs" / f"{timestamp}.json"
        invalid = [b"{", b"\xff", b"[]", b'{"tasks": null}', b'{"tasks": [null]}', b'{"tasks": []}',
                   b'{"tasks": [{"type": {}, "pages": [1], "reason": 2, "codex_problems": {}, "justification": []}]}',
                   b'{"tasks": [], "tasks": []}', b"[" * 2000]
        for content in invalid:
            with self.subTest(content=content[:100]):
                report_path.write_bytes(content)
                entry = json.loads(self.cli("review", "--json"))["items"][0]
                self.assertIsNone(entry["type"])
                self.assertEqual(entry["pages"], [])
                self.assertIsNone(entry["reason"])
                self.assertEqual(entry["codex_problems"], [])
                self.assertIsNone(entry["summary"])
        git(self.wiki, "branch", "curator/rejected/manual", branch)
        entries = json.loads(self.cli("review", "--json"))["items"]
        manual = next(i for i in entries if i["id"].endswith("/manual"))
        self.assertIsNone(manual["run"])
        self.assertIsNone(manual["task"])

    def test_review_empty_does_not_create_cache(self):
        self.assertEqual(json.loads(self.cli("review", "--json")), {"items": []})
        self.assertFalse(self.cache.exists())

    def test_show_branch_patch_stat_and_byte_cap(self):
        branch, commit = self.candidate()
        shown = self.cli("show", branch)
        self.assertIn(commit, shown)
        self.assertIn("1 file changed", shown)
        self.assertIn("+Measured", shown)
        large, _ = self.candidate(edits={A: page("가" * DIFF_LIMIT)})
        content = self.cli("show", large, binary=True)
        self.assertEqual(len(content), DIFF_LIMIT)
        self.assertTrue(content.endswith(DIFF_TRUNCATED))

    def test_land_rejected_merges_deletes_branch_and_clears_only_task_cooldowns(self):
        self.fixture["review"] = '{"accept": false, "problems": ["Human review requested"]}'
        result = self.result()
        timestamp = result["branch"].rsplit("/", 1)[1].rsplit("-", 1)[0]
        result["pages"].append(B)  # A task page need not have changed in the commit.
        self.report(timestamp, result)
        state = read_state(self.cache)
        state["cooldowns"].update({B: time.time() + DAY, REF: time.time() + DAY})
        state["not_duplicates"] = {f"{A}|{B}": time.time() + DAY}
        write_json(self.cache / "state.json", state)
        self.write("pages/history/unrelated.md", page("Concurrent committed work.\n"))
        self.commit("concurrent main")
        self.write("notes.txt", "unfinished\n")
        response = json.loads(self.cli("land", result["branch"], "--json"))
        self.assertEqual(response, {"outcome": "merged", "landed_commit": git(self.wiki, "rev-parse", "HEAD").strip()})
        self.assertEqual((self.wiki / A).read_text(), page(NEW))
        self.assertIn(f"cherry picked from commit {result['commit']}", git(self.wiki, "log", "-1", "--format=%B"))
        self.assertEqual(status(self.wiki), {"notes.txt": " M"})
        self.assertEqual((self.wiki / "notes.txt").read_text(), "unfinished\n")
        self.assertEqual(read_state(self.cache)["cooldowns"], {REF: state["cooldowns"][REF]})
        self.assertEqual(read_state(self.cache)["not_duplicates"], state["not_duplicates"])
        self.assertEqual(json.loads(self.cli("review", "--json")), {"items": []})

    def test_land_deferred_without_report(self):
        branch, _ = self.candidate(outcome="deferred")
        self.cache.mkdir(parents=True)
        write_json(self.cache / "state.json", {"cooldowns": {A: time.time() + DAY}})
        self.assertEqual(json.loads(self.cli("land", branch, "--json"))["outcome"], "merged")
        self.assertEqual(read_state(self.cache)["cooldowns"], {})

    def test_land_defers_dirty_staged_recent_and_non_main_without_mutations(self):
        branch, _ = self.candidate()
        self.cache.mkdir(parents=True)
        write_json(self.cache / "state.json", {"cooldowns": {A: time.time() + DAY}})
        before_state = (self.cache / "state.json").read_bytes()
        before_head = git(self.wiki, "rev-parse", "main")
        for kind in ("dirty", "staged", "recent", "checkout", "operation"):
            with self.subTest(kind=kind):
                if kind == "dirty":
                    self.write(A, page("Unfinished\n"))
                elif kind == "staged":
                    self.write("notes.txt", "staged\n")
                    git(self.wiki, "add", "notes.txt")
                elif kind == "recent":
                    os.utime(self.wiki / A, None)
                elif kind == "checkout":
                    git(self.wiki, "checkout", "--detach", "main")
                else:
                    (self.wiki / ".git/MERGE_HEAD").write_text(before_head)
                before_index = git(self.wiki, "diff", "--cached", "--binary")
                before_dirty = git(self.wiki, "diff", "--binary")
                response = json.loads(self.cli("land", branch, "--json", code=3))
                self.assertEqual(response["outcome"], "deferred")
                self.assertEqual(git(self.wiki, "rev-parse", "main"), before_head)
                self.assertEqual(git(self.wiki, "diff", "--cached", "--binary"), before_index)
                self.assertEqual(git(self.wiki, "diff", "--binary"), before_dirty)
                self.assertEqual((self.cache / "state.json").read_bytes(), before_state)
                self.assertTrue(git(self.wiki, "show-ref", "--verify", f"refs/heads/{branch}"))
                (self.wiki / ".git/MERGE_HEAD").unlink(missing_ok=True)
                git(self.wiki, "reset", "--hard", "main")
                git(self.wiki, "checkout", "main")
                self.age()

    def test_land_checks_new_and_dirty_backlinks_without_report(self):
        self.write(B, page("Merge this fact.\n"))
        self.commit()
        branch, _ = self.candidate(edits={A: page(NEW + "Merge this fact.\n")}, deleted=[B])
        self.write(REF, page("New link to [[knowledge/b]].\n"))
        self.commit("new backlink")
        response = json.loads(self.cli("land", branch, "--json", code=3))
        self.assertIn("new backlink", response["reason"])
        self.write(REF, page("Reference.\n"))
        self.commit()
        self.write("pages/knowledge/new.md", page("Untracked [[knowledge/b]].\n"))
        response = json.loads(self.cli("land", branch, "--json", code=3))
        self.assertIn("dirty backlink", response["reason"])
        self.assertTrue((self.wiki / B).exists())

    def test_land_conflict_aborts_and_preserves_unrelated_work(self):
        branch, _ = self.candidate()
        self.write(A, page("A conflicting replacement.\n"))
        before = self.commit("concurrent main")
        self.write("notes.txt", "unfinished\n")
        response = json.loads(self.cli("land", branch, "--json", code=3))
        self.assertIn("cherry-pick failed", response["reason"])
        self.assertEqual(git(self.wiki, "rev-parse", "HEAD").strip(), before)
        self.assertEqual(status(self.wiki), {"notes.txt": " M"})
        self.assertEqual((self.wiki / "notes.txt").read_text(), "unfinished\n")
        self.assertEqual((self.wiki / A).read_text(), page("A conflicting replacement.\n"))
        self.assertFalse((self.wiki / ".git/CHERRY_PICK_HEAD").exists())

    def test_land_wiki_lock_timeout(self):
        branch, _ = self.candidate()
        with (self.wiki / ".lock").open("a") as stream:
            fcntl.flock(stream, fcntl.LOCK_EX)
            with patch("wiki_curator.runner.LOCK_TIMEOUT", .02):
                result = land_branch(self.wiki, self.cache, branch)
        self.assertEqual(result["outcome"], "deferred")
        self.assertIn("lock busy", result["reason"])

    def test_land_refuses_multi_commit_merge_and_already_landed_branches(self):
        multiple, _ = self.candidate(commits=2)
        for branch in (multiple, "main", "curator/rejected/missing", "curator/rejected/../../main"):
            with self.subTest(branch=branch):
                response = json.loads(self.cli("land", branch, "--json", code=2))
                self.assertEqual(response["outcome"], "refused")
                self.assertTrue(response["reason"])
        branch, commit = self.candidate()
        git(self.wiki, "branch", "curator/rejected/already", "main")
        self.cli("land", "curator/rejected/already", code=2)
        # A merge with two parents must not be accepted even if both are on main.
        root = git(self.wiki, "rev-parse", "main").strip()
        tree = git(self.wiki, "rev-parse", f"{commit}^{{tree}}").strip()
        merge = git(self.wiki, "commit-tree", tree, "-p", root, "-p", commit, input="merge\n").strip()
        git(self.wiki, "branch", "curator/deferred/merge", merge)
        self.cli("land", "curator/deferred/merge", code=2)
        self.cli("land", branch + "^", code=2)
        self.assertEqual((self.wiki / A).read_text(), page(OLD))

    def test_discard_branch_preserves_files_and_cooldowns(self):
        branch, _ = self.candidate(outcome="deferred")
        _, path = self.diff()
        write_json(self.cache / "state.json", {"cooldowns": {A: time.time() + DAY}})
        before = (self.cache / "state.json").read_bytes()
        self.assertEqual(json.loads(self.cli("discard", branch, "--json")), {"outcome": "discarded"})
        self.assertEqual((self.cache / "state.json").read_bytes(), before)
        self.assertEqual(path.read_bytes(), b"rejected patch\n")
        self.assertEqual((self.wiki / A).read_text(), page(OLD))
        self.assertEqual(json.loads(self.cli("review", "--json")), {"items": []})

    def test_land_refuses_checked_out_candidate_before_mutating_main(self):
        branch, _ = self.candidate()
        worktree = self.root / "human-review"
        git(self.wiki, "worktree", "add", str(worktree), branch)
        before = git(self.wiki, "rev-parse", "HEAD")
        response = json.loads(self.cli("land", branch, "--json", code=2))
        self.assertIn("checked out", response["reason"])
        self.assertEqual(git(self.wiki, "rev-parse", "HEAD"), before)
        self.assertFalse((self.cache / "state.json").exists())

    def test_symbolic_curator_refs_cannot_target_other_branches(self):
        branch, commit = self.candidate()
        alias = "curator/rejected/alias"
        git(self.wiki, "symbolic-ref", f"refs/heads/{alias}", f"refs/heads/{branch}")
        for command in ("land", "discard"):
            response = json.loads(self.cli(command, alias, "--json", code=2))
            self.assertIn("symbolic ref", response["reason"])
        self.assertEqual(git(self.wiki, "rev-parse", branch).strip(), commit)

    def test_invalid_ids_and_legacy_diff_ids_are_refused(self):
        identity, path = self.diff()
        invalid = ["main", "refs/heads/main", "curator/rejected/no-such-branch", "curator/other/id",
                   "diff:../../outside", "diff:20261301T000000.000000Z-1", "diff:20260101T000000.000000Z-0", identity]
        for command in ("show", "land", "discard"):
            for value in invalid:
                with self.subTest(command=command, value=value):
                    self.cli(command, value, code=2)
        self.assertEqual(path.read_bytes(), b"rejected patch\n")
        self.assertFalse((self.cache / "state.json").exists())

    def test_mutations_refuse_run_lock_but_readers_do_not_lock(self):
        branch, _ = self.candidate()
        self.cache.mkdir(parents=True)
        with (self.cache / "run.lock").open("a") as lock:
            fcntl.flock(lock, fcntl.LOCK_EX)
            for args in (("land", branch), ("discard", branch)):
                response = json.loads(self.cli(*args, "--json", code=3))
                self.assertEqual(response, {"outcome": "deferred", "reason": "curator run in progress"})
            self.assertEqual(len(json.loads(self.cli("review", "--json"))["items"]), 1)
            self.assertIn("+Measured", self.cli("show", branch))
        self.assertFalse((self.cache / "state.json").exists())
