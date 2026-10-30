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
