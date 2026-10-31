//! Redaction: detect candidate boxes (text always, faces when the model is
//! installed), and fill the boxes the user keeps.
//!
//! Detection is a separate call from the job because the UI shows the boxes for
//! confirmation first; the engine only draws what it is told to draw.

use crate::proc::{self, Budget, Cmd};
use crate::{Context, ensure_file};
use convertme_core::capability::ToolKey;
use convertme_core::error::{ErrorCode, JobError};
use convertme_core::job::Job;
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
    let out = proc::run(
        Cmd::new(&magick)
            .arg("identify")
            .arg("-format")
            .arg("%w %h")
            .arg(path)
            .stdout_piped(),
        cx.ctx,
    )?;
    let out = out.ok()?;
    let text = out.stdout.trim();
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

/// Run a redaction job. `params.boxes` is an array of `Rect`; `params.color`
/// (default `#000000`), `params.ranges` for video (array of `{start,end}`).
pub fn run(job: &Job, cx: &Context<'_>) -> Result<Vec<PathBuf>, JobError> {
    let params = job.intent.params().cloned().unwrap_or_default();
    let boxes: Vec<Rect> = params
        .get("boxes")
        .cloned()
        .map(|v| serde_json::from_value(v).unwrap_or_default())
        .unwrap_or_default();
    if boxes.is_empty() {
        return Err(JobError::new(ErrorCode::Unsupported, "no boxes to redact"));
    }
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
        .unwrap_or("solid");
    if !["solid", "blur"].contains(&style) {
        return Err(JobError::new(
            ErrorCode::Unsupported,
            "`style` must be solid or blur",
        ));
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
        &["boxes", "color", "ranges", "style"]
    } else {
        &["boxes", "color", "style"]
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
        redact_video(job, cx, &boxes, &params)
    } else {
        redact_image(job, cx, &boxes, &color, style)
    }
}

fn redact_image(
    job: &Job,
    cx: &Context<'_>,
    boxes: &[Rect],
    color: &str,
    style: &str,
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
    if style == "blur" {
        // Blur the marked region only: `-region` scopes the next operation. The radius
        // grows with the region so a small box is still unreadable.
        for b in boxes {
            let (x, y, bw, bh) = b.pixels(w, h);
            let radius = (f64::from(bw.min(bh)) * 0.15).max(8.0);
            cmd = cmd
                .arg("-region")
                .arg(format!("{bw}x{bh}+{x}+{y}"))
                .arg("-blur")
                .arg(format!("0x{radius:.1}"))
                .arg("+region");
        }
    } else {
        let mut draw = String::new();
        for b in boxes {
            let (x, y, bw, bh) = b.pixels(w, h);
            draw.push_str(&format!("rectangle {x},{y} {},{}\n", x + bw, y + bh));
        }
        cmd = cmd
            .arg("-fill")
            .arg(color)
            .arg("-stroke")
            .arg("none")
            .arg("-draw")
            .arg(draw.trim_end());
    }
    proc::run(cmd.arg(&out), cx.ctx)?.ok()?;
    ensure_file(&out, "the redactor")?;
    Ok(vec![out])
}

fn redact_video(
    job: &Job,
    cx: &Context<'_>,
    boxes: &[Rect],
    params: &serde_json::Map<String, serde_json::Value>,
) -> Result<Vec<PathBuf>, JobError> {
    let ffmpeg = cx.tools.path(ToolKey::Ffmpeg)?;
    let ext = job
        .source
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_else(|| "mp4".into());
    let out = cx.out(&format!("{}-redacted.{ext}", stem_of(&job.source)));
    let (w, h) = video_size(&job.source, cx)?;

    // Time ranges the boxes apply to; default: the whole clip.
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

    // Per box: split the frame, crop the region, blur it, overlay it back for the
    // requested time ranges. Chained so several boxes compose.
    let mut chain = String::new();
    let mut current = "[0:v]".to_string();
    for (i, b) in boxes.iter().enumerate() {
        let (x, y, bw, bh) = b.pixels(w, h);
        let enable = if ranges.is_empty() {
            String::new()
        } else {
            let expr = ranges
                .iter()
                .map(|(s, e)| format!("between(t,{s},{e})"))
                .collect::<Vec<_>>()
                .join("+");
            format!(":enable='{expr}'")
        };
        chain.push_str(&format!(
            "{current}split[base{i}][crop{i}];[crop{i}]crop={bw}:{bh}:{x}:{y},boxblur=luma_radius=24:luma_power=2[blur{i}];[base{i}][blur{i}]overlay={x}:{y}{enable}[v{i}];"
        ));
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
}
