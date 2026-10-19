"""Inspect and explicitly resolve retained curator work, without running models."""

from contextlib import contextmanager
import fcntl
import json
import subprocess
import sys

from .gitutil import changes, git, safe_page
from .preservation import strict_json
from .runner import artifact_key, land, read_state, review_details, write_json


PREFIXES = ("curator/rejected/", "curator/deferred/")
DIFF_LIMIT = 1024 * 1024
DIFF_TRUNCATED = b"\n[wiki-curator: diff truncated at 1 MiB]\n"


class RunInProgress(Exception):
    pass


@contextmanager
def run_lock(cache):
    cache.mkdir(parents=True, exist_ok=True)
    with (cache / "run.lock").open("a") as stream:
        try:
            fcntl.flock(stream, fcntl.LOCK_EX | fcntl.LOCK_NB)
        except BlockingIOError:
            raise RunInProgress("curator run in progress") from None
        try:
            yield
        finally:
            fcntl.flock(stream, fcntl.LOCK_UN)


def branch_commit(wiki, branch):
    if not branch.startswith(PREFIXES):
        raise ValueError("expected a curator/rejected/ or curator/deferred/ branch")
    ref = f"refs/heads/{branch}"
    git(wiki, "check-ref-format", ref)
    if git(wiki, "symbolic-ref", "--quiet", ref, check=False).strip():
        raise ValueError("curator branch must not be a symbolic ref")
    # An exact ref lookup prevents revision expressions, tags, and option injection.
    return git(wiki, "show-ref", "--verify", "--hash", ref).strip()


def task_report(cache, timestamp, number):
    if timestamp is None:
        return {}
    try:
        report = strict_json((cache / "runs" / f"{timestamp}.json").read_text())
        tasks = report.get("tasks") if isinstance(report, dict) else None
        if isinstance(tasks, list) and number <= len(tasks) and isinstance(tasks[number - 1], dict):
            return tasks[number - 1]
    except (OSError, ValueError, RecursionError):
        pass
    return {}


def item(cache, branch, *, commit=None):
    key = branch.split("/", 2)[2]
    try:
        timestamp, number, _ = artifact_key(key)
    except ValueError:
        timestamp, number = None, None
    task = task_report(cache, timestamp, number)

    def string(value):
        return value if isinstance(value, str) else None

    def strings(value):
        return value if isinstance(value, list) and all(isinstance(v, str) for v in value) else []

    explanation = task.get("justification")
    gates = task.get("gates")
    adjudication = gates.get("adjudicate") if isinstance(gates, dict) else None
    return {
        "id": branch, "kind": "branch", "branch": branch, "run": timestamp, "task": number,
        "type": string(task.get("type")), "pages": strings(task.get("pages")),
        "outcome": branch.split("/")[1],
        "reason": string(task.get("reason")), "codex_problems": strings(task.get("codex_problems")),
        "adjudicate": adjudication if isinstance(adjudication, dict) else None,
        "summary": string(explanation.get("summary")) if isinstance(explanation, dict) else None,
        "commit": commit,
    }


def review_items(wiki, cache):
    entries = []
    refs = git(wiki, "for-each-ref", "--format=%(refname)\t%(objectname)\t%(committerdate:unix)",
               *(f"refs/heads/{prefix}" for prefix in PREFIXES))
    for line in refs.splitlines():
        ref, commit, committed = line.split("\t")
        branch = ref[len("refs/heads/"):]
        entry = item(cache, branch, commit=commit)
        try:
            _, _, created = artifact_key(branch.split("/", 2)[2])
        except ValueError:
            created = int(committed)
        entries.append((created, entry["task"] or 0, entry))
    entries.sort(key=lambda entry: (entry[0], entry[1], entry[2]["id"]), reverse=True)
    return [entry for _, _, entry in entries]


def show(wiki, branch):
    commit = branch_commit(wiki, branch)
    content = git(wiki, "show", "-M", "--stat", "-p", "--no-ext-diff", "--no-textconv",
                  "--no-color", commit, "--", text=False)
    if len(content) > DIFF_LIMIT:
        content = content[:DIFF_LIMIT - len(DIFF_TRUNCATED)] + DIFF_TRUNCATED
    return content


def land_branch(wiki, cache, branch):
    commit = branch_commit(wiki, branch)
    if f"branch refs/heads/{branch}" in git(wiki, "worktree", "list", "--porcelain").splitlines():
        raise ValueError("curator branch is checked out in a worktree; cannot delete it after landing")
    parents = git(wiki, "rev-list", "--parents", "-n", "1", commit).split()
    if len(parents) != 2:
        raise ValueError("branch must have exactly one commit on top of an ancestor of main")
    parent = parents[1]
    # If the sole parent is an ancestor, only the tip can be ahead of main. Also
    # reject tips already on main; they are not pending work.
    base = git(wiki, "merge-base", "main", parent, check=False).strip()
    if base != parent or git(wiki, "rev-list", "--count", f"main..{commit}").strip() != "1":
        raise ValueError("branch must have exactly one commit on top of an ancestor of main")
    state = read_state(cache)
    entry = item(cache, branch, commit=commit)
    paths = {p for _, old, new in changes(wiki, parent, commit) for p in (old, new) if p}
    pages = paths | {p for p in entry["pages"] if safe_page(p)}
    # Explicit review overrides this task's cooldown, without changing persisted
    # state unless landing succeeds. All other landing checks are shared with run.
    cooldowns = {p: until for p, until in state["cooldowns"].items() if p not in pages}
    result = land(wiki, parent, commit, cooldowns)
    if result["outcome"] == "merged":
        for path in pages:
            state["cooldowns"].pop(path, None)
        write_json(cache / "state.json", state)
        git(wiki, "branch", "-D", "--", branch)
    return result


def discard(wiki, branch):
    branch_commit(wiki, branch)
    git(wiki, "branch", "-D", "--", branch)
    return {"outcome": "discarded"}


def print_result(result, as_json):
    if as_json:
        print(json.dumps(result, ensure_ascii=False))
    else:
        detail = result.get("reason") or result.get("landed_commit")
        print(result["outcome"] + (f": {detail}" if detail else ""))


def execute(args, cache):
    wiki, cache = args.wiki.expanduser().resolve(), cache.expanduser().resolve()
    as_json = getattr(args, "json", False)
    try:
        if args.command == "review":
            items = review_items(wiki, cache)
            if as_json:
                print(json.dumps({"items": items}, ensure_ascii=False))
            else:
                for entry in items:
                    # Keep reports with embedded newlines on one line per item.
                    fields = [entry["id"], entry["outcome"], entry["type"], ", ".join(entry["pages"]),
                              entry["summary"], entry["reason"]]
                    print(" | ".join(" ".join(value.splitlines()) for value in fields if value))
                    for line in review_details(entry["codex_problems"], entry["adjudicate"]):
                        print(line)
            return 0
        if args.command == "show":
            sys.stdout.buffer.write(show(wiki, args.id))
            return 0
        with run_lock(cache):
            result = (land_branch(wiki, cache, args.id) if args.command == "land"
                      else discard(wiki, args.id))
        print_result(result, as_json)
        return 3 if result["outcome"] == "deferred" else 0
    except RunInProgress as error:
        print_result({"outcome": "deferred", "reason": str(error)}, as_json)
        return 3
    except (OSError, ValueError, RuntimeError, subprocess.TimeoutExpired) as error:
        print_result({"outcome": "refused", "reason": str(error)}, as_json)
        return 2
