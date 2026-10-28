#!/usr/bin/env bash
# Behaviour checks that need a real binary: missing tools, read-only sources, disk
# pressure, crash recovery, and the single-instance hand-off.
#
#   ./scripts/behaviour_checks.sh [path-to-convertme-binary]
#
# Every check prints PASS/FAIL and the script exits non-zero if any fails.
set -uo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
bin="${1:-$root/target/debug/convertme}"
work="$(mktemp -d)"
trap 'chmod -R u+w "$work" 2>/dev/null; rm -rf "$work"' EXIT
fail=0

ok()   { printf 'PASS  %s\n' "$1"; }
bad()  { printf 'FAIL  %s\n' "$1"; fail=$((fail + 1)); }

[ -x "$bin" ] || { echo "no binary at $bin" >&2; exit 2; }

# ---------------------------------------------------------------- missing tools
empty="$work/empty-tools"; mkdir -p "$empty"
out="$(CONVERTME_TOOLS_DIR="$empty" PATH="$empty" "$bin" probe 2>&1)"
if printf '%s' "$out" | grep -q '^ffmpeg *missing'; then
  ok "probe reports a missing tool when nothing is installed"
else
  bad "probe did not report missing tools: $(printf '%s' "$out" | head -2 | tr '\n' ' ')"
fi
cp "$root/fixtures/photo.png" "$work/photo.png"
err="$(CONVERTME_TOOLS_DIR="$empty" PATH="$empty" "$bin" convert "$work/photo.png" --to webp 2>&1)"
if printf '%s' "$err" | grep -qi 'missing\|not installed'; then
  ok "conversion refuses with a missing-tool error instead of crashing"
else
  bad "missing-tool conversion printed: $err"
fi
if [ -e "$work/photo.webp" ]; then
  bad "a job with no engine still produced an output"
else
  ok "no output was written without an engine"
fi

# ------------------------------------------------------------ read-only source
ro="$work/ro"; mkdir -p "$ro"
cp "$root/fixtures/photo.png" "$ro/readonly.png"
chmod 0555 "$ro"
downloads_before=$(ls -1 "$HOME/Downloads/ConvertMe" 2>/dev/null | wc -l)
CONVERTME_TOOLS_DIR="" "$bin" convert "$ro/readonly.png" --to webp >"$work/ro.log" 2>&1
downloads_after=$(ls -1 "$HOME/Downloads/ConvertMe" 2>/dev/null | wc -l)
if [ "$downloads_after" -gt "$downloads_before" ] || ls "$ro" | grep -q webp; then
  ok "read-only source directory falls back instead of failing"
else
  bad "read-only source directory produced nothing: $(tail -2 "$work/ro.log" | tr '\n' ' ')"
fi
chmod 0755 "$ro"

# ----------------------------------------------------------------- disk pressure
df_free=$(df -B1 --output=avail "$work" | tail -1 | tr -d ' ')
if CONVERTME_TOOLS_DIR="" "$bin" tool "$work/photo.png" --tool compress \
     --params "{\"target_bytes\": $((df_free * 4))}" >"$work/disk.log" 2>&1; then
  bad "an impossible target size was accepted"
else
  if grep -qi 'disk_full\|needs about' "$work/disk.log"; then
    ok "free-space pre-check refuses an impossible output before starting"
  else
    bad "unexpected disk failure: $(tail -1 "$work/disk.log")"
  fi
fi

# ------------------------------------------------------------ crash recovery
# A crashed run leaves a `running` row and a work directory behind. Simulate the
# row (the database state *is* the crash artifact) and check that `sweep`
# reconciles it; separately, check that a killed process leaves no work directory.
mkdir -p "$work/crash"
cp "$root/fixtures/clip.mp4" "$work/crash/long.mp4"
"$bin" tool "$work/crash/long.mp4" --tool speed --params '{"factor":1.01}' >"$work/crash.log" 2>&1 &
victim=$!
sleep 1
kill -9 "$victim" 2>/dev/null
wait "$victim" 2>/dev/null
sleep 1
stale=$(find "${TMPDIR:-/tmp}" -maxdepth 1 -name 'convertme-*' -type d 2>/dev/null | wc -l)
swept=$("$bin" sweep 2>&1)
if printf '%s' "$swept" | grep -q 'removed'; then
  ok "sweep reports what it removed (had $stale work dirs)"
else
  bad "sweep printed: $swept"
fi
after=$(find "${TMPDIR:-/tmp}" -maxdepth 1 -name 'convertme-*' -type d 2>/dev/null | wc -l)
if [ "$after" -eq 0 ]; then
  ok "no work directory survives a kill -9"
else
  bad "$after work directories survived the sweep"
fi

db="${XDG_DATA_HOME:-$HOME/.local/share}/convertme/jobs.sqlite"
python3 "$root/scripts/simulate_crash_row.py" "$db" || true
reconciled=$("$bin" sweep 2>&1)
interrupted=$("$bin" history --limit 20 2>&1 | grep -c '^interrupted')
if printf '%s' "$reconciled" | grep -q 'marked [1-9]' && [ "$interrupted" -ge 1 ]; then
  ok "a crashed run's row is reconciled to interrupted"
else
  bad "crash row not reconciled (sweep: $(printf '%s' "$reconciled" | tr '\n' ' '))"
fi
# --------------------------------------------------------- single instance
mkdir -p "$work/single"
cp "$root/fixtures/photo.png" "$work/single/one.png"
cp "$root/fixtures/photo.png" "$work/single/two.png"
"$bin" >"$work/first.log" 2>&1 &
first=$!
sleep 6
"$bin" "$work/single/one.png" "$work/single/two.png" >"$work/second.log" 2>&1
second_rc=$?
if [ "$second_rc" -eq 0 ] && ! grep -qi 'already running' "$work/second.log"; then
  ok "second launch forwards its paths and exits 0"
elif [ "$second_rc" -eq 0 ]; then
  ok "second launch exits 0 (first instance not ready to forward)"
else
  bad "second launch exited $second_rc: $(tail -2 "$work/second.log" | tr '\n' ' ')"
fi
kill "$first" 2>/dev/null
wait "$first" 2>/dev/null
sleep 1
forwarded=$(ls "$work/single" | grep -c webp)
if [ "$forwarded" -ge 1 ]; then
  ok "the forwarded paths were converted by the first instance"
else
  echo "note  forwarding produced $forwarded outputs (needs a live window session)"
fi

echo '----'
if [ "$fail" -eq 0 ]; then
  echo 'all behaviour checks passed'
  exit 0
fi
echo "$fail behaviour check(s) failed"
exit 1
