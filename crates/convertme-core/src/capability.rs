//! Runtime capability cache: which external tools exist, their versions, and the
//! features the wheels care about (HEIC write support, encoders, …).
//!
//! The cache is written by [`crate::probe`]'s sibling in `convertme-engines` and is
//! read synchronously by the wheels at startup, so no surface ever waits on a
//! `--version` subprocess.

use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The external programs the app knows how to drive.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ToolKey {
    Ffmpeg,
    Ffprobe,
    Magick,
    Gs,
    Qpdf,
    Pdftoppm,
    Pdftotext,
    Pandoc,
    SevenZip,
    Tesseract,
    Sidecar,
    Soffice,
}

impl ToolKey {
    pub const ALL: [ToolKey; 12] = [
        ToolKey::Ffmpeg,
        ToolKey::Ffprobe,
        ToolKey::Magick,
        ToolKey::Gs,
        ToolKey::Qpdf,
        ToolKey::Pdftoppm,
        ToolKey::Pdftotext,
        ToolKey::Pandoc,
        ToolKey::SevenZip,
        ToolKey::Tesseract,
        ToolKey::Sidecar,
        ToolKey::Soffice,
    ];

    pub fn as_str(self) -> &'static str {
        match self {
            ToolKey::Ffmpeg => "ffmpeg",
            ToolKey::Ffprobe => "ffprobe",
            ToolKey::Magick => "magick",
            ToolKey::Gs => "gs",
            ToolKey::Qpdf => "qpdf",
            ToolKey::Pdftoppm => "pdftoppm",
            ToolKey::Pdftotext => "pdftotext",
            ToolKey::Pandoc => "pandoc",
            ToolKey::SevenZip => "7zz",
            ToolKey::Tesseract => "tesseract",
            ToolKey::Sidecar => "convertme-pdf",
            ToolKey::Soffice => "soffice",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        ToolKey::ALL
            .into_iter()
            .find(|k| k.as_str() == s)
            .or(match s {
                // Accept the alternate spellings a platform install may use.
                "7z" | "7za" => Some(ToolKey::SevenZip),
                "libreoffice" | "lowriter" => Some(ToolKey::Soffice),
                "convert" => Some(ToolKey::Magick),
                _ => None,
            })
    }

    /// Candidate file names, in probe order.
    pub fn candidates(self) -> &'static [&'static str] {
        match self {
            ToolKey::SevenZip => &["7zz", "7z"],
            ToolKey::Soffice => &["soffice", "libreoffice"],
            ToolKey::Magick => &["magick"],
            ToolKey::Sidecar => &["convertme-pdf"],
            ToolKey::Ffmpeg => &["ffmpeg"],
            ToolKey::Ffprobe => &["ffprobe"],
            ToolKey::Gs => &["gs"],
            ToolKey::Qpdf => &["qpdf"],
            ToolKey::Pdftoppm => &["pdftoppm"],
            ToolKey::Pdftotext => &["pdftotext"],
            ToolKey::Pandoc => &["pandoc"],
            ToolKey::Tesseract => &["tesseract"],
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            ToolKey::Ffmpeg | ToolKey::Ffprobe => "FFmpeg",
            ToolKey::Magick => "ImageMagick",
            ToolKey::Gs => "Ghostscript",
            ToolKey::Qpdf => "qpdf",
            ToolKey::Pdftoppm | ToolKey::Pdftotext => "Poppler",
            ToolKey::Pandoc => "Pandoc",
            ToolKey::SevenZip => "7-Zip",
            ToolKey::Tesseract => "Tesseract",
            ToolKey::Sidecar => "PDF sidecar",
            ToolKey::Soffice => "LibreOffice",
        }
    }

    /// Install hint shown in Settings when the tool is absent.
    pub fn install_hint(self) -> &'static str {
        match self {
            ToolKey::Soffice => "Install LibreOffice to enable Office → PDF and ODT conversions.",
            ToolKey::Tesseract => "Install Tesseract with the `eng` language data to enable OCR.",
            ToolKey::Magick => "Reinstall ConvertMe: the bundled image engine is missing.",
            _ => "Reinstall ConvertMe: a bundled tool is missing.",
        }
    }
}

/// One probed tool.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Capability {
    pub key: String,
    pub path: Option<PathBuf>,
    /// First line of `--version`, or the 7-Zip banner line.
    pub version: Option<String>,
    pub ok: bool,
    /// Feature flags the planner consults, e.g. `heic_write`, `libx265`, `avif`.
    pub features: Vec<String>,
    /// mtime of the binary when probed; a mismatch invalidates the entry.
    pub mtime: Option<i64>,
}

impl Capability {
    pub fn missing(key: ToolKey) -> Self {
        Self {
            key: key.as_str().to_string(),
            path: None,
            version: None,
            ok: false,
            features: Vec::new(),
            mtime: None,
        }
    }

    pub fn has_feature(&self, feature: &str) -> bool {
        self.ok && self.features.iter().any(|f| f == feature)
    }
}

/// The cache. Cheap to clone the query surface; stored as JSON in the data dir.
#[derive(Debug, Clone, Default, Serialize, Deserialize)]
pub struct Capabilities {
    #[serde(default)]
    pub entries: BTreeMap<String, Capability>,
    /// Wall-clock of the last completed probe, for Settings display only.
    #[serde(default)]
    pub probed_at: Option<String>,
}

impl Capabilities {
    /// Assume every tool is usable. Only for tests and for the planner's default
    /// signature; the GUI always passes a real cache.
    pub fn assume_all() -> Self {
        let mut caps = Self::default();
        for key in ToolKey::ALL {
            caps.entries.insert(
                key.as_str().to_string(),
                Capability {
                    key: key.as_str().to_string(),
                    path: Some(PathBuf::from(key.as_str())),
                    version: Some("assumed".into()),
                    ok: true,
                    features: vec!["heic_write".into(), "avif".into(), "libx265".into()],
                    mtime: None,
                },
            );
        }
        caps
    }

    pub fn get(&self, key: ToolKey) -> Option<&Capability> {
        self.entries.get(key.as_str())
    }

    pub fn available(&self, key: ToolKey) -> bool {
        self.get(key).map(|c| c.ok).unwrap_or(false)
    }

    pub fn has_engine(&self, engine: crate::formats::EngineRef) -> bool {
        engine.required().iter().all(|k| self.available(*k))
    }

    pub fn put(&mut self, cap: Capability) {
        self.entries.insert(cap.key.clone(), cap);
    }

    /// Drop entries whose binary disappeared or was replaced since probing.
    pub fn drop_stale(&mut self) {
        self.entries.retain(|_, cap| match (&cap.path, cap.mtime) {
            (Some(path), Some(mtime)) => file_mtime_secs(path) == Some(mtime) || !path.exists(),
            (Some(path), None) => path.exists(),
            _ => true,
        });
        // A vanished binary must not stay "ok".
        for cap in self.entries.values_mut() {
            if let Some(path) = &cap.path
                && !path.exists()
            {
                cap.ok = false;
            }
        }
    }

    pub fn missing_tools(&self) -> Vec<ToolKey> {
        ToolKey::ALL
            .into_iter()
            .filter(|k| !self.available(*k))
            .collect()
    }

    pub fn load(path: &Path) -> Self {
        let Ok(data) = std::fs::read(path) else {
            return Self::default();
        };
        let mut caps: Capabilities = serde_json::from_slice(&data).unwrap_or_default();
        caps.drop_stale();
        caps
    }

    pub fn save(&self, path: &Path) -> std::io::Result<()> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)?;
        }
        let tmp = path.with_extension("json.tmp");
        std::fs::write(&tmp, serde_json::to_vec_pretty(self)?)?;
        std::fs::rename(&tmp, path)
    }
}

pub fn file_mtime_secs(path: &Path) -> Option<i64> {
    let meta = std::fs::metadata(path).ok()?;
    let modified = meta.modified().ok()?;
    let dur = modified.duration_since(std::time::UNIX_EPOCH).ok()?;
    Some(dur.as_secs() as i64)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn assumed_capabilities_satisfy_every_engine() {
        let caps = Capabilities::assume_all();
        for engine in [
            crate::formats::EngineRef::Ffmpeg,
            crate::formats::EngineRef::LibreOffice,
            crate::formats::EngineRef::Sidecar,
        ] {
            assert!(caps.has_engine(engine));
        }
        assert!(caps.missing_tools().is_empty());
    }

    #[test]
    fn missing_tool_disables_its_engine() {
        let mut caps = Capabilities::assume_all();
        caps.put(Capability::missing(ToolKey::Magick));
        assert!(!caps.has_engine(crate::formats::EngineRef::ImageMagick));
        assert!(caps.has_engine(crate::formats::EngineRef::Pandoc));
        assert_eq!(caps.missing_tools(), vec![ToolKey::Magick]);
    }

    #[test]
    fn cache_roundtrips_and_drops_vanished_binaries() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("capabilities.json");
        let mut caps = Capabilities::assume_all();
        caps.put(Capability {
            key: "magick".into(),
            path: Some(dir.path().join("not-there")),
            version: None,
            ok: true,
            features: vec![],
            mtime: None,
        });
        caps.save(&path).unwrap();
        let loaded = Capabilities::load(&path);
        assert!(!loaded.available(ToolKey::Magick));
    }
}
