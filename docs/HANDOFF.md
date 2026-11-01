# Handoff

ConvertMe is a local Tauri app: a wheel picks a category, a format and a tool; the tool
opens **one dialog sheet**; the job runs through the engines crate and publishes beside
the source. 307 conversion options, 31 backend tools, everything offline.

This file is the state of the product and the protocol for changing it. Read it first,
then `AGENTS.md` (rules) and `docs/LESSONS.md` (the failures that produced the rules;
42–56 are the alignment session, 57–63 the first bug round, 64–70 the design/media
regression, 71–82 the image-tools feedback round, 83–101 the interaction, video and audio rounds).

## Start here (new session)

| | |
|---|---|
| Branch | `master` |
| Last code commit | `1f25fb7` — "tools: previews, playback and outputs follow the user" (Rounds 5–8) |
| Previous code commit | `3e8bbc8` — "the image tools match their own previews" (Round 4) |
| Revert point | branch + annotated tag `savepoint/pre-alignment-2026-10-31` |
| Proof | `artifacts/round8/ui-full` (layout), `artifacts/round8/app-full` (real window), `artifacts/round8/targeted` (tool runs); older: `artifacts/round7/*`, `artifacts/round6/*`, `artifacts/round5/*`, `artifacts/final5/audit` (CLI matrix), `artifacts/perf/run4` (perf) |

Pushed to `origin/master`; CI runs on push and PRs, and this session's commits carry
`[skip ci]` (the local preflight + full `ui-shots`/`app-e2e` runs are the proof above).

First three commands, in this order:

```bash
npm --prefix ui run build && cargo build --release -p convertme-gui   # bundle first, always
./scripts/preflight.sh                                               # gate, fmt, clippy, tests, clean build
./scripts/app-e2e.py --only dialogs-images                           # the real window, ~30 s
```

A full `./scripts/app-e2e.py` takes ~16 minutes and is the acceptance run.

## Invariants a change must not break

1. **One sheet, not a takeover.** `.dialog-sheet` is a flex column (head → file line →
   optional preview band → scrolling fields → footer), 600 px by default (up to 920 px
   when resized), dragged by its head, resized from its visible corner and bottom/right
   edges, closed from the top right. The preview band is a sibling of the scroller, never
   an item inside it.
2. **Rows are the stacked form.** The label sits above a full-width control. Height comes
   from `flex: 0 0 auto` on the children; never use `align-self` on `.field` inside the
   flex-column `.modal-fields` (that is the horizontal axis and shrink-wraps every row —
   lesson 64). The harness asserts ≥ 90 % row fill and fails on overlaps.
3. **The preview is the engine's output.** A preview that is a lookalike is a second
   implementation waiting to disagree (`docs/LESSONS.md` 31). Photo/collage/audio
   previews run the real tool and cache the artifact.
4. **The web view decodes no media.** WebKitGTK's media stack crashed the process on
   `Play`; video playback walks the `video_frames` still sequence, and an audio preview
   plays through the headless system sink in `play_preview`. No `<audio>` or `<video>`
   element exists anywhere, including `preview.tsx` (lessons 61, 66).
5. **One field table.** `commands.rs::fields_for` is the only description of a tool's
   controls; `ui/src/fields.generated.json` is generated from it and a test fails when it
   is stale. An editor that owns a value marks the field `hidden` (the value is still
   collected, so there is never a second control for it).
6. **A control that shows pixels measures the picture.** Crop/annotation/redaction
   overlays live on a frame that is exactly the rendered photo; the container may be
   letterboxed.
7. **One edit, one representation.** A crop with a placed box travels as `x/y/w/h`; the
   ratio is only for a job built without the editor. The engine rejects both (lesson 71).
8. **Status is an overlay.** A transient "Rendering…" line in flow moves every control;
   it sits over the preview, and its geometry is sampled by the harness (lesson 77).
9. **The composer and its hit areas share one layout.** `collage_layout` is returned by
   the backend; the preview draws the rectangles it will save, and per-picture pan/zoom
   is keyed by source index (lesson 76).
10. **A tonal slider's direction is a test.** Every sign change in `photo_filter_args`
   must be covered by the ramp test (lesson 72).

## Savepoint and branches

| Ref | What it is |
|---|---|
| `savepoint/pre-alignment-2026-10-31` | the product before the alignment session; the revert point |
| `work/alignment-foundation` | dialog shell, field grammar, field kinds, dialog commands, harness foundation (merged) |
| `work/images-tools`, `work/video-audio-tools` | the Images and Video/Audio phases (merged) |
| `master` | the merged product, rounds 5–8 (interaction, video, audio) |

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

**Image tools.** Crop submits the placed rectangle for every preset (a 9:16 dragged to
the edge stays there). Annotate is an object editor (select/move/resize, eraser, inline
text with size/family, numbered badges centred by MVG gravity); its draft is `keyed`
so it draws during the drag, the selection ring exists only in Select, a Number field
sets the next badge, a typed size goes to 2000 px, arrow heads scale with the stroke
and use round caps, and an inline editor replaces the mark instead of drawing over it. Collage's Style is Grid/Featured
and Rows/Columns always drive the layout; per-picture pan/zoom is keyed by source index
with one backend layout (`dialog::collage_layout`), and a live crop of the selected cell
follows the pointer while the engine render is in flight. Annotate's draft is keyed so
it draws during the drag, the selection ring exists only in Select, and the Number field
sets the next badge. Add background's image preview is
an `<img>` placed by the engine's own cover-window formula (`ui/src/geometry.ts`,
including the cover origin); both blur layers are filtered so no sharp copy shows at the
canvas edge, and the composite is normalised to sRGB so a grayscale backdrop cannot turn
a colour photo white. Edit
photo's tonal directions are pinned by a ramp test; its preview status and the compress
stand-in never move the fields. Redact has a blur-strength control and a real
with/without preview toggle (`dialog::redact_preview`, cached by the full request).
Both new commands are registered in `app.rs`; the field-table changes (collage
`rows`/`spacing_pct`/`radius_pct`, width select; redact `strength` in both categories)
went through `generated_fields`, so `ui/src/fields.generated.json` is current.

**Engines.** Every external command goes through `proc.rs` (argv, cache limits, watchdog,
bounded stderr). Metadata writes/reads one vocabulary per format (`editable_tags`);
PNG text chunks are read (`comment` → `Title`).

**Harnesses.**

| Layer | What it proves | Command |
|---|---|---|
| `ui-shots.py` | layout, dialog contract, overlaps, crop ratios, preview-band stability, annotate/collage interaction, against the mock | `./scripts/ui-shots.py --out DIR` |
| `app-e2e.py` | the real window via AT-SPI: real jobs, artifacts, design contract, crop placement RMSE, annotate drawing, background blur drag, collage layout/preview | `./scripts/app-e2e.py --out DIR` |
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
| white outline on "Search metadata" | WebKit's native search decoration | filled control, `appearance: none`, ring only on `:focus-visible` | `images-metadata.png` |
| "playing the video crashed the app" | `Play` mounted `<video>`; WebKitGTK's media stack killed the process | no media element; Play walks sampled stills (`video_frames` since Round 3) | `dialogs-video` stays alive |
| "the audio doesn't play" | the preview's work dir was deleted **before** the transcode (always failed); `<audio>` wedged WebKit | transcode before cleanup; command on `spawn_blocking`; headless sink (`play_preview` since Round 3) | `tool-audio-bleep`: MP3 in cache, 1 kHz tone at the drawn range, dialog still open |
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

## Round 4 — the image-tools feedback round

Cause → fix → measurement; lessons 71–82. The report was deliberately kept to Images:
each fix is at a seam the other categories share (representations, slider direction,
preview status, preview geometry, harness visibility).

| Report | Cause | Fix | Proof |
|---|---|---|---|
| "crop ignores the selection; presets crop centred" | `submitModal` dropped `boxes` for a locked ratio and sent `aspect`; the engine's aspect path centres | a placed box always travels as `x/y/w/h`; `aspect` is only for a job built without the editor | `tool-image-crop`: 9:16 dragged left → `1688×3000`, RMSE 0.00152 vs the dragged reference, 0.23977 vs centred |
| "Annotate is incomplete: delete does nothing, numbers don't work" | no eraser, numbers needed a drag a click never made, text needed pre-typing, stroke/text sizes were guessed in the overlay | Select with four handles, Eraser, inline text + size/family, click-to-place numbered badges; both sides use source-pixel sizes | `ui-shots.py`: drag → 1 shape, Number → 3 SVG nodes, Eraser → 1; `tool-image-annotate` finds the rectangle and badge in the file |
| "Collage styles don't make sense; no rows; radius broken; limited to 64; can't move images" | pixel spacing/radius capped at 64, no row count, columns read by two styles, a free-text width, no per-picture transform | percentage spacing/radius, independent rows/columns, output-size select, per-picture `{index,zoom,x,y}`, one backend layout for hit areas | 3×2 = six `1024×682` cells; 2×3 = six `682×1024`; 100 % radius shows background at the corner; zoom 2 + pan 100 moves the sample |
| "Add background blur breaks the cursor and the preview" | the preview placed the picture with CSS cover/background-position, then added a full-size CSS blur layer over the drag surface; a grayscale backdrop also switched the composite to Gray and turned the colour photo white | background is an `<img>` placed by the engine's crop formulas; blur is display-scaled, capped, pointer-transparent; one pointer-captured drag; normalize the canvas to sRGB before compositing | `background_blur_changes_only_the_background_layer`: background-only delta up to 128, photo centre `255/0/0`; real drag `10→40 px` with Padding unchanged; `tool-image-background` keeps the photo centre `(237,140,76)` |
| "Highlights/Whites look inverted; Rendering moves the sliders" | four tonal signs were wrong, hue reached ±90°, dehaze ignored its value; the status was a `<p>` in flow | measured directions for every slider; ramp test; status is an absolutely positioned badge | highlights +100: top-20 % 223→243, black 0; whites +100 white 204→255; blacks +100 black 0→51; fields top constant 502, band 369 |
| "Redact: control the blur level, toggle the preview" | blur sigma was a constant; no redacted preview | `strength` 0–100 in the shared settings, same treatment for job and preview; cached engine preview toggled against the thumbnail | variance 0.304 → 0.087 (10) → 0.019 (90); `tool-image-redact` sees the slider and the toggle |
| "Compress takes a while to render" | the frame stayed black until the real encode finished | show the source thumbnail immediately, overlay the encoding status; facts still come only from the artifact, row height reserved | `ui-shots.py`: status seen, `fields_top` 546 and band 364 constant, frame always has a picture |

### Round 4 measurement

| Layer | Result |
|---|---|
| `app-e2e.py` | **65 passed, 0 failed, 6 skipped** in 71 scenarios (`artifacts/round4/app-full`) |
| `ui-shots.py` | **36 dialogs of 58 entries** (53 tools + 5 interaction probes), no layout failures (`artifacts/round4/ui-full`) |
| `preflight.sh` | ok: gate, fmt, UI typecheck+tests+build, generated-field check, clippy `-D warnings`, workspace tests, clean-checkout build, deb 112.29 MiB |
| Targeted | crop 9:16 dragged → `1688×3000` (`RMSE 0.00152` vs dragged, `0.23977` vs centred); annotate rectangle + badge in the file; collage preview + 4 `Cell` hit areas; background blur drag `10→40 px` with Padding unchanged; redact strength + toggle; Compress status with a constant band |

The six skips are the five categories with no backend tools and the LibreOffice-dependent
slides→PDF conversion. `persistence` exposed a harness bug in Round 4 (`ensure_visible`
unpacked a null sheet extent); the fixed harness passes the full run. The background
sRGB normalisation is in lesson 82; the image-engine tests are joined by the ramp test
(lesson 72), the collage layout tests (lesson 76) and the redaction-strength preview test
(lesson 79).

## Round 5 — the interaction round

The report was about the picture tools as lived with: a collage that did not follow the
pointer, a background preview and file that disagreed, annotate that showed its result
only on release, redaction that took minutes, and two controls whose counts were inert.
Cause → fix → measurement; lessons 83–89.

| Report | Cause | Fix | Proof |
|---|---|---|---|
| "Collage only updates after I finish moving" | every pointer move restarted a 200 ms debounce, so no render started until the pointer stopped; the composer then ran at 512 px | latest-wins scheduling (one render in flight, one queued); a live crop of the selected cell drawn from its thumbnail with the composer's own formula (`collageCellBox`), hidden when a render containing the current transforms lands | `ui-shots.py` samples inside the 120 ms window: `.collage-live` present with a non-zero width mid-drag, gone after the render; `tool-image-collage` passes |
| "Rows and Columns do not make sense" | Row/Column styles ignored both counts by construction | Style is Grid/Featured; Rows/Columns always drive the layout (0 = auto, 1 = one line); spare cells stay background; the readout names the grid (`3 × 2 cells`) | 2 pictures: columns 2 → 0.5-wide cells, columns 3 → 0.333, rows 2 → 0.5 high (`ui-shots.py`); engine tests unchanged |
| "Background blur still shows the left part" | the CSS blur fades at the element edge, and an unblurred "bleed" copy underneath showed the sharp picture along the canvas border | both layers are blurred; the underlay is scaled 1.25 so its own fade lands outside the canvas | real-window preview at blur 50 no longer shows the left text strip |
| "Background zoom/placement is ignored in the file" | the engine applied the pan offset to the source, not to the cover window, whenever the background picture's aspect differed from the canvas; the preview stretched the source into the base box | one cover-window formula with an origin (`baseX/baseY + cropX/cropY`) in the engine and in `geometry.ts` (the preview draws the whole source at `canvasW / visW`) | `add_background_image_crop_starts_at_the_cover_window`; a real job with zoom 2, x −6, y −67 now shows the same POSTER card as the dialog |
| "Redact takes so long in both settings" | `-region … -blur … +region` blurred at canvas cost (207 s on 48 MP) | expanded crop → blur → crop back → composite per box, one magick invocation; sigma unchanged and shared with the preview | 48 MP, two boxes: **207 s → 13.3 s** (15.6×); 5000×3750 PNG: 48 s → 5.7 s; solid ~1.8–2.6 s |
| "Annotate draws nothing until I release" | an unkeyed `<Show when={draft()}>` re-runs its child only on truthiness changes; the draft rect stayed 0×0 until `pointerup` committed it | `keyed` on the draft; the selection ring reads `hitBox` inside the JSX so it follows a move | `ui-shots.py`: the draft rect has non-zero extents mid-drag; the ring moves with a 40 px drag |
| "One rectangle is bugged with a selection outline" | `setSelected(new)` ran after every commit, and the outline was not gated by the tool | the outline and handles exist only in Select | `ui-shots.py`: no `.annotate-selection` while Rectangle/Number is active |
| "Let me choose the number" | badges auto-numbered; the only editor was on a placed badge | a Number field is always mounted (stable toolbar height): it edits the selected badge in Select and the next badge in Number; after placing `7` it carries `8` | `ui-shots.py`: typing `7` places a `7` text node |
| "Activity dots: newest on the left" | history is newest-first and the spark reversed it | render `slice(0, 12)` without reversing | `ui/src/app.tsx`; mock's newest row is first |
| "Arrow tip breaks at higher stroke values" | three butt-capped lines met at the tip; the head was a fixed 8–48 px | head = 3.5 × stroke (bounded), round caps/joins in the SVG and in `draw_command` | `ui-shots.py`: three round-capped lines at stroke 13; stroke 64 renders a clean solid head; engine test pins the command |
| "Text sizes above 200; double-click bug" | the caption was clamped to 200 and the slider was drag-only; the edited text stayed drawn under the textarea | captions up to 2000 with a typed number input (engine + editor); the edited shape and its ring are omitted while the inline editor is open; text selection disabled on the canvas | `ui-shots.py`: typed 300 places a `font-size=300` node (`max=2000`); double-click opens the editor with no ghost text and no ring; engine test accepts 600 and renders it |

### Round 5 measurement

| Layer | Result |
|---|---|
| Rust | `convertme-engines` targeted suites pass (background crop origin, redact solid/blur, tonal ramp incl. ±100 endpoints, annotate arrow caps + a 600 px caption); `convertme-gui` 25/25; `generated_fields` regenerated |
| `ui-shots.py` (full) | **39 dialogs of 61 tools, no layout failures** (`artifacts/round5/ui-full`) |
| `ui-shots.py --category images` | **20 dialogs of 20 tools, no layout failures** (annotate-polish now also checks the thick-arrow caps, typed 300 px text, and the double-click ghost; collage-grid, add-background-layers) |
| `app-e2e.py` (full) | **65 passed, 0 failed, 6 skipped** in 71 scenarios (`artifacts/round5/app-full`); run before the final annotate follow-up, whose own scenario and `dialogs-images` were re-run afterwards |
| `app-e2e.py` targeted | `dialogs-images`, `tool-image-annotate` (re-run after the arrow/text/double-click follow-up), `tool-image-background`, `tool-image-collage`, `tool-image-redact` all pass (`artifacts/round5/targeted`) |
| Redact timing | 8000×6000 JPEG, two blur boxes: 1772 ms solid, 13 345 ms blur (was 207 825 ms) |
| Collage preview | composes from cached 512 px thumbnails: 1.65 s → 0.72–0.77 s for three large PNGs (same composer and formulas) |
| `preflight.sh` | ok (final run after the last UI change) |
| Edit-photo tonal report | not reproducible on the user's own `testing.png` in the real window: highlights and whites are monotonic and correctly directed at ±100 (screenshots `probe-hl-max/min`, `probe-wh-max/min`); the ramp test now covers ±100 so a future regression cannot hide there |

## Round 6 — the video feedback round

| Report | Cause | Fix | Proof |
|---|---|---|---|
| "I selected 3 videos … does not show them" | the selection strip thumbnailed videos with ImageMagick: ~28 s per clip, then failure, so `analyze` never returned | one `still_for` routes a video to the filmstrip's first frame (32 ms), a PDF to its page render, everything else to the thumbnailer; the dialog preview uses it too | `multi-file-videos`: three clips reach the strip in **2.2 s**, all names present, three cached video stills |
| "the playhead is not aligned with the trim end" | each slider had its own min/max (start 0..end, end start..duration, playhead 0..duration) | all three span `0..duration`; the handlers keep the handles apart | end 9.16 and playhead 9.16 both read fraction 0.916; AT-SPI min/max equal (`tool-video-trim`) |
| "a button to save all the frames" | only per-moment chips; the engine's `fps` mode was unreachable from the dialog | **All frames (N)** sends the clip's `avg_frame_rate`; count on the primary action; ceiling 2000 shared with the engine; back to chips | 10 s mock clip: "All frames (300)" → "Save 300 Frames"; engine test writes 60 frames from 2 s at 30 fps |
| "save more than one file in a folder" | multiple artifacts were published as loose files beside the source | a multi-file job publishes into `<stem>/`, names drop the repeated stem; one-folder/one-file behavior unchanged for single results; directory artifacts stay single | frames → `clip-landscape-720p-10s/frame-001.png`; split → `…/part-001.mp4`; pages → `report-5-pages/001.png`; queue test pins the contract |

### Round 6 measurement

| Layer | Result |
|---|---|
| Rust | queue folder test + 7 queue tests; `probe` fps (incl. 30000/1001) and all-frames snapshot tests |
| `ui-shots.py` (full) | **42 dialogs of 64 tools, no layout failures** (`artifacts/round6/ui-full`) |
| `ui-shots.py --category video` | **11 dialogs of 12 tools, no layout failures**; trim axis and all-frames probes (`artifacts/round6/ui-video`) |
| `app-e2e.py` (full) | **66 passed, 0 failed, 6 skipped** in 72 scenarios (`artifacts/round6/app-full`) |
| `app-e2e.py` targeted | `multi-file-videos` (3 stills in 2.2 s), `tool-video-trim`, `tool-video-frames` (real window: **All frames (300)** → **Save 300 Frames**), `tool-video-split`, `tool-pdf-pages`, `immediate-tools` (`artifacts/round6/targeted`) |
| `quality-audit.py --only video-frames` | pass (folder output flattened; `artifacts/round6/audit-video-frames`) |
| Media-info frame rate | ffprobe `avg_frame_rate` (fallback `r_frame_rate`), parsed as a fraction |
| `preflight.sh` | ok (after the last Rust/UI change) |

## Round 7 — the audio feedback round

| Report | Cause | Fix | Proof |
|---|---|---|---|
| "why only the first 10 minutes?" | the waveform decoded 600 s and the cache read always claimed truncation, so a 14 s file said "first 10 minutes" and a long one stretched 10 min across the whole axis | decode at 1 kHz for up to **6 hours**, 2000 overview buckets, a real truncated flag in the cache; a short file shows no note | 14.1 s: 2000 peaks, no note; 11 min: `duration_ms=660000`, 138 ms; round-trip test both flag values |
| "listen and control the audio like the video" | playback was spawn-and-await: no stop, no pause, no position | one controllable sink (`audio.rs`): start/stop, seek via `ffplay -ss`/`mpv --start`, one player at a time; trim plays the source from its start to its end; normalize/channels play the produced result; visualize plays the source; bleep has Original/With Bleeps toggles | `ui-shots`: trim 2–4 s starts at 0:02.35 and stops at 0:04.00; Output and bleep buttons toggle Play/Pause; only one version plays |
| "closing the tool window keeps playing" | the player was a detached child with no stop path | the sheet's cleanup calls `stop_preview`; the main window's close hook stops the sink before exiting | `audio-playback`: an `ffplay` child is present while playing and gone after the sheet closes |
| "buttons play over each other; control original vs bleeps" | two buttons each spawned a player | one sink; starting a version stops the other; an async render that finishes after another start is discarded | `ui-shots`: Pause Original → Pause With Bleeps, and Original is no longer playing |
| "the bleep × does not work" | the waveform surface captured the pointer on pointerdown, so the button never received its click | ignore pointerdown targeting a button; stack the × above the range | `ui-shots`: range count 1 → 0 on × |
| "the preview does not align with the seek" | normalize/channels/bleep drew a marker from a second, internal clock while the output played from its own origin | one controlled playhead: the dialog owns the clock, the waveform draws `playhead` and reports `onSeek`; a click stops the sink and Play starts there | normalize click at 50 % → 0.500, Play → 0.564 after 0.9 s |
| "why does it say left when it is mono?" | Channels always drew L/R and a Left volume; the mono mix used one gain | source channel count in media facts; mono source = one M lane; mono output = one **Volume** field applied to both channels (engine accepts `volume`) | `channels_mono_volume_is_one_control_for_the_mix`; ui-shots Mono → `[Volume]` / `[M]`, Stereo → Left/Right / L,R |
| "use the pause icon instead of writing pause" | buttons toggled their text | play/pause glyphs shared by the transport and the version buttons; the subject label stays | ui-shots reads a polygon (play) or rects (pause) on every transport |
| "can the preview be centered?" | portrait frame `width: 100%` + `max-width: 200px` sat left | `margin-inline: auto` | 200 px frame, 178 px gap left and right |

### Round 7 measurement

| Layer | Result |
|---|---|
| Rust | `audio::tests` (player argv with `-ss`/`--start`, idempotent stop); waveform bucket/truncated round-trip; channels mono volume (~6 dB at 50 %) |
| `ui-shots.py` (full) | **47 dialogs of 69 tools, no layout failures** (`artifacts/round8/ui-full`) |
| `ui-shots.py --category audio` | **12 dialogs of 12 tools, no layout failures**; trim-playback, bleep-controls, normalize-playback (seek), channels-mono, visualizer-centre (`artifacts/round8/ui-audio`) |
| `app-e2e.py` (full) | **67 passed, 0 failed, 6 skipped** in 73 scenarios (`artifacts/round8/app-full`) |
| `app-e2e.py` targeted | `audio-playback` (ffplay child present while playing, gone after the sheet closes), `dialogs-audio`, all five audio-tool jobs (`artifacts/round8/targeted`) |
| Waveform | 14 s → 2000 peaks, no truncation; 11 min → 2000 peaks, 660 s, 138 ms; normalize click 50 % → 0.500, play → 0.564 after 0.9 s |
| Channels | mono → `Volume` + `[M]`; stereo → Left/Right + `[L,R]`; engine 50 % mono volume ≈ 6 dB down |
| `preflight.sh` | ok |

## Numbers from the previous final run (Round 3)

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
2. **Compress still takes 2.5 s to produce its facts** on a 6 MP poster because the
   preview *is* the artifact and oxipng's lossless passes cost that. The dialog now
   shows the source thumbnail immediately and debounces 400 ms; only the size/dimensions
   wait for the encode. The ceiling is 3 s, measured rather than estimated.
3. **The Windows sidecar binary was not rebuilt** here. `sidecars/pdf/build.sh` changed
   (pdf2docx removed from the pip line); rebuild it in the release matrix.
4. **AVIF/HEIC** cannot be decoded by the ImageMagick build on this machine (no
   `heic.la` coder module), so those pairs skip in the audit. The capability probe
   records the feature flags; the wheel does not yet mark them unavailable.
5. **Audio playback is delegated to a headless sink.** The window never decodes audio;
   `play_preview` probes `ffplay`, `mpv`, `paplay`, `pw-play`, `aplay` and falls back to
   the desktop opener only when none exists. There is no pause/seek: the command resolves
   when the sound ends (lesson 66).
6. **Perf was not re-measured this round.** Round 4 changed the preview paths
   (background, collage, annotate, redact, compress); re-run
   `perf-audit.py --baseline artifacts/perf/baseline.json` before the next release. The
   collage composer now skips its per-source size probes when every cell is the default
   centre cover.
7. **Video playback is a still sequence, not a decoded stream.** `video_frames` samples
   at 10 fps for short clips and within a 240-frame budget for long ones (a clip over
   4 minutes plays at 1 fps). The web view must not mount `<video>` (lesson 61/66);
   a smoother player needs a proven out-of-process sink.
8. **Audio playback is delegated to a headless system player.** It stays in the app's
   lifetime (no desktop window), but there is no pause/seek: the command resolves when
   the sound ends. `play_preview` probes `ffplay`, `mpv`, `paplay`, `pw-play`, `aplay`
   and falls back to the desktop opener only when none exists.
9. **A collage pan draws a live crop immediately and the composed image next.** One
   engine render is in flight at a time (120 ms debounce, latest request queued); the
   live cell uses the 512 px thumbnail and the composer's own formula, so it is a bridge
   and never the saved result. Large source pictures still cost one decode per render.
10. **The background preview caps its CSS blur at 32 display px.** The saved file uses
    the full sigma; the cap only protects the compositor on very small pictures.
11. **Annotation fonts resolve to a face installed on the machine** (`magick -list font`).
    There is no bundled font: a machine with no plain Sans/Serif/Mono face falls back to
    ImageMagick's default. The family name is not embedded in the written file.
12. **Redact's with/without preview is a cache artifact**, produced by the same engine
    at 1024 px and counted against the preview cache. The toggle never changes what the
    job writes.
13. **The edit-photo "inverted at ±100" report could not be reproduced** on the user's
    `testing.png` in the real window; highlights/whites are monotonic at the endpoints.
    The ramp test now covers ±100. If it recurs, replay the exact file and read the
    preview pixels before changing the curve.
14. **The add-background preview is still composed in the browser** (it was never an
    engine artifact). Its geometry shares `ui/src/geometry.ts` with the engine's cover
    window and both layers are blurred, but a future change to the shadow/radius maths
    needs the preview updated by hand.
15. **All frames is capped at 2000 images.** A longer clip cannot be exported as every
    frame in one job: the button is disabled and the engine refuses with the count. It
    is one ffmpeg pass; the per-moment chips remain for specific frames.
16. **A multi-output job publishes into `<source stem>/`.** A second run adds deduped
    names inside that folder (`part-001 (2).mp4`) under the never policy; `Replace`
    overwrites. A directory artifact (an extracted archive) is still one output.
17. **The waveform is a six-hour overview at 1 kHz.** A longer recording draws its
    head and says so; the bucket count stays 2000. Decoding the whole file at 8 kHz was
    what made the old ten-minute cap look necessary.
18. **Mid-file playback needs `ffplay` or `mpv`.** The simple sinks (`paplay`, `pw-play`,
    `aplay`) cannot seek and are refused for a trimmed start rather than silently
    playing from zero. There is no desktop-opener fallback: it cannot be stopped.
19. **The sink has no device handshake.** A player that starts but cannot open an audio
    device leaves the dialog's clock running silently; the stop paths still work.

## Traps specific to this codebase

- `qpdf 12` rejects `--json` together with a positional path (and does not put `/Rotate`
  in the page keys): read rotation with `pdfinfo -f 1 -l 1` (`Page 1 rot: 90`).
- The app's cache is `$XDG_CACHE_HOME/convertme/previews`; a harness scenario must read
  the environment, not the launcher's profile path.
- WebKit rebuilds the accessibility tree when a preview changes the DOM: the harness
  re-locates nodes on every attempt (`set_entry`, `press_primary`), and a click that used
  pre-shift extents lands elsewhere. A control below the sheet's scroller is clipped but
  still inside the window; `ensure_visible` compares against the **sheet** and scrolls
  with the wheel (lesson 80).
- A toolbar button with `aria-pressed` is a `toggle button` in AT-SPI, not a `button`;
  `press_button` accepts both, but a direct `find(role="button")` misses it.
- The mock is not the app: it proves layout and the reader path, never the engine.
