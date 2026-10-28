//! User settings. JSON at [`crate::paths::settings_json`], written atomically.
//!
//! Defaults follow the product spec: ⇧ convert, Ctrl file tools, Alt actions, all
//! remappable. Copy never names a platform key; only this table does.

use serde::{Deserialize, Serialize};
use std::path::Path;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum ModifierKey {
    Shift,
    Control,
    Alt,
    Meta,
}

impl ModifierKey {
    pub fn as_str(self) -> &'static str {
        match self {
            ModifierKey::Shift => "shift",
            ModifierKey::Control => "control",
            ModifierKey::Alt => "alt",
            ModifierKey::Meta => "meta",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s.to_ascii_lowercase().as_str() {
            "shift" => ModifierKey::Shift,
            "control" | "ctrl" => ModifierKey::Control,
            "alt" | "option" => ModifierKey::Alt,
            "meta" | "super" | "command" | "cmd" => ModifierKey::Meta,
            _ => return None,
        })
    }

    pub fn label(self) -> &'static str {
        match self {
            ModifierKey::Shift => "Shift",
            ModifierKey::Control => "Ctrl",
            ModifierKey::Alt => "Alt",
            ModifierKey::Meta => "Super",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct Settings {
    /// Key that opens the convert wheel.
    pub convert_key: ModifierKey,
    /// Key that opens the file-tools wheel.
    pub tools_key: ModifierKey,
    /// Key that opens the actions wheel.
    pub actions_key: ModifierKey,
    /// `never` | `ask` | `replace`, applied to *existing outputs* only.
    pub overwrite: String,
    pub history_limit: usize,
    pub preview_cache_bytes: u64,
    /// OCR languages beyond the bundled `eng`.
    pub ocr_languages: Vec<String>,
    /// Extra directory searched for tools before the bundled ones.
    pub extra_tools_dir: Option<std::path::PathBuf>,
    pub update_check: bool,
    /// Interaction mode override: `auto` | `wayland` | `x11` | `windows`.
    pub drag_backend: String,
    /// Launch with `--background` at login, so the wheel works with no window open.
    pub start_at_login: bool,
    /// Free-form user profile used by `redact` to remember box colours.
    pub redact_fill: String,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            convert_key: ModifierKey::Shift,
            tools_key: ModifierKey::Control,
            actions_key: ModifierKey::Alt,
            overwrite: "never".into(),
            history_limit: 500,
            preview_cache_bytes: 256 * 1024 * 1024,
            ocr_languages: vec!["eng".into()],
            extra_tools_dir: None,
            update_check: true,
            drag_backend: "auto".into(),
            start_at_login: false,
            redact_fill: "#000000".into(),
        }
    }
}

impl Settings {
    pub fn load(path: &Path) -> Self {
        std::fs::read(path)
            .ok()
            .and_then(|data| serde_json::from_slice(&data).ok())
            .unwrap_or_default()
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self)?)?;
        std::fs::rename(&tmp, path)
    }

    pub fn overwrite_policy(&self) -> crate::naming::Overwrite {
        crate::naming::Overwrite::parse(&self.overwrite)
    }

    /// The three wheel keys must be distinct, otherwise one wheel shadows another.
    pub fn validate(&self) -> Result<(), String> {
        if self.convert_key == self.tools_key
            || self.convert_key == self.actions_key
            || self.tools_key == self.actions_key
        {
            return Err("the three wheel keys must be different".into());
        }
        if self.history_limit == 0 {
            return Err("history limit must be at least 1".into());
        }
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn defaults_match_the_spec() {
        let s = Settings::default();
        assert_eq!(s.convert_key.as_str(), "shift");
        assert_eq!(s.tools_key.as_str(), "control");
        assert_eq!(s.actions_key.as_str(), "alt");
        assert_eq!(s.overwrite_policy(), crate::naming::Overwrite::Never);
        assert!(s.validate().is_ok());
    }

    #[test]
    fn roundtrips_atomically() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        let s = Settings {
            convert_key: ModifierKey::Meta,
            history_limit: 42,
            ..Default::default()
        };
        s.save(&path).unwrap();
        let back = Settings::load(&path);
        assert_eq!(back.convert_key, ModifierKey::Meta);
        assert_eq!(back.history_limit, 42);
        assert!(!path.with_extension("json.tmp").exists());
    }

    #[test]
    fn duplicate_wheel_keys_are_rejected() {
        let s = Settings {
            tools_key: Settings::default().convert_key,
            ..Default::default()
        };
        assert!(s.validate().is_err());
    }

    #[test]
    fn corrupt_file_falls_back_to_defaults() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("settings.json");
        std::fs::write(&path, b"{not json").unwrap();
        assert_eq!(Settings::load(&path), Settings::default());
    }
}
