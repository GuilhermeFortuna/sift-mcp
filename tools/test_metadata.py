#!/usr/bin/env python3
"""Assert export metadata fields are complete (prefixes may be null)."""

from __future__ import annotations

import json
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))

from export_model import ExportMetadata, assert_metadata_complete, write_metadata


def _complete() -> ExportMetadata:
    return ExportMetadata(
        model_id="org/model@abc",
        revision="abc",
        dims=768,
        max_sequence_length=512,
        pooling="mean",
        normalize="l2",
        query_prefix=None,
        document_prefix=None,
        opset=17,
        precision="fp16",
        onnx_sha256="0" * 64,
        tokenizer_sha256="1" * 64,
    )


def main() -> int:
    meta = _complete()
    assert_metadata_complete(meta)

    incomplete = _complete()
    incomplete.onnx_sha256 = ""
    try:
        assert_metadata_complete(incomplete)
    except AssertionError:
        pass
    else:
        raise AssertionError("expected incomplete metadata to fail")

    # Live artifact, when present after a local export.
    live = Path(__file__).resolve().parents[1] / "models" / "fallback" / "metadata.json"
    if live.is_file():
        data = json.loads(live.read_text())
        live_meta = ExportMetadata(**data)
        assert_metadata_complete(live_meta)
        print(f"live metadata ok: {live}")

    tmp = Path("/tmp/sift-004-metadata-roundtrip.json")
    write_metadata(meta, tmp)
    roundtrip = ExportMetadata(**json.loads(tmp.read_text()))
    assert_metadata_complete(roundtrip)
    print("metadata assertion ok")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
