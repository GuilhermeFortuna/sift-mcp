# Contributing

Install a current [rustup](https://rustup.rs/) and clone this repository.
`rust-toolchain.toml` pins the compiler (currently 1.98.0) and the `rustfmt`
and `clippy` components; rustup installs them on first `cargo` invocation in
the tree.

The single validation command is [`./ci.sh`](ci.sh). Run it before you push.
Continuous integration runs that same script and nothing else.

Install the repository hooks once after cloning:

```bash
git config core.hooksPath .githooks
```

The pre-commit hook checks whitespace, applies rustfmt, automatically applies
machine-fixable Clippy suggestions, and then fails on remaining warnings. The
pre-push hook runs ./ci.sh, which is the complete local and CI validation
suite.

Workflow rules, branch naming (`SIFT-NNN-slug`), design authority
(`docs/tech-stack.md` over `docs/cuda-mcp-rtx2060-plan.md`), and the constraint
that GPU code stays behind the `inference` trait live in [`AGENTS.md`](AGENTS.md).
Read that file; this one does not repeat it.

## Crate layout

Each crate is a library. Behaviour arrives in later tasks.

| Crate | Purpose |
| --- | --- |
| `crates/mcp-client` | Thin stdio MCP server; stdlib + `rmcp` only, no inference or search index |
| `crates/daemon` | Unix-socket server, model residency, request routing |
| `crates/indexing` | Tree-sitter chunking, exclusions, incremental hashing |
| `crates/retrieval` | Lexical search, dense search, RRF fusion, rerank |
| `crates/inference` | ONNX Runtime session management, pooling, batching |
| `crates/storage` | SQLite metadata plus fp16 memmap matrix |
| `crates/change-intel` | Static graph (primary), semantic fallback |
| `crates/eval` | Git-mined labels, metrics, latency reports |

[`tools/`](tools/) holds one-time Python scripts (model export). It is not
runtime and is not imported by any crate.

## GPU tests (local, optional)

Default `./ci.sh` is CPU-only: the `cuda` feature is off and GPU tests are
`#[ignore]`. On a machine with the CUDA 12.x toolkit, enable the feature and
run the ignored tests:

```bash
cargo test -p inference --features cuda -- --ignored
```

ONNX Runtime is pulled in by the optional `ort` dependency when that feature
is on; you do not install it separately for the default build.
