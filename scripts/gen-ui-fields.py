#!/usr/bin/env python3
"""Write the mock's field tables from the backend's own table.

`ui/src/mock.ts` renders the dialogs in the browser harness, and it used to keep a
hand-written copy of every tool's fields. That copy drifted: it showed a `boxes`
control the real crop dialog hides, and offered labels the backend does not have.
This script asks the binary for the table (`convertme probe --fields`) and writes
`ui/src/fields.generated.json`, so both modes render one table.

    ./scripts/gen-ui-fields.py            # rewrite the file
    ./scripts/gen-ui-fields.py --check    # fail if it is stale

The in-tree path is `UPDATE_FIELDS=1 cargo test -p convertme-gui generated_fields`,
which regenerates from the code being compiled (no release build needed). This script
is for CI and for a machine that already has a fresh binary. Either way, run
`npm --prefix ui run build` afterwards: the file lives under `ui/src`.
"""
from __future__ import annotations

import json
import subprocess
import sys
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
BIN = REPO / "target/release/convertme"
OUT = REPO / "ui/src/fields.generated.json"


def main(argv: list[str]) -> int:
    check = "--check" in argv
    if not BIN.is_file():
        print(f"gen-ui-fields: build the release binary first ({BIN})", file=sys.stderr)
        return 2
    result = subprocess.run(
        [str(BIN), "probe", "--fields"], capture_output=True, text=True, check=False
    )
    if result.returncode != 0:
        print(f"gen-ui-fields: probe failed: {result.stderr.strip()}", file=sys.stderr)
        return 2
    table = json.loads(result.stdout)
    text = json.dumps(table, indent=2, sort_keys=True) + "\n"
    if check:
        current = OUT.read_text() if OUT.exists() else ""
        if current != text:
            print("gen-ui-fields: ui/src/fields.generated.json is stale", file=sys.stderr)
            return 1
        print("gen-ui-fields: up to date")
        return 0
    OUT.write_text(text)
    print(f"wrote {OUT.relative_to(REPO)}")
    print("now run: npm --prefix ui run build   (the file is under ui/src)")
    return 0


if __name__ == "__main__":
    raise SystemExit(main(sys.argv[1:]))
