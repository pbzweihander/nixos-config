"""Git and subprocess plumbing. Destructive operations belong only in worktrees."""

import os
from pathlib import Path
import re
import signal
import subprocess


PAGE = re.compile(r"pages/(knowledge|followups|history|projects)/[a-z0-9][a-z0-9._-]*\.md\Z")


def command(argv, cwd, *, env=None, input=None, timeout=120, check=True):
    # Kill the entire process group, including tool children, on timeout/interruption.
    process = subprocess.Popen(
        [str(a) for a in argv], cwd=cwd, env=env, stdin=subprocess.PIPE,
        stdout=subprocess.PIPE, stderr=subprocess.PIPE, text=True,
        start_new_session=True,
    )
    try:
        stdout, stderr = process.communicate(input, timeout=timeout)
    finally:
        try:
            os.killpg(process.pid, signal.SIGKILL)
        except ProcessLookupError:
            pass
        process.wait()
        for stream in (process.stdin, process.stdout, process.stderr):
            stream.close()
    result = subprocess.CompletedProcess(argv, process.returncode, stdout, stderr)
    if check and result.returncode:
        raise RuntimeError(f"{argv[0]} exited {result.returncode}: {stderr.strip()}")
    return result


def git(repo, *args, **kwargs):
    return command(
        ["git", "-c", "core.hooksPath=/dev/null", "-c", "commit.gpgsign=false",
         "--no-optional-locks", "--literal-pathspecs", "-C", str(repo), *args], repo, **kwargs,
    ).stdout


def status(repo):
    """Return both ends of renames, with literal filenames (including whitespace)."""
    fields = iter(git(repo, "status", "--porcelain=v1", "-z", "--untracked-files=all").split("\0"))
    result = {}
    for field in fields:
        if not field:
            continue
        flags, path = field[:2], field[3:]
        result[path] = flags
        if "R" in flags or "C" in flags:
            result[next(fields)] = flags
    return result


def overlaps(a, b):
    return a == b or a.startswith(b.rstrip("/") + "/") or b.startswith(a.rstrip("/") + "/")


def safe_page(path, *, projects=False):
    match = PAGE.fullmatch(path)
    return bool(match and (projects or match[1] != "projects"))


def regular_page(repo, path):
    candidate = Path(repo) / path
    return (safe_page(path, projects=True) and candidate.is_file()
            and not any(p.is_symlink() for p in [candidate, *candidate.parents]
                        if p != Path(repo).parent))


def changes(repo, parent, commit):
    """(status, old path, new path); -M includes renames, never copy detection."""
    fields = iter(git(repo, "diff", "--name-status", "-z", "-M", parent, commit, "--").split("\0"))
    result = []
    for flag in fields:
        if not flag:
            continue
        path = next(fields)
        if flag.startswith("R"):
            result.append((flag, path, next(fields)))
        elif flag == "A":
            result.append((flag, None, path))
        elif flag == "D":
            result.append((flag, path, None))
        else:
            result.append((flag, path, path))
    return result


def page_text(repo, revision, path):
    return git(repo, "show", f"{revision}:{path}") if path else ""


def changed_texts(repo, parent, commit):
    return [
        {"old_path": old, "new_path": new,
         "old": page_text(repo, parent, old), "new": page_text(repo, commit, new)}
        for _, old, new in changes(repo, parent, commit)
        if (old and old.startswith("pages/")) or (new and new.startswith("pages/"))
    ]
