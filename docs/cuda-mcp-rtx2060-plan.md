# CUDA MCP Plan for RTX 2060 (v2)

## Goal

A local MCP server using an RTX 2060 6 GB as a code-intelligence coprocessor for cloud coding agents (Claude Code, Codex, Cursor).

The GPU does not reason. It decides what is worth spending cloud tokens on.

```text
RTX 2060:  embed, retrieve, rank, find-similar
Cloud:     reason, plan, edit, review, decide
```

## Hard constraints

### Independent UI series

The local console is a separately authorized **UI series, Batch 01**, using
task IDs UI-001 through UI-005. It covers passive observability, a local
multiple-repository console, repository operations, and search inspection.
See [the authoritative task ledger](development/STATUS.md#ui-series--batch-01-local-console)
for its spec/plan pairs and cross-series dependencies, and
[the governing stack decision](tech-stack.md#local-console--ui-series-batch-01)
for its Rust service and browser frontend.

This work is not SIFT Batch 02. It neither advances the Phase 2 retrieval gate
nor replaces the pinned-corpus or human/hardware acceptance evidence. Planning
and implementing a UI task never implies authorization to execute another SIFT
task or to mark its acceptance complete.

### Runtime limits

| Constraint | Value |
|---|---|
| VRAM budget | ~5.0 GB usable (desktop attached to GPU) |
| Precision | fp16 only — Turing has no bf16 |
| Corpus size | 10k–200k symbol chunks |
| Latency SLO | `search_code` p95 < 400 ms end-to-end |

The latency SLO is the project's life-or-death number. If `search_code` is slower
than a grep, the agent stops calling it and the project is dead.

Budget breakdown:

```text
query embedding      < 15 ms
lexical search       < 30 ms
dense search         < 10 ms   (exact matmul, see Storage)
rerank 50 candidates < 300 ms
```

---

## Architecture

```text
Cloud coding agent
      |
      v  MCP stdio (thin client, no torch)
Code Intelligence MCP client
      |
      v  unix socket
Persistent daemon (models resident)
      |
      +-- Lexical index (CPU)  ---+
      |                           +--> RRF fusion --> reranker --> results
      +-- Dense index (GPU)    ---+
      |
      +-- Static index (tree-sitter / SCIP)
      |
      v
 RTX 2060 6 GB
```

### Why a daemon

MCP servers are spawned per-client, often per-session. Torch import plus two model
loads is 20–40 s. That cost cannot be paid on every agent start.

- **MCP process**: thin stdio client, stdlib only, starts in ms.
- **Daemon**: holds models, index, and cache resident. Auto-starts on first
  connection, idles out after N minutes.

---

## Retrieval design

### Hybrid, not dense-only

Dense-only retrieval underperforms BM25 on code for exact identifiers, error
strings, and symbol names. Both paths ship in Phase 1.

```text
query
  |
  +--> BM25 / trigram (tantivy or ripgrep) --> top 50
  |
  +--> dense embedding --> exact matmul   --> top 50
  |
  v
Reciprocal Rank Fusion
  |
  v
union (~75 unique) --> cross-encoder --> top 3-10
```

### Storage: no vector DB

At 200k chunks × 1024 dims × fp16 = 400 MB. A single `X @ q` on the GPU is
sub-millisecond and **exact** — no ANN recall loss, no index build, no FAISS.

```text
SQLite      : metadata, symbol table, content hashes
.npy memmap : fp16 embedding matrix, row index == SQLite rowid
```

FAISS is off the roadmap. Revisit only above ~2M chunks.

### Chunking

Symbol-aware chunking via tree-sitter is the single largest quality lever.
It belongs in Phase 1, not as a later refinement.

Units: functions, methods, classes, structs, impl blocks, modules, tests.

Chunk record:

```json
{
  "repository": "vision-engine",
  "file": "src/tracker.rs",
  "language": "rust",
  "symbol": "Tracker::update",
  "symbol_type": "method",
  "signature": "fn update(&mut self, det: &Detection) -> Result<()>",
  "doc_first_line": "Advance the track with a new detection.",
  "line_start": 103,
  "line_end": 182,
  "content_hash": "sha256(...)",
  "embedding_row": 41822
}
```

Oversized symbols: split on statement boundaries with the signature prepended to
each fragment, so every fragment remains independently interpretable.

### Index-time exclusions

Non-negotiable, enforced before embedding:

```text
.env, .env.*, *.pem, *.key, id_rsa*
credentials.*, secrets.*
node_modules/, vendor/, target/, dist/, .venv/
generated files (*_pb2.py, *.generated.*)
binary and minified files
```

---

## MCP tool surface

Start deliberately small. Every tool must beat a baseline the agent already has.

```text
index_repository(path)

search_code(query, top_k=5)          # hybrid + rerank
find_similar_code(code, top_k=5)     # no grep baseline exists — highest value
find_impacted_code(diff)             # static-first, see Phase 3
find_relevant_tests(diff)
get_symbol(file, symbol)             # fetch full body after metadata triage
```

### Tool descriptions are a tuned artifact

Whether an agent calls `search_code` or falls back to grep is determined almost
entirely by the tool description text. Treat it as code:

- Version it.
- Eval it (does the agent call it on queries where it would help?).
- Include a concrete example query and an explicit "prefer this over grep when…"
  clause.

This gets its own benchmark, separate from retrieval quality.

---

## Response shape: metadata-first

Line numbers alone are too thin — the agent resolves them by reading the entire
file, defeating the purpose. Return enough to triage:

```json
[
  {
    "file": "src/timestamp.rs",
    "symbol": "normalize_timestamp",
    "signature": "fn normalize_timestamp(pts: i64, last: i64) -> i64",
    "doc": "Clamp regressing decoder timestamps to monotonic order.",
    "preview": "let mut t = pts;\nif t < last {\n    t = last + 1;",
    "lines": [82, 117],
    "lexical_score": 0.44,
    "dense_score": 0.81,
    "rerank_score": 0.96
  }
]
```

Full body only via `get_symbol()`. Never return whole files.

---

## Caching and incremental indexing

Key on normalized content, **excluding path**, so moved or renamed files do not
trigger re-embedding:

```text
key = sha256(normalize_whitespace(symbol_body))
```

On repository change:

```text
git diff --name-status
   |
   v
re-parse touched files only
   |
   +-- unchanged hash  -> reuse embedding computation, retain occurrence row
   +-- new hash        -> embed, append occurrence row
   +-- vanished hash   -> tombstone row
```

Compact the matrix when tombstones exceed ~20%. Never full-reindex.

---

## Evaluation

### Mine labels from git history

Hand-authoring 10–50 queries as the repo's own author produces a tiny, biased
benchmark. Git history yields thousands of free labeled pairs:

| Source | Query | Expected |
|---|---|---|
| Commits touching 1–3 symbols | commit subject line | changed symbols |
| Bug-fix commits | linked issue title | symbols in the fix |
| Documented symbols | docstring (held out of index) | that symbol |

Filter aggressively: drop merges, drop commits touching >3 symbols, drop
subjects under 4 words or matching `wip|fixup|typo|bump|lint`.

Hold out a hand-written set of ~30 natural questions as a sanity check only —
never as the primary metric.

### Metrics

```text
Top-1 / Top-3 / Top-10 accuracy
MRR
p50 / p95 latency
peak VRAM
```

### Proxy KPI (Phases 1–2)

The full agent A/B is confounded, slow, and expensive. Defer it. Use instead:

```text
bytes of code returned before the correct symbol is in context
    MCP-equipped agent  vs.  grep-only baseline agent
```

Run the real agent benchmark once, after Phase 3, to confirm the proxy tracked
reality.

---

## Model strategy

Prefer several small specialized models over one general coding LLM.

```text
embeddings : Qwen3-Embedding-0.6B (fp16, ~1.3 GB) — instruction-tuned, strong on code
             fallback: jina-embeddings-v2-base-code (137M, 8k ctx)
reranker   : bge-reranker-v2-m3  or  Qwen3-Reranker-0.6B (fp16)
```

VRAM policy: do **not** hold both resident by default. Embedder stays loaded;
reranker lazy-loads on first rerank and evicts after idle. Verify headroom with
a desktop session attached, not on a headless box.

Selection criterion: the smallest model that clears the Top-3 target.

---

## Stack

```text
Python 3.11+
MCP Python SDK
PyTorch CUDA (fp16)
sentence-transformers / transformers
tree-sitter
SQLite
NumPy
tantivy  (or ripgrep shell-out for v0)
```

Deferred until profiling justifies it: ONNX Runtime CUDA, TensorRT.
Never start with custom CUDA kernels.

---

## Phases

### Phase 1 — Retrieval foundation

- tree-sitter symbol chunking
- hybrid lexical + dense retrieval with RRF
- SQLite + memmap storage
- persistent daemon + thin MCP client
- git-mined eval harness
- `index_repository`, `search_code`, `find_similar_code`, `get_symbol`

Exit criteria:

```text
Top-3 >= 0.80 on git-mined eval
p95 latency < 400 ms
cold agent start < 200 ms
```

### Phase 2 — Reranking

Add cross-encoder over the fused candidate set. **Gated**: keep only if Top-1
improves by a material margin over hybrid-without-rerank at acceptable latency.
If the delta is small, drop it and reclaim the VRAM.

### Phase 3 — Change intelligence (static-first)

Impact analysis is a static-analysis problem, not an embedding problem. The
original plan inverted this.

```text
callers / callees / references  -> tree-sitter + SCIP/LSIF, or pyright/cargo output
tests covering a symbol         -> static call graph, then test-name heuristics
specs and docs                  -> embeddings (no static edge exists)
untyped / dynamic edges         -> embeddings as fallback only
```

`find_impacted_code(diff)` returns static edges ranked first, semantic
candidates clearly marked as lower confidence.

### Stop and measure

Run the real agent A/B here. Do not build further until it shows a win.

### Deferred indefinitely

- **`cluster_failures`** — normalizing tracebacks and hashing (top frame +
  assertion type) captures ~90% of the compression deterministically, on CPU,
  in ~50 lines. GPU embeddings add little. Ship as a CPU utility if wanted at
  all; it is not a GPU workload.
- **`classify_items`** — zero-shot classification from a model small enough to
  coexist with the reranker is weak. If ever needed, implement as cosine
  similarity against label-prototype embeddings: no extra model, no extra VRAM.

---

## Repository structure

```text
code-intelligence-mcp/
├── pyproject.toml
├── src/code_intelligence_mcp/
│   ├── mcp_client.py          # thin stdio client, stdlib only
│   ├── daemon/
│   │   ├── server.py          # unix socket, model residency
│   │   └── lifecycle.py       # lazy load, idle evict
│   ├── indexing/
│   │   ├── repository.py
│   │   ├── chunker.py         # tree-sitter
│   │   ├── exclusions.py
│   │   └── incremental.py     # hash-keyed, git-aware
│   ├── retrieval/
│   │   ├── embeddings.py
│   │   ├── lexical.py
│   │   ├── dense.py           # memmap + matmul
│   │   ├── fusion.py          # RRF
│   │   └── reranker.py
│   ├── change_intelligence/
│   │   ├── static_graph.py    # primary
│   │   └── semantic.py        # fallback
│   └── storage/
│       ├── db.py              # SQLite
│       └── matrix.py          # fp16 memmap
├── benchmarks/
│   ├── mine_from_git.py
│   ├── retrieval_eval.py
│   ├── tool_description_eval.py
│   └── agent_ab/
└── tests/
```

---

## Design principle

> Use the local RTX 2060 to determine what information is worth spending
> cloud-model tokens on.

Every component earns its place by beating a baseline the agent already has.
Anything that does not — drop it.
