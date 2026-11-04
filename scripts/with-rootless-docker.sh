#!/usr/bin/env bash
# Start and supervise a private rootless Docker daemon for one command.
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
inputs="${CONVERTME_LINUX_INPUTS:-$root/vendor/linux/inputs}"
lock="$root/packaging/linux/build.lock.json"
if [[ "${1:-}" != -- ]] || (( $# < 2 )); then
  echo 'usage: with-rootless-docker.sh -- command [args...]' >&2; exit 2
fi
shift
for cmd in python3 newuidmap newgidmap unshare setsid; do
  command -v "$cmd" >/dev/null || { echo "rootless Docker requires $cmd" >&2; exit 1; }
done
user="$(id -un)"
for file in /etc/subuid /etc/subgid; do
  if ! python3 - "$file" "$user" "$(id -u)" <<'PY'
import sys
name, uid = sys.argv[2:]
with open(sys.argv[1], encoding='utf-8') as f:
    ranges = [line.strip().split(':') for line in f if line.strip() and not line.startswith('#')]
if not any(len(row) == 3 and row[0] in (name, uid) and int(row[2]) >= 65536 for row in ranges):
    raise SystemExit(1)
PY
  then echo "rootless Docker requires at least 65536 subordinate IDs in $file for $user" >&2; exit 1; fi
done
unshare --user --map-root-user true || { echo 'unprivileged user namespaces unavailable' >&2; exit 1; }
work="$(mktemp -d "${TMPDIR:-/tmp}/convertme-rootless.XXXXXXXX")"
chmod 700 "$work"
pid=''
cleanup() {
  local status=$?
  trap - EXIT INT TERM
  if [[ -n "$pid" ]]; then
    kill -TERM -- "-$pid" 2>/dev/null || true
    for _ in {1..10}; do
      kill -0 -- "-$pid" 2>/dev/null || break
      sleep 1
    done
    kill -KILL -- "-$pid" 2>/dev/null || true
    wait "$pid" 2>/dev/null || true
  fi
  if (( status != 0 )) && [[ -f "$work/daemon.log" ]]; then
    failed_log="${CONVERTME_ROOTLESS_LOG:-$root/convertme-rootless-docker.log}"
    mkdir -p "$(dirname "$failed_log")"
    cp "$work/daemon.log" "$failed_log"
    echo "rootless Docker log: $failed_log" >&2
  fi
  rm -rf "$work"
}
trap cleanup EXIT
trap 'exit 130' INT
trap 'exit 143' TERM
# Never trust archive names or cache presence; the frozen lock owns hashes.
python3 - "$lock" "$inputs" "$work" <<'PY'
import hashlib, json, pathlib, sys, tarfile
lock, inputs, work = map(pathlib.Path, sys.argv[1:])
try:
    records = json.loads(lock.read_text())['docker']['archives']
    if set(records) != {'docker-28.5.2.tgz', 'docker-rootless-extras-28.5.2.tgz'}:
        raise ValueError('expected both Docker 28.5.2 archives in docker.archives')
    for name, expected in records.items():
        archive = inputs / 'docker' / name
        with archive.open('rb') as f:
            if hashlib.file_digest(f, 'sha256').hexdigest() != expected:
                raise ValueError(f'Docker archive hash mismatch: {archive}')
        with tarfile.open(archive, 'r:gz') as tar:
            for member in tar.getmembers():
                path = pathlib.PurePosixPath(member.name)
                if path.is_absolute() or '..' in path.parts or not (member.isdir() or member.isfile()):
                    raise ValueError(f'unsafe Docker archive member: {member.name}')
            tar.extractall(work / 'tools', filter='data')
except (OSError, ValueError, KeyError, tarfile.TarError) as exc:
    raise SystemExit(f'rootless Docker inputs: {exc}')
PY
export PATH="$work/tools/docker:$work/tools/docker-rootless-extras:$PATH"
original_runtime_set="${XDG_RUNTIME_DIR+x}"
original_runtime="${XDG_RUNTIME_DIR-}"
export XDG_RUNTIME_DIR="$work/runtime"
mkdir -m 700 "$XDG_RUNTIME_DIR" "$work/data" "$work/exec" "$work/state"
export DOCKER_HOST="unix://$XDG_RUNTIME_DIR/docker.sock"
# Upstream dockerd-rootless.sh rejects NET=host. VPNKit is the bundled rootless
# network driver; use it for connected capture only. Build/replay commands use
# Docker --network=none.
export DOCKERD_ROOTLESS_ROOTLESSKIT_NET=vpnkit
export DOCKERD_ROOTLESS_ROOTLESSKIT_STATE_DIR="$work/state"
unset DOCKER_CONTEXT DOCKER_TLS_VERIFY DOCKER_CERT_PATH
log="$work/daemon.log"
setsid dockerd-rootless.sh --storage-driver=vfs --bridge=none --iptables=false --data-root "$work/data" --exec-root "$work/exec" >"$log" 2>&1 &
pid=$!
ready=0
for ((i=0; i<120; i++)); do
  if docker info >/dev/null 2>&1; then ready=1; break; fi
  if ! kill -0 "$pid" 2>/dev/null; then break; fi
  sleep 1
done
if (( ! ready )); then echo "rootless Docker failed; log: $log" >&2; exit 1; fi
if ! docker info --format '{{json .SecurityOptions}}' | python3 -c 'import json,sys; assert any("rootless" in x for x in json.load(sys.stdin))'; then
  echo "Docker did not report rootless security; log: $log" >&2; exit 1
fi
if [[ -n "$original_runtime_set" ]]; then
  export XDG_RUNTIME_DIR="$original_runtime"
else
  unset XDG_RUNTIME_DIR
fi
unset DOCKERD_ROOTLESS_ROOTLESSKIT_NET
"$@"
