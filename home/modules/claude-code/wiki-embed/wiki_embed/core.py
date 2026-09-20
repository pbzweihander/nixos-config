from contextlib import closing
from dataclasses import asdict, dataclass
from pathlib import Path, PurePosixPath
import json
import os
import sqlite3
import tempfile
import time

import numpy as np

# Both tiers run the same encoder so there is one vector file and nothing to
# re-embed when the GPU is handed back to a game; only where the query is encoded
# and whether the reranker runs change. A smaller CPU-only model was measured and
# is worse where it matters: thresholded on cosine with no reranker it surfaces
# 10 of 54 against bge-m3's 25, and bge-m3's 56 ms CPU query still fits the hook.
CPU_MODEL = "BAAI/bge-m3"
GPU_MODEL = "BAAI/bge-m3"
RERANKER = "BAAI/bge-reranker-v2-m3"
CACHE_VERSION = 1


@dataclass(frozen=True)
class Section:
    heading: str
    start_line: int
    end_line: int


@dataclass(frozen=True)
class Chunk:
    text: str
    first_line: int


@dataclass(frozen=True)
class Page:
    path: str
    mtime_ns: int
    size: int
    sections: tuple[Section, ...]

    @property
    def page_id(self):
        return self.path[len("pages/"):-len(".md")]


def section_for_line(sections, line):
    # A nested heading can overlap its parent in an index from an older CLI.
    return next((s for s in sorted(sections, key=lambda s: s.start_line, reverse=True)
                 if s.start_line <= line <= s.end_line), None)


def chunk_page(text, page_id):
    lines = text.splitlines()
    body_start = 0
    title, tags = page_id, ""
    if lines and lines[0].strip() == "---":
        end = next((i for i in range(1, len(lines))
                    if lines[i].strip() in ("---", "...")), None)
        if end is not None:
            body_start = end + 1
            # Match the benchmark's textual title/tag prefix, including punctuation.
            # Other YAML fields never enter the model input.
            for line in lines[1:end]:
                if line.startswith("title:"):
                    title = line[6:].strip() or page_id
                elif line.startswith("tags:"):
                    tags = line[5:].strip().strip("[]")
    words = [(word, number) for number, line in enumerate(lines[body_start:], body_start + 1)
             for word in line.split()]
    prefix = f"{title}. {tags}. " if tags else f"{title}. "
    if not words:
        return (Chunk(prefix.strip(), body_start + 1),)
    result = []
    for start in range(0, len(words), 140):
        result.append(Chunk(prefix + " ".join(word for word, _ in words[start:start + 180]),
                            words[start][1]))
        if start + 180 >= len(words):
            break
    return tuple(result)


def read_catalog(root):
    # Never create or migrate the Rust CLI's database, even on a fresh login.
    with closing(sqlite3.connect((root / "index.sqlite").as_uri() + "?mode=ro", uri=True)) as db:
        db.execute("PRAGMA query_only = ON")
        db.execute("BEGIN")
        sections = {}
        for file_id, heading, start, end in db.execute(
                "SELECT file_id, heading, start_line, end_line FROM sections ORDER BY start_line"):
            sections.setdefault(file_id, []).append(Section(heading, start, end))
        pages = []
        for file_id, path, mtime, size in db.execute(
                "SELECT id, path, mtime_ns, size FROM files WHERE coalesce(blocked, '') = '' ORDER BY path"):
            parts = PurePosixPath(path).parts
            if (not path.startswith("pages/") or not path.endswith(".md")
                    or ".." in parts or PurePosixPath(path).is_absolute()):
                continue
            pages.append(Page(path, mtime, size, tuple(sections.get(file_id, ()))))
        return tuple(pages)


@dataclass(frozen=True)
class Entry:
    page: Page
    chunks: tuple[Chunk, ...]
    vectors: np.ndarray


class Snapshot:
    def __init__(self, entries=(), indexed_at=0.0):
        self.entries = tuple(entries)
        self.indexed_at = indexed_at
        self.rows = tuple((entry.page, chunk) for entry in self.entries for chunk in entry.chunks)
        self.vectors = (np.concatenate([entry.vectors for entry in self.entries])
                        if self.rows else np.empty((0, 0), dtype=np.float32))

    def retain(self, pages):
        allowed = {p.path: p for p in pages}
        # Drop blocked, deleted and changed pages before a potentially slow rebuild.
        return Snapshot((Entry(allowed[e.page.path], e.chunks, e.vectors) for e in self.entries
                         if e.page.path in allowed and
                         e.page.mtime_ns == allowed[e.page.path].mtime_ns), self.indexed_at)


def cache_path(root, model):
    return root / ("vectors-" + model.replace("/", "--") + ".npz")


def save_snapshot(path, model, snapshot):
    metadata = {"version": CACHE_VERSION, "model": model, "indexed_at": snapshot.indexed_at,
                "entries": [{"page": asdict(e.page), "chunks": [asdict(c) for c in e.chunks]}
                            for e in snapshot.entries]}
    path.parent.mkdir(parents=True, exist_ok=True)
    fd, temporary = tempfile.mkstemp(prefix=path.name, suffix=".tmp", dir=path.parent)
    try:
        with os.fdopen(fd, "wb") as stream:
            np.savez_compressed(stream, metadata=json.dumps(metadata), vectors=snapshot.vectors)
            stream.flush()
            os.fsync(stream.fileno())
        os.replace(temporary, path)
    finally:
        Path(temporary).unlink(missing_ok=True)


def load_snapshot(path, model):
    with np.load(path, allow_pickle=False) as cache:
        metadata = json.loads(str(cache["metadata"]))
        vectors = np.asarray(cache["vectors"], dtype=np.float32)
    if metadata["version"] != CACHE_VERSION or metadata["model"] != model:
        raise ValueError("incompatible vector cache")
    if vectors.ndim != 2 or not np.isfinite(vectors).all():
        raise ValueError("invalid cached vectors")
    entries, offset = [], 0
    for item in metadata["entries"]:
        page = dict(item["page"])
        page["sections"] = tuple(Section(**s) for s in page["sections"])
        chunks = tuple(Chunk(**c) for c in item["chunks"])
        entries.append(Entry(Page(**page), chunks, vectors[offset:offset + len(chunks)]))
        offset += len(chunks)
    if offset != len(vectors):
        raise ValueError("cache row count mismatch")
    return Snapshot(entries, metadata["indexed_at"])


def refresh_snapshot(root, pages, previous, encode, cancelled=lambda: False, progress=lambda snapshot: None):
    old = {(e.page.path, e.page.mtime_ns): e for e in previous.entries}
    entries = {p.path: Entry(p, old[(p.path, p.mtime_ns)].chunks, old[(p.path, p.mtime_ns)].vectors)
               for p in pages if (p.path, p.mtime_ns) in old}

    def snapshot():
        return Snapshot((entries[p.path] for p in pages if p.path in entries), time.time())

    for page in pages:
        if cancelled():
            raise InterruptedError("refresh cancelled")
        cached = old.get((page.path, page.mtime_ns))
        if cached is not None:
            continue
        path = root / page.path
        try:
            # The CLI may not have scanned a concurrent edit yet. Do not associate
            # new bytes with its old mtime key, or reuse them on the next refresh.
            if not path.resolve().is_relative_to(root / "pages"):
                continue
            before = path.stat()
            if (before.st_mtime_ns, before.st_size) != (page.mtime_ns, page.size):
                continue
            chunks = chunk_page(path.read_text(encoding="utf-8"), page.page_id)
            after = path.stat()
            if (after.st_mtime_ns, after.st_size) != (page.mtime_ns, page.size):
                continue
        except (OSError, UnicodeError):
            continue
        # Small batches bound the GPU work already queued when a query arrives.
        vectors = np.concatenate([encode([c.text for c in chunks[i:i + 8]])
                                  for i in range(0, len(chunks), 8)])
        entries[page.path] = Entry(page, chunks, vectors)
        # Only complete pages enter the mtime cache; a tier handoff can safely
        # reuse them while finishing any interrupted page on the other device.
        progress(snapshot())
    return snapshot()


@dataclass(frozen=True)
class Thresholds:
    # Both cutoffs are the lowest score that rejects every near miss in the
    # benchmark's calibration half, measured on a 258-page wiki. They move as the
    # wiki grows while the usefulness they buy does not, so re-derive them rather
    # than trusting the constants.
    rerank: float = 0.274
    cosine: float = 0.654

    @classmethod
    def from_env(cls):
        values = [float(os.environ.get(name, default)) for name, default in (
            ("WIKI_EMBED_RERANK_THRESHOLD", "0.274"),
            ("WIKI_EMBED_COSINE_THRESHOLD", "0.654"))]
        if not all(np.isfinite(v) for v in values):
            raise ValueError("thresholds must be finite")
        return cls(*values)


def retrieve(snapshot, model, tier, query, op, n, thresholds):
    if not snapshot.rows:
        return []
    similarities = snapshot.vectors @ model.encode_query(query)
    best = {}
    for index, (page, _) in enumerate(snapshot.rows):
        if page.path not in best or similarities[index] > similarities[best[page.path]]:
            best[page.path] = index
    order = sorted(best.values(), key=lambda i: float(similarities[i]), reverse=True)
    if tier == "gpu":
        order = order[:10]
        scores = model.rerank(query, [snapshot.rows[i][1].text for i in order])
    else:
        scores = [similarities[i] for i in order]
    ranked = sorted(zip(order, map(float, scores)), key=lambda pair: pair[1], reverse=True)
    cutoff = (thresholds.rerank if tier == "gpu" else thresholds.cosine) if op == "related" else -np.inf
    result = []
    for index, score in ranked:
        if not np.isfinite(score) or score < cutoff:
            continue
        page, chunk = snapshot.rows[index]
        item = {"path": page.page_id, "score": score}
        section = section_for_line(page.sections, chunk.first_line)
        if section is not None:
            item.update(asdict(section))
        result.append(item)
        if len(result) >= n:
            break
    return result
