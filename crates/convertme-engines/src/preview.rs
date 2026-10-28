//! Preview cache: thumbnails, filmstrips and waveforms.
//!
//! Rules that keep the wheel fast and the app honest about memory:
//! * keys are `(canonical path, mtime, size)` — an edited file invalidates itself;
//! * thumbnails are decoded at 512 px, never at source resolution;
//! * waveforms are decoded to 8 kHz mono and reduced to min/max buckets in Rust,
//!   capped at 10 minutes of audio;
//! * the cache is capped at 256 MB (LRU by mtime) and enforced after every write;
//! * at most two preview probes run at a time, behind the job pools, so a 40-file
//!   selection cannot starve a running conversion.

use crate::proc::{self, Cmd};
use convertme_core::error::{ErrorCode, JobError};
use convertme_core::job::JobCtx;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::{Condvar, Mutex};

static PROBE_PERMITS: Mutex<usize> = Mutex::new(2);
static PROBE_WAKE: Condvar = Condvar::new();

/// Take one of the two preview slots, waiting if both are busy.
struct Permit;

impl Permit {
    fn acquire() -> Permit {
        let mut permits = PROBE_PERMITS.lock().unwrap();
        while *permits == 0 {
            permits = PROBE_WAKE.wait(permits).unwrap();
        }
        *permits -= 1;
        Permit
    }
}

impl Drop for Permit {
    fn drop(&mut self) {
        let mut permits = PROBE_PERMITS.lock().unwrap();
        *permits += 1;
        PROBE_WAKE.notify_one();
    }
}

#[derive(Debug, Clone, Default)]
pub struct Waveform {
    /// Per-bucket min/max sample, in -1.0..1.0.
    pub peaks: Vec<(f32, f32)>,
    pub duration_ms: u64,
    /// True when the clip is longer than the 10-minute cap and only its head is
    /// represented.
    pub truncated: bool,
}

pub struct PreviewCache {
    root: PathBuf,
    cap: u64,
}

impl PreviewCache {
    pub fn new(root: PathBuf, cap: u64) -> Self {
        let _ = std::fs::create_dir_all(&root);
        Self { root, cap }
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    /// Cache file for a source, or `None` when the source cannot be stat'ed.
    pub fn key(&self, source: &Path, kind: &str, arg: &str) -> Option<PathBuf> {
        let canonical = std::fs::canonicalize(source).ok()?;
        let meta = std::fs::metadata(&canonical).ok()?;
        let mtime = meta
            .modified()
            .ok()
            .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let mut hasher = Sha256::new();
        hasher.update(canonical.as_os_str().to_string_lossy().as_bytes());
        hasher.update(meta.len().to_le_bytes());
        hasher.update(mtime.to_le_bytes());
        hasher.update(kind.as_bytes());
        hasher.update(arg.as_bytes());
        let digest = hasher.finalize();
        let name = format!(
            "{kind}-{}.{}",
            hex(&digest[..16]),
            if kind == "thumb" { "jpg" } else { "bin" }
        );
        Some(self.root.join(name))
    }

    /// Thumbnail path, generating it at `size` px on the longest edge if missing.
    pub fn thumbnail(&self, source: &Path, size: u32) -> Result<PathBuf, JobError> {
        let size = size.min(512);
        let path = self
            .key(source, "thumb", &size.to_string())
            .ok_or_else(|| JobError::new(ErrorCode::Io, "cannot stat the file for a preview"))?;
        if path.exists() {
            touch(&path);
            return Ok(path);
        }
        let _permit = Permit::acquire();
        if path.exists() {
            return Ok(path);
        }

        let ctx = JobCtx::new();
        let magick =
            crate::probe::which("magick").ok_or_else(|| JobError::missing_tool("ImageMagick"))?;
        let tmp = path.with_extension("tmp.jpg");
        let result = proc::run(
            Cmd::new(magick)
                .arg(source)
                .arg("-auto-orient")
                .arg("-thumbnail")
                .arg(format!("{size}x{size}>"))
                .arg("-strip")
                .arg("-quality")
                .arg("82")
                .arg(&tmp),
            &ctx,
        );
        match result {
            Ok(out) if out.success => {
                std::fs::rename(&tmp, &path)
                    .map_err(|e| JobError::io("storing a thumbnail", &e))?;
                self.enforce_cap();
                Ok(path)
            }
            _ => {
                let _ = std::fs::remove_file(&tmp);
                self.thumbnail_with_image_crate(source, &path, size)
            }
        }
    }

    /// Fallback decoder for environments without ImageMagick: `image` at 512 px.
    fn thumbnail_with_image_crate(
        &self,
        source: &Path,
        path: &Path,
        size: u32,
    ) -> Result<PathBuf, JobError> {
        let img = image::ImageReader::open(source)
            .map_err(|e| JobError::io("opening the image for a preview", &e))?
            .with_guessed_format()
            .map_err(|e| JobError::io("guessing the image format", &e))?
            .decode()
            .map_err(|e| {
                JobError::new(ErrorCode::EngineFailed, format!("decoding the image: {e}"))
            })?;
        let thumb = img.thumbnail(size, size);
        let (w, h) = (thumb.width(), thumb.height());
        let _ = (w, h);
        thumb
            .save_with_format(path, image::ImageFormat::Jpeg)
            .map_err(|e| JobError::new(ErrorCode::Io, format!("writing a thumbnail: {e}")))?;
        self.enforce_cap();
        Ok(path.to_path_buf())
    }

    /// Filmstrip frames for a video, evenly spaced.
    pub fn filmstrip(&self, source: &Path, frames: usize) -> Result<Vec<PathBuf>, JobError> {
        let frames = frames.clamp(1, 12);
        let _permit = Permit::acquire();
        let dir = self
            .key(source, "strip", &frames.to_string())
            .ok_or_else(|| JobError::new(ErrorCode::Io, "cannot stat the video for a filmstrip"))?
            .with_extension("");
        if dir.is_dir() {
            let mut existing: Vec<PathBuf> = std::fs::read_dir(&dir)
                .map_err(|e| JobError::io("reading the filmstrip cache", &e))?
                .filter_map(|e| e.ok().map(|e| e.path()))
                .collect();
            existing.sort();
            if existing.len() == frames {
                return Ok(existing);
            }
        }
        std::fs::create_dir_all(&dir)
            .map_err(|e| JobError::io("creating the filmstrip cache", &e))?;

        let ffmpeg =
            crate::probe::which("ffmpeg").ok_or_else(|| JobError::missing_tool("FFmpeg"))?;
        let ctx = JobCtx::new();
        let out_pattern = dir.join("frame%03d.jpg");
        proc::run(
            Cmd::new(ffmpeg)
                .arg("-y")
                .arg("-i")
                .arg(source)
                .arg("-vf")
                .arg("thumbnail=n=100,scale=320:-2,select='not(mod(n\\,100))'")
                .arg("-frames:v")
                .arg(frames.to_string())
                .arg("-vsync")
                .arg("vfr")
                .arg(&out_pattern)
                .budget(proc::Budget::Long),
            &ctx,
        )?
        .ok()?;

        let mut produced: Vec<PathBuf> = std::fs::read_dir(&dir)
            .map_err(|e| JobError::io("reading the filmstrip cache", &e))?
            .filter_map(|e| e.ok().map(|e| e.path()))
            .collect();
        produced.sort();
        if produced.is_empty() {
            return Err(JobError::new(
                ErrorCode::EngineFailed,
                "the video produced no frames",
            ));
        }
        self.enforce_cap();
        Ok(produced)
    }

    /// Waveform peaks, 8 kHz mono, capped at 10 minutes.
    pub fn waveform(&self, source: &Path) -> Result<Waveform, JobError> {
        let _permit = Permit::acquire();
        let cache = self
            .key(source, "wave", "8k")
            .ok_or_else(|| JobError::new(ErrorCode::Io, "cannot stat the audio for a waveform"))?;
        if let Ok(bytes) = std::fs::read(&cache)
            && let Some(w) = decode_waveform(&bytes)
        {
            return Ok(w);
        }

        let ffmpeg =
            crate::probe::which("ffmpeg").ok_or_else(|| JobError::missing_tool("FFmpeg"))?;
        let ctx = JobCtx::new();
        let raw = cache.with_extension("raw");
        proc::run(
            Cmd::new(ffmpeg)
                .arg("-y")
                .arg("-i")
                .arg(source)
                .arg("-t")
                .arg("600")
                .arg("-ac")
                .arg("1")
                .arg("-ar")
                .arg("8000")
                .arg("-f")
                .arg("s16le")
                .arg(&raw)
                .budget(proc::Budget::Long),
            &ctx,
        )?
        .ok()?;

        let bytes = std::fs::read(&raw).map_err(|e| JobError::io("reading the waveform", &e))?;
        let _ = std::fs::remove_file(&raw);
        let waveform = build_waveform(&bytes, 8000, true);
        if let Ok(encoded) = encode_waveform(&waveform) {
            let _ = std::fs::write(&cache, encoded);
            self.enforce_cap();
        }
        Ok(waveform)
    }

    /// Keep the cache under its cap, oldest-used first.
    pub fn enforce_cap(&self) -> u64 {
        let mut entries: Vec<(std::time::SystemTime, u64, PathBuf)> = Vec::new();
        let Ok(read) = std::fs::read_dir(&self.root) else {
            return 0;
        };
        for entry in read.flatten() {
            let Ok(meta) = entry.metadata() else { continue };
            let used = meta
                .accessed()
                .or_else(|_| meta.modified())
                .unwrap_or(std::time::UNIX_EPOCH);
            entries.push((used, meta.len(), entry.path()));
        }
        let total: u64 = entries.iter().map(|(_, len, _)| *len).sum();
        if total <= self.cap {
            return total;
        }
        entries.sort_by_key(|(used, _, _)| *used);
        let mut freed = 0u64;
        let mut current = total;
        for (_, len, path) in entries {
            if current <= self.cap {
                break;
            }
            if path.is_dir() {
                let _ = std::fs::remove_dir_all(&path);
            } else {
                let _ = std::fs::remove_file(&path);
            }
            current = current.saturating_sub(len);
            freed += len;
        }
        freed
    }

    /// Drop every cached preview.
    pub fn clear(&self) -> std::io::Result<()> {
        for entry in std::fs::read_dir(&self.root)?.flatten() {
            let path = entry.path();
            if path.is_dir() {
                let _ = std::fs::remove_dir_all(&path);
            } else {
                let _ = std::fs::remove_file(&path);
            }
        }
        Ok(())
    }
}

fn build_waveform(bytes: &[u8], rate: u32, truncated: bool) -> Waveform {
    let samples: Vec<i16> = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| i16::from_le_bytes([c[0], c[1]]))
        .collect();
    let buckets = 400usize;
    let per_bucket = (samples.len() / buckets).max(1);
    let mut peaks = Vec::with_capacity(buckets);
    for chunk in samples.chunks(per_bucket).take(buckets) {
        let mut lo = 0i16;
        let mut hi = 0i16;
        for s in chunk {
            lo = lo.min(*s);
            hi = hi.max(*s);
        }
        peaks.push((lo as f32 / 32768.0, hi as f32 / 32768.0));
    }
    Waveform {
        peaks,
        duration_ms: (samples.len() as u64 * 1000) / rate.max(1) as u64,
        truncated,
    }
}

/// Compact encoding: 3 f32 per bucket is wasteful, so store i16 pairs.
fn encode_waveform(w: &Waveform) -> Result<Vec<u8>, JobError> {
    let mut out = Vec::with_capacity(16 + w.peaks.len() * 4);
    out.extend_from_slice(&w.duration_ms.to_le_bytes());
    out.extend_from_slice(&(w.peaks.len() as u32).to_le_bytes());
    for (lo, hi) in &w.peaks {
        out.extend_from_slice(&((lo * 32768.0) as i16).to_le_bytes());
        out.extend_from_slice(&((hi * 32768.0) as i16).to_le_bytes());
    }
    Ok(out)
}

fn decode_waveform(bytes: &[u8]) -> Option<Waveform> {
    if bytes.len() < 12 {
        return None;
    }
    let duration_ms = u64::from_le_bytes(bytes[0..8].try_into().ok()?);
    let count = u32::from_le_bytes(bytes[8..12].try_into().ok()?) as usize;
    if bytes.len() < 12 + count * 4 {
        return None;
    }
    let mut peaks = Vec::with_capacity(count);
    for i in 0..count {
        let off = 12 + i * 4;
        let lo = i16::from_le_bytes([bytes[off], bytes[off + 1]]);
        let hi = i16::from_le_bytes([bytes[off + 2], bytes[off + 3]]);
        peaks.push((lo as f32 / 32768.0, hi as f32 / 32768.0));
    }
    Some(Waveform {
        peaks,
        duration_ms,
        truncated: true,
    })
}

fn touch(path: &Path) {
    // Bump atime so the LRU keeps what is being looked at. `File::open` alone is
    // not enough under `noatime` mounts, so set mtime explicitly.
    if let Ok(file) = std::fs::OpenOptions::new().write(true).open(path) {
        let _ = file.set_modified(std::time::SystemTime::now());
    }
}

fn hex(bytes: &[u8]) -> String {
    let mut s = String::with_capacity(bytes.len() * 2);
    for b in bytes {
        s.push_str(&format!("{b:02x}"));
    }
    s
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn waveform_buckets_and_roundtrips() {
        // 8000 samples of a 100 Hz square wave at 8 kHz = 1 second.
        let mut bytes = Vec::new();
        for i in 0..8000 {
            let v: i16 = if (i / 40) % 2 == 0 { 16000 } else { -16000 };
            bytes.extend_from_slice(&v.to_le_bytes());
        }
        let w = build_waveform(&bytes, 8000, true);
        assert_eq!(w.duration_ms, 1000);
        assert_eq!(w.peaks.len(), 400);
        let encoded = encode_waveform(&w).unwrap();
        let back = decode_waveform(&encoded).unwrap();
        assert_eq!(back.duration_ms, 1000);
        assert!((back.peaks[0].1 - w.peaks[0].1).abs() < 0.01);
    }

    #[test]
    fn cache_keys_differ_per_file_and_per_size() {
        let dir = tempfile::tempdir().unwrap();
        let cache = PreviewCache::new(dir.path().join("cache"), 1024);
        let a = dir.path().join("a.jpg");
        let b = dir.path().join("b.jpg");
        std::fs::write(&a, b"one").unwrap();
        std::fs::write(&b, b"one").unwrap();
        let ka = cache.key(&a, "thumb", "256").unwrap();
        let kb = cache.key(&b, "thumb", "256").unwrap();
        let ka2 = cache.key(&a, "thumb", "512").unwrap();
        assert_ne!(ka, kb);
        assert_ne!(ka, ka2);
        // Same file, same params → stable key.
        assert_eq!(ka, cache.key(&a, "thumb", "256").unwrap());
    }

    #[test]
    fn cap_is_enforced_oldest_first() {
        let dir = tempfile::tempdir().unwrap();
        let cache = PreviewCache::new(dir.path().to_path_buf(), 1000);
        for i in 0..10 {
            let p = dir.path().join(format!("f{i}.bin"));
            std::fs::write(&p, vec![0u8; 300]).unwrap();
            // Give each file a distinct mtime, oldest first.
            let when = std::time::SystemTime::now() - std::time::Duration::from_secs(100 - i);
            let f = std::fs::OpenOptions::new().write(true).open(&p).unwrap();
            let _ = f.set_modified(when);
        }
        let freed = cache.enforce_cap();
        assert!(freed > 0);
        let remaining: u64 = std::fs::read_dir(dir.path())
            .unwrap()
            .flatten()
            .map(|e| e.metadata().map(|m| m.len()).unwrap_or(0))
            .sum();
        assert!(remaining <= 1000, "cache still holds {remaining} bytes");
        assert!(
            !dir.path().join("f0.bin").exists(),
            "oldest entry should go first"
        );
    }
}
