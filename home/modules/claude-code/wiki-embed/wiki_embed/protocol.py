import json
import math
import re

MAX_LINE = 1024 * 1024


def page_id(value):
    result = value.removeprefix("pages/").removesuffix(".md") if isinstance(value, str) else ""
    if not re.fullmatch(r"(?:knowledge|followups|projects|history)/[a-z0-9._-]+", result):
        raise ValueError("must be a page ID or relative path")
    return result


def encode_message(message):
    return (json.dumps(message, ensure_ascii=False, allow_nan=False, separators=(",", ":")) + "\n").encode("utf-8")


def decode_request(line):
    if len(line) > MAX_LINE:
        raise ValueError("request exceeds 1 MiB")
    if not line.endswith(b"\n"):
        raise ValueError("request must end with a newline")
    try:
        value = json.loads(line.decode("utf-8"))
    except (UnicodeError, ValueError) as error:
        raise ValueError("invalid JSON request") from error
    if not isinstance(value, dict):
        raise ValueError("request must be an object")
    op = value.get("op")
    if op == "status":
        return {"op": op}
    if op == "probe":
        queries, n, states = value.get("queries"), value.get("n"), value.get("states")
        if (not isinstance(queries, list) or not 1 <= len(queries) <= 200
                or any(not isinstance(q, str) or not q.strip() for q in queries)):
            raise ValueError("queries must contain 1 to 200 nonempty strings")
        if type(n) is not int or not 1 <= n <= 100:
            raise ValueError("n must be an integer between 1 and 100")
        if not isinstance(states, dict) or not 1 <= len(states) <= 4:
            raise ValueError("states must contain 1 to 4 entries")
        for name, pages in states.items():
            if not re.fullmatch(r"[a-z][a-z0-9_-]{0,31}", name):
                raise ValueError("invalid state name")
            if not isinstance(pages, dict) or len(pages) > 50:
                raise ValueError("each state must map at most 50 pages")
            seen = set()
            for page, text in pages.items():
                key = page_id(page)
                if key in seen:
                    raise ValueError("duplicate page in state")
                seen.add(key)
                if text is not None and not isinstance(text, str):
                    raise ValueError("page text must be a string or null")
        return {"op": op, "queries": queries, "n": n, "states": states}
    if op in ("similar", "pairs"):
        minimum = value.get("min")
        if type(minimum) not in (int, float) or not math.isfinite(minimum) or not -1 <= minimum <= 1:
            raise ValueError("min must be a finite cosine between -1 and 1")
        if op == "pairs":
            types = value.get("types")
            if (not isinstance(types, list) or not types
                    or any(t not in ("knowledge", "followups", "projects", "history") for t in types)):
                raise ValueError("types must be a nonempty list of page types")
            return {"op": op, "min": minimum, "types": types}
        exclude = value.get("exclude")
        try:
            page_id(exclude)
        except ValueError as error:
            raise ValueError("exclude must be a page ID or relative path") from error
    if op not in ("related", "search", "dense", "similar"):
        raise ValueError("unknown operation")
    key = {"related": "prompt", "search": "query", "dense": "query", "similar": "text"}[op]
    if not isinstance(value.get(key), str) or not value[key].strip():
        raise ValueError(f"{key} must be a nonempty string")
    n = value.get("n") if op == "dense" else value.get("n", 10 if op == "search" else 3)
    maximum = 100 if op == "dense" else 1000
    if type(n) is not int or not 1 <= n <= maximum:
        raise ValueError(f"n must be an integer between 1 and {maximum}")
    result = {"op": op, key: value[key], "n": n}
    if op == "similar":
        result.update(exclude=value["exclude"], min=minimum)
    return result
