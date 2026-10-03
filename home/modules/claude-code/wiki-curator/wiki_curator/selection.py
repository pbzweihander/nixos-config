"""Deterministic task planning, including the collateral pages of link updates."""

from dataclasses import asdict, dataclass
from datetime import date, datetime, timezone
import math
import re

from .gitutil import git, overlaps, page_text, regular_page, safe_page, status
from .preservation import LINK


TYPES = ("broken-links", "duplicates", "snapshots", "log-growth", "missing-queries", "singleton-tags")
DAY = 86400


@dataclass
class Task:
    type: str
    pages: list
    primary: list
    details: dict

    def data(self):
        return asdict(self)


def scalar(text, key):
    """Read only an unambiguous simple top-level YAML scalar; otherwise skip it."""
    parts = re.split(r"(?m)^---\s*$", text, maxsplit=2)
    if len(parts) != 3 or parts[0].strip():
        return ""
    values = re.findall(r"(?m)^" + re.escape(key) + r":\s*([^\n]*)$", parts[1])
    if len(values) != 1:
        return ""
    return values[0].strip().split(" #", 1)[0].strip().strip("\"'")


def catalog(repo, dirty):
    paths = git(repo, "ls-tree", "-r", "--name-only", "-z", "main", "--", "pages/").split("\0")
    texts = {}
    for path in paths:
        if not safe_page(path, projects=True):
            continue
        if any(overlaps(path, p) for p in dirty):
            # The worktree starts from committed main. Include both committed and
            # unfinished backlinks so neither removal nor addition can hide one.
            texts[path] = page_text(repo, "main", path)
            if regular_page(repo, path):
                texts[path] += "\n" + (repo / path).read_text()
        elif regular_page(repo, path):
            texts[path] = (repo / path).read_text()
    for path in dirty:
        if path not in texts and regular_page(repo, path):
            texts[path] = (repo / path).read_text()
    return texts


def blocked(repo, pages, cooldowns, now, dirty=None):
    dirty = status(repo) if dirty is None else dirty
    for path in pages:
        if not safe_page(path):
            return f"protected or invalid page: {path}"
        if any(overlaps(path, p) for p in dirty):
            return f"dirty page: {path}"
        if cooldowns.get(path, 0) > now:
            return f"page on cooldown: {path}"
        full = repo / path
        if full.exists() or full.is_symlink():
            if not regular_page(repo, path):
                return f"non-regular page: {path}"
            if now - full.stat().st_mtime < DAY:
                return f"recent page: {path}"
    return None


def select_tasks(repo, worklist, cooldowns, now, max_tasks=5, only=None):
    if not isinstance(worklist, dict) or any(not isinstance(v, list) for v in worklist.values()):
        raise ValueError("lint did not return a category-to-list object")
    dirty = status(repo)
    texts = catalog(repo, dirty)
    selected, occupied, seen = [], set(), set()

    def incoming(path):
        target = path[6:-3]
        return sorted(p for p, text in texts.items() if any(m[1] == target for m in LINK.finditer(text)))

    def add(kind, primary, details, extra=()):
        pages = sorted(set(primary) | set(extra))
        if (len(selected) >= max_tasks or any(p not in texts for p in primary)
                or occupied.intersection(pages) or blocked(repo, pages, cooldowns, now, dirty)):
            return
        selected.append(Task(kind, pages, primary, details))
        occupied.update(pages)

    for kind in TYPES:
        if only and kind not in only:
            continue
        items = worklist.get(kind, [])
        if any(not isinstance(i, dict) for i in items):
            raise ValueError(f"invalid lint items in {kind}")
        if kind == "duplicates":
            items = sorted(items, key=lambda i: (-float(i["score"]), i["a"], i["b"]))
        else:
            items = sorted(items, key=lambda i: (i["path"], i.get("tag", ""), i.get("detail", "")))
        grouped = []
        for item in items:
            if kind == "duplicates":
                a, b, score = item["a"], item["b"], float(item["score"])
                key = tuple(sorted((a, b)))
                if key in seen or a == b or not math.isfinite(score) or score < 0.90:
                    continue
                seen.add(key)
                if a not in texts or b not in texts or not safe_page(a) or not safe_page(b):
                    continue
                page_kind = a.split("/")[1]
                if page_kind != b.split("/")[1] or page_kind not in ("knowledge", "followups"):
                    continue
                if page_kind == "followups" and any(scalar(texts[p], "status") != "open" for p in (a, b)):
                    continue
                # Prefer the older creation date, then more backlinks, then the slug.
                def rank(p):
                    try:
                        created = date.fromisoformat(scalar(texts[p], "created")).isoformat()
                    except ValueError:
                        created = "9999-12-31"
                    return created, -len(incoming(p)), p
                keep, remove = sorted((a, b), key=rank)
                add(kind, [keep, remove], {"keep": keep, "remove": remove, "score": score,
                                          "redirects": {remove: keep}}, incoming(remove))
                continue
            path = item["path"]
            if not isinstance(path, str) or path not in texts or not safe_page(path):
                continue
            if kind in ("missing-queries", "singleton-tags"):
                if (kind == "missing-queries" and not path.startswith("pages/knowledge/")):
                    continue
                if path in occupied or blocked(repo, [path], cooldowns, now, dirty):
                    continue
                if kind == "missing-queries" and any(i["path"] == path for i in grouped):
                    continue
                if len(grouped) < (5 if kind == "missing-queries" else 20):
                    grouped.append(item)
                continue
            if (kind, path) in seen:
                continue
            seen.add((kind, path))
            if kind == "snapshots":
                if not path.startswith("pages/knowledge/"):
                    continue
                slug = path.rsplit("/", 1)[1][:-3]
                match = re.search(r"\d{4}-\d{2}-\d{2}", slug)
                if not match:
                    continue
                try:
                    date.fromisoformat(match[0])
                except ValueError:
                    continue
                rest = "-".join(part.strip("-_.") for part in (slug[:match.start()], slug[match.end():])
                                if part.strip("-_."))
                destination = f"pages/history/{match[0]}-{rest}.md"
                if not rest or (repo / destination).exists():
                    continue
                add(kind, [path], {"destination": destination, "redirects": {path: destination}},
                    [destination, *incoming(path)])
            elif kind == "log-growth":
                if path.split("/")[1] not in ("knowledge", "followups"):
                    continue
                today = datetime.fromtimestamp(now, timezone.utc).date().isoformat()
                slug = path.rsplit("/", 1)[1][:-3]
                destination = f"pages/history/{today}-{slug}-curation.md"
                if (repo / destination).exists():
                    continue
                add(kind, [path], {"destination": destination}, [destination])
            else:
                add(kind, [path], {"findings": [i["detail"] for i in items if i["path"] == path]})
        if grouped:
            add(kind, sorted({i["path"] for i in grouped}), {"items": grouped})
    return selected
