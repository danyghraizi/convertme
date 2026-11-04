# Commercial Windows product plan

## Goal

Ship a useful Windows converter that can be sold without buying third-party
commercial software licenses. Keep ConvertMe's product code under the project's
control and choose dependencies with terms the project can meet. This is a product
and engineering plan, not a legal determination. “No paid license” does not remove
copyright, source, notice, trademark, or codec-patent obligations.

## Current facts

- The repository declares GPL-3.0-or-later. Selling a GPL build is allowed, but
  recipients keep the GPL's source, modification, and redistribution rights.
- A proprietary edition cannot be made by changing the license label alone. Confirm
  rights for every contributor and imported asset before relicensing first-party code.
- The direct `imagequant` dependency has been removed and PNG palette reduction now
  uses MIT-licensed `color_quant` with app-owned error diffusion. This removes one
  Rust-level copyleft obstacle; it does not make the whole app eligible for
  proprietary distribution.
- The PDF sidecar uses PyMuPDF/MuPDF under AGPL terms. The current tool set also
  includes GPL/AGPL helpers such as FFmpeg, Poppler, Pandoc, and Ghostscript. Treat
  process boundaries as an architecture fact, not as a license conclusion.
- qpdf is Apache-2.0 and can remain a candidate for PDF structure operations. MPL
  Rust crates can be used in a larger product if their own source and notice terms
  are met. Audit exact versions and the final DLL/binary payload before distribution.
- A local unsigned Windows GUI currently launches, but there is no validated Windows
  helper payload or release candidate. The local test folder is deliberately partial.
- The `commercial-windows/` sibling is now an independently implemented proprietary
  image product, not a relabel of the existing GUI. It supports PNG/JPEG/WebP/BMP/GIF/
  TIFF still-image conversion, resizing, rotation, and per-file error reporting. Its
  Windows x64 candidate bundles the Microsoft WebView2 Evergreen installer and
  includes MPL-2.0 crate sources and notices in a separate third-party source archive.
  The generated package is unsigned and has not passed clean-machine installation or
  Smart App Control acceptance.

## Product and license route

Use a staged transition. Keep the current GPL terms during development and do not
publish a proprietary build until ownership and the complete dependency closure have
been reviewed. For an early paid release, the simplest route is a GPL edition sold
with corresponding source and required notices; charge for convenient installers,
updates, support, or training. If the product must be closed-source, build a separate
edition from code with confirmed project ownership and dependencies selected for that
distribution. An open-source release cannot promise that only the maintainer may
resell it.

The practical target is first-party workflow, UI, job management, and format-specific
code plus reviewed permissive or weak-copyleft libraries. Rewriting all image codecs,
media codecs, PDF parsers, and office formats from scratch is not a realistic first
release. Keep optional features out of the commercial edition until their dependency
and data licenses are documented.

## Milestones

### 1. Set the feature boundary and establish ownership

- Record who authored and owns the current commits and whether any work was done under
  employment, contract, or third-party contribution terms.
- Generate an exact inventory of linked crates, Python wheels, helper binaries, DLLs,
  fonts, trained data, and models for each Windows package.
- Replace blanket legal assurances in `NOTICE` with verified versioned facts and
  delivery locations before any distribution.
- Choose the first supported conversion matrix. Start with formats already handled
  by Rust libraries in the workspace, such as common raster image conversion and
  resize/rotate, subtitles, and straightforward tabular data. Do not expose an option
  unless the selected edition has an available engine for it.

### 2. Remove directly linked strong copyleft from the candidate edition

- Validate the `color_quant` replacement against existing image fixtures for output
  quality, speed, dimensions, palette size, transparency, and file size.
- Preserve the current GPL edition while evaluating a separate commercial build
  configuration. Do not copy GPL implementation code into the replacement.
- The commercial image set retains MPL-2.0 crates only with their exact crate sources
  and notices in the generated third-party source archive. This source package excludes
  first-party proprietary app code.

### 3. Replace or defer helper-dependent features

- PDF: investigate PDFium for rendering and text extraction, and keep qpdf for
  structure changes if the complete dependency closure is acceptable. PDFium does not
  replace every current PDF feature or editable-layout reconstruction. Defer high-
  fidelity PDF-to-Office, OCR/redaction, and advanced compression until each has a
  reviewed implementation.
- Images: use Windows Imaging Component or reviewed permissive Rust crates for a
  narrow Windows raster set. Define alpha, orientation, color-profile, and metadata
  behavior in the acceptance matrix. Defer HEIC/AVIF until codec distribution and
  patent questions are resolved.
- Audio/video: consider Windows Media Foundation for a documented subset, and verify
  codecs on clean supported Windows installations. Do not claim FFmpeg parity.
- Documents and archives: retain only conversions whose parser/writer and transitive
  licenses are recorded. State formatting or fidelity limits clearly.

### 4. Make the Windows package reproducible and testable

- Produce a hash-bound payload and source/notice archive for the chosen feature set.
- Make CI provide every required packaging input; fix the current workflow gaps before
  treating Windows packaging as green.
- Implement the missing acceptance scenarios, then install and test the exact candidate
  on clean x64 Windows. Add ARM64 only with native runtime evidence.
- Add a deliberate signing path, sign release artifacts, and validate install,
  Smart App Control, update, uninstall, and file association behavior.

## Immediate next actions

1. Confirm contributor rights and identify the legal copyright holder before public
   sale of the proprietary app.
2. Obtain a trusted signing identity and test the exact signed x64 package on clean
   Windows, including installation, launch, conversion, uninstall, and reputation
   controls.
3. Validate the commercial image app's full decode/encode matrix and user interface on
   clean systems; keep unsupported features out of its claims.
4. Evaluate more conversion families independently. Do not reuse the GPL/AGPL engine
   closure without a specific license and architecture review.

## Exit criteria

A Windows release is ready only when the exact package can be rebuilt from recorded
inputs; the license/source notices match the exact shipped payload; enabled features
pass the acceptance matrix on a clean supported Windows installation; signing and
Smart App Control behavior are validated; and marketing claims match tested behavior.
