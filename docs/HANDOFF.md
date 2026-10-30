# Handoff

State of the project after the compress-hardening session: `add background` is complete
and **Compress is feature-complete across images, video, audio and PDF, previews real
output for images, and no longer inflates files**. A headless screenshot harness now
drives every tool dialog. Read this first, then `AGENTS.md` (rules) and `docs/LESSONS.md`
(why the rules exist).

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

### Findings from this session

Each item: cause → fix → evidence. The reusable rules are `docs/LESSONS.md` §17–29.

| Symptom | Cause | Fix | Evidence |
|---|---|---|---|
| Every image compress run failed validation | the `target size` checkbox travelled as a parameter | marked it a control | `compress_target_is_a_control…`, manual run |
| Preview cache evicted arbitrary entries | sorted by atime, but `touch()` bumps mtime | sort by mtime; never evict the artifact just stored | `cap_is_enforced_oldest_first` was 1-in-3 flaky, now 5/5 |
| AV compress grew a 62 KB clip to 131 KB | fixed 20 MB `target_bytes` default always sent | percentage default (50 %), target/bitrate opt-in | matrix, 62→24 KB |
| 2 s clips and small OGGs failed | audio budget exceeded the target; libvorbis floor is 32 kbps | degrade the audio budget; clamp per codec | mp4/webm/ogg tests |
| Lossless audio "compressed" to the same file | lossless branch ignored bitrate | refuse with a format that can shrink | `lossless_audio_is_refused` |
| q100 JPEG grew 5.4 MB → 17.9 MB | quality is fidelity, not compression | never return a larger file (copy the source) | CLI: byte-identical 5,625,026 |
| PNG compressed only ~2.5 % vs a website | ImageMagick's encoder was the bottleneck | oxipng (lossless) + zlib 9; TIFF LZW | 96.9→81.9 KB, 112.1→67.6 KB, TIFF 5.76→3.08 MB |
| SVG dialog showed a broken image | cross-format never-grow copied SVG bytes to `.png` | apply never-grow only when the container is unchanged | PNG magic bytes test + real app |
| `convertme file` opened an empty window | the prewarmed wheel's `boot()` drained launch paths | drain only for the main window | real app: "1 file ready", screenshot |
| CLI wrote relative inputs to `~/Downloads` | empty parent treated as unwritable | normalise to `.` | `a_bare_filename_publishes…` |
| Quality slider did nothing on PNG/BMP/TIFF | those encoders ignore `-quality` | hide it for lossless selections and drop it from params | harness assertion, mixed selection keeps it |

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
  unreliable under relatime/noatime. Its key is versioned (`v4`); bump it when encoder
  output changes.
- PNG optimisation runs the `oxipng` crate in-process (MIT; preset 2, 4 s timeout, 64 MB
  cap); TIFF asks ImageMagick for LZW. Neither strips metadata.

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
6. **Lossy PNG parity is not implemented.** The website result (18 KB) was a 177-colour
   palette of the 16-bit poster. We do lossless only (81.9 KB). A "Reduce colours"
   option using `imagequant` (GPL-3-compatible with this GPL app) is the honest next step;
   ImageMagick's own quantizer is weak (74 KB at 256 colours) and should not be used.
7. **16-bit PNGs stay 16-bit.** Converting to 8-bit before oxipng would reach ~35 KB on
   the poster, but it is a precision reduction; it needs an explicit choice.
8. GIF/JPEG website parity (gifsicle-style frame optimisation, mozjpeg-style encoding) is
   not implemented; JPEG has quality control and the never-grow rule.
9. The screenshot harness runs against the mock; tools without mock field tables do not
   open a dialog. Real-window WebDriver needs `WebKitWebDriver`, which this install lacks.

## Verification status

- `./scripts/preflight.sh` passes: identity gate, fmt, clippy `-D warnings`, all workspace
  tests (54 core, 81 engine, 17 GUI), node tests for the reader/collector/format helpers,
  UI typecheck/build, clean-checkout builds.
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
- `scripts/ui-shots.py` last run: 30 dialogs across 49 tool states, no layout failures,
  preview loaded; 40 PNGs + `report.json` in
  `/home/dany/Downloads/Testing Files/screenshots/`.

## UI screenshots, without the desktop

```bash
./scripts/ui-shots.py --out "/path/to/shots"     # --build rebuilds the bundle first
```

Headless Firefox drives the built front end through WebDriver (Marionette): it opens every
tool in every category, writes `<category>-main.png` and `<category>-<tool>.png`, plus
`report.json` with field labels, control values and modal/preview rectangles. It fails on
a modal larger than the viewport, a field overflowing the modal, or a preview image that
does not load. It runs against the **mock**, so it exercises rendering and field logic,
not the Rust engine; engine behaviour is covered by the tests and the CLI. Tools whose
mock field table is missing run without a dialog (the report marks `"dialog": false`).

## Test assets (outside the repository)

`/home/dany/Downloads/Testing Files/` holds one file per format for manual testing,
generated with ImageMagick/FFmpeg/espeak/pandoc/groff: images (incl. 48 MP and 63 MP,
transparent, text, SVG), video in six containers, speech in eight audio formats, a 5-page
text PDF and a 3-page scan, DOCX/ODT/HTML/MD, PPTX, XLSX/ODS/CSV, EPUB, SRT/VTT, archives
with hostile names, and a `hostile/` set. `screenshots/` holds the harness output and one
real-app capture (`real-app-svg-loaded.png`).

## Running it

```bash
npm --prefix ui run build      # required before cargo build: the bundle is embedded
cargo build --release -p convertme-gui
./target/release/convertme
```

Changing `ui/src` without rebuilding the bundle now fails the build script (and a test
compares the embedded page with `ui-dist`).

## Backlog

1. **Lossy PNG "Reduce colours"** (imagequant + indexed PNG writer, off by default) to
   close the gap with compresspng.com; keep the lossless path the default.
2. **GIF/JPEG parity** (gifsicle-style optimisation, mozjpeg-style encoding), measured
   against the same test files.
3. Extend the mock field tables (or generate them from the Rust tables) so the screenshot
   harness covers every dialog instead of 31 of 49 tools.
4. **Crop** — the next image tool to bring to the same standard (live preview of the kept
   area is started via `CropEditor`; check handles and multi-file behaviour).
5. Edit photo, Annotate, Redact, Metadata — the rest of the image row.
6. Other categories (documents, slides, spreadsheets, EPUB, subtitles, archives).
7. Performance pass per the plan's Step 0 budgets, including tiled composition.
