# Recommended Stack

Companion to `cuda-mcp-rtx2060-plan-v2.md`.

## Decision

**Rust workspace.** Python confined to `tools/` for one-time model export.
No Python at runtime.

Rationale: every CPU-heavy component (parsing, lexical search, git, storage) is
a native Rust crate — the Python equivalents are bindings to the same code. GPU
inference goes through ONNX Runtime, which is the same C++ kernel path torch
would take, minus the interpreter and ~2 GB of dependencies. Daemon starts in
milliseconds; indexing parallelizes across cores without a GIL.

---

## Runtime

| Component | Choice | Notes |
|---|---|---|
| Language | Rust (stable, edition 2024) | |
| Async runtime | `tokio` | daemon, socket server, MCP transport |
| MCP | `rmcp` | official Rust SDK; stdio server for the thin client |
| Inference | `ort` (ONNX Runtime) | CUDA execution provider, fp16 |
| Parsing | `tree-sitter` + language grammars | symbol-aware chunking |
| Lexical search | `tantivy` | BM25, on-disk index, near-instant open |
| Dense storage | `memmap2` + `half` | fp16 matrix, one row per chunk |
| Dense search | `cudarc` or `ort` matmul | exact `X @ q`; no ANN |
| Metadata | `rusqlite` (bundled SQLite) | symbols, hashes, row mapping |
| Git | `gix` | diff, changed files, history mining |
| Hashing | `blake3` | content keys; faster than sha256, same purpose |
| Tokenization | `tokenizers` (HF, Rust-native) | same tokenizer files as the Python models |
| Serialization | `serde` + `serde_json` | MCP payloads, eval fixtures |
| Logging | `tracing` + `tracing-subscriber` | per-request latency spans |
| Errors | `thiserror` (lib) / `anyhow` (bin) | |
| Config | `figment` or `config` | TOML in repo root |

### Dense search note

At ≤200k chunks × 1024 dims × fp16 = ~400 MB, exact matmul on the GPU is
sub-millisecond. Start with `ort` executing a tiny exported matmul graph, or
`cudarc` with cuBLAS `gemv` if `ort` adds measurable overhead. Do not add FAISS,
hnswlib, or a vector DB.

---

## Models

| Role | Primary | Fallback | Format |
|---|---|---|---|
| Embedding | Qwen3-Embedding-0.6B | jina-embeddings-v2-base-code | ONNX, fp16 |
| Reranker | bge-reranker-v2-m3 | Qwen3-Reranker-0.6B | ONNX, fp16 |

VRAM policy: embedder resident; reranker lazy-loaded on first rerank, evicted
after idle timeout. Verify headroom with a desktop session attached — usable
VRAM is ~5.0 GB, not 6.

Pooling and L2 normalization are implemented in Rust (~30 lines). Confirm
against the Python reference output on a fixed batch before trusting any model.

---

## Tooling (Python, non-runtime)

Lives in `tools/`. Not shipped. Not imported by the daemon.

| Script | Purpose | Deps |
|---|---|---|
| `export_model.py` | HF → ONNX, fp16, opset 17 | `torch`, `optimum`, `onnx` |
| `verify_export.py` | compare Rust vs torch embeddings on a fixed batch | `torch`, `numpy` |
| `mine_git_labels.py` | optional; git-mined eval can also be done in Rust | `gitpython` |

Expect Qwen3 export to need attention (rotary embeddings, last-token pooling).
Budget a day. If it fights you, ship with the fallback models and revisit.

---

## Build and dev

| Concern | Choice |
|---|---|
| Workspace | Cargo workspace, one crate per plan section |
| CUDA | CUDA 12.x toolkit; `ort` downloads a matching ONNX Runtime binary |
| Lint | `clippy` with `-D warnings`; `rustfmt` default |
| Tests | `cargo test` + `insta` snapshots for MCP responses |
| Benchmarks | `criterion` for hot paths; custom harness for retrieval eval |
| CI | GitHub Actions, CPU-only (mock the inference trait) |
| Release | single static binary per platform |

### Crate layout

```text
crates/
├── mcp-client/        # thin stdio MCP server, stdlib + rmcp only, no torch, no ort
├── daemon/            # socket server, model residency, request routing
├── indexing/          # tree-sitter chunking, exclusions, incremental hashing
├── retrieval/         # lexical, dense, RRF fusion, rerank
├── inference/         # ort session management, pooling, batching
├── storage/           # sqlite + memmap
├── change-intel/      # static graph (primary), semantic fallback
└── eval/              # git-mined labels, metrics, latency reports
tools/
└── export_model.py
```

`inference` exposes a trait so `retrieval` and `eval` compile and test without
CUDA. CI runs against a mock; GPU tests are `#[ignore]` and run locally.

---

## Explicitly rejected

| Option | Why not |
|---|---|
| Python runtime | GIL on indexing, 20–40 s startup, ~100 ms per-request overhead on a 400 ms SLO |
| Rust daemon + Python inference process | two runtimes and IPC on the hot path, only to avoid a one-time ONNX export |
| Go | no tree-sitter parity, weak ONNX story, ends up cgo-wrapping the same C++ |
| C++ | same result as Rust with worse tooling and no MCP SDK |
| `candle` for inference | viable, but ONNX Runtime has broader op coverage and a more mature CUDA EP; reconsider if `ort` becomes a maintenance burden |
| FAISS / hnswlib / vector DB | corpus is small enough for exact matmul; ANN adds recall loss for nothing |
| TensorRT / custom CUDA | only after profiling shows `ort` is the bottleneck |

---

## Migration path if the corpus outgrows the design

- Dense search: swap exact matmul for `usearch` (Rust-native HNSW) above ~2M chunks.
- Inference: `ort` → TensorRT EP is a config change, not a rewrite.
- Storage: SQLite → anything; the schema is four tables.

None of these are expected within the RTX 2060's useful life.
