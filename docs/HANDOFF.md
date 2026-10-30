# Handoff

State of the project after the second working session: `add background` is complete and
**Compress now previews its real result**. Read this first, then `AGENTS.md` (rules) and
`docs/LESSONS.md` (why the rules exist).

## Where things stand

`add background` and `compress` are feature-complete and verified. Nothing else has been
started.

Working tools (UI and engine): Compress, Crop, Add background, Edit photo, Metadata,
Redact, plus conversions for images. The wheel, the drag layer, the queue, history and
settings exist as described in the plan; the GUI is usable end to end on this machine.

## What "compress" does now

| Control | Behaviour |
|---|---|
| Quality | encoder fidelity for lossy formats (100 = maximum fidelity, which can be *larger* than the source); hidden when every selected image is lossless |
| Lossless files | PNG: zlib 9 plus oxipng (row filters + deflate), measured 96.9→81.9 KB on the 16-bit poster, 112.1→67.6 KB on a screenshot, 44.5→35.6 KB on an 8-bit gradient; TIFF: LZW (5.76→3.08 MB); BMP: nothing to ask for. Pixels stay identical |
| Never larger | if the encode would grow the file, the result is a copy of the source (same rule as video/audio/PDF); the preview shows “no change” |
| Image size | 100 / 75 / 50 / 25 % of the dimensions |
| Target file size | quality search, then up to three downscale rounds; the smallest result when the target is unreachable (lossless also drops the palette) |
| Preview | the **real encoded file**: true size, true dimensions, percent saved, and a picture of the result. Re-encodes 400 ms after the last edit, with stale responses dropped |
| Run | reuses the preview artifact when source mtime/size and every parameter still match; otherwise encodes once and stores it |

The preview is keyed in the shared 256 MB preview cache by source identity (canonical
path, mtime, size) plus the normalized parameters and the output extension. A cache hit is
a copy; there is no second encode. The whole pipeline is
`encode_compress` → `PreviewCache::store_artifact` → `compress` cache hit.

The other Compress families (no dialog preview; they are not pictures):

| Category | Controls | Behaviour |
|---|---|---|
| Video (incl. GIF) | Size 5–100 % of the source (default 50), or an opt-in target size | bitrate derived from the target; a short clip's audio budget shrinks to fit instead of failing; never grows |
| Audio | Size %, or an opt-in bitrate / target size | per-codec bitrate floors (libvorbis needs 32 kbps); lossless WAV/AIFF/FLAC is refused with "convert to MP3, OGG or Opus" instead of writing the same file back |
| PDF | preset (screen/ebook/printer/prepress), optional target | Ghostscript tiers; falls back to a copy when the result would not be smaller |

Matrix on the fixtures (2026-10-30): images PNG/JPG/BMP/TIFF/WEBP/AVIF/HEIC/SVG plus a
generated PNG all produced valid files (GIF is registered under Video and compresses
there); video MP4/MKV/WEBM/MOV/AVI/WMV/GIF all at ≤ the source size; lossy audio
MP3/OGG/OPUS/M4A/WMA all shrank; PDF compressed, an empty PDF is skipped. `scale 50` on a
1200×800 PNG produced 600×400; target sizes were honoured where reachable and the
smallest result returned where not.

Fixed in the same pass: the `target file size` switch was a plain checkbox, so it travelled
to the backend and `check_params` rejected **every** image compress run ("unknown parameter
`target`"). It is now marked `control` like the background mode switch.

Engine notes worth knowing:

- `add background` fills: 4 solids, a rule, 4 gradients, plus another image; padding and
  corners as percentages of the shorter side (100 % = fully rounded); shadow 0–100 % with
  an angle (0 = above, 180 = below); background image zoom to 3× with drag on the canvas;
  canvas ratio Auto/1:1/16:9/4:3/9:16/3:4/4:5; multi-file arrow picker with per-file
  settings unless "Same settings for every file" is ticked. Its preview shows the real
  canvas size in pixels.
- The background picture is cropped at source resolution and then scaled to the canvas.
- The shadow is built from a rounded-rect mask at ≤ 600 px, blurred, turned into alpha and
  scaled back; above **60 megapixels** it is skipped so the job still succeeds.
- ImageMagick works at depth 8 for the composition, with 1 GiB of RAM, 2 GiB of mapping
  and 16 GiB of disk spill (`crates/convertme-engines/src/proc.rs`).
- The preview cache evicts by **mtime**, not atime: `touch()` bumps mtime, and atime is
  unreliable under relatime/noatime.

## Known limits / next performance work (deliberately not done)

1. **Shadow above 60 Mpx is dropped, not tiled.** Tiling the shadow (or compositing it per
   tile) is the real fix; it needs a streaming composition path.
2. **Very large canvases rely on disk spill.** A 195 Mpx input composes in ~36 s under the
   modest budget, but only when the temp filesystem has room. A free-space pre-check for
   the *temp* dir (not just the output) is missing.
3. **Shadow cost is still the dominant term** for big canvases (several seconds).
4. The installed size and cold-start budgets from the plan have not been re-measured since
   the UI work.
5. Compress target-size previews re-run the whole quality search on every parameter
   change. It is debounced and cached, but a target on a huge file still costs seconds.

## Verification status

- `./scripts/preflight.sh` passes: identity gate, fmt, clippy `-D warnings`, all workspace
  tests (80 engine tests, 17 GUI tests), node tests for the field reader and the parameter
  collector, UI typecheck/build, clean-checkout builds.
- Composition tests pin: proportional padding/corners, rounding belongs to the photo,
  background placement stays inside the frame at -100/0/100, shadow darkens the band.
- Compress tests pin: a preview's bytes equal the job's artifact, the job completes with
  the **source unreadable** (proving reuse rather than a second encode), different
  qualities produce different artifacts, the artifact key keeps its extension, lossless
  compression keeps every pixel and TIFF actually shrinks, q100 on a small JPEG returns
  the original, and the command body reports the real fixture file and resolves a repeat
  request to the same cached artifact.
- Contract tests pin: the field payload names the kinds the UI renders and marks the
  compress target switch as a control; the binary embeds the built bundle; the reader
  keeps every kind and property; controls and hidden fields never become backend
  parameters.
- The CLI publishes a relative input next to the working directory; it used to treat the
  empty parent as unwritable and write to `~/Downloads`.
- SVG compresses to a real PNG (rasterisation is a format change, so the never-grow rule
  does not compare the PNG with the SVG bytes).
- `boot` drains the launch paths only for the main window: the prewarmed wheel boots too
  and used to swallow every "Open with" launch before the main window saw it.
- The dialog was measured in the built bundle (mock mode, headless Firefox): pane
  570×254 px inside the modal, image loaded (512×384), facts read
  `4032 × 3024 · 886 KB · −36% · from 1.4 MB`, and ticking the target switch re-encoded to
  `488 KB · −65%` within the debounce.

## Running it

```bash
npm --prefix ui run build      # required before cargo build: the bundle is embedded
cargo build --release -p convertme-gui
./target/release/convertme
```

Changing `ui/src` without rebuilding the bundle now fails the build script (and a test
compares the embedded page with `ui-dist`).

## Backlog

1. **Crop** — the next tool to bring to the same standard (a live preview of the kept
   area, already started via `CropEditor`; check the handles and multi-file behaviour).
2. Edit photo, Annotate, Redact, Metadata — the rest of the image tool row.
3. Video, audio, PDF, documents, slides, spreadsheets, EPUB, subtitles, archives.
4. Performance pass per the plan's Step 0 budgets, including tiled composition.
