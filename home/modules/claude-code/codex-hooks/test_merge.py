import copy
import json
from pathlib import Path
import tempfile
import tomllib
import unittest

import merge


class MergeTests(unittest.TestCase):
    def setUp(self):
        self.desired = merge.definitions("/nix/store/test/bin/claude-wiki")
        self.hashes = {
            (merge.event_label(event), handler["command"]): "sha256:test"
            for event, group in self.desired.items()
            for handler in group["hooks"]
        }

    def test_foreign_handlers_keep_their_exact_positions(self):
        orca = {"type": "command", "command": ": orca-agent-hook-form=2; true"}
        old = {"type": "command", "command": merge.MARKER + " old"}
        document = {"description": "keep", "hooks": {"SessionStart": [
            {"hooks": [orca]}, {"hooks": [old]}, {"hooks": [orca, old, orca]},
            {"hooks": [old]}, {"matcher": "startup", "hooks": [orca]},
        ]}}
        original = copy.deepcopy(document)
        result = merge.merge_hooks(document, self.desired)
        self.assertEqual(document, original)
        groups = result["hooks"]["SessionStart"]
        self.assertEqual(groups[0], original["hooks"]["SessionStart"][0])
        self.assertEqual(groups[2]["hooks"][0], orca)
        self.assertEqual(groups[2]["hooks"][2], orca)
        self.assertEqual(groups[4], original["hooks"]["SessionStart"][4])
        self.assertEqual(groups[3], {"hooks": []})
        self.assertEqual(merge.merge_hooks(result, self.desired), result)
        config = tomllib.loads(merge.merge_trust("", Path("/home/test/hooks.json"), result, self.hashes))
        self.assertFalse(config["hooks"]["state"]["/home/test/hooks.json:session_start:2:1"]["enabled"])

    def test_atomic_install_backup_and_noop(self):
        with tempfile.TemporaryDirectory() as temp:
            home = Path(temp)
            foreign = {"hooks": {"SessionStart": [{"hooks": [{
                "type": "command", "command": "echo foreign"
            }]}]}}
            hooks_text = json.dumps(foreign)
            key = f"{home}/hooks.json:session_start:0:0"
            config = '# user comment\nmodel = "custom"\n\n' + f'[hooks.state."{key}"]\nenabled = false\ntrusted_hash = "foreign"\n'
            home.joinpath("hooks.json").write_text(hooks_text)
            home.joinpath("config.toml").write_text(config)
            home.joinpath("AGENTS.md").write_text("User instructions stay exactly here.\n")
            merge.install(home, self.desired, self.hashes, "Wiki rules\n")
            self.assertEqual(home.joinpath("hooks.json.claude-wiki.bak").read_text(), hooks_text)
            self.assertEqual(home.joinpath("config.toml.claude-wiki.bak").read_text(), config)
            text = home.joinpath("config.toml").read_text()
            self.assertTrue(text.startswith('# user comment\nmodel = "custom"'))
            states = tomllib.loads(text)["hooks"]["state"]
            self.assertEqual(states[key], {"enabled": False, "trusted_hash": "foreign"})
            self.assertEqual(states[f"{home}/hooks.json:session_start:1:0"]["trusted_hash"], "sha256:test")
            self.assertTrue(home.joinpath("AGENTS.md").read_text().startswith("User instructions stay exactly here.\n"))
            before = {p.name: (p.read_bytes(), p.stat().st_mtime_ns) for p in home.iterdir()}
            merge.install(home, self.desired, self.hashes, "Wiki rules\n")
            self.assertEqual(before, {p.name: (p.read_bytes(), p.stat().st_mtime_ns) for p in home.iterdir()})

    def test_new_home_and_absent_home(self):
        with tempfile.TemporaryDirectory() as temp:
            home = Path(temp)
            merge.install(home / "absent", self.desired, self.hashes, "Rules")
            self.assertFalse((home / "absent").exists())
            merge.install(home, self.desired, self.hashes, "Rules")
            self.assertEqual(len(tomllib.loads(home.joinpath("config.toml").read_text())["hooks"]["state"]), 5)

    def test_invalid_input_is_never_partly_written(self):
        for name, text in [("hooks.json", "{broken"), ("config.toml", "[broken"), ("AGENTS.md", merge.BEGIN * 2)]:
            with self.subTest(name=name), tempfile.TemporaryDirectory() as temp:
                home = Path(temp)
                home.joinpath(name).write_text(text)
                with self.assertRaises(ValueError):
                    merge.install(home, self.desired, self.hashes, "Rules")
                self.assertEqual(list(home.iterdir()), [home / name])
                self.assertEqual(home.joinpath(name).read_text(), text)

    def test_symlinks_are_not_replaced(self):
        with tempfile.TemporaryDirectory() as temp:
            home = Path(temp)
            home.joinpath("target").write_text("{}")
            home.joinpath("hooks.json").symlink_to(home / "target")
            with self.assertRaises(ValueError):
                merge.install(home, self.desired, self.hashes, "Rules")
            self.assertTrue(home.joinpath("hooks.json").is_symlink())


if __name__ == "__main__":
    unittest.main()
