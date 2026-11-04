#!/usr/bin/env bash
# Build the PDF sidecar; an optional argument selects its output directory.
set -euo pipefail
here="$(cd "$(dirname "$0")" && pwd)"
exec "${PYTHON:-python3}" "$here/build.py" "$@"
