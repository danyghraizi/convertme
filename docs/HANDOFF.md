# Handoff

State of the project at the end of the first background-conversion session. Read this
first, then `AGENTS.md` (rules) and `docs/LESSONS.md` (why the rules exist).

## Where things stand

`add background` is feature-complete and verified. Nothing else has been started.

Working tools (UI and engine): Compress, Crop, Add background, Edit photo, Metadata,
Redact, plus conversions for images. The wheel, the drag layer, the queue, history and
settings exist as described in the plan; the GUI is usable end to end on this machine.

## What "add background" does now

| Control | Behaviour |
|---|---|
| Fill presets | 4 solids, a rule, 4 gradients, then an image button that switches the mode to “Another image” |
| Fill | colour (picker + presets + hex), gradient (linear/radial, angle, 2–8 stops with positions), another image with Choose… |
| Padding / corners | percentages of the shorter side: 0–100 % and 0–100 % (100 % = fully rounded) |
| Shadow | 0–100 %, angle 0–360 (0 = above, 180 = below), density and spread both follow the value |
| Background image | zoom (up to 3×), across / up-down, or drag directly on the canvas |
| Canvas ratio | Auto, 1:1, 16:9, 4:3, 9:16, 3:4, 4:5 |
| Preview | the real canvas size in pixels, the photo centred with its padding, the chosen image and shadow |
| Files | arrow picker for a multi-file selection; each file keeps its own settings unless “Same settings for every file” is ticked; Run sends one job per file |

Engine notes worth knowing:

- The background picture is cropped at source resolution and then scaled to the canvas.
- The shadow is built from a rounded-rect mask at ≤ 600 px, blurred, turned into alpha and
  scaled back; above **60 megapixels** it is skipped so the job still succeeds.
- ImageMagick works at depth 8 for the composition, with 1 GiB of RAM, 2 GiB of mapping
  and 16 GiB of disk spill (`crates/convertme-engines/src/proc.rs`).

## Known limits / next performance work (deliberately not done)

1. **Shadow above 60 Mpx is dropped, not tiled.** Tiling the shadow (or compositing it per
   tile) is the real fix; it needs a streaming composition path.
2. **Very large canvases rely on disk spill.** A 195 Mpx input composes in ~36 s under the
   modest budget, but only when the temp filesystem has room. A free-space pre-check for
   the *temp* dir (not just the output) is missing.
3. **Shadow cost is still the dominant term** for big canvases (several seconds).
4. The installed size and cold-start budgets from the plan have not been re-measured since
   the UI work.

## Verification status

- `./scripts/preflight.sh` passes: identity gate, fmt, clippy `-D warnings`, all workspace
  tests (67 engine tests, 13 GUI tests), node tests for the field reader and the parameter
  collector, UI typecheck/build, clean-checkout builds.
- Composition tests pin: proportional padding/corners, rounding belongs to the photo,
  background placement stays inside the frame at -100/0/100, shadow darkens the band.
- Contract tests pin: the field payload names the kinds the UI renders; the binary embeds
  the built bundle; the reader keeps every kind and property; controls and hidden fields
  never become backend parameters.

## Running it

```bash
npm --prefix ui run build      # required before cargo build: the bundle is embedded
cargo build --release -p convertme-gui
./target/release/convertme
```

Changing `ui/src` without rebuilding the bundle now fails the build script (and a test
compares the embedded page with `ui-dist`).

## Backlog

1. **Compress** — the next tool to bring to the same standard (target size or percentage,
   preview before commit).
2. Crop, Edit photo, Annotate, Redact, Metadata — the rest of the image tool row.
3. Video, audio, PDF, documents, slides, spreadsheets, EPUB, subtitles, archives.
4. Performance pass per the plan's Step 0 budgets, including tiled composition.
