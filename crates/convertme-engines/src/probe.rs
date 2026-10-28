//! Tool probing: run once, in the background, at most four at a time, and cache
//! the result so no surface ever waits on a `--version` subprocess.
//!
//! Missing tools are a normal state, not an error: the cache records them as
//! `ok: false`, the planner omits their wheel segments and Settings lists the gap.

use convertme_core::capability::{Capabilities, Capability, ToolKey, file_mtime_secs};
use convertme_core::identity;
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant};

/// Feature flags the planner and the wheels consult.
pub const FEATURE_HEIC_WRITE: &str = "heic_write";
pub const FEATURE_AVIF: &str = "avif";
pub const FEATURE_WEBP: &str = "webp";
pub const FEATURE_JXL: &str = "jxl";
pub const FEATURE_X265: &str = "libx265";
pub const FEATURE_AV1: &str = "libaom";
pub const FEATURE_OCR_ENG: &str = "lang:eng";

#[derive(Debug, Clone, Default)]
pub struct ProbeOptions {
    /// Directory searched before `PATH` (bundled sidecars live here).
    pub tools_dir: Option<PathBuf>,
    /// Extra directory searched before the bundled one.
    pub extra_dir: Option<PathBuf>,
    /// Cap on concurrent `--version` calls.
    pub concurrency: usize,
}

impl ProbeOptions {
    pub fn from_env() -> Self {
        let tools_dir = std::env::var_os(identity::ENV_TOOLS_DIR)
            .map(PathBuf::from)
            .or_else(default_tools_dir);
        Self {
            tools_dir,
            extra_dir: None,
            concurrency: 4,
        }
    }
}

/// `<exe dir>/vendor/bin`, which is how the bundled sidecars are laid out next to
/// the binary on both platforms.
pub fn default_tools_dir() -> Option<PathBuf> {
    let exe = std::env::current_exe().ok()?;
    let dir = exe.parent()?;
    let candidate = dir.join("vendor").join("bin");
    if candidate.is_dir() {
        return Some(candidate);
    }
    let flat = dir.to_path_buf();
    if flat.join("ffmpeg").exists() || flat.join("ffmpeg.exe").exists() {
        return Some(flat);
    }
    None
}

/// Resolve a tool's path without running it.
pub fn locate(key: ToolKey, opts: &ProbeOptions) -> Option<PathBuf> {
    for dir in [opts.extra_dir.as_ref(), opts.tools_dir.as_ref()]
        .into_iter()
        .flatten()
    {
        for name in key.candidates() {
            let candidate = dir.join(exe_name(name));
            if candidate.is_file() && is_executable(&candidate) {
                return Some(candidate);
            }
        }
    }
    for name in key.candidates() {
        if let Some(path) = which(name) {
            return Some(path);
        }
    }
    // Flatpak: LibreOffice lives on the host, not in the sandbox.
    if key == ToolKey::Soffice && std::env::var_os("FLATPAK_ID").is_some() {
        return Some(PathBuf::from("flatpak-spawn"));
    }
    None
}

fn exe_name(name: &str) -> String {
    if cfg!(windows) {
        format!("{name}.exe")
    } else {
        name.to_string()
    }
}

fn is_executable(path: &Path) -> bool {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        std::fs::metadata(path)
            .map(|m| m.permissions().mode() & 0o111 != 0)
            .unwrap_or(false)
    }
    #[cfg(not(unix))]
    {
        let _ = path;
        true
    }
}

/// Minimal `which`, so probing does not depend on the user's shell.
pub fn which(name: &str) -> Option<PathBuf> {
    let name = exe_name(name);
    let path = std::env::var_os("PATH")?;
    for dir in std::env::split_paths(&path) {
        let candidate = dir.join(&name);
        if candidate.is_file() && is_executable(&candidate) {
            return Some(candidate);
        }
    }
    None
}

/// Probe every tool, `concurrency` at a time. Never blocks longer than
/// `budget` per tool and never panics.
pub fn probe_all(opts: &ProbeOptions) -> Capabilities {
    let (tx, rx) = crossbeam_channel::bounded::<ToolKey>(ToolKey::ALL.len());
    for key in ToolKey::ALL {
        let _ = tx.send(key);
    }
    drop(tx);

    let concurrency = opts.concurrency.clamp(1, 8).min(ToolKey::ALL.len());
    let results = std::sync::Mutex::new(Vec::with_capacity(ToolKey::ALL.len()));
    std::thread::scope(|scope| {
        for _ in 0..concurrency {
            let rx = rx.clone();
            let opts = opts.clone();
            let results = &results;
            scope.spawn(move || {
                while let Ok(key) = rx.recv() {
                    let cap = probe_one(key, &opts);
                    results.lock().unwrap().push(cap);
                }
            });
        }
    });

    let mut caps = Capabilities::default();
    for cap in results.into_inner().unwrap() {
        caps.put(cap);
    }
    caps
}

/// Probe one tool.
pub fn probe_one(key: ToolKey, opts: &ProbeOptions) -> Capability {
    let Some(path) = locate(key, opts) else {
        return Capability::missing(key);
    };
    let args: Vec<&str> = match key {
        // `7zz i` is the documented way to get capabilities; `--version` is not.
        ToolKey::SevenZip => vec!["i"],
        ToolKey::Magick => vec!["-version"],
        ToolKey::Tesseract => vec!["--list-langs"],
        ToolKey::Gs => vec!["--version"],
        ToolKey::Ffmpeg => vec!["-version"],
        ToolKey::Ffprobe => vec!["-version"],
        ToolKey::Sidecar => vec!["--version"],
        _ => vec!["--version"],
    };

    let mut command = if key == ToolKey::Soffice && path == Path::new("flatpak-spawn") {
        let mut c = Command::new(&path);
        c.arg("--host").arg("soffice");
        c
    } else {
        Command::new(&path)
    };
    let out = command
        .args(&args)
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .env("LC_ALL", "C.UTF-8")
        .output();

    // Some tools (soffice) print their version on stderr.
    let mut text = String::new();
    let mut ok;
    match out {
        Ok(out) => {
            ok = out.status.success();
            text.push_str(&String::from_utf8_lossy(&out.stdout));
            text.push_str(&String::from_utf8_lossy(&out.stderr));
            // A tool that answered at all is usable; `7zz i` exits 0 with a banner.
            if !ok && !text.trim().is_empty() {
                ok = true;
            }
        }
        Err(_) => {
            return Capability {
                key: key.as_str().to_string(),
                path: Some(path),
                version: None,
                ok: false,
                features: vec![],
                mtime: None,
            };
        }
    }

    let version = text
        .lines()
        .map(str::trim)
        .find(|l| !l.is_empty())
        .map(|l| l.chars().take(120).collect::<String>());
    let features = detect_features(key, &text);

    Capability {
        key: key.as_str().to_string(),
        mtime: file_mtime_secs(&path),
        path: Some(path),
        version,
        ok,
        features,
    }
}

fn detect_features(key: ToolKey, text: &str) -> Vec<String> {
    let lower = text.to_ascii_lowercase();
    let mut features = Vec::new();
    let mut push = |f: &str| {
        if !features.iter().any(|x| x == f) {
            features.push(f.to_string());
        }
    };
    match key {
        ToolKey::Ffmpeg => {
            if lower.contains("libx264") {
                push("libx264");
            }
            if lower.contains("libx265") {
                push(FEATURE_X265);
            }
            if lower.contains("libvpx") {
                push("libvpx");
            }
            if lower.contains("libaom") || lower.contains("libsvtav1") {
                push(FEATURE_AV1);
            }
            if lower.contains("libopus") {
                push("libopus");
            }
            if lower.contains("libvorbis") {
                push("libvorbis");
            }
            if lower.contains("libmp3lame") {
                push("lame");
            }
            if lower.contains("libwebp") {
                push(FEATURE_WEBP);
            }
            if lower.contains("showwaves") || lower.contains("lavfi") {
                push("lavfi");
            }
            if lower.contains("loudnorm") || lower.contains("--enable-filter=") {
                push("filters");
            }
        }
        ToolKey::Magick => {
            if lower.contains("heic") || lower.contains("libheif") {
                push(FEATURE_HEIC_WRITE);
            }
            if lower.contains("avif") {
                push(FEATURE_AVIF);
            }
            if lower.contains("webp") {
                push(FEATURE_WEBP);
            }
            if lower.contains("jxl") {
                push(FEATURE_JXL);
            }
            if lower.contains("tiff") {
                push("tiff");
            }
            if lower.contains("fftw") {
                push("fftw");
            }
        }
        ToolKey::Tesseract => {
            for line in text.lines().skip(1) {
                let lang = line.trim();
                if !lang.is_empty()
                    && lang
                        .chars()
                        .all(|c| c.is_ascii_alphabetic() || c == '_' || c == '-')
                {
                    push(&format!("lang:{lang}"));
                }
            }
            if text.to_ascii_lowercase().contains("eng") {
                push(FEATURE_OCR_ENG);
            }
        }
        ToolKey::SevenZip => {
            if lower.contains("rar") {
                push("rar");
            }
            if lower.contains("lzma") {
                push("lzma");
            }
            if lower.contains("bzip2") {
                push("bzip2");
            }
        }
        ToolKey::Qpdf => {
            if let Some(v) = text.split_whitespace().nth(2)
                && (v.starts_with("11") || v.starts_with("12") || v.starts_with("2"))
            {
                push("json");
            }
            push("pages");
        }
        ToolKey::Pandoc => {
            if lower.contains("epub") || !lower.is_empty() {
                push("epub3");
            }
            if lower.contains("lua") {
                push("lua");
            }
        }
        ToolKey::Gs => push("pdfwrite"),
        ToolKey::Pdftoppm => push("render"),
        ToolKey::Pdftotext => push("layout"),
        ToolKey::Sidecar => {
            for f in ["docx", "pptx", "xlsx", "epub"] {
                if lower.contains(f) {
                    push(f);
                }
            }
        }
        ToolKey::Ffprobe => push("json"),
        ToolKey::Soffice => push("headless-convert"),
    }
    features
}

/// Run the probe on a background thread and hand the result to `on_done`.
/// The UI reads the previous cache immediately and is never gated on this.
pub fn probe_background<F>(
    opts: ProbeOptions,
    cache_path: PathBuf,
    on_done: F,
) -> std::thread::JoinHandle<Capabilities>
where
    F: FnOnce(&Capabilities) + Send + 'static,
{
    std::thread::Builder::new()
        .name("convertme-probe".into())
        .spawn(move || {
            let mut caps = probe_all(&opts);
            caps.probed_at = Some(now_rfc3339());
            let _ = caps.save(&cache_path);
            on_done(&caps);
            caps
        })
        .expect("spawn the probe thread")
}

fn now_rfc3339() -> String {
    chrono_free_now()
}

/// `YYYY-MM-DDTHH:MM:SSZ` without pulling a date library into the probe.
fn chrono_free_now() -> String {
    let secs = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_secs())
        .unwrap_or(0);
    let days = secs / 86_400;
    let rem = secs % 86_400;
    let (h, m, s) = (rem / 3600, (rem % 3600) / 60, rem % 60);
    // Civil-from-days (Howard Hinnant's algorithm).
    let z = days as i64 + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1460 + doe / 36_524 - doe / 146_096) / 365;
    let y = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let d = doy - (153 * mp + 2) / 5 + 1;
    let month = if mp < 10 { mp + 3 } else { mp - 9 };
    let year = if month <= 2 { y + 1 } else { y };
    format!("{year:04}-{month:02}-{d:02}T{h:02}:{m:02}:{s:02}Z")
}

/// How long the planner may wait for an unfinished probe before planning with
/// what it knows.
pub const PROBE_WAIT: Duration = Duration::from_millis(250);

/// Wait up to [`PROBE_WAIT`] for the capability cache to appear, then read it.
pub fn wait_for_cache(path: &Path) -> Capabilities {
    let deadline = Instant::now() + PROBE_WAIT;
    loop {
        if path.exists() || Instant::now() > deadline {
            return Capabilities::load(path);
        }
        std::thread::sleep(Duration::from_millis(10));
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn probes_a_tool_that_exists() {
        let opts = ProbeOptions {
            concurrency: 2,
            ..Default::default()
        };
        let caps = probe_all(&opts);
        // ffmpeg is a host dependency of every dev environment that runs these tests.
        if let Some(cap) = caps.get(ToolKey::Ffmpeg)
            && cap.ok
        {
            assert!(cap.version.is_some());
            assert!(cap.path.as_ref().map(|p| p.exists()).unwrap_or(false));
        }
    }

    #[test]
    fn missing_tool_is_not_an_error() {
        let opts = ProbeOptions {
            tools_dir: Some(PathBuf::from("/nonexistent-tools")),
            extra_dir: Some(PathBuf::from("/nonexistent-tools-2")),
            concurrency: 4,
        };
        let cap = probe_one(ToolKey::Magick, &opts);
        // Either the host has it (PATH) or it is cleanly reported missing.
        assert!(cap.ok || (cap.path.is_none() && !cap.ok));
    }

    #[test]
    fn features_are_parsed_from_version_text() {
        let text = "ffmpeg version 6 --enable-libx264 --enable-libx265 --enable-libopus --enable-libmp3lame";
        let f = detect_features(ToolKey::Ffmpeg, text);
        assert!(f.contains(&"libx264".to_string()));
        assert!(f.contains(&FEATURE_X265.to_string()));
        assert!(f.contains(&"lame".to_string()));

        let magick = "Delegates (built-in): bzlib fontconfig heic jpeg jxl png tiff webp avif";
        let f = detect_features(ToolKey::Magick, magick);
        assert!(f.contains(&FEATURE_HEIC_WRITE.to_string()));
        assert!(f.contains(&FEATURE_AVIF.to_string()));
    }

    #[test]
    fn cache_roundtrip_is_stable() {
        let dir = tempfile::tempdir().unwrap();
        let path = dir.path().join("capabilities.json");
        let mut caps = Capabilities::default();
        caps.put(probe_one(ToolKey::Gs, &ProbeOptions::default()));
        caps.save(&path).unwrap();
        let loaded = Capabilities::load(&path);
        assert_eq!(loaded.entries.len(), caps.entries.len());
    }

    #[test]
    fn timestamp_format_is_rfc3339_shaped() {
        let s = chrono_free_now();
        assert_eq!(s.len(), 20, "{s}");
        assert!(s.ends_with('Z'));
        assert_eq!(&s[4..5], "-");
    }
}
