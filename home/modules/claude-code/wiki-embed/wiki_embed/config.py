"""Current wiki settings with permanent legacy fallbacks."""
import os
from pathlib import Path


def setting(suffix):
    return os.environ.get("AGENT_WIKI_" + suffix, os.environ.get("CLAUDE_WIKI_" + suffix))


def directory(base):
    new, old = Path(base) / "agent-wiki", Path(base) / "claude-wiki"
    return old if not new.exists() and old.exists() else new


def wiki_dir():
    value = setting("DIR")
    return Path(value).expanduser() if value else directory(os.environ.get("XDG_DATA_HOME", Path.home() / ".local/share"))


def socket_path():
    value = setting("SOCKET")
    if value is not None:
        return value or None
    runtime = os.environ.get("XDG_RUNTIME_DIR")
    return str(directory(runtime) / "embed.sock") if runtime else None
