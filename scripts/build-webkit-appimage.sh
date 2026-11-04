#!/usr/bin/env bash
# Rebuild the locked Ubuntu WebKit source package with the AppImage sandbox patch.
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
inputs="${CONVERTME_LINUX_INPUTS:-$root/vendor/linux/inputs}"
if [[ $# != 2 || "$1" != --out ]]; then
  echo 'usage: build-webkit-appimage.sh --out <directory>' >&2; exit 2
fi
out="$(realpath -m "$2")"
python3 "$root/scripts/vendor-linux.py" --check-host
mkdir -p "$(dirname "$out")"
work="$(mktemp -d "$(dirname "$out")/.webkit-build.XXXXXXXX")"
trap 'rm -rf "$work"' EXIT
mkdir "$work/inputs"
python3 - "$root/packaging/linux/build.lock.json" "$inputs/webkit" "$work/inputs" <<'PY'
import hashlib, json, pathlib, re, shutil, subprocess, sys
lock = json.loads(pathlib.Path(sys.argv[1]).read_text())['webkit']
inputs = pathlib.Path(sys.argv[2])
if lock['version'] != '2.52.6-0ubuntu0.24.04.1':
    raise SystemExit('unexpected WebKit source version')
files = lock['source_files']
if not files or not any(name.endswith('.dsc') for name in files):
    raise SystemExit('WebKit source package lock is incomplete')
for name, expected in files.items():
    if pathlib.PurePosixPath(name).name != name or not re.fullmatch('[0-9a-f]{64}', expected):
        raise SystemExit(f'unsafe WebKit source lock entry: {name}')
    with (inputs / name).open('rb') as f:
        if hashlib.file_digest(f, 'sha256').hexdigest() != expected:
            raise SystemExit(f'WebKit source hash mismatch: {name}')
    shutil.copyfile(inputs / name, pathlib.Path(sys.argv[3]) / name)
if not lock['build_dependencies']:
    raise SystemExit('WebKit build dependency lock is empty')
for name, expected in lock['build_dependencies'].items():
    actual = subprocess.check_output(['dpkg-query', '-W', '-f=${Version}', name], text=True)
    if actual != expected:
        raise SystemExit(f'WebKit build dependency {name}: expected {expected}, found {actual}')
PY
mapfile -t dscs < <(find "$work/inputs" -maxdepth 1 -type f -name '*.dsc' -print)
[[ ${#dscs[@]} == 1 ]] || { echo 'exactly one locked WebKit .dsc is required' >&2; exit 1; }
dpkg-source -x "${dscs[0]}" "$work/source"
patch -d "$work/source" -p1 --forward < "$root/packaging/linux/patches/webkit-production-process-path.patch"
(cd "$work/source" && DEB_BUILD_OPTIONS="${DEB_BUILD_OPTIONS:-} nocheck" dpkg-buildpackage -b -uc -us)
mkdir "$work/extracted" "$work/result"
shopt -s nullglob
packages=("$work"/*.deb)
(( ${#packages[@]} > 0 )) || { echo 'WebKit build produced no binary packages' >&2; exit 1; }
for package in "${packages[@]}"; do dpkg-deb -x "$package" "$work/extracted"; done
python3 - "$work/extracted" "$work/result" "$root/packaging/linux/patches/webkit-production-process-path.patch" <<'PY'
import hashlib, json, pathlib, shutil, sys
source, out, patch = map(pathlib.Path, sys.argv[1:])
lib = source / 'usr/lib/x86_64-linux-gnu'
process = lib / 'webkit2gtk-4.1'
records = []
def copy(path, rel):
    if not path.is_file():
        raise SystemExit(f'missing WebKit output: {path}')
    target = out / rel
    target.parent.mkdir(parents=True, exist_ok=True)
    shutil.copy2(path, target, follow_symlinks=True)
    with target.open('rb') as f:
        sha = hashlib.file_digest(f, 'sha256').hexdigest()
    records.append({'path': rel.as_posix(), 'sha256': sha})
for pattern in ('libwebkit2gtk-4.1.so*', 'libjavascriptcoregtk-4.1.so*'):
    paths = sorted(lib.glob(pattern))
    if not paths:
        raise SystemExit(f'missing WebKit output {pattern}')
    for path in paths:
        copy(path, pathlib.PurePosixPath('usr/lib/convertme/gui') / path.name)
for name in ('WebKitWebProcess', 'WebKitNetworkProcess'):
    copy(process / name, pathlib.PurePosixPath('usr/lib/x86_64-linux-gnu/webkit2gtk-4.1') / name)
gpu = process / 'WebKitGPUProcess'
if gpu.is_file():
    copy(gpu, pathlib.PurePosixPath('usr/lib/x86_64-linux-gnu/webkit2gtk-4.1') / gpu.name)
bundle = process / 'injected-bundle'
files = sorted(p for p in bundle.rglob('*') if p.is_file())
if not files:
    raise SystemExit('missing WebKit injected bundle')
for path in files:
    copy(path, pathlib.PurePosixPath('usr/lib/x86_64-linux-gnu/webkit2gtk-4.1/injected-bundle') / path.relative_to(bundle).as_posix())
with patch.open('rb') as f:
    patch_sha = hashlib.file_digest(f, 'sha256').hexdigest()
(out / 'manifest.json').write_text(json.dumps({'version': '2.52.6-0ubuntu0.24.04.1', 'patch_sha256': patch_sha, 'files': records}, indent=2) + '\n')
PY
if [[ -e "$out" ]]; then mv "$out" "$work/previous"; fi
if ! mv "$work/result" "$out"; then
  [[ ! -e "$work/previous" ]] || mv "$work/previous" "$out"
  exit 1
fi
