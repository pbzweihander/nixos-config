#!/usr/bin/env python3
"""Own only the agent-wiki named entry in Antigravity's mutable hooks.json."""

import argparse
import copy
import hashlib
import json
import os
from pathlib import Path
import shlex
import stat
import tempfile


def definitions(wiki):
    def handler(command):
        return {
            "type": "command",
            "command": f"{shlex.quote(wiki)} {command} --harness antigravity",
            "timeout": 5,
        }

    return {
        "SessionStart": [handler("context")],
        "PreInvocation": [handler("remind")],
        "PostInvocation": [handler("remind")],
        "PostToolUse": [{
            "matcher": "read_file|run_command",
            "hooks": [handler("recall"), handler("remind --event")],
        }],
        "Stop": [handler("record")],
    }


def merge_hooks(document, wiki):
    if not isinstance(document, dict):
        raise ValueError("hooks.json must contain an object")
    result = copy.deepcopy(document)
    result["agent-wiki"] = definitions(wiki)
    return result


def foreign_digest(document):
    """Name backups by what this merger does not own.

    Hook commands embed the agent-wiki store path, so the owned entry changes on
    every rebuild. Keying backups on the other keys keeps one backup per distinct
    user configuration instead of one per agent-wiki build.
    """
    if not isinstance(document, dict):
        raise ValueError("hooks.json must contain an object")
    foreign = {key: value for key, value in document.items() if key != "agent-wiki"}
    canonical = json.dumps(foreign, sort_keys=True, ensure_ascii=False, separators=(",", ":"))
    return hashlib.sha256(canonical.encode()).hexdigest()[:16]


def read(path):
    if path.is_symlink():
        raise ValueError(f"refusing symlink: {path}")
    if not path.exists():
        return None
    if not stat.S_ISREG(path.stat().st_mode):
        raise ValueError(f"not a regular file: {path}")
    return path.read_bytes()


def install(path, wiki):
    original = read(path)
    document = json.loads(original) if original is not None else {}
    desired = merge_hooks(document, wiki)
    if desired == document:
        return False
    data = (json.dumps(desired, ensure_ascii=False, indent=2) + "\n").encode()
    path.parent.mkdir(parents=True, exist_ok=True)
    mode = stat.S_IMODE(path.stat().st_mode) if original is not None else 0o600
    temporary = None
    try:
        with tempfile.NamedTemporaryFile(dir=path.parent, prefix=".agent-wiki-hooks-", delete=False) as out:
            temporary = Path(out.name)
            os.fchmod(out.fileno(), mode)
            out.write(data)
            out.flush()
            os.fsync(out.fileno())
        if read(path) != original:
            raise ValueError(f"concurrent modification: {path}")
        if original is not None:
            backup = path.with_name(f"{path.name}.agent-wiki-backup-{foreign_digest(document)}")
            try:
                fd = os.open(backup, os.O_WRONLY | os.O_CREAT | os.O_EXCL, 0o600)
            except FileExistsError:
                existing = read(backup)
                if existing is None or foreign_digest(json.loads(existing)) != foreign_digest(document):
                    raise ValueError(f"backup content mismatch: {backup}") from None
            else:
                with os.fdopen(fd, "wb") as out:
                    out.write(original)
        if read(path) != original:
            raise ValueError(f"concurrent modification: {path}")
        os.replace(temporary, path)
        return True
    finally:
        if temporary is not None:
            temporary.unlink(missing_ok=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--wiki", required=True)
    parser.add_argument("path", type=Path)
    args = parser.parse_args()
    install(args.path, args.wiki)


if __name__ == "__main__":
    main()
