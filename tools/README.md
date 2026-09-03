# tools/

One-time, non-runtime scripts for model export and verification. **Not shipped**
with the daemon and never imported by Rust crates.

## Dependencies

```bash
python3 -m venv tools/.venv
tools/.venv/bin/pip install -r tools/requirements.txt
```

If a CUDA torch wheel exceeds disk quota, install the CPU wheel first:

```bash
tools/.venv/bin/pip install torch==2.14.0 --index-url https://download.pytorch.org/whl/cpu
tools/.venv/bin/pip install -r tools/requirements.txt
```

Versions are pinned in `requirements.txt`. Do not install these into the Rust
workspace. `tools/.venv/` is local and must not be committed.

## Pinned embedding models

| Key | Hugging Face id | Pinned revision |
| --- | --- | --- |
| `primary` | `Qwen/Qwen3-Embedding-0.6B` | `97b0c614be4d77ee51c0cef4e5f07c00f9eb65b3` |
| `fallback` | `jinaai/jina-embeddings-v2-base-code` | `516f4baf13dec4ddddda8631e019b5737c8bc250` |

Both keys share one export path. The scripts abort if the resolved revision
differs from the pin.

Practical truncation length used by the export and fixture: **512** tokens.
Pooling and L2 normalization are **not** baked into the ONNX graph.

| Key | Dims | Pooling | Query prefix | Document prefix | ONNX opset |
| --- | --- | --- | --- | --- | --- |
| `primary` | 1024 | `last_token` | instruction template (see metadata) | none | **18** (see notes) |
| `fallback` | 768 | `mean` | none | none | 17 |

## Commands

Export writes under `models/<key>/` (gitignored): the fp16 ONNX graph,
tokenizer files, and `metadata.json` with artifact SHA-256 hashes.

```bash
tools/.venv/bin/python tools/export_model.py --model fallback
tools/.venv/bin/python tools/export_model.py --model primary

tools/.venv/bin/python tools/verify_export.py --model fallback
tools/.venv/bin/python tools/verify_export.py --model primary
tools/.venv/bin/python tools/verify_export.py --model primary --report-vram
```

Verification compares the framework (fp32 and fp16) and the exported graph
against the committed reference fixtures in
`crates/inference/fixtures/<key>-reference.json`.

## Artifact location

| Path | Committed? |
| --- | --- |
| `models/<key>/model.onnx` (+ optional `.onnx.data`) | No |
| `models/<key>/tokenizer*` | No |
| `models/<key>/metadata.json` | No |
| `tools/artifact-hashes.json` | Yes (expected SHA-256 after a verified export) |
| `crates/inference/fixtures/<key>-reference.json` | Yes |
| `tools/fixture_inputs.json` | Yes |

Obtain artifacts by running the export commands above on a machine with the
pinned Python deps and Hugging Face access. Content hashes in
`tools/artifact-hashes.json` must match `metadata.json` after export.

## Primary export notes

- Torch 2.14's dynamo ONNX exporter emits **opset 18** for Qwen3 and fails to
  down-convert to 17 (`axes` attribute assertion in the ONNX C version
  converter). The classic TorchScript exporter (`dynamo=False`) aborts with
  `unordered_map::at` on rotary attention. We keep the dynamo graph at opset 18
  and record the actual opset in metadata.
- Last-token pooling is **not** baked into the graph. Verification and the
  future Rust runtime must use **left padding** for the primary model so the
  final position is content, then take the last non-pad hidden state and L2-
  normalize.
- Query instruction prefix is recorded in metadata / applied by verify when
  `role == "query"`; documents stay unprefixed.
- Both primary and fallback fixtures verified within tolerance; **SIFT-005
  should target `primary-reference.json`**, with `fallback-reference.json` as
  the alternate oracle.
