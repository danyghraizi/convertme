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
- Adding or renaming a field kind or key means: the Rust field table, `ui/src/types.ts`,
  `ui/src/field-parse.ts`, the mock, and the tests in `commands::field_payload` and
  `ui/src/field-parse.test.ts`.
- Never bypass `readFields` in mock mode: the mock must travel the same path as the app.

## Engine work

- Every external command goes through `crates/convertme-engines/src/proc.rs`: argv arrays,
  cache limits, watchdog, bounded stderr.
- Crop with absolute offsets, never `-gravity center` with signed offsets (it clips).
- Bound intermediates: crop the source, then scale to the canvas; never upscale to
  `zoom × canvas`. Compose at depth 8.
- Masks apply to the layer the control names (rounding is the photo's, not the canvas's).

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
