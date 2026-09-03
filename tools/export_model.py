#!/usr/bin/env python3
"""Export a pinned embedding checkpoint to fp16 ONNX + tokenizer files.

Pooling and L2 normalization are intentionally left out of the graph so the
runtime (and the fixture) can check them independently.
"""

from __future__ import annotations

import argparse
import hashlib
import json
import shutil
import sys
from dataclasses import asdict, dataclass
from pathlib import Path
from typing import Any

from huggingface_hub import hf_hub_download, snapshot_download
from huggingface_hub.hf_api import ModelInfo, model_info

REPO_ROOT = Path(__file__).resolve().parents[1]
DEFAULT_OUT = REPO_ROOT / "models"

# Instruction template recorded for the primary model; applied by callers, not
# by the export graph.
QWEN_QUERY_PREFIX = (
    "Instruct: Given a code search query, retrieve relevant code passages that "
    "answer the query\nQuery: "
)

MODEL_CONFIG: dict[str, dict[str, Any]] = {
    "primary": {
        "hf_id": "Qwen/Qwen3-Embedding-0.6B",
        "revision": "97b0c614be4d77ee51c0cef4e5f07c00f9eb65b3",
        "dims": 1024,
        "max_sequence_length": 512,
        "pooling": "last_token",
        "normalize": "l2",
        "query_prefix": QWEN_QUERY_PREFIX,
        "document_prefix": None,
        "trust_remote_code": True,
    },
    "fallback": {
        "hf_id": "jinaai/jina-embeddings-v2-base-code",
        "revision": "516f4baf13dec4ddddda8631e019b5737c8bc250",
        "dims": 768,
        "max_sequence_length": 512,
        "pooling": "mean",
        "normalize": "l2",
        "query_prefix": None,
        "document_prefix": None,
        "trust_remote_code": True,
    },
}

TOKENIZER_FILENAMES = (
    "tokenizer.json",
    "tokenizer_config.json",
    "special_tokens_map.json",
    "vocab.txt",
    "vocab.json",
    "merges.txt",
    "added_tokens.json",
    "tokenizer.model",
)


@dataclass
class ExportMetadata:
    model_id: str
    revision: str
    dims: int
    max_sequence_length: int
    pooling: str
    normalize: str
    query_prefix: str | None
    document_prefix: str | None
    opset: int
    precision: str
    onnx_sha256: str
    tokenizer_sha256: str


def _sha256_file(path: Path) -> str:
    h = hashlib.sha256()
    with path.open("rb") as f:
        for chunk in iter(lambda: f.read(1024 * 1024), b""):
            h.update(chunk)
    return h.hexdigest()


def _sha256_paths(paths: list[Path]) -> str:
    h = hashlib.sha256()
    for path in sorted(paths, key=lambda p: p.name):
        h.update(path.name.encode())
        h.update(b"\0")
        with path.open("rb") as f:
            for chunk in iter(lambda: f.read(1024 * 1024), b""):
                h.update(chunk)
    return h.hexdigest()


def resolve_revision(hf_id: str, pinned: str) -> str:
    info: ModelInfo = model_info(hf_id, revision=pinned)
    resolved = info.sha
    if resolved is None:
        raise RuntimeError(f"Hugging Face returned no sha for {hf_id}@{pinned}")
    if resolved != pinned:
        raise RuntimeError(
            f"Revision mismatch for {hf_id}: pinned {pinned}, resolved {resolved}"
        )
    return resolved


def _capture_tokenizer(hf_id: str, revision: str, out_dir: Path) -> list[Path]:
    captured: list[Path] = []
    for name in TOKENIZER_FILENAMES:
        try:
            src = hf_hub_download(repo_id=hf_id, filename=name, revision=revision)
        except Exception:
            continue
        dest = out_dir / name
        shutil.copy2(src, dest)
        captured.append(dest)
    if not captured:
        raise RuntimeError(f"No tokenizer files found for {hf_id}@{revision}")
    return captured


def _export_onnx(
    hf_id: str,
    revision: str,
    trust_remote_code: bool,
    out_onnx: Path,
    max_sequence_length: int,
) -> None:
    import torch
    from transformers import AutoModel, AutoTokenizer

    tokenizer = AutoTokenizer.from_pretrained(
        hf_id, revision=revision, trust_remote_code=trust_remote_code
    )
    model = AutoModel.from_pretrained(
        hf_id, revision=revision, trust_remote_code=trust_remote_code, dtype=torch.float16
    )
    model.eval()

    # Fixed shapes keep ORT happy; batch=1, seq=max for the export graph.
    # Runtime may still pad/truncate per batch to <= max_sequence_length.
    sample = tokenizer(
        "export probe",
        return_tensors="pt",
        padding="max_length",
        truncation=True,
        max_length=min(32, max_sequence_length),
    )
    input_ids = sample["input_ids"]
    attention_mask = sample["attention_mask"]

    class LastHiddenWrapper(torch.nn.Module):
        def __init__(self, inner: torch.nn.Module) -> None:
            super().__init__()
            self.inner = inner

        def forward(self, input_ids: torch.Tensor, attention_mask: torch.Tensor) -> torch.Tensor:
            out = self.inner(input_ids=input_ids, attention_mask=attention_mask)
            if hasattr(out, "last_hidden_state"):
                return out.last_hidden_state
            if isinstance(out, (tuple, list)):
                return out[0]
            raise RuntimeError("Model output has no last_hidden_state")

    wrapped = LastHiddenWrapper(model).to(dtype=torch.float16)
    wrapped.eval()

    dynamic_axes = {
        "input_ids": {0: "batch", 1: "sequence"},
        "attention_mask": {0: "batch", 1: "sequence"},
        "last_hidden_state": {0: "batch", 1: "sequence"},
    }

    out_onnx.parent.mkdir(parents=True, exist_ok=True)
    with torch.inference_mode():
        torch.onnx.export(
            wrapped,
            (input_ids, attention_mask),
            str(out_onnx),
            input_names=["input_ids", "attention_mask"],
            output_names=["last_hidden_state"],
            dynamic_axes=dynamic_axes,
            opset_version=17,
            do_constant_folding=True,
        )


def write_metadata(meta: ExportMetadata, path: Path) -> None:
    path.write_text(json.dumps(asdict(meta), indent=2, ensure_ascii=False) + "\n")


def assert_metadata_complete(meta: ExportMetadata) -> None:
    required = {
        "model_id": meta.model_id,
        "revision": meta.revision,
        "dims": meta.dims,
        "max_sequence_length": meta.max_sequence_length,
        "pooling": meta.pooling,
        "normalize": meta.normalize,
        "opset": meta.opset,
        "precision": meta.precision,
        "onnx_sha256": meta.onnx_sha256,
        "tokenizer_sha256": meta.tokenizer_sha256,
    }
    for key, value in required.items():
        if value is None or value == "" or value == 0:
            raise AssertionError(f"metadata field {key!r} must be present and non-null")
    # Prefixes may be null; if present must be non-empty strings.
    for key in ("query_prefix", "document_prefix"):
        value = getattr(meta, key)
        if value is not None and not isinstance(value, str):
            raise AssertionError(f"metadata field {key!r} must be str or null")


def export(model_key: str, out_dir: Path) -> ExportMetadata:
    if model_key not in MODEL_CONFIG:
        raise KeyError(f"unknown model key {model_key!r}; expected primary|fallback")
    cfg = MODEL_CONFIG[model_key]
    hf_id: str = cfg["hf_id"]
    pinned: str = cfg["revision"]
    revision = resolve_revision(hf_id, pinned)

    # Ensure weights are local before export (deliberate download, not at build).
    snapshot_download(repo_id=hf_id, revision=revision)

    model_out = out_dir / model_key
    if model_out.exists():
        shutil.rmtree(model_out)
    model_out.mkdir(parents=True)

    onnx_path = model_out / "model.onnx"
    _export_onnx(
        hf_id=hf_id,
        revision=revision,
        trust_remote_code=bool(cfg["trust_remote_code"]),
        out_onnx=onnx_path,
        max_sequence_length=int(cfg["max_sequence_length"]),
    )

    tok_paths = _capture_tokenizer(hf_id, revision, model_out)
    onnx_parts = [onnx_path]
    onnx_data = model_out / "model.onnx.data"
    if onnx_data.exists():
        onnx_parts.append(onnx_data)
    meta = ExportMetadata(
        model_id=f"{hf_id}@{revision}",
        revision=revision,
        dims=int(cfg["dims"]),
        max_sequence_length=int(cfg["max_sequence_length"]),
        pooling=str(cfg["pooling"]),
        normalize=str(cfg["normalize"]),
        query_prefix=cfg["query_prefix"],
        document_prefix=cfg["document_prefix"],
        opset=17,
        precision="fp16",
        onnx_sha256=_sha256_paths(onnx_parts),
        tokenizer_sha256=_sha256_paths(tok_paths),
    )
    assert_metadata_complete(meta)
    write_metadata(meta, model_out / "metadata.json")
    return meta


def _confirm_ort_loads(onnx_path: Path) -> None:
    import onnxruntime as ort

    session = ort.InferenceSession(str(onnx_path), providers=["CPUExecutionProvider"])
    inputs = {i.name: i for i in session.get_inputs()}
    if "input_ids" not in inputs or "attention_mask" not in inputs:
        raise RuntimeError(f"unexpected ONNX inputs: {list(inputs)}")
    outputs = [o.name for o in session.get_outputs()]
    if "last_hidden_state" not in outputs:
        raise RuntimeError(f"unexpected ONNX outputs: {outputs}")


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--model", required=True, choices=sorted(MODEL_CONFIG))
    parser.add_argument(
        "--out-dir",
        type=Path,
        default=DEFAULT_OUT,
        help="Directory that will contain <model>/ artifacts (default: models/)",
    )
    args = parser.parse_args(argv)

    try:
        meta = export(args.model, args.out_dir)
        onnx_path = args.out_dir / args.model / "model.onnx"
        _confirm_ort_loads(onnx_path)
    except Exception as exc:  # noqa: BLE001 — CLI surface
        print(f"export failed: {exc}", file=sys.stderr)
        return 1

    print(json.dumps(asdict(meta), indent=2))
    print(f"ORT load ok: {onnx_path}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
