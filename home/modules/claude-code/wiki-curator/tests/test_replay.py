import contextlib
import io
import json

from test_curator import A, RepoTest, justification, page
from wiki_curator.gitutil import git, status
from wiki_curator.replay import commit_justification, main, replay


class ReplayTests(RepoTest):
    def curator_commit(self, explanation=None):
        explanation = explanation or justification(dropped=[])
        return self.commit("curator: log-growth: simplify\n\nTask: {}\n\nJustification: "
                           + json.dumps(explanation) + "\n\nCurator-Run: test\n\n"
                           "(cherry picked from commit abcdef)\n")

    def test_replay_reports_new_loss_respects_dropped_and_leaves_checkout_untouched(self):
        self.write(A, page("Keep snake_case and HTTP503."))
        self.commit()
        self.write(A, page("Keep HTTP503."))
        failing = self.curator_commit()
        self.write(A, page("Keep prose."))
        passing = self.curator_commit(justification(dropped=[
            {"token": "HTTP503", "reason": "obsolete"}]))
        # --grep can match bodies of ordinary commits; exclude those subjects.
        self.write(A, page("Keep prose. More text."))
        head = self.commit("ordinary change\n\ncurator: quoted subject")
        self.write(A, page("Uncommitted snake_case changes."))
        self.write("untracked.txt", "untouched")
        before = status(self.wiki)
        index = (self.wiki / ".git/index").read_bytes()
        results = list(replay(self.wiki))
        self.assertEqual([r["commit"] for r in results], [passing, failing])
        self.assertEqual(results[0]["missing"], [])
        self.assertEqual(results[1]["missing"], ["snake_case"])
        self.assertTrue(all(r["error"] is None for r in results))
        output = io.StringIO()
        with contextlib.redirect_stdout(output):
            self.assertEqual(main([str(self.wiki)]), 1)
        self.assertIn(failing, output.getvalue())
        self.assertIn('missing tokens: ["snake_case"]', output.getvalue())
        self.assertIn("Checked 2 curator commits; 1 failed.", output.getvalue())
        self.assertEqual(git(self.wiki, "rev-parse", "HEAD").strip(), head)
        self.assertEqual(status(self.wiki), before)
        self.assertEqual((self.wiki / ".git/index").read_bytes(), index)
        self.assertEqual((self.wiki / A).read_text(), page("Uncommitted snake_case changes."))
        self.assertFalse(self.cache.exists())

    def test_malformed_justification_is_reported_and_replay_continues(self):
        for body in ("", "Justification: not json", "Justification: {}"):
            self.write(A, page(body or "Missing explanation."))
            self.commit("curator: broken-links: invalid explanation\n\n" + body)
        results = list(replay(self.wiki))
        self.assertEqual(len(results), 3)
        self.assertTrue(all(r["error"] for r in results))
        valid = json.dumps(justification())
        with self.assertRaisesRegex(ValueError, "exactly one"):
            commit_justification(f"Justification: {valid}\nJustification: {valid}")
        with self.assertRaisesRegex(ValueError, "duplicate JSON key"):
            commit_justification('Justification: {"summary": "a", "summary": "b"}')

    def test_all_refs_includes_retained_candidates(self):
        parent = git(self.wiki, "rev-parse", "HEAD").strip()
        self.write(A, page("Retained candidate."))
        candidate = self.curator_commit()
        git(self.wiki, "branch", "curator/rejected/test", candidate)
        git(self.wiki, "reset", "--hard", parent)
        self.assertEqual(list(replay(self.wiki)), [])
        self.assertEqual([r["commit"] for r in replay(self.wiki, all_refs=True)], [candidate])
        with contextlib.redirect_stdout(io.StringIO()):
            self.assertEqual(main([str(self.wiki)]), 0)
