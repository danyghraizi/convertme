# ConvertMe

An offline, drag-and-drop file converter with modifier-key wheels, for Windows and
Linux.

Drag a file, hold a key, and a ring of choices appears under the pointer: conversion
targets (Shift), file tools (Ctrl), or file actions (Alt). Nothing is uploaded, and a
source file is never modified — every job writes a new file next to it.

## What is where

| Path | What it holds |
|---|---|
| `crates/convertme-core` | Identity strings, the format registry, the planner, the job queue, naming and publish rules, subtitles |
| `crates/convertme-engines` | One module per tool family: ImageMagick, FFmpeg, Ghostscript/qpdf/Poppler, Pandoc/LibreOffice, 7-Zip, metadata, redaction, previews |
| `crates/convertme-drag` | The only platform-specific code: Windows hooks, X11 keymap polling, the Wayland global-shortcuts portal |
| `crates/convertme-gui` | The Tauri shell, the wheel window, the actions, and the CLI |
| `ui/` | Vite + SolidJS front end, including the wheel geometry |
| `sidecars/pdf` | `convertme-pdf`, the Python sidecar for PDF → DOCX/PPTX/XLSX/EPUB |
| `scripts/` | The identity gate, the Tauri config generator, the icon generator |
| `xtask` | `cargo xtask fixtures|vendor|snapshot|perf|release|gen-conf|debrand` |

## Building

```sh
cargo xtask gen-conf            # crates/convertme-gui/tauri.conf.json
cargo xtask fixtures            # one tiny file per input format
npm --prefix ui ci && npm --prefix ui run build
cargo xtask vendor              # copy the bundled tools into vendor/bin
./sidecars/pdf/build.sh         # the Python sidecar
cargo build --release -p convertme-gui --bin convertme
```

`cargo build` embeds whatever `crates/convertme-gui/ui-dist` holds; it does not run
the front-end bundler. The build script refuses to compile when `ui/src` is newer
than `ui-dist`, so a stale bundle cannot reach a binary. `tauri build` (and
`scripts/bundle.sh`) run the bundler and the identity gate themselves.

The bundler runs the identity gate, which needs the token list:

```sh
export DEBRAND_TOKENS_FILE=~/.config/convertme/forbidden.txt   # one lowercase token per line
```

`./scripts/bundle.sh deb,appimage` produces `app.convertme_<version>_amd64.deb` and
`app.convertme_<version>_amd64.AppImage` (`.rpm` where `rpmbuild` exists); the NSIS
installer and portable zip come from the same script on Windows. The package and the
desktop entry carry the reverse-DNS id — `app.convertme.desktop`, `Name=ConvertMe`,
`Exec=convertme` — while the binary and the CLI are `convertme`.

## Running

```sh
convertme                                   # the window
convertme convert photo.jpg --to webp       # the CLI, same pipeline
convertme tool clip.mp4 --tool trim --params '{"start":1.5,"end":9}'
convertme probe                             # what is installed
convertme history --limit 20
```

Flags are parsed strictly: an unknown or misspelled flag is an error with the usage,
never a silently skipped word. `convertme <command> --help` prints the grammar.

## Living in the background

The wheel needs a process to hold the hook (Windows/X11) or the portal shortcut
(Wayland), so an invisible instance runs from login:

```sh
convertme --background     # no window; the wheel works from any app
convertme                  # a second launch shows and focuses the window
```

Settings → **Start with the system** writes the login entry for you
(`~/.config/autostart/app.convertme.desktop` on Linux,
`HKCU\…\CurrentVersion\Run` on Windows). Closing the window in background mode hides
it; the instance keeps serving the wheel.

## Trying the wheel without dragging

Settings is its own page (the button at the bottom of the sidebar), not a drawer;
Activity stays in the strip above the status bar.

Open the window, choose files (or drop them), then press **Preview wheel** in the
status bar at the bottom: the wheel opens at the pointer, already loaded with that
selection, exactly as it does during a drag. `←/→` or `↑/↓` select, `Enter` runs,
`Esc` closes, and `Shift`/`Ctrl`/`Alt` switch between the convert, tools and actions
rings.

`CONVERTME_SHOW_WHEEL=1 convertme <paths>` does the same from a terminal and exists
for CI and screenshots; it is not needed to use the app.

## Invariants

* **Sources are read-only.** Engines that mutate by nature (`metadata`, `redact`,
  `crop`, `trim`, `bleep`, `organize`) run on a staged copy in the job's work dir;
  the queue publishes the artifact, never the engine.
* **Publish is atomic.** Everything is produced in `<tmp>/convertme-<pid>-<seq>`
  and renamed into place, so a cancel, a crash or a full disk leaves the directory
  untouched. An existing file is never replaced (`(2)` suffix), and the source path
  is never a candidate.
* **Local only.** No telemetry, no metering, no network call except the optional
  update check.
* **No shell strings.** Every tool invocation is an argv array, so filenames with
  spaces, quotes, newlines and emoji work.

## Licensing

GPL-3.0-or-later. The bundled tools and their licences are listed in `NOTICE`;
FFmpeg's corresponding source and the sidecar's source are published per the offers
described there.
