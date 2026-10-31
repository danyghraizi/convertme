#!/usr/bin/env bash
# Everything CI does that can be reproduced on this machine, in the order CI runs it.
#
#   ./scripts/preflight.sh          # gate, fmt, clippy, tests, UI, clean-checkout build
#   ./scripts/preflight.sh --full   # … plus the bundles and the wheel budget
#
# Not covered, because the machine cannot: the Windows compile (no cross C toolchain
# for the bundled SQLite), the container-only perf baseline, and the actions
# themselves. Everything else here has failed a CI run at least once, so it runs
# before a push rather than after.
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
cd "$root"

full=0
[ "${1:-}" = "--full" ] && full=1

export PATH="$HOME/.cargo/bin:$PATH"
export DEBRAND_TOKENS_FILE="${DEBRAND_TOKENS_FILE:-$HOME/.config/convertme/forbidden.txt}"
if [ ! -f "$DEBRAND_TOKENS_FILE" ]; then
  echo "preflight: set DEBRAND_TOKENS_FILE (one lowercase token per line)" >&2
  exit 2
fi

step() { printf '\n== %s\n' "$*"; }

step "identity gate"
python3 scripts/debrand.py --all

step "format"
cargo fmt --all --check

step "front end"
npm --prefix ui ci --no-audit --no-fund >/dev/null
# The mock renders the backend's field table, written to ui/src/fields.generated.json.
# A stale file means the browser harness would render a different dialog from the app.
python3 scripts/gen-ui-fields.py --check
npm --prefix ui run typecheck
npm --prefix ui run test:geometry
npm --prefix ui run test:fields
npm --prefix ui run test:format
npm --prefix ui run build

step "clippy"
cargo clippy --workspace --all-targets -- -D warnings

step "tests"
cargo test --workspace

step "clean checkout: no vendored tools"
# A fresh clone has no vendor/bin. A declared external binary that is absent stops
# the Tauri build script, so the tree must compile without the tools.
mv vendor/bin vendor/bin.preflight-moved
rm -f crates/convertme-gui/tauri.conf.json
restore_vendor() {
  if [ -d vendor/bin.preflight-moved ]; then mv vendor/bin.preflight-moved vendor/bin; fi
}
trap restore_vendor EXIT
cargo check -q -p convertme-gui --tests
restore_vendor
trap - EXIT

step "clean checkout: no generated config"
# The bundler must generate tauri.conf.json itself; a previous build used to leave one
# behind and hide that it did not.
rm -f crates/convertme-gui/tauri.conf.json
./scripts/bundle.sh deb

if [ "$full" = 1 ]; then
  step "bundles"
  DEBRAND_TOKENS_FILE="$DEBRAND_TOKENS_FILE" ./scripts/bundle.sh deb,appimage
  python3 scripts/debrand.py --all

  step "wheel budget"
  cargo xtask perf wheel --runs 20
fi

step "preflight ok"
