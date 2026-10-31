#!/usr/bin/env python3
"""Judge a PDF → DOCX conversion by structure, not by "a file appeared".

    ./scripts/docx-quality.py --source report.pdf --docx out.docx [--reference ref.docx]

Checks, all of them measured from the package itself:

* text coverage — every alphabetic word of the source's text layer is in the document
  (≥ 0.98). A heading-only skeleton fails this.
* structure — at least one Heading 1, two Heading 2s, ten list paragraphs, ten
  `w:numPr` list markers, four drawings, zero page breaks, and no leftover bullet
  glyphs in the text (they belong to the list style, not the words).
* with `--reference`: Heading 1 and Heading 2 within ±1 of the reference, and list
  paragraphs at least 80 % of the reference's.

Exit 1 and print the JSON report on any failure. Every threshold is the contract; a
conversion that misses one is a bug in the sidecar, not a reason to lower the number.
"""
from __future__ import annotations

import argparse
import json
import subprocess
import sys
from pathlib import Path

sys.path.insert(0, str(Path(__file__).resolve().parent))
import validate  # noqa: E402


def source_text(pdf: str) -> str:
    result = subprocess.run(
        ["pdftotext", "-layout", pdf, "-"], capture_output=True, text=True, check=False
    )
    return result.stdout if result.returncode == 0 else ""


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--source", required=True, help="the PDF that was converted")
    parser.add_argument("--docx", required=True, help="the produced DOCX")
    parser.add_argument("--reference", help="the hand-made DOCX to compare against")
    parser.add_argument("--json", dest="json_path", help="write the report here")
    args = parser.parse_args(argv)

    facts = validate.docx(args.docx)
    if facts is None:
        print(f"docx-quality: {args.docx} is not a readable DOCX package", file=sys.stderr)
        return 1

    coverage = validate.text_coverage(source_text(args.source), facts["text"])
    checks = [
        ("text coverage ≥ 0.98", coverage is not None and coverage >= 0.98, coverage),
        ("Heading 1 ≥ 1", facts["heading1"] >= 1, facts["heading1"]),
        ("Heading 2 ≥ 2", facts["heading2"] >= 2, facts["heading2"]),
        ("list paragraphs ≥ 10", facts["lists"] >= 10, facts["lists"]),
        ("list markers (numPr) ≥ 10", facts["numpr"] >= 10, facts["numpr"]),
        ("embedded images ≥ 4", facts["images"] >= 4, facts["images"]),
        ("page breaks == 0", facts["page_breaks"] == 0, facts["page_breaks"]),
        ("no bullet glyph left in text", "•" not in facts["text"], "•" in facts["text"]),
    ]

    reference = None
    if args.reference:
        reference = validate.docx(args.reference)
        if reference is None:
            print(f"docx-quality: {args.reference} is not readable", file=sys.stderr)
            return 1
        checks.extend(
            [
                (
                    "Heading 1 within ±1 of the reference",
                    abs(facts["heading1"] - reference["heading1"]) <= 1,
                    [facts["heading1"], reference["heading1"]],
                ),
                (
                    "Heading 2 within ±1 of the reference",
                    abs(facts["heading2"] - reference["heading2"]) <= 1,
                    [facts["heading2"], reference["heading2"]],
                ),
                (
                    "list paragraphs ≥ 80 % of the reference",
                    facts["lists"] >= reference["lists"] * 0.8,
                    [facts["lists"], reference["lists"]],
                ),
            ]
        )

    report = {
        "source": args.source,
        "docx": args.docx,
        "reference": args.reference,
        "counts": {key: facts[key] for key in
                   ("paragraphs", "heading1", "heading2", "heading3", "lists", "numpr",
                    "images", "page_breaks", "bytes")},
        "media": facts["media"],
        "coverage": coverage,
        "checks": [
            {"name": name, "ok": bool(ok), "actual": actual} for name, ok, actual in checks
        ],
        "ok": all(ok for _name, ok, _actual in checks),
    }

    for name, ok, actual in checks:
        print(f"{'ok  ' if ok else 'FAIL'} {name}: {actual}")
    if args.json_path:
        Path(args.json_path).write_text(json.dumps(report, indent=2) + "\n")
    return 0 if report["ok"] else 1


if __name__ == "__main__":
    raise SystemExit(main())
