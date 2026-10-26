from contextlib import contextmanager
import json
import os
import socket
import subprocess
import threading
import time
import unittest
from unittest.mock import patch

from test_curator import A, B, DAY, REF, NEW, OLD, RepoTest, justification, page
from wiki_curator.gitutil import changes, git, status
from wiki_curator.recall import dense_ranks, queries, recall
from wiki_curator.runner import new_broken_links, read_state, recall_socket


QUERY = "Why does the probe take time?"
EXTRA = "queries: " + json.dumps([QUERY]) + "\n"


@contextmanager
def fake_daemon(path, response):
    requests, errors = [], []
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as listener:
        listener.bind(str(path))
        listener.listen(1)
        listener.settimeout(5)

        def serve():
            try:
                with listener.accept()[0] as connection:
                    connection.settimeout(5)
                    with connection.makefile("rb") as stream:
                        requests.append(json.loads(stream.readline()))
                    data = response if isinstance(response, bytes) else (json.dumps(response) + "\n").encode()
                    connection.sendall(data)
            except Exception as error:
                errors.append(error)

        thread = threading.Thread(target=serve, daemon=True)
        thread.start()
        try:
            yield requests
        finally:
            thread.join(timeout=6)
            path.unlink()
        if errors:
            raise errors[0]


class SocketTests(unittest.TestCase):
    def test_environment_resolution(self):
        for env, expected in (({}, None), ({"XDG_RUNTIME_DIR": "/runtime"}, "/runtime/claude-wiki/embed.sock"),
                              ({"XDG_RUNTIME_DIR": "/runtime", "CLAUDE_WIKI_SOCKET": ""}, None),
                              ({"XDG_RUNTIME_DIR": "/runtime", "CLAUDE_WIKI_SOCKET": "/custom"}, "/custom")):
            with self.subTest(env=env), patch.dict(os.environ, env, clear=True):
                self.assertEqual(recall_socket(), expected)

    def test_dense_timeout_is_120_seconds(self):
        with patch("wiki_curator.recall.socket.socket") as factory:
            connection = factory.return_value.__enter__.return_value
            connection.connect.side_effect = socket.timeout("timed out")
            with self.assertRaises(socket.timeout):
                dense_ranks("/fake", [QUERY], {"parent": {}, "commit": {}})
            connection.settimeout.assert_called_once_with(120)


class QueryTests(unittest.TestCase):
    def test_lists_and_exact_quoting(self):
        for field in (
            'queries: ["error: a, b # c", \'it\'\'s broken\', 캐시 오류]\n',
            'queries:\n  - "error: a, b # c" # comment\n  - \'it\'\'s broken\'\n  - 캐시 오류\n',
            'queries:\n- "error: a, b # c"\n- \'it\'\'s broken\'\n- 캐시 오류\ntags: [test]\n',
        ):
            with self.subTest(field=field):
                self.assertEqual(queries(page("queries: [body is not metadata]", extra=field)),
                                 ["error: a, b # c", "it's broken", "캐시 오류"])
        self.assertEqual(queries(page("queries: [body is not metadata]")), [])
        self.assertEqual(queries(page("", extra="queries: []\n")), [])
        self.assertEqual(queries(page("", extra='queries: ["say \\"hello\\"", "\\uD55C"]\n')),
                         ['say "hello"', "한"])
        self.assertEqual(queries(page("", extra='"quer\\u0069es": ["two\\nlines"]\n')),
                         ["two\nlines"])

    def test_ambiguous_or_invalid_queries_fail_closed(self):
        for field in ("queries: [true]\n", "queries: [42]\n", "queries: *alias\n",
                      "queries: [\"\"]\n", "queries: []\nqueries: []\n",
                      "queries:\n  - |\n    question\n", "  queries: [why]\n",
                      '{queries: ["hidden probe"]}\n'):
            with self.subTest(field=field), self.assertRaises(ValueError):
                queries(page("", extra=field))


class RecallTests(RepoTest):
    def parent(self):
        self.write(A, page(OLD, extra=EXTRA))
        parent = self.commit()
        self.fixture["parent_commit"] = parent
        return parent

    def gate(self, parent, commit, redirects=None, socket_path=None):
        self.save_fixture()
        try:
            return recall(self.wiki, parent, commit, justification(redirects=redirects or {}),
                          {**self.env, "CLAUDE_WIKI_SOCKET": ""}, socket_path)
        finally:
            self.assertEqual(git(self.wiki, "rev-parse", "HEAD").strip(), commit)
            self.assertEqual(status(self.wiki), {})

    def test_ranks_pass_regress_and_report_existing_failures(self):
        parent = self.parent()
        # Removing queries on a modified page must not remove the parent probes.
        self.write(A, page(NEW))
        commit = self.commit()
        for before, after, accept in ((1, 3, True), (3, 4, False), (2, None, False),
                                      (4, None, True), (None, None, True), (10, 1, True)):
            def hits(rank):
                return [REF] * (rank - 1) + [A] if rank else []
            with self.subTest(before=before, after=after):
                self.fixture["search"] = {"parent": {QUERY: hits(before)}, "commit": {QUERY: hits(after)}}
                result = self.gate(parent, commit)
                self.assertEqual(result["accept"], accept, result)
                self.assertEqual(result["probes"], [{"page": A, "expected_page": A, "query": QUERY,
                                                      "parent_rank": before, "commit_rank": after,
                                                      "parent_dense": None, "commit_dense": None}])
                self.assertEqual(result["dense"], "unavailable: no socket configured")
                self.assertEqual(result["ungated"], result["probes"] if before is None else [])
        for call in self.calls("claude-wiki"):
            self.assertEqual(call["args"], ["search", "--json", "--no-semantic", "-n", "10", "--", *QUERY.split()])
            self.assertEqual(call["wiki"], str(self.wiki))
            self.assertEqual(call["socket"], "")
        self.assertEqual({c["head"] for c in self.calls()}, {parent, commit})

    def test_added_page_queries_have_no_parent_rank(self):
        parent = self.parent()
        self.write(B, page("New topic.", extra=EXTRA))
        commit = self.commit()
        self.fixture["search"] = {"parent": {QUERY: [A]}, "commit": {QUERY: [B]}}
        result = self.gate(parent, commit)
        self.assertTrue(result["accept"])
        self.assertEqual(result["probes"], [{"page": B, "expected_page": B, "query": QUERY,
                                              "parent_rank": None, "commit_rank": 1,
                                              "parent_dense": None, "commit_dense": None}])

    def test_korean_dense_regression_and_survival_with_one_request(self):
        query = "검색 캐시 오류를 어떻게 해결하나요?"
        old = page(OLD, extra="queries: " + json.dumps([query, QUERY, query]) + "\n")
        self.write(A, old)
        parent = self.commit()
        self.fixture["parent_commit"] = parent
        self.write(A, page(NEW))
        commit = self.commit()
        for after, accept in ((["knowledge/reference", "knowledge/b", "history/c", "knowledge/a"], False),
                              (["knowledge/b", "knowledge/a"], True), ([], False)):
            response = {"ok": True, "tier": "gpu", "results": {
                "parent": [["knowledge/a"], []], "commit": [after, []]}}
            with self.subTest(after=after), fake_daemon(self.root / "embed.sock", response) as requests:
                result = self.gate(parent, commit, socket_path=self.root / "embed.sock")
            self.assertEqual(result["accept"], accept, result)
            self.assertEqual(result["dense"], "ok")
            self.assertNotIn("ungated", result)
            rank = after.index("knowledge/a") + 1 if "knowledge/a" in after else None
            self.assertEqual(result["probes"][0]["parent_dense"], 1)
            self.assertEqual(result["probes"][0]["commit_dense"], rank)
            self.assertIsNone(result["probes"][0]["parent_rank"])
            self.assertEqual(result["probes"][2], result["probes"][0])
            if not accept:
                self.assertIn(f"(bm25 None→None, dense 1→{rank})", result["problems"][0])
            self.assertEqual(requests, [{"op": "probe", "queries": [query, QUERY], "n": 10,
                                        "states": {"parent": {A: old}, "commit": {A: page(NEW)}}}])

    def test_either_method_can_keep_a_probe_in_the_top_three(self):
        parent = self.parent()
        self.write(A, page(NEW))
        commit = self.commit()
        for bm_before, bm_after, dense_before, dense_after in (([A], [], [], ["knowledge/a"]),
                                                              ([], [A], ["knowledge/a"], [])):
            self.fixture["search"] = {"parent": {QUERY: bm_before}, "commit": {QUERY: bm_after}}
            response = {"ok": True, "tier": "cpu", "results": {
                "parent": [dense_before], "commit": [dense_after]}}
            with fake_daemon(self.root / "embed.sock", response):
                self.assertTrue(self.gate(parent, commit, socket_path=self.root / "embed.sock")["accept"])

    def test_dense_overrides_renamed_added_and_deleted_pages(self):
        parent = self.parent()
        old, reference = (self.wiki / A).read_text(), (self.wiki / REF).read_text()
        added = "pages/followups/added.md"
        git(self.wiki, "mv", A, B)
        git(self.wiki, "rm", REF)
        self.write(added, page("Added page.", extra=EXTRA))
        commit = self.commit()
        response = {"ok": True, "tier": "cpu", "results": {
            "parent": [["knowledge/a"]], "commit": [["knowledge/b", "followups/added"]]}}
        with fake_daemon(self.root / "embed.sock", response) as requests:
            result = self.gate(parent, commit, socket_path=self.root / "embed.sock")
        self.assertTrue(result["accept"], result)
        self.assertEqual(requests[0]["states"], {
            "parent": {A: old, B: None, REF: reference, added: None},
            "commit": {A: None, B: old, REF: None, added: page("Added page.", extra=EXTRA)}})
        moved = next(p for p in result["probes"] if p["page"] == A)
        self.assertEqual((moved["expected_page"], moved["parent_dense"], moved["commit_dense"]), (B, 1, 1))

    def test_unavailable_or_malformed_dense_response_falls_back_without_partial_ranks(self):
        parent = self.parent()
        self.write(A, page(NEW))
        commit = self.commit()
        response = {"ok": True, "tier": "cpu", "results": {
            "parent": [["knowledge/a"]], "commit": [[]]}}
        invalid = [b"not json\n", b"{}", b"[]\n", b"\xff\n", b"{\"ok\":true,\"ok\":false}\n",
                   b"[" * 2000 + b"]" * 2000 + b"\n",
                   {"ok": False, "error": "models/index are warming up; retry shortly"},
                   {"ok": 1}, {**response, "tier": "other"}, {**response, "results": []}]
        for ranks in ([], [None], [[42]], [["pages/knowledge/a.md"]],
                      [["knowledge/a", "knowledge/a"]], [[f"knowledge/p{i}" for i in range(11)]]):
            invalid.append({**response, "results": {**response["results"], "commit": ranks}})
        invalid.append({**response, "results": {"parent": [["knowledge/a"]]}})
        for payload in invalid:
            with self.subTest(payload=payload), fake_daemon(self.root / "embed.sock", payload):
                result = self.gate(parent, commit, socket_path=self.root / "embed.sock")
            self.assertTrue(result["accept"], result)
            self.assertTrue(result["dense"].startswith("unavailable: "))
            self.assertEqual(result["ungated"], result["probes"])
            self.assertIsNone(result["probes"][0]["parent_dense"])
        result = self.gate(parent, commit, socket_path=self.root / "missing.sock")
        self.assertTrue(result["accept"], result)
        self.assertTrue(result["dense"].startswith("unavailable: "))
        with patch("wiki_curator.recall.dense_ranks", side_effect=socket.timeout("timed out")):
            result = self.gate(parent, commit, socket_path=self.root / "embed.sock")
        self.assertEqual(result["dense"], "unavailable: timed out")
        self.fixture["search"] = {"parent": {QUERY: [A]}, "commit": {QUERY: []}}
        result = self.gate(parent, commit, socket_path=self.root / "missing.sock")
        self.assertFalse(result["accept"], result)
        self.assertEqual(result["ungated"], [])

    def test_runner_dense_uses_own_socket_while_cli_socket_stays_disabled(self):
        query = "검색 캐시 오류"
        self.write(A, page(OLD, extra="queries: " + json.dumps([query]) + "\n"))
        parent = self.commit()
        self.fixture["parent_commit"] = parent
        path = self.root / "embed.sock"
        for hits, outcome in (([], "rejected"), (["knowledge/a"], "merged")):
            response = {"ok": True, "tier": "cpu", "results": {
                "parent": [["knowledge/a"]], "commit": [hits]}}
            with fake_daemon(path, response), patch.dict(os.environ, {"CLAUDE_WIKI_SOCKET": str(path)}):
                result = self.result()
            self.assertEqual(result["outcome"], outcome, result)
            self.assertEqual(result["gates"]["recall"]["dense"], "ok")
            if outcome == "rejected":
                self.assertEqual(result["reason"], "recall check failed")
                self.assertFalse(self.calls("codex"))
                (self.cache / "state.json").unlink()
        for call in self.calls("claude-wiki"):
            if call["args"][0] == "search":
                self.assertEqual(call["socket"], "")

    def test_git_rename_maps_queries_to_new_page(self):
        parent = self.parent()
        git(self.wiki, "mv", A, B)
        commit = self.commit()
        self.assertTrue(changes(self.wiki, parent, commit)[0][0].startswith("R"))
        self.fixture["search"] = {"parent": {QUERY: [A]}, "commit": {QUERY: [B]}}
        result = self.gate(parent, commit)
        self.assertTrue(result["accept"], result)
        self.assertEqual(result["probes"][0]["expected_page"], B)
        self.assertEqual(result["probes"][0]["commit_rank"], 1)

    def test_redirect_merge_requires_exact_queries_even_if_already_failing(self):
        parent = self.parent()
        git(self.wiki, "rm", A)
        self.write(REF, page("Merged facts.", extra=EXTRA))
        commit = self.commit()
        self.fixture["search"] = {"parent": {QUERY: [A]}, "commit": {QUERY: [REF]}}
        result = self.gate(parent, commit, {A: REF})
        self.assertTrue(result["accept"], result)
        self.assertEqual(result["probes"][0]["expected_page"], REF)
        self.write(REF, page("Merged facts.", extra='queries: ["Different wording"]\n'))
        commit = self.commit()
        self.fixture["search"] = {}
        result = self.gate(parent, commit, {A: REF})
        self.assertFalse(result["accept"])
        self.assertIn("missing from", " ".join(result["problems"]))

    def test_rename_missing_queries_fails(self):
        self.write(A, page("Same body.\n" * 40, extra=EXTRA))
        parent = self.commit()
        self.fixture["parent_commit"] = parent
        git(self.wiki, "mv", A, B)
        self.write(B, page("Same body.\n" * 40))
        commit = self.commit()
        self.assertTrue(changes(self.wiki, parent, commit)[0][0].startswith("R"))
        result = self.gate(parent, commit)
        self.assertFalse(result["accept"])
        self.assertIn("missing from", result["problems"][0])

    def test_deleted_page_with_queries_without_redirect_fails(self):
        parent = self.parent()
        git(self.wiki, "rm", A)
        commit = self.commit()
        result = self.gate(parent, commit)
        self.assertFalse(result["accept"])
        self.assertIsNone(result["probes"][0]["expected_page"])
        self.assertIn("no surviving redirect", result["problems"][0])

    def test_search_errors_restore_commit(self):
        parent = self.parent()
        self.write(A, page(NEW))
        commit = self.commit()
        for revision in ("parent", "commit"):
            for failure, error in (({"search_exit": {revision: 1}}, RuntimeError),
                                   ({"search_raw": {revision: '{"hits":null}'}}, ValueError)):
                with self.subTest(failure=failure), self.assertRaises(error):
                    self.fixture.pop("search_exit", None)
                    self.fixture.pop("search_raw", None)
                    self.fixture.update(failure)
                    self.gate(parent, commit)

    def test_timeout_restores_commit(self):
        from unittest.mock import patch
        parent = self.parent()
        self.write(A, page(NEW))
        commit = self.commit()
        with patch("wiki_curator.recall.command", side_effect=subprocess.TimeoutExpired("search", 120)):
            with self.assertRaises(subprocess.TimeoutExpired):
                self.gate(parent, commit)

    def test_runner_passes_recall_to_codex_and_rejects_regression_first(self):
        parent = self.parent()
        self.fixture["search"] = {"parent": {QUERY: [A]}, "commit": {QUERY: []}}
        result = self.result()
        self.assertEqual(result["outcome"], "rejected", result)
        self.assertFalse(result["gates"]["recall"]["accept"])
        self.assertEqual(result["gates"]["recall"]["probes"][0]["parent_rank"], 1)
        self.assert_not_retained(result)
        self.assertAlmostEqual(read_state(self.cache)["cooldowns"][A] - time.time(), 14 * DAY, delta=10)
        latest = (self.cache / "latest.md").read_text()
        self.assertIn("gate: recall", latest)
        self.assertTrue(result["gates"]["recall"]["problems"])
        for problem in result["gates"]["recall"]["problems"]:
            self.assertIn(problem, latest)
        self.assertFalse(self.calls("codex"))
        self.assertEqual(git(self.wiki, "rev-parse", "HEAD").strip(), parent)
        (self.cache / "state.json").unlink()
        self.fixture["search"]["commit"][QUERY] = [A]
        result = self.result()
        self.assertEqual(result["outcome"], "merged", result)
        codex = self.calls("codex")[0]
        self.assertEqual(codex["head"], result["commit"])
        self.assertIn('"recall": {', codex["prompt"])
        self.assertIn('"parent_rank": 1', codex["prompt"])


class BrokenLinkTests(RepoTest):
    def test_new_broken_link_rejects_before_check_or_codex(self):
        self.fixture["writes"][A] = page(NEW + "[[knowledge/new-missing|label]]\n")
        self.fixture["check_exit"] = 1
        result = self.result()
        self.assertEqual(result["outcome"], "rejected", result)
        self.assertEqual(result["gates"]["broken-links"]["links"],
                         [{"page": A, "target": "pages/knowledge/new-missing.md"}])
        self.assertNotIn("commit", result)
        self.assertFalse(self.calls("codex"))
        self.assertFalse(any(c["args"][0] == "check" for c in self.calls("claude-wiki")))

    def test_preexisting_broken_link_and_other_check_warnings_pass(self):
        self.fixture["writes"][A] = page(OLD + "More explanation.\n")
        self.fixture["check_exit"] = 1
        result = self.result()
        self.assertEqual(result["outcome"], "merged", result)
        self.assertEqual(result["gates"]["broken-links"], {"accept": True, "links": []})

    def test_added_page_cannot_borrow_another_pages_missing_link(self):
        parent = git(self.wiki, "rev-parse", "HEAD").strip()
        self.write(B, page(OLD + "[[knowledge/reference]]\n"))
        self.assertEqual(new_broken_links(self.wiki, parent, [B]),
                         [{"page": B, "target": "pages/knowledge/missing.md"}])

    def test_existing_link_to_deleted_target_is_newly_broken(self):
        self.write(A, page(NEW))
        parent = self.commit()
        git(self.wiki, "rm", REF)
        self.write(A, page(NEW + "More explanation."))
        self.assertEqual(new_broken_links(self.wiki, parent, [A, REF]),
                         [{"page": A, "target": REF}])


if __name__ == "__main__":
    unittest.main()
