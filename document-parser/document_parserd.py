from __future__ import annotations

import argparse
import json
import sys
import zipfile

from app.parser import DocumentParseError, parse_document


def main() -> int:
    parser = argparse.ArgumentParser(description="Parse an Office document for Agnes")
    parser.add_argument("--path", required=True)
    parser.add_argument("--title")
    parser.add_argument("--media-type")
    args = parser.parse_args()
    try:
        result = parse_document(args.path, args.title, args.media_type)
    except (DocumentParseError, OSError, ValueError, zipfile.BadZipFile) as error:
        print(json.dumps({"error": str(error)}, ensure_ascii=False))
        return 2
    except Exception as error:
        print(json.dumps({"error": f"文档解析失败：{error}"}, ensure_ascii=False))
        return 1
    print(json.dumps(result, ensure_ascii=False, separators=(",", ":")))
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
