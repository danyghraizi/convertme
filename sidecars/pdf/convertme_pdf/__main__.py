"""ConvertMe PDF sidecar: the PDF targets PyMuPDF does and Rust does not.

`convertme-pdf <docx|pptx|xlsx|epub|md> <input.pdf> <output>`

Every subcommand writes exactly one file at `<output>` and prints nothing on
success. Failures print a one-line reason to stderr and exit non-zero, which the
Rust side surfaces as `engine_failed` with the stderr tail.
"""

from __future__ import annotations

import sys

from . import docx as docx_mod
from . import epub as epub_mod
from . import md as md_mod
from . import pptx as pptx_mod
from . import xlsx as xlsx_mod

VERSION = "0.1.0"

COMMANDS = {
    "docx": docx_mod.convert,
    "pptx": pptx_mod.convert,
    "xlsx": xlsx_mod.convert,
    "epub": epub_mod.convert,
    "md": md_mod.convert,
}


def main(argv: list[str]) -> int:
    if not argv:
        print(__doc__, file=sys.stderr)
        return 2
    if argv[0] in ("--version", "-V"):
        print(f"convertme-pdf {VERSION}")
        return 0
    if argv[0] in ("--help", "-h"):
        print(__doc__)
        return 0

    command = argv[0]
    handler = COMMANDS.get(command)
    if handler is None:
        print(f"unknown subcommand {command!r}", file=sys.stderr)
        return 2
    if len(argv) != 3:
        print(f"{command} needs an input PDF and an output path", file=sys.stderr)
        return 2

    try:
        handler(argv[1], argv[2])
    except Exception as error:  # noqa: BLE001 - the message is the contract
        print(f"{command}: {error}", file=sys.stderr)
        return 1
    return 0


if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
