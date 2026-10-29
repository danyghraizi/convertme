//! Generates `tauri.conf.json` from the identity table, then hands over to Tauri.
//!
//! `scripts/gen_tauri_conf.rs` is the only implementation of that generation; it
//! is included here so the app id, product name and updater URL cannot drift.

include!("../../scripts/gen_tauri_conf.rs");

use std::path::Path;
use std::time::SystemTime;

fn main() {
    // Derive identity strings, then the config that consumes it.
    println!("cargo:rerun-if-changed=../../crates/convertme-core/src/identity.rs");
    println!("cargo:rerun-if-changed=../../scripts/gen_tauri_conf.rs");
    let path = write_conf();
    println!("cargo:rerun-if-changed={}", path.display());
    // The frontend bundle is embedded at compile time, so a rebuilt UI must force a
    // rebuild of this crate or the window shows the previous assets.
    println!("cargo:rerun-if-changed=ui-dist");
    println!("cargo:rerun-if-changed=../../ui/src");
    fail_if_stale();
    // Per-file, not per-directory: Vite rewrites existing files in place, which leaves
    // the directory mtime untouched, so a rebuilt bundle was never re-embedded and the
    // window kept showing the previous design.
    let mut files = files_under(Path::new("ui-dist"));
    files.sort();
    for file in &files {
        println!("cargo:rerun-if-changed={}", file.display());
    }
    // A stamp of the bundle actually embedded, so a running window can be told apart
    // from the build on disk.
    let mut stamp = 0u64;
    for file in &files {
        if let Ok(meta) = std::fs::metadata(file)
            && let Ok(modified) = meta.modified()
            && let Ok(since) = modified.duration_since(std::time::UNIX_EPOCH)
        {
            stamp = stamp.wrapping_add(since.as_secs()).wrapping_add(meta.len());
        }
    }
    println!("cargo:rustc-env=UI_BUILD_STAMP={stamp:x}");
    tauri_build::build();
}

/// A `cargo build` does not run `beforeBuildCommand`, so an edited `ui/src` would be
/// embedded as whatever `ui-dist` happened to hold — which is how a window ends up
/// showing the previous design. Fail loudly instead; the fix is one command.
fn fail_if_stale() {
    let dist = Path::new("ui-dist");
    let Some(newest_src) = newest_mtime(Path::new("../../ui/src"), None) else {
        return;
    };
    let Some(newest_dist) = newest_mtime(dist, None) else {
        panic!("ui-dist is missing: run `npm --prefix ui run build` before building the app");
    };
    if newest_src > newest_dist {
        panic!(
            "ui-dist is older than ui/src: run `npm --prefix ui run build` and rebuild. \
             `cargo build` alone cannot rebundle the frontend."
        );
    }
}

fn files_under(path: &Path) -> Vec<std::path::PathBuf> {
    let mut found = Vec::new();
    let Ok(entries) = std::fs::read_dir(path) else {
        return found;
    };
    for entry in entries.flatten() {
        let child = entry.path();
        if child.is_dir() {
            found.extend(files_under(&child));
        } else if child.is_file() {
            found.push(child);
        }
    }
    found
}

/// Recursive newest mtime, skipping the font and image assets that never change.
fn newest_mtime(path: &Path, newest: Option<SystemTime>) -> Option<SystemTime> {
    let meta = std::fs::metadata(path).ok()?;
    let mut latest = newest;
    let mut consider = |m: SystemTime| {
        latest = Some(match latest {
            Some(current) if current > m => current,
            _ => m,
        });
    };
    if meta.is_file() {
        consider(meta.modified().ok()?);
        return latest;
    }
    for entry in std::fs::read_dir(path).ok()? {
        let child = entry.ok()?.path();
        let name = child.file_name().and_then(|n| n.to_str()).unwrap_or("");
        if name == "node_modules" || name == ".vite" {
            continue;
        }
        if let Some(m) = newest_mtime(&child, latest) {
            latest = Some(m);
        }
    }
    latest
}
