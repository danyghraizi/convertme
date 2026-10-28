#!/usr/bin/env bash
# Builds the installers with the Tauri CLI, from the crate that owns the config.
#
#   ./scripts/bundle.sh [deb,rpm,appimage] [--sign]
#
# The CLI resolves `tauri.conf.json` from its working directory, and every path in
# that config is relative to `crates/convertme-gui`, so it must run from there.
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
bundles="${1:-deb,appimage}"
shift || true

if [ -z "$(ls -A "$root/vendor/bin" 2>/dev/null)" ]; then
  echo "vendor/bin is empty: run \`cargo xtask vendor\` first" >&2
  exit 2
fi
triple="$(rustc --print host-tuple)"
if [ ! -x "$root/vendor/bin/convertme-pdf-$triple" ]; then
  echo "the PDF sidecar is missing: run ./sidecars/pdf/build.sh and copy it into vendor/bin" >&2
  exit 2
fi

cd "$root/ui"
[ -d node_modules ] || npm install --no-audit --no-fund
cd "$root/crates/convertme-gui"
exec "$root/ui/node_modules/.bin/tauri" build --bundles "$bundles" "$@"
