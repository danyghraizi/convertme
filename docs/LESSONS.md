# Lessons from building ConvertMe

Written after a day of interface work on `add background`, where most of the time went
into failures that looked like something else. Each item states the trap, the evidence
that exposed it, and the rule that prevents it. Read this before touching the UI, the
engines, or the build.

## 1. A rebuilt binary is not a new window

**Trap.** The UI kept showing an old design after `npm run build && cargo build`. Two
separate causes, both silent:

- `cargo` embeds `ui-dist` at compile time, and the build script only watched the
  *directory* mtime. Vite rewrites files in place, so the directory mtime never changed
  and the old bundle stayed embedded.
- The single-instance handshake forwarded the second launch's paths and exited, leaving
  the **old process** on screen. Nothing on screen said which build it was.

**Rule.** Any embedded asset must have a per-file `cargo:rerun-if-changed`, a test that
compares the embedded page with the built one, and a launch handshake that carries a
build stamp. A launch of a different build must take over the running window, not defer to
it. Show the build id somewhere (Settings) so "stale window" is diagnosable in one look.

## 2. The backend contract is only real if the client parses all of it

**Trap.** The dialog rendered `gradient`, `segmented` and `file` fields as text boxes and
ignored `show_if`/`option_labels`. The Rust side was correct; the UI's field reader had a
whitelist of kinds and copied only a few properties.

**Rule.** When a component renders a payload, the parser is part of the contract:
validate kinds against one list, copy every property, and unit-test the parser with a full
payload. Never let the dev mock bypass the parser — the mock must go through the same code
path, or the mock proves nothing.

## 3. Preview maths and engine maths must be the same function

**Trap.** The composition preview did its arithmetic against a 512 px thumbnail; the
engine used the 5000×3750 file. `padding: 342` looked like a third of the frame and like a
6 % border. The file was right; the dialog lied.

**Rule.** The preview may *draw* a thumbnail, but every number it shows or derives must
come from the real dimensions (ask the backend). Any control in absolute pixels on a
resolution-independent thing is a bug: use proportions of a stated reference (percentage
of the shorter side) and state the unit in the label.

## 4. Reactive reads inside reactive effects destroy DOM under the pointer

**Trap.** A slider moved 1 % and stopped. The sync effect read the *emitted* signal
reactively, so each input re-parsed the value and rebuilt the list rows, replacing the
`<input>` the pointer was holding. Editing a list by index whose order is re-sorted on every
keystroke had the same effect.

**Rule.** An effect that writes state must compare against `untrack`ed values. Lists that
the user manipulates get stable identities (a store with ids; mutate items, don't replace
them). Never sort the array you are editing by position; sort a copy for display only.

## 5. ImageMagick: gravity and signed offsets

**Trap.** `-gravity center -crop WxH-X-Y` **clips** instead of moving the window, so the
background never covered the canvas and the PNG grew transparent wedges.

**Rule.** Crop with **absolute** offsets computed in Rust and clamped to the margins. If
output has alpha where it should be opaque, suspect a crop that went out of bounds.

## 6. Cache ceilings are a function of the job, not a constant

**Trap.** `magick: cache resources exhausted` on ordinary phone photos. The memory ceiling
was 512 MiB; one 5000×3750 Q16-HDRI image alone is ~300 MB. Worse, the background was
upscaled to `zoom × canvas` before cropping: a 7550×6300 canvas at 1.7× needed a
12835×10710 intermediate (~2 GB).

**Rule.** Bound intermediates: crop the source first, then scale to the target. Work at
depth 8 for composition. Set cache limits with headroom (2 GiB memory, 4 GiB map) and test
the largest configuration the UI allows (100 % padding, 3× zoom), not the default one.

## 7. Mask the layer you mean

**Trap.** Corner radius was applied to the finished composition, so the *background* lost
its corners while the photo stayed square — the inverse of the dialog.

**Rule.** Apply a mask to the layer the control names, before compositing. Compare the
preview and the file at the same corners: the canvas corner must stay opaque.

## 8. CSS details that look like bugs

- `outline` is drawn outside the box and is clipped by `overflow: hidden`; use an
  `inset box-shadow` ring for a frame inside a panel.
- A native scrollbar floats over content unless the container reserves it:
  `scrollbar-gutter: stable` plus right padding, and `overscroll-behavior: contain`.
- Modal forms with more children than grid rows silently overlap; count the rows.
- Lock the page (`body { overflow: hidden }`) while a dialog is open, or the wheel lands
  on the grid behind it.
- Borders used as decoration read as rendering artefacts; if a hairline is not a control
  boundary, remove it.

## 9. Verify with measurements, not impressions

What actually found the bugs: bounding boxes of canvas vs photo (centring), computed
`border-radius` per slider value, canvas dimensions for each ratio, pixel sampling of
corners for alpha and background coverage, remount counters, and reproducing the user's
exact parameters on their exact files.

**Rule.** Before claiming a UI fix, assert a number: sizes, colours, element identity.
Before claiming an engine fix, sample the output. "Looks right" is not evidence, and a
passing test that only exercises the mock is not either.

## 10. Keep the contract tests next to the contract

- `field_payload`: the Rust field table must name the kinds the UI renders.
- `the_embedded_page_is_the_built_one`: the binary must embed the built bundle.
- `field-parse.test.ts`: the reader must keep every kind and property.
- `gradient::tests`: the engine's conventions (CSS angle, stop positions) stay pinned.

These caught real regressions during the day; each was written the moment its bug was
understood.

## 11. Small process rules that paid off

- Renaming a field key means updating the payload test, the mock, and the engine's
  allowed-parameter list; the test caught a stale `padding` assertion immediately.
- `clippy -D warnings` catches the leftovers of a refactor (an unused local) before CI.
- When a test run fails once and passes twice, capture the log (`> /tmp/preflight.log`)
  instead of assuming flakiness.
- A brief written summary of each round (cause → fix → measurement) is what let the user
  trust the next build; keep writing them.

## 12. Judgement calls worth reusing

- Prefer proportions over pixels for anything that must survive a resolution change.
- A slider's maximum must mean something (100 % = fully rounded); arbitrary caps read as
  broken controls.
- A drag should move the thing under the pointer in the direction of the pointer, unless
  the UI explicitly says it is moving the frame.
- When the user says "it still doesn't work", first prove *which build* they are running,
  then reproduce their exact inputs. Twice, the answer was a stale process, and twice it
  was a thumbnail-based preview.
