#!/usr/bin/env python3
"""Verify an exported ONNX graph against the framework model and fixture."""

from __future__ import annotations

import argparse
import json
import sys
from dataclasses import asdict, dataclass
from pathlib import Path
from typing import Any

import numpy as np

sys.path.insert(0, str(Path(__file__).resolve().parent))

from export_model import MODEL_CONFIG, DEFAULT_OUT, resolve_revision

REPO_ROOT = Path(__file__).resolve().parents[1]
FIXTURE_INPUTS = Path(__file__).resolve().parent / "fixture_inputs.json"
FIXTURE_DIR = REPO_ROOT / "crates" / "inference" / "fixtures"


@dataclass
class VerifyReport:
    model_key: str
    model_id: str
    max_fp32_fp16_cosine_distance: float
    max_onnx_vs_fp32_cosine_distance: float
    per_case_spread: dict[str, float]
    per_case_onnx: dict[str, float]
    peak_vram_bytes: int | None
    passed: bool
    detail: str


def cosine_distance(a: np.ndarray, b: np.ndarray) -> float:
    a = a.astype(np.float64).reshape(-1)
    b = b.astype(np.float64).reshape(-1)
    denom = (np.linalg.norm(a) * np.linalg.norm(b))
    if denom == 0.0:
        return 0.0 if np.allclose(a, b) else 1.0
    return float(1.0 - np.dot(a, b) / denom)


def l2_normalize(v: np.ndarray) -> np.ndarray:
    n = np.linalg.norm(v.astype(np.float64))
    if n == 0.0:
        return v.astype(np.float32)
    return (v.astype(np.float64) / n).astype(np.float32)


def pool_hidden(
    hidden: np.ndarray, attention_mask: np.ndarray, pooling: str
) -> np.ndarray:
    """hidden: [seq, dims], attention_mask: [seq]."""
    mask = attention_mask.astype(bool)
    if pooling == "mean":
        if not mask.any():
            return hidden.mean(axis=0).astype(np.float32)
        return hidden[mask].mean(axis=0).astype(np.float32)
    if pooling == "last_token":
        # Last non-pad token (right-padded sequences).
        if not mask.any():
            return hidden[-1].astype(np.float32)
        idx = int(np.where(mask)[0][-1])
        return hidden[idx].astype(np.float32)
    if pooling == "cls":
        return hidden[0].astype(np.float32)
    raise ValueError(f"unknown pooling {pooling!r}")


def apply_prefix(text: str, role: str, cfg: dict[str, Any]) -> str:
    if role == "query" and cfg.get("query_prefix"):
        return str(cfg["query_prefix"]) + text
    if role == "document" and cfg.get("document_prefix"):
        return str(cfg["document_prefix"]) + text
    return text


def load_inputs() -> dict[str, Any]:
    return json.loads(FIXTURE_INPUTS.read_text())


def _encode_batch(
    tokenizer: Any,
    texts: list[str],
    max_length: int,
    pooling: str,
) -> dict[str, Any]:
    # last_token pooling needs left padding so the final position is content.
    if pooling == "last_token":
        tokenizer.padding_side = "left"
    else:
        tokenizer.padding_side = "right"
    return tokenizer(
        texts,
        return_tensors="pt",
        padding=True,
        truncation=True,
        max_length=max_length,
    )


def framework_embed_batch(
    model: Any,
    tokenizer: Any,
    texts: list[str],
    max_length: int,
    pooling: str,
    dtype_name: str,
) -> tuple[list[list[int]], list[bool], list[np.ndarray]]:
    import torch

    dtype = torch.float32 if dtype_name == "fp32" else torch.float16
    model = model.to(dtype=dtype)
    model.eval()
    encoded = _encode_batch(tokenizer, texts, max_length, pooling)
    input_ids = encoded["input_ids"]
    attention_mask = encoded["attention_mask"]
    with torch.inference_mode():
        out = model(input_ids=input_ids, attention_mask=attention_mask)
        hidden = out.last_hidden_state.detach().cpu().float().numpy()
    masks = attention_mask.cpu().numpy()
    raw_tokens: list[list[int]] = input_ids.cpu().tolist()
    tokens: list[list[int]] = []
    truncated: list[bool] = []
    vectors: list[np.ndarray] = []
    for i, text in enumerate(texts):
        # Pin the unpadded token sequence so tokenizer faults are distinguishable.
        unpadded = [t for t, m in zip(raw_tokens[i], masks[i]) if m]
        tokens.append(unpadded)
        # Without truncation, would the text exceed max_length?
        full = tokenizer(text, add_special_tokens=True, truncation=False)["input_ids"]
        truncated.append(len(full) > max_length)
        vec = pool_hidden(hidden[i], masks[i], pooling)
        vectors.append(l2_normalize(vec))
    return tokens, truncated, vectors


def onnx_embed_batch(
    session: Any,
    tokenizer: Any,
    texts: list[str],
    max_length: int,
    pooling: str,
) -> list[np.ndarray]:
    encoded = _encode_batch(tokenizer, texts, max_length, pooling)
    input_ids = encoded["input_ids"].cpu().numpy().astype(np.int64)
    attention_mask = encoded["attention_mask"].cpu().numpy().astype(np.int64)
    outputs = session.run(
        ["last_hidden_state"],
        {"input_ids": input_ids, "attention_mask": attention_mask},
    )
    hidden = outputs[0].astype(np.float32)
    vectors: list[np.ndarray] = []
    for i in range(len(texts)):
        vec = pool_hidden(hidden[i], attention_mask[i], pooling)
        vectors.append(l2_normalize(vec))
    return vectors


def emit_fixture(
    model_key: str,
    model_id: str,
    dims: int,
    tolerance_max: float,
    basis: str,
    cases: list[dict[str, Any]],
) -> Path:
    FIXTURE_DIR.mkdir(parents=True, exist_ok=True)
    path = FIXTURE_DIR / f"{model_key}-reference.json"
    payload = {
        "model_id": model_id,
        "dims": dims,
        "tolerance": {
            "metric": "cosine_distance",
            "max": tolerance_max,
            "basis": basis,
        },
        "cases": cases,
    }
    path.write_text(json.dumps(payload, indent=2, ensure_ascii=False) + "\n")
    return path


def verify(
    model_key: str,
    out_dir: Path,
    report_vram: bool,
    emit: bool,
    tolerance_override: float | None,
) -> VerifyReport:
    import torch
    from transformers import AutoModel, AutoTokenizer
    import onnxruntime as ort

    if model_key not in MODEL_CONFIG:
        raise KeyError(model_key)
    cfg = MODEL_CONFIG[model_key]
    hf_id = cfg["hf_id"]
    revision = resolve_revision(hf_id, cfg["revision"])
    model_id = f"{hf_id}@{revision}"
    max_length = int(cfg["max_sequence_length"])
    pooling = str(cfg["pooling"])
    dims = int(cfg["dims"])

    inputs = load_inputs()
    cases_in = inputs["cases"]
    texts = [apply_prefix(c["text"], c["role"], cfg) for c in cases_in]

    tokenizer = AutoTokenizer.from_pretrained(
        hf_id, revision=revision, trust_remote_code=bool(cfg["trust_remote_code"])
    )
    model = AutoModel.from_pretrained(
        hf_id, revision=revision, trust_remote_code=bool(cfg["trust_remote_code"])
    )

    peak_vram: int | None = None
    if report_vram and torch.cuda.is_available():
        torch.cuda.reset_peak_memory_stats()
        torch.cuda.empty_cache()
        model_cuda = model.to(device="cuda", dtype=torch.float16)
        model_cuda.eval()
        encoded = _encode_batch(tokenizer, texts, max_length, pooling)
        encoded = {k: v.to("cuda") for k, v in encoded.items()}
        with torch.inference_mode():
            _ = model_cuda(**encoded)
        peak_vram = int(torch.cuda.max_memory_allocated())
        model = model_cuda.to("cpu")
        del model_cuda
        torch.cuda.empty_cache()

    tokens_fp32, truncated, vecs_fp32 = framework_embed_batch(
        model, tokenizer, texts, max_length, pooling, "fp32"
    )
    _, _, vecs_fp16 = framework_embed_batch(
        model, tokenizer, texts, max_length, pooling, "fp16"
    )

    onnx_path = out_dir / model_key / "model.onnx"
    if not onnx_path.is_file():
        raise FileNotFoundError(f"missing export at {onnx_path}; run export_model.py first")
    session = ort.InferenceSession(str(onnx_path), providers=["CPUExecutionProvider"])
    vecs_onnx = onnx_embed_batch(session, tokenizer, texts, max_length, pooling)

    per_case_spread: dict[str, float] = {}
    per_case_onnx: dict[str, float] = {}
    fixture_cases: list[dict[str, Any]] = []
    for i, case in enumerate(cases_in):
        name = case["name"]
        spread = cosine_distance(vecs_fp32[i], vecs_fp16[i])
        onnx_dist = cosine_distance(vecs_fp32[i], vecs_onnx[i])
        per_case_spread[name] = spread
        per_case_onnx[name] = onnx_dist
        fixture_cases.append(
            {
                "name": name,
                "text": case["text"],
                "role": case["role"],
                "tokens": tokens_fp32[i],
                "truncated": truncated[i],
                "vector": [float(x) for x in vecs_fp32[i].tolist()],
            }
        )
        if len(fixture_cases[-1]["vector"]) != dims:
            raise AssertionError(
                f"vector width {len(fixture_cases[-1]['vector'])} != dims {dims}"
            )

    max_spread = max(per_case_spread.values()) if per_case_spread else 0.0
    max_onnx = max(per_case_onnx.values()) if per_case_onnx else 0.0
    # Small multiple of observed framework fp32-vs-fp16 spread, with a floor.
    chosen = tolerance_override if tolerance_override is not None else max(1e-4, max_spread * 5.0)
    basis = (
        f"Five times the observed max framework fp32-vs-fp16 cosine distance "
        f"({max_spread:.6g}) across fixture inputs, floored at 1e-4; "
        f"chosen max={chosen:.6g}."
    )

    if emit:
        emit_fixture(model_key, model_id, dims, chosen, basis, fixture_cases)

    fixture_path = FIXTURE_DIR / f"{model_key}-reference.json"
    passed = True
    detail_parts: list[str] = []
    if fixture_path.is_file():
        fixture = json.loads(fixture_path.read_text())
        tol = float(fixture["tolerance"]["max"])
        by_name = {c["name"]: c for c in fixture["cases"]}
        for i, case in enumerate(cases_in):
            ref = by_name[case["name"]]
            ref_vec = np.asarray(ref["vector"], dtype=np.float32)
            d_fp16 = cosine_distance(ref_vec, vecs_fp16[i])
            d_onnx = cosine_distance(ref_vec, vecs_onnx[i])
            if d_fp16 > tol or d_onnx > tol:
                passed = False
                detail_parts.append(
                    f"{case['name']}: fp16={d_fp16:.6g} onnx={d_onnx:.6g} tol={tol}"
                )
            # Token pin check (allow pad differences only if lengths match).
            if ref["tokens"] != tokens_fp32[i]:
                passed = False
                detail_parts.append(f"{case['name']}: token mismatch vs fixture")
    else:
        detail_parts.append("no committed fixture yet; emit with --emit-fixture")

    return VerifyReport(
        model_key=model_key,
        model_id=model_id,
        max_fp32_fp16_cosine_distance=max_spread,
        max_onnx_vs_fp32_cosine_distance=max_onnx,
        per_case_spread=per_case_spread,
        per_case_onnx=per_case_onnx,
        peak_vram_bytes=peak_vram,
        passed=passed,
        detail="; ".join(detail_parts) if detail_parts else "all cases within tolerance",
    )


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("--model", required=True, choices=sorted(MODEL_CONFIG))
    parser.add_argument("--out-dir", type=Path, default=DEFAULT_OUT)
    parser.add_argument("--report-vram", action="store_true")
    parser.add_argument(
        "--emit-fixture",
        action="store_true",
        help="Write crates/inference/fixtures/<model>-reference.json from framework fp32",
    )
    parser.add_argument(
        "--tolerance",
        type=float,
        default=None,
        help="Override tolerance when emitting a fixture",
    )
    args = parser.parse_args(argv)

    try:
        report = verify(
            args.model,
            args.out_dir,
            report_vram=args.report_vram,
            emit=args.emit_fixture,
            tolerance_override=args.tolerance,
        )
    except Exception as exc:  # noqa: BLE001
        print(f"verify failed: {exc}", file=sys.stderr)
        return 1

    print(json.dumps(asdict(report), indent=2))
    if report.peak_vram_bytes is not None:
        gb = report.peak_vram_bytes / (1024**3)
        print(f"peak_vram_gb={gb:.3f} (budget ~5.0 GB usable)")
    return 0 if report.passed or args.emit_fixture else 1


if __name__ == "__main__":
    raise SystemExit(main())
