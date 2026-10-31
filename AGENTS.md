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
  them in `ui/src/components/field-editor.tsx`.
- The dialog's copy (title, primary action, rail benefits, eyebrow) lives on `Tool` in
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

- The tool dialog is two panes: the sheet (`600px`) and a rail (`336px`) holding the
  tool's own copy. `ui/src/components/tool-info.tsx` renders the rail; every string it
  shows comes from the tool table.
- `./scripts/ui-shots.py` measures the contract (pane widths, rail copy against the
  backend table, the primary button's label, label/control alignment) and
  `./scripts/app-e2e.py --only dialogs-images` measures it in the real window.
- The primary button is named by the tool (`Save Organized PDF`), never "Run". Use
  `press_primary()` in the harness, not a literal label.

## Verification

- A UI change is verified by a measurement (bounding box, computed style, element
  identity, pixel sample), not by a screenshot alone.
- An engine change is verified by sampling the output (dimensions, corner pixels, alpha).
- Reproduce the user's exact parameters and files before concluding anything about a bug.
- A test that only runs through the mock proves the mock.

## Front-end state

- Effects that write state compare against `untrack`ed values.
- User-edited lists keep stable identities (store with ids, mutate items); sort copies for
  display, never the list being edited.
