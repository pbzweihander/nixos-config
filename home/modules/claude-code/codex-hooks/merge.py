#!/usr/bin/env python3
"""Merge wiki hooks into mutable CODEX_HOMEs, preserving foreign hook positions.

Codex itself computes trust hashes in an empty temporary home. No credentials,
user config, plugins or project hooks are copied into that home or executed.
"""

import argparse
import copy
import json
import os
from pathlib import Path
import select
import shlex
import stat
import subprocess
import tempfile
import time
import tomllib

import tomlkit

MARKER = ": claude-wiki-hook;"
BEGIN = "<!-- claude-wiki:begin -->"
END = "<!-- claude-wiki:end -->"


def owned(handler):
    return isinstance(handler, dict) and handler.get("command", "").startswith(MARKER)


def definitions(wiki):
    result = {}
    for event, command in (
        ("SessionStart", "context"),
        ("UserPromptSubmit", "remind"),
        ("PostToolUse", "recall"),
        ("Stop", "record"),
    ):
        group = {"hooks": [{
            "type": "command",
            "command": f"{MARKER} {shlex.quote(wiki)} {command}" + ("" if command == "record" else " --harness codex"),
            "timeout": 5,
        }]}
        if event == "PostToolUse":
            group["matcher"] = "Bash"
            group["hooks"].append({
                "type": "command",
                "command": f"{MARKER} {shlex.quote(wiki)} remind --event --harness codex",
                "timeout": 2,
            })
        result[event] = group
    return result


def merge_hooks(document, desired):
    result = copy.deepcopy(document)
    if not isinstance(result, dict):
        raise ValueError("hooks.json must contain an object")
    hooks = result.setdefault("hooks", {})
    if not isinstance(hooks, dict):
        raise ValueError("hooks must be an object")
    for event, groups in hooks.items():
        if not isinstance(groups, list):
            raise ValueError(f"{event} must be an array")
        for group in groups:
            if not isinstance(group, dict) or not isinstance(group.get("hooks"), list):
                raise ValueError(f"{event} contains an invalid hook group")
            if any(not isinstance(h, dict) for h in group["hooks"]):
                raise ValueError(f"{event} contains an invalid handler")
    for event in hooks.keys() | desired.keys():
        groups = hooks.setdefault(event, [])
        installed = False
        for index, group in enumerate(groups):
            handlers = group["hooks"]
            if handlers and all(owned(h) for h in handlers):
                if event in desired and not installed:
                    groups[index] = copy.deepcopy(desired[event])
                    installed = True
                else:
                    # Empty groups keep later foreign groups at their trusted indexes.
                    groups[index] = {"hooks": []}
            else:
                for index, handler in enumerate(handlers):
                    if owned(handler):
                        # Mixed groups also need stable handler indexes and matchers.
                        handlers[index] = {"type": "command", "command": f"{MARKER} :", "timeout": 1}
        if event in desired and not installed:
            groups.append(copy.deepcopy(desired[event]))
    return result


def hook_listing(codex, home):
    env = {k: v for k, v in os.environ.items()
           if not k.startswith(("CODEX_", "ORCA_", "OPENAI_"))}
    env["CODEX_HOME"] = str(home)
    env["HOME"] = str(home)
    env["XDG_STATE_HOME"] = str(home / "state")
    env["XDG_CONFIG_HOME"] = str(home / "config")
    env["XDG_CACHE_HOME"] = str(home / "cache")
    with tempfile.TemporaryFile() as errors:
        process = subprocess.Popen(
            [codex, "app-server", "--stdio"], cwd=home, env=env,
            stdin=subprocess.PIPE, stdout=subprocess.PIPE, stderr=errors,
        )
        pending = b""

        def request(number, method, params):
            nonlocal pending
            message = {"id": number, "method": method, "params": params}
            process.stdin.write((json.dumps(message) + "\n").encode())
            process.stdin.flush()
            deadline = time.monotonic() + 15
            while True:
                while b"\n" in pending:
                    line, pending = pending.split(b"\n", 1)
                    response = json.loads(line)
                    if response.get("id") == number:
                        if "error" in response:
                            raise ValueError(f"Codex {method}: {response['error']}")
                        return response["result"]
                remaining = deadline - time.monotonic()
                if remaining <= 0 or not select.select([process.stdout], [], [], remaining)[0]:
                    raise ValueError(f"Codex {method} timed out")
                chunk = os.read(process.stdout.fileno(), 65536)
                if not chunk:
                    raise ValueError(f"Codex stopped during {method}")
                pending += chunk

        try:
            request(1, "initialize", {
                "clientInfo": {"name": "claude-wiki-hook-installer", "version": "1"},
                "capabilities": {"experimentalApi": True},
            })
            listing = request(2, "hooks/list", {"cwds": [str(home)]})
            return [hook for item in listing["data"] for hook in item["hooks"]]
        finally:
            process.terminate()
            try:
                process.wait(timeout=5)
            except subprocess.TimeoutExpired:
                process.kill()
                process.wait()
            process.stdin.close()
            process.stdout.close()


def trust_hashes(codex, desired):
    # Ask the installed runtime, rather than pinning an undocumented hash algorithm.
    with tempfile.TemporaryDirectory(prefix="claude-wiki-codex-") as temporary:
        home = Path(temporary).resolve()
        document = {"hooks": {event: [group] for event, group in desired.items()}}
        home.joinpath("hooks.json").write_text(json.dumps(document))
        home.joinpath("config.toml").write_text('cli_auth_credentials_store = "file"\n')
        hashes = {}
        for hook in hook_listing(codex, home):
            if hook["sourcePath"] != str(home / "hooks.json"):
                continue
            label = hook["key"].rsplit(":", 3)[1]
            hashes[(label, hook.get("command"))] = hook["currentHash"]
        if len(hashes) != sum(len(group["hooks"]) for group in desired.values()):
            raise ValueError("Codex did not recognize all wiki hooks; review them with /hooks")
        return hashes


def event_label(event):
    return "".join("_" + c.lower() if c.isupper() else c for c in event).lstrip("_")


def merge_trust(text, hooks_path, hooks, hashes):
    config = tomlkit.parse(text)
    states = config.setdefault("hooks", {}).setdefault("state", {})
    for event, groups in hooks["hooks"].items():
        label = event_label(event)
        for i, group in enumerate(groups):
            for j, handler in enumerate(group["hooks"]):
                if not owned(handler):
                    continue
                key = f"{hooks_path}:{label}:{i}:{j}"
                state = states.setdefault(key, {})
                # Disable tombstones so they cannot trigger a pointless trust prompt.
                digest = hashes.get((label, handler["command"]))
                if digest is None:
                    state["enabled"] = False
                    continue
                state["enabled"] = True
                state["trusted_hash"] = digest
    result = tomlkit.dumps(config)
    tomllib.loads(result)
    return result


def merge_rules(text, rules):
    block = f"{BEGIN}\n{rules.rstrip()}\n{END}"
    if BEGIN in text or END in text:
        if text.count(BEGIN) != 1 or text.count(END) != 1:
            raise ValueError("ambiguous wiki markers in AGENTS.md")
        start, end = text.index(BEGIN), text.index(END)
        if end < start:
            raise ValueError("reversed wiki markers in AGENTS.md")
        return text[:start] + block + text[end + len(END):]
    return text + ("\n\n" if text and not text.endswith("\n\n") else "") + block + "\n"


def read(path):
    # These files belong to the user/Orca, never replace a Nix-managed symlink.
    if path.is_symlink():
        raise ValueError(f"refusing to replace symlink {path}")
    return path.read_text() if path.exists() else None


def atomic_write(path, text, mode):
    descriptor, name = tempfile.mkstemp(prefix=f".{path.name}.", dir=path.parent)
    try:
        with os.fdopen(descriptor, "w") as output:
            os.fchmod(output.fileno(), mode)
            output.write(text)
            output.flush()
            os.fsync(output.fileno())
        os.replace(name, path)
    finally:
        if os.path.exists(name):
            os.unlink(name)


def install(home, desired, hashes, rules):
    if not home.is_dir():
        return
    hooks_path, config_path = home / "hooks.json", home / "config.toml"
    rules_path = home / "AGENTS.md"
    originals = {path: read(path) for path in (hooks_path, config_path, rules_path)}
    old_hooks = json.loads(originals[hooks_path] or "{}")
    hooks = merge_hooks(old_hooks, desired)
    updated = {
        hooks_path: originals[hooks_path] if hooks == old_hooks else json.dumps(hooks, indent=2) + "\n",
        config_path: merge_trust(originals[config_path] or "", hooks_path, hooks, hashes),
        rules_path: merge_rules(originals[rules_path] or "", rules),
    }
    # Prepare and validate everything before touching any destination. Reject a
    # concurrent Orca/user write; the next activation can safely retry.
    for path, original in originals.items():
        if read(path) != original:
            raise ValueError(f"{path} changed while preparing the merge")
    for path, text in updated.items():
        original = originals[path]
        if text == original:
            continue
        if read(path) != original:
            raise ValueError(f"{path} changed before writing")
        mode = stat.S_IMODE(path.stat().st_mode) if path.exists() else 0o600
        if original is not None:
            atomic_write(path.with_name(path.name + ".claude-wiki.bak"), original, mode)
        atomic_write(path, text, mode)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--codex", required=True)
    parser.add_argument("--wiki", required=True)
    parser.add_argument("--rules", type=Path, required=True)
    parser.add_argument("homes", type=Path, nargs="+")
    args = parser.parse_args()
    homes = [home.absolute() for home in args.homes if home.is_dir()]
    if not homes:
        return
    desired = definitions(args.wiki)
    hashes = trust_hashes(args.codex, desired)
    for home in homes:
        install(home, desired, hashes, args.rules.read_text())


if __name__ == "__main__":
    main()
