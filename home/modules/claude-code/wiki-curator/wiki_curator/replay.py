"""Read-only replay of preservation checks against historical curator commits."""

import argparse
import json
from pathlib import Path
import subprocess

from .gitutil import git
from .preservation import preserved, strict_json, validate_justification


def commit_justification(message):
    entries = [line.removeprefix("Justification: ") for line in message.splitlines()
               if line.startswith("Justification: ")]
    if len(entries) != 1:
        raise ValueError("expected exactly one Justification: JSON line in commit body")
    return validate_justification(strict_json(entries[0]))


def replay(repo, *, all_refs=False):
    """Yield results without checking out revisions or writing indexes/caches."""
    commits = git(repo, "log", *(["--all"] if all_refs else []),
                  "--grep=^curator:", "--format=%H", "--").splitlines()
    for commit in commits:
        subject = git(repo, "show", "-s", "--format=%s", commit).strip()
        # --grep also searches bodies; only curator subjects identify candidates.
        if not subject.startswith("curator:"):
            continue
        result = {"commit": commit, "subject": subject, "missing": [], "error": None}
        try:
            parents = git(repo, "show", "-s", "--format=%P", commit).split()
            if not parents:
                raise ValueError("curator commit has no parent")
            explanation = commit_justification(git(repo, "show", "-s", "--format=%b", commit))
            result["missing"] = preserved(repo, parents[0], commit, explanation)
        except (OSError, RuntimeError, ValueError, subprocess.TimeoutExpired) as error:
            result["error"] = str(error)
        yield result


def main(argv=None):
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("repo", type=Path, help="wiki repository to inspect (required)")
    parser.add_argument("--all", action="store_true", dest="all_refs",
                        help="include all refs, including retained rejected/deferred candidates")
    args = parser.parse_args(argv)
    checked = failed = 0
    for result in replay(args.repo.resolve(), all_refs=args.all_refs):
        checked += 1
        if result["missing"] or result["error"]:
            failed += 1
            reason = result["error"] or "missing tokens: " + json.dumps(result["missing"], ensure_ascii=False)
            print(f"{result['commit']} {result['subject']}: {reason}")
    print(f"Checked {checked} curator commits; {failed} failed.")
    return int(failed > 0)


if __name__ == "__main__":
    raise SystemExit(main())
