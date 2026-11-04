#!/usr/bin/env bash
# Connected capture and offline Linux release replay. Never uses the system daemon.
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
lock="$root/packaging/linux/build.lock.json"
inputs="$root/vendor/linux/inputs"
if [[ "${1:-}" == --inside ]]; then
  [[ $# == 4 ]] || exit 2
  checkout="$2"; input_root="$3"; output_root="$4"
  work_base="${CONVERTME_WORK_BASE:-/work}"
  mkdir -p "$work_base/convertme"
  tar -C "$checkout" --exclude='./.git' --exclude='./target' --exclude='./vendor' \
    --exclude='./artifacts' --exclude='./ui/node_modules' --exclude='./crates/convertme-gui/ui-dist' \
    --exclude='./sidecars/pdf/build' --exclude='./sidecars/pdf/dist' --exclude='./.node' \
    --exclude='./fixtures' --exclude='./.env' --exclude='./.env.*' -cf - . | tar -C "$work_base/convertme" -xf -
  cd "$work_base/convertme"
  python3 scripts/vendor-linux.py --check-host
  python3 - "$input_root" <<'PY'
import json, pathlib, subprocess, sys
lock = json.loads(pathlib.Path('packaging/linux/build.lock.json').read_text())
for name, package in sorted(lock['ubuntu']['packages'].items()):
    got = subprocess.check_output(['dpkg-query', '-W', '-f=${Version}\\t${Architecture}', name], text=True).strip()
    expected = package['version'] + '\t' + package['architecture']
    if got != expected: raise SystemExit(f'Ubuntu package {name}: expected {expected}, got {got}')
for command, key in ((['rustc', '--version'], 'rust'), (['node', '--version'], 'node'), (['python3', '--version'], 'python')):
    actual = subprocess.check_output(command, text=True).strip()
    expected = lock['toolchains'][key]['version']
    if expected not in actual: raise SystemExit(f'{key}: expected {expected}, got {actual}')
PY
  export CARGO_HOME="$work_base/cargo" NPM_CONFIG_CACHE="$work_base/npm" HOME="$work_base/home"
  mkdir -p "$CARGO_HOME" "$NPM_CONFIG_CACHE" "$HOME"
  cp -a "$input_root/cargo/." "$CARGO_HOME/"
  cp -a "$input_root/npm/." "$NPM_CONFIG_CACHE/"
  export CARGO_NET_OFFLINE=true NPM_CONFIG_OFFLINE=true PIP_NO_INDEX=1
  export CONVERTME_PDF_WHEELHOUSE="$input_root/python/linux"
  export CONVERTME_LINUX_TOOLS_CACHE="$input_root/tools"
  export CONVERTME_LINUX_INPUTS="$input_root"
  npm --prefix ui ci --offline --no-audit --no-fund
  npm --prefix ui run build
  # AppImage packaging is intentionally blocked until its portable ELF and data
  # closure is proven. Keep the release replay aligned with bundle.sh and README.
  scripts/bundle.sh deb,rpm
  python3 - "$output_root" <<'PY'
import pathlib, shutil, sys
root = pathlib.Path('target/release/bundle')
dest = pathlib.Path(sys.argv[1])
for suffix in ('deb', 'rpm'):
    found = list(root.rglob('*.' + suffix))
    if len(found) != 1: raise SystemExit(f'expected one built {suffix}, found {len(found)}')
    shutil.copyfile(found[0], dest / ('ConvertMe-0.1.1-linux-x64.' + suffix))
PY
  python3 scripts/release-materials.py --target x86_64-unknown-linux-gnu \
    --payload vendor/linux/x86_64-unknown-linux-gnu/tools --inputs "$input_root/sources" --out "$work_base/materials"
  python3 scripts/release-inventory.py create --target x86_64-unknown-linux-gnu \
    --artifacts "$output_root" --payload vendor/linux/x86_64-unknown-linux-gnu/tools --compliance "$work_base/materials"
  exit
fi
out=''
mode=build
native=0
usage() { echo 'usage: build-linux-release.sh --capture-inputs --inputs DIR | [--inputs DIR] --out DIR [--native]' >&2; exit 2; }
while (( $# )); do
  case "$1" in
    --capture-inputs) [[ "$mode" == build ]] || usage; mode=capture; shift ;;
    --inputs) (( $# >= 2 )) || usage; inputs="$2"; shift 2 ;;
    --out) (( $# >= 2 )) || usage; out="$2"; shift 2 ;;
    --native) native=1; shift ;;
    *) usage ;;
  esac
done
inputs="$(realpath -m "$inputs")"
if [[ "$mode" == capture ]]; then
  [[ -z "$out" && "$native" == 0 ]] || usage
  mkdir -p "$inputs/docker"
  command -v curl >/dev/null || { echo 'capture requires curl' >&2; exit 1; }
  tmp="$(mktemp -d "$inputs/docker/.capture.XXXXXXXX")"
  trap 'rm -rf "$tmp"' EXIT
  for name in docker-28.5.2.tgz docker-rootless-extras-28.5.2.tgz; do
    curl --fail --location --proto '=https' --tlsv1.2 --output "$tmp/$name" "https://download.docker.com/linux/static/stable/x86_64/$name"
  done
  python3 - "$lock" "$inputs/docker" "$tmp" <<'PY'
import hashlib, json, os, pathlib, sys, tempfile
lock, dest, tmp = map(pathlib.Path, sys.argv[1:])
data = json.loads(lock.read_text())
records = data.setdefault('docker', {}).setdefault('archives', {})
names = ('docker-28.5.2.tgz', 'docker-rootless-extras-28.5.2.tgz')
measured = {}
for name in names:
    with (tmp / name).open('rb') as f:
        measured[name] = hashlib.file_digest(f, 'sha256').hexdigest()
    if name in records and records[name] != measured[name]:
        raise SystemExit(f'existing Docker pin differs from downloaded {name}; refusing to repin')
    if (dest / name).exists():
        with (dest / name).open('rb') as f:
            if hashlib.file_digest(f, 'sha256').hexdigest() != measured[name]:
                raise SystemExit(f'existing Docker input differs: {dest / name}')
for name in names:
    os.replace(tmp / name, dest / name)
records.update(measured)
fd, path = tempfile.mkstemp(prefix='.build.lock.', dir=lock.parent)
try:
    with os.fdopen(fd, 'w') as f:
        json.dump(data, f, indent=2, sort_keys=True)
        f.write('\n')
    os.replace(path, lock)
finally:
    if os.path.exists(path): os.unlink(path)
PY
  python3 "$root/scripts/prepare-linux-pdf-inputs.py" --capture --out "$inputs/python/linux"
  python3 - "$root/packaging/linux/tools.lock.json" "$lock" "$inputs" <<'PY'
import hashlib, json, os, pathlib, sys, tempfile, urllib.request
tools_lock, lock_path, inputs = map(pathlib.Path, sys.argv[1:])
lock = json.loads(lock_path.read_text())
tools = json.loads(tools_lock.read_text())
def acquire(url, path, expected=None, headers=None):
    path.parent.mkdir(parents=True, exist_ok=True)
    fd, name = tempfile.mkstemp(prefix='.capture-', dir=path.parent)
    try:
        digest = hashlib.sha256()
        with os.fdopen(fd, 'wb') as output, urllib.request.urlopen(urllib.request.Request(url, headers=headers or {})) as response:
            if not response.geturl().startswith('https://'): raise ValueError(f'insecure redirect: {url}')
            while chunk := response.read(1024 * 1024):
                digest.update(chunk)
                output.write(chunk)
        actual = digest.hexdigest()
        if expected and actual != expected: raise ValueError(f'hash mismatch for {url}: {actual}')
        if path.exists():
            with path.open('rb') as old:
                if hashlib.file_digest(old, 'sha256').hexdigest() != actual:
                    raise ValueError(f'existing input differs: {path}')
        os.replace(name, path)
        return actual
    finally:
        if os.path.exists(name): os.unlink(name)
for component in ('qpdf', 'imagemagick'):
    entry = tools[component]
    acquire(entry['url'], inputs / 'tools' / entry['sha256'], entry['sha256'])
assets = (
    ('313839329', 'linuxdeploy-x86_64.AppImage', 'c20cd71e3a4e3b80c3483cef793cda3f4e990aca14014d23c544ca3ce1270b4d'),
    ('699466557', 'appimagetool-x86_64.AppImage', '62aebc77c2149e96b593b94da9bbd1881e0984a56399ff6d7a03b7ddd7c9ce68'),
)
for asset, name, digest in assets:
    repo = 'linuxdeploy/linuxdeploy' if name.startswith('linuxdeploy') else 'AppImage/appimagetool'
    acquire(f'https://api.github.com/repos/{repo}/releases/assets/{asset}', inputs / 'appimage' / name, digest,
            {'Accept': 'application/octet-stream', 'User-Agent': 'convertme-release-capture'})
plugin = acquire('https://raw.githubusercontent.com/linuxdeploy/linuxdeploy-plugin-gtk/7a3fbc31a9e5075073ff8790f26effbac5f84453/linuxdeploy-plugin-gtk.sh',
                 inputs / 'appimage/linuxdeploy-plugin-gtk.sh', lock.get('appimage', {}).get('gtk_plugin_sha256'))
lock.setdefault('appimage', {})['gtk_plugin_sha256'] = plugin
fd, name = tempfile.mkstemp(prefix='.build.lock.', dir=lock_path.parent)
try:
    with os.fdopen(fd, 'w') as output:
        json.dump(lock, output, indent=2, sort_keys=True)
        output.write('\n')
    os.replace(name, lock_path)
finally:
    if os.path.exists(name): os.unlink(name)
PY
  echo 'Docker archives captured. Linux input capture is incomplete: a signed Ubuntu 24.04 snapshot, exact binary/source package closure, pinned Ubuntu OCI image and complete retained builder image, Rust 1.99 and Node 22 publisher-verified archives, and Cargo/npm/Python offline inputs must still be acquired and recorded before replay.' >&2
  exit 1
fi
[[ -n "$out" ]] || usage
out="$(realpath -m "$out")"
[[ "$out" != "$root" && "$out" != "$inputs" && "$inputs" != "$out/"* && "$out" != "$inputs/"* ]] || { echo 'output and inputs must be separate directories' >&2; exit 2; }
# Validate the entire retained input set before opening a daemon or executing bytes.
python3 - "$lock" "$inputs" <<'PY'
import hashlib, json, pathlib, re, sys
lock, root = map(pathlib.Path, sys.argv[1:])
def fail(message): raise SystemExit('Linux release input lock: ' + message)
try:
    data = json.loads(lock.read_text())
    if data.get('schema') != 1 or data.get('target') != 'x86_64-unknown-linux-gnu': fail('wrong schema or target')
    docker = data['docker']['archives']
    if set(docker) != {'docker-28.5.2.tgz', 'docker-rootless-extras-28.5.2.tgz'}: fail('capture both Docker 28.5.2 archives')
    ubuntu = data['ubuntu']
    if not re.fullmatch(r'sha256:[0-9a-f]{64}', ubuntu['oci_digest']): fail('Ubuntu OCI digest missing')
    if not ubuntu['snapshot_timestamp'] or not ubuntu['signature_evidence'] or not ubuntu['packages'] or not ubuntu['sources']: fail('signed snapshot and complete binary/source records required')
    for name, package in ubuntu['packages'].items():
        if not name or any(not package.get(key) for key in ('version', 'architecture', 'source', 'source_version', 'deb')): fail(f'incomplete Ubuntu package record: {name}')
    image = data['builder_image']
    if not re.fullmatch(r'sha256:[0-9a-f]{64}', image['image_id']): fail('builder image ID missing')
    if not image['archive']: fail('builder image archive missing')
    for tool, prefix in (('rust', '1.99.'), ('node', '22.')):
        if not data['toolchains'][tool]['version'].startswith(prefix): fail(f'wrong {tool} version')
    if not data['toolchains']['python']['version'].startswith('3.12.'): fail('wrong Python version')
    files = data['inputs']
    if not isinstance(files, dict) or not files: fail('complete input file manifest required')
    required = [image['archive'], *('docker/' + name for name in docker),
                *ubuntu['signature_evidence'],
                *(package['deb'] for package in ubuntu['packages'].values()),
                data['toolchains']['rust']['archive'], data['toolchains']['rust']['publisher_checksums'],
                data['toolchains']['node']['archive'], data['toolchains']['node']['publisher_checksums']]
    for name, source in ubuntu['sources'].items():
        if not source.get('dsc') or not source.get('archives') or not source.get('buildinfo'):
            fail(f'incomplete Ubuntu source package record: {name}')
        required.extend((source['dsc'], source['buildinfo'], *source['archives']))
    for name, package in ubuntu['packages'].items():
        if package['source'] not in ubuntu['sources']:
            fail(f'missing source closure for Ubuntu package {name}: {package["source"]}')
    for name in required:
        if name not in files: fail(f'input not in manifest: {name}')
    for name, record in files.items():
        rel = pathlib.PurePosixPath(name)
        if rel.is_absolute() or not rel.parts or any(part in ('', '.', '..') for part in rel.parts): fail(f'unsafe input path: {name}')
        expected = record['sha256']
        if not re.fullmatch(r'[0-9a-f]{64}', expected) or not isinstance(record['size'], int) or record['size'] <= 0: fail(f'invalid input record: {name}')
        path = root.joinpath(*rel.parts)
        if any(p.is_symlink() for p in (path, *list(path.parents)[:len(rel.parts)-1])): fail(f'symlink in input: {name}')
        if not path.is_file() or path.stat().st_size != record['size']: fail(f'missing or wrong-sized input: {name}')
        with path.open('rb') as f:
            if hashlib.file_digest(f, 'sha256').hexdigest() != expected: fail(f'hash mismatch: {name}')
    for name, digest in docker.items():
        if files['docker/' + name]['sha256'] != digest: fail(f'inconsistent Docker pin: {name}')
    for prefix in ('cargo/', 'npm/', 'python/linux/', 'python/sources/', 'tools/', 'sources/'):
        if not any(name.startswith(prefix) for name in files): fail(f'missing offline input closure: {prefix}')
    for path in root.rglob('*'):
        if path.is_symlink(): fail(f'symlink in input set: {path.relative_to(root)}')
        if path.is_file() and path.relative_to(root).as_posix() not in files:
            fail(f'unlocked input file: {path.relative_to(root)}')
except (OSError, KeyError, TypeError, ValueError) as exc:
    fail(f'missing or invalid captured record: {exc}')
PY
[[ ! -e "$out" ]] || { echo "refusing to replace existing output: $out" >&2; exit 1; }
mkdir -p "$(dirname "$out")"
stage="$(mktemp -d "$(dirname "$out")/.convertme-release.XXXXXXXX")"
trap 'rm -rf "$stage"' EXIT
mkdir "$stage/artifacts"
python3 - "$root" "$stage" <<'PY'
import io, os, pathlib, subprocess, sys, tarfile
root, stage = map(pathlib.Path, sys.argv[1:])
tracked = set(subprocess.check_output(['git', 'ls-files', '-z', '--cached'], cwd=root).split(b'\0'))
others = set(subprocess.check_output(['git', 'ls-files', '-z', '--others', '--exclude-standard'], cwd=root).split(b'\0'))
excluded = {'.git', 'target', 'vendor', 'node_modules', 'ui-dist', 'artifacts', 'fixtures', '__pycache__', '.node', '.venv', '.ssh', '.aws'}
extensions = {'.py', '.sh', '.rs', '.json', '.toml', '.lock', '.nsh', '.ps1', '.txt', '.yml', '.yaml', '.patch', '.html', '.css', '.ts', '.tsx', '.svg', '.png', '.ico', '.icns', '.in', '.spec'}
archive = stage / 'source-snapshot.tar.xz'
with tarfile.open(archive, 'w:xz', format=tarfile.PAX_FORMAT) as tar:
    for raw in sorted((tracked | others) - {b''}):
        name = os.fsdecode(raw)
        rel = pathlib.PurePosixPath(name)
        if rel.is_absolute() or '..' in rel.parts or any(p in excluded for p in rel.parts): continue
        if any(p.startswith('.env') or p in ('.npmrc', '.pypirc', '.netrc') or 'credential' in p.lower() or 'secret' in p.lower() for p in rel.parts): continue
        if raw not in tracked and rel.suffix not in extensions and rel.name not in ('Containerfile', 'AppRun'): continue
        path = root.joinpath(*rel.parts)
        if not path.exists() and not path.is_symlink(): continue
        if path.is_symlink() or any(p.is_symlink() for p in list(path.parents)[:len(rel.parts)-1]):
            raise SystemExit(f'unsafe source symlink: {name}')
        if not path.is_file(): continue
        info = tar.gettarinfo(str(path), arcname='convertme/' + rel.as_posix())
        info.uid = info.gid = 0
        info.uname = info.gname = ''
        info.mtime = 0
        with path.open('rb') as source: tar.addfile(info, source)
with tarfile.open(archive, 'r:xz') as tar:
    tar.extractall(stage, filter='data')
PY
python3 "$root/scripts/release-inventory.py" source-identity --snapshot "$stage/source-snapshot.tar.xz" --out "$stage/source-identity.json"
if (( native )); then
  python3 "$root/scripts/vendor-linux.py" --check-host
  mkdir "$stage/work"
  unshare --user --map-root-user --net env -i \
    PATH="$PATH" CONVERTME_WORK_BASE="$stage/work" \
    CONVERTME_SOURCE_SNAPSHOT="$stage/source-snapshot.tar.xz" \
    CONVERTME_SOURCE_IDENTITY_FILE="$stage/source-identity.json" \
    bash "$stage/convertme/scripts/build-linux-release.sh" --inside "$stage/convertme" "$inputs" "$stage/artifacts"
else
  image_archive="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["builder_image"]["archive"])' "$lock")"
  image_id="$(python3 -c 'import json,sys; print(json.load(open(sys.argv[1]))["builder_image"]["image_id"])' "$lock")"
  export CONVERTME_LINUX_INPUTS="$inputs" CONVERTME_ROOTLESS_LOG="$stage/rootless-docker.log"
  if ! "$root/scripts/with-rootless-docker.sh" -- bash -c '
    archive=$1; image=$2; checkout=$3; inputs=$4; output=$5; snapshot=$6; identity=$7; token=${8:-}
    docker load --input "$archive"
    actual=$(docker image inspect --format "{{.Id}}" "$image")
    [[ "$actual" == "$image" ]] || { echo "retained builder image ID mismatch" >&2; exit 1; }
    args=(--rm --network=none --pull=never --read-only --tmpfs /work:exec --tmpfs /tmp:exec
      --mount "type=bind,src=$checkout,dst=/checkout,readonly"
      --mount "type=bind,src=$inputs,dst=/inputs,readonly"
      --mount "type=bind,src=$output,dst=/out")
    args+=(--mount "type=bind,src=$snapshot,dst=/run/convertme-source.tar.xz,readonly"
      --env CONVERTME_SOURCE_SNAPSHOT=/run/convertme-source.tar.xz
      --mount "type=bind,src=$identity,dst=/run/convertme-source-identity.json,readonly"
      --env CONVERTME_SOURCE_IDENTITY_FILE=/run/convertme-source-identity.json)
    if [[ -n "$token" ]]; then
      args+=(--mount "type=bind,src=$token,dst=/run/convertme-forbidden.txt,readonly"
        --env DEBRAND_TOKENS_FILE=/run/convertme-forbidden.txt)
    fi
    docker run "${args[@]}" "$image" bash /checkout/scripts/build-linux-release.sh --inside /checkout /inputs /out
  ' _ "$inputs/$image_archive" "$image_id" "$stage/convertme" "$inputs" "$stage/artifacts" "$stage/source-snapshot.tar.xz" "$stage/source-identity.json" "${DEBRAND_TOKENS_FILE:-}"; then
    log="$(dirname "$out")/convertme-rootless-docker.log"
    [[ ! -f "$stage/rootless-docker.log" ]] || cp "$stage/rootless-docker.log" "$log"
    echo "Rootless build failed; daemon log: $log. On Ubuntu 24.04 x86_64 use: $root/scripts/build-linux-release.sh --inputs '$inputs' --out '$out' --native" >&2
    exit 1
  fi
fi
mv "$stage/artifacts" "$out"
