#!/usr/bin/env python3
"""Replay the local miss study with a built CLI, writing only temporary copies.

Usage: python3 replay-recall.py /absolute/path/to/claude-wiki --output /tmp/replay.json
The study itself and its transcript inputs are read-only. --explain measures each
output independently, like m6/m7, without consuming session dedupe or budgets.
"""
import argparse
from collections import Counter
import json
import hashlib
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile
import types


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("binary", type=Path)
    parser.add_argument("--study", type=Path, default=Path.home() / ".cache/wiki-bench/miss")
    parser.add_argument("--output", type=Path, required=True)
    args = parser.parse_args()
    binary = str(args.binary.resolve())
    study = args.study.resolve()
    sys.path.insert(0, str(study / "scripts"))
    # Preserve m7's exact seed, ordering, sampling and snapshot selection. Retain
    # the source command as well, so the real CLI exercises its reader exclusion.
    cost = types.ModuleType("study_cost")
    cost.__file__ = str(study / "scripts/m7_cost.py")
    code = (study / "scripts/m7_cost.py").read_text().replace(
        "items.append(dict(sid=", "items.append(dict(command=cmd, sid=")
    code = code.replace("MISS = f'{HOME}/.cache/wiki-bench/miss'", f"MISS = {str(study)!r}")
    exec(compile(code, str(study / "scripts/m7_cost.py"), "exec"), cost.__dict__)
    m3 = {x["idx"]: x for x in json.loads((study / "out/m3.json").read_text())}
    m4 = json.loads((study / "out/m4.json").read_text())
    real = set(m3) - {1, 62, 102, 3, 23, 71, 96, 99, 100}
    expected = {11, 14, 34, 35, 61, 69, 77, 82, 92, 105}
    catches = set()
    pair_results = []
    counts = Counter()
    with tempfile.TemporaryDirectory(prefix="wiki-recall-replay-") as directory:
        root = Path(directory)
        replay_binary = root / "claude-wiki"
        shutil.copy2(binary, replay_binary)
        digest = hashlib.sha256(replay_binary.read_bytes()).hexdigest()
        copies = {}

        def snapshot(original):
            if original not in copies:
                dest = root / str(len(copies))
                # copy2 retains mtimes, which break equal-score ties in the study.
                shutil.copytree(Path(original) / "pages", dest / "pages")
                # Reuse the study index to preserve the original ranking metadata;
                # the binary can refresh the copy if its format has changed.
                shutil.copy2(Path(original) / "index.sqlite", dest / "index.sqlite")
                copies[original] = dest
            return copies[original]

        def recall(original, text, command, failed, cwd=""):
            dest = snapshot(original)
            for spelling in (str(Path.home() / ".local/share/claude-wiki"),
                             "~/.local/share/claude-wiki", "$HOME/.local/share/claude-wiki",
                             "${HOME}/.local/share/claude-wiki"):
                command = command.replace(spelling, str(dest))
            payload = dict(session_id="replay", transcript_path=None, cwd=cwd,
                           hook_event_name="PostToolUseFailure" if failed else "PostToolUse",
                           tool_name="Bash", tool_input={"command": command},
                           error=text if failed else None,
                           tool_response={"stdout": text, "exit_code": int(failed)})
            env = dict(os.environ, CLAUDE_WIKI_DIR=str(dest),
                       CLAUDE_WIKI_SOCKET=str(root / "dead-socket"),
                       CLAUDE_WIKI_HOOK_LOCK_MS="20000", XDG_STATE_HOME=str(root / "state"))
            run = subprocess.run([str(replay_binary), "recall", "--explain"], input=json.dumps(payload),
                                 capture_output=True, text=True, env=env, timeout=120, check=True)
            return json.loads(run.stdout)["selected"]

        for row in m4:
            if row["idx"] not in real or row["gate"].startswith("wiki-operation"):
                continue
            item = m3[row["idx"]]
            occurrence = item["occ"][row["occ"]]
            selected = recall(item["snap"], occurrence["text"], occurrence["input"].get("command", ""),
                              occurrence["is_error"], occurrence["cwd"] or item["cwd"])
            caught = item["page"] in selected
            if caught:
                catches.add(item["idx"])
            pair_results.append(dict(pair=item["idx"], occurrence=row["occ"], caught=caught,
                                     selected=selected))
        print(f"pairs: {len(catches)}/{len(real)} {sorted(catches)}", flush=True)
        for number, item in enumerate(cost.sample, 1):
            selected = recall(cost.snap_for(item["t"]), item["text"], item["command"], item["err"])
            group = "failure" if item["err"] else "success"
            counts[group + "_outputs"] += 1
            counts[group + "_injections"] += bool(selected)
            if number % 200 == 0:
                print(f"sample: {number}/{len(cost.sample)}", flush=True)
    report = dict(binary=binary, sha256=digest,
                  pairs=len(real), caught=sorted(catches), expected=sorted(expected),
                  missing=sorted(expected - catches), noise=dict(counts), occurrences=pair_results)
    args.output.write_text(json.dumps(report, indent=2) + "\n")
    print(json.dumps({key: value for key, value in report.items() if key != "occurrences"}, indent=2))


if __name__ == "__main__":
    main()
