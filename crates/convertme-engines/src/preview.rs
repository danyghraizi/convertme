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
use convertme_core::capability::ToolKey;
use convertme_core::error::{ErrorCode, JobError};
use convertme_core::job::JobCtx;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::{Condvar, Mutex};

static PROBE_PERMITS: Mutex<usize> = Mutex::new(2);
static PROBE_WAKE: Condvar = Condvar::new();

/// Take one of the two preview slots, waiting if both are busy.
pub struct Permit;

impl Permit {
    /// The process-wide preview pool. Two slots, shared with the probes, so a
    /// superseded preview and twenty rapid ones still cannot starve a job.
    pub fn acquire() -> Permit {
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
    /// True when the recording is longer than the waveform cap and only its head is
    /// represented.
    pub truncated: bool,
}

/// Waveforms are drawn at 1 kHz: plenty for 2000 overview buckets, and a three-hour
/// recording is 10.8 MB of raw samples instead of 172 MB at 8 kHz.
const WAVEFORM_RATE: u32 = 1_000;
/// Six hours is "a few hours" with headroom; past that the head is drawn and the
/// dialog says so. The samples are still bounded, not the user's file.
const WAVEFORM_SECONDS: u64 = 6 * 3600;
/// The overview never needs more than the dialog's width, and the cache stores every
/// bucket as 4 bytes.
const WAVEFORM_BUCKETS: usize = 2000;

/// Pages rendered for the organizer are capped here: past this the grid draws a
/// numbered card, so a 900-page document still opens immediately.
const MAX_PAGE_THUMBS: usize = 100;

/// The DPI the page grid renders at. Small on purpose: it is a thumbnail.
const PAGE_THUMB_DPI: &str = "32";

/// One page of a document, as the organizer needs it.
#[derive(Debug, Clone, serde::Serialize)]
pub struct PageImage {
    pub page: u32,
    /// In PostScript points, straight from the page's MediaBox.
    pub width: u32,
    pub height: u32,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub thumb: Option<PathBuf>,
}

/// One dialog's preview requests, in order.
///
/// A slider drag asks for a new preview every few hundred milliseconds. Each request
/// cancels the one it supersedes, so the ImageMagick or ffmpeg process behind a
/// preview the user has already moved past is *killed* rather than left to finish and
/// burn a core. Sharing one `PreviewCache` already bounds the pool to two processes;
/// this bounds the work to the request that matters.
#[derive(Default)]
pub struct PreviewGuard {
    /// One slot per preview stream, keyed by what the request is *for*.
    ///
    /// A single global slot made two unrelated previews cancel each other: the crop
    /// dialog asks for a filmstrip and a waveform for the same file, the second
    /// `begin()` killed the first's process, and the canvas stayed empty. A key is the
    /// file plus the kind of preview, so only a request that supersedes the same thing
    /// cancels anything.
    current: Mutex<std::collections::HashMap<String, JobCtx>>,
}

impl PreviewGuard {
    /// Start a request for `key`, cancelling the request it supersedes.
    pub fn begin(&self, key: &str) -> JobCtx {
        let mut current = self.current.lock().unwrap();
        if let Some(previous) = current.remove(key) {
            previous.cancel();
        }
        let fresh = JobCtx::new();
        current.insert(key.to_string(), fresh.clone());
        fresh
    }

    /// Drop one stream's request without starting another (its dialog closed).
    pub fn finish(&self, key: &str) {
        let mut current = self.current.lock().unwrap();
        if let Some(previous) = current.remove(key) {
            previous.cancel();
        }
    }

    /// Drop every request, e.g. the window closed.
    pub fn finish_all(&self) {
        let mut current = self.current.lock().unwrap();
        for (_key, previous) in current.drain() {
            previous.cancel();
        }
    }
}

#[derive(Clone)]
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

    /// One preview slot, for the rendered previews that run their own encoder.
    pub fn preview_permit(&self) -> Permit {
        Permit::acquire()
    }

    /// Bytes the cache is holding, and how many entries it has.
    pub fn stats(&self) -> (u64, usize) {
        let mut bytes = 0u64;
        let mut files = 0usize;
        if let Ok(read) = std::fs::read_dir(&self.root) {
            for entry in read.flatten() {
                match entry.metadata() {
                    Ok(meta) if meta.is_dir() => {
                        let (b, f) = tree_usage(&entry.path());
                        bytes += b;
                        files += f;
                    }
                    Ok(meta) => {
                        bytes += meta.len();
                        files += 1;
                    }
                    Err(_) => {}
                }
            }
        }
        (bytes, files)
    }

    /// The cap itself, so Settings can say what it is enforcing.
    pub fn cap(&self) -> u64 {
        self.cap
    }

    /// Cache file for a source, or `None` when the source cannot be stat'ed.
    pub fn key(&self, source: &Path, kind: &str, arg: &str) -> Option<PathBuf> {
        self.file_key(
            source,
            kind,
            arg,
            if kind == "thumb" { "jpg" } else { "bin" },
        )
    }

    /// Cache path for an output that must keep its extension, so the asset protocol
    /// can serve it back (the dialog shows the encoded file itself, not a drawing).
    pub fn artifact_key(&self, source: &Path, kind: &str, arg: &str, ext: &str) -> Option<PathBuf> {
        self.file_key(source, kind, arg, ext)
    }

    fn file_key(&self, source: &Path, kind: &str, arg: &str, ext: &str) -> Option<PathBuf> {
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
        let name = format!("{kind}-{}.{ext}", hex(&digest[..16]));
        Some(self.root.join(name))
    }

    /// Copy an already-encoded artifact into the cache under the key of its request.
    /// The bytes are published by rename, so a reader never sees a partial file.
    pub fn store_artifact(
        &self,
        source: &Path,
        kind: &str,
        arg: &str,
        ext: &str,
        from: &Path,
    ) -> Result<PathBuf, JobError> {
        let path = self
            .artifact_key(source, kind, arg, ext)
            .ok_or_else(|| JobError::new(ErrorCode::Io, "cannot stat the source for a preview"))?;
        let stamp = std::time::SystemTime::now()
            .duration_since(std::time::UNIX_EPOCH)
            .map(|d| d.as_nanos())
            .unwrap_or(0);
        let tmp = path.with_extension(format!("{}-{stamp}.tmp", std::process::id()));
        std::fs::copy(from, &tmp)
            .map_err(|e| JobError::io("copying into the preview cache", &e))?;
        if let Err(e) = std::fs::rename(&tmp, &path) {
            let _ = std::fs::remove_file(&tmp);
            return Err(JobError::io("storing an encoded preview", &e));
        }
        self.enforce_cap_keeping(Some(&path));
        Ok(path)
    }

    /// Thumbnail path, generating it at `size` px on the longest edge if missing.
    pub fn thumbnail(&self, source: &Path, size: u32, ctx: &JobCtx) -> Result<PathBuf, JobError> {
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

        let magick = crate::probe::locate(ToolKey::Magick, &crate::probe::ProbeOptions::from_env())
            .ok_or_else(|| JobError::missing_tool("ImageMagick"))?;
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
            ctx,
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
    pub fn filmstrip(
        &self,
        source: &Path,
        frames: usize,
        ctx: &JobCtx,
    ) -> Result<Vec<PathBuf>, JobError> {
        let frames = frames.clamp(1, 12);
        let _permit = Permit::acquire();
        let dir = self
            .key(source, "strip", &frames.to_string())
            .ok_or_else(|| JobError::new(ErrorCode::Io, "cannot stat the video for a filmstrip"))?
            .with_extension("");
        if dir.is_dir()
            && let Some(existing) = read_frames(&dir)
            && existing.len() == frames
        {
            return Ok(existing);
        }
        std::fs::create_dir_all(&dir)
            .map_err(|e| JobError::io("creating the filmstrip cache", &e))?;

        let ffmpeg = crate::probe::locate(ToolKey::Ffmpeg, &crate::probe::ProbeOptions::from_env())
            .ok_or_else(|| JobError::missing_tool("FFmpeg"))?;
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
                .arg(&out_pattern)
                .budget(proc::Budget::Long),
            ctx,
        )?
        .ok()?;

        let produced = read_frames(&dir).ok_or_else(|| {
            JobError::new(ErrorCode::EngineFailed, "the video produced no frames")
        })?;
        self.enforce_cap();
        Ok(produced)
    }

    /// A playback sequence: images sampled at a chosen rate, not a strip.
    ///
    /// The web view must not decode the file (lesson 61), so Play walks this
    /// sequence. `fps` is what makes a 6 s clip play at 10 fps instead of the four
    /// representative frame groups a thumbnail strip can produce; `max` bounds the
    /// work and the cache for a long clip (the dialog derives both from the
    /// duration).
    pub fn preview_frames(
        &self,
        source: &Path,
        fps: u32,
        max: usize,
        ctx: &JobCtx,
    ) -> Result<Vec<PathBuf>, JobError> {
        let fps = fps.clamp(1, 30);
        let max = max.clamp(1, 300);
        let _permit = Permit::acquire();
        let dir = self
            .key(source, "frames", &format!("{fps}-{max}"))
            .ok_or_else(|| JobError::new(ErrorCode::Io, "cannot stat the video for playback"))?
            .with_extension("");
        let marker = dir.join("complete");
        if marker.is_file()
            && let Some(existing) = read_frames(&dir)
            && !existing.is_empty()
        {
            return Ok(existing);
        }
        // A directory without its marker is an interrupted run; start clean so a
        // stale tail cannot be played back after the shorter one.
        if dir.exists() {
            let _ = std::fs::remove_dir_all(&dir);
        }
        std::fs::create_dir_all(&dir)
            .map_err(|e| JobError::io("creating the playback cache", &e))?;

        let ffmpeg = crate::probe::locate(ToolKey::Ffmpeg, &crate::probe::ProbeOptions::from_env())
            .ok_or_else(|| JobError::missing_tool("FFmpeg"))?;
        let out_pattern = dir.join("frame%03d.jpg");
        proc::run(
            Cmd::new(ffmpeg)
                .arg("-y")
                .arg("-i")
                .arg(source)
                .arg("-vf")
                .arg(format!("fps={fps},scale=480:-2"))
                .arg("-frames:v")
                .arg(max.to_string())
                .arg(&out_pattern)
                .budget(proc::Budget::Long),
            ctx,
        )?
        .ok()?;

        let produced = read_frames(&dir).ok_or_else(|| {
            JobError::new(
                ErrorCode::EngineFailed,
                "the video produced no playback frames",
            )
        })?;
        let _ = std::fs::write(&marker, b"1");
        self.enforce_cap();
        Ok(produced)
    }

    /// Waveform peaks, 1 kHz mono, covering up to six hours of the recording.
    pub fn waveform(&self, source: &Path, ctx: &JobCtx) -> Result<Waveform, JobError> {
        self.waveform_filtered(source, "1k", None, ctx)
    }

    /// The two source channels, one waveform each, for the Channels dialog: the
    /// gains are applied as a row scale, so the preview shows what each channel
    /// carries before the `pan` filter mixes them.
    pub fn waveform_channels(
        &self,
        source: &Path,
        ctx: &JobCtx,
    ) -> Result<(Waveform, Waveform), JobError> {
        // A mono file has one channel; the second row then shows the same signal,
        // which is what the engine does when it duplicates it.
        let left = self.waveform_filtered(source, "1k-c0", Some("pan=mono|c0=c0"), ctx)?;
        let right = match self.waveform_filtered(source, "1k-c1", Some("pan=mono|c0=c1"), ctx) {
            Ok(right) => right,
            Err(_) => left.clone(),
        };
        Ok((left, right))
    }

    /// Peaks for a decoded stream, with an optional FFmpeg audio filter in front of
    /// the resample so both channels come from one measurement path.
    fn waveform_filtered(
        &self,
        source: &Path,
        arg: &str,
        filter: Option<&str>,
        ctx: &JobCtx,
    ) -> Result<Waveform, JobError> {
        let _permit = Permit::acquire();
        let cache = self
            .key(source, "wave", arg)
            .ok_or_else(|| JobError::new(ErrorCode::Io, "cannot stat the audio for a waveform"))?;
        if let Ok(bytes) = std::fs::read(&cache)
            && let Some(w) = decode_waveform(&bytes)
        {
            return Ok(w);
        }

        let ffmpeg = crate::probe::locate(ToolKey::Ffmpeg, &crate::probe::ProbeOptions::from_env())
            .ok_or_else(|| JobError::missing_tool("FFmpeg"))?;
        let raw = cache.with_extension("raw");
        let mut cmd = Cmd::new(ffmpeg).arg("-y").arg("-i").arg(source);
        if let Some(filter) = filter {
            cmd = cmd.arg("-af").arg(filter);
        }
        proc::run(
            cmd.arg("-t")
                .arg(WAVEFORM_SECONDS.to_string())
                .arg("-ac")
                .arg("1")
                .arg("-ar")
                .arg(WAVEFORM_RATE.to_string())
                .arg("-f")
                .arg("s16le")
                .arg(&raw)
                .budget(proc::Budget::Long),
            ctx,
        )?
        .ok()?;

        let bytes = std::fs::read(&raw).map_err(|e| JobError::io("reading the waveform", &e))?;
        let _ = std::fs::remove_file(&raw);
        // A clip that filled the cap is longer than what is drawn; a short one is
        // complete and must not claim otherwise.
        let cap_samples = (WAVEFORM_SECONDS * WAVEFORM_RATE as u64) as usize;
        let truncated = bytes.len() / 2 >= cap_samples.saturating_sub(WAVEFORM_RATE as usize);
        let waveform = build_waveform(&bytes, WAVEFORM_RATE, truncated);
        if let Ok(encoded) = encode_waveform(&waveform) {
            let _ = std::fs::write(&cache, encoded);
            self.enforce_cap();
        }
        Ok(waveform)
    }

    /// The page organizer's data: every page's size, plus a thumbnail for the first
    /// hundred. A document longer than that keeps every page editable — the grid
    /// draws a numbered card where there is no image.
    pub fn pdf_pages(&self, source: &Path, ctx: &JobCtx) -> Result<Vec<PageImage>, JobError> {
        let _permit = Permit::acquire();
        let sizes = pdf_page_sizes(source)?;
        let dir = self
            .key(source, "pdfpages", "32")
            .ok_or_else(|| JobError::new(ErrorCode::Io, "cannot stat the PDF for its pages"))?
            .with_extension("");
        let rendered = read_frames(&dir).unwrap_or_default();
        let wanted = sizes.len().min(MAX_PAGE_THUMBS);
        let thumbs: Vec<PathBuf> = if rendered.len() >= wanted && wanted > 0 {
            rendered
        } else {
            render_pdf_pages(source, &dir, wanted, ctx)?
        };

        Ok(sizes
            .into_iter()
            .enumerate()
            .map(|(index, (width, height))| PageImage {
                page: index as u32 + 1,
                width,
                height,
                thumb: thumbs.get(index).cloned(),
            })
            .collect())
    }

    /// Keep the cache under its cap, oldest-used first.
    pub fn enforce_cap(&self) -> u64 {
        self.enforce_cap_keeping(None)
    }

    /// Evict oldest-first, but never the entry being served: an artifact larger than the
    /// whole cap is still the result the preview must show.
    fn enforce_cap_keeping(&self, keep: Option<&Path>) -> u64 {
        let mut entries: Vec<(std::time::SystemTime, u64, PathBuf)> = Vec::new();
        let Ok(read) = std::fs::read_dir(&self.root) else {
            return 0;
        };
        for entry in read.flatten() {
            let Ok(meta) = entry.metadata() else {
                continue;
            };
            // `touch` bumps mtime because atime is unreliable under relatime/noatime;
            // evicting by atime therefore evicted an arbitrary entry.
            let used = meta
                .modified()
                .or_else(|_| meta.accessed())
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
            if keep.is_some_and(|k| k == path.as_path()) {
                continue;
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

/// Page sizes straight from the document, so the organizer can show a card for a
/// page it never rendered.
fn pdf_page_sizes(path: &Path) -> Result<Vec<(u32, u32)>, JobError> {
    use lopdf::Object;
    let doc = lopdf::Document::load(path)
        .map_err(|e| JobError::new(ErrorCode::EngineFailed, format!("reading the PDF: {e}")))?;

    fn resolve<'a>(doc: &'a lopdf::Document, object: &'a Object) -> &'a Object {
        let mut current = object;
        for _ in 0..8 {
            match current {
                Object::Reference(id) => match doc.get_object(*id) {
                    Ok(next) => current = next,
                    Err(_) => break,
                },
                _ => break,
            }
        }
        current
    }

    let number = |object: &Object| -> Option<f32> {
        match object {
            Object::Integer(i) => Some(*i as f32),
            Object::Real(f) => Some(*f),
            _ => None,
        }
    };

    let mut sizes = Vec::new();
    for (_, id) in doc.get_pages() {
        let page = doc
            .get_object(id)
            .ok()
            .and_then(|o| o.as_dict().ok())
            .ok_or_else(|| {
                JobError::new(ErrorCode::EngineFailed, "a PDF page is not a dictionary")
            })?;
        let media = page
            .get(b"MediaBox")
            .ok()
            .map(|o| resolve(&doc, o))
            .and_then(|o| o.as_array().ok());
        let (width, height) = match media {
            Some(box_) if box_.len() == 4 => {
                let values: Vec<f32> = box_.iter().filter_map(number).collect();
                if values.len() == 4 {
                    ((values[2] - values[0]).abs(), (values[3] - values[1]).abs())
                } else {
                    (A4_WIDTH_PT, A4_HEIGHT_PT)
                }
            }
            _ => (A4_WIDTH_PT, A4_HEIGHT_PT),
        };
        // A page turned on its side reads as landscape in the grid.
        let rotate = page
            .get(b"Rotate")
            .ok()
            .and_then(|o| number(resolve(&doc, o)))
            .unwrap_or(0.0) as i64;
        let (width, height) = if rotate.rem_euclid(180) == 90 {
            (height, width)
        } else {
            (width, height)
        };
        sizes.push((
            width.round().max(1.0) as u32,
            height.round().max(1.0) as u32,
        ));
    }
    if sizes.is_empty() {
        return Err(JobError::new(
            ErrorCode::EngineFailed,
            "the document has no pages",
        ));
    }
    Ok(sizes)
}

/// A4 in PostScript points, used when a page has no readable MediaBox.
const A4_WIDTH_PT: f32 = 595.0;
const A4_HEIGHT_PT: f32 = 842.0;

/// Image files in a cache directory, in page/frame order.
fn read_frames(dir: &Path) -> Option<Vec<PathBuf>> {
    let mut frames: Vec<(u64, PathBuf)> = std::fs::read_dir(dir)
        .ok()?
        .filter_map(|entry| entry.ok())
        .filter_map(|entry| {
            let path = entry.path();
            let extension = path.extension()?.to_string_lossy().to_ascii_lowercase();
            if !matches!(extension.as_str(), "jpg" | "jpeg" | "png") {
                return None;
            }
            let suffix: String = path
                .file_stem()?
                .to_string_lossy()
                .chars()
                .rev()
                .take_while(|c| c.is_ascii_digit())
                .collect::<Vec<_>>()
                .into_iter()
                .rev()
                .collect();
            let index = suffix.parse().unwrap_or(0);
            Some((index, path))
        })
        .collect();
    if frames.is_empty() {
        return None;
    }
    frames.sort_by_key(|(index, _)| *index);
    Some(frames.into_iter().map(|(_, path)| path).collect())
}

/// Render the first `wanted` pages at the grid's DPI.
fn render_pdf_pages(
    source: &Path,
    dir: &Path,
    wanted: usize,
    ctx: &JobCtx,
) -> Result<Vec<PathBuf>, JobError> {
    if wanted == 0 {
        return Ok(Vec::new());
    }
    std::fs::create_dir_all(dir).map_err(|e| JobError::io("creating the page cache", &e))?;
    let pdftoppm = crate::probe::locate(ToolKey::Pdftoppm, &crate::probe::ProbeOptions::from_env())
        .ok_or_else(|| JobError::missing_tool("Poppler (pdftoppm)"))?;
    let prefix = dir.join("page");
    proc::run(
        Cmd::new(pdftoppm)
            .arg("-jpeg")
            .arg("-r")
            .arg(PAGE_THUMB_DPI)
            .arg("-f")
            .arg("1")
            .arg("-l")
            .arg(wanted.to_string())
            .arg(source)
            .arg(&prefix)
            .budget(proc::Budget::Long),
        ctx,
    )?
    .ok()?;
    Ok(read_frames(dir).unwrap_or_default())
}

/// Drop preview files nobody has looked at for `max_age_days`.
///
/// Age is the file's mtime, not its atime: `touch` already bumps mtime when a preview
/// is served (atime is unreliable under relatime/noatime), so this is the same clock the
/// LRU uses. The cap stays the hard ceiling; this only removes the truly cold entries of
/// a cache whose owner has not opened the app in a while.
pub fn sweep_previews(root: std::path::PathBuf, max_age_days: u64) -> usize {
    let cutoff = std::time::SystemTime::now()
        .checked_sub(std::time::Duration::from_secs(max_age_days * 86_400))
        .unwrap_or(std::time::UNIX_EPOCH);
    let Ok(read) = std::fs::read_dir(&root) else {
        return 0;
    };
    let mut removed = 0;
    for entry in read.flatten() {
        let Ok(meta) = entry.metadata() else { continue };
        let used = meta.modified().unwrap_or(std::time::UNIX_EPOCH);
        if used >= cutoff {
            continue;
        }
        let path = entry.path();
        let result = if path.is_dir() {
            std::fs::remove_dir_all(&path)
        } else {
            std::fs::remove_file(&path)
        };
        if result.is_ok() {
            removed += 1;
        }
    }
    removed
}

fn tree_usage(dir: &Path) -> (u64, usize) {
    let mut bytes = 0u64;
    let mut files = 0usize;
    let Ok(read) = std::fs::read_dir(dir) else {
        return (0, 0);
    };
    for entry in read.flatten() {
        match entry.metadata() {
            Ok(meta) if meta.is_dir() => {
                let (b, f) = tree_usage(&entry.path());
                bytes += b;
                files += f;
            }
            Ok(meta) => {
                bytes += meta.len();
                files += 1;
            }
            Err(_) => {}
        }
    }
    (bytes, files)
}

fn build_waveform(bytes: &[u8], rate: u32, truncated: bool) -> Waveform {
    let samples: Vec<i16> = bytes
        .as_chunks::<2>()
        .0
        .iter()
        .map(|c| i16::from_le_bytes([c[0], c[1]]))
        .collect();
    let buckets = WAVEFORM_BUCKETS;
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
    let mut out = Vec::with_capacity(13 + w.peaks.len() * 4);
    out.extend_from_slice(&w.duration_ms.to_le_bytes());
    out.extend_from_slice(&(w.peaks.len() as u32).to_le_bytes());
    // The truncated flag belongs in the cache: a 6-hour file read back used to claim
    // "first 10 minutes" on every waveform, even a 14-second one.
    out.push(u8::from(w.truncated));
    for (lo, hi) in &w.peaks {
        out.extend_from_slice(&((lo * 32768.0) as i16).to_le_bytes());
        out.extend_from_slice(&((hi * 32768.0) as i16).to_le_bytes());
    }
    Ok(out)
}

fn decode_waveform(bytes: &[u8]) -> Option<Waveform> {
    if bytes.len() < 13 {
        return None;
    }
    let duration_ms = u64::from_le_bytes(bytes[0..8].try_into().ok()?);
    let count = u32::from_le_bytes(bytes[8..12].try_into().ok()?) as usize;
    let truncated = bytes[12] & 1 == 1;
    if bytes.len() < 13 + count * 4 {
        return None;
    }
    let mut peaks = Vec::with_capacity(count);
    for i in 0..count {
        let off = 13 + i * 4;
        let lo = i16::from_le_bytes([bytes[off], bytes[off + 1]]);
        let hi = i16::from_le_bytes([bytes[off + 2], bytes[off + 3]]);
        peaks.push((lo as f32 / 32768.0, hi as f32 / 32768.0));
    }
    Some(Waveform {
        peaks,
        duration_ms,
        truncated,
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
        assert_eq!(w.peaks.len(), WAVEFORM_BUCKETS);
        let encoded = encode_waveform(&w).unwrap();
        let back = decode_waveform(&encoded).unwrap();
        assert_eq!(back.duration_ms, 1000);
        assert!(back.truncated, "the flag must survive the cache");
        assert!((back.peaks[0].1 - w.peaks[0].1).abs() < 0.01);
        let complete = build_waveform(&bytes, 8000, false);
        let back = decode_waveform(&encode_waveform(&complete).unwrap()).unwrap();
        assert!(
            !back.truncated,
            "a complete waveform must not claim truncation"
        );
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
    fn artifact_key_keeps_its_extension_and_arg() {
        let dir = tempfile::tempdir().unwrap();
        let cache = PreviewCache::new(dir.path().to_path_buf(), 1 << 20);
        let src = dir.path().join("a.jpg");
        std::fs::write(&src, b"one").unwrap();
        let key = cache
            .artifact_key(&src, "compress", "jpg|quality=80", "jpg")
            .unwrap();
        assert_eq!(key.extension().and_then(|e| e.to_str()), Some("jpg"));
        assert_ne!(
            key,
            cache
                .artifact_key(&src, "compress", "jpg|quality=70", "jpg")
                .unwrap()
        );
        assert_eq!(
            key,
            cache
                .artifact_key(&src, "compress", "jpg|quality=80", "jpg")
                .unwrap()
        );
    }

    #[test]
    fn a_stored_artifact_survives_the_cap() {
        let cache_dir = tempfile::tempdir().unwrap();
        let input_dir = tempfile::tempdir().unwrap();
        let cache = PreviewCache::new(cache_dir.path().to_path_buf(), 100);
        let src = input_dir.path().join("a.jpg");
        let produced = input_dir.path().join("produced.jpg");
        std::fs::write(&src, b"source").unwrap();
        std::fs::write(&produced, vec![7u8; 300]).unwrap();
        let old = cache_dir.path().join("old.bin");
        std::fs::write(&old, vec![0u8; 300]).unwrap();
        let when = std::time::SystemTime::now() - std::time::Duration::from_secs(60);
        std::fs::OpenOptions::new()
            .write(true)
            .open(&old)
            .unwrap()
            .set_modified(when)
            .unwrap();

        let stored = cache
            .store_artifact(&src, "compress", "jpg|quality=80", "jpg", &produced)
            .unwrap();
        assert!(
            stored.exists(),
            "the artifact just stored must not be evicted"
        );
        assert!(!old.exists(), "older entries go first");
    }

    /// A slider drag asks for a preview per frame; the one the user moved past must be
    /// cancelled, or twenty encoders run for one answer.
    #[test]
    fn a_new_preview_request_cancels_the_one_before_it() {
        let guard = PreviewGuard::default();
        let first = guard.begin("filmstrip:/a.mp4");
        assert!(!first.is_cancelled());
        let second = guard.begin("filmstrip:/a.mp4");
        assert!(
            first.is_cancelled(),
            "the superseded request must be cancelled"
        );
        assert!(!second.is_cancelled());
        guard.finish("filmstrip:/a.mp4");
        assert!(
            second.is_cancelled(),
            "closing the dialog cancels the request"
        );
    }

    /// Two different previews of the same file must not cancel each other: the video
    /// crop dialog asks for a filmstrip and a waveform at the same time.
    #[test]
    fn different_preview_streams_do_not_cancel_each_other() {
        let guard = PreviewGuard::default();
        let film = guard.begin("filmstrip:/a.mp4");
        let wave = guard.begin("waveform:/a.mp4");
        assert!(
            !film.is_cancelled(),
            "an unrelated preview killed the filmstrip"
        );
        assert!(!wave.is_cancelled());
        guard.finish_all();
        assert!(film.is_cancelled() && wave.is_cancelled());
    }

    /// The age sweep removes cold files and keeps what is being used.
    #[test]
    fn the_sweep_removes_only_cold_previews() {
        let dir = tempfile::tempdir().unwrap();
        let fresh = dir.path().join("fresh.jpg");
        let cold = dir.path().join("cold.jpg");
        std::fs::write(&fresh, b"x").unwrap();
        std::fs::write(&cold, b"y").unwrap();
        let when = std::time::SystemTime::now() - std::time::Duration::from_secs(9 * 86_400);
        std::fs::OpenOptions::new()
            .write(true)
            .open(&cold)
            .unwrap()
            .set_modified(when)
            .unwrap();
        let removed = sweep_previews(dir.path().to_path_buf(), 7);
        assert_eq!(removed, 1, "only the cold file goes");
        assert!(fresh.exists(), "a preview from today stays");
        assert!(!cold.exists());
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
