//! Redaction: detect candidate boxes (text always, faces when the model is
//! installed), and fill the boxes the user keeps.
//!
//! Detection is a separate call from the job because the UI shows the boxes for
//! confirmation first; the engine only draws what it is told to draw.

use crate::proc::{self, Budget, Cmd};
use crate::{Context, ensure_file};
use convertme_core::capability::ToolKey;
use convertme_core::error::{ErrorCode, JobError};
use convertme_core::job::{Job, JobCtx, ToolParams};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};

/// A rectangle in normalised (0..1) page coordinates, so the same box works at
/// preview size and at full resolution.
#[derive(Debug, Clone, Copy, PartialEq, Serialize, Deserialize)]
pub struct Rect {
    pub x: f64,
    pub y: f64,
    pub w: f64,
    pub h: f64,
}

impl Rect {
    fn clamped(self) -> Self {
        let x = self.x.clamp(0.0, 1.0);
        let y = self.y.clamp(0.0, 1.0);
        Self {
            x,
            y,
            w: self.w.clamp(0.0, 1.0 - x),
            h: self.h.clamp(0.0, 1.0 - y),
        }
    }

    fn pixels(self, width: u32, height: u32) -> (u32, u32, u32, u32) {
        let r = self.clamped();
        let x = (r.x * width as f64).round() as u32;
        let y = (r.y * height as f64).round() as u32;
        let w = (r.w * width as f64).round().max(1.0) as u32;
        let h = (r.h * height as f64).round().max(1.0) as u32;
        (
            x.min(width.saturating_sub(1)),
            y.min(height.saturating_sub(1)),
            w,
            h,
        )
    }
}

#[derive(Debug, Clone, Serialize)]
pub struct Detected {
    pub rect: Rect,
    /// `text` | `face`
    pub kind: &'static str,
    /// Recognised text or confidence, for the preview list.
    pub detail: String,
}

/// Whether face detection can run at all (the ONNX runtime plus the SCRFD weights).
pub fn faces_available(cx: &Context<'_>) -> bool {
    detect::model_path().is_some() && cx.tools.has(ToolKey::Magick)
}

/// Candidate boxes for a still image.
pub fn detect_image(
    path: &Path,
    cx: &Context<'_>,
    want_faces: bool,
) -> Result<Vec<Detected>, JobError> {
    let mut out = detect_text(path, cx)?;
    if want_faces {
        out.extend(detect::faces(path, cx)?);
    }
    Ok(out)
}

/// Text boxes from Tesseract's TSV output (word level, confidence >= 40).
pub fn detect_text(path: &Path, cx: &Context<'_>) -> Result<Vec<Detected>, JobError> {
    let tesseract = cx.tools.path(ToolKey::Tesseract)?;
    let (width, height) = image_size(path, cx)?;
    let langs = "eng";
    let out = proc::run(
        Cmd::new(&tesseract)
            .arg(path)
            .arg("stdout")
            .arg("-l")
            .arg(langs)
            .arg("tsv")
            .stdout_piped()
            .budget(Budget::Short),
        cx.ctx,
    )?;
    let out = out.ok()?;

    let mut boxes = Vec::new();
    for line in out.stdout.lines().skip(1) {
        let cols: Vec<&str> = line.split('\t').collect();
        if cols.len() < 12 {
            continue;
        }
        let Ok(level) = cols[0].parse::<u32>() else {
            continue;
        };
        if level != 5 {
            continue;
        }
        let (Ok(left), Ok(top), Ok(w), Ok(h)) = (
            cols[6].parse::<f64>(),
            cols[7].parse::<f64>(),
            cols[8].parse::<f64>(),
            cols[9].parse::<f64>(),
        ) else {
            continue;
        };
        let conf: f64 = cols[10].parse().unwrap_or(0.0);
        let text = cols[11].trim();
        if conf < 40.0 || text.is_empty() {
            continue;
        }
        boxes.push(Detected {
            rect: Rect {
                x: left / width as f64,
                y: top / height as f64,
                w: w / width as f64,
                h: h / height as f64,
            },
            kind: "text",
            detail: text.to_string(),
        });
    }
    Ok(boxes)
}

fn image_size(path: &Path, cx: &Context<'_>) -> Result<(u32, u32), JobError> {
    let magick = cx.tools.path(ToolKey::Magick)?;
    identify_dims(&magick, path, cx.ctx)
}

fn video_size(path: &Path, cx: &Context<'_>) -> Result<(u32, u32), JobError> {
    let ffprobe = cx.tools.path(ToolKey::Ffprobe)?;
    let out = proc::run(
        Cmd::new(&ffprobe)
            .args([
                "-v",
                "error",
                "-select_streams",
                "v:0",
                "-show_entries",
                "stream=width,height",
            ])
            .args(["-of", "csv=s=x:p=0"])
            .arg(path)
            .stdout_piped(),
        cx.ctx,
    )?;
    let out = out.ok()?;
    let text = out.stdout.trim();
    let mut parts = text.split('x');
    let w: u32 = parts
        .next()
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(0);
    let h: u32 = parts
        .next()
        .and_then(|v| v.trim().parse().ok())
        .unwrap_or(0);
    if w == 0 || h == 0 {
        return Err(JobError::new(
            ErrorCode::EngineFailed,
            "could not read the video size",
        ));
    }
    Ok((w, h))
}

/// The strength a request that does not carry one gets. A blur has to hide the region
/// even at the smallest strength, so the range starts at a visible amount rather than
/// at "do nothing".
const DEFAULT_STRENGTH: f64 = 50.0;

/// A redaction request, parsed once for the job and for the preview, so the two can
/// never disagree about what the boxes get.
#[derive(Debug, Clone)]
struct Settings {
    boxes: Vec<Rect>,
    color: String,
    /// `solid` | `blur`
    style: String,
    /// 0..100, only meaningful when the style is `blur`.
    strength: f64,
}

impl Settings {
    fn from_params(params: &serde_json::Map<String, serde_json::Value>) -> Result<Self, JobError> {
        let boxes: Vec<Rect> = params
            .get("boxes")
            .cloned()
            .map(|v| serde_json::from_value(v).unwrap_or_default())
            .unwrap_or_default();
        let color = params
            .get("color")
            .and_then(|v| v.as_str())
            .unwrap_or("#000000")
            .to_string();
        if color.trim().is_empty() {
            return Err(JobError::new(
                ErrorCode::Unsupported,
                "`color` cannot be empty",
            ));
        }
        let style = params
            .get("style")
            .and_then(|v| v.as_str())
            .unwrap_or("solid")
            .to_string();
        if style != "solid" && style != "blur" {
            return Err(JobError::new(
                ErrorCode::Unsupported,
                "`style` must be solid or blur",
            ));
        }
        Ok(Self {
            boxes,
            color,
            style,
            strength: strength_of(params)?,
        })
    }
}

/// `strength` as a percentage of the maximum blur, 0..100. A value that is absent (or
/// null) is the default; anything outside the range or not finite is rejected rather
/// than clamped, so a broken dialog shows an error instead of a blur of the wrong size.
fn strength_of(params: &serde_json::Map<String, serde_json::Value>) -> Result<f64, JobError> {
    match params.get("strength") {
        None | Some(serde_json::Value::Null) => Ok(DEFAULT_STRENGTH),
        Some(value) => match value.as_f64() {
            Some(raw) if raw.is_finite() && (0.0..=100.0).contains(&raw) => Ok(raw),
            _ => Err(JobError::new(
                ErrorCode::Unsupported,
                "`strength` must be a number from 0 to 100",
            )),
        },
    }
}

/// The Gaussian sigma for a region of `width`×`height` pixels. It grows with the box
/// so a large face and a small word are equally unreadable, and with the strength so
/// the dialog's slider is honest: at 0 the region is still hidden, at 100 it is gone.
fn image_sigma(width: u32, height: u32, strength: f64) -> f64 {
    let scaled = f64::from(width.min(height)) * (0.06 + 0.30 * strength.clamp(0.0, 100.0) / 100.0);
    scaled.max(2.0)
}

/// The `boxblur` luma radius for a region of `width`×`height` pixels. Same idea as the
/// still image, but ffmpeg's radius is a whole number of pixels with a hard limit.
fn video_luma_radius(width: u32, height: u32, strength: f64) -> u32 {
    let scaled = f64::from(width.min(height)) * (0.05 + 0.30 * strength.clamp(0.0, 100.0) / 100.0);
    scaled.round().clamp(2.0, 64.0) as u32
}

/// The treatment for every kept box, as ImageMagick arguments. Geometry is in pixels of
/// the image the arguments run on, so the job passes the source size and the preview
/// passes the thumbnail's: the same function draws both.
fn treatment_args(
    boxes: &[Rect],
    color: &str,
    style: &str,
    strength: f64,
    width: u32,
    height: u32,
) -> Vec<String> {
    let mut args = Vec::new();
    if style == "blur" {
        // Blur the marked region only, but as a cropped patch rather than through
        // `-region`: `-region WxH+x+y -blur` costs 53 s for two boxes on a 20 MP photo
        // because it blurs the whole frame behind a mask. A patch grown by the blur's
        // own reach, blurred, cropped back and composited at the box origin is the same
        // picture at a fraction of the work.
        if !boxes.is_empty() {
            // `-composite` places a patch by its `-geometry` offset, which is measured
            // from the gravity point: the default centre would move every box towards
            // the middle of the image.
            args.push("-gravity".into());
            args.push("NorthWest".into());
        }
        for b in boxes {
            let (x, y, bw, bh) = b.pixels(width, height);
            let sigma = image_sigma(bw, bh, strength);
            // A Gaussian reads across its kernel: without real pixels on every side the
            // patch's own edge bleeds into the result. Three sigma covers the visible
            // tail. Clamp to the image and remember how far the patch starts before the
            // box, so the crop-back lands on the box again.
            let margin = (3.0 * sigma).ceil() as u32;
            let ex = x.saturating_sub(margin);
            let ey = y.saturating_sub(margin);
            let ew = (x + bw + margin).min(width).saturating_sub(ex).max(1);
            let eh = (y + bh + margin).min(height).saturating_sub(ey).max(1);
            args.push("(".into());
            args.push("+clone".into());
            args.push("-crop".into());
            args.push(format!("{ew}x{eh}+{ex}+{ey}"));
            args.push("+repage".into());
            args.push("-blur".into());
            args.push(format!("0x{sigma:.2}"));
            args.push("-crop".into());
            args.push(format!("{bw}x{bh}+{}+{}", x - ex, y - ey));
            args.push("+repage".into());
            args.push(")".into());
            args.push("-geometry".into());
            args.push(format!("+{x}+{y}"));
            args.push("-composite".into());
        }
    } else {
        let mut draw = String::new();
        for b in boxes {
            let (x, y, bw, bh) = b.pixels(width, height);
            draw.push_str(&format!("rectangle {x},{y} {},{}\n", x + bw, y + bh));
        }
        args.push("-fill".into());
        args.push(color.to_string());
        args.push("-stroke".into());
        args.push("none".into());
        args.push("-draw".into());
        args.push(draw.trim_end().to_string());
    }
    args
}

/// Run a redaction job. `params.boxes` is an array of `Rect`; `params.color`
/// (default `#000000`), `params.style` (`solid` | `blur`), `params.strength`
/// (0..100, default 50, the blur radius), `params.ranges` for video (array of
/// `{start,end}`) when the boxes carry no range of their own.
pub fn run(job: &Job, cx: &Context<'_>) -> Result<Vec<PathBuf>, JobError> {
    let params = job.intent.params().cloned().unwrap_or_default();
    let settings = Settings::from_params(&params)?;
    if settings.boxes.is_empty() {
        return Err(JobError::new(ErrorCode::Unsupported, "no boxes to redact"));
    }

    let is_video = matches!(
        job.input_format.map(|f| f.0),
        Some("MP4")
            | Some("MOV")
            | Some("MKV")
            | Some("WEBM")
            | Some("AVI")
            | Some("WMV")
            | Some("GIF")
    );

    let allowed: &[&str] = if is_video {
        &["boxes", "color", "ranges", "style", "strength"]
    } else {
        &["boxes", "color", "style", "strength"]
    };
    for key in params.keys() {
        if !allowed.contains(&key.as_str()) {
            return Err(JobError::new(
                ErrorCode::Unsupported,
                format!("unknown parameter `{key}`"),
            ));
        }
    }

    if is_video {
        redact_video(job, cx, &params, settings.strength)
    } else {
        redact_image(job, cx, &settings)
    }
}

fn redact_image(
    job: &Job,
    cx: &Context<'_>,
    settings: &Settings,
) -> Result<Vec<PathBuf>, JobError> {
    let magick = cx.tools.path(ToolKey::Magick)?;
    let ext = job
        .source
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_else(|| "png".into());
    let out = cx.out(&format!("{}-redacted.{ext}", stem_of(&job.source)));
    let (w, h) = image_size(&job.source, cx)?;

    let mut cmd = Cmd::new(&magick).arg(&job.source);
    cmd = cmd.args(treatment_args(
        &settings.boxes,
        &settings.color,
        &settings.style,
        settings.strength,
        w,
        h,
    ));
    proc::run(cmd.arg(&out), cx.ctx)?.ok()?;
    ensure_file(&out, "the redactor")?;
    Ok(vec![out])
}

/// The still with its redactions, at `size` on the long edge, drawn by the same
/// geometry and the same treatment as the job.
///
/// The dialog toggles this against the plain thumbnail, so what the user inspects is
/// the engine's output rather than a CSS approximation of a blur. The thumbnail is
/// written first and then measured: the treatment runs at the thumbnail's own pixel
/// size (boxes are normalised), so a 63 MP photo is never decoded twice to be
/// previewed.
pub fn image_preview(
    tools: &crate::Toolset,
    cache: &crate::preview::PreviewCache,
    source: &Path,
    params: &ToolParams,
    out: &Path,
    size: u32,
) -> Result<(), JobError> {
    let magick = tools.path(ToolKey::Magick)?;
    let ctx = JobCtx::new();
    let settings = Settings::from_params(params)?;
    let thumb = out.with_extension("thumb.png");
    // One preview at a time: the permit is held across both magick runs, and the
    // intermediate file is unique to this render's cache key.
    let _permit = cache.preview_permit();
    let result = (|| -> Result<(), JobError> {
        proc::run(
            Cmd::new(&magick)
                .arg(source)
                .arg("-auto-orient")
                .arg("-thumbnail")
                .arg(format!("{size}x{size}>"))
                .arg(&thumb)
                .budget(Budget::Short),
            &ctx,
        )?
        .ok()?;
        let (tw, th) = identify_dims(&magick, &thumb, &ctx)?;
        let mut cmd = Cmd::new(&magick).arg(&thumb);
        // An empty box list is the plain thumbnail, which is what the dialog shows
        // before any region is marked.
        if !settings.boxes.is_empty() {
            cmd = cmd.args(treatment_args(
                &settings.boxes,
                &settings.color,
                &settings.style,
                settings.strength,
                tw,
                th,
            ));
        }
        proc::run(cmd.arg(out).budget(Budget::Short), &ctx)?.ok()?;
        ensure_file(out, "the redaction preview")?;
        Ok(())
    })();
    let _ = std::fs::remove_file(&thumb);
    result
}

/// The pixel size of an image, read from its header.
fn identify_dims(magick: &Path, path: &Path, ctx: &JobCtx) -> Result<(u32, u32), JobError> {
    let out = proc::run(
        Cmd::new(magick)
            .arg("identify")
            .arg("-format")
            .arg("%w %h")
            .arg(path)
            .stdout_piped(),
        ctx,
    )?;
    let out = out.ok()?;
    let text = out.stdout.trim().to_string();
    let mut parts = text.split_whitespace();
    let w: u32 = parts.next().and_then(|v| v.parse().ok()).unwrap_or(0);
    let h: u32 = parts.next().and_then(|v| v.parse().ok()).unwrap_or(0);
    if w == 0 || h == 0 {
        return Err(JobError::new(
            ErrorCode::EngineFailed,
            "could not read the image size",
        ));
    }
    Ok((w, h))
}

/// A box with its own time range. The dialog writes `start`/`end` on each box; the
/// CLI may instead pass one global `ranges` list, which every box inherits.
#[derive(Debug, Clone, Copy, Deserialize)]
struct TimedRect {
    x: f64,
    y: f64,
    w: f64,
    h: f64,
    #[serde(default)]
    start: Option<f64>,
    #[serde(default)]
    end: Option<f64>,
}

impl TimedRect {
    fn rect(self) -> Rect {
        Rect {
            x: self.x,
            y: self.y,
            w: self.w,
            h: self.h,
        }
    }
}

/// A time range as ffmpeg's filter `enable` expression, or an empty string for the
/// whole clip.
fn enable_expr(windows: &[(f64, f64)]) -> Option<String> {
    if windows.is_empty() {
        return None;
    }
    Some(
        windows
            .iter()
            .map(|(start, end)| format!("between(t,{start:.3},{end:.3})"))
            .collect::<Vec<_>>()
            .join("+"),
    )
}

/// `#rrggbb` as a filter colour accepted inside `drawbox=` (the `#` would start a
/// comment in some filter parsers).
fn filter_color(value: &str) -> String {
    let trimmed = value.trim();
    if let Some(hex) = trimmed.strip_prefix('#')
        && hex.len() == 6
        && hex.chars().all(|c| c.is_ascii_hexdigit())
    {
        return format!("0x{hex}");
    }
    trimmed.to_string()
}

fn redact_video(
    job: &Job,
    cx: &Context<'_>,
    params: &serde_json::Map<String, serde_json::Value>,
    strength: f64,
) -> Result<Vec<PathBuf>, JobError> {
    let ffmpeg = cx.tools.path(ToolKey::Ffmpeg)?;
    let ext = job
        .source
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_else(|| "mp4".into());
    let out = cx.out(&format!("{}-redacted.{ext}", stem_of(&job.source)));
    let (w, h) = video_size(&job.source, cx)?;

    let style = params
        .get("style")
        .and_then(|v| v.as_str())
        .unwrap_or("solid");
    if style != "solid" && style != "blur" {
        return Err(JobError::new(
            ErrorCode::Unsupported,
            "style must be solid or blur",
        ));
    }
    let color = filter_color(
        params
            .get("color")
            .and_then(|v| v.as_str())
            .unwrap_or("#000000"),
    );

    let boxes: Vec<TimedRect> = serde_json::from_value(
        params
            .get("boxes")
            .cloned()
            .unwrap_or(serde_json::Value::Array(Vec::new())),
    )
    .map_err(|_| JobError::new(ErrorCode::Unsupported, "boxes must be rectangles"))?;
    if boxes.is_empty() {
        return Err(JobError::new(ErrorCode::Unsupported, "no boxes to redact"));
    }

    // The fallback when a box has no range of its own.
    let ranges: Vec<(f64, f64)> = params
        .get("ranges")
        .and_then(|v| v.as_array())
        .map(|arr| {
            arr.iter()
                .filter_map(|v| {
                    let s = v.get("start")?.as_f64()?;
                    let e = v.get("end")?.as_f64()?;
                    Some((s, e))
                })
                .collect()
        })
        .unwrap_or_default();

    // Per box: the whole frame is split once per box; solid fills the region with
    // `drawbox` (no copy of the frame), blur crops it, blurs and overlays it back.
    let mut chain = String::new();
    let mut current = "[0:v]".to_string();
    for (i, timed) in boxes.iter().enumerate() {
        let b = timed.rect();
        let (x, y, bw, bh) = b.pixels(w, h);
        let windows: Vec<(f64, f64)> = match (timed.start, timed.end) {
            (Some(start), Some(end)) if start.is_finite() && end.is_finite() => {
                if start < 0.0 || end <= start {
                    return Err(JobError::new(
                        ErrorCode::Unsupported,
                        "each redaction range needs 0 <= start < end",
                    ));
                }
                vec![(start, end)]
            }
            (None, None) => ranges.clone(),
            _ => {
                return Err(JobError::new(
                    ErrorCode::Unsupported,
                    "a redaction range needs both start and end",
                ));
            }
        };
        let enable = enable_expr(&windows)
            .map(|expr| format!(":enable='{expr}'"))
            .unwrap_or_default();
        if style == "solid" {
            chain.push_str(&format!(
                "{current}drawbox=x={x}:y={y}:w={bw}:h={bh}:color={color}:t=fill{enable}[v{i}];"
            ));
        } else {
            // The radius follows the region and the strength: a small face still
            // disappears, and the dialog's slider changes the result the job produces.
            let radius = video_luma_radius(bw, bh, strength);
            chain.push_str(&format!(
                "{current}split[base{i}][crop{i}];\
                 [crop{i}]crop={bw}:{bh}:{x}:{y},\
                 boxblur=luma_radius={radius}:luma_power=2[blur{i}];\
                 [base{i}][blur{i}]overlay={x}:{y}{enable}[v{i}];"
            ));
        }
        current = format!("[v{i}]");
    }
    let filter = chain.to_string().trim_end_matches(';').to_string();

    let mut cmd = Cmd::new(&ffmpeg)
        .arg("-y")
        .arg("-i")
        .arg(&job.source)
        .arg("-filter_complex")
        .arg(&filter)
        .arg("-map")
        .arg(&current)
        .arg("-map")
        .arg("0:a?")
        .arg("-c:v")
        .arg("libx264")
        .arg("-preset")
        .arg("veryfast")
        .arg("-c:a")
        .arg("aac")
        .arg("-progress")
        .arg("pipe:1")
        .arg("-nostats")
        .arg(&out)
        .stdout_piped()
        .budget(Budget::Long);
    cmd = cmd.arg("-movflags").arg("+faststart");

    let progress = std::sync::Arc::new(parking_lot::Mutex::new(0.0f64));
    let total = duration_seconds(&job.source, cx).unwrap_or(0.0);
    proc::run_with_lines(cmd, cx.ctx, |line| {
        if let Some(v) = line.strip_prefix("out_time_ms=")
            && let (Ok(us), true) = (v.trim().parse::<f64>(), total > 0.0)
        {
            let frac = (us / 1_000_000.0 / total).clamp(0.0, 1.0);
            *progress.lock() = frac;
            cx.ctx.set_progress(frac);
        }
    })?
    .ok()?;
    ensure_file(&out, "the redactor")?;
    Ok(vec![out])
}

fn duration_seconds(path: &Path, cx: &Context<'_>) -> Result<f64, JobError> {
    let ffprobe = cx.tools.path(ToolKey::Ffprobe)?;
    let out = proc::run(
        Cmd::new(&ffprobe)
            .args([
                "-v",
                "error",
                "-show_entries",
                "format=duration",
                "-of",
                "csv=p=0",
            ])
            .arg(path)
            .stdout_piped(),
        cx.ctx,
    )?;
    let out = out.ok()?;
    Ok(out.stdout.trim().parse().unwrap_or(0.0))
}

fn stem_of(path: &Path) -> String {
    path.file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "file".into())
}

mod detect {
    //! Face detection needs a detector model in the data dir; when it is absent the
    //! preview simply has no face boxes and text redaction still works.
    use super::*;

    /// The face model lives in the data dir, never in the installer: it is a
    /// separate download the user opts into.
    pub const MODEL_FILE: &str = "face_detection_yunet_2023mar.onnx";

    pub fn model_path() -> Option<PathBuf> {
        let candidate = convertme_core::paths::data_dir()
            .join("models")
            .join(MODEL_FILE);
        candidate.is_file().then_some(candidate)
    }

    pub fn faces(path: &Path, cx: &Context<'_>) -> Result<Vec<Detected>, JobError> {
        let Some(model) = model_path() else {
            return Err(JobError::new(
                ErrorCode::Unsupported,
                "face detection needs a detector model in the app data dir (models/face_detection_yunet_2023mar.onnx)",
            ));
        };
        crate::face::detect(path, &model, cx)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::ffi::OsString;

    #[test]
    fn rects_are_clamped_and_scaled() {
        let r = Rect {
            x: 0.5,
            y: 0.5,
            w: 2.0,
            h: -1.0,
        }
        .clamped();
        assert_eq!(r.w, 0.5);
        assert_eq!(r.h, 0.0);
        let (x, y, w, h) = Rect {
            x: 0.25,
            y: 0.25,
            w: 0.5,
            h: 0.5,
        }
        .pixels(100, 100);
        assert_eq!((x, y, w, h), (25, 25, 50, 50));
    }

    #[test]
    fn empty_boxes_are_rejected_before_any_tool_runs() {
        let dir = tempfile::tempdir().unwrap();
        let png = dir.path().join("a.png");
        std::fs::write(&png, b"not-an-image").unwrap();
        let tools = crate::Toolset::probe_now();
        let work = convertme_core::job::WorkDir::create(dir.path().join("w")).unwrap();
        let jc = convertme_core::job::JobCtx::new();
        let cx = Context {
            tools: &tools,
            work: &work,
            ctx: &jc,
            preview: None,
        };
        let job = Job::new(
            png,
            convertme_core::job::Intent::Tool {
                tool: convertme_core::formats::ToolId::Redact,
                params: Default::default(),
            },
        );
        assert_eq!(run(&job, &cx).unwrap_err().code, ErrorCode::Unsupported);
    }

    #[test]
    fn per_box_ranges_and_colours_build_the_filter() {
        assert_eq!(
            enable_expr(&[(1.0, 2.0), (3.5, 4.25)]).unwrap(),
            "between(t,1.000,2.000)+between(t,3.500,4.250)"
        );
        assert_eq!(enable_expr(&[]), None);
        assert_eq!(filter_color("#e4572e"), "0xe4572e");
        assert_eq!(filter_color("black"), "black");
        // A key the pixel encoder accepts and the filter parser does not.
        assert_eq!(filter_color("#nothex"), "#nothex");
    }

    #[test]
    fn a_timed_box_keeps_its_range() {
        let boxed: TimedRect =
            serde_json::from_str(r#"{"x":0.1,"y":0.2,"w":0.3,"h":0.4,"start":1.0,"end":2.0}"#)
                .unwrap();
        assert_eq!((boxed.start, boxed.end), (Some(1.0), Some(2.0)));
        assert_eq!(boxed.rect().x, 0.1);
        let plain: TimedRect =
            serde_json::from_str(r#"{"x":0.1,"y":0.2,"w":0.3,"h":0.4}"#).unwrap();
        assert!(plain.start.is_none() && plain.end.is_none());
    }

    // ------------------------------------------------------- blur strength

    /// The slider is honest only if the numbers it feeds are bounded and monotonic:
    /// 0 is still a blur (never "no redaction"), 100 is the maximum, tiny regions get
    /// a usable floor, and a value outside 0..100 is an error rather than a clamp.
    #[test]
    fn strength_maps_to_a_bounded_blur() {
        let near = |actual: f64, expected: f64| (actual - expected).abs() < 1e-9;
        assert!(
            near(image_sigma(100, 100, 0.0), 6.0),
            "{}",
            image_sigma(100, 100, 0.0)
        );
        assert!(near(image_sigma(100, 100, 50.0), 21.0));
        assert!(near(image_sigma(100, 100, 100.0), 36.0));
        // A 10 px box would get 0.6 px of sigma at strength 0: the floor keeps it hidden.
        assert!(near(image_sigma(10, 10, 0.0), 2.0));
        assert!(image_sigma(400, 400, 100.0) > image_sigma(400, 400, 0.0));

        assert_eq!(video_luma_radius(100, 100, 0.0), 5);
        assert_eq!(video_luma_radius(100, 100, 50.0), 20);
        assert_eq!(video_luma_radius(100, 100, 100.0), 35);
        // ffmpeg's boxblur takes whole pixels within 2..64.
        assert_eq!(video_luma_radius(4, 4, 0.0), 2);
        assert_eq!(video_luma_radius(4000, 4000, 100.0), 64);

        let params = |value: serde_json::Value| -> serde_json::Map<String, serde_json::Value> {
            serde_json::json!({ "strength": value })
                .as_object()
                .cloned()
                .unwrap()
        };
        assert_eq!(strength_of(&Default::default()).unwrap(), DEFAULT_STRENGTH);
        assert_eq!(strength_of(&params(serde_json::Value::Null)).unwrap(), 50.0);
        assert_eq!(strength_of(&params(serde_json::json!(0))).unwrap(), 0.0);
        assert_eq!(strength_of(&params(serde_json::json!(100))).unwrap(), 100.0);
        for bad in [
            serde_json::json!(101),
            serde_json::json!(-1),
            serde_json::json!(1e300),
            serde_json::json!("blurry"),
            serde_json::json!(true),
        ] {
            assert_eq!(
                strength_of(&params(bad.clone())).unwrap_err().code,
                ErrorCode::Unsupported,
                "strength {bad} must be rejected"
            );
        }
    }

    // ----------------------------------------------------- the real engine

    struct Fixture {
        dir: tempfile::TempDir,
        tools: crate::Toolset,
        magick: PathBuf,
    }

    /// The tests below exercise the installed ImageMagick, as the job does; a machine
    /// without it skips them instead of failing for the wrong reason.
    fn fixture() -> Option<Fixture> {
        let tools = crate::Toolset::probe_now();
        let magick = tools.path(ToolKey::Magick).ok()?;
        let dir = tempfile::tempdir().ok()?;
        Some(Fixture { dir, tools, magick })
    }

    fn make_image(f: &Fixture, name: &str, args: Vec<OsString>) -> PathBuf {
        let path = f.dir.path().join(name);
        let mut cmd = Cmd::new(&f.magick);
        for arg in args {
            cmd = cmd.arg(arg);
        }
        let output = proc::run(cmd.arg(&path), &JobCtx::new()).expect("run magick");
        assert!(output.success, "magick failed: {}", output.stderr_tail);
        path
    }

    /// Run a still redaction and return the file it wrote. Each run gets its own work
    /// dir, so two strengths cannot overwrite each other's output.
    fn redact_file(f: &Fixture, source: &Path, name: &str, params: serde_json::Value) -> PathBuf {
        let work = convertme_core::job::WorkDir::create(f.dir.path().join(format!("work-{name}")))
            .expect("a work dir");
        let jc = JobCtx::new();
        let cx = Context {
            tools: &f.tools,
            work: &work,
            ctx: &jc,
            preview: None,
        };
        let job = Job::new(
            source.to_path_buf(),
            convertme_core::job::Intent::Tool {
                tool: convertme_core::formats::ToolId::Redact,
                params: params.as_object().cloned().unwrap_or_default(),
            },
        );
        run(&job, &cx)
            .expect("the redactor")
            .into_iter()
            .next()
            .expect("an output")
    }

    fn pixels(path: &Path) -> ::image::RgbImage {
        ::image::open(path).expect("decode the image").to_rgb8()
    }

    /// The pixels of a normalised box, clamped to the image.
    fn region(image: &::image::RgbImage, rect: Rect) -> Vec<(u32, u32)> {
        let (w, h) = image.dimensions();
        let (x, y, bw, bh) = rect.pixels(w, h);
        let mut out = Vec::new();
        for py in y..(y + bh).min(h) {
            for px in x..(x + bw).min(w) {
                out.push((px, py));
            }
        }
        out
    }

    /// Mean absolute difference per channel inside the box: how far a blur moved the
    /// region away from the photo.
    fn region_diff(a: &::image::RgbImage, b: &::image::RgbImage, rect: Rect) -> f64 {
        let points = region(a, rect);
        assert!(!points.is_empty());
        let total: f64 = points
            .iter()
            .map(|&(x, y)| {
                let pa = a.get_pixel(x, y);
                let pb = b.get_pixel(x, y);
                (0..3)
                    .map(|c| f64::from(pa[c].abs_diff(pb[c])))
                    .sum::<f64>()
                    / 3.0
            })
            .sum();
        total / points.len() as f64
    }

    /// Luminance variance inside the box: the smaller it is, the flatter the region.
    fn region_variance(image: &::image::RgbImage, rect: Rect) -> f64 {
        let points = region(image, rect);
        let values: Vec<f64> = points
            .iter()
            .map(|&(x, y)| {
                let p = image.get_pixel(x, y);
                0.2126 * f64::from(p[0]) + 0.7152 * f64::from(p[1]) + 0.0722 * f64::from(p[2])
            })
            .collect();
        let mean = values.iter().sum::<f64>() / values.len() as f64;
        values.iter().map(|v| (v - mean) * (v - mean)).sum::<f64>() / values.len() as f64
    }

    /// The control exists because a blur has a strength: the same box on the same
    /// photo must be flatter and further from the source at 90 than at 10, and a solid
    /// fill must ignore the number entirely.
    #[test]
    fn redaction_blur_strength_changes_the_result() {
        let Some(f) = fixture() else { return };
        let source = make_image(
            &f,
            "noise.png",
            vec![
                // Coarse noise: 16 px blocks. Pixel-level noise is destroyed by either
                // strength, so it could not tell them apart; structure at the scale
                // between the two sigmas can.
                OsString::from("-size"),
                OsString::from("20x15"),
                OsString::from("xc:gray"),
                OsString::from("+noise"),
                OsString::from("Random"),
                OsString::from("-scale"),
                OsString::from("320x240!"),
            ],
        );
        let boxed = serde_json::json!([{ "x": 0.25, "y": 0.25, "w": 0.5, "h": 0.5 }]);
        let blurred = |strength: i64, name: &str| {
            redact_file(
                &f,
                &source,
                name,
                serde_json::json!({ "boxes": boxed, "style": "blur", "strength": strength }),
            )
        };
        let low = pixels(&blurred(10, "low"));
        let high = pixels(&blurred(90, "high"));
        let original = pixels(&source);
        let rect = Rect {
            x: 0.25,
            y: 0.25,
            w: 0.5,
            h: 0.5,
        };

        assert!(
            region_diff(&original, &low, rect) > 1.0,
            "strength 10 did nothing"
        );
        assert!(
            region_diff(&original, &high, rect) > 1.0,
            "strength 90 did nothing"
        );
        // Measured inside the box, away from its edge: pasting a blurred region back
        // leaves a seam at the border, and the seam would hide the smoothing.
        let inner = Rect {
            x: rect.x + rect.w * 0.2,
            y: rect.y + rect.h * 0.2,
            w: rect.w * 0.6,
            h: rect.h * 0.6,
        };
        let low_variance = region_variance(&low, inner);
        let high_variance = region_variance(&high, inner);
        assert!(
            high_variance < low_variance,
            "strength 90 (variance {high_variance:.1}) is not smoother than 10 ({low_variance:.1})"
        );
        assert!(
            region_diff(&original, &high, rect) > region_diff(&original, &low, rect),
            "strength 90 is not further from the source than 10"
        );

        // A solid fill is a rectangle of one colour: the strength cannot show in it.
        let solid = |strength: i64, name: &str| {
            pixels(&redact_file(
                &f,
                &source,
                name,
                serde_json::json!({
                    "boxes": boxed,
                    "style": "solid",
                    "color": "#000000",
                    "strength": strength,
                }),
            ))
        };
        let solid_low = solid(10, "solid-low");
        let solid_high = solid(90, "solid-high");
        assert!(
            solid_low.as_raw() == solid_high.as_raw(),
            "solid fills must ignore the blur strength"
        );
    }

    /// The preview is the engine's own treatment: a solid box on a solid photo must
    /// show the fill colour inside the box and the source colour outside, at the
    /// requested preview size.
    #[test]
    fn redaction_preview_uses_the_same_treatment() {
        let Some(f) = fixture() else { return };
        let source = make_image(
            &f,
            "solid.png",
            vec![
                OsString::from("-size"),
                OsString::from("400x400"),
                OsString::from("xc:#2244aa"),
            ],
        );
        let out = f.dir.path().join("preview.png");
        let cache = crate::preview::PreviewCache::new(f.dir.path().join("cache"), 64 * 1024 * 1024);
        let params = serde_json::json!({
            "boxes": [{ "x": 0.25, "y": 0.25, "w": 0.5, "h": 0.5 }],
            "style": "solid",
            "color": "#00ff00",
        });
        image_preview(
            &f.tools,
            &cache,
            &source,
            params.as_object().expect("an object"),
            &out,
            200,
        )
        .expect("the preview");

        let preview = pixels(&out);
        assert_eq!(
            preview.dimensions(),
            (200, 200),
            "the long edge is the preview size"
        );
        let inside = preview.get_pixel(100, 100);
        assert!(
            inside[0] < 16 && inside[1] > 240 && inside[2] < 16,
            "the box is not the fill colour: {inside:?}"
        );
        let outside = preview.get_pixel(10, 10);
        assert!(
            outside[0] < 80 && outside[2] > 150,
            "outside the box is not the source colour: {outside:?}"
        );
    }
}
