# Handoff

State of the project after the real-app harness session: `add background` is complete,
**Compress is feature-complete across images, video, audio and PDF**, and
`scripts/app-e2e.py` now drives the release binary under Xvfb, runs a real job for
**every backend tool**, and validates the published artifacts. The mock screenshot
harness remains the fast render check. One engine bug is open (never-grow compares
before PNG optimization; see "First-run failures"); four broken surfaces found while
extending coverage are fixed. Read this first, then `AGENTS.md` (rules) and
`docs/LESSONS.md` (why the rules exist).

## Where things stand

`add background` and `compress` are feature-complete and verified. The image, video,
audio, PDF and archive tools each have a real job in the harness; four broken surfaces
found while extending coverage are fixed (Metadata fields, crop aspect submit, Redact
boxes editor, video filmstrip). Documents, Slides, Spreadsheets, EPUB and Subtitles
still have conversions but no backend tools.

`scripts/app-e2e.py` drives the real window through AT-SPI, runs real jobs and validates
the published artifacts. Current run: 65 scenarios, 58 passed, 1 failed (the open
never-grow ordering bug), 6 skipped with reasons. Conversions are one representative
pair per category, not the full 307-option matrix. The wheel, drag layer, queue, history
and settings exist; the GUI is usable end to end on this machine.

## What "compress" does now

| Control | Behaviour |
|---|---|
| Quality | encoder fidelity for lossy formats (100 = maximum fidelity, which can be *larger* than the source); hidden when every selected image is lossless |
| Lossless files | PNG: zlib 9 plus oxipng (row filters + deflate), measured 96.9→81.9 KB on the 16-bit poster, 112.1→67.6 KB on a screenshot, 44.5→35.6 KB on an 8-bit gradient; TIFF: LZW (5.76→3.08 MB); BMP: nothing to ask for. Pixels stay identical |
| Never larger | if the encode would grow the file, the result is a copy of the source (same rule as video/audio/PDF); the preview shows “no change” |
| Image size | 100 / 75 / 50 / 25 % of the dimensions |
| Reduce colours (PNG only) | opt-in, **off by default**; libimagequant palette reduction to 256/128/64/32/16 colours, written as an indexed PNG. Dithers when the result is smaller than the lossless encode, retries flat when it is not, and the never-grow rule still returns the original if both lose |
| Target file size | quality search, then up to three downscale rounds. PNG uses imagequant at the chosen palette (or 256) inside the search and keeps the lossless candidate too; the smallest result when the target is unreachable, and the dialog says "out of reach" instead of silently missing it |
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
| PNG parity with the website stalled at 81.9 KB | the site used a 177-colour palette; we only did lossless | add opt-in "Reduce colours" with libimagequant + our own indexed PNG writer | 96,910 → 16,955 B at ≤256 colours, 16,802 B at 64 (2,000×3,000 16-bit poster), pixels allowed to change |
| Choosing 16 colours returned the lossless file (81,917 B) | dithering a 16-colour poster made the indexed stream larger than the source; the never-grow rule then copied the source | retry the remap with dithering 0 and keep the smaller palette artifact | 81,917 → 11,083 B at 16 colours, colour type 3, palette 16 |
| Target 1,000 B produced 42 KB; "both options" could be larger than reduce-only | the target search used ImageMagick `-colors` and only quantized after picking a downscaled result | PNG target ladder uses imagequant starting at the chosen palette, measures the lossless candidate, downscales and repeats; the dialog reports an unreachable target | target 1,000 B → 1,597 B (1,445×2,168, 2 colours), target-only and 256+target byte-identical; 16+target also 1,597 B |
| No test combined target + palette at all | the target search ran first with ImageMagick's quantizer, then the palette step ran on the chosen result; the cache key did not change either | one `TargetSearch` handles both, the cache is versioned v5, and a test pins preview == job | `target_and_colours_are_searched_together`: 1,597 B, indexed, job bytes equal to the preview |

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
  unreliable under relatime/noatime. Its key is versioned (`v5`); bump it when encoder
  output changes.
- PNG optimisation runs the `oxipng` crate in-process (MIT; preset 2, 4 s timeout, 64 MB
  cap); TIFF asks ImageMagick for LZW. Neither strips metadata.
- "Reduce colours" runs `imagequant` in-process (GPL-3.0-or-later, the libimagequant
  algorithm behind pngquant) at speed 4, gated on a `colours` parameter (2–256, absent =
  lossless). It dithers at 1.0; when that artifact is not smaller than the lossless
  encode it retries at 0.0 and keeps the smaller result. The indexed stream is written
  by the `png` crate with an explicit palette, tRNS for partial alpha and an sRGB chunk;
  oxipng then does the row filters/deflate. Single-threaded, so the same request
  produces the same artifact. `colours` is only accepted for a PNG output; the dialog
  hides the controls for any other selection. The cache is **v5**: the target path and
  the flat fallback changed the bytes for requests that already existed.
- `TargetSearch` is one search for target size and palette together: it measures the
  lossless candidate, then (PNG) quantizes with imagequant starting at `colours` or 256
  and halves the palette down to 2, then downscales 85 % and repeats. The first result
  under the target wins; otherwise the smallest is returned. The dialog compares the
  previewed bytes with `target_bytes` and says "The target of X is out of reach; this is
  the smallest result." A search for 1,000 B on the poster takes ~12 s (six megapixels,
  eight palette levels, two downscales); reachable targets stop early and are much
  faster.
- The palette result runs **before** `prefer_smaller`, so a genuinely smaller lossy
  result is kept while anything that grew still falls back to the source, as lesson 24
  requires. `produce_compress` is shared by the preview and the job; target paths skip
  the second palette pass because `TargetSearch` already quantized.

## Known limits / next performance work (deliberately not done)

1. **Shadow above 60 Mpx is dropped, not tiled.** Tiling the shadow (or compositing it per
   tile) is the real fix; it needs a streaming composition path.
2. **Very large canvases rely on disk spill.** A 195 Mpx input composes in ~36 s under the
   modest budget, but only when the temp filesystem has room. A free-space pre-check for
   the *temp* dir (not just the output) is missing.
3. **Shadow cost is still the dominant term** for big canvases (several seconds).
4. The installed size and cold-start budgets from the plan have not been re-measured since
   the UI work.
5. Compress target-size previews re-run the whole search on every parameter change. It
   is debounced and cached, and a reachable target stops at the first fitting palette;
   an unreachable one (1,000 B on the 6 MP poster) walks every palette level and costs
   ~12 s. A cheap floor estimate could skip that walk.
6. **The target ladder stops at two 85 % downscales.** A 1,000 B target on the poster
   lands at 1,597 B with a 2-colour palette at 1,445×2,168; reaching 1,000 B needs a
   deeper downscale. The dialog now states that the target is out of reach rather than
   implying it was met.
7. **The lossless path still keeps 16-bit PNGs 16-bit.** "Reduce colours" converts to
   8-bit as part of the palette step, which is why it is lossy; a lossless 16→8-bit
   choice would be a separate precision reduction and is not offered.
8. GIF/JPEG website parity (gifsicle-style frame optimisation, mozjpeg-style encoding) is
   not implemented; JPEG has quality control and the never-grow rule.
9. `scripts/ui-shots.py` still runs against the mock: it proves rendering, fields and
   layout, not the engine. The real-app harness (`scripts/app-e2e.py`) covers the engine
   through the window; WebDriver remains absent, so it uses AT-SPI + Xvfb. Tools without
   backend field tables still show inert/fallback cards (see the harness findings).

## Verification status

- `./scripts/preflight.sh` passes: identity gate, fmt, clippy `-D warnings`, all workspace
  tests (58 core, 89 engine, 18 GUI), node tests for the reader/collector/format helpers,
  UI typecheck/build, clean-checkout builds.
- Reduce-colours tests pin: a 16-colour PNG is smaller than the lossless encode, is
  colour type 3 with ≤16 palette entries, keeps its dimensions and changes pixels; a
  smooth gradient at 16 colours beats the lossless file (the flat-remap fallback), not
  just returns the source; a transparent corner stays transparent and an opaque disc
  stays opaque (the tRNS path); a JPEG output with `colours` is refused rather than
  written as a palette PNG; the switch is a control, the palette size is shown only
  while it is on, and the collector sends no `colours` for the default lossless request.
- Target + palette tests pin: the target search downscales and quantizes together, is
  no larger than palette-only or target-only, produces a palette PNG, and its preview
  bytes equal the published job bytes.
- Measured on the 2,000×3,000 16-bit poster: lossless 81,917 B; 16 colours 11,083 B
  (palette 16); target 1,000 B → 1,597 B at 1,445×2,168 with 2 colours; 256+target and
  16+target both 1,597 B; the target is reported as out of reach.
- The screenshot harness now ticks both options and sets a 1,000 B target: the Colours
  select appears, the preview reports 4032×3024 · 8.8 KB · −99 %, the note reads "The
  target of 1000 B is out of reach; this is the smallest result.", modal 640×786 in an
  854 px viewport, no overflow.
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
- `scripts/ui-shots.py` last run: 31 dialogs across 50 tool states, no layout failures,
  preview loaded; 41 PNGs + `report.json` (the extra dialog is Compress with target +
  palette on, including the out-of-reach note).
- `scripts/app-e2e.py` (release binary, isolated profile, AT-SPI): **65 scenarios, 58
  passed / 1 failed / 6 skipped** on 2026-10-30. The failure is the multi-file 16-bit
  scale/never-grow ordering bug (documented above). Every backend tool has at least one
  real job. The poster cases pass exactly: 1,597 B at 1,445×2,168, colour type 3,
  palette 2, out-of-reach note present; 16 colours alone 11,083 B; reachable target
  22,701 B. Every scenario records a screenshot plus `report.json` (build chain,
  parameters, a11y snapshot, artifact stat, validator output, checks); published
  artifacts are copied to `artifacts/<scenario>/`. Final report:
  `artifacts/app-e2e/final/report.json`.

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
It cannot prove that a button runs a job, that the preview's size is the published
artifact's size, or that a 1,000 B target produced a valid, smaller file. That is the
next task.

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

## Real-app end-to-end harness (built 2026-10-30)

`scripts/app-e2e.py` is the second testing layer. It launches
`target/release/convertme` under Xvfb + `dbus-run-session` with a throwaway
HOME/XDG profile, drives the real window through AT-SPI, runs real jobs and
validates the published artifacts. `scripts/ui-shots.py` stays the fast mock
render check; the two are not substitutes. Every backend tool now has at least
one real job.

### Running it

```bash
npm --prefix ui run build && cargo build --release -p convertme-gui
./scripts/app-e2e.py                    # artifacts/app-e2e/<stamp>/{report.json,screenshots,logs}
./scripts/app-e2e.py --build            # do both builds first
./scripts/app-e2e.py --only compress    # substring filter
./scripts/app-e2e.py --list
```

Exit code 1 if any check failed. Skips are printed and stored with a reason.
The 2026-10-30 run has **65 scenarios: 58 passed, 1 failed, 6 skipped**. The
failure is the one open engine bug (see "First-run failures"). Published
artifacts are copied into `<out>/artifacts/<scenario>/`, so the report outlives
the throwaway profile.

### Isolation and environment

- HOME, XDG_DATA_HOME, XDG_CACHE_HOME, XDG_CONFIG_HOME, XDG_RUNTIME_DIR and
  TMPDIR live under a temp profile. Inputs are copied into a per-scenario work
  directory, so fixtures, jobs, settings and the preview cache are never
  touched.
- Xvfb on a free display, 1400×1000×24; `dbus-run-session`;
  `AT_SPI_BUS_ADDRESS` from `org.a11y.Bus.GetAddress`;
  `/usr/lib/at-spi2-registryd` started by hand.
- App env: `NO_AT_BRIDGE=0 GTK_A11Y=1 GDK_BACKEND=x11
  WEBKIT_DISABLE_SANDBOX_THIS_IS_DANGEROUS=1
  WEBKIT_DISABLE_DMABUF_RENDERER=1`. Do **not** set
  `WEBKIT_DISABLE_COMPOSITING_MODE=1`: the main window renders, but opening a
  modal turns the window blank grey, so screenshots and clicks fail.
- `~/.local/opt/<tool>` roots are symlinked into the profile because
  `target/release/qpdf` is a wrapper that execs
  `$HOME/.local/opt/qpdf/bin/qpdf`. Without the link, PDF merge/split fail only
  under the harness ("No such file or directory").
- Screenshots use `import -window <visible ConvertMe>`; dialog/frame extents
  come from AT-SPI, so no window position is assumed.

### Driving the real window

- Tool cards are **named push buttons** (`"01 Compress Hit a target size…"`);
  the unnamed node is the wrapping list item. Press the button through the
  AT-SPI action interface. Coordinate clicks are a fallback inside Xvfb only.
- Native selects open a popup. Focus the combo, click, send Up/Down and Return,
  then verify the selected menu item.
- Entries are focused, cleared with `ctrl+a` and typed with xdotool; the value
  is read back. In PyGObject `Accessible.get_text()` is a deprecated interface
  accessor, not the text; call `Atspi.Text.get_text(iface, 0, count)`.
- **Background content is not exposed while a modal is open.** Capture history
  rows before opening the dialog; the Activity panel behind the modal is
  invisible to AT-SPI.
- Launching with a file does not mean the category and target list have
  settled. Toggle the category, wait for the card, and re-read targets after
  analysis (the fallback target list is replaced).
- Zero-field tools start a job on the card press. Snapshot the work directory
  *before* pressing anything that may run.

### Build identity

The harness computes the UI stamp with build.rs's algorithm from `ui-dist`,
asserts the stamp bytes are inside the release binary, reads the interface
build id from Settings, and compares `/proc/<pid>/exe` with the launched
binary. All four agree on a good build. The bundle is compressed inside the
binary, so grepping for "Interface build" finds nothing; search the stamp.

### Coverage

- build identity; every category's cards; **all 31 backend tools have a real job**:
  image crop (aspect), annotate, add background, edit photo, metadata, redact (OCR or a
  drawn region); video trim, crop, speed, save frames (10 PNGs), split (2 clips),
  redact; audio normalize, trim, channels, visualize, bleep, metadata; PDF organize,
  metadata, save pages (5 PNGs); archive extract; plus the compress families and the
  zero-field mute/join/merge/split/extract jobs. Fallback categories are skips with the
  observed behaviour.
- compress: poster target+palette (1,597 B, 1,445×2,168, colour type 3, palette
  2), 16 colours alone (11,083 B), target+16, reachable target (22,701 B),
  lossless pixels (AE 0), alpha, metadata, scale, JPEG never-grow, video/audio/
  PDF compress with ffprobe/pdfinfo, tiny never-grow, multi-file shared and
  per-file.
- conversions: image, video, audio, PDF→TXT, MD→DOCX, XLSX→CSV, EPUB→MD,
  SRT→VTT, ZIP→TAR; archive Extract; zero-field tools (mute, join, merge,
  split, extract text).
- negatives: corrupt input, missing ImageMagick (sidecar-less binary + empty
  PATH), hostile filename, cancel mid-job, never-overwrite; persistence of
  history and the overwrite setting across a restart.

### First-run failures

| Scenario | Symptom | Cause | Status |
|---|---|---|---|
| multi-file | Shared "Same settings" + 50 % scaled the poster but returned the 16-bit screenshot at 1600×1000 | `produce_compress` calls `prefer_smaller` **before** `optimize_png`: the resized encoder output (122,328 B) exceeded the source (112,129 B), so it copied the source, which oxipng then shrank to 67,606 B. The optimized resize would be 96,639 B | open |

The first coverage pass also exposed four broken surfaces, fixed in this session
and pinned by the new scenarios:

- **Images → Metadata had no field table.** The card ran a job with `{}` and
  failed "nothing to change". It now exposes the tags both image writers accept
  (Title/Description/Artist/Copyright) plus "Remove all tags";
  `tool-image-metadata` writes Title and checks pixels and the comment.
- **Choosing a crop aspect sent both a rectangle and a ratio.** The editor fits
  the preview box to the ratio, and `submitModal` forwarded both, so every
  aspect crop was rejected ("use either ... not both"). Aspect crops now send
  the ratio only; `tool-image-crop` pins 1:1 → 2,000×2,000.
- **Redact had no region editor.** `boxes` was filtered out of the generic
  field list and only Crop had a dedicated editor, so Redact could never get a
  box. The shared `BoxesEditor` is now rendered for Redact (images and video),
  with its Detect faces/text and drag tools.
- **The filmstrip command used `-vsync`, removed in ffmpeg 9.** Video previews
  were "unavailable" and the Redact canvas could not be used. The option is
  gone, and `preview_thumb` asks the filmstrip for a video frame directly
  instead of letting ImageMagick fail first.

Recorded skips: Documents, Slides, Spreadsheets, EPUB and Subtitles have no
backend tools; the UI shows `FALLBACK_TOOLS` cards with no field table or
engine. Slides → PDF is disabled because LibreOffice is absent. Every skip in
`report.json` carries the reason and the observed card.

Also observed, not failed by the matrix: a corrupt PNG is counted as "1 file
ready" by analysis; the preview and the job then fail with
`engine_failed: magick … bad header`, and no artifact is published. The
scenario pins the clean failure, not the missing early warning.

### What it cannot prove

- Native file pickers are not clicked; paths are seeded on the command line.
- Free crop handle drag and background placement/zoom drag are not driven; crop uses
  the fixed-ratio branch and redaction uses OCR or a drawn rectangle. The wheel is not
  driven by this harness.
- Conversions are one representative pair per category (image, video, audio, PDF→TXT,
  MD→DOCX, XLSX→CSV, EPUB→MD, SRT→VTT, ZIP→TAR), not the 307-option matrix.
- The mock render check (`ui-shots.py`) remains the only fast layout check.

## Next session: start here

1. Fix the open engine bug: `produce_compress` decides never-grow before
   `optimize_png`, so a legitimate resize can be discarded (the `multi-file` scenario
   fails with 67,606 B at 1600×1000 instead of the 96,639 B 800×500 resize). Optimize
   before comparing, or compare the final bytes, and add an engine test.
2. Rebuild and re-run:
   `npm --prefix ui run build && cargo build --release -p convertme-gui`,
   `./scripts/app-e2e.py`, `./scripts/ui-shots.py`, then `./scripts/preflight.sh`.
   The suite must go to 59 passed / 0 failed / 6 skipped; do not weaken the scenario.
3. If the goal is broader app coverage, extend conversions from one pair per category
   to the registry matrix (307 pairs) with the same harness, and implement tools for the
   five categories that still show fallback cards.

Read the harness section below for the AT-SPI recipe, the environment gotchas
(`WEBKIT_DISABLE_COMPOSITING_MODE` blanks modals; `qpdf` is a `$HOME/.local/opt`
wrapper; `-vsync` is gone in ffmpeg 9) and the report format.

## Backlog

1. **Fix the remaining harness finding**: make the never-grow comparison use the
   optimized artifact (`produce_compress`: optimize before `prefer_smaller`, or compare
   the final bytes). Add an engine test for it (the `multi-file` scenario is the
   regression).
2. **GIF/JPEG parity** (gifsicle-style optimisation, mozjpeg-style encoding), measured
   against the same test files.
3. Extend the mock field tables (or generate them from the Rust tables) so the screenshot
   harness covers every dialog instead of 31 of 50 tool states.
4. **Crop free-hand editing** — the aspect branch and the resulting job are covered;
   add handle-drag, free rectangle and per-file crop behaviour.
5. Extend conversions from one representative pair per category to the full registry
   matrix (307 options), driven by the same real-app harness.
6. Implement tools for documents, slides, spreadsheets, EPUB, subtitles and archives
   (the UI currently shows fallback cards with no backend).
7. Performance pass per the plan's Step 0 budgets, including tiled composition.
