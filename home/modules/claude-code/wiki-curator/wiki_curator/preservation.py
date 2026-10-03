"""Body-token preservation. Semantic equivalence is the separate reviewer's job."""

import json
import re

from .gitutil import changed_texts, changes, safe_page


LINK = re.compile(r"\[\[([a-z]+/[a-z0-9._-]+)(?:\|[^\]\n]*)?\]\]")
INLINE = re.compile(r"(?<!`)(`+)(?!`)([^\n]*?)(?<!`)\1(?!`)")
NUMBER = re.compile(
    r"(?<![\w])(?:[vV])?[+-]?\d+(?:[.,]\d+)*(?:\.[xX*])?"
    r"(?:[-+][a-zA-Z]+(?:[.-]?\d+)*)?(?:[a-zA-Zµμ°]+|%)?(?![\w])"
)
URL = re.compile(r"(?:https?|ftp)://[^\s<>`\]\"']+")


def strict_json(text):
    def unique_object(pairs):
        result = {}
        for key, value in pairs:
            if key in result:
                raise ValueError(f"duplicate JSON key: {key}")
            result[key] = value
        return result

    def invalid_constant(value):
        raise ValueError(f"invalid JSON constant: {value}")

    return json.loads(text, object_pairs_hook=unique_object, parse_constant=invalid_constant)


def body(text):
    lines = text.splitlines(keepends=True)
    if lines and lines[0].strip() == "---":
        for index, line in enumerate(lines[1:], 1):
            if line.strip() in ("---", "..."):
                return "".join(lines[index + 1:])
    # An unterminated header cannot hide facts from the gate.
    return text


def tokens(text):
    text = body(text)
    result = set(NUMBER.findall(text))
    result.update(m.group().rstrip(".,;:!?)") for m in URL.finditer(text))
    result.update(f"[[{m[1]}]]" for m in LINK.finditer(text))
    # Fences themselves are not inline spans; do not remove their contents.
    for line in text.splitlines():
        if re.match(r"^\s*(`{3,}|~{3,})", line):
            continue
        result.update("`" + m[2] + "`" for m in INLINE.finditer(line))
    return result


def validate_justification(value):
    if not isinstance(value, dict) or set(value) != {"summary", "dropped", "redirects"}:
        raise ValueError("justification must contain summary, dropped, redirects")
    if (not isinstance(value["summary"], str) or not value["summary"].strip()
            or len(value["summary"]) > 500 or "\n" in value["summary"] or "\r" in value["summary"]):
        raise ValueError("summary must be a nonempty, single line (at most 500 characters)")
    if not isinstance(value["dropped"], list) or not isinstance(value["redirects"], dict):
        raise ValueError("invalid dropped or redirects")
    for entry in value["dropped"]:
        if (not isinstance(entry, dict) or set(entry) != {"token", "reason"}
                or any(not isinstance(v, str) or not v.strip() for v in entry.values())):
            raise ValueError("each dropped token needs a nonempty token and reason")
    for old, new in value["redirects"].items():
        if not safe_page(old) or not isinstance(new, str) or not safe_page(new) or old == new:
            raise ValueError("redirects must map distinct, non-project page paths")
    return value


def load_justification(path):
    if path.is_symlink() or not path.is_file():
        raise ValueError("justification must be a regular file")
    return validate_justification(strict_json(path.read_text()))


def preserved(repo, parent, commit, justification):
    """Return sorted missing tokens; raises on malformed/cyclic redirects."""
    validate_justification(justification)
    redirects = {
        old[6:-3]: new[6:-3]
        for flag, old, new in changes(repo, parent, commit)
        if flag.startswith("R") and safe_page(old) and safe_page(new)
    }
    for old, new in justification["redirects"].items():
        old, new = old[6:-3], new[6:-3]
        if old in redirects and redirects[old] != new:
            raise ValueError("justification contradicts git rename detection")
        redirects[old] = new

    def normalize(token):
        if not LINK.fullmatch(token):
            return token
        target, seen = LINK.fullmatch(token)[1], set()
        while target in redirects:
            if target in seen:
                raise ValueError("cyclic redirects")
            seen.add(target)
            target = redirects[target]
        return f"[[{target}]]"

    for target in redirects:
        normalize(f"[[{target}]]")
    texts = changed_texts(repo, parent, commit)
    old = set().union(*(tokens(p["old"]) for p in texts))
    new = {normalize(t) for p in texts for t in tokens(p["new"])}
    dropped = {normalize(e["token"]) for e in justification["dropped"]}
    return sorted(t for t in old if normalize(t) not in new | dropped)
