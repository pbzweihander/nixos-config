"""Copied as three executables by tests. Never invoked against the user's wiki."""

import json
import os
from pathlib import Path
import subprocess
import sys
import time


fixture = json.loads(Path(os.environ["CURATOR_FIXTURE"]).read_text())
name = Path(sys.argv[0]).name
prompt = sys.stdin.read()
head = subprocess.run(["git", "rev-parse", "HEAD"], check=True, capture_output=True, text=True).stdout.strip()
with Path(os.environ["CURATOR_CALLS"]).open("a") as stream:
    stream.write(json.dumps({"name": name, "args": sys.argv[1:], "cwd": os.getcwd(),
                             "wiki": os.environ.get("CLAUDE_WIKI_DIR"),
                             "socket": os.environ.get("CLAUDE_WIKI_SOCKET"),
                             "prompt": prompt, "head": head}) + "\n")


def git(root, *args):
    subprocess.run(["git", "-C", str(root), *args], check=True, capture_output=True)


def edits(root, operations):
    for path, text in operations.get("writes", {}).items():
        target = root / path
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_text(text)
    for old, new in operations.get("moves", {}).items():
        (root / new).parent.mkdir(parents=True, exist_ok=True)
        git(root, "mv", old, new)
    for path in operations.get("deletes", []):
        git(root, "rm", path)


if name == "claude-wiki":
    if sys.argv[1] == "lint":
        print(fixture.get("lint_raw", json.dumps(fixture["lint"])))
        sys.exit(fixture.get("lint_exit", 0))
    if sys.argv[1] == "tags":
        print("nix (10)\nlinux (5)")
        sys.exit(0)
    if sys.argv[1] == "check":
        print("fake page validation")
        if fixture.get("revision_check_exit") and any(
                json.loads(line)["name"] == "codex"
                for line in Path(os.environ["CURATOR_CALLS"]).read_text().splitlines()):
            sys.exit(fixture["revision_check_exit"])
        sys.exit(fixture.get("check_exit", 0))
    if sys.argv[1] == "search":
        revision = "parent" if head == fixture.get("parent_commit") else "commit"
        query = " ".join(sys.argv[sys.argv.index("--") + 1:])
        hits = fixture.get("search", {}).get(revision, {}).get(query, [])
        print(fixture.get("search_raw", {}).get(revision, json.dumps({"hits": [{"rel": p} for p in hits]})))
        sys.exit(fixture.get("search_exit", {}).get(revision, 0))
    raise SystemExit("unexpected wiki command")
elif name == "claude":
    if "--tools" in sys.argv:
        time.sleep(fixture.get("adjudicate_sleep", 0))
        edits(Path.cwd(), fixture.get("adjudicate_writes", {}))
        print(fixture.get("adjudicate", '{"verdict": "uphold", "reason": "검토 필요", "edits": []}'))
        sys.exit(fixture.get("adjudicate_exit", 0))
    if fixture.get("wait_for"):
        while not Path(fixture["wait_for"]).exists():
            time.sleep(0.02)
    time.sleep(fixture.get("claude_sleep", 0))
    edits(Path.cwd(), fixture)
    if "justification" in fixture:
        path = Path(".curator/justification.json")
        path.parent.mkdir(exist_ok=True)
        path.write_text(json.dumps(fixture["justification"]))
        if fixture.get("stage_justification"):
            git(Path.cwd(), "add", ".curator")
    sys.exit(fixture.get("claude_exit", 0))
elif name == "codex":
    count = sum(json.loads(line)["name"] == "codex"
                for line in Path(os.environ["CURATOR_CALLS"]).read_text().splitlines())
    if count > 1:
        fixture.update(fixture.get("second_review", {}))
    time.sleep(fixture.get("codex_sleep", 0))
    if fixture.get("move_main"):
        main = Path(os.environ["CURATOR_MAIN"])
        operations = fixture["move_main"]
        edits(main, operations)
        git(main, "add", "--", *operations["writes"])
        git(main, "commit", "-m", "concurrent session")
        if fixture.get("backdate_main"):
            for path in operations["writes"]:
                os.utime(main / path, (1, 1))
    edits(Path.cwd(), fixture.get("review_edits", {}))
    output = Path(sys.argv[sys.argv.index("-o") + 1])
    if not fixture.get("no_review_file"):
        output.write_text(fixture.get("review", '{"accept": true, "problems": []}') + "\n")
    print("stdout is not the review")
    sys.exit(fixture.get("codex_exit", 0))
raise SystemExit("unexpected fake executable")
