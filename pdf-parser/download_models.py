from __future__ import annotations

import argparse
import json
import shutil
from pathlib import Path

from docling.datamodel.pipeline_options import LayoutOptions
from docling.models.stages.layout.layout_model import LayoutModel
from docling.models.stages.ocr.rapid_ocr_model import RapidOcrModel
from docling.models.stages.table_structure.table_structure_model import (
    TableStructureModel,
)


def main() -> int:
    parser = argparse.ArgumentParser(description="Download Agnes PDF parser models")
    parser.add_argument("--output-dir", required=True)
    args = parser.parse_args()
    output_dir = Path(args.output_dir).resolve()
    output_dir.mkdir(parents=True, exist_ok=True)
    LayoutModel.download_models(
        local_dir=output_dir / LayoutOptions().model_spec.model_repo_folder,
        progress=True,
    )
    TableStructureModel.download_models(
        local_dir=output_dir / TableStructureModel._model_repo_folder,
        progress=True,
    )
    for language in ("chinese", "english"):
        RapidOcrModel.download_models(
            backend="onnxruntime",
            lang=language,
            local_dir=output_dir / RapidOcrModel._model_repo_folder,
            progress=True,
        )
    shutil.rmtree(
        output_dir
        / TableStructureModel._model_repo_folder
        / "model_artifacts"
        / "tableformer"
        / "accurate",
        ignore_errors=True,
    )
    for cache_path in output_dir.rglob(".cache"):
        if cache_path.is_dir():
            shutil.rmtree(cache_path)
    manifest = {
        "schema_version": 1,
        "package_id": "docling-pdf-local",
        "package_version": "1",
        "docling_version": "2.113.0",
        "models": ["layout", "tableformer", "rapidocr"],
    }
    (output_dir / "agnes-models.json").write_text(
        json.dumps(manifest, ensure_ascii=False, indent=2) + "\n",
        encoding="utf-8",
    )
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
