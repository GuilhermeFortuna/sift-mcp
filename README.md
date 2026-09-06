# Sift MCP

> **Make the GPU do the looking. Let the coding agent do the thinking.**

Sift is a local code-intelligence MCP server for cloud coding agents such as
Claude Code, Codex, and Cursor. It turns a workstation GPU—targeting an RTX
2060—into a fast, private coprocessor for indexing repositories, retrieving
relevant symbols, and finding analogous code.

```text
RTX 2060                         Cloud coding agent
embed · retrieve · rank    →     reason · plan · edit · review
```

The result is less time spent dumping whole files into context, better answers
to natural-language code questions, and a retrieval layer that remains local.

> **Project status:** active Phase 1 development. The codebase and CPU-safe
> validation path are present; target-GPU measurements and the formal Phase 1
> acceptance record are still tracked in [docs/development/STATUS.md](docs/development/STATUS.md).

## Why Sift exists

`grep` is excellent at exact strings. It is less useful when the question is:

- “Where are decoder timestamps made monotonic?”
- “Show me code similar to this error-handling function.”
- “Which symbol should I open before changing this behavior?”

Sift combines lexical search for identifiers and error strings with dense
semantic search for intent. Results are deliberately **triage-sized**: each
hit includes its file, symbol, signature, lines, preview, and scores. Only
`get_symbol` returns a complete symbol body.

## Architecture

```text
┌──────────────────────┐
│ Cloud coding agent   │
└──────────┬───────────┘
           │ MCP stdio
┌──────────▼───────────┐       spawned on first request
│ Thin mcp-client      │────────────────────────────┐
│ no model, no index   │                             │ Unix socket
└──────────┬───────────┘                             │
           │                                          ▼
           │                              ┌──────────────────────┐
           │                              │ Resident sift-daemon │
           │                              │ models + indexes     │
           │                              └──────┬───────────────┘
           │                                     │
           │               ┌─────────────────────┼─────────────────────┐
           │               ▼                     ▼                     ▼
           │        tree-sitter             Tantivy              exact dense
           │        symbol chunks            BM25                 matmul
           │               └─────────────────────┼─────────────────────┘
           │                                     ▼
           │                              RRF fusion → results
           │
           └──────────────────────────────────────────────────────────────
```

The client is spawned per agent session, so it stays deliberately small and
lazy. The daemon owns the expensive state, auto-starts when needed, serves
requests over a protected Unix socket, and evicts resident GPU state after an
idle timeout.

## What is implemented

| Layer | Responsibility |
| --- | --- |
| `indexing` | Tree-sitter symbol extraction, stable content hashes, Git-aware incremental updates, and pre-read exclusions |
| `storage` | SQLite chunk metadata plus an fp16 embedding matrix with tombstones, verification, and compaction |
| `inference` | Common `Embedder` trait, CPU `MockEmbedder`, artifact verification, pooling, normalization, and optional CUDA/ONNX inference |
| `retrieval` | Tantivy BM25, exhaustive dense ranking, reciprocal-rank fusion, diagnostics, and bounded previews |
| `daemon` | Resident model/index service, Unix-socket protocol, request routing, progress frames, and idle eviction |
| `mcp-client` | Thin Rust stdio server exposing the four Phase 1 tools |
| `eval` | Git-mined labels, documentation and handwritten sets, ablations, metrics, and proxy-KPI measurement |
| `tools/` | One-time Python model export and verification; never runtime code |

### Supported source languages

Rust, Python, TypeScript, JavaScript, Go, C, and C++.

Before a file is opened, Sift excludes secrets and common noise such as
`.env*`, keys, credentials, `node_modules/`, `vendor/`, `target/`, `dist/`,
`.venv/`, generated files, binary content, Git-ignored paths, unsupported
languages, and files larger than 1 MiB.

## MCP tools

The Phase 1 surface is intentionally small:

| Tool | Use it for |
| --- | --- |
| `index_repository` | Full or incremental repository indexing, with streamed progress |
| `search_code` | Natural-language or exact-identifier questions over hybrid retrieval |
| `find_similar_code` | Finding analogous implementations from a pasted code snippet |
| `get_symbol` | Fetching the complete body of a known symbol by file and qualified name |

Tool descriptions live in [crates/mcp-client/descriptions.toml](crates/mcp-client/descriptions.toml),
where they can be reviewed and versioned independently from handler code.

Example result record:

```json
{
  "file": "src/timestamp.rs",
  "symbol": "normalize_timestamp",
  "signature": "fn normalize_timestamp(pts: i64, last: i64) -> i64",
  "doc": "Clamp regressing decoder timestamps to monotonic order.",
  "lines": [82, 117],
  "preview": "let mut t = pts;\nif t < last {\n    t = last + 1;",
  "lexical_score": 0.44,
  "dense_score": 0.81,
  "fused_score": 0.032276
}
```

`top_k` defaults to 5 and is bounded to 1–20. `index_repository` defaults to
incremental updates; pass `full=true` only when a rebuild is intentional.

## Quick start: CPU-safe development

The default workspace builds and tests without a GPU, CUDA toolkit, model
files, or ONNX Runtime. Install a current
[rustup](https://rustup.rs/), then:

```bash
git clone <repository-url>
cd sift-mcp

# The one validation command used locally and in CI.
./ci.sh
```

For the local Console workflow, use the single launcher. It builds stale UI
assets and release binaries on demand, starts only the Console, and leaves
daemon lifecycle to Console/MCP repository registrations:

```bash
./scripts/sift run       # Console at http://127.0.0.1:7331
./scripts/sift dev       # Console plus Vite hot reload
./scripts/sift status
./scripts/sift logs
./scripts/sift stop
```

Use `./scripts/sift build` for all release binaries. The explicit
`./scripts/sift daemon` command is reserved for manual CUDA diagnostics and
requires `SIFT_REPO`, `SIFT_MODEL`, and `SIFT_STORE`; it does not make the
launcher responsible for unrelated registered daemons.

Index a repository with the deterministic CPU mock embedder:

```bash
cargo run -p indexing --example index_repo -- /path/to/repository --timing
```

Search the resulting store:

```bash
cargo run -p retrieval --example search -- \
  /path/to/repository/.sift-index \
  --query "where are decoder timestamps normalized?"
```

Print the exact descriptions an MCP client advertises to an agent:

```bash
cargo run -p mcp-client -- --print-tool-descriptions
```

## CUDA-backed setup

The production daemon uses the optional `cuda` feature and an exported fp16
ONNX embedding model. CUDA is isolated behind the `inference` crate; it is not
needed by the default build. The Rust runtime pins `ort` to the CUDA-12-capable
`2.0.0-rc.12` release (ONNX Runtime 1.24.2); CUDA 13-only `ort` releases are
not compatible with this baseline.

```bash
# Build the thin client and the CUDA daemon separately.
cargo build --release -p mcp-client
ORT_CUDA_VERSION=12 cargo build --release -p daemon --bin sift-daemon --features cuda

# Export a pinned model into models/<key>/.
python3 -m venv tools/.venv
tools/.venv/bin/pip install -r tools/requirements.txt
tools/.venv/bin/python tools/export_model.py --model primary
tools/.venv/bin/python tools/verify_export.py --model primary --report-vram
```

The CUDA build requires a working CUDA 12.x installation and matching runtime
libraries visible to the daemon, including cuDNN 9 for the selected ONNX Runtime
CUDA 12 build. Set `ORT_CUDA_VERSION=12` when building
on a host with multiple CUDA toolkits or when CUDA version detection is
ambiguous. The daemon registers only the CUDA execution provider and fails
startup if it cannot initialize; it does not fall back to CPU.

The primary model is Qwen3-Embedding-0.6B (1024 dimensions); the fallback is
`jina-embeddings-v2-base-code` (768 dimensions). Both use 512-token inputs and
fp16 artifacts. See [tools/README.md](tools/README.md) for pinned revisions,
artifact hashes, export notes, and the fallback workflow.

Register Sift in an agent using absolute paths. The model argument must point
to the exported model directory itself, for example `models/primary`:

```json
{
  "mcpServers": {
    "sift": {
      "command": "/absolute/path/to/sift-mcp/target/release/mcp-client",
      "args": [
        "--store", "/absolute/path/to/project/.sift-store",
        "--repo", "/absolute/path/to/project",
        "--model", "/absolute/path/to/sift-mcp/models/primary",
        "--daemon-binary", "/absolute/path/to/sift-mcp/target/release/sift-daemon"
      ]
    }
  }
}
```

Or generate a Cursor-shaped registration snippet with:

```bash
SIFT_REPO=/absolute/path/to/project \
SIFT_STORE=/absolute/path/to/project/.sift-store \
SIFT_MODEL=/absolute/path/to/sift-mcp/models/primary \
MCP_CLIENT_BIN=/absolute/path/to/sift-mcp/target/release/mcp-client \
scripts/register-mcp-server.sh
```

## Design principles

- **Hybrid retrieval:** BM25 catches exact code vocabulary; embeddings catch intent.
- **Exact dense search:** the current corpus target fits an fp16 matrix, so Sift uses exhaustive scoring instead of ANN, FAISS, or a vector database.
- **Metadata first:** agents get enough context to triage without opening whole files.
- **Incremental by content:** moved or renamed symbols can reuse embeddings when their normalized content is unchanged.
- **Fail closed on sensitive paths:** exclusions happen before file contents are read.
- **CPU-first validation:** inference consumers depend on a trait, with GPU tests isolated behind the non-default `cuda` feature.
- **Measured claims only:** accuracy, p95 latency, cold start, and VRAM are tracked as explicit acceptance measurements.

## Current boundaries

These are deliberate follow-on work, not hidden promises:

- Cross-encoder reranking is deferred to Phase 2.
- Impact analysis and relevant-test selection are Phase 3 tools.
- The full agent A/B benchmark is deferred; Phase 1 uses retrieval metrics and a proxy KPI.
- Model files and downloaded ONNX runtime artifacts are local and gitignored.
- GPU parity, target-machine latency, sustained workload, and real-agent usability require the human acceptance runs listed in the project status docs.

## Repository map

```text
crates/
├── mcp-client    thin stdio MCP server
├── daemon        resident Unix-socket service
├── indexing      parsing, exclusions, incremental indexing
├── retrieval     BM25, dense search, fusion, results
├── inference     mock + optional CUDA/ONNX embedding
├── storage       SQLite + fp16 matrix
├── change-intel  change-impact foundation
└── eval          labels, metrics, and acceptance harness
docs/             architecture, tech stack, specs, plans, and status
scripts/          registration, timing, load, and daemon checks
tools/            one-time model export and verification
```

## Development

Read [AGENTS.md](AGENTS.md) before making changes. The project uses one
branch per numbered SIFT task, keeps runtime code in Rust, and treats
[./ci.sh](ci.sh) as the single validation entry point.

For crate boundaries, optional CUDA rules, contribution workflow, and local GPU
tests, see [CONTRIBUTING.md](CONTRIBUTING.md). For the design rationale, see
[docs/tech-stack.md](docs/tech-stack.md) and the
[project direction](docs/cuda-mcp-rtx2060-plan.md).

## License

No license has been declared yet.
