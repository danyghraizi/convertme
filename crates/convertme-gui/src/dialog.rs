//! The commands a dialog calls to draw itself: media facts, a filmstrip, a waveform,
//! PDF page thumbnails, file metadata and the rendered previews.
//!
//! Every one of these is *read-only and bounded*: the source is opened for reading,
//! everything produced lands in the preview cache or the work directory, and each
//! preview has a fixed resolution. They never run on the UI thread — a dialog that
//! blocked the window while it decoded a 63 MP photo was the reason the wheel froze
//! on open — and they share the cache's two-slot probe pool (`PreviewCache`), so a
//! slider drag cannot starve a running job.
//!
//! `audio_preview` and `photo_preview` run the *real* engine, not an approximation:
//! what the dialog shows is what the job will publish (a CSS filter is not the
//! engine, and the two drifted apart within a week).

use crate::state::AppState;
use convertme_core::engine::EngineRunner;
use convertme_core::formats::{Category, Registry, ToolId};
use convertme_core::job::{Intent, Job, JobCtx, ToolParams, WorkDir};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use tauri::State;

/// Version stamp of the rendered previews. Bump it whenever a filter or an encoder
/// changes, so a dialog never shows bytes an older build produced.
const PREVIEW_VERSION: &str = "v1";

/// One rendered audio preview may not be larger than this; a cap that is checked
/// after the write is the only way to keep one long clip from taking the whole cache.
const AUDIO_PREVIEW_MAX_BYTES: u64 = 8 * 1024 * 1024;

#[derive(Serialize)]
pub struct MediaInfoOut {
    pub duration: f64,
    pub width: u32,
    pub height: u32,
    pub has_audio: bool,
    pub has_video: bool,
    pub format: String,
}

/// Duration, frame size and stream kinds. The timeline and the crop surface need
/// these before they can draw anything.
#[tauri::command]
pub async fn media_info(state: State<'_, AppState>, path: String) -> Result<MediaInfoOut, String> {
    let tools = state.tools.clone();
    let file = PathBuf::from(path);
    let facts = tauri::async_runtime::spawn_blocking(move || {
        convertme_engines::av::media_info(&tools, &file)
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| e.to_string())?;
    Ok(MediaInfoOut {
        duration: facts.duration,
        width: facts.width,
        height: facts.height,
        has_audio: facts.has_audio,
        has_video: facts.has_video,
        format: facts.format,
    })
}

/// Filmstrip frames for the trim/split/crop dialogs, at the cache's own paths.
#[tauri::command]
pub async fn filmstrip(
    state: State<'_, AppState>,
    path: String,
    frames: usize,
) -> Result<Vec<String>, String> {
    let cache = state.cache.clone();
    // A new request supersedes the one before it *for the same file and kind*: the
    // superseded encoder is killed, not merely ignored, and a waveform asked for at the
    // same time is not collaterally cancelled.
    // The key is the whole request: the crop canvas asks for one frame and the
    // timeline for twelve, and those are different previews of the same file.
    let ctx = state
        .preview_guard
        .begin(&format!("filmstrip:{path}:{frames}"));
    let file = PathBuf::from(path);
    tauri::async_runtime::spawn_blocking(move || {
        cache.filmstrip(&file, frames, &ctx).map(|frames| {
            frames
                .iter()
                .map(|p| p.to_string_lossy().into_owned())
                .collect()
        })
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| e.to_string())
}

/// The frames Play walks: images sampled at `fps`, bounded by `max`.
#[tauri::command]
pub async fn video_frames(
    state: State<'_, AppState>,
    path: String,
    fps: u32,
    max: usize,
) -> Result<Vec<String>, String> {
    let cache = state.cache.clone();
    let ctx = state
        .preview_guard
        .begin(&format!("video-frames:{path}:{fps}:{max}"));
    tauri::async_runtime::spawn_blocking(move || {
        let file = PathBuf::from(path);
        cache
            .preview_frames(&file, fps, max, &ctx)
            .map(|frames| {
                frames
                    .iter()
                    .map(|p| p.to_string_lossy().into_owned())
                    .collect()
            })
            .map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[derive(Serialize)]
pub struct WaveformOut {
    pub duration_ms: u64,
    pub truncated: bool,
    /// Per-bucket min/max, in -1.0..1.0.
    pub peaks: Vec<[f32; 2]>,
}

/// Peaks for the waveform editor. The bucket count is capped so a ten-minute
/// recording does not serialise a megabyte of JSON into the window.
#[tauri::command]
pub async fn waveform_data(
    state: State<'_, AppState>,
    path: String,
) -> Result<WaveformOut, String> {
    let cache = state.cache.clone();
    let ctx = state.preview_guard.begin(&format!("waveform:{path}"));
    let file = PathBuf::from(path);
    tauri::async_runtime::spawn_blocking(move || {
        cache.waveform(&file, &ctx).map(|w| {
            let peaks = downsample_peaks(&w.peaks, 4000);
            WaveformOut {
                duration_ms: w.duration_ms,
                truncated: w.truncated,
                peaks,
            }
        })
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| e.to_string())
}

#[derive(Serialize)]
pub struct ChannelsOut {
    pub left: WaveformOut,
    pub right: WaveformOut,
}

/// Both source channels, so the Channels dialog can show L and R separately.
#[tauri::command]
pub async fn channel_waveforms(
    state: State<'_, AppState>,
    path: String,
) -> Result<ChannelsOut, String> {
    let cache = state.cache.clone();
    let ctx = state.preview_guard.begin(&format!("channels:{path}"));
    let file = PathBuf::from(path);
    tauri::async_runtime::spawn_blocking(move || {
        cache
            .waveform_channels(&file, &ctx)
            .map(|(left, right)| ChannelsOut {
                left: WaveformOut {
                    duration_ms: left.duration_ms,
                    truncated: left.truncated,
                    peaks: downsample_peaks(&left.peaks, 4000),
                },
                right: WaveformOut {
                    duration_ms: right.duration_ms,
                    truncated: right.truncated,
                    peaks: downsample_peaks(&right.peaks, 4000),
                },
            })
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| e.to_string())
}

/// Reduce peaks to at most `limit` buckets, keeping the extremes of each group so a
/// transient still shows.
fn downsample_peaks(peaks: &[(f32, f32)], limit: usize) -> Vec<[f32; 2]> {
    if peaks.len() <= limit {
        return peaks.iter().map(|(lo, hi)| [*lo, *hi]).collect();
    }
    let group = peaks.len().div_ceil(limit);
    peaks
        .chunks(group)
        .map(|chunk| {
            let mut lo: f32 = 0.0;
            let mut hi: f32 = 0.0;
            for (min, max) in chunk {
                lo = lo.min(*min);
                hi = hi.max(*max);
            }
            [lo, hi]
        })
        .collect()
}

#[derive(Serialize)]
pub struct PageOut {
    pub page: u32,
    /// In PostScript points, from the page's own MediaBox.
    pub width: u32,
    pub height: u32,
    /// Rendered thumbnail, or `null` past the first hundred pages.
    pub thumb: Option<String>,
}

/// Every page of a PDF, with a thumbnail for the first hundred. A longer document
/// keeps every page editable: the grid draws a numbered card instead of an image.
#[tauri::command]
pub async fn pdf_pages(state: State<'_, AppState>, path: String) -> Result<Vec<PageOut>, String> {
    let cache = state.cache.clone();
    let ctx = state.preview_guard.begin(&format!("pdf-pages:{path}"));
    let file = PathBuf::from(path);
    tauri::async_runtime::spawn_blocking(move || {
        cache.pdf_pages(&file, &ctx).map(|pages| {
            pages
                .into_iter()
                .map(|page| PageOut {
                    page: page.page,
                    width: page.width,
                    height: page.height,
                    thumb: page.thumb.map(|p| p.to_string_lossy().into_owned()),
                })
                .collect()
        })
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| e.to_string())
}

#[derive(Serialize)]
pub struct LoudnessOut {
    pub input_i: f64,
    pub input_tp: f64,
    pub input_lra: f64,
}

/// What the file currently measures, so Normalize can show "Input: −23.4 LUFS"
/// beside the target the user is choosing.
#[tauri::command]
pub async fn measure_loudness(
    state: State<'_, AppState>,
    path: String,
) -> Result<LoudnessOut, String> {
    let tools = state.tools.clone();
    let file = PathBuf::from(path);
    tauri::async_runtime::spawn_blocking(move || {
        convertme_engines::av::measure_loudness(&tools, &file)
    })
    .await
    .map_err(|e| e.to_string())?
    .map(|l| LoudnessOut {
        input_i: l.input_i,
        input_tp: l.input_tp,
        input_lra: l.input_lra,
    })
    .map_err(|e| e.to_string())
}

// --------------------------------------------------------------- previews

/// The parameters of a preview request, as the dialog holds them.
fn tool_params(params: Option<serde_json::Value>) -> ToolParams {
    let mut out = ToolParams::new();
    if let Some(serde_json::Value::Object(map)) = params {
        for (key, value) in map {
            out.insert(key, value);
        }
    }
    out
}

/// Run a tool for a preview and return the artifact it produced.
///
/// This is the same dispatcher, the same engines and the same parameter validation
/// as the job the dialog will queue, so a preview cannot disagree with the result.
fn run_tool_preview(
    tools: &convertme_engines::Toolset,
    cache: &convertme_engines::preview::PreviewCache,
    source: &Path,
    category: Category,
    tool: ToolId,
    params: &ToolParams,
) -> Result<(PathBuf, WorkDir), convertme_core::error::JobError> {
    let work = WorkDir::create(
        std::env::temp_dir().join(format!("convertme-preview-{}", uuid::Uuid::new_v4())),
    )
    .map_err(|e| convertme_core::error::JobError::io("creating a preview work dir", &e))?;
    let artifact = (|| {
        let dispatcher = convertme_engines::Dispatcher::with_preview(tools.clone(), cache.clone());
        let job = Job::new(
            source.to_path_buf(),
            Intent::Tool {
                tool,
                params: params.clone(),
            },
        );
        // A tool that only makes sense in one category keeps that category's engine.
        let _ = category;
        let ctx = JobCtx::new();
        let artifacts = dispatcher.run(&job, &work, &ctx)?;
        artifacts.into_iter().next().ok_or_else(|| {
            convertme_core::error::JobError::new(
                convertme_core::error::ErrorCode::EngineFailed,
                "the preview produced nothing",
            )
        })
    })()?;
    // The caller still needs the artifact (an audio result lives here until it is
    // transcoded into the cache), so the work dir is cleaned up there. Deleting it
    // here made every audio preview read a file that no longer existed.
    Ok((artifact, work))
}

/// The key a rendered preview is cached under: source identity, tool, parameters and
/// the encoder version.
fn preview_key_arg(tool: &str, params: &ToolParams, extra: &str) -> String {
    let mut parts: Vec<String> = params
        .iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect();
    parts.sort();
    format!("{tool}|{}|{extra}|{PREVIEW_VERSION}", parts.join(","))
}

/// An audio tool's result, transcoded to MP3 so WebKit can play it, cached by request.
///
/// Blocking by nature (the engine and ffmpeg run to completion), so every caller hands
/// it to `spawn_blocking`: a command that blocks the async runtime's worker never gets
/// to send its reply, and the dialog sits on "Rendering…" forever.
fn audio_preview_blocking(
    tools: &convertme_engines::Toolset,
    cache: &convertme_engines::preview::PreviewCache,
    path: String,
    tool: String,
    params: Option<serde_json::Value>,
) -> Result<String, String> {
    let parsed = ToolId::parse(&tool).ok_or_else(|| format!("unknown tool {tool:?}"))?;
    let source = PathBuf::from(path);
    let params = tool_params(params);

    let key = cache
        .artifact_key(
            &source,
            "audio",
            &preview_key_arg(parsed.as_str(), &params, "mp3-96"),
            "mp3",
        )
        .ok_or_else(|| "cannot stat the source for a preview".to_string())?;
    if key.exists() {
        return Ok(key.to_string_lossy().into_owned());
    }

    let (produced, work) =
        run_tool_preview(tools, cache, &source, Category::Audio, parsed, &params)
            .map_err(|e| e.to_string())?;
    let transcoded =
        convertme_engines::av::transcode_preview(tools, &produced, &key, AUDIO_PREVIEW_MAX_BYTES);
    // The artifact has been read; the work dir goes before the result is returned.
    work.cleanup();
    transcoded.map_err(|e| e.to_string())?;
    cache.enforce_cap();
    Ok(key.to_string_lossy().into_owned())
}

/// Play a tool's result before committing: the bleep, the normalized or the trimmed
/// clip, rendered by the engine into the cache.
#[tauri::command]
pub async fn audio_preview(
    state: State<'_, AppState>,
    path: String,
    tool: String,
    params: Option<serde_json::Value>,
) -> Result<String, String> {
    let tools = state.tools.clone();
    let cache = state.cache.clone();
    tauri::async_runtime::spawn_blocking(move || {
        audio_preview_blocking(&tools, &cache, path, tool, params)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// Play an audio preview through the machine's audio sink.
///
/// The window never decodes audio itself: WebKitGTK's media stack is what killed the
/// process on Play, and a machine without an audio sink wedged the web process even
/// when the element was created off-DOM. A headless player (`ffplay -nodisp`, `mpv
/// --no-video`, `paplay`, `pw-play` or `aplay`) keeps the sound in this app's
/// lifetime without opening a second application. The desktop opener is only a last
/// resort on a platform with none of them.
#[tauri::command]
pub async fn play_preview(path: String) -> Result<(), String> {
    let source = PathBuf::from(path);
    if !source.is_file() {
        return Err("the preview file is gone".to_string());
    }
    tauri::async_runtime::spawn_blocking(move || {
        use convertme_engines::proc::{Budget, Cmd};
        let player = ["ffplay", "mpv", "paplay", "pw-play", "aplay"]
            .iter()
            .find_map(|name| convertme_engines::probe::which(name).map(|path| (*name, path)));
        let ctx = JobCtx::new();
        let Some((name, program)) = player else {
            #[cfg(unix)]
            {
                let cmd = Cmd::new("xdg-open").arg(&source).budget(Budget::Short);
                return convertme_engines::proc::run(cmd, &ctx)
                    .map(|_| ())
                    .map_err(|e| format!("could not open the preview: {}", e.message));
            }
            #[cfg(not(unix))]
            {
                return Err(
                    "no audio player is installed (ffplay, mpv, paplay, pw-play or aplay)"
                        .to_string(),
                );
            }
        };
        let cmd = match name {
            "ffplay" => Cmd::new(program)
                .args(["-nodisp", "-autoexit", "-loglevel", "quiet"])
                .arg(&source),
            "mpv" => Cmd::new(program)
                .args(["--no-video", "--really-quiet", "--no-terminal"])
                .arg(&source),
            _ => Cmd::new(program).arg(&source),
        }
        .budget(Budget::Long);
        convertme_engines::proc::run(cmd, &ctx)
            .map(|_| ())
            .map_err(|e| format!("could not play the preview: {}", e.message))
    })
    .await
    .map_err(|e| e.to_string())?
}

/// The bleeped audio, so the dialog can offer "Original" and "With Bleeps".
#[tauri::command]
pub async fn bleep_preview(
    state: State<'_, AppState>,
    path: String,
    params: Option<serde_json::Value>,
) -> Result<String, String> {
    let tools = state.tools.clone();
    let cache = state.cache.clone();
    tauri::async_runtime::spawn_blocking(move || {
        audio_preview_blocking(&tools, &cache, path, "bleep".to_string(), params)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// The photo with its adjustments applied, at 1024 px on the long edge.
///
/// The dialog's sliders need to show the engine's output; a CSS approximation of
/// gamma and noise is a second implementation that drifts (lesson 8).
#[tauri::command]
pub async fn photo_preview(
    state: State<'_, AppState>,
    path: String,
    params: Option<serde_json::Value>,
) -> Result<String, String> {
    let source = PathBuf::from(path);
    let params = tool_params(params);
    let cache = state.cache.clone();
    let key = cache
        .artifact_key(
            &source,
            "photo",
            &preview_key_arg("edit-photo", &params, "1024"),
            "jpg",
        )
        .ok_or_else(|| "cannot stat the source for a preview".to_string())?;
    if key.exists() {
        return Ok(key.to_string_lossy().into_owned());
    }
    let tools = state.tools.clone();
    tauri::async_runtime::spawn_blocking(move || {
        convertme_engines::image::photo_preview(&tools, &cache, &source, &params, &key, 1024)
            .map_err(|e| e.to_string())?;
        cache.enforce_cap();
        Ok::<String, String>(key.to_string_lossy().into_owned())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// The still with its redactions, at 1024 px on the long edge, drawn by the redactor.
///
/// The dialog toggles this against the plain thumbnail, so the blur (and its strength)
/// can be judged before saving: the same geometry and the same treatment as the job,
/// not a CSS approximation of a blur.
#[tauri::command]
pub async fn redact_preview(
    state: State<'_, AppState>,
    path: String,
    params: Option<serde_json::Value>,
) -> Result<String, String> {
    let source = PathBuf::from(path);
    let params = tool_params(params);
    let cache = state.cache.clone();
    let key = cache
        .artifact_key(
            &source,
            "redact",
            &preview_key_arg("redact", &params, "1024"),
            "png",
        )
        .ok_or_else(|| "cannot stat the source for a preview".to_string())?;
    if key.exists() {
        return Ok(key.to_string_lossy().into_owned());
    }
    let tools = state.tools.clone();
    tauri::async_runtime::spawn_blocking(move || {
        convertme_engines::redact::image_preview(&tools, &cache, &source, &params, &key, 1024)
            .map_err(|e| e.to_string())?;
        cache.enforce_cap();
        Ok::<String, String>(key.to_string_lossy().into_owned())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// The collage at 512 px, rendered by the real engine into the cache.
///
/// The preview is the same composer as the job, so changing the layout in the dialog
/// shows exactly the arrangement the save will produce.
#[tauri::command]
pub async fn collage_preview(
    state: State<'_, AppState>,
    paths: Vec<String>,
    params: Option<serde_json::Value>,
) -> Result<String, String> {
    let inputs: Vec<PathBuf> = paths.into_iter().map(PathBuf::from).collect();
    if inputs.is_empty() {
        return Err("a collage needs at least one image".to_string());
    }
    let raw = params
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default();
    let params = convertme_core::params::normalize(Category::Images, ToolId::Collage, raw)?;
    let cache = state.cache.clone();
    let tools = state.tools.clone();
    // The cache key covers every input, not just the first: the same lead image with a
    // different second picture is a different collage.
    let identity = inputs
        .iter()
        .map(|path| {
            std::fs::metadata(path)
                .ok()
                .map(|meta| {
                    let mtime = meta
                        .modified()
                        .ok()
                        .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                        .map(|d| d.as_nanos())
                        .unwrap_or(0);
                    format!("{}:{}:{mtime}", path.display(), meta.len())
                })
                .unwrap_or_else(|| path.display().to_string())
        })
        .collect::<Vec<_>>()
        .join("|");
    let source = inputs[0].clone();
    let key = cache
        .artifact_key(
            &source,
            "collage",
            &preview_key_arg("collage", &params, &identity),
            "png",
        )
        .ok_or_else(|| "cannot stat the source for a preview".to_string())?;
    if key.exists() {
        return Ok(key.to_string_lossy().into_owned());
    }
    tauri::async_runtime::spawn_blocking(move || {
        convertme_engines::image::collage_preview(&tools, &cache, &inputs, &params, &key, 512)
            .map_err(|e| e.to_string())?;
        cache.enforce_cap();
        Ok::<String, String>(key.to_string_lossy().into_owned())
    })
    .await
    .map_err(|e| e.to_string())?
}

/// The cell arrangement the composer will use, in output pixels.
///
/// The dialog overlays its hit areas on this: one layout implementation, shared with
/// the composer and the 512 px preview, so a drag cannot land on a different picture
/// than the one that gets cropped.
#[tauri::command]
pub async fn collage_layout(
    state: State<'_, AppState>,
    paths: Vec<String>,
    params: Option<serde_json::Value>,
) -> Result<serde_json::Value, String> {
    let inputs: Vec<PathBuf> = paths.into_iter().map(PathBuf::from).collect();
    if inputs.is_empty() {
        return Err("a collage needs at least one image".to_string());
    }
    let raw = params
        .and_then(|value| value.as_object().cloned())
        .unwrap_or_default();
    let params = convertme_core::params::normalize(Category::Images, ToolId::Collage, raw)?;
    let width = params
        .get("width")
        .and_then(|value| value.as_f64())
        .unwrap_or(2048.0)
        .round()
        .max(16.0) as u32;
    let tools = state.tools.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let layout = convertme_engines::image::collage_layout(&tools, &inputs, &params, width)
            .map_err(|e| e.to_string())?;
        serde_json::to_value(layout).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

// --------------------------------------------------------------- metadata

/// One editable or read-only row of the metadata browser.
#[derive(Serialize, Deserialize, Clone)]
pub struct MetaFieldOut {
    pub key: String,
    pub label: String,
    pub value: String,
    /// The writer can set this tag.
    pub editable: bool,
    /// The writer can delete this tag on its own, so the row offers a minus.
    ///
    /// These are separate: PNG and WEBP are written through ImageMagick, which sets a
    /// tag but can only clear whole profiles. Marking them "not editable" hid the text
    /// fields entirely, so setting a title on a PNG became impossible.
    pub removable: bool,
}

#[derive(Serialize, Deserialize, Clone)]
pub struct MetaGroupOut {
    pub name: String,
    pub fields: Vec<MetaFieldOut>,
}

/// The metadata browser's data: file facts, then every tag the writer can change.
///
/// A tag the file does not carry is still listed (with an empty value), because
/// otherwise it could never be set; a tag the writer cannot delete is marked
/// read-only, which is what hides its minus button.
#[tauri::command]
pub async fn file_metadata(
    state: State<'_, AppState>,
    path: String,
) -> Result<Vec<MetaGroupOut>, String> {
    let source = PathBuf::from(path);
    let tools = state.tools.clone();
    tauri::async_runtime::spawn_blocking(move || {
        metadata_groups(&tools, &source).map_err(|e| e.to_string())
    })
    .await
    .map_err(|e| e.to_string())?
}

fn metadata_groups(
    tools: &convertme_engines::Toolset,
    source: &Path,
) -> Result<Vec<MetaGroupOut>, convertme_core::error::JobError> {
    use convertme_core::error::{ErrorCode, JobError};

    let format = Registry::detect(source).ok_or_else(|| {
        JobError::new(
            ErrorCode::Unsupported,
            "the file's format is not registered",
        )
    })?;
    let facts = convertme_engines::av::media_info(tools, source).ok();
    let existing = convertme_engines::metadata::read(source);

    let mut groups = Vec::new();
    let mut file_fields = vec![MetaFieldOut {
        key: "Name".into(),
        label: "Name".into(),
        value: source
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_default(),
        editable: false,
        removable: false,
    }];
    if let Ok(meta) = std::fs::metadata(source) {
        file_fields.push(MetaFieldOut {
            key: "Size".into(),
            label: "Size".into(),
            value: size_label(meta.len()),
            editable: false,
            removable: false,
        });
    }
    file_fields.push(MetaFieldOut {
        key: "Format".into(),
        label: "Format".into(),
        value: format.label.to_string(),
        editable: false,
        removable: false,
    });
    if let Some(facts) = &facts {
        let dimension = match (facts.width, facts.height) {
            (0, _) | (_, 0) => None,
            (w, h) => Some(format!("{w} × {h}")),
        };
        if let Some(dimension) = dimension {
            file_fields.push(MetaFieldOut {
                key: "Dimensions".into(),
                label: "Dimensions".into(),
                value: dimension,
                editable: false,
                removable: false,
            });
        }
        if facts.duration > 0.0 {
            file_fields.push(MetaFieldOut {
                key: "Duration".into(),
                label: "Duration".into(),
                value: format_duration(facts.duration),
                editable: false,
                removable: false,
            });
        }
    }
    groups.push(MetaGroupOut {
        name: "File".into(),
        fields: file_fields,
    });

    // The group name follows the family the writer belongs to.
    let group_name = match format.category {
        Category::Images => "Image",
        Category::Audio => "Audio",
        Category::Video => "Video",
        _ => "Document",
    };
    let writable: &[&'static str] = convertme_engines::metadata::editable_tags(format.id);
    // A writer without a delete API cannot remove one tag; the row then has no minus
    // button and `remove` refuses it.
    let removable = convertme_engines::metadata::supports_remove(format.id);

    let mut fields: Vec<MetaFieldOut> = Vec::new();
    let mut seen: Vec<String> = Vec::new();
    for (key, value) in &existing {
        fields.push(MetaFieldOut {
            key: key.clone(),
            label: key.clone(),
            value: value.clone(),
            editable: writable.iter().any(|t| t.eq_ignore_ascii_case(key)),
            removable,
        });
        seen.push(key.to_ascii_lowercase());
    }
    for tag in writable {
        if seen.iter().any(|k| k == &tag.to_ascii_lowercase()) {
            continue;
        }
        fields.push(MetaFieldOut {
            key: tag.to_string(),
            label: tag.to_string(),
            value: String::new(),
            editable: true,
            removable,
        });
    }
    groups.push(MetaGroupOut {
        name: group_name.into(),
        fields,
    });
    Ok(groups)
}

fn size_label(bytes: u64) -> String {
    const KIB: f64 = 1024.0;
    let value = bytes as f64;
    if value < KIB {
        format!("{bytes} B")
    } else if value < KIB * KIB {
        format!("{:.1} KB", value / KIB)
    } else if value < KIB * KIB * KIB {
        format!("{:.1} MB", value / (KIB * KIB))
    } else {
        format!("{:.2} GB", value / (KIB * KIB * KIB))
    }
}

fn format_duration(seconds: f64) -> String {
    let total = seconds.max(0.0).round() as u64;
    let (minutes, rest) = (total / 60, total % 60);
    let (hours, minutes) = (minutes / 60, minutes % 60);
    if hours > 0 {
        format!("{hours}:{minutes:02}:{rest:02}")
    } else {
        format!("{minutes}:{rest:02}")
    }
}
