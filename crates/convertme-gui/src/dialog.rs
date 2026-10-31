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
    let file = PathBuf::from(path);
    tauri::async_runtime::spawn_blocking(move || {
        cache
            .filmstrip(&file, frames)
            .map(|frames| frames.iter().map(|p| p.to_string_lossy().into_owned()).collect())
    })
    .await
    .map_err(|e| e.to_string())?
    .map_err(|e| e.to_string())
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
pub async fn waveform_data(state: State<'_, AppState>, path: String) -> Result<WaveformOut, String> {
    let cache = state.cache.clone();
    let file = PathBuf::from(path);
    tauri::async_runtime::spawn_blocking(move || {
        cache.waveform(&file).map(|w| {
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
    let file = PathBuf::from(path);
    tauri::async_runtime::spawn_blocking(move || {
        cache.waveform_channels(&file).map(|(left, right)| ChannelsOut {
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
    let file = PathBuf::from(path);
    tauri::async_runtime::spawn_blocking(move || {
        cache.pdf_pages(&file).map(|pages| {
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
    state: &AppState,
    source: &Path,
    category: Category,
    tool: ToolId,
    params: &ToolParams,
) -> Result<PathBuf, convertme_core::error::JobError> {
    let work = WorkDir::create(std::env::temp_dir().join(format!(
        "convertme-preview-{}",
        uuid::Uuid::new_v4()
    )))
    .map_err(|e| convertme_core::error::JobError::io("creating a preview work dir", &e))?;
    let result = (|| {
        let dispatcher = convertme_engines::Dispatcher::with_preview(
            state.tools.clone(),
            state.cache.clone(),
        );
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
        artifacts
            .into_iter()
            .next()
            .ok_or_else(|| {
                convertme_core::error::JobError::new(
                    convertme_core::error::ErrorCode::EngineFailed,
                    "the preview produced nothing",
                )
            })
    })();
    // The rendered artifact is copied into the cache below; the work dir goes now.
    work.cleanup();
    result
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
async fn audio_preview_blocking(
    state: &AppState,
    path: String,
    tool: String,
    params: Option<serde_json::Value>,
) -> Result<String, String> {
    let parsed = ToolId::parse(&tool).ok_or_else(|| format!("unknown tool {tool:?}"))?;
    let source = PathBuf::from(path);
    let params = tool_params(params);
    let cache = state.cache.clone();

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

    let produced = run_tool_preview(state, &source, Category::Audio, parsed, &params)
        .map_err(|e| e.to_string())?;
    convertme_engines::av::transcode_preview(&state.tools, &produced, &key, AUDIO_PREVIEW_MAX_BYTES)
        .and_then(|_| {
            state.cache.enforce_cap();
            Ok(())
        })
        .map_err(|e| e.to_string())?;
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
    audio_preview_blocking(&state, path, tool, params).await
}

/// The bleeped audio, so the dialog can offer "Original" and "With Bleeps".
#[tauri::command]
pub async fn bleep_preview(
    state: State<'_, AppState>,
    path: String,
    params: Option<serde_json::Value>,
) -> Result<String, String> {
    audio_preview_blocking(&state, path, "bleep".to_string(), params).await
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

// --------------------------------------------------------------- metadata

/// One editable or read-only row of the metadata browser.
#[derive(Serialize, Deserialize, Clone)]
pub struct MetaFieldOut {
    pub key: String,
    pub label: String,
    pub value: String,
    pub editable: bool,
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
        JobError::new(ErrorCode::Unsupported, "the file's format is not registered")
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
    }];
    if let Ok(meta) = std::fs::metadata(source) {
        file_fields.push(MetaFieldOut {
            key: "Size".into(),
            label: "Size".into(),
            value: size_label(meta.len()),
            editable: false,
        });
    }
    file_fields.push(MetaFieldOut {
        key: "Format".into(),
        label: "Format".into(),
        value: format.label.to_string(),
        editable: false,
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
            });
        }
        if facts.duration > 0.0 {
            file_fields.push(MetaFieldOut {
                key: "Duration".into(),
                label: "Duration".into(),
                value: format_duration(facts.duration),
                editable: false,
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
    let writable: Vec<&'static str> = convertme_engines::metadata::editable_tags(format.id)
        .iter()
        .copied()
        .collect();
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
            editable: writable.iter().any(|t| t.eq_ignore_ascii_case(key)) && removable,
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
            editable: removable,
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
