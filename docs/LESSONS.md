# Lessons from building ConvertMe

Written after a day of interface work on `add background`, where most of the time went
into failures that looked like something else.

The transferable half of this lives in `~/.omp/agent/AGENTS.md`, which every session loads
regardless of repository. This file keeps the project-specific detail: names, files,
commands, and the exact measurements that proved each fix. Each item states the trap, the evidence
that exposed it, and the rule that prevents it. Read this before touching the UI, the
engines, or the build.

Lessons 1–41 came from the first build-out, 42–56 from the design-alignment session,
57–63 from its first round of real use, 64–70 from the design/media regression, and
71–82 from the image-tools feedback round. When a rule sounds abstract, the scenario named
under **Evidence** is its executable version in `scripts/app-e2e.py` or `scripts/ui-shots.py`.

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
  instead of assuming flakiness. `could not compile <crate> (lib test)` usually comes from
  `clippy --all-targets -D warnings`, not from the tests: read the log, not the summary.
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

## 13. A memory ceiling is a safety rail, not a performance knob

**Trap.** "cache resources exhausted" was answered twice by raising ImageMagick's limit
(512 MiB → 2 GiB → 4 GiB). That makes the app need more RAM to do the same work, and the
product has to run on a 4 GB laptop. It also hides the real cost: a shadow layer the size
of the canvas, and a composed 195 Mpx file being composed again.

**Rule.** Keep the limits modest (1 GiB RAM, 2 GiB mapping, disk spill) and make the work
cheaper: bound intermediates, downscale what is blurred, skip an optional layer above a
size that the budget cannot hold, and spill to disk instead of failing. When a ceiling is
reached, the answer is a smaller working set, not a bigger allowance.

## 14. Optional effects must degrade, not fail

Above ~60 megapixels the shadow is skipped and the job succeeds; tiling or a
per-tile shadow is the honest fix and is recorded as such. A background composition on a
195 Mpx input (12500x15625 → 13500x16625) now completes in 36 s under the modest budget.

## 15. CopyOpacity copies alpha, not intensity

`-compose CopyOpacity` takes the source's *alpha* channel; a blurred grey mask therefore
needs `-alpha copy` first, and the canvas it is composited onto must start transparent
(`xc:none`), or the shadow becomes an opaque slab.

## 16. The preview must follow the selection

The preview components loaded their thumbnail once, on mount, so switching file kept the
old picture on screen while the fields changed underneath. Any preview that takes a path
reloads on that path; per-file settings are stored as *collected parameters* (never the
whole value store — a UI control such as `mode` leaked and the backend rejected it).

## 17. Evict on the clock the touch path writes

**Trap.** The preview cache sorted entries by `accessed()` for LRU eviction, but `touch()`
deliberately sets `modified()` because atime is unreliable under `relatime`/`noatime`.
Under a normal mount every file's atime was "now", so eviction picked an arbitrary entry;
`cap_is_enforced_oldest_first` failed one run in three.

**Rule.** One timestamp is the age: the one the read path bumps. Sort by mtime, and make
the eviction test set mtimes and assert the oldest is the one removed. Never mix atime and
mtime as fallbacks for the same ordering.

## 18. A switch that drives another field is a control

**Trap.** Image compress rendered "Compress to a target file size" as a plain checkbox.
`collectParams` sends every visible non-control field, so `target` reached the engine,
`check_params` rejected it as unknown, and **every** compress run from the dialog failed
validation — with the fields looking correct on screen.

**Rule.** Every field referenced by a `show_if` is a control (`control: true`) unless the
backend explicitly accepts that key; pin it with a contract test on both sides — the Rust
field table and the collector test — and assert that the engine rejects the leaked key.
The mock must carry the same flag, because the mock travels the reader.

## 19. A preview of a compressed file must be the encoded file

The compress dialog first had no preview. An estimate derived from a thumbnail would repeat
lesson 3 (preview maths ≠ engine maths), so the dialog encodes the real file once and shows
its true size, dimensions and pixels. The engine keys the artifact by source identity
(canonical path, mtime, size) **and** the normalized parameters, so the identical job copies
the preview bytes instead of encoding again. The test proves reuse by making the source
unreadable before the job: only a cache hit can finish.

## 20. A default must be relative to the file

**Trap.** Video and audio compress shipped a fixed `target_bytes` default of 20 MB. The
dialog always sent it, so "compress" a 62 KB clip with the defaults produced **131 KB** —
the feature inflated the file it was meant to shrink. Image compress had already learned
this (percentages of the shorter side); the AV table had not.

**Rule.** When a field's sensible value depends on the input, express it as a proportion
and compute the absolute number in the engine from the real `size`. An absolute number in
a field table is only correct when the user opted into it.

## 21. A knob the engine ignores is a lie

**Trap.** The video compress dialog offered "Or bitrate", but `compress_video` never read
`bitrate`; the audio dialog offered `bitrate` on WAV/FLAC, where the lossless branch
ignored it and wrote the same file back. Both "worked" and did nothing.

**Rule.** Every field in the table must be read by that category's engine code. Split a
shared field table per category instead of offering controls one branch drops. When an
operation cannot honour the control for a format (a bitrate on lossless audio), refuse
with a message naming a format that can — never silently no-op. When the control is dead
only for some inputs, hide it for those inputs: image compress hides Quality when every
selected file is lossless (PNG/BMP/TIFF/SVG); the engine pins the same list in
`lossy_extension_list_matches_the_dialog`.

## 22. A relative target must respect the encoder's floor

**Trap.** 50 % of a 41 kbps OGG is 20 kbps, and libvorbis refuses to open below 32 kbps:
`encoder setup failed`, exit 234. The same shape appeared in video (`a 0.0 MB target is
too small for 2.0 s of video`) because the audio stream alone exceeded the whole target.

**Rule.** Clamp a derived bitrate to the codec's minimum (`min_audio_kbps`) and degrade the
other streams (give video what is left of the audio budget) instead of failing a request
the UI lets the user make. Test the smallest input the UI can select, not a comfortable
one.

## 23. An empty path component is not a directory

**Trap.** The CLI was given `photo.png`; `Path::parent()` returns `Some("")`, `"".is_dir()`
is false, so the destination check declared the source folder unwritable and published to
`~/Downloads/ConvertMe`. The GUI was unaffected (absolute paths), which is why it survived.
Found by running the compress matrix from a temporary folder with a relative name.

**Rule.** Normalise a path's parent to `"."` before asking whether it is writable, and test
the CLI with a relative filename, not only with absolute fixture paths.

## 24. Quality is fidelity, not compression

**Trap.** The Compress dialog's slider reads 1–100, which invites "100 % = smallest".
It is encoder quality: at 100 a 5.4 MB JPEG was re-encoded to 17.9 MB (+239 %), because
the source had been saved at a much lower quality. A re-encode cannot recover detail; it
only spends bytes.

**Rule.** A tool called Compress never returns a larger file. Compare the artifact with
the source and keep the source when the encoder grew it (video, audio, PDF and images now
all do this). Say "no change" in the preview instead of a negative saving. When the source
is lossless, ask the encoder for its best *lossless* settings first — PNG zlib 9 (a few
percent), TIFF LZW (~half an uncompressed TIFF) — rather than a plain re-encode that
changes nothing.

## 25. A cache key must cover the code that produced it

**Trap.** Preview artifacts are keyed by source identity plus parameters. After the image
compressor changed (lossless settings, never-grow), the same request produced different
bytes while the old artifact still matched the key — the dialog would keep showing the old
17.9 MB preview.

**Rule.** Version the key when output behaviour changes (`COMPRESS_CACHE_VERSION`), or
clear the cache. The source mtime covers the input, never the algorithm.

## 26. ImageMagick is not a PNG optimizer

**Trap.** PNG compress wrote the stream ImageMagick's encoder produced (zlib 9 at best)
and called it done: 96,910 bytes for a 2000×3000 gradient. A real optimizer (oxipng,
trying row filters and deflate strategies) reached 81,917 without changing a pixel; a
screenshot went 112,129 → 67,606. The encoder was the bottleneck, not the image.

**Rule.** For PNG, run a real optimizer over the encoded file (oxipng, in-process, with a
timeout and a size cap) and keep the smaller result. Do not compare against a website's
number without checking whether it is lossless: a 16-bit 2000×3000 poster shrank to 18 KB
there by reducing it to a 177-colour palette — lossy colour reduction, not compression.
If that is wanted, make it an explicit "reduce colours" choice.

## 27. A prewarmed window must not consume launch state

**Trap.** The wheel window is created hidden at startup. Its `boot()` command drained the
pending launch paths, so `convertme photo.jpg` opened the main window with an empty
selection — the file had been consumed by a window that never uses it. This was invisible
until the app was driven with a file on the command line.

**Rule.** Launch state belongs to the window that acts on it. Take it only for the main
window (or pass it explicitly); a prewarmed/auxiliary webview may read settings but must
not drain pending data.

## 28. Never compare file sizes across formats

**Trap.** "Compress never grows a file" compared the produced PNG with the source SVG and
kept the 502-byte SVG, renaming it `.png`. The dialog then showed a broken image: a PNG
extension with SVG bytes.

**Rule.** Apply the never-grow rule only when the container is unchanged. A format change
(SVG→PNG) has no comparable size, and an artifact must always be in its stated format;
test the magic bytes, not just the name.

## 29. Drive the UI without the desktop

**Trap.** Clicking the real Tauri window through X11 (XTest) looked possible, but the
pointer belongs to the live session: the user's mouse moved between warp and click, and
the prewarmed transparent wheel window sat above the app. Two clicks landed on the wrong
row; nothing in the app was at fault.

**Rule.** For repeatable UI checks, drive the built front end in headless Firefox over
Marionette (clicks in the DOM, screenshots to files). Reserve OS-level input for a
one-off, supervised check. A screenshot harness must assert numbers (modal inside the
viewport, no horizontal overflow, preview image loaded), not only save pictures.

## 30. A lossy option needs its own writer, its own default and its own format gate

**Trap.** "Reduce colours" looks like a flag to pass to the encoder. ImageMagick's
`-colors` is a weak quantizer (74 KB at 256 colours on the poster where libimagequant
reaches 17 KB) and still writes a truecolour PNG unless asked for an indexed one. Feeding
its output back through oxipng cannot recover the palette space it never used.

**Rule.** Use the real quantizer (imagequant, the libimagequant behind pngquant) and write
the indexed stream yourself with an explicit palette, tRNS for partial alpha and an sRGB
chunk. Keep the option **off by default** and hide it unless every selected file is a PNG;
the engine rejects `colours` for any other output rather than writing palette bytes under
a `.jpg` name. Run the palette step **before** the never-grow comparison, so a smaller
lossy artifact wins but a palette that grew still falls back to the source. One
`produce_compress` function feeds both the preview and the job, so the two cannot drift.

**Evidence.** 2,000×3,000 16-bit poster: lossless 96,910 → 81,917 B; ≤256 colours (162
used) 16,955 B; 64 colours 16,802 B. Tests pin colour type 3, palette ≤ the requested
size, dimensions unchanged, changed pixels, and alpha through tRNS.

**Trap.** Dithering a small palette over a smooth gradient can exceed the *lossless*
encode. The never-grow rule then copies the source, and choosing 16 colours appears to do
nothing (81,917 B out, same as lossless). The target search had the same shape: it ran
first with ImageMagick's quantizer and only reduced the palette afterwards, so it picked a
42 KB downscaled file while the palette pass could have made 11 KB at full size.

**Rule.** When a palette artifact is not smaller, retry the remap with dithering 0 and
keep the smaller result; a compressor may prefer less dithering to a larger file. When a
target and a palette are both requested, one search must own both — quantize at the chosen
palette, downscale, repeat, and keep the lossless candidate — not two stages that each
pick a local optimum. Then compare the final artifact with the source exactly once.

**Evidence.** Poster, 16 colours: 81,917 → 11,083 B. Target 1,000 B: 42,423 → 1,597 B
(1,445×2,168, 2 colours); 256+target and 16+target both 1,597 B. Tests pin the flat
fallback on a gradient, target+palette ordering, and preview bytes == job bytes.

## 31. A note about the preview must wait for that preview

**Trap.** The "target out of reach" note was computed from the *new* `target_bytes` and the
*previous* preview's byte count. During the 400 ms debounce it compared 1,000 B with the
old 1.4 MB artifact while the facts still showed 1.4 MB. Reactive values mixed two
generations of data.

**Rule.** Every preview carries a stale flag: set it when the request key changes, clear
it only when that request's result arrives. Anything derived from the artifact (notes,
warnings, comparisons) is suppressed while stale. Do not use a debounce timer as the
staleness signal; the timer starts *after* the user edits.

**Evidence.** Headless harness: after ticking both options and typing 1000, the note and
the facts now agree (`4032×3024 · 8.8 KB · −99 %`, note present), modal 640×786, no
overflow. Before the flag, the note appeared while the facts still read 1.4 MB.

## 32. A harness that fakes the backend can only prove the fake

**Trap.** The headless harness drove every dialog and asserted modal geometry, field
labels, preview loading and even the unreachable-target note — all against the mock. It
reported the target as met while the real engine returned 42 KB, and had no way to see
that a both-options run wrote 81 KB. The screenshots and the report were green; the
artifact was wrong.

**Rule.** Keep two layers, and name what each proves. The mock harness proves rendering,
field collection, visibility and layout. The real-app harness must launch the release
binary, drive the real window, run the job, and validate the **published file**: magic
bytes, dimensions, colour type, palette, alpha, sampled pixels, pages/frames/streams,
metadata, and never-grow. Preview numbers are only evidence when they equal the job's
bytes. Every scenario records a screenshot plus a machine-readable result, and a skip is
reported with a reason, never omitted.

**Evidence.** The user's report was the missing test: target 1,000 B, 16 colours, both
options. Only running the engine CLI on the exact poster exposed the dithering fallback
(81,917 B) and the weak target search (42,423 B). The mock could not have caught either.

**Implementation note.** `WebKitWebDriver` is not shipped by `webkit2gtk-4.1` and
`tauri-driver` has nothing to attach to, so the harness uses AT-SPI under Xvfb. The
probe's first reading — "tool cards are unnamed list items" — was wrong: the list item
is the wrapper, the child is a named push button (§33). `scripts/app-e2e.py` now drives
the real window, runs the jobs and validates the artifacts; the recipe (export
`AT_SPI_BUS_ADDRESS`, start `/usr/lib/at-spi2-registryd`, launch with `NO_AT_BRIDGE=0
GTK_A11Y=1`) and its environment gotchas are in `docs/HANDOFF.md`, "Real-app end-to-end
harness".

## 33. The accessibility tree is the GUI test API

**Trap.** `WebKitWebDriver` is absent on this machine, and the first probe reported the
tool cards as unnamed `list item`s. That read as "the GUI cannot be driven". The
unnamed node is only the wrapping `<li>`; its child is a named push button
(`"01 Compress Hit a target size…"`) with a `press` action.

**Rule.** Dump roles, names, states and actions before concluding a toolkit is
untestable. Drive buttons through the AT-SPI action interface, keep a virtual-display
coordinate click only as a fallback, and read text with
`Atspi.Text.get_text(iface, 0, count)`: in PyGObject `Accessible.get_text()` is a
deprecated interface accessor, not the string.

**Evidence.** `button '01 Compress …'` + `do_action(0)` opened the real dialog; the
category toggles, target buttons, check boxes, sliders and entries all expose names and
actions, enough to run and validate the jobs without WebDriver.

## 34. A fallback card is not a registered tool

**Trap.** The UI renders `FALLBACK_TOOLS` when `wheels()` returns no tools. Those cards
look identical to real ones, but the backend has no field table or engine for them.
Clicking Compress on Documents runs an empty-parameter job. Images → Metadata did the
same because `fields_for` had no `(Images, Metadata)` arm; the card failed
`unsupported: nothing to change` until the table was added.

**Rule.** Derive matrix coverage from the backend registry, not from the cards on
screen. Distinguish an intentional zero-field tool (Mute, Merge, Extract) from a missing
field table; report the latter as a finding/skip with the observed error. A card that
looks runnable but has no fields is a product gap, not a harness detail.

**Evidence.** `dialogs-documents` … `dialogs-subtitles` skip with "backend registers no
tools"; `dialogs-images` used to fail on Metadata with
`metadata · failed / unsupported`, and now passes after the field table was added
(`tool-image-metadata`). Without engines, the app shows fallback labels
(`Compress to size`, `Edit metadata`) and Settings names the missing tool.

## 35. Never-grow must compare the bytes you publish

**Trap.** Scale 50 % on the 16-bit indexed screenshot returned the original at
1600×1000. The resized ImageMagick encode was 122,328 B, larger than the 112,129 B
source, so `prefer_smaller` copied the source; `optimize_png` then shrank that copy to
67,606 B. The optimized resize would have been 96,639 B — comfortably below the source.
The comparison ran on an intermediate, so a user control was silently ignored.

**Rule.** Order the pipeline so the size decision sees the final bytes: encode →
optional palette → optimize → compare with the source → publish (or copy the source).
A never-grow rule that runs before the optimizer can discard a legitimate resize.

**Evidence.** `convertme tool shot.png --tool compress --params '{"scale":50}'` →
1600×1000, 67,606 B. The harness's shared multi-file run scaled the poster but not this
file; `multi-file` pins the failure.

## 36. Capture state before the modal, and wait for the artifact

**Trap.** Two timing errors made the full suite report 19 false failures while each
scenario passed alone:

- History rows behind an open modal are not exposed by AT-SPI. `before_rows = []`, so
  every old failed row was "new" after Run.
- Zero-field tools start the job on the card press. Snapshotting the work directory after
  the dialog wait missed the published folder.

**Rule.** Read history and snapshot the work directory **before** opening a dialog or
pressing a job-starting card. Do not toggle Activity behind a modal. Wait for the new
artifact's own bytes/dimensions (or the terminal history row), not for derived text: the
compress note can render one frame before the preview effect marks the artifact stale,
pairing the new target with the old size.

**Evidence.** `history_after_show` had rows, `history_before` was empty behind the
dialog; capturing before the dialog removed the false failures. `extract-archive` went
from 180 s timeout to 6 s once the pre-click snapshot was used. The poster milestone only
settled when waiting for `1445 × 2168`.

## 37. Isolate HOME, but mirror the tool roots it points at

**Trap.** `target/release/qpdf` is a wrapper that execs
`$HOME/.local/opt/qpdf/bin/qpdf`. Under the throwaway profile, PDF merge/split failed
with "No such file or directory" while the same jobs work in the real session.

**Rule.** Isolate app state (HOME/XDG), then symlink the real `~/.local/opt/<tool>`
roots into the profile and record what was linked. The profile is disposable; the
machine's external tools are part of what is under test.

**Evidence.** Without the links, `merge · failed / engine_failed: … qpdf: … No such file
or directory`; with them, PDF merge kept all 8 pages and split wrote 5 files.

## 38. Build identity is a chain, not a screenshot

**Trap.** Settings showing an interface build id does not prove which binary answered,
and the Tauri asset is compressed, so `grep "Interface build" target/release/convertme`
finds nothing.

**Rule.** Compute the UI stamp with `build.rs`'s algorithm from `ui-dist`, assert those
stamp bytes are in the release binary, read the interface build id from the running
Settings page, and compare `/proc/<pid>/exe` with the launched path. All four must agree;
a screenshot alone is not evidence.

**Evidence.** stamp `29568232a6` in the binary; Settings `2026-10-30 14:20` equals the
bundle literal; `/proc/<pid>/exe` is `target/release/convertme`; `--version` 0.1.0 equals
the Settings line.


## 39. A field that is filtered out of the renderer is a dead control

**Trap.** ToolModal's `visibleFields` drops every `boxes` field (`key !== 'boxes'`) and
only Crop had a dedicated editor. Redact therefore opened a complete-looking dialog with
a colour field and a no-op "Find text automatically" checkbox (since removed), but no way to mark a region;
every run failed "no boxes to redact". The dialog matrix passed because the dialog
opened and Cancel worked.

**Rule.** If a renderer filters a field kind, every tool that owns that kind needs its
own editor. A dialog opening is not evidence that a tool works: run each tool's job.
Export and reuse the shared editor (the same `BoxesEditor` now serves Crop's sibling
Redact) rather than leaving the field invisible.

**Evidence.** Redact's a11y tree had only Fill colour and the checkbox; after rendering
`BoxesEditor`, the Detect faces/text buttons and a Preview image appear, and
`tool-image-redact` (OCR + drawn region) and `tool-video-redact` pass.

## 40. A removed upstream option breaks the sidecar path silently

**Trap.** The filmstrip command passed `-vsync vfr`. ffmpeg 9 removed `-vsync`; the
command exited 8 with "Unrecognized option 'vsync'", so the filmstrip cache stayed
empty. Video previews fell back to "Preview unavailable", and the Redact canvas could
not be used. ImageMagick also hung trying to decode the video before the fallback ran.

**Rule.** Pin and record the sidecar versions in the harness report, exercise the
preview/filmstrip paths (not just the final job), and use options that exist in the
bundled version. For a video preview, ask the video path (filmstrip) directly instead of
letting a still-image decoder fail first.

**Evidence.** The exact ffmpeg invocation returned rc 8 with `[]`; removing `-vsync`
produced `frame001.jpg`; `preview_thumb` now uses `filmstrip` for `Category::Video`, and
`tool-video-redact` passes.

## 41. Mutually exclusive edits must be sent as one representation

**Trap.** CropEditor fits the preview box to the selected aspect ratio and calls
`onChange`, so the dialog held both `boxes` and `aspect`. `submitModal` forwarded the
rectangle as x/y/w/h alongside the ratio, and the engine rejected every aspect crop:
"use either a crop rectangle or an aspect ratio, not both". The control looked
functional; only running it exposed the conflict.

**Rule.** When two representations describe the same edit, submit exactly one. Make the
branch explicit (free crop → rectangle; fixed ratio → aspect only) and test both. A
preview-fitted rectangle is not user input and must not travel as one.

**Evidence.** `tool-image-crop` failed before the fix; after it, 1:1 produces
2,000×2,000. `tool-video-crop` exercises the same branch.

## 42. An ImageMagick setting is process-global, even across `( … )` groups

**Trap.** Collage composed every cell with `-gravity center -extent` inside a parenthesised
group, then placed it with `-geometry +x+y -composite`. The composite used the
group's `center` gravity, so every picture landed in the middle of the canvas and the
rest was background. The engine test sampled each quadrant and saw `(0,0,0)`.

**Rule.** Reset inherited settings explicitly before an operation that depends on them:
`-gravity NorthWest` before `-geometry`. A group scopes the image list, not the
settings.

**Evidence.** Before: quadrants `(0,0,0)`; after: `(255,0,0)`, `(0,128,0)`, `(0,0,255)`
and the background gutter, `2048×2048` with 16 px spacing and 1000 px cells.

## 43. A preview must call the same engine function as the job

**Trap.** The photo dialog drew its sliders with CSS filters and a tint overlay while the
job used the ImageMagick chain; gamma and grain could not be approximated, so the preview
and the saved file disagreed. The new editor controls (grain, noise, clarity) made the
gap impossible to hide.

**Rule.** One function computes the filter arguments (`photo_filter_args`), and both the
job and `photo_preview` call it. The preview is debounced and a stale response is
dropped by a request token. `tool-image-edit` measured mean luma 0.398 → 0.731 for +1 EV
(≥10 % required), grain 100 raised variance and noise 100 lowered it.

## 44. A redaction style is a mode switch, not a parameter

**Trap.** The Style segmented control drove the colour field's `show_if`, but the field
table contract says a `show_if` driver is a control — and controls are not submitted.
Marking it a plain parameter left the colour visible in blur mode (a dead control);
marking it a control without a hand-off dropped `style` from the job entirely.

**Rule.** Send the value explicitly from the modal (`params.style = values.style` for
redact) so the switch can be a control. The engine validates `style` and rejects unknown
keys; `solid` fills the box, `blur` uses `-region … -blur`. `tool-image-redact` asserts
the solid centre is black and the blurred centre differs from both the source and black.
## 42. A CLI verb that is missing from the dispatch list hangs a headless run

**Trap.** `main` decides between the CLI and the window from a hardcoded list of verbs.
A new `preview` subcommand existed in `cli::run` but not in that list, so
`convertme preview …` launched the *window*; without a display it waited forever. The
performance harness had already been running for seven minutes on its first case before
anyone looked at the process list.

**Rule.** One list, in one place (`cli::is_cli_command`), used by `main` to route and by
`cli::run` to dispatch. And a harness must not be able to hang: pass a per-run timeout to
every child process, so a mis-dispatch fails in seconds with a message instead of
blocking the gate.

**Evidence.** The hung child was `convertme /tmp/…/photo.jpg --kind thumb --size 512`,
i.e. a path where a verb should have been. With the verb shared, the same case reports a
number in ~90 ms.

## 43. A file copied at build time is a second build to prove

**Trap.** Tauri copies `externalBin` next to the executable at build time. Rebuilding
the PDF sidecar and installing it into `vendor/bin` looked like an update, but the app
kept running the copy in `target/release/` from the previous build. PDF→DOCX still
produced the old document (one Heading 1, no list items) while the same sidecar binary,
run directly, produced 7/4/29.

**Rule.** When a build step copies an artifact, the copy is part of "which build is
running": rebuild the consumer or refresh the copy, and record the *running* binary's
version in the harness report (`tool_versions` now includes `convertme-pdf`). A sidecar
should print a build stamp, not just a semver.

**Evidence.** `target/release/convertme-pdf` was 4 minutes older than
`vendor/bin/convertme-pdf-x86_64-unknown-linux-gnu`; after copying, the same scenario
reported 7 Heading 1, 4 Heading 2, 29 list items, coverage 1.0.

## 44. A mock with its own copy of the field tables is a second product

**Trap.** The browser mock kept its own hand-written `TOOL_FIELDS`. It drifted from the
Rust table: the crop dialog showed a `boxes` editor the real dialog hides, so the
screenshot harness measured a dialog with the surface drawn twice, and the "Fill colour"
row measured 19 px out of alignment for a control the app does not have.

**Rule.** Generate the mock's tables from the product's own table
(`ui/src/fields.generated.json` from `convertme probe --fields`) and fail a test when the
file is stale. A mock may fake *data*; it must not fake *structure*.

**Evidence.** After the switch, `ui-shots` went from "31 dialogs of 50 tools" with three
alignment failures to "26 dialogs of 51 tools, no layout failures", and the crop dialog
shows one "Area to keep" surface.

## 45. A tool card is a promise that a dialog opens

**Trap.** The reference has a Collage tool, so it was added to the tool table with its
copy — before its engine existed. The card appeared on the Images wheel, ran immediately
(the field table was empty), and failed with "unsupported". Discovery would have produced
a broken card for every user.

**Rule.** Register a tool together with its engine and its fields, and keep a test that
every non-fieldless tool has a field table (`only_fieldless_tools_run_without_a_dialog`).
A card that cannot do its job is worse than a missing card.

**Evidence.** `dialogs-images` failed with `Images/Collage: dialog opened — collage ·
failed · unsupported`; after removing the card, the same scenario passes.

## 46. PDF → DOCX is a structure problem, not a text problem

**Trap.** The first writer emitted one paragraph per block and a Heading 1 for the
largest font on each page. A résumé became 36 paragraphs, 1 heading, no lists, no images
— text-complete and structurally useless.

**Rule.** Decide the structure once, in an analyser both writers share
(`sidecars/pdf/convertme_pdf/reflow.py`): body size from character counts, heading levels
from size rank, list items from bullet glyphs, sub-entries from an all-bold first line,
icons from short spans with no alphanumerics (rasterised, never written as text), and no
page breaks at all. `scripts/docx-quality.py` is the contract: coverage ≥ 0.98, one
Heading 1, two Heading 2s, ten list paragraphs *and* ten `w:numPr`, four drawings, zero
page breaks, and within ±1 heading of the reference.

**Evidence.** Before: 36 paragraphs, 1 H1, 0 lists, 0 numPr, 0 images. After: 68
paragraphs, 7 H1, 4 H2, 29 list paragraphs, 29 `w:numPr`, 14 drawings, 0 page breaks,
coverage 1.0 — against a reference with 8 H1, 4 H2, 30 lists, 29 numPr.

## 47. `w:pStyle` is not numbering

**Trap.** Python-docx's `List Bullet` style renders bullets, so the DOCX *looked* like a
list — but the package had zero `w:numPr`, so the document did not declare its own lists
and any tool reading it saw plain paragraphs.

**Rule.** Write the numbering, not just the style: create a `w:num` with a bullet
`w:abstractNum` and point every list paragraph at it. Measure `w:numPr`, not the visual
result.

**Evidence.** `numpr: 0` with `lists: 29` before; `lists: 29, numpr: 29` after the fix.

## 48. ImageMagick's identify format is not a template language

**Trap.** The shared validator asked for `{width}|{height}|…`; ImageMagick printed the
braces back, and the audit died on `int("{width}")` rather than reporting a fact.

**Rule.** Use the tool's own specifiers (`%m|%w|%h|%[colorspace]|%[type]|%z`) and parse
defensively: a validator that raises reports less than one that says "unreadable".

**Evidence.** `magick identify -format '%m|%w|%h|%[colorspace]|%[type]|%z'` returns
`PNG|2000|3000|sRGB|TrueColorAlpha|16`.

## 49. The first measurement attempt is part of the harness

**Trap.** The performance harness passed the file path where the subcommand belongs
(`convertme <path> --kind thumb`), which opened the window and blocked. Nothing measured
anything, and the run looked like a slow benchmark instead of a broken command.

**Rule.** Give every measured child a timeout (`timeout=`), print the exact argv on
failure, and make an unavailable case say so (`unavailable: <stderr>`) instead of
waiting. A harness that cannot fail fast is a harness that lies.

**Evidence.** The hung job showed `convertme /tmp/…/photo.jpg --kind thumb --size 512`
as its child; with the verb and a timeout the whole suite finishes in about a minute.

## 50. "Can set" and "can delete" are different capabilities

**Trap.** The metadata browser asked the writer one question — `supports_remove` — and
used the answer for both editing and deleting. PNG and WEBP are written through
ImageMagick, which sets a tag but can only clear whole profiles, so the answer was no and
every row rendered as read-only text: setting a title on a PNG became impossible, and the
dialog showed no text fields at all.

**Rule.** A capability is per operation. `editable` (the writer can set this tag) and
`removable` (the writer can delete this tag alone) are separate fields; the input is
shown whenever the first holds, the minus button only when the second does. A field that
says `editable: false` must mean "the writer refuses this", not "we were not sure".

**Evidence.** The a11y tree of the Images metadata dialog had `list item` + `label` and no
`entry` at all when the rows were read-only. After the split: `tool-image-metadata`
passes (Title written to a PNG, read back, pixels identical).

## 51. A preview cache key must cover the whole request

**Trap.** The preview guard cancelled requests by file alone, so the crop canvas (one
filmstrip frame) and the timeline (twelve frames) cancelled each other: the canvas showed
no preview, and the timeline showed frames. Both were "the same file".

**Rule.** Supersede-by-key, and let the key be the request: file *and* kind *and* the
parameters that change the output (frame count, size, params, version). Two previews of
one file are normal — the audio dialogs ask for a waveform and a channel split at once.

**Evidence.** The video crop canvas was empty with a single-slot guard; with
`filmstrip:<path>:<frames>` it renders, and the audio dialogs keep both waveforms.

## 52. A dynamic loudness pass is not a target

**Trap.** `loudnorm` ran once for files under twenty seconds and twice for longer ones.
The single pass is dynamic: it landed at −17.07 LUFS for a −16 target, and the acceptance
is ±1 LUFS — the approximation ate the entire tolerance.

**Rule.** Measure, then apply: always two passes, feeding `measured_I/TP/LRA` into the
second. The cost is one extra decode of a file the user explicitly asked to re-encode.

**Evidence.** `tool-audio-normalize`: −17.07 LUFS before, within −16 ±1 after, with the
same duration and the true peak under −1.5 dBTP.

## 53. A control that needs a prior gesture is a dead control

**Trap.** The video crop dialog's Width/Height inputs returned early when no rectangle
existed yet, so they displayed `0` and typing did nothing. The reference dialog's whole
point is "enter exact output dimensions" — first.

**Rule.** Seed the thing the control edits. Typing a width with no box creates a centred
full-frame box and then applies the width; the control works on the first keystroke.

**Evidence.** The inputs now change the rectangle without a drag; the engine's free-crop
path is measured by `quality-audit.py --only video-crop` (640 × 360 from 0.25/0.25/0.5/0.5).

## 54. The accessibility tree is only what is on screen

**Trap.** A dialog's text was reported missing by the real-app harness even though a
screenshot showed it: WebKit prunes elements scrolled out of the sheet from the
accessibility tree, so "not in the tree" meant "not scrolled into view". The same trap hit
every synthetic drag on a surface below the fold.

**Rule.** Scroll with the wheel over the sheet before reading content that may be below
the fold, and scroll a drag surface into view before dragging it. When a canvas still
refuses synthetic pointer motion, say so in the scenario and measure that behaviour at
the engine layer (`quality-audit.py`) instead of pretending the dialog was verified.

**Evidence.** `scroll_dialog` plus `ensure_visible` turned the normalize readout and the
video crop canvas from "absent" into present; `tool-audio-normalize` and
`tool-video-crop` pass, and `audio-bleep`/`video-redact` name the part the audit covers.

## 55. The floor of a search is one measurement, not the walk

**Trap.** The compress target search walked palette 256 → 2 across up to three
downscales to find the smallest image it could make. When the target was unreachable
(a 1,000-byte poster) that walk was the answer: thirteen seconds to learn "no".

**Rule.** Compute the *floor* first — the smallest artifact the format can produce (two
85 % downscales and the minimum palette) — and compare it with the target before walking
anything. If the floor is above the target, nothing in the ladder can be below it, and
the floor is the result. Reaching the same artifact by the shortest path is what keeps
the reachable cases byte-identical.

**Evidence.** `compress-target-1000b-poster`: 13,445 ms → 1,267 ms, and the pinned
1,597 B artifact for a reachable target is unchanged (`compress-poster-16-target`).

## 56. A measurement that includes setup measures the setup

**Trap.** The preview command re-probed the host on every invocation: ten `--version`
spawns per measurement. A 186 ms ImageMagick preview was reported as 777 ms, and the
team nearly optimised the encoder instead of the wrapper.

**Rule.** A measurement harness must run what the app runs. The app probes once at
startup and reuses the capability cache; the CLI's preview seeds the same cache and then
does only the work. Subtract process start explicitly, and print that subtraction — the
number is only comparable if the reader can see what was removed.

**Evidence.** `photo-preview-1024`: 907 → 201 ms with the probe removed and the JPEG
decoder asked for a 2× image (`jpeg:size`), which is what a 1024 px preview can afford.

## 57. A design reference is a reference, not a spec

**Trap.** The Feedback_1 captures were read literally: a two-pane dialog with the tool's
copy on the right, a close button at the top left, a fixed 600 px sheet. The rail ate a
third of the screen, the dialog could no longer be moved, and the × sat where no Linux
or Windows window puts it — the reference screenshots are macOS, and their side panel is
annotation, not product.

**Rule.** Take the *grammar* from a reference (rows, labels, one primary action) and the
copy, but keep the platform's conventions and the product's own affordances: a dialog
inside the window, draggable by its head, resizable from its corner, closing from the
top right, with the tool's explanation left to the card that opened it.

**Evidence.** The rail is gone (`About this tool` no longer in the accessibility tree);
every dialog measures 600×h centred in the window, while the window itself stays visible.

## 58. A tall control in a stretched grid row paints over the next row

**Trap.** `.modal-fields` is a grid; each `.field` is an item, stretched to its track.
A 40 px track with a 72 px control (the colour picker's swatches plus hex, the gradient
editor, the crop readout) did not grow the row: the control overflowed and drew on top
of the following field. The dialog looked like the screenshot was corrupted — and no
screenshot test noticed, because both rows existed.

**Rule.** A row must be as tall as what it holds: in a flex column, `flex: 0 0 auto` on
the item lets its own content decide the height. **Do not use `align-self: start` for
this** — in a flex column that is the horizontal axis and shrink-wraps the row (the
regression in lesson 64). Measure *overlaps* between sibling controls, not just their
presence, and treat an overlap as a layout failure.

**Evidence.** The harness's overlap probe reported `#ffffff / 0` and `#101418 / 4`; the
colour row measures 76 px with the control inside, and the images pass reports no
overlaps.

## 59. Percentages on a letterboxed preview are not percentages of the picture

**Trap.** The crop box was positioned as a fraction of the *surface*, while the picture
inside it was `object-fit`-letterboxed. A 1:1 preset therefore drew a box that was not
square in the file, and 9:16 looked square on screen. The engine used fractions of the
source, so preview and output could not agree.

**Rule.** Put the overlay on a frame that is exactly the rendered picture (a shrink-to-
fit wrapper around the `<img>`), and compute every fraction against that frame. Verify
with numbers: the chosen ratio must match both the box on screen and the pixel dimensions
the dialog will submit.

**Evidence.** `crop-1:1` → screen ratio 1.000, 3024×3024 px; `crop-9:16` → 0.562,
1701×3024 px. `ui-shots.py` asserts both every run.

## 60. A reader and an editable-tag table must use the same names

**Trap.** `editable_tags` advertised `Title`, `Description`, `Artist`, `Copyright` for
images, but the reader inserted raw EXIF names, and PNG text chunks were not read at all.
The metadata form showed empty rows for a file that had the value, so "does it fill what
is already there?" had no answer.

**Rule.** One vocabulary: the reader maps its container's keys onto the same names the
writer accepts, and the harness proves it by writing a tag, reopening the dialog on the
output and reading the field back.

**Evidence.** `png_text_chunks_are_read_as_the_forms_own_keys` (a `Comment` chunk reads
as `Title`), and the metadata scenarios now reopen the published file and check the
`Title` entry reads `Harness`.

## 61. Never decode media in the web view

**Trap.** Pressing Play mounted `<video>` on the source clip. WebKitGTK's media stack
(GStreamer without a working sink) took the whole process down, so a transport button
could kill the app. Replacing it with an off-DOM `new Audio(...).play()` was not enough:
initialising the backend alone wedged the web process, the dialog vanished from the
accessibility tree, and the harness could no longer press Save.

**Rule.** The window must not depend on the platform decoder to stay alive. Video is a
sequence of stills, and an audio preview is played through a headless system sink (see
66); neither needs a media element. A test must press Play and keep the dialog alive.

**Evidence.** `dialogs-video` keeps the process alive through Play/Step;
`tool-audio-bleep` finds the MP3 in the cache, the tone at the drawn range, and the
dialog still open after the preview (`press_primary` now retries for 3 s because the
earlier attempt lost the tree).

## 62. A keyed list of inputs loses the character being typed

**Trap.** The redaction rows were a `<For each={boxes()}>`. Every keystroke replaced the
box object, so `<For>` (keyed by identity) re-created the row — including the `<input>`
under the caret. Typing "0.5" left "0": the first character landed, the element was then
destroyed and rebuilt from the old value.

**Rule.** An editable list is keyed by *index* while it is being edited (`<Index>`), or
its rows are mutated in place; rebuilding a focused control on input is how a form
becomes impossible to fill. Test it the way a user types: enter a multi-character value
and read it back.

**Evidence.** `tool-video-redact`: `Region 1 start` read `0` after typing `0.5`; with
`<Index>` the typed span reaches the job and the per-box frames change where expected.

## 63. A preview work dir cannot be deleted before the artifact is used

**Trap.** `run_tool_preview` rendered the tool into a temporary work dir and cleaned that
dir up before returning the artifact path. The audio preview then asked ffmpeg to
transcode a file that no longer existed, so `Play Output`/`With Bleeps` always failed —
and the dialog showed "The preview failed", because the rejection is a string and the
catch only understood `Error`.

**Rule.** Ownership of a temporary directory ends when its last consumer is done: return
the guard to the caller, clean up after the copy/transcode, and surface a rejected
command's string. An error is the one message the user needs; never replace it.

**Evidence.** `tool-audio-bleep` (`the bleep preview file was written`) failed before the
fix and passes after it: the cache gains the MP3 and the dialog plays it.

## 64. In a flex column, `align-self` is horizontal

**Trap.** The dialog's field list is `display: flex; flex-direction: column`. A rule
added to stop tall controls painting over the next row used `.field { align-self: start }`.
That is the *main* axis being vertical and the *cross* axis horizontal: every row
shrink-wrapped to its content. The same sheet had a 199 px select beside a 552 px slider,
a 129 px number box, and a segmented control that wrapped into a second ragged line. The
design read as "some boxes small, some extend to the end".

**Rule.** Height is controlled by `flex: 0 0 auto` on the children, never by changing
cross-axis alignment. A form row is `width: 100%`. Measure the control's width against
its row, not only its vertical centre.

**Evidence.** `ui-shots.py` now asserts a non-block control fills ≥ 90 % of its row and
starts at the label's left edge; the before-build reported 3 % for a checkbox and every
row was 367 px inside a 552 px field.

## 65. Restore the grammar, not the reference's geometry

**Trap.** The alignment session moved labels to the left of a 34 % column to imitate the
macOS reference. Short controls then sat a column away from their words, and a `<select>`
was styled as a pill. The original stacked form — label above, full-width control — was
the design the product already used.

**Rule.** Take the *grammar* from a reference (one label per control, one primary action)
but keep the product's own geometry when it is already coherent. A pill select is not a
control style; a segmented row is one track of equal cells, not a wrapper that wraps.

**Evidence.** `ui-shots.py` `31 dialogs of 53 tools, no layout failures`; the crop
aspect list is 7 × 76 px cells in one 552 px row.

## 66. `<audio>` is the same crash as `<video>`

**Trap.** Lesson 61 removed `<video>` from the video transport but left an `<audio>`
element mounted on Play in `WaveformTimeline`, and `ui/src/preview.tsx` still mounted
both. Pressing Play in Trim audio re-initialised WebKitGTK's media stack and took the
process down.

**Rule.** A media element is forbidden everywhere in the web view, including a separate
preview window. Audio goes through `play_preview` (headless `ffplay`/`mpv`/`paplay`/
`pw-play`/`aplay`, or a desktop opener only as a last resort); video walks stills.

**Evidence.** `grep -n "<audio\\|<video" ui/src` is clean. `tool-audio-trim` now presses
Play and asserts the dialog is still there afterwards.

## 67. A playhead over a static strip is not playback

**Trap.** "Play" moved the playhead across a fixed filmstrip; the picture never changed,
so the control looked broken. The speed slider changed a value that only affected the
saved file, not the preview.

**Rule.** Play must change the visible frame. Render an evenly sampled sequence at a
stated rate (`video_frames`, 10 fps for a short clip, a 240-frame budget for longer
ones), advance it by the speed factor, and bound both the frame and the playhead by the
trim span. Trim handles move the selection only; seeking and Play clamp into it.

**Evidence.** Mock trim: Play 0.45 s moved the still from frame 1 to 2 and the playhead
to 0.40; 4× speed reached 1.6 s in 0.4 s. CLI: 60 frames from a 6 s clip in 0.55 s,
cached on the second call.

## 68. A metadata writer must create the block it edits

**Trap.** `little_exif::Metadata::new_from_path` returns "No EXIF data found!" for a JPEG
with no EXIF block. `exif_direct` propagated that as an engine failure, so adding a title
to a plain JPEG was impossible. Separately, `display_value()` quotes ASCII fields
(`"A title"`) and the form rendered the quotes; a PDF's `/Info` is usually an indirect
reference, so the reader found no dictionary and the fields stayed empty.

**Rule.** Treat "no block yet" as an empty metadata object, not a refusal. Read ASCII
values from the value bytes, dereference `/Info`, decode its text strings, and map every
tag the writer accepts. Test both a file with existing tags and one without.

**Evidence.** `exif_reader_prefills_every_writable_image_tag` writes eight tags into a
fresh JPEG and reads them back unquoted; `pdf_info_behind_an_indirect_reference_is_read_and_kept`
round-trips six Info keys.

## 69. A PDF cover is a rendered page, and a failed cover is still a file

**Trap.** The selection strip asked ImageMagick for a PDF thumbnail. ImageMagick returned
a black rectangle, and `analyze` silently dropped any file whose thumbnail failed — two
selected PDFs, one black card.

**Rule.** Render PDF covers through `PreviewCache::pdf_pages`, the same renderer the page
grid uses. `analyze` emits one `Thumb` per readable input in order; an empty `thumb` is a
placeholder card, not a missing file.

**Evidence.** The new `tool-pdf-organize-multi` scenario sees both names and two cards
with two PDFs.

## 70. A page grid that loads on mount shows file 1 for every file

**Trap.** `PageOrganizer` called `pdfPages` in `onMount`. The dialog switches files
without remounting the editor, so file 2 of 2 kept file 1's count and cards — the
reported "1 pages" in a five-page report.

**Rule.** Key the load on `props.path` with a `createEffect`, reset the entries, and
ignore a response whose token is stale. Publish the new `{order, rotate}` per file.

**Evidence.** `tool-pdf-organize-multi` switches from a 5-page report to a 3-page scan
and counts 5 then 3 page cards.

## 71. When two representations describe one edit, the explicit one is the edit

**Trap.** The crop editor fitted the preset box on screen and also stored the box, but
`submitModal` deleted `boxes` and sent only `aspect` for a locked ratio — the comment
called the rectangle "a preview detail". The engine's aspect path centres, so every
preset crop ignored the placement the user dragged. The dialog showed the right box and
the file had a different one; the failure was in the branch between them.

**Rule.** Send exactly one representation, and prefer the one the user moved. If a box
exists, send `x/y/w/h` and omit `aspect`; use `aspect` only when no box exists. Keep the
engine's mutual-exclusion check: it is what makes the next leak fail instead of silently
choosing a default.

**Evidence.** `tool-image-crop`, 9:16 dragged to the left third: output `1688×3000`,
RMSE `0.00152` against the dragged reference and `0.23977` against the centred one; the
left-third patch `(156,122,190)` equals the source there and differs from the centre's
`(95,128,173)`.

## 72. A slider's sign is a measurement, not a reading of its name

**Trap.** Four tonal controls moved the wrong way, and two more were wrong in range:
`whites` raised ImageMagick's white point (darkening), `blacks` raised the black point,
`highlights` raised the sigmoid midpoint (darkening), `shadows` used `-gamma 1/(1+s)`
where ImageMagick's gamma is inverse, `dehaze` ignored its magnitude, and hue's
`100 + h/3.6` reached only ±90° although the slider spans ±180°. The labels looked
plausible; only a ramp exposed them.

**Rule.** Render a black→white ramp (and a colour/noise sample for the colour/detail
controls) at +N and −N and assert the intended region moves in the intended direction.
Pin the endpoints and the neutral value (`1.0` gamma), not just "it runs".

**Evidence.** `photo_sliders_have_the_expected_direction`: highlights +100 raises the
top 20 % from 223 to 243 while black stays 0; whites +100 takes the white point from 204
to 255; blacks +100 lifts 0 → 51; shadows +60 lifts x64 from 27 to 107; hue +180 now
emits `-modulate …,200` instead of `…,150`.

## 73. An annotation editor is an object model, not a canvas of ink

**Trap.** The first editor let a user drag a shape but its toolbar had no eraser, numbers
required a drag that a click never produced, text required pre-typing in a side input,
and the SVG used `naturalWidth/400` for strokes while the engine used raw pixels. It
looked like a drawing surface and behaved like a list of marks with no handles.

**Rule.** Every mark is an object with a source-pixel `size`, a colour, and a hit box;
the tools are Select (move + four corner handles), Eraser (click removes the topmost
shape), Text (inline entry at the click point), and Number (a click places an
auto-numbered badge). Delete/Backspace remove the selected object; Escape clears the
selection instead of closing the dialog when one exists. The overlay is drawn in the
same pixel space the engine uses. The canvas loads the **shown file's** path, not a
modal-level preview value that may be absent in the mock.

**Evidence.** `ui-shots.py` drives real pointer actions: a drag creates 1 shape, Number
adds a badge (3 SVG elements), Eraser returns to 1. `tool-image-annotate` finds the
rectangle border at `(320,370)` = `(255,90,54)` and the badge at `(1268,250)` in the
published file, not only in the overlay.

## 74. A badge is centred by the text engine, not by guessed font metrics

**Trap.** The number's digit was drawn with a hand-guessed ascent/advance offset, so it
sat at the bottom edge of the circle and moved if the label had two digits. Measuring
digit widths suggested a different constant for every glyph and every font.

**Rule.** Under MVG `gravity center`, `text X,Y` puts the text's centre at the canvas
centre plus `(X,Y)`. For a badge at `(cx,cy)` on a `W×H` canvas, draw with
`gravity center text {cx-W/2},{cy-H/2}` and reset to `gravity NorthWest`. The result is
width-independent and needs no font-metric table.

**Evidence.** `annotate_number_and_text_are_centred_and_legible` samples the badge centre
for one digit and for `10`; `tool-image-annotate` writes the badge at the click point.

## 75. Spacing and radius are proportions; rows and columns are a layout

**Trap.** Collage's corner radius and spacing were fixed pixel counts capped at 64 on a
2048 px canvas, so the controls looked broken at realistic output sizes. `columns` was
only read by two of four styles, there was no row count, and the output width was a free
text box that could not be discovered before a typo changed the resolution.

**Rule.** Express spacing and radius as percentages of the cell's shorter side
(100 % radius = a circle), and give the grid independent `rows` and `columns` controls:
explicit rows are honoured, spare cells stay background, and rows grow when the pictures
do not fit. An output size is a choice (1024/2048/4096), not free text. Round the
picture layer with a mask, as `add_background` does.

**Evidence.** 6 pictures at 2048 px: 3×2 gives six `1024×682` cells at x 0/1024 and y
0/682/1364; 2×3 gives six `682×1024` cells. `radius_pct=100` leaves the cell corner
`(0,0,0)` background while every cell centre keeps its source colour.

## 76. One layout function feeds the composer, the preview and the hit areas

**Trap.** A preview that re-derives the grid in JavaScript drifts from the composer the
first time one side rounds a cell or treats a span differently — the drag then moves a
different picture from the one that gets cropped.

**Rule.** Return the layout (`width/height/cols/rows/cells`) from the backend and draw
the preview's hit regions from those rectangles. Key per-image `{index, zoom, x, y}` by
the **source** index, not the ordered position, so reordering does not move a transform
to another file. Apply pan as a crop window inside the cover-resized picture; scale the
pointer delta by the rendered cell. An `auto` canvas ratio is the **auto-oriented**
source ratio (a rotated phone photo must not get a landscape canvas around a portrait
picture).

**Evidence.** `collage_cell_pan_and_zoom_move_the_picture`: one `1024×1024` cell, zoom 2,
pixel `(256,512)` is red at pan 0 and blue at x=100. `tool-image-collage` still matches
all four quadrants and the default 2048×2048.

## 77. A preview status is an overlay, and that is measurable

**Trap.** "Rendering…" was a `<p>` under the picture. When it appeared, the preview band
grew by a line and pushed every slider down; when it finished, they jumped back. The
user read it as the slider moving under the pointer.

**Rule.** Transient status lives in a positioned badge over the preview (or in a
permanently reserved row). Keep the facts row's height reserved so numbers arriving
later cannot resize the band either. Measure the fields' top edge and the band height
while requests are in flight; a change over 2 px is a failure.

**Evidence.** `ui-shots.py` samples 25–31 times through a render for Edit photo,
Add background and Compress: `fields_top` and `preview_height` are constant
(`502/369`, `404/271`, `546/364` in the three dialogs), while Compress still reports
`Encoding the preview…`.

## 78. A CSS blur is a rendering cost and a hit-testing layer

**Trap.** Image mode drew the background with `background-size: … auto` and a
`background-position` percentage, which letterboxes when the picture's aspect differs
from the canvas, then added a full-size `filter: blur(Npx)` layer on every 0↔positive
change. The geometry disagreed with the engine, and the blur layer sat over the canvas
where the pointer was dragging.

**Rule.** Draw the background as an `<img>` sized and offset by the engine's cover/zoom/
crop formulas, scale the blur radius by the display/source ratio, cap it, and make every
child `pointer-events: none`. One canvas owns the drag with pointer capture; no layer is
created or removed as a slider moves. Use a plain thumbnail, never the full picture, for
a live preview.

**Evidence.** `background_blur_changes_only_the_background_layer`: checkerboard backdrop
changes by up to 128 channel levels inside the background-only region at `bg_blur=35`,
while the photo centre stays `255/0/0`. `tool-image-background` runs image mode + blur in
the real window and the photo centre is byte-identical `(237,140,76)`.

## 79. A strength knob must reach both the job and its preview

**Trap.** Redaction's blur radius was a constant `min(bw,bh)*0.15`. Adding a slider only
to the dialog would have produced a preview that never matched the file; adding it only
to the engine would have left a dead control.

**Rule.** Parse the treatment once (`style`, `colour`, `strength`, boxes) into settings
shared by the job and the preview, and render the preview through the same function at
its own pixel size. The image sigma is
`max(2, min(bw,bh) * (0.06 + 0.30 * strength/100))`; video's `boxblur` radius is the same
expression with `0.05` and a `2..64` clamp. The preview is an engine artifact cached by
the full request; the dialog toggles between it and the plain thumbnail. Normalised boxes
need no rescaling when the surface is a thumbnail.

**Evidence.** `redaction_blur_strength_changes_the_result` measures interior variance
`0.304 → 0.087 (strength 10) → 0.019 (90)`; `redaction_preview_uses_the_same_treatment`
fills the same box at 200 px. `tool-image-redact` sees the strength slider and the
"Preview redaction" toggle in the real window and still writes the solid/blur artifacts.

## 80. "Visible" means inside the sheet, and a toggle is not a button

**Trap.** `ensure_visible` compared a control's extents with the whole **window**. A
field below the dialog's scroller is clipped by the sheet but still inside the window,
so the harness clicked its off-screen coordinates — on the backdrop — and closed the
dialog. Separately, the annotation toolbar's `aria-pressed` buttons are `toggle button`
in AT-SPI; `press_button` looked only for `button` and skipped every tool.

**Rule.** Scroll a control into the **sheet's** viewport before clicking it, re-locate
it afterwards (the wheel rebuilds the tree), and guard every extent lookup: a sheet that
has not laid out yet returns no box, and unpacking it crashed the persistence scenario
after the rest of the run was green. Give role `toggle button` the same weight as
`button` in a toolbar. A skip must name the role it looked for.

**Evidence.** `tool-image-background` image mode initially failed at `Background blur`
because the click hit the backdrop; with sheet scrolling and a re-located node the
scenario passes. `tool-image-annotate` initially skipped with "no Rectangle tool"; after
the role fix it draws and saves a rectangle and a number. The first full Round 4 run
failed `persistence` with `TypeError: 'NoneType' object is not iterable` at the sheet
extent; the guarded rerun is 65/0/6.

## 81. A stand-in preview must be the source, and the facts must wait

**Trap.** Compress showed a black frame with "Encoding the preview…" for the real encode
(2.5 s on a poster). The artifact is the only honest source of dimensions and bytes, but
the wait does not have to be black.

**Rule.** Show the source thumbnail immediately (the same picture the artifact derives
from), overlay the encoding status, and replace it with the artifact when it lands. Only
the artifact may supply size/dimensions/notes. Reserve the facts row so the dialog does
not grow when they arrive.

**Evidence.** `ui-shots.py` samples the Compress dialog through an encode: the status is
seen and `fields_top` stays 546, `preview_height` 364; the frame always shows a loaded
picture.

## 82. A composite inherits the colour space of its canvas

**Trap.** `add_background` built the canvas from the chosen background picture and then
composited the photo on it. A grayscale background picture put the whole operation in
Gray; the colour photo came out white (`255/255/255` where red was expected), and the
unit test that sampled the photo centre caught it.

**Rule.** A canvas is a colour surface regardless of the picture used as its fill.
Normalise it to sRGB after the fill (and before the photo/shadow composites), and assert
the photo's own colour in the composite — not just that a file was written.

**Evidence.** `background_blur_changes_only_the_background_layer` uses a black-and-white
checkerboard backdrop and requires the photo centre to stay `255/0/0`; the real-window
`tool-image-background` run samples `(237,140,76)` in the file.

## 83. A Solid `<Show>` is a mount switch, not a reactive binding

**Trap.** The annotation draft was `<Show when={draft()}>{(shape) =>
drawShape(shape())}</Show>`. A non-keyed `Show` memoises on *truthiness*
(`equals: (a, b) => !a === !b`), so when the draft went from one object to the next the
child was never re-invoked. `drawShape` ran once at pointerdown, when the rectangle was
0×0, and the rectangle appeared only when pointerup committed it — reported as "the
preview only shows the result after I finish".

**Rule.** A value that changes shape-by-shape while a gesture is active must be `keyed`
(`<Show when={draft()} keyed>`), or the reactive read must happen *inside* a JSX
expression. A local `const box = hitBox(shape())` outside the JSX is computed once; the
same bug left the selection ring behind while a shape was dragged.

**Evidence.** `ui-shots.py` now samples the draft mid-drag: the rectangle has a non-zero
`getBoundingClientRect()` while the pointer is down, and the selection ring follows a
40 px move in Select.

## 84. Crop before the blur; `-region` is not a cheap mask

**Trap.** A blur redaction emitted `-region WxH+X+Y -blur 0xS +region` per box. On a
48 MP JPEG that cost 207 s through the app (181.9 s in `magick` alone) with the app's
`MAGICK_THREAD_LIMIT=2`; the user reported redaction as unusably slow in both styles.

**Rule.** For a still, build each blurred box as an expanded crop (`+clone -crop …`),
blur the crop, crop back to the box, and composite it at the box origin. Blur cost is
then proportional to the box, not the canvas. Keep the sigma formula shared with the
preview.

**Evidence.** Same 48 MP file, same boxes: **13.3 s** (15.6× faster) end-to-end; a
5000×3750 PNG went 48 s → 5.7 s. The solid style is unchanged at ~1.8–2.6 s.

## 85. A cover crop has an origin

**Trap.** The background image was cropped with `baseW = bg_h × target`, `baseH = bg_h`
… and then the pan offset was applied to the **source** as if the cover window started
at (0,0). Whenever the picture's aspect differed from the canvas, the file showed a
different region from the dialog: the preview's card edges and title were replaced by a
plain area, and zoom/placement looked ignored.

**Rule.** Compute the cover window's origin in the source (`(sourceW - baseW)/2` or
`(sourceH - baseH)/2`) and add it to the crop offset. The UI draws the *whole* source at
`canvasWidth / visW` and subtracts the same origin; forcing the source into the base
window's box stretches a non-square picture.

**Evidence.** `add_background_image_crop_starts_at_the_cover_window` (red/blue 200×100
backdrop on a 1:1 canvas: corner is blue at the base origin, red at the source origin);
`geometry.test.ts` pins the same offsets; a real job with zoom 2, x −6, y −67 now shows
the same POSTER card the dialog shows.

## 86. A control that cannot change the result is a bug, not a hint

**Trap.** Collage exposed Style = Grid/Row/Column/Featured *and* Rows/Columns. Row and
Column ignored both counts by construction, so the dialog offered two controls that
provably did nothing in the selected mode ("Rows and Columns here do not make much
sense if we can't really change the collage").

**Rule.** Keep one representation of the layout. Row and Column are the grid with a
fixed count, so they were removed from the dialog (the engine still accepts them for
old requests); Grid/Featured always obey Rows/Columns, and 0 = auto. Spare cells stay
background, including when columns exceed the picture count.

**Evidence.** `ui-shots.py`: 2 pictures, columns 2 → cells 0.5 wide; columns 3 → 0.333;
rows 2 → height 0.5; readout `3 × 2 cells`.

## 87. A debounced engine preview needs a live layer

**Trap.** Every pan move restarted a 200 ms debounce, and the whole collage was
recomposed by ImageMagick, so no render happened until the pointer stopped — the user
saw "it only updates after I finish", then a long wait.

**Rule.** One request in flight, latest-wins (a change during a render queues exactly
one follow-up). While the engine catches up, draw the selected cell from its thumbnail
with the *same* crop formula as the composer (`collageCellBox`), and drop the live layer
only when a render containing the current transforms lands. The live layer is a bridge,
never the saved result.

**Evidence.** `ui-shots.py` samples inside the 120 ms window: `.collage-live` is present
with a non-empty width while the pointer is down, and disappears after the render.

## 88. A blurred layer must not reveal a sharp copy underneath

**Trap.** CSS `filter: blur()` fades to transparent at the element's own edge. An
unblurred "bleed" copy underneath filled that edge — and along the canvas border it
showed the sharp picture, so a blurred document background still had readable text on
the left ("I can still see the left part of the background").

**Rule.** The underlay must be blurred too, and larger than the canvas so its own fade
lands outside the visible area. Never place an unfiltered copy under a filtered one.

**Evidence.** Real-window preview at blur 50 on `screenshot-text.png`: the left-edge
text strip is gone; the saved file is unchanged (the engine blurs before resizing).

## 89. History dots read newest-first

**Trap.** The activity spark took `historyRows().slice(0, 12).reverse()`. History is
stored newest-first, so the reverse put the oldest job on the left and the newest on the
right; the user expects the dot next to "Activity" to be the latest.

**Rule.** Do not reverse a list that is already in the order being shown. If the UI
wants a different order, sort at the source and name it.

**Evidence.** `ui/src/app.tsx` renders `slice(0, 12)`; the mock's first row is the
newest.

## 90. An arrow head is a function of the stroke, and its tip needs a cap

**Trap.** The arrow drew three butt-capped lines meeting at the tip, with a head length
fixed at `clamp(0.03 × min(w,h), 8, 48)`. At stroke 13 and above the flat ends met at
three different angles and left wedge-shaped gaps: the tip read as broken. At stroke 64
the head was shorter than the stroke itself.

**Rule.** Scale the head with the stroke (`3.5 × stroke`, bounded by a quarter of the
short side and 80 % of the arrow's length) and give all three lines
`stroke-linecap: round` / `stroke-linejoin: round` (MVG accepts both). The SVG overlay
and `draw_command` use the same formula, so the dialog and the file agree.

**Evidence.** `annotate_arrow_uses_round_caps_and_a_large_text_is_allowed` asserts the
command carries both cap/join settings and a 140 px head for a 40 px stroke;
`ui-shots.py` checks the three SVG lines at stroke 13; stroke 64 renders a solid head.

## 91. A text size is free, not a slider's range

**Trap.** Caption sizes were capped at 200 in the engine and in the editor, and the
slider's value was a read-only output. A title on a 3000 px poster could not exceed the
old cap, and a number could only be dragged, not typed.

**Rule.** Let the engine accept a caption up to 2000 px and put a number input beside
the slider. Strokes stay 1..64, numbers keep their box-derived badge, and the caption's
resize handles scale the size (not the box) up to the same ceiling.

**Evidence.** `annotate_accepts_a_text_above_the_old_cap` (600 px caption changes the
picture); `ui-shots.py` types 300 and sees a `font-size=300` node and `max=2000`.

## 92. An inline editor replaces the mark, it does not sit over it

**Trap.** Double-clicking a text annotation opened a `<textarea>` at the text's box while
the SVG text stayed rendered underneath. The result looked like two overlapping texts,
plus the selection ring around the box; the report called it a double-click bug.

**Rule.** While the inline editor is open, omit the shape it edits from the SVG, hide the
selection ring, and disable text selection on the canvas. The textarea is the only copy
until Enter or blur commits it.

**Evidence.** `ui-shots.py`: double-click opens the editor with no `text` node for the
edited string and no `.annotate-selection`; Escape restores the mark.

## 93. A video has no still for ImageMagick

**Trap.** The selection strip built each card with the image thumbnailer. For a video
that means ImageMagick walking the file: 27–28 s per clip, then "storing a thumbnail:
No such file or directory". Three selected videos left the app on "Reading files" for
over a minute with no cards — reported as "the selection does not show them".

**Rule.** One `still_for` decides a card's still: a PDF goes to its page renderer, a
video to `filmstrip(source, 1)` (ffmpeg seeks, 32 ms), everything else to the image
thumbnailer. The dialog preview command and the selection analysis use that same
function, so a new category cannot get the wrong decoder in one place only.

**Evidence.** `multi-file-videos`: three clips (AVI/MP4/MKV) reach the strip in 2.2 s,
all three names are there, and the preview cache holds three `strip-*/frame001.jpg`
stills.

## 94. Sliders that show the same quantity share one axis

**Trap.** Trim start was `min=0, max=end`, trim end `min=start, max=duration`, and the
playhead `0..duration`. At start 5.04 / end 9.16 the end handle sat at 82 % and the
playhead at 91 %, so the same time read differently on two rows — "the playhead is not
aligned with the trim end".

**Rule.** All time sliders span the clip (`0..duration`); the handlers clamp the values
so handles cannot cross. A slider whose range follows another control is a second,
invisible scale.

**Evidence.** `ui-shots`: start/end/playhead all `min=0, max=10`; end and playhead at
9.16 both report fraction 0.916. `tool-video-trim` reads the same min/max from AT-SPI.

## 95. "All frames" is the engine's fps mode, with a stated ceiling

**Trap.** The frames dialog only offered per-moment chips; the engine already had an
`fps`/`interval` snapshot mode but the dialog never sent it.

**Rule.** Send `fps = the clip's own average frame rate` (ffprobe's
`avg_frame_rate`) from an **All frames (N)** button, show the count on the primary
action, and disable the button (with a reason) above the engine's ceiling. The engine
writes one ffmpeg pass and errors clearly when `duration × rate` exceeds the cap; the
UI constant and the engine constant are the same number.

**Evidence.** `snapshot_all_frames_uses_the_native_frame_rate` (2 s at 30 fps → 60
PNGs); `probe_reads_a_fractional_frame_rate`; `ui-shots` shows "All frames (300)" and
"Save 300 Frames" for the 10 s mock clip, with the chips hidden and restorable.

## 96. Several outputs from one source are one result

**Trap.** Split parts, saved frames and exported PDF pages were published as a pile of
files beside the original (`clip-part-001.mp4`, `clip-part-002.mp4`, `frame-001.png`,
…), mixing runs and unrelated results.

**Rule.** When a job returns more than one file (a directory artifact still counts as
one), publish them into a folder named after the source stem, strip the repeated
`stem-` prefix from each name, and apply the overwrite policy per file. The primary
history output stays the first file. The harness must recurse into that folder; a
folder named `<stem>-out`/`<stem>-extract` is still a single artifact.

**Evidence.** Queue test `several_outputs_publish_into_a_folder_named_after_the_source`;
app-e2e `tool-video-frames` → `clip-landscape-720p-10s/frame-001.png`,
`tool-video-split` → `…/part-001.mp4`, `tool-pdf-pages` and the PDF split case →
`report-5-pages/…`; `quality-audit video-frames` passes.

## 97. A duration cap must be a bounded overview, not a silent stretch

**Trap.** The waveform decoded the first 600 s and *always* said `truncated` when read
back from the cache, so a 14-second recording claimed "first 10 minutes shown", and a
two-hour recording drew its first ten minutes stretched across the whole axis. The
dialog was lying about both the file and what it displayed.

**Rule.** Decode at a rate that makes hours cheap (1 kHz), cover up to six hours, and
store a real truncation flag in the cache. The flag is set only when the decode reached
the cap, so a short file never claims to be partial. The bucket count is bounded
(2000) independently of duration: the drawing needs an overview, not one bucket per
2.5 ms.

**Evidence.** 14.1 s speech: 2000 peaks, `truncated=false`; an 11-minute WAV: 2000
peaks, `duration_ms=660000`, decoded in 138 ms; the cache round-trip test asserts both
flag values.

## 98. Audio playback is one sink with a lifecycle, not a fire-and-forget call

**Trap.** `play_preview` spawned a player and **awaited it to completion**. There was no
stop command, so Pause was impossible, a second preview played over the first, and
closing the dialog left the sound running.

**Rule.** One shared external sink in app state: starting a play stops the previous
player and returns immediately, the dialog advances its own clock, and `stop_preview`
kills the child. The dialog's cleanup and the main window's close hook both stop it.
Seeking uses `ffplay -ss` / `mpv --start`; the simple sinks refuse a mid-file start
rather than silently playing from zero. Do not fall back to a desktop opener: it cannot
be stopped.

**Evidence.** Unit tests pin the player argv and idempotent stop; `audio-playback`
launches the real window with `SDL_AUDIODRIVER=dummy`, sees an `ffplay` child while
playing, closes the sheet, and sees no player child afterwards.

## 99. A transport must play the selection it is drawn over

**Trap.** The trim waveform's Play started at 0 and ran to the end of the file, ignoring
the trim handles; the playhead was not even clickable. The report: "it doesn't stop at
where I trimmed it nor start from where I trimmed it."

**Rule.** With a selection, Play starts at its start (or the cursor inside it) and the
clock stops at its end, calling `stop_preview`. Clicking the waveform seeks and stops.
The button is Play/Pause, not a disabled "Playing…" label.

**Evidence.** `ui-shots`: trim 2–4 s plays at `0:02.35` and stops at `0:04.00`, button
`Pause` → `▶`.

## 100. A control inside an editable surface needs the first pointer event

**Trap.** The × inside a bleep range sat on the waveform surface. The surface captured
the pointer on `pointerdown`, so the button's click walked to the surface and the range
never disappeared.

**Rule.** In the surface's `pointerdown`, ignore events whose target is a button; give
the button its own stacking order and `pointer-events: auto`. The same rule applies to
any in-canvas control (page cards, crop handles with their own handlers).

**Evidence.** `ui-shots`: draw a bleep range, press its ×, the range count goes 1 → 0.

## 101. A fixed-aspect preview is centred by its box

**Trap.** The portrait visualizer frame had `width: 100%` and `max-width: 200px`, so it
sat at the left of a 552 px row.

**Rule.** `margin-inline: auto` on the constrained frame; the measuring check compares
the left and right gaps, not "it looks centred".

**Evidence.** `ui-shots`: portrait frame 200 px wide, 178 px gap on each side.

## 102. A seek must move the clock that plays, not a second marker

**Trap.** Clicking the normalize waveform moved the waveform's own playhead, while the
"Output" preview advanced a different clock that always started at 0. The marker showed
the click and the sound did not follow it — "the playback does not align with it".

**Rule.** A waveform that is drawn over a dialog-owned preview takes the dialog's
playhead (`playhead` prop) and reports a click through `onSeek`. The dialog's clock is
the only one: a click stops the sink, sets the position, and Play starts the sink there.
Bleep drags still create ranges; a click without a drag seeks.

**Evidence.** `ui-shots`: normalize click at 50 % → playhead 0.500; Play starts there and
reaches 0.564 after 0.9 s; Output shows the pause glyph.

## 103. Mono is one control, not a left channel with a silent right

**Trap.** Channels always drew L and R and always labelled the gain "Left volume"; a
mono recording showed an empty R lane and "Left volume" 113 %, and the mono mix used
`left_volume` for one channel only.

**Rule.** The media facts carry the source channel count. A mono source renders one lane
labelled **M**; a mono output exposes a single **Volume** field (`volume`), applied to
both source channels before the mono mix. Stereo keeps Left/Right. The engine accepts
`volume` alongside the old keys.

**Evidence.** `channels_mono_volume_is_one_control_for_the_mix` (50 % is ~6 dB down);
`ui-shots`: Mono → labels `[Volume]`, lanes `[M]`; Stereo → Left/Right, L/R.

## 104. A transport is an icon and its subject

**Trap.** The audio buttons said "Pause" or "Original" with no play/pause glyph, so a
version row read like navigation, and the word changed width every state.

**Rule.** Transport buttons keep the subject ("Original", "With Bleeps", "Output",
"Audio") and prefix a play or pause glyph. The accessible name still says Play/Pause;
the visible label stays constant. The same glyph pair is used by the waveform transport.

**Evidence.** `ui-shots` reads polygons (play) and rects (pause) on the buttons; only the
active version carries the pause glyph.
