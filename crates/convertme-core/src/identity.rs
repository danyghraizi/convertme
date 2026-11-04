// The one and only source of identity strings for ConvertMe.
//
// Every other crate, the generated Tauri config, the packaging metadata and the
// debrand gate read from here; no literal from this table is repeated elsewhere
// in the tree.
//
// This module is deliberately dependency-free (std only) so that build scripts
// and `scripts/gen_tauri_conf.rs` can `include!` it without pulling the crate in.

use std::path::PathBuf;

/// Display name shown in window titles, installers and About.
pub const APP_NAME: &str = "ConvertMe";
/// Reverse-DNS application id.
pub const APP_ID: &str = "app.convertme";
/// Binary name, CLI name and install directory leaf.
pub const BIN: &str = "convertme";
/// Base name for the per-user, per-session Windows single-instance pipe.
pub const PIPE: &str = "convertme-ipc";
/// Linux socket path relative to `$XDG_RUNTIME_DIR`.
pub const SOCKET_REL: &str = "convertme/ipc.sock";
/// Base name for the per-user, per-session Windows single-instance mutex.
pub const MUTEX: &str = "convertme-single";
/// Prefix of every temp work directory: `<tmp>/convertme-<pid>-<seq>`.
pub const TMP_PREFIX: &str = "convertme";
/// Directory name used under the platform data/config/cache roots.
pub const DIR_NAME: &str = "convertme";
/// Windows install directory (per-user, no elevation).
pub const WIN_INSTALL_DIR: &str = r"%LOCALAPPDATA%\Programs\ConvertMe";
/// Windows roaming data directory.
pub const WIN_DATA_DIR: &str = r"%APPDATA%\ConvertMe";
/// Linux data directory.
pub const LINUX_DATA_DIR: &str = "~/.local/share/convertme";
/// Linux config directory.
pub const LINUX_CONFIG_DIR: &str = "~/.config/convertme";
/// Linux desktop entry file name.
pub const DESKTOP_FILE: &str = "app.convertme.desktop";
/// Env var that raises the log level.
pub const ENV_LOG: &str = "CONVERTME_LOG";
/// Env var that overrides the bundled tools directory.
pub const ENV_TOOLS_DIR: &str = "CONVERTME_TOOLS_DIR";
/// Update manifest URL template (`{target}`, `{arch}`, `{version}`).
pub const UPDATE_MANIFEST_TEMPLATE: &str =
    "https://convertme.app/updates/{target}/{arch}/{version}";
/// Leaf directory used for the Downloads fallback, under the OS Downloads dir.
pub const DOWNLOAD_SUBDIR: &str = "ConvertMe";
/// Python sidecar package and executable stem.
pub const SIDECAR_BIN: &str = "convertme-pdf";
/// Python sidecar module package.
pub const SIDECAR_PACKAGE: &str = "convertme_pdf";

/// Windows marker for the data dir. Kept next to the literals so a reader sees the
/// whole identity table at once.
pub const WIN_CACHE_DIR: &str = r"%LOCALAPPDATA%\ConvertMe\cache";

/// `$XDG_RUNTIME_DIR/convertme/ipc.sock`, or a per-uid `/tmp` fallback when the
/// session has no runtime dir.
pub fn socket_path() -> PathBuf {
    match std::env::var_os("XDG_RUNTIME_DIR") {
        Some(dir) if !dir.is_empty() => PathBuf::from(dir).join(SOCKET_REL),
        _ => PathBuf::from(format!("/tmp/{DIR_NAME}-{}/ipc.sock", current_uid())),
    }
}

#[cfg(unix)]
fn current_uid() -> u32 {
    // Avoid a libc dependency in this dependency-free module.
    std::fs::read_to_string("/proc/self/status")
        .ok()
        .and_then(|s| {
            s.lines()
                .find_map(|l| l.strip_prefix("Uid:"))
                .and_then(|l| l.split_whitespace().next().map(|u| u.to_owned()))
        })
        .and_then(|u| u.parse().ok())
        .unwrap_or(0)
}

#[cfg(not(unix))]
fn current_uid() -> u32 {
    0
}

/// `<tmp>/convertme-<pid>-<seq>` for the given sequence number.
pub fn temp_work_dir(seq: u64) -> PathBuf {
    std::env::temp_dir().join(format!("{TMP_PREFIX}-{}-{seq}", std::process::id()))
}

/// `$XDG_DOWNLOAD_DIR/ConvertMe` (Linux) or `%USERPROFILE%\Downloads\ConvertMe`.
pub fn downloads_dir() -> PathBuf {
    if let Some(dir) = dirs_download() {
        return dir.join(DOWNLOAD_SUBDIR);
    }
    std::env::temp_dir().join(DOWNLOAD_SUBDIR)
}

fn dirs_download() -> Option<PathBuf> {
    if cfg!(windows) {
        let profile = std::env::var_os("USERPROFILE")?;
        return Some(PathBuf::from(profile).join("Downloads"));
    }
    // XDG user dir; fall back to `~/Downloads`.
    if let Ok(config) = std::fs::read_to_string(
        std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .or_else(|| std::env::var_os("HOME").map(|h| PathBuf::from(h).join(".config")))
            .map(|p| p.join("user-dirs.dirs"))
            .unwrap_or_default(),
    ) {
        for line in config.lines() {
            if let Some(rest) = line.trim().strip_prefix("XDG_DOWNLOAD_DIR=") {
                let val = rest.trim_matches('"');
                if let Some(home) = std::env::var_os("HOME") {
                    let val = val.replace("$HOME", &home.to_string_lossy());
                    return Some(PathBuf::from(val));
                }
            }
        }
    }
    std::env::var_os("HOME").map(|h| PathBuf::from(h).join("Downloads"))
}
