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
  a control row is as tall as its content (`align-self: start` in a flex column) — a
  stretched row let tall controls paint over the next one.
- The front end never decodes audio or video: WebKitGTK's media stack crashed the process.
  The video transport walks the filmstrip; a produced audio preview is handed to the
  desktop player (`play_preview`).
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
- Crop with absolute offsets, never `-gravity center` with signed offsets (it clips).
- Bound intermediates: crop the source, then scale to the canvas; never upscale to
  `zoom × canvas`. Compose at depth 8.
- Masks apply to the layer the control names (rounding is the photo's, not the canvas's).

## Dialogs

- The tool dialog is one sheet (`600px`): head, optional preview band, scrolling
  settings, footer. The reference's explanatory side panel was annotation, not product;
  it is gone, and the tool's copy still lives on the tool card and in the tool table.
- `ui/src/components/tool-modal.tsx` owns the shell; `styles.css` owns `.modal-*`.
  A per-tool editor's CSS lives beside its component (`*.css` imported there), never in
  the shared file.
- `./scripts/ui-shots.py` measures the contract (sheet margins and width, title against
  the backend table, primary label, row alignment, **overlaps between controls**, the
  crop ratio probes) and `./scripts/app-e2e.py --only dialogs-images` re-measures it in
  the real window, including a drag of the head (`dialog-moves`).
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
- An engine change is verified by sampling the output (dimensions, corner pixels, alpha).
- Reproduce the user's exact parameters and files before concluding anything about a bug.
- A test that only runs through the mock proves the mock.

## Front-end state

- Effects that write state compare against `untrack`ed values.
- User-edited lists keep stable identities (store with ids, mutate items); sort copies for
  display, never the list being edited. While a row is being *typed into*, key it by
  index (`<Index>`) and commit on `change` — replacing the object on every keystroke
  rebuilds the focused input and loses the character.
- A control that shows a pixel size must measure the picture's own frame, not its
  container: percentages over a letterboxed preview are not percentages of the photo.
