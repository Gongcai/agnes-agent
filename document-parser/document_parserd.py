from __future__ import annotations

import argparse
import json
import sys
import zipfile

from app.parser import DocumentParseError, parse_document


def emit(payload: dict[str, object]) -> None:
    print(
        json.dumps(payload, ensure_ascii=False, separators=(",", ":")),
        flush=True,
    )


def main() -> int:
    parser = argparse.ArgumentParser(description="Parse an Office document for Agnes")
    parser.add_argument("--path", required=True)
    parser.add_argument("--title")
    parser.add_argument("--media-type")
    args = parser.parse_args()
    try:
        result = parse_document(
            args.path,
            args.title,
            args.media_type,
            progress=lambda stage, percent, message: emit(
                {
                    "type": "progress",
                    "stage": stage,
                    "percent": percent,
                    "message": message,
                }
            ),
        )
    except (DocumentParseError, OSError, ValueError, zipfile.BadZipFile) as error:
        emit({"type": "error", "error": str(error)})
        return 2
    except Exception as error:
        emit({"type": "error", "error": f"文档解析失败：{error}"})
        return 1
    emit({"type": "result", "payload": result})
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
