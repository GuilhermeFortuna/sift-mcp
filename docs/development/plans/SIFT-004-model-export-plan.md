# SIFT-004 implementation plan: Model export and reference fixture

**Status:** authoritative in [`../STATUS.md`](../STATUS.md)  
**Specification:** [`../specs/SIFT-004-model-export-spec.md`](../specs/SIFT-004-model-export-spec.md)  
**Depends on:** SIFT-001

## Current-system context

SIFT-001 created `tools/` with a README stating it is non-runtime and no Python
packaging, and added `.gitignore` entries for `models/` and `*.onnx`. The
workspace has no model artifacts and `crates/inference` is empty behind a
default-off `cuda` feature. Nothing in the repository knows the embedding
width, the maximum sequence length, or the pooling strategy any model expects,
and `storage::MatrixHeader` from SIFT-002 has a `model_id` field with nothing to
put in it.

`docs/tech-stack.md` names Qwen3-Embedding-0.6B as primary with
jina-embeddings-v2-base-code as fallback, requires fp16 and opset 17, warns that
the Qwen3 export will need attention for rotary embeddings and last-token
pooling, and budgets a day for it. The gap this task closes is that the runtime
has no model to load and — the part that matters — no independent evidence of
what correct output looks like.

## Interfaces produced

This task produces no Rust surface. It produces two scripts, a metadata
document, and a fixture consumed by SIFT-005.

```python
# tools/export_model.py
def export(model_key: str, out_dir: Path) -> ExportMetadata:
    """HF checkpoint at a pinned revision -> fp16 ONNX graph + tokenizer files.

    model_key is "primary" or "fallback"; both take the same code path.
    Raises if the resolved revision differs from the pinned one.
    """
```

```python
# tools/verify_export.py
def verify(model_key: str, out_dir: Path, report_vram: bool) -> VerifyReport:
    """Runs the fixture inputs through the framework model and through the
    exported graph, and compares both against the committed reference vectors.
    Non-zero exit if any comparison exceeds the fixture's tolerance.
    """
```

```jsonc
// models/<model_key>/metadata.json  (generated; not committed)
{
  "model_id": "Qwen/Qwen3-Embedding-0.6B@<revision-sha>",  // goes into MatrixHeader.model_id
  "revision": "<git sha of the HF repo>",
  "dims": 1024,                    // embedding width in elements
  "max_sequence_length": 512,      // tokens; inputs beyond this are truncated
  "pooling": "last_token",         // "last_token" | "mean" | "cls"
  "normalize": "l2",
  "query_prefix": "<instruction template, or null>",
  "document_prefix": null,
  "opset": 17,
  "precision": "fp16",
  "onnx_sha256": "<hash of the exported graph>",
  "tokenizer_sha256": "<hash of the tokenizer files>"
}
```

```jsonc
// crates/inference/fixtures/<model_key>-reference.json  (committed)
{
  "model_id": "...",
  "dims": 1024,
  "tolerance": {
    "metric": "cosine_distance",
    "max": 1e-3,                   // per-vector ceiling
    "basis": "why this value; see Implementation decisions"
  },
  "cases": [
    {
      "name": "code_snippet",
      "text": "...",
      "role": "document",          // "document" | "query"
      "tokens": [151646, 1236, ...],
      "truncated": false,
      "vector": [0.0123, -0.0456, ...]   // f32 decimal, post-pooling, post-L2
    }
  ]
}
```

## Implementation decisions

- **One export function handles both models, selected by a key into a pinned
  configuration table.** A code path unique to the primary model means the
  fallback is untested until the day the primary fails, which is the day it
  needs to work.

- **The model revision is pinned by commit hash and the script aborts if the
  resolved revision differs.** A tag on a model repository can move; an index
  built against a silently updated checkpoint returns degraded results with no
  error and no way to notice.

- **The graph is exported to return per-token hidden states, with pooling and
  normalization left out.** Baking last-token pooling into the graph would make
  the Rust side unable to detect a pooling mismatch, and the fixture's whole
  purpose is to make pooling independently checkable. The cost is one matrix
  reduction in Rust, which the tech stack document estimates at about thirty
  lines.

- **The fixture stores vectors as decimal f32 text rather than base64 fp16.**
  The fixture is a review artifact and a diff of it should be readable; storing
  the reference at higher precision than the runtime also means the tolerance
  measures the runtime's error rather than the fixture's rounding.

- **Reference vectors come from the framework implementation, not from the
  exported graph.** Verifying the export against itself proves only that ONNX
  Runtime is deterministic. The framework output is the only available ground
  truth, and `verify_export.py` compares both against it.

- **Tolerance is stated as a cosine-distance ceiling per vector, with the value
  chosen by measuring the framework model's own fp32-versus-fp16 spread on the
  fixture inputs and taking a small multiple of the observed maximum.** A
  tolerance picked by intuition either passes a broken export or fails a correct
  one, and neither failure is diagnosable.

- **The fixture pins token sequences alongside vectors.** Without them, a
  divergence in SIFT-005 could be a tokenizer difference or a pooling bug, and
  distinguishing the two after the fact costs more than recording them now.

- **The fixture includes an empty string, an over-length input, and non-ASCII
  text.** These are the cases where tokenizer implementations differ — special
  token handling, truncation side, byte-level fallback — and they are the ones
  a happy-path fixture omits.

- **Prefix conventions are recorded as data in the metadata, not applied by the
  export.** Qwen3 embedding models expect an instruction prefix on queries and
  none on documents; if that convention lives in Python it cannot be applied by
  the Rust runtime, and an asymmetry between how the index was built and how
  queries are embedded degrades every result.

- **Exported artifacts are hashed and the hashes recorded in metadata, while the
  artifacts themselves stay out of version control.** A multi-gigabyte blob in
  git makes the repository unusable; the hash lets a runtime detect that the
  file on disk is not the file that was verified.

- **Dependencies are pinned in a `tools/requirements.txt` that no runtime crate
  references.** The tech stack document confines Python to tooling, and an
  unpinned `torch` is the fastest way to make an export irreproducible.

- **VRAM is measured with a desktop session attached, using peak allocation
  during a representative batch rather than model size alone.** Model size
  understates the requirement by the activation memory, and the tech stack
  document explicitly warns that the usable budget is ~5.0 GB rather than 6 GB.

## Ordered implementation

1. Create the branch `SIFT-004-model-export`.
2. Write `tools/requirements.txt` with pinned `torch`, `transformers`,
   `optimum`, `onnx`, `onnxruntime`, and `numpy` versions, and a
   `tools/README.md` recording the pinned model revisions for both keys, the
   commands, and that nothing here is shipped. Commit.
3. Write the fixture input list — a code snippet, a prose sentence, a bare
   identifier, an empty string, an input longer than the maximum sequence
   length, and non-ASCII text — as a committed inputs file with no vectors yet,
   each case naming its role. Commit.
4. Write `tools/export_model.py` with the pinned configuration table, the
   revision check, fp16 opset-17 export returning per-token hidden states, and
   tokenizer file capture. Run it for the fallback model first, since the tech
   stack document expects it to be the easier export, and confirm the graph
   loads in ONNX Runtime. Commit.
5. Write the metadata emission: model identity and revision, dims, maximum
   sequence length, pooling strategy, normalization, prefix conventions, opset,
   precision, and both artifact hashes. Assert by test that every field is
   present and non-null except the prefixes. Commit.
6. Write `tools/verify_export.py`: run the fixture inputs through the framework
   model at fp32 and at fp16 to measure the model's own precision spread, run
   them through the exported graph, and compare all three. Have it emit the
   observed spread so the tolerance can be chosen from data rather than
   asserted. Run it for the fallback model. Commit.
7. Set the fixture tolerance from the measured spread, record the basis text in
   the fixture, and emit the reference vectors and token sequences for the
   fallback model. Confirm `verify_export.py` passes against the committed
   fixture. Commit.
8. Add a test — runnable with no framework installed — asserting the fixture is
   valid JSON, contains every required case name, and that every case carries
   both a token sequence and a vector of the declared width. Run it against a
   deliberately truncated fixture and confirm it fails. Confirm it passes.
   Commit.
9. Run `export_model.py` for the primary model. Expect the rotary-embedding and
   last-token-pooling issues the tech stack document warns about; record what
   was needed in `tools/README.md`. Commit.
10. Run `verify_export.py` for the primary model, emit its fixture, and confirm
    it passes within tolerance. If the primary export cannot be made to pass
    within the budgeted day, record the failure and its symptom in
    `tools/README.md`, ship the fallback fixture as the one SIFT-005 targets,
    and report this in the handoff rather than extending the task. Commit.
11. Confirm `.gitignore` excludes `models/`, `*.onnx`, and tokenizer artifacts,
    and that the metadata hashes are recorded in a committed location. Confirm
    `git status --porcelain` is clean after a full export. Commit.
12. Human step: run
    `python tools/export_model.py --model primary && python tools/verify_export.py --model primary`
    on the target machine and report the comparison result against tolerance.
13. Human step: run the same two commands for `--model fallback` and report the
    result, confirming the fallback path is exercised rather than assumed.
14. Human step: run `python tools/verify_export.py --model primary
    --report-vram` with a desktop session attached and report peak GPU memory
    for a representative batch against the ~5.0 GB usable budget.
15. Run the full validation suite and confirm it passes.

## Validation

- **Unit:** fixture structural validation, runnable with no framework present,
  exercised against a truncated fixture first.
- **Integration:** `verify_export.py` comparing framework fp32, framework fp16,
  and the exported graph across every fixture case, for both model keys.
- **Regression:** the committed fixture is the locked reference for SIFT-005;
  a change to it after that point invalidates every index built against the
  model and must be justified in review.
- **Manual:** export and verification on the target machine for both models;
  correct means every case within the stated tolerance and a recorded VRAM
  figure.
- **Measurement:** the framework's own fp32-versus-fp16 cosine-distance spread
  across the fixture, reported per case, since it sets the tolerance; peak GPU
  memory for a representative batch with a desktop session attached.

```bash
cargo fmt --all -- --check
cargo clippy --workspace --all-targets -- -D warnings
cargo test --workspace
cargo build --workspace --release
./ci.sh
python -m pip install -r tools/requirements.txt
python tools/export_model.py --model fallback && python tools/verify_export.py --model fallback
python tools/export_model.py --model primary  && python tools/verify_export.py --model primary
python tools/verify_export.py --model primary --report-vram
```

## Handoff

Report, for each model key, the pinned revision, embedding width, maximum
sequence length, pooling strategy, prefix convention, and the exported graph's
size on disk; the measured fp32-versus-fp16 cosine-distance spread per fixture
case and the tolerance chosen from it with its basis; the verification result
for both the framework fp16 path and the exported graph against every fixture
case; peak GPU memory for a representative batch with a desktop session
attached, against the ~5.0 GB budget; whether the primary export required the
rotary-embedding or last-token-pooling attention the tech stack document warned
about and what was done; and, if the primary export was abandoned within the
budgeted day, exactly which failure was hit and that the fallback fixture is the
one SIFT-005 must target.
