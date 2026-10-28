"""PyInstaller entry point: `convertme-pdf` on the command line."""

import sys

from convertme_pdf.__main__ import main

if __name__ == "__main__":
    sys.exit(main(sys.argv[1:]))
