# Release readiness handoff

This document is the portable continuation point for the Linux and Windows release
work. Keep it current when release inputs, platform evidence, or blockers change. The
product and engineering invariants remain in `AGENTS.md`, `docs/HANDOFF.md`, and
`docs/LESSONS.md`.

## Current state

- Source checkpoint: `work/commercial-windows-image-2026-11-04` is committed locally.
  The push attempt failed because the configured `127.0.0.1` proxy could not reach
  GitHub. Push this branch from a network-enabled checkout before expecting the
  commercial Windows GitHub Actions workflow to run.

- Product direction from the maintainer (2026-11-04): build a sellable Windows product
  without paid third-party software licenses. The existing full-featured repository is
  GPL-3.0-or-later; its PDF sidecar uses AGPL PyMuPDF and its helper list includes
  GPL/AGPL tools. The GPL edition remains intact. A separate closed-source x64 app now
  exists in `commercial-windows/`, independently implemented without the GPL engine
  crates. It converts PNG, JPEG, WebP, BMP, GIF, and TIFF. Its locked Rust closure is
  limited to permissive licenses plus MPL-2.0 components whose exact sources and
  notices accompany the candidate. This is a narrow image release, not a proprietary
  build of the full ConvertMe catalog. Contributor rights and the legal copyright
  holder still need confirmation; see `docs/COMMERCIAL_PRODUCT_PLAN.md`.

- Product version: `0.1.1`; release builds are unsigned.
- Supported release targets: Linux x86_64 DEB/RPM and Windows x64. Windows ARM64
  workflow jobs are gated on `CONVERTME_ARM64_RELEASE_ACCEPTED=true` until x64 release
  acceptance is complete and ARM64 has native runtime evidence.
- The existing GPL product still lacks complete Windows helper/compliance inputs and
  clean-machine release acceptance. The separate commercial image pipeline can create
  an unsigned x64 portable candidate and matching third-party source archive; this is
  not evidence of public release readiness.
- Local commercial x64 candidate packages were built and their internal SHA-256
  manifests verified (9 portable-package files and 75 third-party-source files). The
  package contains only the proprietary image app, notices, installer script, and
  Microsoft-signed WebView2 runtime; the matching source archive includes the exact
  MPL-2.0 crate sources but no first-party app source. Candidate hashes are
  `e931294d8f62c1c3cce6753e86959c4327e5b3dfcd9a4598081038f1fb4e41e7` (portable ZIP)
  and `fda769b027a94daddd3930065827d08d6ae9f4013752ecf019569ab0370c1f17` (third-party
  source ZIP). These files are ignored local artifacts, not retained CI assets.
- The commercial app's four unit tests and release build pass. A local startup smoke
  launch exits at Tauri window setup with Windows `Access is denied` (exit 101), so
  actual launch and conversion in a normal desktop session remain unverified. Do not
  treat this sandbox launch as a product defect or as clean-machine acceptance until
  reproduced outside this restricted process environment.
- The ignored local artifact directory has a quarantined superseded ZIP and staging
  directories from failed packaging runs. Do not share the directory wholesale; the
  commercial CI workflow filters uploads to the current candidate and third-party
  source filenames.
- `cargo fmt --all --check` passes. `cargo test --workspace --locked --offline` is
  blocked at Windows linking because the checkout has no offline ONNX Runtime import
  library and `ORT_LIB_PATH` is unset (`OrtGetApiBase` unresolved). The commercial app's
  own four tests pass. `scripts/preflight.sh` could not start under Git Bash in the
  restricted process sandbox (signal pipe creation denied, Win32 error 5); its
  debrand-token input is also absent. Do not label the full preflight green.
- Linux package replay is fail-closed and requires the complete locked offline input
  set described below. AppImage remains blocked; do not advertise or require it.
- The full Windows packager remains fail-closed and requires native Windows, a
  verified offline WebView2 installer, a validated helper payload, and compliance/source
  materials. The narrow commercial image packager instead validates and bundles its
  pinned Microsoft WebView2 runtime and MPL source closure.
- Do not publish a release or claim full desktop acceptance until the completion
  criteria below are met.

### Windows continuation on this machine (2026-11-04)

- Work is on local branch `work/windows-x64-release` from `6fda71c`. The x64 and
  ARM64 Microsoft WebView2 offline installers were captured from Microsoft's
  endpoint and reverified by Authenticode at version `1.3.307.15`; their hashes and
  signers are recorded in `packaging/windows/webview2.lock.json`. The installers are
  local under the ignored `vendor/windows/runtime/` directory. Capturing the ARM64
  record exposed that `prepare-windows-runtime.ps1` used `File.Replace` with a null
  backup path, which fails on this PowerShell/.NET runtime. The script now replaces
  existing files via same-volume backup renames and restores the old file if promotion
  fails; ARM64 capture and both lock verifications pass.
- The continuation snapshot was downloaded and its outer SHA-256 verified. Its
  embedded manifest verified 32 files, and the extracted Windows acceptance fixture
  manifest matches `8bc6a0603441fcbcaef23cada0eafb4d15ed84f9f0ce840ba73b2eec23eea259`.
  The snapshot explicitly has no Windows helper payload, source/compliance materials,
  candidate package, or Windows acceptance evidence.
- Rust 1.99.1, Node 22.23.4, CPython 3.12.10, and Visual Studio 2022 C++ Build Tools
  are installed on this machine. UI build and typecheck passed; UI tests passed (31),
  and the Windows `convertme-core` unit tests passed (64). Windows core Clippy also
  passes with `-D warnings`. The two Windows failures
  reported by CI were fixed in this branch: the naming test now compares paths rather
  than slash-delimited strings, and the directory rename maps `HRESULT_FROM_WIN32`
  errors back to `io::ErrorKind::AlreadyExists`.
- GitHub CI run `41805892091` on `6fda71c` confirms those failures: the Windows x64
  job had 62 passing core tests and exactly the two failures fixed above. The UI job
  passed; the Linux Rust job stopped at `cargo fmt --all --check` due to formatting in
  `convertme-engines/src/probe.rs`, `convertme-engines/src/proc.rs`, and
  `xtask/src/main.rs`. Those files have now been normalized with `cargo fmt --all`,
  and the full workspace formatting check passes locally.
- Retained Actions artifacts contain no Windows helper payload. The inspected
  `bundles` artifact from run `40881467590` contains only a Linux AppImage and DEB;
  no Windows-named payload or candidate is retained remotely. A local unsigned
  commercial image candidate is now available under ignored `artifacts/` output.
- No full-featured Windows candidate has been built. The source-closed FFmpeg build inputs, prepared
  Poppler prefix, helper payload, and corresponding source/compliance inputs remain
  missing. The gap is also present in tracked source: `packaging/windows/tools.lock.json`
  points at `packaging/windows/ffmpeg.lock.json` and
  `packaging/windows/poppler-explicit.lock`, but neither file exists in this checkout
  or its Git history. `packaging/sources.lock.json` only has a Ghostscript component;
  it does not yet record the Windows FFmpeg, Poppler, Pandoc, PyMuPDF sidecar, or
  `convertme-pdf` source/license/recipe inputs required by the compliance generator.
- The exact pinned Ghostscript archive is now captured under ignored
  `vendor/source-inputs/ghostscript/`; `packaging/sources.lock.json` binds the archive,
  its extracted top-level license, and the Windows packaging recipe files. All four
  corresponding size/hash checks pass locally. Other required components and the
  payload-wide ownership/PyInstaller analyses are still absent.
- `release-materials.py` also used the native Windows host string `AMD64`, which did
  not match the existing `windows-x86_64` cargo-about pin. It now canonicalizes
  `AMD64`/`x86_64` and ARM64 aliases without changing pins; the focused host mapping
  tests pass.
- The pinned cargo-about 0.9.3 Windows x64 and ARM64 archives were downloaded to
  ignored `vendor/source-inputs/` and both archive and executable hashes match
  `packaging/sources.lock.json`. They are retained but not run under the host policy.
- The Windows sidecar wheelhouse now contains 26 wheels (108,968,537 bytes), selected
  by `pip download --require-hashes` from `requirements-windows.txt`. Its local
  manifest binds every wheel hash and the requirements-file hash
  `49714cb4efcad633986bf13f85a54dfb8fa38d71c5991507eb156a8b200ab233`.
- Under ignored `vendor/source-inputs/python-sidecar/`, all 26 wheels were matched to
  official PyPI metadata and 22 source archives (186,726,523 bytes) were checked
  against PyPI SHA-256 and size. The capture retains license and recipe evidence and
  verified 1,856 records. Thirteen packages have complete source evidence; nine native
  packages still need wheel-build provenance and native-library ownership review.
  FlatBuffers, ONNX Runtime, and the two PyMuPDF compatibility packages have no sdist;
  the PyMuPDF pair contains only dist-info metadata and still needs inclusion analysis.
  This is evidence only and is not yet represented in the tracked source lock.
- The FFmpeg capture path expects a retained build descriptor, source bundle, toolchain
  image, and provenance; the repository has no capture step that creates that closure
  from scratch. A source-closed FFmpeg lock cannot be reconstructed from version
  strings alone. `vendor-windows.py` therefore cannot assemble a validated helper
  payload, and `release-materials.py` cannot create compliance materials from the
  current inputs.
- This machine has no Docker or installed WSL distribution, so the FFmpeg capture's
  Linux rootless-Docker route cannot run locally. Native Windows Poppler capture
  resolved the exact 43-package conda-forge environment. The verifier now handles the
  five retained historical noarch feedstocks and rendered recipe forms, with five
  focused provenance tests passing. Capture still cannot emit a lock: the cache lacks
  25 distinct feedstock archives referenced by package records and upstream source
  archives for every source-bearing package. Some feedstocks declare only HTTP URLs,
  so capture needs verifiable HTTPS equivalents where available. This host's configured
  proxy points at `127.0.0.1:9`, and direct HTTPS to GitHub and official source hosts
  fails with WinError 10061. No Poppler lock or prepared prefix was generated, and no
  validated Windows helper payload is present under `vendor/windows/`.
- Earlier in this session, Smart App Control blocked unsigned build artifacts, and
  Application Control blocked Cargo-generated helper binaries. The maintainer has
  since turned Smart App Control off for local testing. This changes the local test
  conditions only; release artifacts remain unsigned and a future signed candidate
  still needs policy testing. Do not infer release readiness from the changed local
  policy state.
- No code-signing certificate is installed in either Windows certificate store, and
  the repository's GitHub Actions secret list contains only `DEBRAND_TOKENS`; it has
  no Windows signing certificate or password. The commercial image packager accepts
  an installed trusted certificate and verifies signed outputs, but this local
  candidate remains unsigned until a certificate is supplied.
- The current `ci.yml` and `release.yml` Windows jobs invoke `bundle-windows.ps1`
  without its required `-RuntimeInstaller` and `-ComplianceDir` inputs; the x64 job
  also has no validated helper payload provisioning step. Those workflow jobs cannot
  become green until the locked source, payload, and compliance inputs exist and can be
  supplied with their hashes verified.
- The full-product Windows packager requires an explicit edition and only builds `gpl`;
  its existing app/helper closure is not approved for proprietary distribution. The
  separate `commercial-windows.yml` workflow builds the narrow image candidate and
  retains it as an unsigned CI artifact. The commercial bundler accepts a trusted
  certificate thumbprint and signs the executable and PowerShell install scripts when
  one is available.
- GitHub CLI authentication has now been restored. Direct GitHub API calls work when
  this shell clears its configured HTTP(S) proxy (`127.0.0.1:9`) and adds Git for
  Windows to `PATH`. Actions history and artifacts were inspected; no Windows helper
  payload or commercial candidate is retained remotely. No remote changes or workflow
  dispatches have been made.
- `AGENTS.md` requires `scripts/preflight.sh` before any push. Its default
  `DEBRAND_TOKENS_FILE` is absent on this machine, and this Windows host has no WSL
  distribution or Linux rootless-Docker environment, so the full preflight has not
  run. The Windows-relevant local gates that did run are workspace formatting, UI
  tests/build, and the core tests/strict Clippy documented above.
- In the current continuation, a release-profile GUI build completed using a local
  ONNX Runtime 1.31.1 import library workaround. This is a developer launch only; it
  does not match the pinned release input. An ignored partial tools folder now has the
  locally captured Poppler executables/DLLs and PyInstaller PDF sidecar. Their direct
  version commands work when the sidecar temp directory is set inside that folder.
  `CONVERTME_TOOLS_DIR` now accepts either that folder or its `manifest.json` file; the
  previous file-path form treated the manifest as a directory and reported every
  helper missing. The sidecar contains AGPL PyMuPDF and must remain test-only for any
  proprietary edition.

## Next work, in order

### 1. Resume from a clean GitHub clone

Read `AGENTS.md`, `docs/HANDOFF.md`, this file, then inspect the current release
workflow and scripts. Use a separate working branch for changes. Generated build
directories such as `target/` and `ui/node_modules/` can be recreated. Do not assume
ignored files or local caches exist on the new machine.

### 2. Complete Windows packaging inputs and acceptance

Run from a supported, interactive Windows desktop as a standard (non-administrator)
user. For x64 and ARM64 package builds, provide:

- the validated x64 helper payload (also used by the ARM64 app build);
- the matching Microsoft-signed offline WebView2 installer for each target;
- a compliance directory whose manifest hashes bind the source archives, licenses,
  source lock, payload, and target.

`scripts/bundle-windows.ps1` checks these inputs and creates the setup, portable ZIP,
source/license archive, and inventory. It must receive explicit `-HelperPayload`,
`-RuntimeInstaller`, and `-ComplianceDir` paths. The current workflow does not yet
provide the latter two inputs; fix that integration only when the workflow can obtain
them from a verifiable, hash-bound source. Do not replace the checks with unverified
downloads or credentials committed to the repository.

The full acceptance runner is `scripts/run-windows-acceptance.ps1`. It requires the
exact candidate manifest, package, target inventory, and extracted fixture set. Its
automated matrix has 20 scenarios; the prior session had only six automated passes
and recorded the other 14 as blocked. It also requires separate manual evidence for
audio output, monitor/DPI behavior, installation lifecycle, Explorer drop, shell
integration, and path coverage. Keep blocked or unrun cases visible in the report.
An x64 Windows host can establish x64 runtime evidence; ARM64 runtime evidence needs
an ARM64 Windows environment.

### 3. Complete Linux inputs and replay

`scripts/build-linux-release.sh --capture-inputs --inputs <dir>` is an explicit
connected capture operation. It must not invent or silently repin inputs. Capture is
currently incomplete. The lock and manifest must contain and verify:

- Docker rootless archives and the pinned Ubuntu 24.04 builder image;
- signed Ubuntu snapshot evidence and the complete binary **and source** package
  closure;
- publisher-verified Rust 1.99 and Node 22 archives;
- offline Cargo and npm caches, Python wheelhouses, bundled tool archives, and all
  source materials needed for the package and inventory.

Only after that closure is complete should replay run with
`scripts/build-linux-release.sh --inputs <dir> --out <dir>`. The default route uses a
private rootless Docker daemon; `--native` is limited to the pinned Ubuntu 24.04 x64
environment. Do not fall back to the host distribution or rootful Docker. Linux
replay currently produces DEB/RPM only. AppImage requires a separately proven ELF and
runtime-data closure before it can be enabled.

## Acceptance and release gate

For each supported OS/architecture/package cell, retain the exact package hash,
inventory, fixture manifest and hashes, environment details, scenario results, and
manual observations. Package acceptance must refer to the exact candidate assets;
CLI probes or a successful build alone are not desktop acceptance.

The current workflow is manual-only and has no publisher job. Do not add automatic
publishing or weaken gates to make incomplete artifacts pass. Before calling the
release ready, verify every required input, package, source/license archive, native
acceptance cell, and evidence report from the same candidate state.

## Known local limitations at handoff

- The previous Linux capture stopped because the system Python had no `pip`; an
  earlier attempt also exposed a syntax error in the wheel capture helper. Recheck
  `scripts/prepare-linux-pdf-inputs.py` and use an isolated, pinned capture
  environment. A syntax-only compile succeeded in the later local checkout, but the
  wheel capture has not been rerun successfully.
- The Linux capture driver still reports the signed snapshot, package/source
  closure, retained builder image, Rust/Node archives, and offline caches as missing.
- No complete `vendor/windows/` helper payload or full-featured Windows candidate is
  present in the checkout. The narrow image candidate remains local under ignored
  `artifacts/` and is not part of the tracked source tree.
- Native Windows MSVC, Authenticode, UI Automation, audible playback, and ARM64
  runtime evidence cannot be established on a Linux host.
- Existing ignored evidence under `artifacts/` and captured inputs under `vendor/`
  are local state, not GitHub source. Never commit credentials or claim those files
  exist on a clean clone.

## Supporting records

The 2026-11-04 continuation-input snapshot is attached to the draft GitHub release
`continuation-inputs-2026-11-04`. It is about 337 MB and is intentionally marked
incomplete. Download it from an authenticated clone with:

```bash
gh release download continuation-inputs-2026-11-04 \
  --repo danyghraizi/convertme \
  --pattern 'convertme-continuation-inputs-2026-11-04.tar.gz*'
sha256sum -c convertme-continuation-inputs-2026-11-04.tar.gz.sha256
tar -xzf convertme-continuation-inputs-2026-11-04.tar.gz
```

Archive SHA-256: `ec2401bc31c2ef33879bc719fa158225a05fc8a38bd30455eb729dd901c091af`.
The archive embeds `INPUTS-MANIFEST.json` with the size and SHA-256 of each input.

The previous OMP session's detailed implementation notes and scenario worksheets
were local to that machine. This guide preserves the current decisions and execution
contracts needed to resume independently. The repository scripts and acceptance
contract are authoritative for exact command behavior and machine-readable schemas.
