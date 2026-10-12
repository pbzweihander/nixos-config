"""Strict adjudicator output and bounded edits to an already reviewed commit."""

from .gitutil import changes, git, regular_page, safe_page
from .preservation import body, strict_json
from .recall import string_list


def frontmatter_only(repo, parent, commit):
    for flag, old, new in changes(repo, parent, commit):
        if flag != "M" or old != new or not safe_page(old):
            return False
        if body(git(repo, "show", f"{parent}:{old}", text=False)) != body(
                git(repo, "show", f"{commit}:{new}", text=False)):
            return False
    return True


def singleton_tags(repo, parent, paths):
    """Require distinct one-for-one replacements from other parent pages."""
    catalog = {}
    for path in git(repo, "ls-tree", "-r", "--name-only", "-z", parent, "--", "pages/").split("\0"):
        if safe_page(path, projects=True):
            catalog[path] = string_list(git(repo, "show", f"{parent}:{path}"), "tags")
    pages, problems = [], []
    for path in paths:
        old_tags = catalog.get(path, [])
        new_tags = string_list((repo / path).read_text(), "tags") if regular_page(repo, path) else []
        old, new = set(old_tags), set(new_tags)
        dropped, added = sorted(old - new), sorted(new - old)
        existing = set().union(*(tags for other, tags in catalog.items() if other != path))
        unchanged = (path in catalog and regular_page(repo, path)
                     and body(git(repo, "show", f"{parent}:{path}", text=False))
                     == body((repo / path).read_bytes()))
        issues = []
        if not unchanged:
            issues.append("page body changed")
        if len(new_tags) < len(old_tags) or len(new) < len(old):
            issues.append("tag count decreased")
        if len(set(added) & existing) < len(dropped):
            issues.append("dropped tags lack one-for-one replacements from other parent pages")
        if set(added) - existing:
            issues.append("unknown replacement tags: " + ", ".join(sorted(set(added) - existing)))
        pages.append({"path": path, "dropped_tags": dropped, "added_tags": added,
                      "body_unchanged": unchanged, "problems": issues})
        if issues:
            problems.append(f"{path}: {'; '.join(issues)}; dropped tags: {', '.join(dropped) or '(none)'}")
    return {"accept": not problems, "pages": pages, "problems": problems}


def parse_adjudication(text):
    lines = text.strip().splitlines()
    if not lines:
        raise ValueError("Claude returned no adjudication JSON")
    value = strict_json(lines[-1])
    if (not isinstance(value, dict) or set(value) != {"verdict", "reason", "edits"}
            or value["verdict"] not in ("override", "revise", "uphold")
            or not isinstance(value["reason"], str) or not value["reason"].strip()
            or not isinstance(value["edits"], list) or len(value["edits"]) > 10):
        raise ValueError("invalid adjudication schema")
    for edit in value["edits"]:
        if (not isinstance(edit, dict) or set(edit) != {"path", "old", "new"}
                or any(not isinstance(v, str) for v in edit.values())
                or not safe_page(edit["path"]) or not edit["old"] or edit["old"] == edit["new"]):
            raise ValueError("invalid adjudication edit")
    if bool(value["edits"]) != (value["verdict"] == "revise"):
        raise ValueError("only revise requires edits")
    return value


def apply_edits(repo, parent, commit, edits):
    allowed = {new for _, _, new in changes(repo, parent, commit) if new}
    texts, replacements = {}, {}
    # Validate against the immutable candidate, before writing any page. Matching
    # offsets also prevent edits from creating matches for later edits.
    for edit in edits:
        path = edit["path"]
        if path not in allowed or not regular_page(repo, path):
            raise ValueError(f"edit outside changed pages: {path}")
        if path not in texts:
            texts[path] = git(repo, "show", f"{commit}:{path}", text=False).decode("utf-8")
        text = texts[path]
        old = edit["old"]
        start = text.find(old)
        if start < 0 or text.find(old, start + 1) >= 0:
            raise ValueError(f"edit old must occur exactly once: {path}")
        end = start + len(old)
        spans = replacements.setdefault(path, [])
        if any(start < other_end and other_start < end for other_start, other_end, _ in spans):
            raise ValueError(f"overlapping edits: {path}")
        spans.append((start, end, edit["new"]))
    for path, spans in replacements.items():
        text = texts[path]
        for start, end, replacement in sorted(spans, reverse=True):
            text = text[:start] + replacement + text[end:]
        (repo / path).write_bytes(text.encode("utf-8"))
