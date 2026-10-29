//! The Tauri command surface. Every command is thin: it validates, delegates to
//! core/engines, and returns plain data. Nothing here does file I/O on the UI
//! thread that could block for long, and nothing spawns a process inline.

use crate::state::AppState;
use convertme_core::formats::{Category, Registry, ToolId};
use convertme_core::job::{Intent, Job};
use convertme_core::settings::Settings;
use convertme_core::{Store, plan};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::path::{Path, PathBuf};
use tauri::{Emitter, State};

#[derive(Serialize)]
pub struct Segment {
    pub id: String,
    pub label: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub blurb: Option<String>,
    pub disabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

#[derive(Serialize)]
pub struct WheelSet {
    pub convert: Vec<Segment>,
    pub tools: Vec<Segment>,
    pub actions: Vec<Segment>,
}

#[derive(Serialize)]
pub struct BackendInfo {
    pub kind: String,
    pub description: String,
}

#[derive(Serialize)]
pub struct About {
    pub name: String,
    pub version: String,
    pub options: usize,
    pub tools: usize,
}

#[derive(Serialize)]
pub struct MissingTool {
    pub key: String,
    pub label: String,
    pub hint: String,
}

#[derive(Serialize)]
pub struct CapStatus {
    pub missing: Vec<MissingTool>,
}

#[derive(Serialize)]
pub struct Boot {
    pub settings: Settings,
    pub backend: BackendInfo,
    pub about: About,
    pub capabilities: CapStatus,
    pub pending_paths: Vec<String>,
}

#[derive(Serialize)]
pub struct SkippedOut {
    pub path: String,
    pub message: String,
}

#[derive(Serialize)]
pub struct AnalyzeOut {
    pub count: usize,
    pub category: Option<String>,
    pub format: Option<String>,
    pub thumbs: Vec<Thumb>,
    pub skipped: Vec<SkippedOut>,
}

#[derive(Serialize)]
pub struct Thumb {
    pub path: String,
    pub thumb: String,
    /// Bytes on disk, so a tool can show what it is working with.
    pub size: u64,
}

#[derive(Serialize)]
pub struct RunOut {
    pub jobs: Vec<String>,
    pub skipped: Vec<SkippedOut>,
}

#[derive(Serialize)]
pub struct HistoryRowOut {
    pub id: String,
    pub input: String,
    pub output: Option<String>,
    pub status: String,
    pub started: Option<i64>,
    pub ended: Option<i64>,
    pub error_code: Option<String>,
    pub error_tail: Option<String>,
    pub intent: String,
}

/// Show a field only while another field has a given value. Compression, for one,
/// is either a percentage or a size, never both.
#[derive(Serialize, Clone)]
pub struct ShowIf {
    pub key: String,
    pub equals: String,
}

#[derive(Serialize, Default)]
pub struct Field {
    pub key: String,
    pub label: String,
    pub kind: String,
    pub default: Value,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub min: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub max: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub step: Option<f64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub unit: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub options: Option<Vec<String>>,
    /// Human labels for `options`, when the stored values are not presentable.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub option_labels: Option<Vec<String>>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub show_if: Option<ShowIf>,
    /// Section heading, when a tool's controls fall into groups.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub group: Option<String>,
}

impl Field {
    /// Render this field only while `key == equals`.
    fn when(mut self, key: &str, equals: &str) -> Self {
        self.show_if = Some(ShowIf {
            key: key.into(),
            equals: equals.into(),
        });
        self
    }

    /// Put this control under a section heading.
    fn group(mut self, name: &str) -> Self {
        self.group = Some(name.into());
        self
    }

    fn labelled(mut self, labels: &[&str]) -> Self {
        self.option_labels = Some(labels.iter().map(|l| (*l).to_string()).collect());
        self
    }
}

#[derive(Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum IntentIn {
    Convert {
        target: String,
    },
    Tool {
        tool: String,
        #[serde(default)]
        params: serde_json::Map<String, Value>,
    },
}

fn paths_of(input: Vec<String>) -> Vec<PathBuf> {
    input.into_iter().map(PathBuf::from).collect()
}

fn err_to_string(e: impl std::fmt::Display) -> String {
    e.to_string()
}

#[tauri::command]
pub fn boot(state: State<'_, AppState>) -> Boot {
    let settings = state.settings.lock().clone();
    let caps = state.caps.read().clone();
    let pending = state
        .pending
        .lock()
        .drain(..)
        .map(|p| p.to_string_lossy().into_owned())
        .collect();
    Boot {
        settings,
        backend: BackendInfo {
            kind: state.backend.as_str().to_string(),
            description: state.backend.describe().to_string(),
        },
        about: About {
            name: convertme_core::identity::APP_NAME.to_string(),
            version: env!("CARGO_PKG_VERSION").to_string(),
            options: convertme_core::conversion_option_count(),
            tools: convertme_core::file_tool_count(),
        },
        capabilities: CapStatus {
            missing: caps
                .missing_tools()
                .into_iter()
                .map(|k| MissingTool {
                    key: k.as_str().to_string(),
                    label: k.label().to_string(),
                    hint: k.install_hint().to_string(),
                })
                .collect(),
        },
        pending_paths: pending,
    }
}

#[tauri::command]
pub fn take_pending_paths(state: State<'_, AppState>) -> Vec<String> {
    state
        .pending
        .lock()
        .drain(..)
        .map(|p| p.to_string_lossy().into_owned())
        .collect()
}

#[tauri::command]
pub async fn analyze(state: State<'_, AppState>, paths: Vec<String>) -> Result<AnalyzeOut, String> {
    let files = paths_of(paths);
    let (category, format) = {
        // Scope the guard so nothing non-Send is held across the await below.
        let mut selection = state.selection.lock();
        selection.set(files.clone());
        (selection.category, selection.format.clone())
    };

    let mut thumbs = Vec::new();
    let mut skipped = Vec::new();
    for path in &files {
        match std::fs::metadata(path) {
            Ok(meta) if meta.is_file() => {
                if meta.len() == 0 {
                    skipped.push(SkippedOut {
                        path: path.to_string_lossy().into_owned(),
                        message: "the file is empty (0 bytes)".into(),
                    });
                }
            }
            Ok(_) => {}
            Err(e) => skipped.push(SkippedOut {
                path: path.to_string_lossy().into_owned(),
                message: format!("{e}"),
            }),
        }
    }

    // Thumbnails run off the UI thread; the first eight are enough for the strip.
    let cache_root = state.cache.root().to_path_buf();
    let cap = state.settings.lock().preview_cache_bytes;
    let cache = convertme_engines::preview::PreviewCache::new(cache_root, cap);
    let wanted: Vec<PathBuf> = files.iter().take(8).cloned().collect();
    let thumbs_result = tauri::async_runtime::spawn_blocking(move || {
        let mut out = Vec::new();
        for path in wanted {
            if let Ok(thumb) = cache.thumbnail(&path, 512) {
                out.push(Thumb {
                    path: path.to_string_lossy().into_owned(),
                    thumb: thumb.to_string_lossy().into_owned(),
                    size: std::fs::metadata(&path).map(|m| m.len()).unwrap_or(0),
                });
            }
        }
        out
    })
    .await
    .unwrap_or_default();
    thumbs.extend(thumbs_result);

    Ok(AnalyzeOut {
        count: files.len(),
        category: category.map(|c| c.key().to_string()),
        format,
        thumbs,
        skipped,
    })
}

#[tauri::command]
pub fn wheels(state: State<'_, AppState>, category: String, format: Option<String>) -> WheelSet {
    let Some(category) = Category::parse(&category) else {
        return WheelSet {
            convert: vec![],
            tools: vec![],
            actions: vec![],
        };
    };
    let caps = state.caps.read().clone();
    let source = format.as_deref().and_then(Registry::by_id).map(|f| f.id);

    let convert = Registry::wheel_targets(category, source)
        .into_iter()
        .map(|f| {
            let engine = source.and_then(|from| Registry::engine_for(from, f.id));
            let (disabled, hint) = match engine {
                Some(e) if !caps.has_engine(e) => {
                    (true, Some(format!("{} is not installed", e.label())))
                }
                _ => (false, None),
            };
            Segment {
                id: f.id.0.to_string(),
                label: f.label.to_string(),
                blurb: Some(format!("Convert to {}", f.label)),
                disabled,
                hint,
            }
        })
        .collect();

    let tools = Registry::tools_for_available(category, &caps)
        .into_iter()
        .map(|t| Segment {
            id: t.id.as_str().to_string(),
            label: t.label.to_string(),
            blurb: Some(t.blurb.to_string()),
            disabled: false,
            hint: None,
        })
        .collect();

    WheelSet {
        convert,
        tools,
        actions: action_segments(),
    }
}

fn action_segments() -> Vec<Segment> {
    let mut actions = vec![];
    if cfg!(windows) {
        actions.push(Segment {
            id: "share".into(),
            label: "Share".into(),
            blurb: Some("Hand the file to a nearby app through the system share sheet.".into()),
            disabled: false,
            hint: None,
        });
    }
    actions.push(Segment {
        id: "preview".into(),
        label: "Float".into(),
        blurb: Some("Open an always-on-top preview window.".into()),
        disabled: false,
        hint: None,
    });
    actions.push(Segment {
        id: "copy".into(),
        label: "Copy".into(),
        blurb: Some("Copy the selection to another folder.".into()),
        disabled: false,
        hint: None,
    });
    actions.push(Segment {
        id: "move".into(),
        label: "Move".into(),
        blurb: Some(
            "Move the selection to another folder. This is the only action that relocates a file."
                .into(),
        ),
        disabled: false,
        hint: None,
    });
    actions.push(Segment {
        id: "copy-path".into(),
        label: "Copy path".into(),
        blurb: Some("Put the full path on the clipboard.".into()),
        disabled: false,
        hint: None,
    });
    actions.push(Segment {
        id: "terminal".into(),
        label: "Terminal".into(),
        blurb: Some("Open a terminal in the file's folder.".into()),
        disabled: false,
        hint: None,
    });
    actions
}

#[tauri::command]
pub async fn run(
    state: State<'_, AppState>,
    paths: Vec<String>,
    category: String,
    intent: IntentIn,
) -> Result<RunOut, String> {
    let category =
        Category::parse(&category).ok_or_else(|| format!("unknown category {category:?}"))?;
    let intent = match intent {
        IntentIn::Convert { target } => {
            Intent::convert(&target).ok_or_else(|| format!("unknown format {target:?}"))?
        }
        IntentIn::Tool { tool, params } => {
            let tool = ToolId::parse(&tool).ok_or_else(|| format!("unknown tool {tool:?}"))?;
            let params = convertme_core::params::normalize(category, tool, params)?;
            Intent::Tool { tool, params }
        }
    };

    let files = paths_of(paths);
    let caps = state.caps.read().clone();
    let plan = match plan::plan_with_caps(&files, category, intent, &caps) {
        Ok(plan) => plan,
        Err(convertme_core::PlanError::Unsupported(e))
        | Err(convertme_core::PlanError::MissingTool(e)) => return Err(err_to_string(e)),
        Err(e) => return Err(err_to_string(e)),
    };

    let skipped: Vec<SkippedOut> = plan
        .skipped
        .iter()
        .map(|s| SkippedOut {
            path: s.input.to_string_lossy().into_owned(),
            message: s.error.message.clone(),
        })
        .collect();

    let mut jobs = Vec::new();
    for planned in plan.jobs {
        let job = Job::with_inputs(planned.inputs, planned.intent);
        match state.queue.submit(job) {
            Ok((id, _)) => jobs.push(id),
            Err(e) => {
                return Err(format!("could not queue {}: {e}", e.message));
            }
        }
    }
    Ok(RunOut { jobs, skipped })
}

#[tauri::command]
pub fn tool_fields(category: String, tool: String) -> Vec<Field> {
    let Some(category) = Category::parse(&category) else {
        return vec![];
    };
    let Some(tool) = ToolId::parse(&tool) else {
        return vec![];
    };
    fields_for(category, tool)
}

fn num(key: &str, label: &str, default: f64, min: f64, max: f64, step: f64, unit: &str) -> Field {
    Field {
        key: key.into(),
        label: label.into(),
        kind: "number".into(),
        default: serde_json::json!(default),
        min: Some(min),
        max: Some(max),
        step: Some(step),
        unit: Some(unit.into()),
        ..Default::default()
    }
}

fn range(key: &str, label: &str, min: f64, max: f64, step: f64, unit: &str) -> Field {
    let mut field = num(key, label, 0.0, min, max, step, unit);
    field.kind = "range".into();
    field
}

fn select(key: &str, label: &str, default: &str, options: &[&str]) -> Field {
    Field {
        key: key.into(),
        label: label.into(),
        kind: "select".into(),
        default: Value::String(default.into()),
        min: None,
        max: None,
        step: None,
        options: Some(options.iter().map(|s| s.to_string()).collect()),
        ..Default::default()
    }
}

fn text(key: &str, label: &str, default: &str, unit: Option<&str>) -> Field {
    Field {
        key: key.into(),
        label: label.into(),
        kind: "text".into(),
        default: Value::String(default.into()),
        min: None,
        max: None,
        step: None,
        unit: unit.map(str::to_string),
        ..Default::default()
    }
}

fn checkbox(key: &str, label: &str, default: bool) -> Field {
    Field {
        key: key.into(),
        label: label.into(),
        kind: "checkbox".into(),
        default: serde_json::json!(default),
        ..Default::default()
    }
}

fn fields_for(category: Category, tool: ToolId) -> Vec<Field> {
    match (category, tool) {
        (Category::Images, ToolId::Compress) => vec![
            {
                let mut field = num("quality", "Quality", 80.0, 1.0, 100.0, 1.0, "");
                field.kind = "range".into();
                field
            },
            select("scale", "Image size", "100", &["100", "75", "50", "25"]).labelled(&[
                "Keep original dimensions",
                "75 % of the dimensions",
                "50 % of the dimensions",
                "25 % of the dimensions",
            ]),
            checkbox("target", "Compress to a target file size", false),
            num(
                "target_bytes",
                "Target size",
                500_000.0,
                1000.0,
                2_000_000_000.0,
                1000.0,
                "bytes",
            )
            .when("target", "true"),
        ],
        (Category::Images, ToolId::Crop) => vec![
            Field {
                key: "boxes".into(),
                label: "Area to keep".into(),
                kind: "boxes".into(),
                default: serde_json::json!([]),
                ..Default::default()
            },
            select(
                "aspect",
                "Aspect ratio",
                "free",
                &["free", "1:1", "16:9", "4:3", "9:16", "3:4"],
            )
            .labelled(&["Free", "1:1", "16:9", "4:3", "9:16", "3:4"]),
        ],
        (Category::Images, ToolId::Annotate) => vec![
            text("text", "Text", "", None),
            text("color", "Ink colour", "#f5f5f5", None),
        ],
        (Category::Images, ToolId::AddBackground) => vec![
            text("color", "Background", "#101418", None),
            num("padding", "Padding", 64.0, 0.0, 2000.0, 1.0, "px"),
            num("radius", "Corner radius", 24.0, 0.0, 500.0, 1.0, "px"),
            select(
                "aspect",
                "Canvas",
                "1:1",
                &["1:1", "4:3", "16:9", "9:16", "source"],
            ),
        ],
        (Category::Images, ToolId::EditPhoto) => vec![
            range("exposure", "Exposure", -3.0, 3.0, 0.1, "EV").group("Light"),
            range("brightness", "Brightness", -100.0, 100.0, 1.0, "").group("Light"),
            range("contrast", "Contrast", -100.0, 100.0, 1.0, "").group("Light"),
            {
                let mut field = range("gamma", "Gamma", 0.2, 3.0, 0.01, "").group("Light");
                // 1.0 is "leave it alone"; the slider starts neutral, not at zero.
                field.default = serde_json::json!(1.0);
                field
            },
            range("temperature", "Temperature", -100.0, 100.0, 1.0, "").group("Colour"),
            range("saturation", "Saturation", -100.0, 100.0, 1.0, "").group("Colour"),
            range("hue", "Hue", -180.0, 180.0, 1.0, "°").group("Colour"),
        ],
        (Category::Images, ToolId::Redact) => {
            let mut fields = vec![
                Field {
                    key: "boxes".into(),
                    label: "Areas to fill".into(),
                    kind: "boxes".into(),
                    default: serde_json::json!([]),
                    ..Default::default()
                },
                text("color", "Fill colour", "#000000", None),
            ];
            fields.push(checkbox("detect_text", "Find text automatically", true));
            fields
        }
        (Category::Video, ToolId::Compress) | (Category::Audio, ToolId::Compress) => vec![
            num(
                "target_bytes",
                "Target size",
                20_000_000.0,
                0.0,
                20_000_000_000.0,
                1.0,
                "bytes",
            ),
            num("bitrate", "Or bitrate", 0.0, 0.0, 512.0, 1.0, "kbps"),
        ],
        (Category::Video, ToolId::Trim) | (Category::Audio, ToolId::Trim) => vec![
            num("start", "Start", 0.0, 0.0, 86_400.0, 0.1, "s"),
            num("end", "End", 0.0, 0.0, 86_400.0, 0.1, "s"),
        ],
        (Category::Video, ToolId::Crop) => {
            vec![select(
                "aspect",
                "Aspect ratio",
                "free",
                &["free", "1:1", "16:9", "4:3", "9:16", "3:2"],
            )]
        }
        (Category::Video, ToolId::Speed) => vec![num("factor", "Speed", 2.0, 0.25, 8.0, 0.05, "×")],
        (Category::Video, ToolId::Mute) => vec![],
        (Category::Video, ToolId::Snapshot) => vec![
            text("times", "Timestamps", "", Some("seconds, comma separated")),
            num("interval", "Or every", 0.0, 0.0, 3600.0, 0.1, "s"),
        ],
        (Category::Video, ToolId::Redact) => vec![
            Field {
                key: "boxes".into(),
                label: "Areas to blur".into(),
                kind: "boxes".into(),
                default: serde_json::json!([]),
                ..Default::default()
            },
            text(
                "ranges",
                "Time ranges",
                "",
                Some("start-end, comma separated"),
            ),
        ],
        (Category::Video, ToolId::Join) | (Category::Pdf, ToolId::Merge) => vec![],
        (Category::Video, ToolId::Split) => {
            vec![text(
                "sections",
                "Sections",
                "",
                Some("start-end, comma separated"),
            )]
        }
        (Category::Audio, ToolId::Normalize) => {
            vec![num(
                "lufs",
                "Target loudness",
                -16.0,
                -30.0,
                -5.0,
                0.5,
                "LUFS",
            )]
        }
        (Category::Audio, ToolId::Channels) => {
            vec![select("channels", "Channels", "1", &["1", "2"])]
        }
        (Category::Audio, ToolId::Visualize) => vec![
            text("color", "Background", "#0b0d10", None),
            select(
                "size",
                "Size",
                "1280x720",
                &["1280x720", "1920x1080", "1080x1080"],
            ),
        ],
        (Category::Audio, ToolId::Bleep) => {
            vec![text(
                "ranges",
                "Ranges",
                "",
                Some("start-end, comma separated"),
            )]
        }
        (Category::Audio, ToolId::Metadata) => {
            let tags = convertme_engines::metadata::editable_tags(
                convertme_core::formats::FormatId("MP3"),
            );
            let mut fields: Vec<Field> = tags
                .iter()
                .map(|t| text(&format!("set.{t}"), t, "", None))
                .collect();
            fields.push(checkbox("strip", "Remove all tags", false));
            fields
        }
        (Category::Pdf, ToolId::Compress) => vec![
            select(
                "preset",
                "Quality",
                "ebook",
                &["screen", "ebook", "printer", "prepress"],
            ),
            num(
                "target_bytes",
                "Target size",
                0.0,
                0.0,
                2_000_000_000.0,
                1.0,
                "bytes",
            ),
        ],
        (Category::Pdf, ToolId::Organize) => vec![
            text("order", "Page order", "", Some("1,3,2")),
            text("rotate", "Rotate", "", Some("1:90,2:180")),
        ],
        (Category::Pdf, ToolId::Metadata) => {
            let tags = convertme_engines::metadata::editable_tags(
                convertme_core::formats::FormatId("PDF"),
            );
            let mut fields: Vec<Field> = tags
                .iter()
                .map(|t| text(&format!("set.{t}"), t, "", None))
                .collect();
            fields.push(checkbox("strip", "Remove all information", false));
            fields
        }
        (Category::Pdf, ToolId::SavePages) => {
            vec![select("format", "Image format", "jpg", &["jpg", "png"])]
        }
        (Category::Pdf, ToolId::Split) => vec![],
        (Category::Pdf, ToolId::ExtractText) => vec![],
        (Category::Archives, ToolId::Extract) => vec![],
        _ => vec![],
    }
}

#[tauri::command]
pub fn history(state: State<'_, AppState>, limit: usize) -> Result<Vec<HistoryRowOut>, String> {
    let store = Store::open(&state.db_path).map_err(err_to_string)?;
    let rows = store.history(limit.clamp(1, 1000)).map_err(err_to_string)?;
    Ok(rows
        .into_iter()
        .map(|r| {
            // The column holds JSON; a person reads a phrase, so it is rendered
            // before any field is moved out of the row.
            let intent = r
                .intent_value()
                .map(|intent| intent.describe())
                .unwrap_or_else(|| r.intent.clone());
            HistoryRowOut {
                id: r.id,
                input: r.input.to_string_lossy().into_owned(),
                output: r.output.map(|p| p.to_string_lossy().into_owned()),
                status: r.status.as_str().to_string(),
                started: r.started_at,
                ended: r.ended_at,
                error_code: r.error_code.map(|c| c.as_str().to_string()),
                intent,
                error_tail: r.error_tail,
            }
        })
        .collect())
}

#[tauri::command]
pub fn cancel_job(state: State<'_, AppState>, id: String) {
    state.queue.cancel(&id);
}

#[tauri::command]
pub fn cancel_all(state: State<'_, AppState>) {
    state.queue.cancel_all();
}

#[tauri::command]
pub fn retry(state: State<'_, AppState>, id: String) -> Result<(), String> {
    let store = Store::open(&state.db_path).map_err(err_to_string)?;
    let row = store
        .row(&id)
        .map_err(err_to_string)?
        .ok_or_else(|| "unknown job".to_string())?;
    let intent = row
        .intent_value()
        .ok_or_else(|| "the stored intent is unreadable".to_string())?;
    state
        .queue
        .submit(Job::new(row.input, intent))
        .map(|_| ())
        .map_err(err_to_string)
}

/// Turn the login entry on or off. The setting and the file are kept in step, so a
/// refreshed Settings page cannot disagree with what the system will do.
#[tauri::command]
pub fn set_autostart(state: State<'_, AppState>, enabled: bool) -> Result<Settings, String> {
    crate::autostart::set(enabled)?;
    let mut settings = state.settings.lock().clone();
    settings.start_at_login = enabled;
    settings
        .save(&state.settings_path)
        .map_err(|e| e.to_string())?;
    *state.settings.lock() = settings.clone();
    Ok(settings)
}

/// What the system currently has, regardless of what the settings file says.
#[tauri::command]
pub fn autostart_status() -> bool {
    crate::autostart::is_enabled()
}

#[tauri::command]
pub fn save_settings(state: State<'_, AppState>, settings: Settings) -> Result<(), String> {
    settings.validate()?;
    if let Some(dir) = state.settings_path.parent() {
        std::fs::create_dir_all(dir).map_err(|e| e.to_string())?;
    }
    settings
        .save(&state.settings_path)
        .map_err(|e| e.to_string())?;
    *state.settings.lock() = settings;
    Ok(())
}

#[tauri::command]
pub async fn preview_thumb(
    state: State<'_, AppState>,
    path: String,
    size: u32,
) -> Result<String, String> {
    let cache_root = state.cache.root().to_path_buf();
    let cap = state.settings.lock().preview_cache_bytes;
    tauri::async_runtime::spawn_blocking(move || {
        let cache = convertme_engines::preview::PreviewCache::new(cache_root, cap);
        cache
            .thumbnail(Path::new(&path), size)
            .map(|p| p.to_string_lossy().into_owned())
            .map_err(err_to_string)
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub async fn detect_boxes(
    state: State<'_, AppState>,
    path: String,
    faces: bool,
) -> Result<Vec<serde_json::Value>, String> {
    let tools = state.tools.clone();
    tauri::async_runtime::spawn_blocking(move || {
        let work = convertme_core::job::WorkDir::create(
            std::env::temp_dir().join(format!("convertme-detect-{}", std::process::id())),
        )
        .map_err(|e| e.to_string())?;
        let jc = convertme_core::JobCtx::new();
        let cx = convertme_engines::Context {
            tools: &tools,
            work: &work,
            ctx: &jc,
        };
        let result = convertme_engines::redact::detect_image(Path::new(&path), &cx, faces);
        work.cleanup();
        let boxes = result.map_err(|e| e.message)?;
        Ok(boxes
            .into_iter()
            .map(|b| serde_json::to_value(b).unwrap_or(serde_json::Value::Null))
            .collect())
    })
    .await
    .map_err(|e| e.to_string())?
}

#[tauri::command]
pub fn drag_status(state: State<'_, AppState>) -> serde_json::Value {
    serde_json::json!({
        "kind": state.backend.as_str(),
        "description": state.backend.describe(),
        "pointer": serde_json::Value::Null,
    })
}

#[tauri::command]
pub fn wheel_show(app: tauri::AppHandle, x: Option<f64>, y: Option<f64>) -> Result<(), String> {
    crate::overlay::show_wheel(&app, x, y)
}

/// Open the wheel for the current selection from the window itself. The dev env
/// variable only exists for CI and screenshots; a person testing the wheel should
/// not have to know a command line to see it.
#[tauri::command]
pub fn preview_wheel(
    app: tauri::AppHandle,
    paths: Vec<String>,
    x: Option<f64>,
    y: Option<f64>,
) -> Result<(), String> {
    crate::overlay::show_wheel(&app, x, y)?;
    let _ = app.emit_to(crate::overlay::WHEEL_LABEL, "wheel-mode", "convert");
    if !paths.is_empty() {
        let _ = app.emit_to(crate::overlay::WHEEL_LABEL, "open-paths", paths);
    }
    Ok(())
}

#[tauri::command]
pub fn wheel_hide(app: tauri::AppHandle) -> Result<(), String> {
    crate::overlay::hide_wheel(&app)
}

#[tauri::command]
pub fn overlay_open(app: tauri::AppHandle) -> Result<(), String> {
    crate::overlay::open_overlay(&app)
}

#[tauri::command]
pub fn overlay_close(app: tauri::AppHandle) -> Result<(), String> {
    crate::overlay::close_overlay(&app)
}

#[tauri::command]
pub fn wheel_mode(
    app: tauri::AppHandle,
    mode: String,
    x: Option<f64>,
    y: Option<f64>,
) -> Result<(), String> {
    let _ = app.emit("wheel-mode", mode);
    crate::overlay::show_wheel(&app, x, y)
}

#[tauri::command]
pub fn run_action(
    app: tauri::AppHandle,
    action: String,
    paths: Vec<String>,
    dest: Option<String>,
) -> Result<(), String> {
    crate::actions::run(
        &app,
        &action,
        &paths_of(paths),
        dest.as_deref().map(Path::new),
    )
}

#[tauri::command]
pub fn open_path(path: String) -> Result<(), String> {
    crate::actions::open_with_default(Path::new(&path)).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn reveal(path: String) -> Result<(), String> {
    crate::actions::reveal(Path::new(&path)).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn copy_path(app: tauri::AppHandle, path: String) -> Result<(), String> {
    use tauri_plugin_clipboard_manager::ClipboardExt;
    app.clipboard().write_text(path).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn open_terminal(path: String) -> Result<(), String> {
    crate::actions::open_terminal(Path::new(&path)).map_err(|e| e.to_string())
}

#[tauri::command]
pub fn toggle_preview(app: tauri::AppHandle, path: String) -> Result<(), String> {
    crate::actions::toggle_preview(&app, Path::new(&path))
}

/// Used by the queue event forwarder and the drag watcher.
pub fn emit_json(app: &tauri::AppHandle, event: &str, payload: serde_json::Value) {
    let _ = app.emit(event, payload);
}
