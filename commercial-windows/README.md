# ConvertMe Image

This is a standalone, proprietary Windows image edition. It is a clean implementation and does not link the GPL-licensed `convertme-core`, `convertme-engines`, or `convertme-gui` crates. `LICENSE` contains the product license notice; the project must confirm the copyright holder's legal identity and execute its customer license before public sale.

The current feature set converts still images among PNG, JPEG, WebP, BMP, GIF, and TIFF, and supports proportional downscaling and 90-degree rotation. Animated inputs are read as a still frame. JPEG output composites transparent pixels over white. Source metadata is not copied. Outputs are created beside the original with collision-safe names; source files are never overwritten.

The app processes files on the local machine. It bundles no FFmpeg, Poppler, Ghostscript, ImageMagick, Pandoc, OCR engine, Python sidecar, or model. Its Rust dependencies are pinned in `Cargo.lock`, its Rust compiler is pinned to 1.99.1, and third-party license notices include both Rust crate dependencies and the Rust standard library.

## Build and package

```powershell
Push-Location commercial-windows
cargo check --locked
cargo test --locked
Pop-Location
.\scripts\bundle-commercial-windows.ps1
```

The bundler creates a portable x64 ZIP and matching third-party source ZIP. The portable package includes a current-user PowerShell installer, a shared Microsoft WebView2 Evergreen Runtime installer, a hash manifest, the product license, generated dependency notices, and a pointer to the third-party source ZIP. That source ZIP contains the exact MPL-2.0 dependency sources and their notices; it does not contain first-party app source. The WebView2 file is checked against the versioned SHA-256 and Microsoft Authenticode signer in `packaging/windows/webview2.lock.json` before packaging. Microsoft documents bundling its Evergreen standalone installer for offline WebView2 distribution: https://learn.microsoft.com/en-us/microsoft-edge/webview2/concepts/distribution.

The default package is clearly named `candidate-unsigned`. To sign the executable, install a trusted code-signing certificate in the current-user or local-machine personal certificate store and pass its thumbprint with `-CertificateThumbprint`. `-RequireSigning` fails the build when that identity is absent. Unsigned packages are for internal validation only; the PowerShell installer is not an Authenticode-signed installer executable.

This is a narrow raster image release, not the existing application's broad conversion catalog. It does not implement file associations, native drag-and-drop, in-app updates, or automated update testing. It depends on the system's installed Evergreen WebView2 Runtime after the included installer runs.
