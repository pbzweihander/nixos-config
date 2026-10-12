"""One-run orchestration. Only land() writes the main checkout's files/index."""

from contextlib import contextmanager
from datetime import datetime, timezone
import fcntl
import fnmatch
import hashlib
import json
import math
import os
from pathlib import Path
import re
import shutil
import stat
import subprocess
import time

from .gitutil import (changed_texts, changes, command, git, page_text,
                      regular_page, safe_page, status)
from .preservation import LINK, load_justification, preserved, strict_json
from .prompts import claude_prompt, review_prompt
from .recall import recall
from .selection import DAY, blocked, pair_key, select_tasks


CLAUDE_TIMEOUT = 20 * 60
CODEX_TIMEOUT = 15 * 60
LOCK_TIMEOUT = 120
DIFF_LIMIT = 1024 * 1024
DIFF_TRUNCATED = b"\n[wiki-curator: diff truncated at 1 MiB]\n"
ALLOWED_TOOLS = [
    "Read", "Edit", "Write", "Glob", "Grep", "Bash(claude-wiki check:*)",
    "Bash(claude-wiki tags)", "Bash(claude-wiki links:*)", "Bash(claude-wiki search:*)",
    "Bash(git mv:*)", "Bash(git rm:*)", "Bash(git status:*)", "Bash(git diff:*)",
]


class Interrupted(Exception):
    """Stop the run, after subprocess and worktree cleanup, on service shutdown."""


def snapshot(worktree):
    """Detect even ignored edits; only known, root-level derived files are exempt."""
    derived = (".lock", "index.sqlite*", "usage.sqlite*", "sessions.sqlite*", "vectors-*.npz")
    result = {}
    for directory, dirs, files in os.walk(worktree, followlinks=False):
        for name in list(dirs):
            path = Path(directory) / name
            if path.is_symlink():
                dirs.remove(name)
                files.append(name)
            elif path == worktree / ".curator":
                dirs.remove(name)
        for name in files:
            path = Path(directory) / name
            rel = path.relative_to(worktree).as_posix()
            mode = path.lstat().st_mode
            if (stat.S_ISREG(mode) and "/" not in rel
                    and any(fnmatch.fnmatchcase(rel, pattern) for pattern in derived)):
                continue
            if stat.S_ISLNK(mode):
                value = os.readlink(path)
            elif stat.S_ISREG(mode):
                value = hashlib.sha256(path.read_bytes()).hexdigest()
            else:
                value = "non-regular file"
            result[rel] = (mode, value)
    return result


def atomic_write(path, text):
    temporary = path.with_name(path.name + ".tmp")
    with temporary.open("wb" if isinstance(text, bytes) else "w") as stream:
        stream.write(text)
        stream.flush()
        os.fsync(stream.fileno())
    temporary.replace(path)


def write_json(path, value):
    atomic_write(path, json.dumps(value, ensure_ascii=False, indent=2) + "\n")


def artifact_key(value):
    """Validate a run/task filename stem without accepting paths or git revisions."""
    if not isinstance(value, str):
        raise ValueError("invalid curator artifact id")
    match = re.fullmatch(r"([0-9]{8}T[0-9]{6}(?:\.[0-9]{6})?Z)-([1-9][0-9]*)", value)
    if not match:
        raise ValueError("invalid curator artifact id")
    timestamp, number = match.groups()
    date_format = "%Y%m%dT%H%M%S.%fZ" if "." in timestamp else "%Y%m%dT%H%M%SZ"
    date = datetime.strptime(timestamp, date_format).replace(tzinfo=timezone.utc)
    return timestamp, int(number), date.timestamp()


def read_state(cache):
    path = cache / "state.json"
    state = strict_json(path.read_text()) if path.exists() else {"cooldowns": {}}
    if not isinstance(state, dict) or not isinstance(state.get("cooldowns"), dict):
        raise ValueError("invalid state.json; refusing to bypass cooldowns")
    for path, until in state["cooldowns"].items():
        if (not safe_page(path) or isinstance(until, bool) or not isinstance(until, (int, float))
                or not math.isfinite(until)):
            raise ValueError("invalid cooldown in state.json")
    not_duplicates = state.setdefault("not_duplicates", {})
    if not isinstance(not_duplicates, dict):
        raise ValueError("invalid not_duplicates in state.json")
    for pair, until in not_duplicates.items():
        paths = pair.split("|")
        if (len(paths) != 2 or any(not safe_page(p) for p in paths)
                or paths[0] >= paths[1] or isinstance(until, bool)
                or not isinstance(until, (int, float)) or not math.isfinite(until)):
            raise ValueError("invalid not_duplicates entry in state.json")
    dismissed = state.setdefault("dismissed_diffs", [])
    if not isinstance(dismissed, list):
        raise ValueError("invalid dismissed_diffs in state.json")
    seen = set()
    for item in dismissed:
        if not isinstance(item, str) or not item.startswith("diff:") or item in seen:
            raise ValueError("invalid dismissed_diffs entry in state.json")
        artifact_key(item[5:])
        seen.add(item)
    return state


def save_rejection_diff(worktree, parent, destination):
    """Keep page edits before cleanup; only the disposable worktree index is used."""
    untracked = git(worktree, "ls-files", "--others", "-z", "--", "pages/").split("\0")
    for path in filter(None, untracked):
        # Include ignored new pages too: scope checks can reject those very edits.
        git(worktree, "add", "--intent-to-add", "--force", "--", path)
    diff = git(worktree, "diff", "--binary", "--no-ext-diff", "--no-textconv", "--no-color",
               "--src-prefix=a/", "--dst-prefix=b/", parent, "--", "pages/", text=False)
    if not diff:
        return None
    if len(diff) > DIFF_LIMIT:
        diff = diff[:DIFF_LIMIT - len(DIFF_TRUNCATED)] + DIFF_TRUNCATED
    atomic_write(destination, diff)
    return str(destination)


@contextmanager
def wiki_lock(wiki):
    with (wiki / ".lock").open("a") as stream:
        deadline = time.monotonic() + LOCK_TIMEOUT
        while True:
            try:
                fcntl.flock(stream, fcntl.LOCK_EX | fcntl.LOCK_NB)
                break
            except BlockingIOError:
                if time.monotonic() >= deadline:
                    raise TimeoutError("wiki lock busy for 120 seconds")
                time.sleep(min(0.1, max(0, deadline - time.monotonic())))
        try:
            yield
        finally:
            fcntl.flock(stream, fcntl.LOCK_UN)


def verify_main(wiki):
    if Path(git(wiki, "rev-parse", "--show-toplevel").strip()).resolve() != wiki:
        raise RuntimeError("wiki directory must be the repository root")
    if git(wiki, "symbolic-ref", "--quiet", "HEAD").strip() != "refs/heads/main":
        raise RuntimeError("wiki checkout must be on main")
    for marker in ("CHERRY_PICK_HEAD", "MERGE_HEAD", "REVERT_HEAD", "rebase-merge", "rebase-apply", "sequencer"):
        path = Path(git(wiki, "rev-parse", "--git-path", marker).strip())
        if (wiki / path).exists():
            raise RuntimeError(f"wiki has an operation in progress: {marker}")


def cleanup_worktree(wiki, worktree):
    registered = [line[9:] for line in git(wiki, "worktree", "list", "--porcelain").splitlines()
                  if line.startswith("worktree ")]
    if str(worktree) in registered:
        git(wiki, "worktree", "remove", "--force", str(worktree))
    elif worktree.exists() or worktree.is_symlink():
        raise RuntimeError(f"refusing to remove an unregistered directory: {worktree}")
    git(wiki, "worktree", "prune")


def parse_review(text):
    lines = text.strip().splitlines()
    if not lines:
        raise ValueError("Codex returned no final JSON")
    value = strict_json(lines[-1])
    if (not isinstance(value, dict) or set(value) != {"accept", "problems"}
            or type(value["accept"]) is not bool or not isinstance(value["problems"], list)
            or any(not isinstance(p, str) for p in value["problems"])):
        raise ValueError("invalid Codex review schema")
    if value["accept"] and value["problems"]:
        raise ValueError("Codex accepted but also reported problems")
    return value


def check_scope(worktree, parent, task, justification, paths):
    """Check structure and link-only collateral edits before trusting either model."""
    expected = task.details.get("redirects", {})
    if justification["redirects"] != expected:
        raise ValueError("redirects do not match the planned move/merge")
    if not set(paths).issubset(task.pages):
        raise ValueError("changed an unplanned page")
    for path in paths:
        full = worktree / path
        if (full.exists() or full.is_symlink()) and not regular_page(worktree, path):
            raise ValueError(f"non-regular page: {path}")
        if path not in task.primary and path != task.details.get("destination"):
            old = page_text(worktree, parent, path)
            def replace_link(match):
                old_path = f"pages/{match[1]}.md"
                target = expected.get(old_path)
                return match[0].replace(match[1], target[6:-3], 1) if target else match[0]
            if not full.is_file() or LINK.sub(replace_link, old) != full.read_text():
                raise ValueError(f"collateral edit is not just a link update: {path}")
    for old, new in expected.items():
        if (worktree / old).exists() or not regular_page(worktree, new) or old not in paths or new not in paths:
            raise ValueError("planned deletion/move was not completed")
        # Includes frontmatter and examples conservatively; no dangling old targets.
        for page in (worktree / "pages").rglob("*.md"):
            if page.is_symlink() or any(m[1] == old[6:-3] for m in LINK.finditer(page.read_text())):
                raise ValueError(f"old link or symlink remains: {page.relative_to(worktree)}")
    if task.type == "log-growth":
        primary, destination = task.primary[0], task.details["destination"]
        if not regular_page(worktree, destination):
            raise ValueError("missing new history entry")
        for source, target in ((primary, destination), (destination, primary)):
            if not any(m[1] == target[6:-3] for m in LINK.finditer((worktree / source).read_text())):
                raise ValueError("history and current-state page must link both ways")
    allowed_deletions = set(expected)
    if any(not (worktree / p).exists() and p not in allowed_deletions for p in paths):
        raise ValueError("unplanned page deletion")


def new_broken_links(worktree, parent, paths):
    """Subtract only links already missing in this same page at the parent."""
    previous = set(filter(None, git(worktree, "ls-tree", "-r", "--name-only", "-z",
                                   parent, "--", "pages/").split("\0")))
    broken = []
    for path in sorted(paths):
        if not regular_page(worktree, path):
            continue
        old = page_text(worktree, parent, path) if path in previous else ""
        missing_before = {f"pages/{m[1]}.md" for m in LINK.finditer(old)
                          if f"pages/{m[1]}.md" not in previous}
        missing_after = {f"pages/{m[1]}.md" for m in LINK.finditer((worktree / path).read_text())
                         if not regular_page(worktree, f"pages/{m[1]}.md")}
        broken.extend({"page": path, "target": target}
                      for target in sorted(missing_after - missing_before))
    return broken


def land(wiki, parent, commit, cooldowns):
    """Shared landing gate for the service and explicit human review."""
    edits = changes(wiki, parent, commit)
    paths = {p for _, old, new in edits for p in (old, new) if p}
    removed = {old for _, old, new in edits if old and old != new}
    try:
        with wiki_lock(wiki):
            verify_main(wiki)
            dirty = status(wiki)
            reason = blocked(wiki, sorted(paths), cooldowns, time.time(), dirty)
            if reason:
                return {"outcome": "deferred", "reason": reason}
            # cherry-pick would include or reject unrelated staged changes. Never run
            # it with a nonempty index, and never unstage someone else's work.
            if any(flags[0] not in (" ", "?") for flags in dirty.values()):
                return {"outcome": "deferred", "reason": "main index has staged changes"}
            if removed:
                # Another session can add backlinks while the models run. Such a page
                # was not reviewed; a cherry-pick must not leave its link dangling.
                for path in git(wiki, "ls-tree", "-r", "--name-only", "-z", "main", "--", "pages/").split("\0"):
                    if path and path not in paths:
                        text = page_text(wiki, "main", path)
                        if any(f"pages/{m[1]}.md" in removed for m in LINK.finditer(text)):
                            return {"outcome": "deferred", "reason": f"new backlink in {path}"}
                for path in dirty:
                    if regular_page(wiki, path) and any(f"pages/{m[1]}.md" in removed for m in LINK.finditer((wiki / path).read_text())):
                        return {"outcome": "deferred", "reason": f"dirty backlink in {path}"}
            before = git(wiki, "rev-parse", "HEAD").strip()
            try:
                git(wiki, "cherry-pick", "-x", commit)
            except BaseException as error:
                # Only abort an operation that this invocation started, under the lock.
                git(wiki, "cherry-pick", "--abort", check=False)
                if git(wiki, "rev-parse", "HEAD").strip() != before or status(wiki) != dirty:
                    raise RuntimeError("cherry-pick recovery did not restore main; inspect repository") from error
                if not isinstance(error, (RuntimeError, subprocess.TimeoutExpired)):
                    raise
                return {"outcome": "deferred", "reason": f"cherry-pick failed: {error}"}
            return {"outcome": "merged", "landed_commit": git(wiki, "rev-parse", "HEAD").strip()}
    except (TimeoutError, RuntimeError) as error:
        return {"outcome": "deferred", "reason": str(error)}


def perform_task(wiki, worktree, timestamp, task, state, result):
    env = {**os.environ, "CLAUDE_WIKI_DIR": str(worktree), "CLAUDE_WIKI_SOCKET": ""}
    parent = git(worktree, "rev-parse", "HEAD").strip()
    result["parent_commit"] = parent
    gates = result["gates"]

    def reject(gate, reason, **details):
        gates[gate] = {**details, "accept": False, "reason": reason}
        result.update(outcome="rejected", reason=reason)

    tags = command(["claude-wiki", "tags"], worktree, env=env).stdout if task.type == "singleton-tags" else "Run claude-wiki tags when needed."
    argv = ["claude", "-p", "--setting-sources", "", "--no-session-persistence",
            "--permission-mode", "acceptEdits", "--max-turns", "40"]
    if os.environ.get("WIKI_CURATOR_MODEL"):
        argv += ["--model", os.environ["WIKI_CURATOR_MODEL"]]
    argv += ["--allowedTools", *ALLOWED_TOOLS]
    before = snapshot(worktree)
    try:
        process = command(argv, worktree, env=env, input=claude_prompt(task, tags),
                          timeout=CLAUDE_TIMEOUT, check=False)
    except (OSError, subprocess.TimeoutExpired) as error:
        reject("claude", str(error))
        return
    if process.returncode:
        reject("claude", f"Claude exited {process.returncode}: {process.stderr.strip()}")
        return
    gates["claude"] = {"accept": True}
    if git(worktree, "rev-parse", "HEAD").strip() != parent:
        reject("scope", "Claude changed HEAD")
        return
    after = snapshot(worktree)
    dirty = set(status(worktree)) | {p for p in before.keys() | after.keys() if before.get(p) != after.get(p)}
    page_paths = sorted(p for p in dirty if p.startswith("pages/"))
    # Check scope even on otherwise empty attempts: an out-of-scope edit is rejection.
    if any(not p.startswith(".curator/") and not safe_page(p) for p in dirty):
        reject("scope", "changed a protected page or a path outside pages/ and .curator/")
        return
    justification_path = worktree / ".curator/justification.json"
    if not page_paths:
        result["outcome"] = "no-op"
        if task.type == "duplicates" and not (worktree / ".curator").is_symlink():
            try:
                if justification_path.is_file() and not justification_path.is_symlink():
                    explanation = strict_json(justification_path.read_text())
                    if (isinstance(explanation, dict) and isinstance(explanation.get("summary"), str)
                            and explanation["summary"].strip()):
                        result["reason"] = explanation["summary"]
            except (OSError, ValueError):
                # No-op suppression does not depend on Claude supplying valid JSON.
                pass
        return
    try:
        if (worktree / ".curator").is_symlink() or justification_path.is_symlink():
            raise ValueError("justification must not be a symlink")
        justification = load_justification(justification_path)
        result["justification"] = justification
        check_scope(worktree, parent, task, justification, page_paths)
        reason = blocked(wiki, page_paths, state["cooldowns"], time.time())
        if reason:
            raise ValueError(reason)
    except (OSError, ValueError, RuntimeError) as error:
        reject("scope", str(error))
        return
    gates["scope"] = {"accept": True}
    try:
        broken = new_broken_links(worktree, parent, page_paths)
    except (OSError, ValueError, RuntimeError) as error:
        reject("broken-links", str(error))
        return
    if broken:
        reject("broken-links", "new broken wiki links", links=broken)
        return
    gates["broken-links"] = {"accept": True, "links": []}
    check_argv = ["claude-wiki", "check"]
    if task.type == "missing-queries":
        check_argv.append("--probe")
    # Include deleted paths: the wiki CLI accepts them for deletion/link validation.
    try:
        check = command([*check_argv, *page_paths], worktree, env=env, check=False)
    except (OSError, subprocess.TimeoutExpired) as error:
        reject("check", str(error))
        return
    valid_codes = (0,) if task.type == "missing-queries" else (0, 1)
    gates["check"] = {"accept": check.returncode in valid_codes, "exit_code": check.returncode,
                      "stdout": check.stdout, "stderr": check.stderr}
    if check.returncode not in valid_codes:
        result.update(outcome="rejected", reason="wiki validation failed")
        return
    if snapshot(worktree) != after:
        reject("check", "page validation changed candidate files")
        return
    # Validation is allowed to update ignored indexes, but never candidate content.
    git(worktree, "reset", "--mixed", parent)
    git(worktree, "add", "--", *page_paths)
    message = (f"curator: {task.type}: {justification['summary']}\n\n"
               f"Task: {json.dumps(task.data(), ensure_ascii=False)}\n\n"
               f"Justification: {json.dumps(justification, ensure_ascii=False)}\n\n"
               f"Curator-Run: {timestamp}\n")
    git(worktree, "commit", "-F", "-", input=message)
    commit = git(worktree, "rev-parse", "HEAD").strip()
    result["commit"] = commit
    try:
        missing = preserved(worktree, parent, commit, justification)
    except (ValueError, RuntimeError) as error:
        reject("preservation", str(error))
        return
    gates["preservation"] = {"accept": not missing, "missing_tokens": missing}
    if missing:
        result.update(outcome="rejected", reason="preservation check failed")
        return
    try:
        gates["recall"] = recall(worktree, parent, commit, justification, env)
        if (git(worktree, "rev-parse", "HEAD").strip() != commit or snapshot(worktree) != after
                or any(not p.startswith(".curator/") for p in status(worktree))):
            raise ValueError("recall validation changed candidate files")
    except (OSError, ValueError, RuntimeError, subprocess.TimeoutExpired) as error:
        reject("recall", str(error))
        return
    if not gates["recall"]["accept"]:
        reject("recall", "recall check failed", **gates["recall"])
        return
    output = worktree / ".curator/codex-review.txt"
    output.unlink(missing_ok=True)
    try:
        review = command(["codex", "exec", "--skip-git-repo-check", "-s", "read-only",
                          "--color", "never", "-o", str(output), "-"], worktree, env=env,
                         input=review_prompt(task, justification, changed_texts(worktree, parent, commit), gates),
                         timeout=CODEX_TIMEOUT, check=False)
        if review.returncode:
            raise ValueError(f"Codex exited {review.returncode}: {review.stderr.strip()}")
        if output.is_symlink() or not output.is_file():
            raise ValueError("Codex output must be a regular file")
        verdict = parse_review(output.read_text())
    except (OSError, ValueError, subprocess.TimeoutExpired) as error:
        reject("codex", str(error))
        result["codex_problems"] = [str(error)]
        return
    gates["codex"] = verdict
    result["codex_problems"] = verdict["problems"]
    if not verdict["accept"]:
        result.update(outcome="rejected", reason="Codex rejected the change")
        return
    if (git(worktree, "rev-parse", "HEAD").strip() != commit or snapshot(worktree) != after
            or any(not p.startswith(".curator/") for p in status(worktree))):
        reject("codex", "worktree changed during read-only review")
        return
    landing = land(wiki, parent, commit, state["cooldowns"])
    gates["landing"] = landing.copy()
    result.update(landing)


def summary(report):
    lines = [f"Wiki curator: {report['timestamp']}", f"Wiki: {report['wiki']}"]
    if report.get("error"):
        lines.append(f"Run failed: {report['error']}")
    if not report["tasks"]:
        lines.append("No eligible tasks.")
    for result in report["tasks"]:
        evidence = f"; diff: {result['diff']}" if result.get("diff") else ""
        lines.append(f"- {result['outcome']}: {result['type']} ({', '.join(result['pages'])}){evidence}")
        for key in ("reason", "branch", "commit", "landed_commit"):
            if result.get(key):
                lines.append(f"  {key}: {result[key]}")
    return "\n".join(lines) + "\n"


def run(wiki, cache, max_tasks=5, only=None):
    wiki, cache = wiki.expanduser().resolve(), cache.expanduser().resolve()
    cache.mkdir(parents=True, exist_ok=True)
    with (cache / "run.lock").open("a") as lock:
        try:
            fcntl.flock(lock, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            print("wiki-curator is already running; skipped.")
            return 0
        timestamp = datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%S.%fZ")
        report = {"timestamp": timestamp, "wiki": str(wiki), "tasks": []}
        runs = cache / "runs"
        runs.mkdir(exist_ok=True)
        worktree = cache / "worktree"
        registered = False
        try:
            state = read_state(cache)
            verify_main(wiki)
            cleanup_worktree(wiki, worktree)
            env = {**os.environ, "CLAUDE_WIKI_DIR": str(wiki)}
            lint = command(["claude-wiki", "lint", "--json"], wiki, env=env)
            tasks = select_tasks(wiki, strict_json(lint.stdout), state["cooldowns"], time.time(), max_tasks, only,
                                 not_duplicates=state["not_duplicates"])
            registered = True
            git(wiki, "worktree", "add", "--detach", str(worktree), "main")
            for number, task in enumerate(tasks, 1):
                git(worktree, "checkout", "--detach", "--force", "main")
                git(worktree, "clean", "-fd")
                curator = worktree / ".curator"
                if curator.is_symlink() or curator.is_file():
                    curator.unlink()
                elif curator.exists():
                    shutil.rmtree(curator)
                result = {**task.data(), "outcome": "error", "gates": {}, "codex_problems": []}
                report["tasks"].append(result)
                reason = blocked(wiki, task.pages, state["cooldowns"], time.time())
                try:
                    if reason:
                        result.update(outcome="deferred", reason=reason)
                    else:
                        perform_task(wiki, worktree, timestamp, task, state, result)
                except Interrupted as error:
                    result.update(outcome="rejected", reason=str(error))
                    report["error"] = str(error)
                except Exception as error:
                    result.update(outcome="error", reason=f"{type(error).__name__}: {error}")
                if task.type == "duplicates" and result["outcome"] == "no-op":
                    state["not_duplicates"][pair_key(*task.primary)] = time.time() + 90 * DAY
                    write_json(cache / "state.json", state)
                if result["outcome"] in ("rejected", "deferred", "error"):
                    days = 1 if result["outcome"] == "deferred" else 14
                    for path in task.pages:
                        state["cooldowns"][path] = max(state["cooldowns"].get(path, 0), time.time() + days * DAY)
                    # Persist before branch creation so a git failure cannot bypass cooldown.
                    write_json(cache / "state.json", state)
                    if result.get("commit"):
                        label = "deferred" if result["outcome"] == "deferred" else "rejected"
                        branch = f"curator/{label}/{timestamp}-{number}"
                        git(wiki, "branch", branch, result["commit"])
                        result["branch"] = branch
                    elif result.get("parent_commit") and result["outcome"] != "deferred":
                        diff = save_rejection_diff(worktree, result["parent_commit"], runs / f"{timestamp}-{number}.diff")
                        if diff:
                            result["diff"] = diff
                write_json(runs / f"{timestamp}.json", report)
                if report.get("error"):
                    break
            write_json(cache / "state.json", state)
        except Exception as error:
            report["error"] = f"{type(error).__name__}: {error}"
        finally:
            if registered:
                try:
                    cleanup_worktree(wiki, worktree)
                except Exception as error:
                    report["error"] = f"{report.get('error', '')} Worktree cleanup failed: {error}".strip()
            write_json(runs / f"{timestamp}.json", report)
            text = summary(report)
            atomic_write(cache / "latest.md", text)
            print(text, end="")
        return 1 if report.get("error") else 0


def show_status(wiki, cache):
    latest = cache / "latest.md"
    print(latest.read_text() if latest.exists() else "No curator runs recorded.\n", end="")
    branches = git(wiki, "for-each-ref", "--format=%(refname:short)", "refs/heads/curator/rejected/")
    print("\nRejected branches:\n" + (branches or "(none)\n"), end="")
