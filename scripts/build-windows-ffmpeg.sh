#!/usr/bin/env bash
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
mode=''
lock="$root/packaging/windows/ffmpeg.lock.json"
out=''
while (( $# )); do
    case "$1" in
        --capture|--replay) [[ -z "$mode" ]] || { echo 'choose exactly one mode' >&2; exit 2; }; mode="$1"; shift ;;
        --lock) (( $# >= 2 )) || { echo '--lock requires a path' >&2; exit 2; }; lock="$2"; shift 2 ;;
        --out) (( $# >= 2 )) || { echo '--out requires a path' >&2; exit 2; }; out="$2"; shift 2 ;;
        *) echo "unknown argument: $1" >&2; exit 2 ;;
    esac
done
[[ -n "$mode" && -n "$out" ]] || { echo 'usage: build-windows-ffmpeg.sh --capture|--replay --lock <path> --out <dir>' >&2; exit 2; }
inputs="${CONVERTME_FFMPEG_INPUTS:-$root/vendor/windows/ffmpeg-build-inputs}"
[[ -d "$inputs" ]] || { echo "FFmpeg $mode requires retained inputs $inputs" >&2; exit 1; }
if [[ "$mode" == --capture ]]; then
    [[ -f "$inputs/capture.json" ]] || { echo "FFmpeg capture requires $inputs/capture.json: complete exact package/stage source, license, recipe, image filesystem ownership, Docker and Buildx records and all referenced bytes" >&2; exit 1; }
    exec "$root/scripts/with-rootless-docker.sh" -- python3 "$root/scripts/windows-ffmpeg-replay.py" --capture --lock "$lock" --inputs "$inputs" --out "$out"
fi
[[ -f "$lock" ]] || { echo "FFmpeg replay requires captured lock $lock" >&2; exit 1; }
exec "$root/scripts/with-rootless-docker.sh" -- python3 "$root/scripts/windows-ffmpeg-replay.py" --lock "$lock" --inputs "$inputs" --out "$out"
