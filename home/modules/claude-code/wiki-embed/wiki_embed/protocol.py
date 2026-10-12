import json
import math
import re

MAX_LINE = 1024 * 1024


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
        page_id = exclude.removeprefix("pages/").removesuffix(".md") if isinstance(exclude, str) else ""
        if not re.fullmatch(r"(?:knowledge|followups|projects|history)/[a-z0-9._-]+", page_id):
            raise ValueError("exclude must be a page ID or relative path")
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
