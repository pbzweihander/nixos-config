import json
import os
import subprocess
import sys
from unittest.mock import patch

from test_curator import A, B, REF, NEW, OLD, RepoTest, justification, page
from wiki_curator.adjudication import apply_edits, frontmatter_only, parse_adjudication, singleton_tags
from wiki_curator.gitutil import git
from wiki_curator.runner import read_state


PROBLEM = "쿼리 표현이 어색합니다"
REASON = "본문이 동일하고 검색 표현의 문체 차이뿐입니다"
QUERY = "Why does the probe take time?"


class AdjudicationTests(RepoTest):
    def prepare(self, verdict="override", edits=None, frontmatter=True):
        if frontmatter:
            self.fixture.update(lint={"missing-queries": [{"path": A}]},
                                writes={A: page(OLD, extra=f'queries: ["{QUERY}"]\n')},
                                justification=justification(dropped=[]))
        self.fixture["review"] = json.dumps({"accept": False, "problems": [PROBLEM]})
        self.fixture["adjudicate"] = json.dumps({"verdict": verdict, "reason": REASON, "edits": edits or []})

    def adjudicators(self):
        return [c for c in self.calls("claude") if "--tools" in c["args"]]

    def assert_rejected(self, result):
        self.assertEqual(result["outcome"], "rejected", result)
        self.assertTrue(result["branch"].startswith("curator/rejected/"))
        self.assertFalse(result["gates"]["codex"]["accept"])
        self.assertNotIn("diff", result)
        self.assertEqual(list((self.cache / "runs").glob("*.diff")), [])
        self.assertEqual(git(self.wiki, "rev-parse", result["branch"]).strip(), result["commit"])
        self.assertEqual((self.wiki / A).read_text(), page(OLD))
        self.assertIn(A, read_state(self.cache)["cooldowns"])

    def test_frontmatter_override_lands_with_isolated_call_and_report(self):
        self.prepare()
        os.environ["WIKI_CURATOR_MODEL"] = "fixture-model"
        result = self.result()
        self.assertEqual(result["outcome"], "merged", result)
        stage = result["gates"]["adjudicate"]
        self.assertTrue(stage["accept"])
        self.assertTrue(stage["frontmatter_only"])
        self.assertEqual(stage["reason"], REASON)
        call, = self.adjudicators()
        self.assertEqual(call["args"], ["-p", "--setting-sources", "", "--no-session-persistence",
                                        "--tools", "", "--max-turns", "1", "--model", "fixture-model"])
        self.assertEqual(call["wiki"], call["cwd"])
        self.assertEqual(call["socket"], "")
        self.assertIn(PROBLEM, call["prompt"])
        self.assertIn('"recall":', call["prompt"])
        self.assertIn('"old":', call["prompt"])
        self.assertIn('"new":', call["prompt"])
        self.assertEqual(len(self.calls("codex")), 1)
        latest = (self.cache / "latest.md").read_text()
        self.assertIn("merged (adjudicated: override)", latest)
        self.assertLess(latest.index(PROBLEM), latest.index(REASON))

    def test_body_override_is_upheld_and_visible_in_review(self):
        self.prepare(frontmatter=False)
        result = self.result()
        self.assert_rejected(result)
        stage = result["gates"]["adjudicate"]
        self.assertEqual(stage["verdict"], "uphold")
        self.assertEqual(stage["requested_verdict"], "override")
        self.assertFalse(stage["frontmatter_only"])
        latest = (self.cache / "latest.md").read_text()
        self.assertLess(latest.index(PROBLEM), latest.index(REASON))
        output = subprocess.run([sys.executable, "-m", "wiki_curator", "review"],
                                capture_output=True, text=True, check=True).stdout
        self.assertLess(output.index(PROBLEM), output.index(REASON))
        output = subprocess.run([sys.executable, "-m", "wiki_curator", "review", "--json"],
                                capture_output=True, text=True, check=True).stdout
        self.assertEqual(json.loads(output)["items"][0]["adjudicate"], stage)

    def test_valid_revision_runs_gates_and_second_review_and_lands_whole_change(self):
        self.prepare("revise", [{"path": A, "old": "Measured", "new": "Measured locally"}], frontmatter=False)
        self.fixture["second_review"] = {"review": '{"accept": true, "problems": []}'}
        result = self.result()
        self.assertEqual(result["outcome"], "merged", result)
        self.assertEqual((self.wiki / A).read_text(), page(NEW.replace("Measured", "Measured locally")))
        stage = result["gates"]["adjudicate"]
        self.assertTrue(stage["accept"])
        self.assertTrue(stage["second_review"]["accept"])
        self.assertEqual(set(stage["gates"]), {"scope", "broken-links", "check", "preservation", "recall", "codex"})
        self.assertTrue(all(gate["accept"] for gate in stage["gates"].values()))
        self.assertEqual(git(self.wiki, "rev-parse", result["commit"] + "^").strip(), result["parent_commit"])
        self.assertNotEqual(stage["original_commit"], result["commit"])
        self.assertEqual(len(self.adjudicators()), 1)
        self.assertEqual(len(self.calls("codex")), 2)
        self.assertEqual(len([c for c in self.calls("claude-wiki") if c["args"][0] == "check"]), 2)
        self.assertIn("merged (adjudicated: revise)", (self.cache / "latest.md").read_text())

    def test_invalid_edits_and_second_rejection_keep_candidate(self):
        for edit, error in (({"path": A, "old": "---", "new": "--"}, "exactly once"),
                            ({"path": REF, "old": "Reference", "new": "The reference"}, "outside changed pages"),
                            ({"path": A, "old": "absent", "new": "present"}, "exactly once"),
                            ({"path": A, "old": QUERY, "new": "How long does the probe take?"}, None)):
            with self.subTest(edit=edit):
                (self.cache / "state.json").unlink(missing_ok=True)
                self.calls_path.unlink(missing_ok=True)
                self.prepare("revise", [edit])
                result = self.result()
                self.assert_rejected(result)
                stage = result["gates"]["adjudicate"]
                self.assertFalse(stage["accept"])
                self.assertEqual(len(self.adjudicators()), 1)
                if error:
                    self.assertIn(error, stage["error"])
                    self.assertEqual(result["commit"], stage["original_commit"])
                else:
                    self.assertFalse(stage["second_review"]["accept"])
                    self.assertEqual(len(self.calls("codex")), 2)
                    self.assertIn("second review:", (self.cache / "latest.md").read_text())

    def test_revision_rechecks_links_preservation_and_validation(self):
        for old, new, gate in (("knowledge/reference", "knowledge/unknown", "broken-links"),
                               ("45s", "46s", "preservation"),
                               ("Measured", "Measured locally", "check")):
            with self.subTest(gate=gate):
                (self.cache / "state.json").unlink(missing_ok=True)
                self.calls_path.unlink(missing_ok=True)
                self.prepare("revise", [{"path": A, "old": old, "new": new}], frontmatter=False)
                self.fixture["revision_check_exit"] = 2 if gate == "check" else 0
                result = self.result()
                self.assert_rejected(result)
                self.assertFalse(result["gates"]["adjudicate"]["gates"][gate]["accept"])
                self.assertEqual(len(self.calls("codex")), 1)

    def test_unusable_codex_can_be_adjudicated(self):
        self.prepare()
        self.fixture["review"] = "not JSON"
        result = self.result()
        self.assertEqual(result["outcome"], "merged", result)
        self.assertFalse(result["gates"]["codex"]["accept"])
        self.assertTrue(result["gates"]["adjudicate"]["accept"])

    def test_unusable_adjudication_uphold_disabled_and_integrity(self):
        cases = [{"adjudicate": "not JSON"}, {"adjudicate_exit": 1}, {"adjudicate_sleep": 2},
                 {"adjudicate": '{"verdict":"uphold","reason":"판단 보류","edits":[]}'},
                 {"adjudicate_writes": {"writes": {A: page(NEW)}}},
                 {"review_edits": {"writes": {A: page(NEW)}}}, {"disabled": True}]
        for case in cases:
            with self.subTest(case=case):
                (self.cache / "state.json").unlink(missing_ok=True)
                self.calls_path.unlink(missing_ok=True)
                self.fixture = {"lint": {}, "writes": {}}
                self.prepare()
                self.fixture.update(case)
                with patch.dict(os.environ, {"WIKI_CURATOR_ADJUDICATE": "0" if case.get("disabled") else "1"}), \
                     patch("wiki_curator.runner.CLAUDE_TIMEOUT", .15 if "adjudicate_sleep" in case else 10):
                    result = self.result()
                self.assert_rejected(result)
                if case.get("disabled") or "review_edits" in case:
                    self.assertNotIn("adjudicate", result["gates"])
                    self.assertFalse(self.adjudicators())
                else:
                    self.assertFalse(result["gates"]["adjudicate"]["accept"])

    def test_adjudicator_failure_uses_stdout_when_stderr_is_empty(self):
        cases = [("Reached max turns\nPlease retry", "", "Reached max turns Please retry"),
                 ("output", "  stderr failure\n", "stderr failure"),
                 ("omitted prefix" + "x" * 600 + "\nFailed", " \n", "x" * 492 + " Failed")]
        for stdout, stderr, detail in cases:
            with self.subTest(stderr=stderr, detail=detail):
                (self.cache / "state.json").unlink(missing_ok=True)
                self.prepare()
                self.fixture.update(adjudicate_exit=1, adjudicate=stdout, adjudicate_stderr=stderr)
                result = self.result()
                self.assert_rejected(result)
                self.assertEqual(result["gates"]["adjudicate"]["error"], "Claude adjudicator exited 1: " + detail)
                self.assertIn(detail, (self.cache / "latest.md").read_text())

    def test_style_notes_can_accept_without_adjudication(self):
        self.prepare()
        self.fixture["review"] = json.dumps({"accept": True, "problems": [PROBLEM]})
        result = self.result()
        self.assertEqual(result["outcome"], "merged", result)
        self.assertFalse(self.adjudicators())
        self.assertIn("bare\n   keyword list", self.calls("codex")[0]["prompt"])

    def test_byte_comparison_disallows_line_ending_changes_and_moves(self):
        parent = git(self.wiki, "rev-parse", "HEAD").strip()
        (self.wiki / A).write_bytes(page(OLD).replace("\n", "\r\n").encode())
        commit = self.commit()
        self.assertFalse(frontmatter_only(self.wiki, parent, commit))
        git(self.wiki, "mv", A, B)
        moved = self.commit()
        self.assertFalse(frontmatter_only(self.wiki, commit, moved))

    def test_schema_rejects_unbounded_or_malformed_edits(self):
        valid = {"verdict": "revise", "reason": REASON, "edits": [{"path": A, "old": "old", "new": "new"}]}
        for change in ({"edits": valid["edits"] * 11}, {"edits": []}, {"verdict": "override"},
                       {"reason": " "}, {"edits": [{"path": "../outside", "old": "a", "new": "b"}]},
                       {"edits": [{"path": A, "old": "", "new": "b"}]}):
            with self.subTest(change=change), self.assertRaises(ValueError):
                parse_adjudication(json.dumps({**valid, **change}))

    def test_edits_use_original_offsets_preserve_bytes_and_validate_before_writes(self):
        parent = git(self.wiki, "rev-parse", "HEAD").strip()
        raw = page(NEW).replace("\n", "\r\n").encode()
        (self.wiki / A).write_bytes(raw)
        commit = self.commit()
        edits = [{"path": A, "old": "Measured", "new": "Measured probe"},
                 {"path": A, "old": "probe --fast", "new": "probe --slow"}]
        for invalid in ({"path": REF, "old": "Reference", "new": "Other"},
                        {"path": A, "old": "Measured `probe", "new": "Observed"}):
            with self.subTest(invalid=invalid), self.assertRaises(ValueError):
                apply_edits(self.wiki, parent, commit, [edits[0], invalid])
            self.assertEqual((self.wiki / A).read_bytes(), raw)
        apply_edits(self.wiki, parent, commit, edits)
        self.assertEqual((self.wiki / A).read_bytes(),
                         raw.replace(b"Measured", b"Measured probe").replace(b"probe --fast", b"probe --slow"))

    def test_revision_recall_failure_prevents_second_review(self):
        self.prepare("revise", [{"path": A, "old": QUERY, "new": "How long does the probe take?"}])
        with patch("wiki_curator.runner.recall", side_effect=[
                {"accept": True, "probes": [], "problems": []},
                {"accept": False, "probes": [], "problems": ["query fell out of the top 3"]}]):
            result = self.result()
        self.assert_rejected(result)
        self.assertFalse(result["gates"]["adjudicate"]["gates"]["recall"]["accept"])
        self.assertEqual(len(self.calls("codex")), 1)


class SingletonTagTests(RepoTest):
    def test_parent_catalog_and_body_bytes_are_authoritative(self):
        self.write(A, page(OLD).replace("[nix]", "[zstd]"))
        parent = self.commit()
        # Introducing a tag on another candidate page cannot make it an existing tag.
        self.write(A, page(OLD).replace("[nix]", "[invented]"))
        self.write(REF, page("Reference.\n").replace("[nix]", "[invented]"))
        self.assertFalse(singleton_tags(self.wiki, parent, [A, REF])["accept"])
        self.write(A, page(OLD))
        self.assertTrue(singleton_tags(self.wiki, parent, [A])["accept"])
        (self.wiki / A).write_bytes(page(OLD).replace("\n", "\r\n").encode())
        self.assertFalse(singleton_tags(self.wiki, parent, [A])["accept"])

    def test_replacements_drop_unknown_body_and_duplicate_tags(self):
        original = page(OLD).replace("[nix]", "[nix, zstd, int8]")
        self.write(A, original)
        self.write(REF, page("Reference.\n").replace("[nix]", "[nix, compression, quantization]"))
        self.commit()
        cases = [("[nix, compression]", OLD, False),
                 ("[nix, compression, invented]", OLD, False),
                 ("[nix, compression, compression]", OLD, False),
                 ("[nix, compression, quantization]", NEW, False),
                 ("[nix, compression, quantization]", OLD, True)]
        for tags, body, accept in cases:
            with self.subTest(tags=tags, body=body):
                (self.cache / "state.json").unlink(missing_ok=True)
                self.calls_path.unlink(missing_ok=True)
                self.fixture = {"lint": {"singleton-tags": [{"path": A, "tag": "zstd"}, {"path": A, "tag": "int8"}]},
                                "writes": {A: page(body).replace("[nix]", tags)}, "justification": justification()}
                result = self.result()
                gate = result["gates"]["singleton-tags"]
                self.assertEqual(gate["accept"], accept, result)
                self.assertEqual(gate["pages"][0]["dropped_tags"], ["int8", "zstd"])
                self.assertEqual(result["outcome"], "merged" if accept else "rejected")
                if not accept:
                    self.assertIn("zstd", result["reason"])
                    self.assertNotIn("adjudicate", result["gates"])
                    self.assertFalse(self.calls("codex"))
                self.assertIn("one-for-one", self.calls("claude")[0]["prompt"])

    def test_revision_cannot_drop_tags_after_initial_tag_gate_passed(self):
        self.write(A, page(OLD).replace("[nix]", "[zstd]"))
        self.commit()
        self.fixture.update(lint={"singleton-tags": [{"path": A, "tag": "zstd"}]},
                            writes={A: page(OLD)}, review='{"accept": false, "problems": ["태그 검토"]}',
                            adjudicate=json.dumps({"verdict": "revise", "reason": "태그 수정", "edits": [
                                {"path": A, "old": "tags: [nix]", "new": "tags: []"}]}))
        result = self.result()
        self.assertEqual(result["outcome"], "rejected", result)
        self.assertTrue(result["gates"]["singleton-tags"]["accept"])
        self.assertFalse(result["gates"]["adjudicate"]["gates"]["singleton-tags"]["accept"])
        self.assertEqual(len(self.calls("codex")), 1)
