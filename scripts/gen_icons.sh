#!/usr/bin/env bash
# Regenerates the app icons from one vector source. Run after changing the mark.
set -euo pipefail
here="$(cd "$(dirname "$0")/.." && pwd)"
out="$here/crates/convertme-gui/icons"
mkdir -p "$out"

svg="$out/mark.svg"
cat > "$svg" <<'SVG'
<svg xmlns="http://www.w3.org/2000/svg" viewBox="0 0 256 256" width="256" height="256">
  <rect width="256" height="256" rx="56" fill="#12161b"/>
  <circle cx="128" cy="128" r="78" fill="none" stroke="#2ea8a0" stroke-width="14" stroke-linecap="round"
          stroke-dasharray="330 160" transform="rotate(-38 128 128)"/>
  <path d="M96 96h72l-22-22" fill="none" stroke="#e9eef4" stroke-width="14" stroke-linecap="round" stroke-linejoin="round"/>
  <path d="M160 160H88l22 22" fill="none" stroke="#e9eef4" stroke-width="14" stroke-linecap="round" stroke-linejoin="round"/>
</svg>
SVG

render() { # size outfile
  magick -background none "$svg" -resize "$1x$1" "$2"
}

render 32  "$out/32x32.png"
render 128 "$out/128x128.png"
render 256 "$out/128x128@2x.png"
render 512 "$out/icon.png"
magick "$out/icon.png" -define icon:auto-resize=256,128,64,48,32,16 "$out/icon.ico"
if magick "$out/icon.png" "$out/icon.icns" 2>/dev/null; then :; else
  # ImageMagick without the ICNS coder: fall back to png2icns when available.
  if command -v png2icns >/dev/null; then
    png2icns "$out/icon.icns" "$out/32x32.png" "$out/128x128.png" "$out/128x128@2x.png"
  else
    echo "warning: no ICNS encoder available; icon.icns not written" >&2
  fi
fi
ls -l "$out"
