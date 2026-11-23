import os
from pathlib import Path
import tempfile
import unittest
from unittest.mock import patch

from wiki_curator.config import setting, socket_path, wiki_dir


class ConfigTests(unittest.TestCase):
    def test_legacy_settings_primary_precedence_and_empty_socket(self):
        with patch.dict(os.environ, {"CLAUDE_WIKI_DIR": "/old", "CLAUDE_WIKI_SOCKET": "/old.sock"}, clear=True):
            self.assertEqual(wiki_dir(), Path("/old"))
            self.assertEqual(socket_path(), "/old.sock")
            os.environ.update(AGENT_WIKI_DIR="/new", AGENT_WIKI_SOCKET="")
            self.assertEqual(wiki_dir(), Path("/new"))
            self.assertIsNone(socket_path())
            self.assertEqual(setting("SOCKET"), "")

    def test_paths_before_and_after_migration(self):
        with tempfile.TemporaryDirectory() as home, patch.dict(os.environ, {"XDG_DATA_HOME": home, "XDG_RUNTIME_DIR": home}, clear=True):
            base = Path(home)
            self.assertEqual(wiki_dir(), base / "agent-wiki")
            old, new = base / "claude-wiki", base / "agent-wiki"
            old.mkdir()
            self.assertEqual(wiki_dir(), old)
            self.assertEqual(socket_path(), str(old / "embed.sock"))
            old.rename(new)
            old.symlink_to(new)
            self.assertEqual(wiki_dir(), new)
            self.assertEqual(socket_path(), str(new / "embed.sock"))
