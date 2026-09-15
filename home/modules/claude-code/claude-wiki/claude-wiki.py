#!/usr/bin/env python3
"""Shared markdown wiki for Claude Code sessions, searched with SQLite FTS5 (BM25).

Pages under <root>/pages/<type>/ are the source of truth. index.sqlite is derived
from them and refreshed from file mtimes on every command, so edits made with any
tool are picked up. `sync` also commits the pages to the wiki's git repo.
"""

import argparse
import contextlib
import fcntl
import os
import re
import sqlite3
import subprocess
import sys
from collections import Counter
from datetime import datetime
from pathlib import Path

import yaml

TYPES = ("knowledge", "projects", "followups", "history")
STATUSES = ("open", "done")
SCHEMA_VERSION = 2
# bm25() weights in fts column order: path, type, created, status, title, tags, project, body
WEIGHTS = (0.0, 0.0, 0.0, 0.0, 10.0, 5.0, 3.0, 1.0)
# lines of the current project's overview page printed at session start
OVERVIEW_LINES = 60
FRONTMATTER = re.compile(r"\A---\n(.*?)\n---\n?", re.S)
YAML_LOADER = getattr(yaml, "CSafeLoader", yaml.SafeLoader)
VERBS = {"A": "add", "M": "update", "D": "delete", "R": "rename", "C": "copy"}
COLUMNS = "fts.path, fts.type, fts.status, fts.title, fts.project, fts.tags, files.mtime_ns"


def wiki_root():
    if os.environ.get("CLAUDE_WIKI_DIR"):
        return Path(os.environ["CLAUDE_WIKI_DIR"]).expanduser()
    data = os.environ.get("XDG_DATA_HOME") or Path.home() / ".local" / "share"
    return Path(data) / "claude-wiki"


def git(root, *args):
    return subprocess.run(
        ["git", "-C", str(root), *args], check=True, text=True, capture_output=True
    )


@contextlib.contextmanager
def locked(root):
    # Every command may write the index, and sync commits; serialize sessions.
    with open(root / ".lock", "w") as f:
        fcntl.flock(f, fcntl.LOCK_EX)
        yield


def ensure_layout(root):
    for t in TYPES:
        (root / "pages" / t).mkdir(parents=True, exist_ok=True)
    gitignore = root / ".gitignore"
    if not gitignore.exists():
        gitignore.write_text("index.sqlite*\n.lock\n")
    if not (root / ".git").exists():
        git(root, "init", "-q")
        # Commits are made unattended from any session; don't depend on a signing key.
        git(root, "config", "commit.gpgsign", "false")


def page_type(rel):
    parts = Path(rel).parts
    return parts[1] if len(parts) == 3 and parts[0] == "pages" and parts[1] in TYPES else ""


def parse_page(text, rel):
    """Return the page's fields for the index, and problems worth a warning on sync."""
    ptype = page_type(rel)
    meta, body, problems = {}, text, []
    m = FRONTMATTER.match(text)
    if not m:
        problems.append("has no frontmatter")
    else:
        body = text[m.end() :]
        try:
            loaded = yaml.load(m.group(1), Loader=YAML_LOADER)
        except yaml.YAMLError as e:
            problems.append("has invalid YAML frontmatter: " + " ".join(str(e).split()))
        else:
            if isinstance(loaded, dict) or loaded is None:
                meta = loaded or {}
                required = ("title", "created") + (("status",) if ptype == "followups" else ())
                problems += [f"has no {k} in its frontmatter" for k in required if not meta.get(k)]
            else:
                problems.append("has frontmatter that is not a YAML mapping")
    status = str(meta.get("status") or "")
    if status and status not in STATUSES:
        problems.append(f"has status {status!r}; use one of {', '.join(STATUSES)}")
    tags = meta.get("tags") or []
    if isinstance(tags, str):
        tags = tags.split(",")
    elif not isinstance(tags, list):
        tags = [tags]
    # Tags are stored space-separated, so a multi-word tag becomes kebab-case.
    tags = [re.sub(r"\s+", "-", str(t).strip()) for t in tags if str(t).strip()]
    heading = re.search(r"^#\s+(.+)$", body, re.M)
    fields = {
        "type": ptype,
        "status": status,
        "title": str(meta.get("title") or (heading.group(1).strip() if heading else Path(rel).stem)),
        "tags": " ".join(tags),
        # a project overview page is named after its project
        "project": str(meta.get("project") or (Path(rel).stem if ptype == "projects" else "")),
        "created": str(meta.get("created") or ""),
        "body": body,
    }
    return fields, problems


def open_index(root):
    db = sqlite3.connect(root / "index.sqlite", timeout=30)
    if db.execute("pragma user_version").fetchone()[0] != SCHEMA_VERSION:
        db.executescript(
            f"""
            drop table if exists files;
            drop table if exists fts;
            create table files(id integer primary key, path text unique, mtime_ns integer, size integer);
            create virtual table fts using fts5(
                path unindexed, type unindexed, created unindexed, status unindexed,
                title, tags, project, body,
                tokenize = 'porter unicode61 remove_diacritics 2');
            pragma user_version = {SCHEMA_VERSION};
            """
        )
    return db


def refresh(root, db):
    on_disk = {}
    for path in (root / "pages").rglob("*.md"):
        st = path.stat()
        on_disk[path.relative_to(root).as_posix()] = (st.st_mtime_ns, st.st_size)
    indexed = {row[1]: row for row in db.execute("select id, path, mtime_ns, size from files")}
    with db:
        for rel, (rowid, _, _, _) in indexed.items():
            if rel not in on_disk:
                db.execute("delete from fts where rowid = ?", (rowid,))
                db.execute("delete from files where id = ?", (rowid,))
        for rel, stamp in on_disk.items():
            old = indexed.get(rel)
            if old and (old[2], old[3]) == stamp:
                continue
            page, _ = parse_page((root / rel).read_text(errors="replace"), rel)
            if old:
                rowid = old[0]
                db.execute("delete from fts where rowid = ?", (rowid,))
                db.execute("update files set mtime_ns = ?, size = ? where id = ?", (*stamp, rowid))
            else:
                rowid = db.execute(
                    "insert into files(path, mtime_ns, size) values (?, ?, ?)", (rel, *stamp)
                ).lastrowid
            db.execute(
                "insert into fts(rowid, path, type, created, status, title, tags, project, body)"
                " values (?, ?, ?, ?, ?, ?, ?, ?, ?)",
                (
                    rowid,
                    rel,
                    page["type"],
                    page["created"],
                    page["status"],
                    page["title"],
                    page["tags"],
                    page["project"],
                    page["body"],
                ),
            )


def filters(args):
    clauses, params = [], []
    for column in ("type", "status", "project"):
        if getattr(args, column):
            clauses.append(f"fts.{column} = ?")
            params.append(getattr(args, column))
    if args.tag:
        clauses.append("(' ' || fts.tags || ' ') like ?")
        params.append(f"% {args.tag} %")
    return "".join(" and " + c for c in clauses), params


def fts_query(terms):
    """OR of prefix-matched words. An argument of several words (quoted in the shell)
    is also matched as an exact phrase, so pages containing it rank higher."""
    parts = []
    for term in terms:
        words = re.findall(r"\w+", term)
        if len(words) > 1:
            parts.append('"' + " ".join(words) + '"')
        parts += [f'"{w}"*' for w in words]
    return " OR ".join(dict.fromkeys(parts))


def updated(mtime_ns):
    return datetime.fromtimestamp(mtime_ns / 1e9).strftime("%Y-%m-%d")


def show(root, row, snippet=None):
    rel, ptype, status, title, project, tags, mtime_ns = row
    meta = [ptype or "?"] + ([status] if status else []) + ["updated " + updated(mtime_ns)]
    if project:
        meta.append("project=" + project)
    if tags:
        meta.append("tags=" + ",".join(tags.split()))
    print(root / rel)
    print(f"  {title}  ({'; '.join(meta)})")
    if snippet:
        print("  " + " ".join(snippet.split()))


def cmd_search(root, db, args):
    query = fts_query(args.terms)
    if not query:
        sys.exit("claude-wiki: no search terms")
    where, params = filters(args)
    rows = db.execute(
        f"select {COLUMNS}, snippet(fts, -1, '[', ']', '…', 16)"
        " from fts join files on files.id = fts.rowid"
        f" where fts match ?{where}"
        f" order by bm25(fts, {', '.join(map(str, WEIGHTS))}) limit ?",
        (query, *params, args.n),
    ).fetchall()
    if not rows:
        total = db.execute("select count(*) from files").fetchone()[0]
        print(f"no pages match {query} ({total} pages in the wiki)")
    for row in rows:
        show(root, row[:-1], row[-1])


def cmd_list(root, db, args):
    where, params = filters(args)
    rows = db.execute(
        f"select {COLUMNS} from fts join files on files.id = fts.rowid"
        f" where 1{where} order by files.mtime_ns desc limit ?",
        (*params, args.n),
    ).fetchall()
    if not rows:
        print("no pages")
    for row in rows:
        show(root, row)


def cmd_tags(root, db, args):
    tags, projects = Counter(), Counter()
    for tag_list, project in db.execute("select tags, project from fts"):
        tags.update(tag_list.split())
        if project:
            projects[project] += 1
    for label, counts in (("tags", tags), ("projects", projects)):
        print(f"{label}: " + (", ".join(f"{k} ({n})" for k, n in counts.most_common()) or "none"))


def cmd_sync(root, db, args):
    git(root, "add", "-A")
    changes = [line.split("\t") for line in git(root, "diff", "--cached", "--name-status").stdout.splitlines()]
    if not changes:
        print("nothing to commit")
        return
    for status, *paths in changes:
        rel = paths[-1]
        if status.startswith("D") or not rel.endswith(".md"):
            continue
        if not page_type(rel):
            print(f"warning: {rel} is not directly under pages/<{'|'.join(TYPES)}>/", file=sys.stderr)
        for problem in parse_page((root / rel).read_text(errors="replace"), rel)[1]:
            print(f"warning: {rel} {problem}", file=sys.stderr)
    summary = [f"{VERBS.get(s[0], s)} {p[-1].removeprefix('pages/').removesuffix('.md')}" for s, *p in changes]
    message = args.message or "wiki: " + ", ".join(summary[:5]) + (
        f" (+{len(summary) - 5} more)" if len(summary) > 5 else ""
    )
    git(root, "commit", "-q", "-m", message, "-m", "\n".join(summary))
    print(f"committed: {message}")


def current_project():
    """Name of the repository the session runs in; a worktree resolves to its main checkout."""
    try:
        common = subprocess.run(
            ["git", "rev-parse", "--path-format=absolute", "--git-common-dir"],
            check=True,
            text=True,
            capture_output=True,
        ).stdout.strip()
    except (subprocess.CalledProcessError, FileNotFoundError):
        return None
    path = Path(common)
    return path.parent.name if path.name == ".git" else path.name.removesuffix(".git")


def cmd_context(root, db, args):
    """Session start summary, printed by a SessionStart hook into the session context."""
    total = db.execute("select count(*) from files").fetchone()[0]
    print(
        f"Shared wiki: {total} pages under {root}/. Search it with `claude-wiki search <terms>`"
        " before non-trivial work; load the claude-wiki skill before writing to it."
    )
    project = current_project()
    overview = f"pages/projects/{project}.md"
    query = f"select {COLUMNS} from fts join files on files.id = fts.rowid"
    sections = []
    if project:
        print(f"Current project: {project}")
        if (root / overview).exists():
            page, _ = parse_page((root / overview).read_text(errors="replace"), overview)
            lines = page["body"].strip().splitlines()
            print(f"\nProject overview ({overview}):")
            print("\n".join(lines[:OVERVIEW_LINES]))
            if len(lines) > OVERVIEW_LINES:
                print(f"[{len(lines) - OVERVIEW_LINES} more lines in the page]")
        else:
            print(f"No overview page for {project} yet; write {overview} once you have explored the project.")
        sections += [
            (
                f"Open follow-ups for {project}:",
                f"{query} where fts.type = 'followups' and fts.status = 'open' and fts.project = ?"
                " order by files.mtime_ns desc limit ?",
                (project, args.n),
            ),
            (
                f"Knowledge pages for {project}:",
                f"{query} where fts.type = 'knowledge' and fts.project = ? order by files.mtime_ns desc limit ?",
                (project, args.n),
            ),
        ]
    sections.append(
        (
            "Recently updated:",
            f"{query} where not (fts.type = 'followups' and fts.status = 'done')"
            " order by files.mtime_ns desc limit ?",
            (3 * args.n,),
        )
    )
    seen = {overview}
    for heading, sql, params in sections:
        rows = [r for r in db.execute(sql, params) if r[0] not in seen][: args.n]
        if not rows:
            continue
        print("\n" + heading)
        for rel, ptype, status, title, project_, _, mtime_ns in rows:
            seen.add(rel)
            extra = "".join(f", {x}" for x in (status, project_ if project_ != project else "") if x)
            print(f"- {rel}: {title} ({ptype}, {updated(mtime_ns)}{extra})")
    tags = Counter(t for (tag_list,) in db.execute("select tags from fts") for t in tag_list.split())
    if tags:
        print("\nTags: " + ", ".join(f"{k} ({n})" for k, n in tags.most_common(30)))


COMMANDS = {
    "search": cmd_search,
    "list": cmd_list,
    "tags": cmd_tags,
    "sync": cmd_sync,
    "context": cmd_context,
}


def main():
    parser = argparse.ArgumentParser(prog="claude-wiki", description=__doc__.splitlines()[0])
    sub = parser.add_subparsers(dest="cmd", required=True)
    search = sub.add_parser(
        "search", help="BM25 search; words are stemmed, prefix-matched and OR-ed, quoted phrases rank higher"
    )
    search.add_argument("terms", nargs="+")
    listing = sub.add_parser("list", help="most recently updated pages")
    for p, n in ((search, 10), (listing, 20)):
        p.add_argument("-n", type=int, default=n, help=f"max results (default {n})")
        p.add_argument("--type", choices=TYPES)
        p.add_argument("--status", choices=STATUSES)
        p.add_argument("--project")
        p.add_argument("--tag")
    sub.add_parser("tags", help="tags and projects in use, with page counts")
    context = sub.add_parser("context", help="session start summary (used by the SessionStart hook)")
    context.add_argument("-n", type=int, default=10, help="max pages per section (default 10)")
    sync = sub.add_parser("sync", help="index pages and commit changes to the wiki's git repo")
    sync.add_argument("-m", "--message", help="commit message (default: list of changed pages)")
    sync.add_argument("--rebuild", action="store_true", help="rebuild the index from scratch")
    args = parser.parse_args()

    root = wiki_root()
    root.mkdir(parents=True, exist_ok=True)
    try:
        with locked(root):
            ensure_layout(root)
            if args.cmd == "sync" and args.rebuild:
                for f in root.glob("index.sqlite*"):
                    f.unlink()
            db = open_index(root)
            refresh(root, db)
            COMMANDS[args.cmd](root, db, args)
    except subprocess.CalledProcessError as e:
        sys.exit(f"claude-wiki: {' '.join(e.cmd)} failed:\n{e.stderr.strip()}")


if __name__ == "__main__":
    main()
