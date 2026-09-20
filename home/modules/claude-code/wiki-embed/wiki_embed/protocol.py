import json

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
    if op not in ("related", "search"):
        raise ValueError("unknown operation")
    key = "prompt" if op == "related" else "query"
    if not isinstance(value.get(key), str) or not value[key].strip():
        raise ValueError(f"{key} must be a nonempty string")
    n = value.get("n", 3 if op == "related" else 10)
    if type(n) is not int or not 1 <= n <= 1000:
        raise ValueError("n must be an integer between 1 and 1000")
    return {"op": op, key: value[key], "n": n}
