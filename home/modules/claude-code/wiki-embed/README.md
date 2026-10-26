# wiki-embed

Resident semantic retrieval for claude-wiki. The home-manager module installs the
package and enables the user service at login. Models are loaded from
`$HF_HOME` (the service sets it to the user's Hugging Face cache), offline only.

```sh
wiki-embed-client '{"op":"status"}'
wiki-embed-client '{"op":"related","prompt":"한국어 질문","n":3}'
wiki-embed-client '{"op":"search","query":"VRAM pressure","n":10}'
wiki-embed-client '{"op":"similar","text":"---\ntitle: Cache\n---\nPage body","exclude":"knowledge/cache","n":3,"min":0.85}'
wiki-embed-client '{"op":"pairs","min":0.88,"types":["knowledge","followups"]}'
journalctl --user -u wiki-embed
```

The socket is `$XDG_RUNTIME_DIR/claude-wiki/embed.sock`; its directory is mode
0700 and the socket is mode 0600. Each connection accepts one newline-terminated
JSON object and returns one newline-terminated JSON object. Invalid requests
return `{"ok":false,"error":"..."}`. Requests are limited to 1 MiB, `n` to 1–1000,
and incomplete requests time out after two seconds. A missing `n` defaults to
3 for `related` and `similar`, and 10 for `search`.

`related` and `search` rank distinct pages by their best 180-word chunk (40-word
overlap). GPU retrieval reranks the ten best pages using raw cross-encoder
logits. `related` filters each result, regardless of `n`, by `WIKI_EMBED_RERANK_THRESHOLD` (default
0.274) or `WIKI_EMBED_COSINE_THRESHOLD` (0.654). Rules that normalise a score
against the other candidates -- a top-two gap, a margin, a z-score -- were
measured and are both worse and more sensitive to a single outlier, so the raw
score is the only test. Explicit `search` returns the top matches with no cutoff.

### Dense probes with page overrides

`dense` takes `query` and a required `n` (1–100), ranks pages by their best chunk's
cosine, and uses neither a reranker nor a threshold on either tier. `probe` uses
the same ranking to compare temporary versions of pages against the live snapshot:

```json
{"op":"probe","queries":["캐시 오류","cache failure"],"n":10,"states":{"parent":{"pages/knowledge/cache.md":"---\ntitle: Cache\n---\nOld body"},"commit":{"knowledge/cache":"---\ntitle: Cache\n---\nNew body","knowledge/obsolete":null}}}
```

`queries` contains 1–200 nonempty strings. `n` is required and must be 1–100.
`states` contains 1–4 named mappings; names match `[a-z][a-z0-9_-]{0,31}`. Each
mapping overrides at most 50 pages, keyed by page ID or relative markdown path
(the same validation as `similar.exclude`). Aliases for the same page cannot
appear twice in a state. A string supplies the full markdown text; `null` removes
the page. Empty mappings and empty page texts are allowed. The whole request
still fits within the 1 MiB line limit.

For each state, all named pages are removed from the snapshot and supplied texts
are chunked and embedded fresh in batches of eight, even if identical to indexed
text or another state's text. Unnamed pages retain their snapshot vectors. This
does not change the live index, cache or files. Every state and query uses one
tier; a GPU error retries the entire request on CPU. The response contains up to
`n` distinct page IDs per query, in query order:

```json
{"ok":true,"tier":"cpu","results":{"parent":[["knowledge/cache"],["knowledge/cache"]],"commit":[["knowledge/cache"],["knowledge/cache"]]}}
```

The curator gives these requests a 120-second timeout. Warmup and invalid-request
errors use the usual `{"ok":false,"error":"..."}` envelope.

### Page similarity

`similar` chunks the supplied page text with the same title/tag prefix and
180-word windows as indexing, encodes them as passages, and averages then
L2-normalizes the chunk vectors. Each snapshot caches the same normalized mean
for every indexed page. It returns up to `n` pages with cosine at least `min`,
excluding either the page ID (`knowledge/cache`) or relative path
(`pages/knowledge/cache.md`) in `exclude`, as `{"path":"knowledge/other","score":0.91}`.
It does not rerank.

`pairs` compares page vectors for the requested types (`knowledge`, `followups`,
`projects`, `history`). It returns `{"a":"knowledge/one","b":"knowledge/two","score":0.91}`
items at or above `min`, sorted by descending score with page IDs breaking ties,
capped at 500. The CLI filters closed follow-ups from its lint worklist. Both new
operations require a finite `min` between -1 and 1 and use the usual
`{"ok":true,"tier":"cpu","results":[...]}` envelope. The Rust client gives them
5,000 ms by default (`CLAUDE_WIKI_SEMANTIC_BATCH_TIMEOUT_MS`); hooks keep their
separate 300 ms budget.

Both tiers run bge-m3, so there is one shared in-memory index and vector file,
with no re-embedding when the tier changes. When the card has room at startup,
the GPU models load first and begin indexing without waiting for the CPU model.
The CPU model loads next and stays resident for fallback. Model constructors are
serialized because Transformers temporarily changes torch's global default dtype;
queries and indexing do not take that constructor lock. The GPU tier loads as soon as
the card has ≥8 GB free, without waiting, and after yielding it waits for that to
hold continuously for 60 seconds before taking the card back. Busy percent never
gates loading -- a game server or a compositor alone sits at 10-30 % with spikes,
so any low bar would keep the tier from loading at all -- but it does decide when
to let go. It yields immediately below
6 GB free, or after five seconds at ≥50% busy while none of our own GPU work
occurred between polls. Polling is every two seconds, so a five-second interval
is acted on at the next poll. Missing telemetry also yields. These are decimal
GB, matching the benchmark. `WIKI_EMBED_NO_GPU=1` prevents all GPU model loads.

One background index worker checks the read-only SQLite catalog every five
seconds. It waits for a requested GPU to finish loading, or uses the CPU when the
GPU is unavailable or disabled. Both tiers read the same immutable snapshot, so
they never duplicate a rebuild or race writes to the shared cache. Unchanged
`(path, mtime_ns)` entries reuse their vectors; sections can change without
embedding again. Blocked/deleted
pages are removed before embedding additions. Pages edited since the CLI's
last scan are deferred until its metadata catches up. Completed pages are
published as the index grows: `status.pages` and `status.chunks` report progress,
and requests can search that partial index. Progress is checkpointed every five
seconds and at completion or handoff. An interrupted page is finished by the next
tier; already completed pages are reused. The shared atomic
`vectors-BAAI--bge-m3.npz` cache lives in `$XDG_DATA_HOME/claude-wiki` (default
`~/.local/share/claude-wiki`). Cache files contain no pickle objects. Requests
never wait on the refresh, model loader, or telemetry reader. During initial
startup, requests return a warmup error until a model and some vectors are available
(an empty catalog is ready immediately);
`status` is always available, with `indexed_at: 0` before the first index.

For an isolated manual run, use `wiki-embed --wiki-dir /tmp/wiki --socket
/tmp/wiki-runtime/embed.sock`. The package's `checkPhase` runs
`python -m unittest discover -s tests -v` with numpy and fake models; no torch,
GPU, Hugging Face cache, or network is needed by those tests.

The startup tests run the real daemon workers with gated fake loaders and encoders.
They cover a stalled CPU loader, a pending GPU load, partial indexing, GPU failure,
and handoff without duplicate embedding. This guards against the former startup
dependency where the GPU loader waited for `cpu_ready`, which was only set after
the entire cold CPU index completed.
