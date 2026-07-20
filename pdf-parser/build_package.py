from __future__ import annotations

import argparse
import hashlib
import json
import os
import zipfile
from pathlib import Path


def file_hash(path: Path) -> str:
    digest = hashlib.sha256()
    with path.open("rb") as source:
        while chunk := source.read(1024 * 1024):
            digest.update(chunk)
    return digest.hexdigest()


def include_model_file(path: Path, models_path: Path) -> bool:
    relative = path.relative_to(models_path)
    if ".cache" in relative.parts:
        return False
    return relative.parts[:4] != (
        "docling-project--docling-models",
        "model_artifacts",
        "tableformer",
        "accurate",
    )


def main() -> int:
    parser = argparse.ArgumentParser(description="Build an Agnes PDF model package")
    parser.add_argument("--parser", required=True)
    parser.add_argument("--models", required=True)
    parser.add_argument("--target", required=True)
    parser.add_argument("--output", required=True)
    args = parser.parse_args()
    parser_path = Path(args.parser).resolve()
    models_path = Path(args.models).resolve()
    output_path = Path(args.output).resolve()
    if not parser_path.is_file():
        raise SystemExit(f"Parser binary does not exist: {parser_path}")
    if not (models_path / "agnes-models.json").is_file():
        raise SystemExit(f"Model manifest does not exist: {models_path}")

    parser_name = "docling-pdf-parserd.exe" if os.name == "nt" else "docling-pdf-parserd"
    sources: list[tuple[Path, str]] = [(parser_path, f"bin/{parser_name}")]
    sources.extend(
        (path, f"models/{path.relative_to(models_path).as_posix()}")
        for path in sorted(models_path.rglob("*"))
        if path.is_file() and include_model_file(path, models_path)
    )
    files = [
        {
            "path": archive_path,
            "size": source.stat().st_size,
            "sha256": file_hash(source),
        }
        for source, archive_path in sources
    ]
    manifest = {
        "schema_version": 1,
        "package_id": "docling-pdf-local",
        "package_version": "1",
        "docling_version": "2.113.0",
        "target": args.target,
        "parser": f"bin/{parser_name}",
        "models_dir": "models",
        "files": files,
    }
    output_path.parent.mkdir(parents=True, exist_ok=True)
    with zipfile.ZipFile(output_path, "w", allowZip64=True) as archive:
        archive.writestr(
            "agnes-pdf-model-package.json",
            json.dumps(manifest, ensure_ascii=False, indent=2) + "\n",
            compress_type=zipfile.ZIP_DEFLATED,
        )
        for source, archive_path in sources:
            archive.write(source, archive_path, compress_type=zipfile.ZIP_STORED)
    checksum = file_hash(output_path)
    output_path.with_suffix(output_path.suffix + ".sha256").write_text(
        f"{checksum}  {output_path.name}\n",
        encoding="utf-8",
    )
    print(output_path)
    print(f"sha256={checksum}")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
