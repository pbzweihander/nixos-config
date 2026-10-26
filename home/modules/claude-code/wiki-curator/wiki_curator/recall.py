"""BM25 and dense regression checks against parent and candidate page texts."""

import json
import re
import socket
import textwrap

from .gitutil import changed_texts, changes, command, git, page_text, safe_page
from .preservation import strict_json


def string_list(text, key):
    """Read ordinary YAML flow/block lists without a YAML dependency.

    Support plain, single-quoted and JSON-quoted strings. Unsupported syntax
    fails closed rather than silently losing a query or tag.
    """
    header = re.match(r"\A---\n(.*?)\n---(?:\n|$)", text, re.S)
    if not header:
        raise ValueError("missing or unterminated YAML frontmatter")
    lines = textwrap.dedent(header[1]).splitlines()
    # YAML merges can supply fields indirectly; do not guess at their meaning.
    if any(re.match(r"\s*<<\s*:", line) for line in lines):
        raise ValueError("unsupported YAML merge in frontmatter")
    fields = []
    for i, line in enumerate(lines):
        if not line or line[0].isspace() or line.startswith(("- ", "#")):
            continue
        field = re.match(r'''^([\w-]+|'(?:[^']|'')*'|"(?:[^"\\]|\\.)*")\s*:''', line)
        if not field:
            raise ValueError("unsupported frontmatter mapping syntax")
        field_key = field[1]
        if field_key.startswith('"'):
            field_key = json.loads(field_key)
        elif field_key.startswith("'"):
            field_key = field_key[1:-1].replace("''", "'")
        if field_key == key:
            fields.append(i)
    if not fields:
        if any(re.match(rf"\s*(?:{key}|'{key}'|\"{key}\")\s*:", line) for line in lines):
            raise ValueError(f"unsupported indented {key} field")
        return []
    if len(fields) != 1:
        raise ValueError(f"duplicate {key} field")
    start = fields[0]
    value = lines[start].split(":", 1)[1].strip()
    tail = []
    for line in lines[start + 1:]:
        if line and not line[0].isspace() and not line.startswith(("- ", "#")):
            break
        tail.append(line)

    def scalar(value, flow=False):
        value = value.lstrip()
        if value.startswith('"'):
            # JSON quoting is a YAML subset, including Unicode and escaped quotes.
            result, end = json.JSONDecoder().raw_decode(value)
            rest = value[end:]
        elif value.startswith("'"):
            match = re.match(r"'((?:[^'\n]|'')*)'", value)
            if not match:
                raise ValueError(f"unsupported quoted {key} item")
            result, rest = match[1].replace("''", "'"), value[match.end():]
        else:
            if not value or value[0] in "!&*{}[]|>@`\"'":
                raise ValueError(f"unsupported {key} item scalar")
            match = re.match(r"[^,\]\n]+" if flow else r"[^\n]+", value)
            if not match:
                raise ValueError(f"invalid {key} item scalar")
            result, rest = match[0], value[match.end():]
            result = re.split(r"\s+#", result, maxsplit=1)[0].strip()
            if (re.search(r":(?:\s|$)", result)
                    or result.lower() in ("true", "false", "null", "~")
                    or re.fullmatch(r"[-+\d.][\d.eExXa-fA-F_+-]*", result)):
                raise ValueError(f"{key} item must be a string; quote ambiguous scalars")
        if not isinstance(result, str) or not result.strip():
            raise ValueError(f"{key} item must be a nonempty string")
        return result, rest.lstrip()

    result = []
    if value.startswith("["):
        rest = "\n".join([value, *tail])[1:].lstrip()
        while not rest.startswith("]"):
            query, rest = scalar(rest, flow=True)
            result.append(query)
            if rest.startswith(","):
                rest = rest[1:].lstrip()
            elif not rest.startswith("]"):
                raise ValueError(f"unsupported {key} flow list")
        if rest[1:].strip() and not rest[1:].lstrip().startswith("#"):
            raise ValueError(f"unexpected text after {key}")
    else:
        if value and not value.startswith("#"):
            raise ValueError(f"{key} must be a list of strings")
        for line in tail:
            line = line.strip()
            if not line or line.startswith("#"):
                continue
            if not line.startswith("- "):
                raise ValueError(f"unsupported {key} block list")
            query, rest = scalar(line[2:])
            if rest and not rest.startswith("#"):
                raise ValueError(f"unexpected text after {key} item")
            result.append(query)
        if not result:
            raise ValueError(f"{key} must be a list of strings (use [] for empty)")
    return result


def queries(text):
    return string_list(text, "queries")


def dense_ranks(socket_path, queries, states):
    if socket_path is None:
        raise ValueError("no socket configured")
    limit = 1024 * 1024
    request = (json.dumps({"op": "probe", "queries": queries, "n": 10, "states": states},
                          ensure_ascii=False, allow_nan=False) + "\n").encode("utf-8")
    if len(request) > limit:
        raise ValueError("request exceeds 1 MiB")
    with socket.socket(socket.AF_UNIX, socket.SOCK_STREAM) as connection:
        connection.settimeout(120)
        connection.connect(str(socket_path))
        connection.sendall(request)
        with connection.makefile("rb") as stream:
            line = stream.readline(limit + 1)
    if len(line) > limit or not line.endswith(b"\n"):
        raise ValueError("invalid daemon response line")
    response = strict_json(line.decode("utf-8"))
    if not isinstance(response, dict) or type(response.get("ok")) is not bool:
        raise ValueError("invalid daemon response")
    if not response["ok"]:
        error = response.get("error")
        raise ValueError(error if isinstance(error, str) and error else "invalid daemon error")
    results = response.get("results")
    if (response.get("tier") not in ("cpu", "gpu") or not isinstance(results, dict)
            or set(results) != set(states)):
        raise ValueError("invalid daemon results")
    for ranks in results.values():
        if not isinstance(ranks, list) or len(ranks) != len(queries):
            raise ValueError("invalid daemon query ranks")
        for hits in ranks:
            if (not isinstance(hits, list) or len(hits) > 10
                    or any(not isinstance(p, str) or not re.fullmatch(
                        r"(?:knowledge|followups|projects|history)/[a-z0-9._-]+", p) for p in hits)
                    or len(set(hits)) != len(hits)):
                raise ValueError("invalid daemon page ranks")
    return results


def recall(worktree, parent, commit, justification, env, socket_path=None):
    """Record 1-based ranks (None means absent from the first ten results)."""
    edits = changes(worktree, parent, commit)
    redirects = {old: new for flag, old, new in edits if flag.startswith("R")}
    for old, new in justification["redirects"].items():
        if old in redirects and redirects[old] != new:
            raise ValueError("justification contradicts git rename detection")
        redirects[old] = new

    def destination(path):
        seen = set()
        while path in redirects:
            if path in seen:
                raise ValueError("cyclic redirects")
            seen.add(path)
            path = redirects[path]
        return path

    current = set(filter(None, git(worktree, "ls-tree", "-r", "--name-only", "-z",
                                  commit, "--", "pages/").split("\0")))
    probes, problems = [], []
    texts = changed_texts(worktree, parent, commit)
    for change in texts:
        old, new = change["old_path"], change["new_path"]
        source = old or new
        if not safe_page(source, projects=True):
            raise ValueError(f"invalid probe page: {source}")
        expected = destination(source)
        expected = expected if expected in current else None
        try:
            source_queries = queries(change["old"] if old else change["new"])
            moved_queries = (queries(page_text(worktree, commit, expected))
                             if old and expected and old != expected else [])
        except ValueError as error:
            raise ValueError(f"{source}: {error}") from error
        for query in source_queries:
            probes.append({"page": source, "expected_page": expected, "query": query,
                           "parent_rank": None, "commit_rank": None,
                           "parent_dense": None, "commit_dense": None})
            if expected is None:
                problems.append(f"{source}: deleted page has queries but no surviving redirect target")
            elif old and old != expected and query not in moved_queries:
                problems.append(f"{source}: query {query!r} missing from {expected}")

    def rank_all(field, path_key):
        cache = {}
        for probe in probes:
            query = probe["query"]
            if query not in cache:
                output = command(["claude-wiki", "search", "--json", "--no-semantic",
                                  "-n", "10", "--", *query.split()], worktree, env=env)
                value = strict_json(output.stdout)
                if (not isinstance(value, dict) or not isinstance(value.get("hits"), list)
                        or any(not isinstance(hit, dict) or not isinstance(hit.get("rel"), str)
                               for hit in value["hits"])):
                    raise ValueError("invalid claude-wiki search JSON")
                cache[query] = [hit["rel"] for hit in value["hits"][:10]]
            hits = cache[query]
            path = probe[path_key]
            probe[field] = hits.index(path) + 1 if path in hits else None

    if probes:
        try:
            git(worktree, "checkout", "--detach", parent)
            rank_all("parent_rank", "page")
        finally:
            git(worktree, "checkout", "--detach", "--force", commit)
        rank_all("commit_rank", "expected_page")
    dense = "ok"
    try:
        if socket_path is None:
            raise ValueError("no socket configured")
        if probes:
            # Remove both ends of renames, including a destination already in the live index.
            paths = {p[key] for p in texts for key in ("old_path", "new_path") if p[key]}
            states = {name: dict.fromkeys(sorted(paths)) for name in ("parent", "commit")}
            for change in texts:
                if change["old_path"]:
                    states["parent"][change["old_path"]] = change["old"]
                if change["new_path"]:
                    states["commit"][change["new_path"]] = change["new"]
            distinct = list(dict.fromkeys(p["query"] for p in probes))
            ranks = dense_ranks(socket_path, distinct, states)
            positions = {query: i for i, query in enumerate(distinct)}
            for probe in probes:
                for state, path_key in (("parent", "page"), ("commit", "expected_page")):
                    hits = ranks[state][positions[probe["query"]]]
                    path = probe[path_key]
                    page_id = path[6:-3] if path else None
                    probe[state + "_dense"] = hits.index(page_id) + 1 if page_id in hits else None
    except (OSError, ValueError, RecursionError) as error:
        dense = f"unavailable: {error}"

    for probe in probes:
        before, after = probe["parent_rank"], probe["commit_rank"]
        before_dense, after_dense = probe["parent_dense"], probe["commit_dense"]
        found_before = any(rank is not None and rank <= 3 for rank in (before, before_dense))
        found_after = any(rank is not None and rank <= 3 for rank in (after, after_dense))
        if found_before and not found_after:
            problems.append(f"{probe['page']}: query {probe['query']!r} fell out of the top 3 "
                            f"(bm25 {before}→{after}, dense {before_dense}→{after_dense})")
    result = {"accept": not problems, "probes": probes, "problems": problems, "dense": dense}
    if dense != "ok":
        result["ungated"] = [p for p in probes if p["parent_rank"] is None]
    return result
