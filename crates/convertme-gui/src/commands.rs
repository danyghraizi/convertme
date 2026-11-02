//! The Tauri command surface. Every command is thin: it validates, delegates to
//! core/engines, and returns plain data. Nothing here does file I/O on the UI
//! thread that could block for long, and nothing spawns a process inline.

use crate::state::AppState;
use convertme_core::formats::{Category, Registry, ToolId};
use convertme_core::job::{Intent, Job, JobCtx};
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
    /// Dialog copy from the tool table: the sheet/rail heading, the primary button,
    /// the rail's check lines, and an eyebrow that replaces the category label.
    /// Convert and action segments carry none of it.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub title: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub action: Option<String>,
    #[serde(skip_serializing_if = "Vec::is_empty")]
    pub benefits: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub eyebrow: Option<String>,
    pub disabled: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub hint: Option<String>,
}

impl Segment {
    /// A segment with no dialog copy: a convert target, a file action, or a tool
    /// whose dialog keeps the plain label and the default button.
    fn plain(id: &str, label: &str, blurb: Option<&str>) -> Segment {
        Segment {
            id: id.to_string(),
            label: label.to_string(),
            blurb: blurb.map(str::to_string),
            title: None,
            action: None,
            benefits: Vec::new(),
            eyebrow: None,
            disabled: false,
            hint: None,
        }
    }
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
    /// One line of help under the control, when the value needs explaining.
    #[serde(skip_serializing_if = "Option::is_none")]
    pub help: Option<String>,
    /// A control that drives the form (a mode switch) rather than a job parameter.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub control: bool,
    /// Owned by a dedicated editor (the crop surface, the page grid, a timeline):
    /// its value is still submitted, but no generic control is drawn for it.
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub hidden: bool,
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

    /// Drives other fields; never sent to the backend.
    fn control(mut self) -> Self {
        self.control = true;
        self
    }

    /// Put this control under a section heading.
    fn group(mut self, name: &str) -> Self {
        self.group = Some(name.into());
        self
    }

    /// One line of help under the control.
    fn help(mut self, text: &str) -> Self {
        self.help = Some(text.into());
        self
    }

    /// The value is submitted, but a dedicated editor draws it.
    fn hidden(mut self) -> Self {
        self.hidden = true;
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

/// The cover of a PDF: its first rendered page.
///
/// ImageMagick reads a PDF by shelling out to Ghostscript and, without that policy,
/// returns a black rectangle. The page cache already renders pages the way the
/// organizer sees them, so it is the only correct source for a document's still.
fn pdf_cover(
    cache: &convertme_engines::preview::PreviewCache,
    source: &Path,
    ctx: &JobCtx,
) -> Result<PathBuf, String> {
    cache
        .pdf_pages(source, ctx)
        .map_err(err_to_string)?
        .into_iter()
        .next()
        .and_then(|page| page.thumb)
        .ok_or_else(|| "the document produced no page thumbnail".to_string())
}

/// A still for a strip card: a PDF's cover, otherwise the format's thumbnail. An
/// error here becomes a placeholder card, not a missing file.
fn still_for(
    cache: &convertme_engines::preview::PreviewCache,
    source: &Path,
    size: u32,
    ctx: &JobCtx,
) -> Result<PathBuf, String> {
    match Registry::detect(source).map(|format| format.category) {
        Some(Category::Pdf) => pdf_cover(cache, source, ctx),
        // A video has no still for ImageMagick: it decodes the file by walking it,
        // which took 28 s per clip here and failed. The filmstrip's first frame is
        // the decoder that does know how to seek, and it answers in milliseconds.
        Some(Category::Video) => {
            let mut frames = cache.filmstrip(source, 1, ctx).map_err(err_to_string)?;
            frames
                .pop()
                .ok_or_else(|| "the video produced no preview frame".to_string())
        }
        _ => cache.thumbnail(source, size, ctx).map_err(err_to_string),
    }
}

#[tauri::command]
pub fn boot(window: tauri::WebviewWindow, state: State<'_, AppState>) -> Boot {
    let settings = state.settings.lock().clone();
    let caps = state.caps.read().clone();
    // The wheel window is prewarmed at startup and boots too. Draining the pending
    // paths there lost every "Open with" launch; only the main window consumes them.
    let pending: Vec<String> = if window.label() == "main" {
        state
            .pending
            .lock()
            .drain(..)
            .map(|p| p.to_string_lossy().into_owned())
            .collect()
    } else {
        Vec::new()
    };
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

    // Thumbnails run off the UI thread. Every file that exists keeps a card, in input
    // order: a preview that fails (an empty or unsupported file) yields an empty path
    // and the strip draws the extension instead. Dropping it would hide a file from
    // the selection the user made; `skipped` still explains the ones that will fail.
    let readable: Vec<(PathBuf, u64)> = files
        .iter()
        .filter_map(|path| {
            let meta = std::fs::metadata(path).ok()?;
            if !meta.is_file() {
                return None;
            }
            Some((path.clone(), meta.len()))
        })
        .collect();
    let cache_root = state.cache.root().to_path_buf();
    let cap = state.settings.lock().preview_cache_bytes;
    let cache = convertme_engines::preview::PreviewCache::new(cache_root, cap);
    let ctx = state.preview_guard.begin("strip");
    let thumbs_result = tauri::async_runtime::spawn_blocking(move || {
        readable
            .into_iter()
            .map(|(path, size)| Thumb {
                thumb: still_for(&cache, &path, 512, &ctx)
                    .map(|thumb| thumb.to_string_lossy().into_owned())
                    .unwrap_or_default(),
                path: path.to_string_lossy().into_owned(),
                size,
            })
            .collect::<Vec<_>>()
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
    WheelSet {
        convert: convert_segments(category, source, &caps),
        tools: tool_segments(category, &caps),
        actions: action_segments(),
    }
}

/// The convert wheel: one segment per writable target, disabled (with the reason)
/// when its engine is missing.
fn convert_segments(
    category: Category,
    source: Option<convertme_core::formats::FormatId>,
    caps: &convertme_core::capability::Capabilities,
) -> Vec<Segment> {
    Registry::wheel_targets(category, source)
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
                disabled,
                hint,
                ..Segment::plain(f.id.0, f.label, Some(&format!("Convert to {}", f.label)))
            }
        })
        .collect()
}

/// The tool wheel. The dialog's copy travels with the segment, so a category whose
/// wheel came from this payload never renders a rail with no words in it.
fn tool_segments(
    category: Category,
    caps: &convertme_core::capability::Capabilities,
) -> Vec<Segment> {
    Registry::tools_for_available(category, caps)
        .into_iter()
        .map(|t| Segment {
            title: Some(t.title.to_string()),
            action: Some(t.action.to_string()),
            benefits: t.benefits.iter().map(|b| (*b).to_string()).collect(),
            eyebrow: t.eyebrow.map(str::to_string),
            ..Segment::plain(t.id.as_str(), t.label, Some(t.blurb))
        })
        .collect()
}

fn action_segments() -> Vec<Segment> {
    let mut actions = vec![];
    if cfg!(windows) {
        actions.push(Segment::plain(
            "share",
            "Share",
            Some("Hand the file to a nearby app through the system share sheet."),
        ));
    }
    actions.push(Segment::plain(
        "preview",
        "Float",
        Some("Open an always-on-top preview window."),
    ));
    actions.push(Segment::plain(
        "copy",
        "Copy",
        Some("Copy the selection to another folder."),
    ));
    actions.push(Segment::plain(
        "move",
        "Move",
        Some(
            "Move the selection to another folder. This is the only action that relocates a file.",
        ),
    ));
    actions.push(Segment::plain(
        "copy-path",
        "Copy path",
        Some("Put the full path on the clipboard."),
    ));
    actions.push(Segment::plain(
        "terminal",
        "Terminal",
        Some("Open a terminal in the file's folder."),
    ));
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
            // What the dialog actually submitted, on one line: the one record that
            // answers "did the control reach the engine?" without a debugger.
            logln!(
                "queued {} in {}: {}",
                tool,
                category,
                serde_json::Value::Object(params.clone())
            );
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

/// Pixel size of a picture, so the composition preview can show real dimensions instead
/// of the size of the thumbnail it draws.
#[tauri::command]
pub fn image_size(path: String) -> Option<(u32, u32)> {
    convertme_engines::image_dimensions(std::path::Path::new(&path))
}

#[derive(Serialize)]
pub struct CompressPreviewOut {
    pub path: String,
    pub bytes: u64,
    pub source_bytes: u64,
    pub width: u32,
    pub height: u32,
}

/// Encode the file once at the dialog's settings, so the size and the picture it shows
/// are the real result. The artifact lands in the shared cache, and the job with the
/// identical request copies it instead of encoding again.
#[tauri::command]
pub async fn compress_preview(
    state: State<'_, AppState>,
    path: String,
    params: Value,
) -> Result<CompressPreviewOut, String> {
    let tools = state.tools.clone();
    let cache_root = state.cache.root().to_path_buf();
    let cap = state.settings.lock().preview_cache_bytes;
    tauri::async_runtime::spawn_blocking(move || {
        preview_compress_blocking(&tools, cache_root, cap, Path::new(&path), params)
    })
    .await
    .map_err(|e| e.to_string())?
}

/// The body of [`compress_preview`], without Tauri, so the whole path (normalize →
/// encode → cache → reported numbers) can be exercised with a real file.
pub(crate) fn preview_compress_blocking(
    tools: &convertme_engines::Toolset,
    cache_root: PathBuf,
    cap: u64,
    path: &Path,
    params: Value,
) -> Result<CompressPreviewOut, String> {
    let params = params.as_object().cloned().unwrap_or_default();
    let params = convertme_core::params::normalize(Category::Images, ToolId::Compress, params)?;
    let cache = convertme_engines::preview::PreviewCache::new(cache_root, cap);
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    // Named like the queue's work dirs (`convertme-<pid>-…`) so a crashed preview is
    // swept: the sweeper reads the pid and skips names it cannot parse.
    let work = convertme_core::job::WorkDir::create(
        std::env::temp_dir().join(format!("convertme-{}-p{stamp}", std::process::id())),
    )
    .map_err(|e| e.to_string())?;
    let jc = convertme_core::JobCtx::new();
    let cx = convertme_engines::Context {
        tools,
        work: &work,
        ctx: &jc,
        preview: Some(&cache),
    };
    let job = Job::new(
        path.to_path_buf(),
        Intent::Tool {
            tool: ToolId::Compress,
            params,
        },
    );
    let result = convertme_engines::image::preview_compress(&job, &cx);
    work.cleanup();
    let preview = result.map_err(|e| e.message)?;
    Ok(CompressPreviewOut {
        path: preview.path.to_string_lossy().into_owned(),
        bytes: preview.bytes,
        source_bytes: preview.source_bytes,
        width: preview.width,
        height: preview.height,
    })
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

/// A slider that starts somewhere other than the minimum.
fn ranged(
    key: &str,
    label: &str,
    default: f64,
    min: f64,
    max: f64,
    step: f64,
    unit: &str,
) -> Field {
    let mut field = num(key, label, default, min, max, step, unit);
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

/// The metadata browser: one structured field over the writer's `set`/`remove`/`strip`
/// keys. The per-tag text fields it replaces offered tags the writer could not set and
/// had no way to delete one, which is what the reference's browser exists for.
fn metadata_field() -> Field {
    Field {
        key: "meta".into(),
        label: "Metadata".into(),
        kind: "metadata".into(),
        default: serde_json::json!({ "set": {}, "remove": [], "strip": false }),
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
            // Lossy, so it stays off by default: the lossless path is what PNG normally
            // means. Drives `colours`, a control, so it never travels as a parameter.
            checkbox("reduce", "Reduce colours (lossy)", false).control(),
            select(
                "colours",
                "Colours",
                "256",
                &["256", "128", "64", "32", "16"],
            )
            .labelled(&[
                "256 colours",
                "128 colours",
                "64 colours",
                "32 colours",
                "16 colours",
            ])
            .when("reduce", "true"),
            // Drives `target_bytes`; a control, so it never travels as a parameter.
            checkbox("target", "Compress to a target file size", false).control(),
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
            }
            .hidden(),
            {
                // The reference puts the ratios under the crop surface as pills.
                let mut field = select(
                    "aspect",
                    "Aspect ratio",
                    "free",
                    &["free", "1:1", "16:9", "4:3", "9:16", "3:4", "4:5"],
                )
                .labelled(&["Free", "1:1", "16:9", "4:3", "9:16", "3:4", "4:5"]);
                field.kind = "segmented".into();
                field
            },
        ],
        (Category::Images, ToolId::Annotate) => vec![
            // A plain caption burned into the corner, for the quick case where the
            // user does not want to place a shape on the canvas. It comes first so it
            // stays above the canvas and is reachable without scrolling.
            text("text", "Caption", "", None),
            Field {
                key: "shapes".into(),
                label: "Annotations".into(),
                kind: "shapes".into(),
                default: serde_json::json!([]),
                ..Default::default()
            }
            .hidden(),
            // The canvas toolbar owns the ink and the shapes; the value still travels.
            Field {
                key: "color".into(),
                label: "Ink colour".into(),
                kind: "color".into(),
                default: serde_json::json!("#ff5a36"),
                ..Default::default()
            }
            .hidden(),
        ],
        (Category::Images, ToolId::Collage) => vec![
            // The order comes first: which image is the feature is a layout decision,
            // not a spacing one. It is rendered by the file-order editor.
            Field {
                key: "order".into(),
                label: "Image order".into(),
                kind: "files".into(),
                default: serde_json::json!([]),
                ..Default::default()
            },
            // Row and column layouts are the grid with one of the counts at 1.
            // Keeping them as separate choices made both counts dead controls.
            select("style", "Style", "grid", &["grid", "featured"])
                .labelled(&["Grid", "Featured (one large picture)"])
                .help("Every layout fills the rows and columns below in order."),
            num("rows", "Rows (0 = auto)", 0.0, 0.0, 6.0, 1.0, "")
                .help("1 makes a single row; 0 fits the pictures into the columns."),
            num("columns", "Columns", 2.0, 1.0, 6.0, 1.0, "")
                .help("1 makes a single column."),
            select(
                "aspect",
                "Canvas ratio",
                "1:1",
                &["auto", "1:1", "16:9", "4:3", "3:2", "2:3", "9:16", "3:4", "4:5"],
            )
            .labelled(&[
                "Auto", "1:1", "16:9", "4:3", "3:2", "2:3", "9:16", "3:4", "4:5",
            ]),
            // A select, not free text: the output size is a choice, and a typo there
            // silently changed resolution instead of failing.
            select(
                "width",
                "Output size",
                "2048",
                &["1024", "2048", "4096"],
            )
            .labelled(&[
                "Small (1024 px)",
                "Medium (2048 px)",
                "Large (4096 px)",
            ]),
            select(
                "fit",
                "Fit",
                "crop",
                &["crop", "contain"],
            )
            .labelled(&["Fill cells (crop edges)", "Fit inside cells"]),
            ranged("spacing_pct", "Spacing", 0.0, 0.0, 100.0, 1.0, "%")
                .group("Spacing & background"),
            Field {
                key: "background".into(),
                label: "Background".into(),
                kind: "color".into(),
                default: serde_json::json!("#ffffff"),
                group: Some("Spacing & background".into()),
                ..Default::default()
            },
            ranged(
                "radius_pct",
                "Corner radius",
                0.0,
                0.0,
                100.0,
                1.0,
                "%",
            )
            .group("Spacing & background"),
        ],
        (Category::Images, ToolId::AddBackground) => vec![
            select(
                "mode",
                "Background",
                "colour",
                &["colour", "gradient", "image"],
            )
            .labelled(&["A colour", "A gradient", "Another image"])
            .control(),
            Field {
                key: "color".into(),
                label: "Colour".into(),
                kind: "color".into(),
                default: serde_json::json!("#101418"),
                show_if: Some(ShowIf {
                    key: "mode".into(),
                    equals: "colour".into(),
                }),
                ..Default::default()
            },
            Field {
                key: "gradient".into(),
                label: "Gradient".into(),
                kind: "gradient".into(),
                default: serde_json::json!("linear|0|#f6c26b@0,#e4572e@100"),
                show_if: Some(ShowIf {
                    key: "mode".into(),
                    equals: "gradient".into(),
                }),
                ..Default::default()
            },
            {
                let mut field = text("image", "Background image", "", Some("path"));
                field.kind = "file".into();
                field.show_if = Some(ShowIf {
                    key: "mode".into(),
                    equals: "image".into(),
                });
                field
            },
            {
                let mut field = ranged("bg_zoom", "Image zoom", 1.0, 1.0, 3.0, 0.1, "\u{00d7}");
                field.show_if = Some(ShowIf {
                    key: "mode".into(),
                    equals: "image".into(),
                });
                field
            },
            {
                let mut field = ranged("bg_x", "Image across", 0.0, -100.0, 100.0, 1.0, "%");
                field.show_if = Some(ShowIf {
                    key: "mode".into(),
                    equals: "image".into(),
                });
                field
            },
            {
                let mut field = ranged("bg_y", "Image up/down", 0.0, -100.0, 100.0, 1.0, "%");
                field.show_if = Some(ShowIf {
                    key: "mode".into(),
                    equals: "image".into(),
                });
                field
            },
            {
                let mut field = range("bg_blur", "Background blur", 0.0, 50.0, 1.0, "px");
                field.show_if = Some(ShowIf {
                    key: "mode".into(),
                    equals: "image".into(),
                });
                // The engine blurs the picture at its own resolution, before stretching
                // the crop to the canvas: the same value is softer on a large picture.
                field.help("Pixels on the background picture itself, before it is scaled to the canvas, so a large picture needs more for the same softness.")
            },
            ranged("padding_pct", "Padding", 4.0, 0.0, 100.0, 1.0, "%"),
            ranged("radius_pct", "Corner radius", 4.0, 0.0, 100.0, 1.0, "%"),
            ranged("shadow_pct", "Shadow", 2.0, 0.0, 100.0, 1.0, "%"),
            ranged(
                "shadow_angle",
                "Shadow angle",
                180.0,
                0.0,
                360.0,
                5.0,
                "\u{00b0}",
            ),
            {
                // Every ratio visible, as in the reference, and "Auto" means the
                // engine's own default: the photo plus equal padding.
                let mut field = select(
                    "aspect",
                    "Canvas ratio",
                    "free",
                    &["free", "1:1", "16:9", "4:3", "9:16", "3:4", "4:5"],
                )
                .labelled(&["Auto", "1:1", "16:9", "4:3", "9:16", "3:4", "4:5"]);
                field.kind = "segmented".into();
                field
            },
        ],
        (Category::Images, ToolId::EditPhoto) => vec![
            {
                let mut field = range("exposure", "Exposure", -5.0, 5.0, 0.01, "EV");
                field.group = Some("Light".into());
                field
            },
            range("brightness", "Brightness", -100.0, 100.0, 1.0, "").group("Light"),
            range("contrast", "Contrast", -100.0, 100.0, 1.0, "").group("Light"),
            range("highlights", "Highlights", -100.0, 100.0, 1.0, "").group("Light"),
            range("shadows", "Shadows", -100.0, 100.0, 1.0, "").group("Light"),
            range("whites", "Whites", -100.0, 100.0, 1.0, "").group("Light"),
            range("blacks", "Blacks", -100.0, 100.0, 1.0, "").group("Light"),
            {
                let mut field = range("gamma", "Gamma", 0.2, 3.0, 0.01, "").group("Light");
                // 1.0 is "leave it alone"; the slider starts neutral, not at zero.
                field.default = serde_json::json!(1.0);
                field
            },
            range("temperature", "Temperature", -100.0, 100.0, 1.0, "").group("Colour"),
            range("tint", "Tint", -100.0, 100.0, 1.0, "").group("Colour"),
            range("saturation", "Saturation", -100.0, 100.0, 1.0, "").group("Colour"),
            range("vibrance", "Vibrance", -100.0, 100.0, 1.0, "").group("Colour"),
            range("hue", "Hue", -180.0, 180.0, 1.0, "°").group("Colour"),
            range("clarity", "Clarity", -100.0, 100.0, 1.0, "").group("Detail"),
            range("dehaze", "Dehaze", -100.0, 100.0, 1.0, "").group("Detail"),
            range("grain", "Grain", 0.0, 100.0, 1.0, "").group("Detail"),
            range("noise", "Noise reduction", 0.0, 100.0, 1.0, "").group("Detail"),
            range("sharpness", "Sharpness", 0.0, 100.0, 1.0, "").group("Detail"),
        ],
        // The tags differ per container (EXIF for JPG/TIFF, ImageMagick for PNG/WEBP);
        // the browser asks the writer which ones it can change.
        (Category::Images, ToolId::Metadata) => vec![metadata_field()],
        (Category::Images, ToolId::Redact) => vec![
            Field {
                key: "boxes".into(),
                label: "Areas to fill".into(),
                kind: "boxes".into(),
                default: serde_json::json!([]),
                ..Default::default()
            }
            .hidden(),
            {
                // Two real treatments: a solid fill or a blur of the region. The
                // switch is a control (the modal adds `style` to the parameters) so
                // the colour's `show_if` has a driver that is not itself a parameter.
                let mut field = select("style", "Style", "solid", &["solid", "blur"])
                    .labelled(&["Solid", "Blur"])
                    .control();
                field.kind = "segmented".into();
                field
            },
            {
                // How strong the blur is: 0 still hides the region (the engine keeps a
                // floor), 100 is the maximum. A solid fill has no blur, so the field is
                // hidden and — because it is a parameter, not a control — not sent.
                let mut field = ranged("strength", "Blur strength", 50.0, 0.0, 100.0, 1.0, "%");
                field.show_if = Some(ShowIf {
                    key: "style".into(),
                    equals: "blur".into(),
                });
                field
            },
            {
                // The colour has no meaning when the region is blurred, so the control
                // is not shown (and not sent) in that mode.
                let mut field = Field {
                    key: "color".into(),
                    label: "Colour".into(),
                    kind: "color".into(),
                    default: serde_json::json!("#000000"),
                    ..Default::default()
                };
                field.show_if = Some(ShowIf {
                    key: "style".into(),
                    equals: "solid".into(),
                });
                field
            },
        ],
        (Category::Video, ToolId::Compress) => vec![
            // Relative to the source, as lesson 11: a fixed byte default compressed a
            // 62 KB clip into 131 KB.
            ranged(
                "percent",
                "Size (% of the original)",
                50.0,
                5.0,
                100.0,
                1.0,
                "%",
            ),
            checkbox("size_target", "Or a target file size", false).control(),
            num(
                "target_bytes",
                "Target size",
                5_000_000.0,
                1000.0,
                20_000_000_000.0,
                1000.0,
                "bytes",
            )
            .when("size_target", "true"),
        ],
        (Category::Audio, ToolId::Compress) => vec![
            ranged(
                "percent",
                "Size (% of the original)",
                50.0,
                5.0,
                100.0,
                1.0,
                "%",
            ),
            checkbox("bitrate_set", "Or a bitrate", false).control(),
            num("bitrate", "Bitrate", 128.0, 8.0, 512.0, 1.0, "kbps").when("bitrate_set", "true"),
            checkbox("size_target", "Or a target file size", false).control(),
            num(
                "target_bytes",
                "Target size",
                5_000_000.0,
                1000.0,
                20_000_000_000.0,
                1000.0,
                "bytes",
            )
            .when("size_target", "true"),
        ],
        // The timeline draws start and end: two number inputs would be a second,
        // unused control for the handles. They still travel to the engine.
        (Category::Video, ToolId::Trim) => vec![
            num("start", "Start", 0.0, 0.0, 86_400.0, 0.01, "s").hidden(),
            num("end", "End", 0.0, 0.0, 86_400.0, 0.01, "s").hidden(),
        ],
        (Category::Audio, ToolId::Trim) => vec![
            num("start", "Start", 0.0, 0.0, 86_400.0, 0.01, "s").hidden(),
            num("end", "End", 0.0, 0.0, 86_400.0, 0.01, "s").hidden(),
            checkbox("trim_silence", "Trim silent ends automatically", false).help(
                "Removes a silent lead-in and tail; pauses inside the recording are kept.",
            ),
        ],
        (Category::Video, ToolId::Crop) => {
            let mut aspect = select(
                "aspect",
                "Aspect ratio",
                "free",
                &["free", "1:1", "16:9", "4:3", "9:16", "3:2"],
            )
            .labelled(&["Free", "1:1", "16:9", "4:3", "9:16", "3:2"]);
            aspect.kind = "segmented".into();
            vec![
                Field {
                    key: "boxes".into(),
                    label: "Area to keep".into(),
                    kind: "boxes".into(),
                    default: serde_json::json!([]),
                    ..Default::default()
                }
                .hidden(),
                aspect,
            ]
        }
        (Category::Video, ToolId::Speed) => {
            vec![ranged("factor", "Speed", 1.0, 0.25, 4.0, 0.05, "×")]
        }
        (Category::Video, ToolId::Mute) => vec![],
        // The still-frame chips are the value; the editor injects `times`, and the
        // hidden field keeps the tool's dialog open instead of running immediately.
        (Category::Video, ToolId::Snapshot) => vec![Field {
            key: "times".into(),
            label: "Frames".into(),
            kind: "text".into(),
            default: serde_json::json!([]),
            ..Default::default()
        }
        .hidden()],
        (Category::Video, ToolId::Redact) => {
            let mut style = select("style", "Style", "solid", &["solid", "blur"])
                .labelled(&["Solid", "Blur"]);
            style.kind = "segmented".into();
            vec![
                Field {
                    key: "boxes".into(),
                    label: "Areas to blur".into(),
                    kind: "boxes".into(),
                    default: serde_json::json!([]),
                    ..Default::default()
                }
                .hidden(),
                style,
                {
                    // The same control as the still image's: a blur has a strength,
                    // 0..100, and the video engine maps it to the boxblur radius.
                    let mut field =
                        ranged("strength", "Blur strength", 50.0, 0.0, 100.0, 1.0, "%");
                    field.show_if = Some(ShowIf {
                        key: "style".into(),
                        equals: "blur".into(),
                    });
                    field
                },
                Field {
                    key: "color".into(),
                    label: "Colour".into(),
                    kind: "color".into(),
                    default: serde_json::json!("#000000"),
                    show_if: Some(ShowIf {
                        key: "style".into(),
                        equals: "solid".into(),
                    }),
                    ..Default::default()
                },
            ]
        }
        // A multi-input tool orders its inputs in the dialog; the editor owns the
        // list and the value travels as indices into the selection.
        (Category::Video, ToolId::Join)
        | (Category::Pdf, ToolId::Merge)
        | (Category::Documents, ToolId::Merge) => vec![Field {
            key: "order".into(),
            label: "Order".into(),
            kind: "files".into(),
            default: serde_json::json!([]),
            ..Default::default()
        }],
        // The split points are the timeline's markers; `submitModal` turns them into
        // consecutive sections. The field exists so the card opens a dialog.
        (Category::Video, ToolId::Split) => vec![Field {
            key: "sections".into(),
            label: "Sections".into(),
            kind: "text".into(),
            default: serde_json::json!([]),
            ..Default::default()
        }
        .hidden()],
        (Category::Audio, ToolId::Normalize) => vec![
            ranged("lufs", "Target loudness", -16.0, -30.0, -5.0, 0.5, "LUFS"),
            ranged("range", "Loudness range", 11.0, 1.0, 50.0, 1.0, "LU"),
            ranged("true_peak", "True peak limit", -1.5, -9.0, 0.0, 0.5, "dBTP"),
        ],
        (Category::Audio, ToolId::Channels) => {
            let mut output = select("channels", "Output", "2", &["1", "2"])
                .labelled(&["Mono", "Stereo"]);
            output.kind = "segmented".into();
            vec![
                output,
                // One output control, not a stereo pair with a dead half: a mono mix
                // is a single volume, a stereo mix has a left and a right.
                ranged("volume", "Volume", 100.0, 0.0, 200.0, 1.0, "%")
                    .when("channels", "1"),
                ranged("left_volume", "Left volume", 100.0, 0.0, 200.0, 1.0, "%")
                    .when("channels", "2"),
                ranged("right_volume", "Right volume", 100.0, 0.0, 200.0, 1.0, "%")
                    .when("channels", "2"),
            ]
        }
        (Category::Audio, ToolId::Visualize) => {
            let mut style = select("style", "Style", "waveform", &["waveform", "bars", "line"])
                .labelled(&["Waveform", "Bars", "Line"]);
            style.kind = "segmented".into();
            let mut shape = select("shape", "Shape", "16:9", &["16:9", "9:16", "1:1"])
                .labelled(&["Landscape · 16:9", "Portrait · 9:16", "Square · 1:1"]);
            shape.kind = "segmented".into();
            vec![
                style,
                shape,
                Field {
                    key: "color".into(),
                    label: "Background".into(),
                    kind: "color".into(),
                    default: serde_json::json!("#101014"),
                    ..Default::default()
                },
            ]
        }
        // The waveform lane is the value; `submitModal` injects `ranges`.
        (Category::Audio, ToolId::Bleep) => vec![Field {
            key: "ranges".into(),
            label: "Ranges".into(),
            kind: "text".into(),
            default: serde_json::json!([]),
            ..Default::default()
        }
        .hidden()],
        (Category::Audio, ToolId::Metadata) => vec![metadata_field()],
        (Category::Pdf, ToolId::Compress) => vec![
            // The stored values are Ghostscript's PDFSETTINGS names, and they must stay
            // that way: the engine interpolates them straight into `-dPDFSETTINGS`.
            // The labels spell out what each tier costs, because four bare words told
            // the user nothing. Measured with Ghostscript 10.08: color image resolution
            // 72/150/300/300 dpi, JPEG quality factor 0.76/0.76/0.4/0.15.
            select(
                "preset",
                "Quality",
                "ebook",
                &["screen", "ebook", "printer", "prepress"],
            )
            .labelled(&[
                "Screen · 72 dpi (smallest)",
                "Ebook · 150 dpi (balanced)",
                "Printer · 300 dpi (print ready)",
                "Prepress · 300 dpi (best images)",
            ])
            .help(
                "Presets differ mainly in image downsampling and compression: Screen and Ebook shrink most but soften photos, Printer and Prepress keep 300 dpi, and Prepress preserves the most image detail and usually makes the largest file.",
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
        // The organizer is a surface, not a field: it owns `order` and `rotate`, and
        // the one control beside it is the width match.
        (Category::Pdf, ToolId::Organize) => vec![
            Field {
                key: "pages".into(),
                label: "Pages".into(),
                kind: "pages".into(),
                default: serde_json::json!({ "order": [], "rotate": [] }),
                ..Default::default()
            },
            checkbox("same_width", "Make all pages the same width", false).help(
                "Pages are scaled proportionally to match the narrowest page; their heights may still differ.",
            ),
        ],
        (Category::Pdf, ToolId::Metadata) => vec![metadata_field()],
        (Category::Pdf, ToolId::SavePages) => {
            vec![select("format", "Image format", "jpg", &["jpg", "png"])]
        }
        (Category::Pdf, ToolId::Split) => vec![],
        (Category::Pdf, ToolId::ExtractText) => vec![],
        (Category::Archives, ToolId::Extract) => vec![],
        _ => vec![],
    }
}

#[derive(Serialize)]
pub struct CacheStats {
    pub bytes: u64,
    pub files: usize,
    pub cap_bytes: u64,
}

/// What the preview cache is holding, so Settings can show it and offer to clear it.
#[tauri::command]
pub fn cache_stats(state: State<'_, AppState>) -> CacheStats {
    let (bytes, files) = state.cache.stats();
    CacheStats {
        bytes,
        files,
        cap_bytes: state.cache.cap(),
    }
}

/// Drop every cached preview. It is a cache: nothing is lost that cannot be regenerated
/// from the source file, so there is no confirmation dialog.
#[tauri::command]
pub fn clear_cache(state: State<'_, AppState>) -> Result<CacheStats, String> {
    state.cache.clear().map_err(err_to_string)?;
    let (bytes, files) = state.cache.stats();
    Ok(CacheStats {
        bytes,
        files,
        cap_bytes: state.cache.cap(),
    })
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
        let source = Path::new(&path);
        // One still path for the strip and for a dialog: a video goes to its
        // filmstrip, a PDF to its page renderer, everything else to the thumbnailer.
        let ctx = JobCtx::new();
        let thumb = still_for(&cache, source, size, &ctx)?;
        Ok(thumb.to_string_lossy().into_owned())
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
            preview: None,
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

#[cfg(test)]
mod field_payload {
    /// The dialog can only be right if the payload names the controls it renders: this
    /// is the contract between the Rust field table and the SolidJS switch.
    #[test]
    fn add_background_describes_its_controls() {
        let fields = super::tool_fields("images".into(), "add-background".into());
        let kind = |key: &str| {
            fields
                .iter()
                .find(|f| f.key == key)
                .map(|f| f.kind.clone())
                .unwrap_or_default()
        };
        assert_eq!(kind("gradient"), "gradient");
        assert_eq!(kind("image"), "file");
        assert_eq!(kind("aspect"), "segmented");
        assert_eq!(kind("color"), "color");
        assert_eq!(kind("padding_pct"), "range");
        assert_eq!(kind("radius_pct"), "range");
        assert_eq!(kind("shadow_pct"), "range");
        assert_eq!(kind("shadow_angle"), "range");
        let aspect = fields.iter().find(|f| f.key == "aspect").unwrap();
        assert_eq!(
            aspect.option_labels.as_deref(),
            Some(
                ["Auto", "1:1", "16:9", "4:3", "9:16", "3:4", "4:5"]
                    .map(String::from)
                    .as_slice()
            )
        );
        let mode = fields.iter().find(|f| f.key == "mode").unwrap();
        assert_eq!(
            mode.option_labels.as_deref(),
            Some(
                ["A colour", "A gradient", "Another image"]
                    .map(String::from)
                    .as_slice()
            )
        );
    }

    /// The collage controls: rows/columns are real numbers, the output width is a
    /// choice, and spacing/radius are percentages of the cell, not pixel counts.
    #[test]
    fn collage_describes_its_controls() {
        let fields = super::tool_fields("images".into(), "collage".into());
        let field = |key: &str| fields.iter().find(|f| f.key == key).unwrap();
        assert_eq!(field("rows").kind, "number");
        assert_eq!(
            (field("rows").min, field("rows").max),
            (Some(0.0), Some(6.0))
        );
        assert_eq!(
            (field("columns").min, field("columns").max),
            (Some(1.0), Some(6.0))
        );
        assert_eq!(field("spacing_pct").kind, "range");
        assert_eq!(field("spacing_pct").max, Some(100.0));
        assert_eq!(field("radius_pct").kind, "range");
        assert_eq!(field("radius_pct").max, Some(100.0));
        assert_eq!(field("width").kind, "select");
        assert_eq!(
            field("width").options.as_deref(),
            Some(["1024", "2048", "4096"].map(String::from).as_slice())
        );
        assert_eq!(
            field("width").option_labels.as_deref(),
            Some(
                ["Small (1024 px)", "Medium (2048 px)", "Large (4096 px)"]
                    .map(String::from)
                    .as_slice()
            )
        );
        let aspect = field("aspect").options.clone().unwrap_or_default();
        assert!(aspect.contains(&"3:2".to_string()));
        assert!(aspect.contains(&"2:3".to_string()));
        // The pixel fields are gone: a value in pixels was invisible at other sizes.
        assert!(
            fields
                .iter()
                .all(|f| f.key.as_str() != "spacing" && f.key.as_str() != "radius")
        );
    }

    /// The PDF preset stores Ghostscript PDFSETTINGS names verbatim; the dialog is
    /// what translates them into a resolution and a tradeoff. Both halves are the
    /// contract: a renamed stored value breaks the engine, a dropped label brings
    /// back the four opaque words.
    #[test]
    fn pdf_compress_describes_its_presets() {
        let fields = super::tool_fields("pdf".into(), "compress".into());
        let preset = fields.iter().find(|f| f.key == "preset").unwrap();
        assert_eq!(preset.kind, "select");
        assert_eq!(preset.default, serde_json::json!("ebook"));
        assert_eq!(
            preset.options.as_deref(),
            Some(
                ["screen", "ebook", "printer", "prepress"]
                    .map(String::from)
                    .as_slice()
            )
        );
        assert_eq!(
            preset.option_labels.as_deref(),
            Some(
                [
                    "Screen · 72 dpi (smallest)",
                    "Ebook · 150 dpi (balanced)",
                    "Printer · 300 dpi (print ready)",
                    "Prepress · 300 dpi (best images)",
                ]
                .map(String::from)
                .as_slice()
            )
        );
        let help = preset.help.as_deref().expect("the preset needs help text");
        // The help is the one place that compares the two 300 dpi tiers, which the
        // labels alone cannot: Prepress is the larger file.
        assert!(help.contains("300 dpi"), "help names the shared resolution");
        assert!(help.contains("Prepress"), "help explains prepress");
        assert!(help.contains("largest file"), "help warns about file size");
    }

    /// Metadata is one browser over three backend keys, in every category that has a
    /// writer. The per-tag text fields it replaced offered tags the writer rejected.
    #[test]
    fn every_metadata_tool_shows_the_browser() {
        for category in ["images", "audio", "pdf"] {
            let fields = super::tool_fields(category.into(), "metadata".into());
            assert_eq!(fields.len(), 1, "{category} metadata has extra fields");
            let field = &fields[0];
            assert_eq!(field.key, "meta");
            assert_eq!(field.kind, "metadata");
            assert_eq!(
                field.default,
                serde_json::json!({ "set": {}, "remove": [], "strip": false }),
                "{category}: the browser starts with nothing to change"
            );
        }
    }

    /// A switch that drives `show_if` must be a control or it travels as a parameter;
    /// that exact leak rejected every image compress run.
    #[test]
    fn every_show_if_driver_is_a_control() {
        use convertme_core::formats::{Category, Registry};
        for category in Category::ALL {
            for tool in Registry::tools_for(category) {
                let fields = super::fields_for(category, tool.id);
                for field in &fields {
                    let Some(rule) = &field.show_if else { continue };
                    let driver = fields
                        .iter()
                        .find(|f| f.key == rule.key)
                        .unwrap_or_else(|| {
                            panic!(
                                "{} {}: show_if names missing `{}`",
                                category.key(),
                                tool.id.as_str(),
                                rule.key
                            )
                        });
                    // A compress switch exists only to reveal another control, so it
                    // must not travel. A style/channel switch is itself a parameter the
                    // engine reads (`style=blur`, `channels=1`): hiding one row is the
                    // point, sending the driver is correct.
                    let parameter_driver = matches!(rule.key.as_str(), "style" | "channels");
                    assert!(
                        driver.control || parameter_driver,
                        "{} {}: `{}` drives `{}` but is not a control",
                        category.key(),
                        tool.id.as_str(),
                        rule.key,
                        field.key
                    );
                }
            }
        }
    }

    /// Video and audio compress defaulted to an absolute 20 MB target: a 62 KB clip
    /// came out at 131 KB. The default must be a proportion of the source.
    #[test]
    fn av_compress_defaults_are_relative() {
        let video = super::tool_fields("video".into(), "compress".into());
        let percent = video
            .iter()
            .find(|f| f.key == "percent")
            .expect("a relative size field");
        assert_eq!(percent.kind, "range");
        assert_eq!(percent.default, serde_json::json!(50.0));
        assert!(
            video.iter().all(|f| f.key != "bitrate"),
            "the video encoder reads no bitrate parameter"
        );
        let target = video
            .iter()
            .find(|f| f.key == "target_bytes")
            .expect("a target-size option");
        assert!(target.show_if.is_some(), "the target hides until asked for");

        let audio = super::tool_fields("audio".into(), "compress".into());
        assert!(audio.iter().any(|f| f.key == "percent"));
        assert!(audio.iter().any(|f| f.key == "bitrate"));
    }

    /// The dialog sends only non-control, visible fields. The target switch must stay a
    /// control: as a parameter the engine rejects the whole job ("unknown parameter
    /// `target`"), which made every image compress run fail.
    #[test]
    fn compress_target_is_a_control_and_its_parameters_validate() {
        use convertme_core::formats::{Category, ToolId};
        use convertme_core::job::{Intent, Job};
        use std::path::PathBuf;

        let fields = super::tool_fields("images".into(), "compress".into());
        let target = fields
            .iter()
            .find(|f| f.key == "target")
            .expect("the target switch exists");
        assert!(target.control, "the target switch must not travel");

        let reduce = fields
            .iter()
            .find(|f| f.key == "reduce")
            .expect("the reduce-colours switch exists");
        assert!(reduce.control, "the reduce-colours switch must not travel");
        let colours = fields
            .iter()
            .find(|f| f.key == "colours")
            .expect("the palette size exists");
        assert_eq!(
            colours.show_if.as_ref().map(|rule| rule.key.as_str()),
            Some("reduce"),
            "the palette size is shown only while reduce is on"
        );
        assert_eq!(colours.default, serde_json::json!("256"));

        let job = |params: serde_json::Value| {
            Job::new(
                PathBuf::from("/tmp/photo.jpg"),
                Intent::Tool {
                    tool: ToolId::Compress,
                    params: params.as_object().cloned().expect("object params"),
                },
            )
        };
        convertme_engines::image::validate(
            &job(serde_json::json!({
                "quality": 80,
                "scale": 50,
                "target_bytes": 500_000,
                "colours": 64
            })),
            Category::Images,
        )
        .expect("the parameters the dialog collects must validate");
        assert!(
            convertme_engines::image::validate(
                &job(serde_json::json!({ "target": true })),
                Category::Images
            )
            .is_err(),
            "a leaked control must be rejected, which is why it is marked as one"
        );
        assert!(
            convertme_engines::image::validate(
                &job(serde_json::json!({ "colours": 1 })),
                Category::Images
            )
            .is_err(),
            "a palette below two colours cannot be indexed meaningfully"
        );
    }

    /// The command wrapper, end to end: normalize → encode → cache → the numbers the
    /// dialog draws. Skipped when the fixture or ImageMagick is absent.
    #[test]
    fn compress_preview_reports_and_reuses_a_real_file() {
        let fixture = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../fixtures/photo.jpg")
            .canonicalize();
        let Ok(fixture) = fixture else { return };
        let tools = convertme_engines::Toolset::probe_now();
        if tools
            .path(convertme_core::capability::ToolKey::Magick)
            .is_err()
        {
            return;
        }
        let cache = tempfile::tempdir().expect("temp cache");
        let run = || {
            super::preview_compress_blocking(
                &tools,
                cache.path().to_path_buf(),
                64 << 20,
                &fixture,
                serde_json::json!({ "quality": 70 }),
            )
            .expect("preview")
        };

        let first = run();
        assert!(
            std::path::Path::new(&first.path).starts_with(cache.path()),
            "the artifact must live in the shared preview cache"
        );
        assert_eq!(
            first.source_bytes,
            std::fs::metadata(&fixture).expect("metadata").len()
        );
        assert!(first.bytes > 0, "the encoded file must have bytes");
        let (width, height) =
            convertme_engines::image_dimensions(std::path::Path::new(&first.path))
                .expect("decode the artifact");
        assert_eq!((first.width, first.height), (width, height));

        // The identical request resolves to the same artifact: the dialog and the job
        // share one encode.
        let second = run();
        assert_eq!(second.path, first.path);
        assert_eq!(second.bytes, first.bytes);

        // q100 on this already-compressed fixture is larger; the preview must show the
        // original instead of a growth.
        let unchanged = super::preview_compress_blocking(
            &tools,
            cache.path().to_path_buf(),
            64 << 20,
            &fixture,
            serde_json::json!({ "quality": 100 }),
        )
        .expect("preview");
        assert_eq!(
            unchanged.bytes, unchanged.source_bytes,
            "the preview promised a larger file"
        );
    }
}

/// Every (category, tool) field table, as one JSON object.
///
/// This is what `ui/src/fields.generated.json` holds, and what the mock renders from:
/// the dialog's fields come from the backend table in both modes, so a mock cannot
/// show a control the app does not have (it used to: a hand-written mock table had
/// `boxes` visible for the crop dialog and the surface appeared twice).
pub fn all_tool_fields() -> serde_json::Value {
    let mut categories = serde_json::Map::new();
    for category in Category::ALL {
        let mut tools = serde_json::Map::new();
        for tool in Registry::tools_for(category) {
            tools.insert(
                tool.id.as_str().to_string(),
                serde_json::to_value(fields_for(category, tool.id)).unwrap_or_default(),
            );
        }
        categories.insert(category.key().to_string(), serde_json::Value::Object(tools));
    }
    serde_json::Value::Object(categories)
}

/// The table as the generated file holds it: keys sorted, so the file is diffable.
pub fn sorted_json(value: &serde_json::Value) -> String {
    fn sorted(value: &serde_json::Value) -> serde_json::Value {
        match value {
            serde_json::Value::Object(map) => {
                let mut out = serde_json::Map::new();
                let mut keys: Vec<&String> = map.keys().collect();
                keys.sort();
                for key in keys {
                    out.insert(key.clone(), sorted(&map[key]));
                }
                serde_json::Value::Object(out)
            }
            serde_json::Value::Array(items) => {
                serde_json::Value::Array(items.iter().map(sorted).collect())
            }
            other => other.clone(),
        }
    }
    serde_json::to_string_pretty(&sorted(value)).unwrap_or_default()
}

#[cfg(test)]
mod generated_fields {
    /// The mock's copy of the field tables must be the backend's copy.
    ///
    /// `scripts/gen-ui-fields.py` writes the file; this test is what makes forgetting
    /// to run it a failure instead of a mock that quietly renders a stale dialog.
    #[test]
    fn the_ui_field_table_is_current() {
        let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("../../ui/src/fields.generated.json");
        let text = std::fs::read_to_string(&path).unwrap_or_else(|e| {
            panic!(
                "{} is missing ({e}); run `./scripts/gen-ui-fields.py`",
                path.display()
            )
        });
        let current = super::all_tool_fields();
        // Escape hatch for the writer: the file is data the mock imports, so the test
        // is the only thing that can regenerate it from the code that is compiling.
        if std::env::var_os("UPDATE_FIELDS").is_some() {
            std::fs::write(&path, super::sorted_json(&current) + "\n")
                .expect("writing the field table");
            return;
        }
        let on_disk: serde_json::Value =
            serde_json::from_str(&text).expect("parse the generated file");
        assert_eq!(
            on_disk, current,
            "the mock's field tables are stale: run `UPDATE_FIELDS=1 cargo test -p convertme-gui \
             generated_fields`, then `npm --prefix ui run build`"
        );
    }
}

#[cfg(test)]
mod wheels_payload {
    use super::*;
    use convertme_core::capability::Capabilities;

    /// The dialog draws its title, its primary button and the rail from these four
    /// keys. A wheel that omits them renders an empty sheet head and a rail with no
    /// copy, which is exactly what the mock-only harness cannot see.
    #[test]
    fn tool_segments_carry_the_dialog_copy() {
        let caps = Capabilities::assume_all();
        for category in Category::ALL {
            for segment in super::tool_segments(category, &caps) {
                let tool = Registry::tool(category, ToolId::parse(&segment.id).unwrap()).unwrap();
                assert_eq!(
                    segment.title.as_deref(),
                    Some(tool.title),
                    "{category}/{} title",
                    segment.id
                );
                assert_eq!(
                    segment.action.as_deref(),
                    Some(tool.action),
                    "{category}/{} action",
                    segment.id
                );
                assert_eq!(segment.benefits.len(), tool.benefits.len());
                assert_eq!(segment.eyebrow.as_deref(), tool.eyebrow);
                // The blurb is the rail's body copy.
                assert_eq!(segment.blurb.as_deref(), Some(tool.blurb));
            }
        }
    }

    /// Compress and Metadata exist per category but read the same in each; the
    /// reference calls that "Any file" instead of the category word.
    #[test]
    fn cross_format_editors_say_any_file() {
        let caps = Capabilities::assume_all();
        for category in [
            Category::Images,
            Category::Video,
            Category::Audio,
            Category::Pdf,
        ] {
            let segments = super::tool_segments(category, &caps);
            let compress = segments.iter().find(|s| s.id == "compress").unwrap();
            assert_eq!(compress.title.as_deref(), Some("Compress"));
            assert_eq!(compress.action.as_deref(), Some("Compress"));
            assert_eq!(compress.eyebrow.as_deref(), Some("Any file"));
            assert!(!compress.benefits.is_empty());
        }
        // An ordinary tool falls back to the category label: no eyebrow.
        let images = super::tool_segments(Category::Images, &caps);
        let crop = images.iter().find(|s| s.id == "crop").unwrap();
        assert_eq!(crop.eyebrow, None);
        assert_eq!(crop.action.as_deref(), Some("Apply"));
    }

    /// Every tool card is a promise that a dialog opens. A card whose field table is
    /// empty runs the job straight from the wheel, which is only right for tools that
    /// genuinely have nothing to ask (Mute, Split pages, Extract text, Extract).
    #[test]
    fn only_fieldless_tools_run_without_a_dialog() {
        let caps = Capabilities::assume_all();
        let fieldless = [
            (Category::Video, ToolId::Mute),
            (Category::Pdf, ToolId::Split),
            (Category::Pdf, ToolId::ExtractText),
            (Category::Archives, ToolId::Extract),
        ];
        for category in Category::ALL {
            for segment in super::tool_segments(category, &caps) {
                let tool = ToolId::parse(&segment.id).unwrap();
                let has_fields = !super::fields_for(category, tool).is_empty();
                if fieldless.contains(&(category, tool)) {
                    assert!(!has_fields, "{category}/{tool} should ask nothing");
                } else {
                    assert!(
                        has_fields,
                        "{category}/{tool} runs silently from the wheel: its dialog has no fields"
                    );
                }
            }
        }
    }

    /// Convert targets and file actions are not tools: a stale title on them would
    /// make the dialog claim they have a settings sheet.
    #[test]
    fn convert_and_action_segments_carry_no_dialog_copy() {
        let caps = Capabilities::assume_all();
        for segment in super::convert_segments(Category::Images, None, &caps) {
            assert!(segment.title.is_none());
            assert!(segment.action.is_none());
            assert!(segment.benefits.is_empty());
            assert!(segment.eyebrow.is_none());
        }
        for segment in super::action_segments() {
            assert!(segment.title.is_none());
            assert!(segment.action.is_none());
            assert!(segment.benefits.is_empty());
        }
    }
}
