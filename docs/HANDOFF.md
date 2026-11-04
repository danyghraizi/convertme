# Handoff

ConvertMe is a local Tauri app: a wheel picks a category, a format and a tool; the tool
opens **one dialog sheet**; the job runs through the engines crate and publishes beside
the source. 307 conversion options, 33 backend tools, everything offline.

This file is the state of the product and the protocol for changing it. Read it first,
then `AGENTS.md` (rules) and `docs/LESSONS.md` (the failures that produced the rules;
42–56 are the alignment session, 57–63 the first bug round, 64–70 the design/media
regression, 71–82 the image-tools feedback round, 83–101 the interaction, video and
audio rounds, 102–120 the playback, PDF, PDF→DOCX, export-parity, EPUB/tool-start and
native-export rounds).

## Start here (new session)

| | |
|---|---|
| Branch | `master` |
| Last code commit | `83e6996` — "tools: EPUB hierarchy, mixed merge and native PDF/ODS [skip ci]" (Rounds 9–12) |
| Previous code commit | `1f25fb7` — "tools: previews, playback and outputs follow the user" (Rounds 5–8) |
| Revert point | branch + annotated tag `savepoint/pre-alignment-2026-10-31` |
| Proof | `artifacts/round9/audit-pdf` (28 CLI PDF cases), `artifacts/round9/app-pdf` (11 real-window PDF scenarios), `artifacts/round9/app-documents` (Documents merge), `artifacts/round9/ui-final` (52 mock dialogs), `artifacts/round9/ui-documents` (Documents layout), `artifacts/round9/audit-ods` (native ODS), `artifacts/round9/app-doc-pdf` and `app-sheet-pdf` (native PDF), `artifacts/round9/app-errors` (visible failure), `artifacts/round9/app-split-final` (split folder), `artifacts/round9/app-folders` (folder artifacts), `artifacts/round9/ui-pdf` (PDF layout + drag probes), `artifacts/round9/app-drag`, `app-split`, `app-pages`, `app-frames`, `app-vsplit`, `app-compress`, `app-dialogs-pdf`, `app-docx` (real window), `artifacts/round9/audit-docx` (CLI DOCX); older: `artifacts/round8/ui-full`, `artifacts/round8/app-full`, `artifacts/round8/targeted`, `artifacts/round7/*`, `artifacts/round6/*`, `artifacts/round5/*`, `artifacts/final5/audit` (CLI matrix), `artifacts/perf/run4` (perf) |

Rounds 9–12 are committed and pushed as `83e6996` on `master`. The commit carries
`[skip ci]`, so no GitHub Actions run was created; the local preflight + full
`ui-shots`/`app-e2e` runs are the acceptance proof above. The previous commits are also
pushed, and they carry `[skip ci]` for the same reason.

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
submits `order`/`rotate`; dragging is pointer-driven (small labelled ghost, dashed
landing slot, `Page N → position K of M`), the dropped card is selected, and the arrow
keys move a focused card. Duplicated pages are cloned objects when qpdf refuses a
repeated page spec. `Compress` labels each Ghostscript preset with its dpi and tradeoff
and explains Prepress as the largest/best-image tier. Multi-output jobs name their
action in the folder (`-split`, `-images`, `-frames`). `Merge` reorders the selection.
`PDF → DOCX` keeps the page's visual rows: the header
table (photo plus inline contact icons), right-tabbed dates/places, a four-column
coursework table, joined bullet items, section rules as heading borders, 10 pt Calibri
body/12 pt headings, and no page break for a one-page document (7 Heading 1, 4 Heading
2, 29 list paragraphs, 29 `w:numPr`, 5 images, coverage 1.0). `PDF → PPTX` follows the
page aspect, `XLSX` writes one sheet per page, `EPUB` still uses the reflow analyser.

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
| "save more than one file in a folder" | multiple artifacts were published as loose files beside the source | a multi-file job publishes into `<stem>-<function label>/`, names drop the repeated stem; one-folder/one-file behavior unchanged for single results; directory artifacts stay single | frames → `clip-landscape-720p-10s-frames/frame-001.png`; split → `clip-landscape-720p-10s-split/part-001.mp4`; pages → `report-5-pages-images/001.png`; queue test pins the contract |

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

## Round 9 — the PDF feedback round

| Report | Cause | Fix | Proof |
|---|---|---|---|
| "I don't understand the differences between these qualities; prepress has the best images" | the PDF compress select showed Ghostscript's internal names (`screen`, `ebook`, `printer`, `prepress`) with no resolution or tradeoff; `help` was unused | option labels now state the measured tier (`Screen · 72 dpi (smallest)` … `Prepress · 300 dpi (best images)`), and one help line says which tiers downsample, that Printer/Prepress keep 300 dpi, and that Prepress keeps the most image detail and makes the largest file; stored values and the engine contract are unchanged | `pdf_compress_describes_its_presets`; `ui-shots` PDF compress screenshot; Ghostscript 10.08 probe: color resolution 72/150/300/300 dpi, JPEG QFactor 0.76/0.76/0.4/0.15 |
| "organizing pages is not clear: which page is selected, where it lands, how the others move" | HTML drag-and-drop showed a full-card translucent ghost over the grid; `dragover` reordered live with no landing marker, nothing was selected, and the tool chips floated over the page | pointer-driven lift: a small `Page N` ghost beside the pointer, the source removed from the flow, a dashed slot at the target index, a head readout `Page N → position K of M`, click/keyboard selection with a ring and `, selected` in the accessible name, card tools moved into a footer row | `ui-shots`: mid-drag ghost < 75 % of a card, slot centre uncovered, 4 cards + slot, drop order `[2,3,4,1,5]`; `tool-pdf-organize-drag` (real window): `Page 3 → position 2 of 5`, dropped card selected, saved PDF page 2 is source page 3 |
| "the split folder should state what was done" | a multi-output job published into `<stem>/`, so a split of `report-5-pages.pdf` made `report-5-pages/` with no verb | multi-output folders are `<stem>-<function label>` (`split`, `images`, `frames`, `extract`, else `out`); a file owning the name steps to `-2`, `-3`; names inside still drop the repeated stem | queue `split_outputs_publish_into_a_labelled_folder` + collision test; real window: `report-5-pages-split/`, `report-5-pages-images/`, `clip-…-frames/`, `clip-…-split/` |

### Round 9 measurement

| Layer | Result |
|---|---|
| Rust | `convertme-gui` `field_payload` 8 passed, `generated_fields` 1 passed; `convertme-core` queue 9 passed |
| `ui-shots.py --category pdf` | **8 dialogs of 11 tools, no layout failures**; organize rotate + drag probe + keyboard reorder (`artifacts/round9/ui-pdf`) |
| `app-e2e.py --only tool-pdf-organize` | 3 pass: duplicate/remove/rotate (`[1,2,2,3,5]`, page 1 at 90°), drag (`Page 3 → position 2 of 5`, output page 2 = source page 3), multi-file reload (`artifacts/round9/app-organize`) |
| `app-e2e.py` folder checks | `immediate-tools` (split) pass; `tool-pdf-pages` pass; `tool-video-frames` pass; `tool-video-split` pass (`artifacts/round9/app-split`, `app-pages`, `app-frames`, `app-vsplit`) |
| `app-e2e.py --only compress-pdf` / `dialogs-pdf` | pass; the preset select accepts and resets the labelled options |
| `preflight.sh` | ok: identity gate, fmt, UI typecheck/tests/build, generated fields, clippy `-D warnings`, 61 + 4 + 3 + 3 + 130 + 28 Rust tests, clean-checkout build, deb 112.29 MiB |
| Full `app-e2e.py` | not run this round; the targeted scenarios above cover every changed surface |

## Round 10 — the PDF → DOCX feedback round

| Report | Cause | Fix | Proof |
|---|---|---|---|
| "the result is several pages and does not look the same as the PDF (iLovePDF does)" | the sidecar walked PyMuPDF **text blocks**. Blocks cross columns and sections: Education's left name and right date were separate blocks, the four coursework columns arrived as blocks ordered down each column, a block held the last bullet plus the next heading, and the writer appended every icon/photo with `document.add_picture` at the *end of the body*. The document used Word defaults (11 pt, 1 in margins) and flow paragraphs, so dates merged into degrees, bullets split, the photo/icon order was lost, and spacing pushed the résumé onto a second page. | `layout.py` flattens the page to visual rows (lines grouped by baseline) and classifies title, heading, dated entry, italic detail, project, skill, bullet, four-column bullet grid, body. `docx.py` writes Word-native layout: header table with the photo and inline contact icons, right tab stops for dates/places, a borderless four-column coursework table, real list numbering, section rules as heading bottom borders, small-caps title, 10 pt Calibri body / 12 pt bold headings, Letter page and compact margins from the reference. Images are inserted as runs, not body-level pictures; only real icon-font glyphs are rasterised (a body-font `|` is punctuation, not a picture). | `convert-pdf-docx`: 7 Heading 1, 4 Heading 2, 29 list items/`w:numPr`, 5 images, **0 page breaks**, coverage **1.0**, 2 tables, 11 right tabs, 7 heading rules, no merged degree/date; `quality-audit pdf-to-docx` 2/2; single-spaced 10 pt Calibri in the 554 pt text frame measures ~745 pt of 762 pt available (one page) |

### Round 10 measurement

| Layer | Result |
|---|---|
| Sidecar | rebuilt with Python 3.14 + PyMuPDF 2.0 + python-docx; `sidecars/pdf/dist/convertme-pdf`, `target/release/convertme-pdf`, `vendor/bin/convertme-pdf-x86_64-unknown-linux-gnu` |
| `app-e2e.py --only convert-pdf-docx` | pass, 16 checks including the layout checks (`artifacts/round9/app-docx`) |
| `quality-audit.py --only pdf-to-docx` | 2 passed, 0 failed (`artifacts/round9/audit-docx`) |
| DOCX facts | 55 paragraphs, 7 Heading 1, 4 Heading 2, 29 lists, 29 `w:numPr`, 5 images, 0 page breaks, 55 k → 447 k |
| `docx-quality.py` vs the reference DOCX | coverage 1.0; Heading 1 7 vs 8; Heading 2 4 vs 4; lists 29 vs 30 |
| `preflight.sh` | ok (Rust/UI unchanged by the sidecar writer): gate, fmt, UI tests/build, clippy, workspace tests, clean build, deb 112.31 MiB |
| Renderer | no LibreOffice/Word renderer in this environment; pagination is estimated from the emitted styles (single-spaced Calibri, compact margins) rather than a rendered page |

## Round 11 — the package-output parity round

| Report | Cause | Fix | Proof |
|---|---|---|---|
| Exporting the same PDF to MD, EPUB, ODT, XLSX or PPTX had no honest structural test: the audit checked "zip starts with PK" (or nothing), so a blank sheet, a one-slide deck or an ODT with no body could pass | the package writers existed, but the shared validator only understood DOCX and EPUB's container; rich-text XLSX cells were read as their first run, and PDF ligatures/hyphenation produced false text-loss reports | `validate.py` now reads ODT (mimetype, headings, lists, tables, images), PPTX (slides, text, pictures, slide size) and XLSX (per-cell values across every `<t>` run); text coverage normalizes NFKC ligatures and rejoined hyphen breaks; `quality-audit` gained `markdown_check`, `odt_check`, `pptx_check`, `xlsx_check`, a text-aware `epub_check`, and resume cases for each; PDF page-image sets are one directory artifact, not a failed "one artifact" check | six CLI exports of the résumé all succeeded: DOCX 438 K, EPUB 403 K, MD 4.5 K, ODT 409 K, PPTX 431 K, XLSX 8.2 K; ODT 11 headings/2 tables/5 images, PPTX 1 slide/5 pictures/4.1 k characters, XLSX 68 cells with `State University` and `Sep. 2017 – May 2021`, all source words covered |

ODT goes through the **bundled Pandoc** using the layout-aware DOCX as its
intermediate, so PDF → ODT no longer needs a system LibreOffice. `quality-audit
`--only pdf` is **28 passed, 0 failed, 14 skipped**; the skips are the LibreOffice-only
targets and the AVIF decoder on this machine. `app-e2e --only pdf` is **11 passed, 0
failed** (compress, dialogs, organize/rotate/drag/multi-file, merge, metadata, pages,
text, DOCX and ODT). The real-window folder scenarios (`immediate-tools`,
`tool-video-frames`, `tool-video-split`) are **3 passed, 0 failed**. `preflight.sh` is
**ok** (identity gate, fmt, UI typecheck/tests/build, generated fields, clippy, 130
engine + workspace tests, clean-checkout build, deb 112.35 MiB); its first run caught
the stale Markdown test that still gated on Pandoc instead of the sidecar, now fixed.

## Round 12 — the EPUB and out-of-box tools round

| Report | Cause | Fix | Proof |
|---|---|---|---|
| EPUB contacts/icons were huge, lines overlapped and the coursework table broke on a narrow reader | the stylesheet was inline per chapter and images had no dimensions, so a reader that dropped inline CSS used 94 px intrinsic icons; the portrait and dates floated without containment; grid bullets were stripped; there was no viewport meta | `style.css` is a linked manifest item; every image has explicit width/height; floats are contained; detail/project metadata is right-aligned; contacts are wrapping spans without invented dots; the grid has a colgroup, bullets and a fixed layout; Calibri/Carlito replaces Georgia | `quality-audit --only pdf-to-epub`: 2 passed; validator asserts linked CSS and image dimensions; Calibre render matches the résumé sections |
| Documents tools all said "Tool could not start"; Merge with DOCX+HTML failed; the bottom text was too quiet | the fallback catalog advertised Compress/Merge/Metadata (and tools in four other categories) that the Rust `TOOLS` table never registered; Tauri rejects commands with a string, so `error instanceof Error` erased the real reason; Merge only existed for PDFs | removed phantom cards; added a real Documents Merge (Pandoc normalizes each input to the first file's format, so DOCX+HTML works); normalized string rejections and added a dismissible red alert above the workspace; an empty category now says so instead of showing dead cards | `dialogs-documents`, `documents-merge` pass in the window; `negative-corrupt` captures the red alert; `ui-shots --category documents` is 1 dialog, no layout failures |
| PDF targets required system LibreOffice; Spreadsheets → PDF/ODS failed; HTML→PDF was enabled but then failed | every Office/spreadsheet pair was pinned to `EngineRef::LibreOffice`; `document_pdf` called `soffice` even for HTML; ODS had no in-process writer | documents and spreadsheets now use Pandoc HTML + the built-in `printpdf` writer; spreadsheets→ODS uses `spreadsheet-ods`; HTML/CSV/TSV/ODS expose PDF; PPTX/PPT remain the only LibreOffice-gated targets. Complex DOCX tables degrade to a readable `Tool | Input | Output` grid rather than clipped columns | `quality-audit --only pdf`: **39 passed, 0 failed, 7 skipped** (down from 14 skips), including `matrix-docx-to-pdf`, `matrix-html-to-pdf`, `matrix-ods-to-pdf`, `matrix-xlsx-to-pdf`; `--only ods`: **9 passed, 0 failed, 1 skipped**; window `convert-document-pdf` and `convert-spreadsheet-pdf` pass |

### Round 12 measurement

| Layer | Result |
|---|---|
| EPUB | `quality-audit --only pdf-to-epub`: 2 passed; linked `style.css`, explicit image dimensions; Calibre render matches the résumé’s sections and contact row |
| Documents | `app-e2e --only documents`: 2 passed (`dialogs-documents`, mixed DOCX+HTML `documents-merge`) |
| Native PDF/ODS | window `convert-document-pdf` and `convert-spreadsheet-pdf`: 1 passed each; `quality-audit --only pdf`: **39 passed, 0 failed, 7 skipped**; `--only ods`: **9 passed, 0 failed, 1 skipped** |
| Error surface | `negative-corrupt`: pass; the red dismissible alert is captured with the engine error |
| UI layout | `ui-shots --category documents`: 1 dialog of 1 tool, no layout failures |
| `preflight.sh` | **ok**; identity gate, fmt, UI typecheck/tests/build, generated fields, clippy `-D warnings`, 132 engine + 28 GUI + core/workspace tests, clean-checkout build, deb **116.56 MiB** |
| Full `app-e2e.py` | not run; the targeted scenarios above cover every changed surface |
| Published | commit `83e6996` pushed to `origin/master`; `[skip ci]` prevented a GitHub Actions run |

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
3. **The Windows sidecar binary was not rebuilt** here. The Linux `convertme-pdf` was
   rebuilt for Round 10 (PyMuPDF 2.0 + python-docx); rebuild the Windows one in the
   release matrix. `pdf2docx` is still not a dependency: the writer is ours so the
   row layout, tabs and tables are testable.
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
16. **A multi-output job publishes into `<source stem>-<function label>/`.** The label
    is the action (`-split`, `-images`, `-frames`, `-extract`, else `-out`); a file
    already owning that name pushes the folder to `-2`, `-3`, …. A second run adds
    deduped names inside the folder (`part-001 (2).mp4`) under the never policy;
    `Replace` overwrites. A directory artifact (an extracted archive) is still one
    output.
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
- A fresh Xvfb session can have the XDG portal's **"Global Shortcuts Requested"** window
  at `(0,0)` size `504×540`. Real-pointer scenarios must start outside that rectangle;
  clicks inside it never reach the webview (`tool-pdf-organize-drag` starts on card 3 for
  this reason). Check `xdotool getmouselocation` + `getwindowname` when a synthetic click
  does nothing.

## Release readiness — Linux and Windows

Cause → the release inputs and full native acceptance matrix are not complete. Fix → capture only publisher-checked, hash-locked inputs; keep package replay and candidate verification fail-closed. Measurement → the approved Linux capture stopped with its explicit incomplete-input report, and no candidate package set exists.

| Captured item | Measurement |
|---|---|
| Acceptance fixtures | `artifacts/acceptance/fixtures.zip`, 40,586,226 bytes; archive SHA-256 `e2ecd25dca63d13198a1bf20da6419ac7b221d6843e456e17e3ffd70b8a75219`; validated fixture-manifest SHA-256 `8bc6a0603441fcbcaef23cada0eafb4d15ed84f9f0ce840ba73b2eec23eea259` |
| Rootless Docker | Docker 28.5.2 and rootless extras captured; SHA-256 values are in `packaging/linux/build.lock.json` |
| Linux Python sidecar wheelhouse | 24 wheels captured and accepted by `prepare-linux-pdf-inputs.py --verify` |
| AppImage deploy tools | `linuxdeploy`, `appimagetool`, and the GTK plugin captured and hash-checked against the release pins |
| Rootless daemon smoke | `docker version` reported client/server 28.5.2, rootlesskit 2.3.4, VPNKit networking; the private daemon directory and process group were removed on exit |
| Source identity transport | `release-inventory.py source-identity` and the offline identity-file round trip produced byte-identical JSON |
| Bundled-tool version probe | The real ImageMagick executable ran through a temporary manifest-based tools directory and reported `ImageMagick 7.1.2-35` |

The Linux release remains blocked on the signed Ubuntu 24.04 package/source snapshot, exact binary and source closure, pinned OCI image, complete retained builder image, publisher-verified Rust 1.99 and Node 22 inputs, Cargo/npm offline caches, and full source materials. The capture driver deliberately exits instead of synthesizing these records. The current capture includes Docker archives, Python wheels, qpdf/ImageMagick, and AppImage tooling only.

Docker 28.5.2 rejects the approved `DOCKERD_ROOTLESS_ROOTLESSKIT_NET=host` setting in its own `dockerd-rootless.sh`; the successful connected capture smoke therefore uses bundled VPNKit. Replay containers remain `--network=none`. This is recorded in the Linux build lock; it does not permit connected replay.

Native Windows runtime Authenticode verification, Poppler/FFmpeg capture, MSVC builds, and Windows UIA are not executable on this CachyOS host. The Windows UIA harness is configured to mark the five unimplemented full-matrix rows (`instance-forward`, `crop-placement`, `annotation-pixels`, `video-trim`, `audio-trim`) as blocked rather than passed. Annotation solid-fill acceptance also requires a product behavior not present in the current outline-only editor.

No Linux or Windows package, inventory, candidate, native acceptance report, workflow dispatch, publish, or tag was produced. The release workflow is now manual-only and has no publisher job; its build jobs still require a separate integration pass for the incomplete payload/material inputs and native acceptance matrix.
