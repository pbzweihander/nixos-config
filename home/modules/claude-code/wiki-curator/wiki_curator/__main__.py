import argparse
import json
import os
from pathlib import Path
import signal

from .preservation import load_justification, preserved
from .review import execute as review_command
from .runner import Interrupted, run, show_status
from .selection import TYPES


def positive(value):
    number = int(value)
    if number < 0:
        raise argparse.ArgumentTypeError("must be nonnegative")
    return number


def main(argv=None):
    parser = argparse.ArgumentParser(prog="wiki-curator")
    commands = parser.add_subparsers(dest="command", required=True)
    run_parser = commands.add_parser("run")
    run_parser.add_argument("--max-tasks", type=positive, default=os.environ.get("WIKI_CURATOR_MAX_TASKS", "5"))
    run_parser.add_argument("--only", choices=TYPES, nargs="+", action="extend")
    status_parser = commands.add_parser("status")
    review_parser = commands.add_parser("review", help="list retained rejected and deferred work")
    review_parser.add_argument("--json", action="store_true")
    show_parser = commands.add_parser("show", help="inspect a retained curator branch")
    show_parser.add_argument("id")
    land_parser = commands.add_parser("land", help="land a retained curator branch")
    land_parser.add_argument("id", metavar="branch")
    land_parser.add_argument("--json", action="store_true")
    discard_parser = commands.add_parser("discard", help="discard a retained curator branch")
    discard_parser.add_argument("id")
    discard_parser.add_argument("--json", action="store_true")
    for child in (run_parser, status_parser, review_parser, show_parser, land_parser, discard_parser):
        child.add_argument("--wiki", type=Path, default=Path(os.environ.get("CLAUDE_WIKI_DIR", "~/.local/share/claude-wiki")).expanduser())
    preservation_parser = commands.add_parser("check-preservation")
    preservation_parser.add_argument("parent")
    preservation_parser.add_argument("commit")
    preservation_parser.add_argument("--justification", type=Path)
    args = parser.parse_args(argv)
    cache = Path(os.environ.get("XDG_CACHE_HOME", "~/.cache")).expanduser() / "wiki-curator"

    def interrupted(signum, frame):
        raise Interrupted(f"interrupted by signal {signum}")

    signal.signal(signal.SIGTERM, interrupted)
    signal.signal(signal.SIGINT, interrupted)
    try:
        if args.command == "run":
            return run(args.wiki, cache, args.max_tasks, args.only)
        if args.command == "status":
            show_status(args.wiki.expanduser().resolve(), cache)
            return 0
        if args.command in ("review", "show", "land", "discard"):
            return review_command(args, cache)
        justification = load_justification(args.justification) if args.justification else {
            "summary": "manual preservation check", "dropped": [], "redirects": {},
        }
        missing = preserved(Path.cwd(), args.parent, args.commit, justification)
        print(json.dumps({"missing_tokens": missing}, ensure_ascii=False))
        return 1 if missing else 0
    except (OSError, ValueError, RuntimeError) as error:
        parser.exit(1, f"wiki-curator: {error}\n")


if __name__ == "__main__":
    raise SystemExit(main())
