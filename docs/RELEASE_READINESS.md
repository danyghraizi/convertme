# Release readiness handoff

This document is the portable continuation point for the Linux and Windows release
work. Keep it current when release inputs, platform evidence, or blockers change. The
product and engineering invariants remain in `AGENTS.md`, `docs/HANDOFF.md`, and
`docs/LESSONS.md`.

## Current state

- Product version: `0.1.1`; release builds are unsigned.
- Supported release targets: Linux x86_64 DEB/RPM; Windows x64 and ARM64 packages.
- This repository contains the release implementation and acceptance harnesses, but
  **does not contain complete release inputs, candidate packages, or acceptance
  evidence**. A fresh GitHub clone is enough to continue source work, not to build a
  release immediately.
- Linux package replay is fail-closed and requires the complete locked offline input
  set described below. AppImage remains blocked; do not advertise or require it.
- Windows packaging is fail-closed and requires native Windows, a verified offline
  WebView2 installer, a validated helper payload, and compliance/source materials.
- Do not publish a release or claim full desktop acceptance until the completion
  criteria below are met.

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
- No `vendor/windows/` helper payload or Windows candidate package is present in the
  checkout at handoff.
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
