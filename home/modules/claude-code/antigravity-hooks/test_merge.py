import importlib.util
import json
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location("merge", Path(__file__).with_name("merge.py"))
merge = importlib.util.module_from_spec(spec)
spec.loader.exec_module(merge)


class MergeTests(unittest.TestCase):
    def test_updates_only_owned_top_level_key_and_is_idempotent(self):
        original = {"other": {"enabled": False, "Stop": [{"command": "other"}]},
                    "agent-wiki": {"old": True}, "unknown": ["preserve", 42]}
        desired = merge.merge_hooks(original, "/nix/store/wiki/bin/agent-wiki")
        self.assertEqual(desired["other"], original["other"])
        self.assertEqual(desired["unknown"], original["unknown"])
        self.assertEqual(original["agent-wiki"], {"old": True})
        self.assertEqual(merge.merge_hooks(desired, "/nix/store/wiki/bin/agent-wiki"), desired)
        self.assertNotIn("PreToolUse", desired["agent-wiki"])
        for event, handlers in desired["agent-wiki"].items():
            if event == "PostToolUse":
                handlers = handlers[0]["hooks"]
            for handler in handlers:
                self.assertIn("--harness antigravity", handler["command"])
                self.assertNotIn("hooks", handler)

    def test_creation_backup_permissions_and_idempotent_bytes(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "config/hooks.json"
            self.assertTrue(merge.install(path, "/nix/store/one/bin/agent-wiki"))
            self.assertEqual(path.stat().st_mode & 0o777, 0o600)
            original = path.read_bytes()
            before = path.stat().st_mtime_ns
            self.assertFalse(merge.install(path, "/nix/store/one/bin/agent-wiki"))
            self.assertEqual(path.stat().st_mtime_ns, before)
            self.assertTrue(merge.install(path, "/nix/store/two/bin/agent-wiki"))
            self.assertEqual(next(path.parent.glob("*.agent-wiki-backup-*")).read_bytes(), original)

    def test_malformed_and_symlinks_are_untouched(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "hooks.json"
            for content in [b"[1]", b"{broken"]:
                path.write_bytes(content)
                with self.assertRaises(ValueError):
                    merge.install(path, "/bin/wiki")
                self.assertEqual(path.read_bytes(), content)
            target = Path(directory) / "target"
            target.write_text("{}")
            path.unlink()
            path.symlink_to(target)
            with self.assertRaises(ValueError):
                merge.install(path, "/bin/wiki")
            self.assertEqual(target.read_text(), "{}")

    def test_concurrent_edit_is_preserved(self):
        with tempfile.TemporaryDirectory() as directory:
            path = Path(directory) / "hooks.json"
            path.write_text("{}")
            read = merge.read
            calls = 0

            def changing_read(target):
                nonlocal calls
                calls += 1
                if calls == 2:
                    path.write_text('{"foreign": {}}')
                return read(target)

            with patch.object(merge, "read", side_effect=changing_read):
                with self.assertRaisesRegex(ValueError, "concurrent modification"):
                    merge.install(path, "/bin/wiki")
            self.assertEqual(json.loads(path.read_text()), {"foreign": {}})
            self.assertEqual(list(path.parent.glob(".agent-wiki-hooks-*")), [])


if __name__ == "__main__":
    unittest.main()
