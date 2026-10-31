# Handoff

ConvertMe is a local Tauri app: a wheel picks a category, a format and a tool; the tool
opens **one dialog sheet**; the job runs through the engines crate and publishes beside
the source. 307 conversion options, 31 backend tools, everything offline.

This file is the state of the product and the protocol for changing it. Read it first,
then `AGENTS.md` (rules) and `docs/LESSONS.md` (the failures that produced the rules;
42–56 are the alignment session, 57–63 the first bug round).

## Start here (new session)

| | |
|---|---|
| Branch | `master` |
| Last code commit | `HEAD` — "restore the field grammar and make previews play" (Round 3) |
| Previous code commit | `a57b948` — "the dialog is one movable sheet, and the controls fit their rows" |
| Revert point | branch + annotated tag `savepoint/pre-alignment-2026-10-31` |
| Proof | `artifacts/round3/ui` (layout), `artifacts/round3/app/*` (targeted real-window scenarios), `artifacts/final5/audit` (CLI matrix), `artifacts/perf/run4` (perf) |

Nothing is pushed; CI runs on push to `master` and PRs, and merge commits carry
`[skip ci]`.

First three commands, in this order:

```bash
npm --prefix ui run build && cargo build --release -p convertme-gui   # bundle first, always
./scripts/preflight.sh                                               # gate, fmt, clippy, tests, clean build
./scripts/app-e2e.py --only dialogs-images                           # the real window, ~30 s
```

A full `./scripts/app-e2e.py` takes ~16 minutes and is the acceptance run.

## Invariants a change must not break

1. **One sheet, not a takeover.** `.dialog-sheet` is a flex column (head → file line →
   optional preview band → scrolling fields → footer), 600 px wide, dragged by its head,
   resized by its corner, closed from the top right. The preview band is a sibling of the
   scroller, never an item inside it.
2. **Rows are as tall as their content.** `.field { align-self: start }` and a flex-column
   `.modal-fields`: a stretched grid row let a colour strip or the gradient editor paint
   over the next row. The harness fails on overlaps.
3. **The preview is the engine's output.** A preview that is a lookalike is a second
   implementation waiting to disagree (`docs/LESSONS.md` 31). Photo/collage/audio
   previews run the real tool and cache the artifact.
4. **The web view decodes no media.** WebKitGTK's media stack crashed the process on
   `Play`; the video transport walks the filmstrip, and an audio preview is written to the
   cache and handed to the desktop player (`play_preview`).
5. **One field table.** `commands.rs::fields_for` is the only description of a tool's
   controls; `ui/src/fields.generated.json` is generated from it and a test fails when it
   is stale. An editor that owns a value marks the field `hidden` (the value is still
   collected, so there is never a second control for it).
6. **A control that shows pixels measures the picture.** Crop/annotation/redaction
   overlays live on a frame that is exactly the rendered photo; the container may be
   letterboxed.

## Savepoint and branches

| Ref | What it is |
|---|---|
| `savepoint/pre-alignment-2026-10-31` | the product before the alignment session; the revert point |
| `work/alignment-foundation` | dialog shell, field grammar, field kinds, dialog commands, harness foundation (merged) |
| `work/images-tools`, `work/video-audio-tools` | the Images and Video/Audio phases (merged) |
| `master` | the merged product plus the first bug round |

## Where things stand

**Dialogs.** One 600 px sheet with the file line, a pinned preview band for the image
tools (compress, crop, redact, annotate, background, edit photo, collage), the scrolling
settings, and a footer with `Reset` and the tool's own action ("Save Organized PDF",
"Create Collage"). `ui-shots.py` measures the geometry and **overlaps**; `app-e2e.py`
measures the same in the real window and drags the head (`dialog-moves`).

**Fields.** `commands.rs::fields_for` — number/text/range/select/segmented/colour/
gradient/checkbox, plus `boxes`, `pages`, `files`, `metadata`, `shapes` for surfaces.
Every editor with its own chrome hides its field. Table changes go through the Rust test
`generated_fields` + `npm build` (see `AGENTS.md`).

**PDF.** `Organize pages` duplicates, rotates, removes, reorders, matches widths and
submits `order`/`rotate`; duplicated pages are cloned objects when qpdf refuses a repeated
page spec. `Merge` reorders the selection. `PDF → DOCX` keeps structure (7 Heading 1,
4 Heading 2, 29 list paragraphs, 29 `w:numPr`, 14 drawings, 0 page breaks, coverage 1.0);
`PDF → PPTX` follows the page aspect, `XLSX` writes one sheet per page, `EPUB` shares the
DOCX reflow analyser.

**Engines.** Every external command goes through `proc.rs` (argv, cache limits, watchdog,
bounded stderr). Metadata writes/reads one vocabulary per format (`editable_tags`);
PNG text chunks are read (`comment` → `Title`).

**Harnesses.**

| Layer | What it proves | Command |
|---|---|---|
| `ui-shots.py` | layout, dialog contract, overlaps, crop ratios, against the mock | `./scripts/ui-shots.py --out DIR` |
| `app-e2e.py` | the real window via AT-SPI: real jobs, artifacts, design contract | `./scripts/app-e2e.py --out DIR` |
| `quality-audit.py` | every reachable conversion through the CLI, each artifact validated | `./scripts/quality-audit.py --out DIR` |
| `docx-quality.py` | one DOCX against the structure contract and a reference | `./scripts/docx-quality.py --source PDF --docx OUT --reference REF` |
| `perf-audit.py` | wall time, peak RSS, temp peak, cache bytes, installed size against ceilings | `./scripts/perf-audit.py --baseline artifacts/perf/baseline.json` |

`app-e2e.py --only <substring>` runs one area. The harness runs the app under Xvfb with
its own `HOME`/`XDG_CACHE_HOME`; a scenario that looks for a cached file must read
`os.environ["XDG_CACHE_HOME"]`, not the launcher's profile path.

## Round 2 — the bugs reported from real use

Cause → fix → measurement for each; lessons 57–63.

| Report | Cause | Fix | Proof |
|---|---|---|---|
| "1:1 does not look 1:1", "9:16 looks like 1:1" | the crop box was a fraction of the **letterboxed surface**, not of the picture | overlay on a frame that is exactly the rendered photo; one W/H row inside the crop editor | `crop-1:1` → 1.000, 3024×3024 px; `crop-9:16` → 0.562, 1701×3024 px |
| annotate "does not work" | canvas uncapped and letterboxed; ink was a second unlabelled colour row under it; shapes invisible until Save | shared frame geometry, shapes drawn in SVG, ink only in the toolbar, `shapes`/`color` hidden | `tool-image-annotate`; `images-annotate.png` |
| collage "looks bugged" | preview scrolled with the settings; a `<details>` repeated the field's own label | preview pinned above the scroller; one "Image order" row | `images-collage.png` |
| "don't like that it is full screen … better when we could move and resize it" | the reference's side rail had been implemented literally (it was macOS annotation) | rail deleted; draggable head (double-click re-centres), corner grip, × top-right | `dialog-moves` moves the sheet by 40, 24 and re-centres it |
| add background "broke" | a tall control (gradient editor, colour strip) was stretched into a 40 px row and painted over the next row | flex-column field list; each row sized by content; sheet is a flex column too | overlap probe went from `#101418 / 4` to none across 31 dialogs |
| "make the preview always viewable" | the preview was one item in the scrolling list | it is a band between the file line and the scroller | the band is outside `.modal-fields` |
| metadata "does it fill what is there?" | reader emitted raw EXIF names and never read PNG text chunks; the rows use friendly names | one vocabulary; PNG `tEXt`/`iTXt` read (`comment` → `Title`) | unit test + scenarios reopen the published file and read `Title` |
| white outline on "Search metadata" | WebKit's native search decoration | filled pill, `appearance: none`, ring only on `:focus-visible` | `images-metadata.png` |
| "playing the video crashed the app" | `Play` mounted `<video>`; WebKitGTK's media stack killed the process | no media element; Play walks the filmstrip playhead | `dialogs-video` stays alive |
| "the audio doesn't play" | the preview's work dir was deleted **before** the transcode (always failed); `<audio>` wedged WebKit | transcode before cleanup; command on `spawn_blocking`; file opens in the desktop player | `tool-audio-bleep`: MP3 in cache, 1 kHz tone at the drawn range, dialog still open |
| timecodes untypable in a redaction row | the row list was keyed by identity: every keystroke rebuilt the focused `<input>` | `<Index>` rows; number inputs commit on change | `tool-video-redact` types `0.5`, frames change inside 0.5–2.0 s |
| box spacing | 4 px gaps around 18 px swatches | 6 px gaps around 20 px swatches | `images-collage.png` |

## Round 3 — the design regression and the media path

Cause → fix → measurement; lessons 64–70.

| Report | Cause | Fix | Proof |
|---|---|---|---|
| "cannot resize vertically", "boxes small, some full width" | `.field { align-self: start }` in the flex-column field list: cross-axis alignment shrink-wrapped every row (a 199 px select, a 129 px number box, a 552 px slider) | field grammar restored to the product's stacked form: label above a full-width control; selects share the text-input frame; segmented controls are one equal-cell grid | `ui-shots.py`: 31 dialogs, no failures; the contract now asserts ≥ 90 % row fill and left alignment |
| "dropdown is a pill and far from Image size", "aspect ratio is one big pill" | label-left 34 % column plus a pill `<select>` | stacked labels, rectangular select, equal-width aspect cells | crop is 7 × 76 px cells in a 552 px row; `images-add-background` screenshot |
| background presets stacked on the left | the one-row preset markup had been split into two groups | one row: solids \| separator \| gradients \| gap \| image button | `images-add-background` |
| dialog resized only a little vertically | invisible corner grip and a hard 780 px height ceiling | visible corner and bottom/right grips, `94vh` ceiling, app window minimum 720 × 480 | a drag grew the sheet from 622.5 to 740 px |
| "video does not play", "speed does nothing" | Play moved a playhead over a static strip; the speed field never reached the preview | `video_frames` samples the clip (10 fps, 240-frame budget); Play advances the still by the speed factor; trim bounds and the readout divider are in the transport | 6 s clip: 60 frames in 0.55 s, cached; mock Play moved frame 1 → 2 at 0.40 s, 4× reached 1.6 s in 0.4 s |
| "Trim start/end drag the playhead" | the handle setters called `seek` | handles only change the range; seeking/Play clamp into it; the cursor may sit outside until moved | trim start 2.0 left the cursor at 0; Play started at 2.0 |
| crop video had two previews | a second `Step through` timeline under the crop canvas | removed; the crop canvas is the only surface | `previews: 1`, `timelines: 0` |
| "audio opens another app", "app crashed playing Trim audio" | `<audio>` in `WaveformTimeline` and `preview.tsx`; `play_preview` called `xdg-open` | no media element anywhere; `play_preview` uses a headless sink; preview window plays video as stills and audio through the sink | `grep` clean; `tool-audio-trim` presses Play and the dialog stays open |
| visualizer background did nothing | the preview never read `color` | frame background from `values.color`, wave ink chosen by luminance | light `#e6e6f2` → dark ink, dark `#101014` → white |
| metadata fields empty | PDF `/Info` indirect; EXIF ASCII values quoted; PNG Description collapsed into Title; a fresh JPEG has no EXIF block | dereference/decode PDF Info; read EXIF bytes unquoted; separate Description; start a new EXIF block when none exists | new Rust tests round-trip all writable image tags and six PDF Info keys; `tool-image-metadata` shows the existing Title |
| 2 PDFs, 1 card, black preview; "1 pages" | ImageMagick's PDF decode; `analyze` dropped failed thumbs; `PageOrganizer` loaded on mount only | PDF cover through `pdf_pages`; one card per readable file; path-keyed page load with a stale token; `pdf-pages` added to the CLI verb list | `tool-pdf-organize-multi`: both names, 5 page cards then 3 after Next file; CLI `pdf-pages` prints 5 |

### Round 3 measurement

| Layer | Result |
|---|---|
| `app-e2e.py` | **65 passed, 0 failed, 6 skipped** in 71 scenarios (`artifacts/round3/app/full`) |
| `ui-shots.py` | **31 dialogs of 53 tools, no layout failures** (`artifacts/round3/ui`) |
| `preflight.sh` | ok: gate, fmt, UI typecheck+tests+build, generated-field check, clippy `-D warnings`, workspace tests, clean build, deb 112 MiB |
| Targeted | metadata prefill, PDF cover + page switch, all video/audio tools, `tool-audio-trim` Play, CLI `frames` (60 stills in 0.55 s) and `pdf-pages` (5) |

The six skips are the five categories with no backend tools and the LibreOffice-dependent
slides→PDF conversion, unchanged from the previous handoff.

## Numbers from the final run

| Layer | Result |
|---|---|
| `app-e2e.py` | **64 passed, 0 failed, 6 skipped** in 70 scenarios (`artifacts/final5/app`) |
| `ui-shots.py` | **31 dialogs of 53 tools, no layout failures** (`artifacts/final3/ui`) |
| `quality-audit.py` | **230 passed, 0 failed, 52 skipped** (`artifacts/final5/audit`) |
| `docx-quality.py` | coverage 1.0; 7/4/29/29/14/0 against a reference of 8/4/30/29/12 |
| `preflight.sh` | ok: gate, fmt, UI typecheck+tests+build, generated-field check, clippy `-D warnings`, workspace tests, clean-checkout build, deb 112 MiB |

The six skips are five categories with no backend tools (Documents, Slides,
Spreadsheets, EPUB, Subtitles) and the LibreOffice-dependent slides→PDF conversion. Every
scenario that was skipped for a harness gap in the previous handoff now **passes**:
video-redaction ranges, bleep ranges + preview, page-organizer rotation (it was a broken
`qpdf --show-pages --json` probe, not a product bug), and `dialog-moves` is new.

Per-group detail (`artifacts/final5/app/report.json`): compress 11/11; tool-image 7/7;
tool-video 7/7; tool-pdf 4/4; tool-audio 6/6; convert 10/10 + 1 skip (LibreOffice);
multi-file/never-grow/reachable-target/immediate-tools/extract 6/6; negative 5/5;
persistence/build-identity/dialog-moves 3/3.

### Performance (`artifacts/perf/run4`)

Measured before this round; no hot path changed except the audio preview (now a
`spawn_blocking` transcode). `artifacts/perf/baseline.json` is the 15 % regression gate.

| Case | Measured | Ceiling |
|---|---|---|
| cold start, p95 | 343 ms | 600 ms |
| wheel repeat latency, p95 | 26.8 ms | 50 ms |
| idle | 0.03 % CPU, 0.00 own wakeups/s | 0.2 % / 0.5 |
| thumbnail 4000×3000 → 512 px | 203 ms | 300 ms |
| PDF page grid, 5 pages | 60 ms | 2 s |
| waveform, 30 s | 109 ms | 300 ms |
| filmstrip, 12 frames | 255 ms | 1.5 s |
| photo preview, 4000×3000 at 1024 px | 211 ms | 700 ms |
| compress preview, poster defaults | 2,968 ms | 3 s (measured) |
| compress to 1,000 B, unreachable | 2,877 ms | 5 s (was 13,445 ms) |
| background, 63 MP | 3,514 ms, 1.46 GiB peak | 10 s, 1.5 GiB |
| PDF organize, 5 pages | 574 ms | 2 s |
| PDF → DOCX, résumé | 1,442 ms | 10 s |
| video trim, 10 s re-encode | 555 ms | 6 s |
| normalize, 30 s | 2,979 ms | 4 s |
| installed bundle | 281 MB | 320 MB |

## How to verify a change

1. Build the bundle before the binary: `npm --prefix ui run build && cargo build --release -p convertme-gui`.
   The build script fails on a stale bundle; a test fails if the binary embedded a
   different page.
2. One area: `./scripts/app-e2e.py --only <scenario substring>`; the full run before a
   merge. A UI change also runs `./scripts/ui-shots.py --out DIR` (layout, overlaps,
   ratios) and a field-table change runs the generator test.
3. An engine change is verified by sampling the artifact (dimensions, corner pixels,
   alpha, streams, pages), not by the exit code. `quality-audit.py` does this for the
   conversion matrix.
4. `./scripts/preflight.sh` before declaring anything done. If a scenario fails, replay
   the exact fixture and parameters; a screenshot alone proves neither layout nor
   behaviour (it is scaled and cropped by the window manager).

## Open items

1. **The normalize input readout** renders ("Input: −23.4 LUFS", visible in the mock
   screenshot) and the measurement is unit-tested, but this WebKit build does not expose
   that row in the real window's accessibility tree; the scenario records it instead of
   asserting it.
2. **Compress preview is 2.5 s** on a 6 MP poster because the preview *is* the artifact
   and oxipng's lossless passes cost that; the dialog debounces 400 ms and renders off
   the UI thread. The ceiling is 3 s, measured rather than estimated.
3. **The Windows sidecar binary was not rebuilt** here. `sidecars/pdf/build.sh` changed
   (pdf2docx removed from the pip line); rebuild it in the release matrix.
4. **AVIF/HEIC** cannot be decoded by the ImageMagick build on this machine (no
   `heic.la` coder module), so those pairs skip in the audit. The capability probe
   records the feature flags; the wheel does not yet mark them unavailable.
5. **Audio playback is delegated.** The window never decodes audio; the preview file
   opens in the desktop player. In-app playback would need a working sink and a fallback
   that cannot wedge the web process (lesson 61).
6. **Perf was not re-measured this round.** The only touched hot path is the audio
   preview; re-run `perf-audit.py --baseline artifacts/perf/baseline.json` when a change
   touches previews, the queue or startup.
7. **Video playback is a still sequence, not a decoded stream.** `video_frames` samples
   at 10 fps for short clips and within a 240-frame budget for long ones (a clip over
   4 minutes plays at 1 fps). The web view must not mount `<video>` (lesson 61/66);
   a smoother player needs a proven out-of-process sink.
8. **Audio playback is delegated to a headless system player.** It stays in the app's
   lifetime (no desktop window), but there is no pause/seek: the command resolves when
   the sound ends. `play_preview` probes `ffplay`, `mpv`, `paplay`, `pw-play`, `aplay`
   and falls back to the desktop opener only when none exists.

## Traps specific to this codebase

- `qpdf 12` rejects `--json` together with a positional path (and does not put `/Rotate`
  in the page keys): read rotation with `pdfinfo -f 1 -l 1` (`Page 1 rot: 90`).
- The app's cache is `$XDG_CACHE_HOME/convertme/previews`; a harness scenario must read
  the environment, not the launcher's profile path.
- WebKit rebuilds the accessibility tree when a preview changes the DOM: the harness
  re-locates nodes on every attempt (`set_entry`, `press_primary`), and a click that used
  pre-shift extents lands elsewhere.
- The mock is not the app: it proves layout and the reader path, never the engine.
