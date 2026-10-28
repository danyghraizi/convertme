#!/usr/bin/env bash
# Verifies that the identity gate actually gates: a planted token must fail the
# build, a clean tree must pass, and a missing token list must refuse to run.
set -uo pipefail

here="$(cd "$(dirname "$0")" && pwd)"
work="$(mktemp -d)"
trap 'rm -rf "$work"' EXIT

mkdir -p "$work/scripts"
cp "$here/debrand.py" "$work/scripts/debrand.py"
printf 'nothing to see here\n' > "$work/clean.txt"

token="zqx$(date +%s)selftest"
fail=0

echo 'clean tree, token supplied inline:'
if DEBRAND_TOKENS="$token" python3 "$work/scripts/debrand.py" --src; then :; else
  echo 'FAIL: clean tree was rejected'; fail=1
fi

echo 'planted token:'
printf 'this file mentions %s in passing\n' "$token" > "$work/planted.txt"
if DEBRAND_TOKENS="$token" python3 "$work/scripts/debrand.py" --src; then
  echo 'FAIL: planted token was not caught'; fail=1
else
  echo 'ok: planted token rejected'
fi
rm -f "$work/planted.txt"

echo 'base64 token:'
printf 'aggressor %s\n' "$token" > "$work/planted_b64.txt"
b64="b64:$(printf '%s' "$token" | base64 -w0)"
if DEBRAND_TOKENS="$b64" python3 "$work/scripts/debrand.py" --src; then
  echo 'FAIL: base64 token was not caught'; fail=1
else
  echo 'ok: base64 token rejected'
fi
rm -f "$work/planted_b64.txt"

echo 'utf-16 token inside a binary:'
python3 - "$work/artifact.exe" "$token" <<'PY'
import sys
open(sys.argv[1], 'wb').write(b'MZ\x90\x00' + sys.argv[2].encode('utf-16-le') + b'\x00' * 32)
PY
mkdir -p "$work/target/release/bundle"
mv "$work/artifact.exe" "$work/target/release/bundle/artifact.exe"
if DEBRAND_TOKENS="$token" python3 "$work/scripts/debrand.py" --all; then
  echo 'FAIL: utf-16 token in an artifact was not caught'; fail=1
else
  echo 'ok: artifact token rejected'
fi
rm -rf "$work/target"

echo 'missing token list:'
if env -u DEBRAND_TOKENS -u DEBRAND_TOKENS_FILE python3 "$work/scripts/debrand.py" --src; then
  echo 'FAIL: gate passed without tokens'; fail=1
else
  echo 'ok: gate refuses to run without tokens'
fi

if [ "$fail" -ne 0 ]; then
  echo 'debrand selftest: FAILED'
  exit 1
fi
echo 'debrand selftest: ok'
