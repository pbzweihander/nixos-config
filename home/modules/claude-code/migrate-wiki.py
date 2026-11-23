#!/usr/bin/env python3
"""Move wiki directories without overwriting data; retain old path aliases."""
import argparse
import os
from pathlib import Path
import subprocess
import sys


def migrate(base, mv="mv"):
    old, new = base / "claude-wiki", base / "agent-wiki"
    if old.is_symlink():
        if old.resolve() != new.resolve():
            print(f"agent-wiki: leave unrelated symlink {old} unchanged", file=sys.stderr)
        return
    if os.path.lexists(old):
        if not old.is_dir() or os.path.lexists(new):
            print(f"agent-wiki: leaving {old} and {new} unchanged; merge manually if needed", file=sys.stderr)
            return
        # Never copy across filesystems or replace a concurrently created target.
        subprocess.run([mv, "--no-copy", "--no-clobber", "--no-target-directory", str(old), str(new)], check=True)
        if old.exists():
            print(f"agent-wiki: could not move {old}; leaving it unchanged", file=sys.stderr)
            return
    if new.is_dir() and not os.path.lexists(old):
        old.symlink_to(new.name, target_is_directory=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--mv", default="mv")
    parser.add_argument("bases", nargs="*", type=Path)
    args = parser.parse_args()
    for base in dict.fromkeys(args.bases):
        migrate(base, args.mv)


if __name__ == "__main__":
    main()
