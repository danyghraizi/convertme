# Working on ConvertMe

Short rules for anyone (human or agent) changing this repository. The long version, with
the failures that produced each rule, is `docs/LESSONS.md`.

## Build

- The window runs `crates/convertme-gui/ui-dist`, embedded at compile time. After any
  change under `ui/src`, run `npm --prefix ui run build` **before** `cargo build`. The
  build script fails if the bundle is older, and a test fails if the binary embedded a
  different page.
- A second launch takes over a running window of a different build; Settings shows the
  interface build. If a change "does nothing", check the build id first.
- `./scripts/preflight.sh` before any push: gate, fmt, clippy, tests, UI build, clean
  checkout build.

## The UI/backend contract

- `crates/convertme-gui/src/commands.rs` describes every tool's fields; the UI renders
  them in `ui/src/components/field-editor.tsx`. An editor that owns a value (a crop
  surface, the page grid, a timeline) marks its field `hidden`, so the dialog cannot show
  a second control for it.
- A tool dialog is **one sheet**: head (drag handle, close at the top right), optional
  preview band, scrolling settings, footer. The preview is pinned *outside* the scroller;
  a control row is as tall as its content.
- A transient preview status is an **overlay** over the picture, never a paragraph in the
  band's flow: inserting a line moves every field. Reserve the facts row and sample the
  fields' top edge while rendering (`ui-shots.py` fails on a >2 px move).
- One edit travels as **one representation**. A crop with a placed box sends `x/y/w/h`
  and no `aspect`; the ratio is only for a job built without the editor.
- Collage hit areas come from the backend `collage_layout`; never re-derive the grid in
  the UI. Per-picture transforms are keyed by source index.
- **Field grammar:** the label sits **above** a full-width control. Do not use
  `align-self: start` on `.field` inside the flex-column `.modal-fields` — the flex cross
  axis is horizontal, so it shrink-wraps the row (a 199 px select beside a 552 px input).
  `ui-shots.py` asserts every non-block control fills at least 90 % of its row.
- The sheet resizes from a visible corner grip and its bottom/right edges; its height
  ceiling is the window (`94vh`), not a hard 780 px.
- The **web view never mounts `<audio>` or `<video>`** — not in a dialog, and not in
  `preview.tsx`. Audio plays through `play_preview` (a headless `ffplay`/`mpv`/`paplay`/
  `pw-play`/`aplay` sink); video walks the `video_frames` still sequence. Playback speed
  scales the playhead rate.
- An annotation draft is `<Show when={draft()} keyed>`; an unkeyed Show re-runs its child
  only when truthiness changes and leaves a zero-size rectangle until pointerup. The
  selection ring reads `hitBox(shape())` inside the JSX and is gated to Select. The Number
  field is always mounted (stable toolbar height) and sets the next badge.
- An arrow's head scales with its stroke (3.5×, bounded) and all three lines use round
  caps/joins in the SVG and MVG; a fixed head plus butt caps breaks the tip at wide
  strokes. A caption is free-sized to 2000 px and typed in a number input.
- While an inline editor is open, omit the shape being edited from the SVG and hide the
  selection ring; drawing both is the double-click ghost.
- The front end never decodes audio or video: WebKitGTK's media stack crashed the process.
  Video playback walks the `video_frames` sequence; a produced audio preview plays through
  the system sink in `play_preview`, without opening a desktop app.
- The dialog's copy (title, primary action, benefit lines) lives on `Tool` in
  `crates/convertme-core/src/formats.rs` and reaches the UI through the wheel payload,
  `ui/src/catalog.ts` (the empty-wheel fallback) and `convertme probe --tools`.
- The mock renders `ui/src/fields.generated.json`, which is the backend's own table.
  After changing a field table: `UPDATE_FIELDS=1 cargo test -p convertme-gui
  generated_fields`, then `npm --prefix ui run build` (the file lives under `ui/src`, so
  the frontend build is what makes the bundle current). Plain `cargo test` fails while
  the file is stale. `./scripts/gen-ui-fields.py` writes the same file from a built
  binary, for a machine that has one.
- Adding or renaming a field kind or key means: the Rust field table, `ui/src/types.ts`,
  `ui/src/field-parse.ts`, the tests in `commands::field_payload` and
  `ui/src/field-parse.test.ts`, and a new `is-block`/editor arm when it needs one.
- Never bypass `readFields` in mock mode: the mock must travel the same path as the app.
- A field a dedicated editor owns is `hidden: true` in the Rust table; its value is
  still collected, so the editor never adds a second control for the same number.

## Engine work

- Every external command goes through `crates/convertme-engines/src/proc.rs`: argv arrays,
  cache limits, watchdog, bounded stderr.
- A PDF still is a page render (`PreviewCache::pdf_pages`), never ImageMagick's black
  rectangle. Every selected file keeps a card even when its thumbnail fails.
- A page grid loads per path (`createEffect`), not on mount: the second file of a
  multi-file dialog must show its own page count and cards. Ignore a stale response.
- A metadata writer must be able to add an EXIF block to a file that has none
  (`little_exif::Metadata::new()` when `new_from_path` says "No EXIF data found").
  PDF `/Info` is usually indirect; dereference it and decode its strings.
- A CLI verb is in `CLI_VERBS` *and* the `run` dispatch. A dispatch arm missing from the
  list launches the window instead (the `pdf-pages` trap).
- Crop with absolute offsets, never `-gravity center` with signed offsets (it clips).
- Bound intermediates: crop the source, then scale to the canvas; never upscale to
  `zoom × canvas`. Compose at depth 8.
- A blur redaction is an expanded crop → blur → crop back → composite per box. `-region`
  plus `-blur` costs canvas-wide time (207 s on 48 MP; 13 s with patches).
- A cover crop has an origin: add `(source - base)/2` to the pan offset when the
  picture's aspect differs from the canvas. The browser draws the whole source at
  `canvasWidth / visW`, never stretched into the base box (`ui/src/geometry.ts`).
- A video still comes from `filmstrip(source, 1)`, a PDF still from its page render, and
  only a still comes from the image thumbnailer (`still_for`). ImageMagick on a video
  walks the clip (~28 s) and fails.
- The waveform is a 1 kHz, six-hour overview with 2000 buckets and a real truncation flag;
  a short file must not show the "part shown" note.
- Audio playback is one shared sink (`audio.rs`): starting a play stops the previous one,
  `stop_preview` kills the child, and every sheet's cleanup calls it. Never spawn a
  player and await it, and never fall back to a desktop opener (it cannot be stopped).
- Several files from one job are one result: publish them into `<source stem>/` (names
  without the repeated stem) and treat a directory artifact as one output. The e2e
  harness recurses into that folder; a folder named `<stem>-out` is still one artifact.
- A snapshot's all-frames mode sends `fps` (the clip's average rate), not thousands of
  times; the UI and engine ceiling is 2000 frames.
- Masks apply to the layer the control names (rounding is the photo's, not the canvas's).
- A tonal slider's direction is measured on a ramp at +N/−N (highlights, whites, blacks,
  shadows, hue range, dehaze magnitude). A sign is not implied by the label.
- A composite inherits the colour space of its canvas: a grayscale backdrop needs an
  explicit `-colorspace sRGB` before the photo/shadow composites.

## Dialogs

- The tool dialog is one sheet (`600px`): head, optional preview band, scrolling
  settings, footer. The reference's explanatory side panel was annotation, not product;
  it is gone, and the tool's copy still lives on the tool card and in the tool table.
- `ui/src/components/tool-modal.tsx` owns the shell; `styles.css` owns `.modal-*`.
  A per-tool editor's CSS lives beside its component (`*.css` imported there), never in
  the shared file.
- `./scripts/ui-shots.py` measures the contract (sheet margins and width, title against
  the backend table, primary label, row alignment, **overlaps between controls**, the
  crop ratio probes, preview-band stability, annotate drawing/erasing and collage cell
  selection) and `./scripts/app-e2e.py --only dialogs-images` re-measures it in the real
  window, including a drag of the head (`dialog-moves`). The real-image scenarios add
  crop-placement RMSE, annotate pixels, a background blur slider drag and the collage
  preview/hit areas.
- The primary button is named by the tool (`Save Organized PDF`), never "Run". Use
  `press_primary()` in the harness, not a literal label.

## Verification

- A UI change is verified by a measurement (bounding box, computed style, element
  identity, pixel sample, **overlap between controls**), not by a screenshot alone.
  Screenshots of this app are scaled and cropped by the window manager; coordinates read
  from them are not measurements.
- The harness's `set_entry` / `press_primary` re-locate the node and retry: WebKit
  rebuilds the accessibility tree when a preview inserts media, and a click that used
  pre-shift extents lands elsewhere.
- "Visible" means inside the **sheet**, not the window: a control below the dialog's
  scroller is clipped, and clicking its coordinates hits the backdrop and closes the
  dialog. Scroll with the wheel, re-locate, and guard a null extent.
- A toolbar button with `aria-pressed` is a `toggle button` in AT-SPI; accept both roles.
- A preview change is not proven by a screenshot: sample the preview band and the fields'
  top edge through the render, and validate the published file's pixels.
- An engine change is verified by sampling the output (dimensions, corner pixels, alpha).
- Reproduce the user's exact parameters and files before concluding anything about a bug.
- A test that only runs through the mock proves the mock.

## Front-end state

- Effects that write state compare against `untrack`ed values.
- Time sliders that show the same quantity (trim start/end, playhead) share one
  `0..duration` axis; a slider whose min/max follows another handle is a hidden second
  scale.
- A transport plays the selection it is drawn over: trim audio starts at the trim start
  and stops at its end; the button toggles Play/Pause. A control inside an editable
  surface (the bleep ×) must be excluded from the surface's pointerdown/capture.
- One clock per preview: a waveform over a dialog-owned preview takes `playhead` and
  reports `onSeek`; clicking stops the sink and sets the position the next Play uses.
  Never draw a marker from a second internal clock.
- A mono source shows one lane labelled M; a mono output has one **Volume** field applied
  to both source channels. Stereo keeps Left/Right. The media facts carry the channel
  count.
- Transport buttons keep their subject label and prefix a play/pause glyph; the visible
  text does not change with state.
- A fixed-aspect preview is centred with `margin-inline: auto`; measure both gaps.
- A debounced engine preview needs one request in flight, latest-wins, plus a live layer:
  the collage draws the selected cell from its thumbnail with `collageCellBox` and drops
  it when a render containing the current transforms lands. Restarting the debounce per
  pointer move means no render until the pointer stops.
- Collage Rows/Columns always drive the layout (Style is Grid/Featured). Do not add a
  mode whose counts are ignored.
- Activity dots are newest-first: history is already sorted, so do not reverse it.
- User-edited lists keep stable identities (store with ids, mutate items); sort copies for
  display, never the list being edited. While a row is being *typed into*, key it by
  index (`<Index>`) and commit on `change` — replacing the object on every keystroke
  rebuilds the focused input and loses the character.
- A control that shows a pixel size must measure the picture's own frame, not its
  container: percentages over a letterboxed preview are not percentages of the photo.
