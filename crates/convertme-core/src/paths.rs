//! Platform directories, all derived from [`crate::identity`].

use crate::identity;
use std::path::PathBuf;

/// `%APPDATA%\ConvertMe` on Windows, `~/.local/share/convertme` on Linux.
pub fn data_dir() -> PathBuf {
    if cfg!(windows) {
        std::env::var_os("APPDATA")
            .map(|a| PathBuf::from(a).join(identity::APP_NAME))
            .unwrap_or_else(|| PathBuf::from(identity::WIN_DATA_DIR))
    } else {
        std::env::var_os("XDG_DATA_HOME")
            .map(|d| PathBuf::from(d).join(identity::DIR_NAME))
            .or_else(|| {
                std::env::var_os("HOME").map(|h| {
                    PathBuf::from(h)
                        .join(".local/share")
                        .join(identity::DIR_NAME)
                })
            })
            .unwrap_or_else(|| PathBuf::from(".").join(identity::DIR_NAME))
    }
}

/// `~/.config/convertme`.
pub fn config_dir() -> PathBuf {
    if cfg!(windows) {
        data_dir()
    } else {
        std::env::var_os("XDG_CONFIG_HOME")
            .map(|d| PathBuf::from(d).join(identity::DIR_NAME))
            .or_else(|| {
                std::env::var_os("HOME")
                    .map(|h| PathBuf::from(h).join(".config").join(identity::DIR_NAME))
            })
            .unwrap_or_else(|| PathBuf::from(".").join(identity::DIR_NAME))
    }
}

/// `%LOCALAPPDATA%\ConvertMe\cache` or `~/.cache/convertme`.
pub fn cache_dir() -> PathBuf {
    if cfg!(windows) {
        std::env::var_os("LOCALAPPDATA")
            .map(|a| PathBuf::from(a).join(identity::APP_NAME).join("cache"))
            .unwrap_or_else(|| PathBuf::from(identity::WIN_CACHE_DIR))
    } else {
        std::env::var_os("XDG_CACHE_HOME")
            .map(|d| PathBuf::from(d).join(identity::DIR_NAME))
            .or_else(|| {
                std::env::var_os("HOME")
                    .map(|h| PathBuf::from(h).join(".cache").join(identity::DIR_NAME))
            })
            .unwrap_or_else(|| std::env::temp_dir().join(identity::DIR_NAME))
    }
}

pub fn previews_dir() -> PathBuf {
    cache_dir().join("previews")
}

pub fn history_db() -> PathBuf {
    data_dir().join("jobs.sqlite")
}

pub fn capabilities_json() -> PathBuf {
    data_dir().join("capabilities.json")
}

pub fn settings_json() -> PathBuf {
    config_dir().join("settings.json")
}

pub fn instance_lock() -> PathBuf {
    data_dir().join("instance.lock")
}

pub fn log_dir() -> PathBuf {
    data_dir().join("logs")
}

/// Create the runtime directories, ignoring "already exists".
pub fn ensure_dirs() -> std::io::Result<()> {
    for dir in [data_dir(), config_dir(), cache_dir(), previews_dir()] {
        std::fs::create_dir_all(dir)?;
    }
    Ok(())
}
