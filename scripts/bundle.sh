#!/usr/bin/env bash
# Build Linux packages on Ubuntu 24.04 x86_64.
# Usage: ./scripts/bundle.sh [deb,rpm,appimage] [tauri build options...]
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
bundles="${1:-deb}"
shift || true

python3 "$root/scripts/vendor-linux.py" --check-host
IFS=',' read -r -a selections <<< "$bundles"
declare -A seen=()
[[ -n "$bundles" && "$bundles" != ,* && "$bundles" != *, && "$bundles" != *,,* ]] || {
  echo "empty Linux bundle selection: $bundles" >&2; exit 2;
}
for format in "${selections[@]}"; do
  case "$format" in deb|rpm|appimage) ;; *) echo "unsupported Linux bundle: $format" >&2; exit 2;; esac
  [[ -z "${seen[$format]+present}" ]] || { echo "duplicate Linux bundle: $format" >&2; exit 2; }
  seen[$format]=1
done
if [[ -n "${seen[appimage]+present}" ]]; then
  echo 'AppImage packaging is blocked until the owned AppDir and WebKit closure are available' >&2
  exit 2
fi

work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT
# Building on the release base is essential: an existing sidecar may have been
# compiled against a newer host's libc even when its file name is correct.
"$root/sidecars/pdf/build.sh" "$work/pdf"
python3 "$root/scripts/vendor-linux.py" --sidecar "$work/pdf/convertme-pdf"
python3 "$root/scripts/linux-dependencies.py" "$bundles" "$work/linux-conf.json"

# The generated base config owns the resources mappings; the override supplies
# dependency requirements computed from this build's staged ELF executables.
(cd "$root" && cargo xtask gen-conf --target x86_64-unknown-linux-gnu >/dev/null)
export CONVERTME_PACKAGE_BUILD=1
cd "$root/ui"
[ -d node_modules ] || npm ci --offline --no-audit --no-fund
cd "$root/crates/convertme-gui"
"$root/ui/node_modules/.bin/tauri" build --bundles "$bundles" --config "$work/linux-conf.json" "$@"
