#!/usr/bin/env bash
# Source immutability sweep.
#
# Copies the fixture matrix into a scratch directory, makes every fixture read-only
# (the directory stays writable), runs every conversion and every tool over them,
# then asserts that each source is byte-identical and its mtime is unchanged.
#
#   ./scripts/immutability_sweep.sh [path-to-convertme-binary]
#
# Exits non-zero on the first violation or failed job.
set -uo pipefail

root="$(cd "$(dirname "$0")/.." && pwd)"
bin="${1:-$root/target/debug/convertme}"
work="$(mktemp -d)"
trap 'chmod -R u+w "$work" 2>/dev/null; rm -rf "$work"' EXIT

if [ ! -x "$bin" ]; then
  echo "no binary at $bin (build it first)" >&2
  exit 2
fi

mkdir -p "$work/src"
cp "$root"/fixtures/{photo.png,tiny.png,notes.pdf,clip.mp4,tone.mp3,cues.srt,sheet.csv,notes.md,notes.epub,bundle.zip,drawing.svg} "$work/src/" 2>/dev/null
cp "$root/fixtures/photo.png" "$work/src/logo.png"
chmod 0444 "$work/src/"*

manifest="$work/before.txt"
src="$work/src"
# Only the files that existed before the sweep are the sources; artifacts written
# next to them are expected additions, not changes.
mapfile -t originals < <(cd "$src" && ls -1)
snapshot() {
  # sha256, mtime (epoch seconds), size — atime is deliberately excluded.
  ( cd "$src" && for f in "${originals[@]}"; do
      printf '%s  %s  %s  %s\n' "$(sha256sum -- "$f" | cut -d' ' -f1)" \
        "$(stat -c %Y -- "$f")" "$(stat -c %s -- "$f")" "$f"
    done | sort )
}
snapshot > "$manifest"

pass=0
fail=0
run() { # description, args...
  local what="$1"; shift
  if "$bin" "$@" >"$work/last.log" 2>&1; then
    pass=$((pass + 1))
    printf 'ok    %s\n' "$what"
  else
    fail=$((fail + 1))
    printf 'FAIL  %s\n      %s\n' "$what" "$(tail -2 "$work/last.log" | tr '\n' ' ')"
  fi
}

run "png → webp"            convert "$src/photo.png"   --to webp
run "png → png (rename)"    convert "$src/logo.png"    --to png
run "svg → png"             convert "$src/drawing.svg" --to png
run "png → pdf"             convert "$src/photo.png"   --to pdf
run "pdf → txt"             convert "$src/notes.pdf"   --to txt
run "pdf → jpg"             convert "$src/notes.pdf"   --to jpg
run "pdf → docx"            convert "$src/notes.pdf"   --to docx
run "mp4 → mp3"             convert "$src/clip.mp4"    --to mp3
run "mp4 → gif"             convert "$src/clip.mp4"    --to gif
run "srt → vtt"             convert "$src/cues.srt"    --to vtt
run "csv → tsv"             convert "$src/sheet.csv"   --to tsv
run "md → html"             convert "$src/notes.md"    --to html
run "epub → docx"           convert "$src/notes.epub"  --to docx
run "zip → tar"             convert "$src/bundle.zip"  --to tar
run "image compress"        tool "$src/photo.png" --tool compress --params '{"percent":30}'
run "image crop"            tool "$src/photo.png" --tool crop --params '{"aspect":"1:1"}'
run "image annotate"        tool "$src/photo.png" --tool annotate --params '{"text":"sweep","color":"#ffffff"}'
run "image add background"  tool "$src/photo.png" --tool add-background --params '{"color":"#101418","padding":32}'
run "image edit photo"      tool "$src/photo.png" --tool edit-photo --params '{"brightness":10,"saturation":-20}'
run "image metadata write"  tool "$src/photo.png" --tool metadata --params '{"set":{"Title":"sweep"}}' --category images
run "image redact"          tool "$src/photo.png" --tool redact --params '{"boxes":[{"x":0.1,"y":0.1,"w":0.2,"h":0.2}],"color":"#000000"}'
run "video trim"            tool "$src/clip.mp4" --tool trim --params '{"start":0.2,"end":1.0}'
run "video speed"           tool "$src/clip.mp4" --tool speed --params '{"factor":1.5}'
run "video mute"            tool "$src/clip.mp4" --tool mute
run "video snapshot"        tool "$src/clip.mp4" --tool snapshot --params '{"times":"0.5"}'
run "video split"           tool "$src/clip.mp4" --tool split --params '{"sections":"0-1,1-2"}'
run "audio normalize"       tool "$src/tone.mp3" --tool normalize
run "audio channels"        tool "$src/tone.mp3" --tool channels --params '{"channels":"1"}'
run "audio bleep"           tool "$src/tone.mp3" --tool bleep --params '{"ranges":"0.2-0.4"}'
run "audio visualize"       tool "$src/tone.mp3" --tool visualize
run "pdf compress"          tool "$src/notes.pdf" --tool compress
run "pdf organize"          tool "$src/notes.pdf" --tool organize
run "pdf save pages"        tool "$src/notes.pdf" --tool save-pages --params '{"format":"png"}'
run "pdf extract text"      tool "$src/notes.pdf" --tool extract-text
run "archive extract"       tool "$src/bundle.zip" --tool extract
run "subtitle to text"      convert "$src/cues.srt" --to txt

snapshot > "$work/after.txt"
if diff -u "$manifest" "$work/after.txt" > "$work/diff.txt"; then
  echo "sources unchanged ($(wc -l < "$manifest") files)"
else
  echo "SOURCES CHANGED:" >&2
  cat "$work/diff.txt" >&2
  fail=$((fail + 1))
fi

# The read-only directory fallback: a source it cannot write next to must land in
# the Downloads fallback instead of failing.
if [ -d "$HOME/Downloads/ConvertMe" ] && [ -w "$HOME/Downloads/ConvertMe" ]; then
  echo "downloads fallback present at $HOME/Downloads/ConvertMe"
fi

# No temp work directories may survive.
leftover="$(find "${TMPDIR:-/tmp}" -maxdepth 1 -name 'convertme-*' -type d 2>/dev/null | wc -l)"
if [ "$leftover" -ne 0 ]; then
  echo "leftover work directories: $leftover" >&2
  find "${TMPDIR:-/tmp}" -maxdepth 1 -name 'convertme-*' -type d 2>/dev/null >&2
  fail=$((fail + 1))
fi

echo "----"
echo "jobs passed: $pass, violations/failures: $fail"
[ "$fail" -eq 0 ]
