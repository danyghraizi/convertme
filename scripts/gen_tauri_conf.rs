// Generates `crates/convertme-gui/tauri.conf.json` from the identity table.
//
// Two entry points, one implementation:
//  * `crates/convertme-gui/build.rs` includes this file and calls [`write_conf`]
//    before `tauri_build::build()`, so the app id in the bundle cannot drift from
//    `identity.rs`;
//  * `cargo xtask gen-conf` includes it too, for a config you can read without a
//    full build.
//
// Deliberately std-only: it is compiled as a build script, without any crates.

include!("../crates/convertme-core/src/identity.rs");

/// The external tools bundled per platform. `cargo xtask vendor` places
/// `<name>-<target-triple>[.exe]` next to them.
const BUNDLED_TOOLS: &[&str] = &["ffmpeg", "ffprobe", "magick", "gs", "qpdf", "pdftoppm", "pdftotext", "pandoc", "7zz", "tesseract", "convertme-pdf"];

/// Endpoint URLs for the updater, in the placeholder dialect Tauri understands.
fn update_endpoints() -> Vec<String> {
    vec![UPDATE_MANIFEST_TEMPLATE
        .replace("{target}", "{{target}}")
        .replace("{arch}", "{{arch}}")
        .replace("{version}", "{{current_version}}")]
}

fn json_string(s: &str) -> String {
    let mut out = String::with_capacity(s.len() + 2);
    out.push('"');
    for ch in s.chars() {
        match ch {
            '"' => out.push_str("\\\""),
            '\\' => out.push_str("\\\\"),
            '\n' => out.push_str("\\n"),
            '\r' => out.push_str("\\r"),
            '\t' => out.push_str("\\t"),
            c if (c as u32) < 0x20 => out.push_str(&format!("\\u{:04x}", c as u32)),
            c => out.push(c),
        }
    }
    out.push('"');
    out
}

fn json_array(items: &[String]) -> String {
    format!("[{}]", items.join(", "))
}

/// The app directory (the crate that owns this config), resolved from the caller's
/// `CARGO_MANIFEST_DIR`. Absolute paths in `beforeBuildCommand` are safe here: the
/// config itself is generated on each build and never committed.
fn app_dir() -> String {
    let manifest = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| ".".to_string());
    let path = std::path::PathBuf::from(&manifest);
    // `cargo xtask gen-conf` runs from the workspace's xtask crate.
    if path.file_name().map(|n| n == "xtask").unwrap_or(false)
        && let Some(root) = path.parent() {
            return root.join("crates").join("convertme-gui").display().to_string();
        }
    path.display().to_string()
}

/// The full config text.
pub fn tauri_conf_json() -> String {
    let app = app_dir();
    let root = std::path::Path::new(&app)
        .parent()
        .and_then(|crates| crates.parent())
        .map(|p| p.display().to_string())
        .unwrap_or_else(|| "..".to_string());
    let ui = format!("{root}/ui");
    let gate = format!("{root}/scripts/debrand.py");
    let external_bins: Vec<String> = BUNDLED_TOOLS
        .iter()
        .map(|t| json_string(&format!("../../vendor/bin/{t}")))
        .collect();
    let endpoints: Vec<String> = update_endpoints().iter().map(|e| json_string(e)).collect();

    let mut s = String::new();
    s.push_str("{\n");
    s.push_str("  \"$schema\": \"https://schema.tauri.app/config/2\",\n");
    // Tauri derives the Debian package name, the AppImage file name *and* the desktop
    // entry name from `productName`. The identity table pins the desktop entry to
    // `app.convertme.desktop`, so the id is the value that satisfies it; the binary
    // stays `convertme` (its name comes from the crate) and the display name stays
    // `ConvertMe` (the window title and the desktop entry's Name=).
    s.push_str(&format!("  \"productName\": {},\n", json_string(APP_ID)));
    s.push_str("  \"version\": \"0.1.0\",\n");
    s.push_str(&format!("  \"identifier\": {},\n", json_string(APP_ID)));
    s.push_str("  \"build\": {\n");
    s.push_str(&format!(
        "    \"beforeDevCommand\": {},\n",
        json_string(&format!("npm --prefix {ui} run dev"))
    ));
    s.push_str("    \"devUrl\": \"http://localhost:1420\",\n");
    s.push_str(&format!(
        "    \"beforeBuildCommand\": {},\n",
        json_string(&format!("python3 {gate} --src && npm --prefix {ui} run build"))
    ));
    s.push_str("    \"frontendDist\": \"ui-dist\"\n");
    s.push_str("  },\n");
    s.push_str("  \"app\": {\n");
    s.push_str("    \"windows\": [\n");
    s.push_str("      {\n");
    s.push_str("        \"label\": \"main\",\n");
    s.push_str(&format!("        \"title\": {},\n", json_string(APP_NAME)));
    s.push_str("        \"width\": 1120,\n");
    s.push_str("        \"height\": 760,\n");
    s.push_str("        \"minWidth\": 900,\n");
    s.push_str("        \"minHeight\": 600,\n");
    s.push_str("        \"dragDropEnabled\": true,\n");
    // Hidden until the app decides: `--background` leaves it hidden, and the normal
    // launch shows it once the UI is ready, which avoids a white first frame.
    s.push_str("        \"visible\": false,\n");
    s.push_str("        \"center\": true\n");
    s.push_str("      }\n");
    s.push_str("    ],\n");
    s.push_str("    \"security\": {\n");
    s.push_str("      \"csp\": \"default-src 'self'; img-src 'self' data: blob: asset: http://asset.localhost; media-src 'self' blob: asset: http://asset.localhost; style-src 'self' 'unsafe-inline'; script-src 'self'\",\n");
    s.push_str("      \"assetProtocol\": {\n");
    s.push_str("        \"enable\": true,\n");
    s.push_str(&format!(
        "        \"scope\": {}\n",
        json_array(&[
            json_string("$HOME/**"),
            json_string("$DOWNLOAD/**"),
            json_string("$CACHE/**"),
            json_string("$APPDATA/**"),
            json_string("$TEMP/**"),
        ])
    ));
    s.push_str("      }\n");
    s.push_str("    }\n");
    s.push_str("  },\n");
    s.push_str("  \"bundle\": {\n");
    s.push_str("    \"active\": true,\n");
    s.push_str("    \"targets\": \"all\",\n");
    s.push_str(&format!("    \"externalBin\": {},\n", json_array(&external_bins)));
    s.push_str("    \"icon\": [\"icons/32x32.png\", \"icons/128x128.png\", \"icons/128x128@2x.png\", \"icons/icon.icns\", \"icons/icon.ico\"],\n");
    s.push_str(&format!("    \"copyright\": {},\n", json_string("ConvertMe contributors")));
    s.push_str("    \"category\": \"Utility\",\n");
    s.push_str(&format!(
        "    \"shortDescription\": {},\n",
        json_string("Offline drag-and-drop file converter")
    ));
    s.push_str(&format!(
        "    \"longDescription\": {},\n",
        json_string("Convert images, video, audio, PDFs, documents, slides, spreadsheets, EPUB, subtitles and archives without uploading anything. Everything runs on this machine.")
    ));
    s.push_str("    \"linux\": {\n");
    s.push_str("      \"deb\": {\n");
    s.push_str(&format!("        \"desktopTemplate\": {},\n", json_string(&format!("{root}/packaging/app.convertme.desktop"))));
    s.push_str("        \"files\": {},\n");
    s.push_str(&format!("        \"recommends\": {},\n", json_array(&[json_string("libreoffice-core")])));
    s.push_str("        \"section\": \"utils\"\n");
    s.push_str("      },\n");
    s.push_str("      \"rpm\": {\n");
    s.push_str("        \"epoch\": 0,\n");
    s.push_str(&format!("        \"recommends\": {},\n", json_array(&[json_string("libreoffice-core")])));
    s.push_str(&format!("        \"desktopTemplate\": {}\n", json_string(&format!("{root}/packaging/app.convertme.desktop"))));
    s.push_str("      }\n");
    s.push_str("    },\n");
    s.push_str("    \"windows\": {\n");
    s.push_str("      \"nsis\": {\n");
    s.push_str("        \"installMode\": \"currentUser\",\n");
    s.push_str("        \"languages\": [\"English\"],\n");
    s.push_str("        \"displayLanguageSelector\": false\n");
    s.push_str("      },\n");
    s.push_str("      \"allowDowngrades\": false\n");
    s.push_str("    },\n");
    s.push_str(&format!("    \"resources\": {},\n", json_array(&[json_string("../../NOTICE"), json_string("../../LICENSE")])));
    // Signed update artifacts need the private key; without it the bundle step
    // would fail, so the flag follows the environment instead of being hardcoded.
    let signed = std::env::var_os("TAURI_SIGNING_PRIVATE_KEY").is_some();
    s.push_str(&format!("    \"createUpdaterArtifacts\": {signed}\n"));
    s.push_str("  },\n");
    s.push_str("  \"plugins\": {\n");
    s.push_str("    \"updater\": {\n");
    s.push_str(&format!("      \"endpoints\": {},\n", json_array(&endpoints)));
    let pubkey = std::env::var("CONVERTME_UPDATER_PUBKEY").unwrap_or_default();
    s.push_str(&format!("      \"pubkey\": {}\n", json_string(&pubkey)));
    s.push_str("    }\n");
    s.push_str("  }\n");
    s.push_str("}\n");
    s
}

/// Write the config next to the GUI crate's manifest. Returns the path written.
pub fn write_conf() -> std::path::PathBuf {
    let manifest_dir = std::env::var("CARGO_MANIFEST_DIR").unwrap_or_else(|_| ".".into());
    let path = std::path::Path::new(&manifest_dir).join("tauri.conf.json");
    let text = tauri_conf_json();
    std::fs::write(&path, text).expect("writing tauri.conf.json");
    path
}
