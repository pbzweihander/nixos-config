import importlib.util
import json
import os
from pathlib import Path
import subprocess
import tempfile
import unittest

ROOT = Path(__file__).resolve().parent
spec = importlib.util.spec_from_file_location("migration", ROOT / "migrate-wiki.py")
migration = importlib.util.module_from_spec(spec)
spec.loader.exec_module(migration)


class MigrationTests(unittest.TestCase):
    def test_recap_uses_new_settings_and_legacy_paths(self):
        with tempfile.TemporaryDirectory() as home:
            base = Path(home)
            data = base / "data"
            old, new = data / "claude-wiki", data / "agent-wiki"
            env = {k: v for k, v in os.environ.items() if not k.startswith(("AGENT_WIKI_", "CLAUDE_WIKI_"))}
            env.update(HOME=home, XDG_DATA_HOME=str(data), XDG_CACHE_HOME=str(base / "cache"))
            def check(expected, extra=None):
                result = subprocess.run(["fish", "--no-config", str(ROOT / "wiki-recap/wiki-recap.fish"), "--update"],
                                        env={**env, **(extra or {})}, capture_output=True, text=True, check=True)
                self.assertIn(f"no wiki at {expected}", result.stderr)
            check(new)
            old.mkdir(parents=True)
            check(old)
            new.mkdir()
            check(new)
            check("/legacy", {"CLAUDE_WIKI_DIR": "/legacy"})
            check("/primary", {"CLAUDE_WIKI_DIR": "/legacy", "AGENT_WIKI_DIR": "/primary"})

    def test_old_only_already_migrated_and_conflicting_directories(self):
        for location in (".local/share", ".local/state", ".cache", "runtime"):
            with self.subTest(location=location), tempfile.TemporaryDirectory() as home:
                base = Path(home) / location
                old, new = base / "claude-wiki", base / "agent-wiki"
                old.mkdir(parents=True)
                (old / "keep").write_text("unchanged content")
                migration.migrate(base)
                self.assertTrue(old.is_symlink())
                self.assertEqual(old.resolve(), new.resolve())
                before = (new / "keep").stat()
                migration.migrate(base)
                self.assertEqual((new / "keep").stat(), before)
                self.assertEqual((old / "keep").read_text(), "unchanged content")
                old.unlink()
                old.mkdir()
                (old / "other").write_text("manual merge required")
                migration.migrate(base)
                self.assertFalse(old.is_symlink())
                self.assertEqual((old / "other").read_text(), "manual merge required")
                self.assertEqual((new / "keep").read_text(), "unchanged content")

    def test_unrelated_symlinks_and_files_are_never_replaced(self):
        with tempfile.TemporaryDirectory() as home:
            base = Path(home)
            old, new = base / "claude-wiki", base / "agent-wiki"
            old.symlink_to("elsewhere")
            migration.migrate(base)
            self.assertEqual(os.readlink(old), "elsewhere")
            old.unlink()
            old.mkdir()
            new.symlink_to("dangling")
            migration.migrate(base)
            self.assertTrue(old.is_dir())
            self.assertEqual(os.readlink(new), "dangling")

    def test_claude_settings_replace_exact_legacy_commands_only(self):
        commands = {
            "SessionStart": ["context"], "UserPromptSubmit": ["remind"],
            "SessionEnd": ["record"], "PostToolUse": ["recall", "remind --event"],
            "PreToolUse": ["recall"], "PreCompact": ["remind --event"],
            "PostToolUseFailure": ["recall", "remind --event"],
        }
        def hook(command):
            return {"type": "command", "command": command}
        static = {"hooks": {event: [{"hooks": [hook("agent-wiki " + cmd)]} for cmd in cmds]
                            for event, cmds in commands.items()}}
        legacy = json.loads(json.dumps(static).replace("agent-wiki", "claude-wiki"))
        foreign = [hook("echo claude-wiki context"), hook("/custom/claude-wiki context"),
                   {"type": "prompt", "command": "claude-wiki context"}]
        legacy["hooks"]["SessionStart"][0]["hooks"].extend(foreign)
        legacy["permissions"] = {"allow": ["Bash(foreign *)"]}
        with tempfile.TemporaryDirectory() as home:
            config = Path(home) / "static.json"
            config.write_text(json.dumps(static))
            def merge(value):
                return json.loads(subprocess.check_output(
                    ["jq", "--slurpfile", "static", str(config), "-f", str(ROOT / "merge-settings.jq")],
                    input=json.dumps(value), text=True))
            result = merge(legacy)
            self.assertEqual(result["hooks"]["SessionStart"][0]["hooks"], foreign)
            self.assertEqual(result["permissions"], legacy["permissions"])
            for event, cmds in commands.items():
                actual = [h["command"] for g in result["hooks"][event] for h in g["hooks"] if h["type"] == "command"]
                for cmd in cmds:
                    self.assertEqual(actual.count("agent-wiki " + cmd), 1)
                    self.assertNotIn("claude-wiki " + cmd, actual)
            self.assertEqual(merge(result), result)


if __name__ == "__main__":
    unittest.main()
