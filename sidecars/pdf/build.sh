#!/usr/bin/env bash
# Builds the `convertme-pdf` sidecar as a single executable.
#
#   ./build.sh [output-dir]
#
# The result is `<output-dir>/convertme-pdf` (`.exe` on Windows); `cargo xtask
# vendor` copies it into `vendor/bin/convertme-pdf-<target-triple>` so Tauri's
# `externalBin` picks it up.
set -euo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
out="${1:-$here/dist}"
python="${PYTHON:-python3}"

cd "$here"
"$python" -m pip install --quiet --upgrade pyinstaller pymupdf python-docx python-pptx openpyxl

rm -rf "$here/build" "$here/dist"
"$python" -m PyInstaller \
  --noconfirm \
  --clean \
  --distpath "$out" \
  --workpath "$here/build" \
  "$here/convertme-pdf.spec"

echo "sidecar at $out/convertme-pdf"
"$out/convertme-pdf" --version
