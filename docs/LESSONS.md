# Lessons from building ConvertMe

Written after a day of interface work on `add background`, where most of the time went
into failures that looked like something else.

The transferable half of this lives in `~/.omp/agent/AGENTS.md`, which every session loads
regardless of repository. This file keeps the project-specific detail: names, files,
commands, and the exact measurements that proved each fix. Each item states the trap, the evidence
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
