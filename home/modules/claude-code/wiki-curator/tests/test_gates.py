import json
import subprocess
import time
import unittest

from test_curator import A, B, DAY, REF, NEW, OLD, RepoTest, justification, page
from wiki_curator.gitutil import changes, git, status
from wiki_curator.recall import queries, recall
from wiki_curator.runner import new_broken_links, read_state


QUERY = "Why does the probe take time?"
EXTRA = "queries: " + json.dumps([QUERY]) + "\n"


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

    def gate(self, parent, commit, redirects=None):
        self.save_fixture()
        try:
            return recall(self.wiki, parent, commit, justification(redirects=redirects or {}),
                          {**self.env, "CLAUDE_WIKI_SOCKET": ""})
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
                                                      "parent_rank": before, "commit_rank": after}])
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
                                              "parent_rank": None, "commit_rank": 1}])

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
