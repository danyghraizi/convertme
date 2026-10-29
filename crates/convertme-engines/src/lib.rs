//! `convertme-engines`: one module per tool family, all sharing one process helper
//! and one capability cache.
//!
//! Every module exposes functions with the same shape:

//! ```ignore
//! pub fn run(job: &Job, cx: &Context<'_>) -> Result<Vec<PathBuf>, JobError>
//! ```
//!
//! and writes its artifacts into `cx.work.out()`. Returning a path outside the work
//! dir is refused by the queue, so no engine can publish anything itself.

pub mod archives;
pub mod av;
pub mod docs;
pub mod face;
pub mod image;
pub mod metadata;
pub mod pdf;
pub mod preview;
pub mod probe;
pub mod proc;
pub mod redact;

use convertme_core::capability::{Capabilities, ToolKey};
use convertme_core::engine::EngineRunner;
use convertme_core::error::{ErrorCode, JobError};
use convertme_core::formats::{Category, FormatId, Registry, ToolId};
use convertme_core::job::{Intent, Job, JobCtx, WorkDir};
use parking_lot::RwLock;
use std::path::{Path, PathBuf};
use std::sync::Arc;

/// Resolved tool paths plus the capability cache that produced them.
#[derive(Clone)]
pub struct Toolset {
    caps: Arc<RwLock<Capabilities>>,
    opts: probe::ProbeOptions,
}

impl Toolset {
    pub fn new(caps: Arc<RwLock<Capabilities>>, opts: probe::ProbeOptions) -> Self {
        Self { caps, opts }
    }

    /// Probe the host now and cache the result (used by the CLI and by tests).
    pub fn probe_now() -> Self {
        let opts = probe::ProbeOptions::from_env();
        let caps = probe::probe_all(&opts);
        Self {
            caps: Arc::new(RwLock::new(caps)),
            opts,
        }
    }

    pub fn capabilities(&self) -> Capabilities {
        self.caps.read().clone()
    }

    pub fn options(&self) -> &probe::ProbeOptions {
        &self.opts
    }

    pub fn has(&self, key: ToolKey) -> bool {
        self.caps.read().available(key)
    }

    pub fn has_engine(&self, engine: convertme_core::formats::EngineRef) -> bool {
        if self.caps.read().has_engine(engine) {
            return true;
        }
        // A "no" from the cache can be out of date: the cache may predate the
        // install, or the background probe may not have finished yet. Look once
        // (`path` caches the answer) instead of refusing a job the tool can run.
        engine.required().iter().all(|key| self.path(*key).is_ok())
    }

    pub fn feature(&self, key: ToolKey, feature: &str) -> bool {
        self.caps
            .read()
            .get(key)
            .map(|c| c.has_feature(feature))
            .unwrap_or(false)
    }

    /// Resolve a tool, re-probing once if the cache predates an install.
    pub fn path(&self, key: ToolKey) -> Result<PathBuf, JobError> {
        if let Some(cap) = self.caps.read().get(key)
            && cap.ok
            && let Some(path) = &cap.path
            && path.exists()
        {
            return Ok(path.clone());
        }
        if let Some(path) = probe::locate(key, &self.opts) {
            let mut caps = self.caps.write();
            caps.put(probe::probe_one(key, &self.opts));
            return Ok(path);
        }
        Err(JobError::missing_tool(key.label()))
    }

    /// The LibreOffice command line, honouring Flatpak's host spawn.
    pub fn soffice_invocation(&self) -> Result<Vec<PathBuf>, JobError> {
        let path = self.path(ToolKey::Soffice)?;
        if path
            .file_name()
            .map(|n| n == "flatpak-spawn")
            .unwrap_or(false)
        {
            Ok(vec![
                path,
                PathBuf::from("--host"),
                PathBuf::from("soffice"),
            ])
        } else {
            Ok(vec![path])
        }
    }
}

/// What an engine module gets: the tools, the only writable directory, and the
/// progress/cancel handle.
pub struct Context<'a> {
    pub tools: &'a Toolset,
    pub work: &'a WorkDir,
    pub ctx: &'a JobCtx,
}

impl<'a> Context<'a> {
    /// A fresh artifact path inside `out/`.
    pub fn out(&self, name: &str) -> PathBuf {
        self.work.out().join(name)
    }

    /// A scratch path in the work root (not published).
    pub fn scratch(&self, name: &str) -> PathBuf {
        self.work.scratch(name)
    }

    /// The staged copy of an input, for engines that mutate their input.
    pub fn copy_of(&self, index: usize, job: &Job) -> Result<PathBuf, JobError> {
        let source = job
            .inputs
            .get(index)
            .ok_or_else(|| JobError::new(ErrorCode::Io, "missing input"))?;
        let name = source
            .file_name()
            .map(|n| n.to_string_lossy().into_owned())
            .unwrap_or_else(|| format!("input-{index}"));
        let staged = self.work.src().join(&name);
        if !staged.exists() {
            std::fs::copy(source, &staged)
                .map_err(|e| JobError::io(format!("staging {name}"), &e))?;
        }
        Ok(staged)
    }

    pub fn stage(&self, name: &str) -> String {
        self.ctx.set_stage(name);
        name.to_string()
    }
}

/// The single [`EngineRunner`] implementation: routes a job to its module.
pub struct Dispatcher {
    tools: Toolset,
}

impl Dispatcher {
    pub fn new(tools: Toolset) -> Self {
        Self { tools }
    }

    pub fn tools(&self) -> &Toolset {
        &self.tools
    }

    fn route(&self, job: &Job) -> Result<Route, JobError> {
        let input = job
            .input_format
            .or_else(|| Registry::detect(&job.source).map(|f| f.id))
            .ok_or_else(|| JobError::new(ErrorCode::Unsupported, "unknown input format"))?;
        let category = Registry::get(input).map(|f| f.category).ok_or_else(|| {
            JobError::new(
                ErrorCode::Unsupported,
                format!("{input} is not a known format"),
            )
        })?;

        match &job.intent {
            Intent::Convert { target } => {
                let engine = Registry::engine_for(input, *target)
                    .ok_or_else(|| JobError::unsupported(input.0, target.0))?;
                if !self.tools.has_engine(engine) {
                    return Err(JobError::missing_tool(engine.label()));
                }
                Ok(match category {
                    Category::Images => Route::Image,
                    Category::Video | Category::Audio => Route::Av,
                    Category::Pdf => Route::Pdf,
                    Category::Documents | Category::Epub | Category::Slides => Route::Docs,
                    Category::Spreadsheets => Route::Docs,
                    Category::Subtitles => Route::Subtitle,
                    Category::Archives => Route::Archive,
                })
            }
            Intent::Tool { tool, .. } => {
                let spec = Registry::tool(category, *tool).ok_or_else(|| {
                    JobError::new(
                        ErrorCode::Unsupported,
                        format!("{tool} is not a {category} tool"),
                    )
                })?;
                if !self.tools.has_engine(spec.engine) {
                    return Err(JobError::missing_tool(spec.engine.label()));
                }
                // Tool ids repeat across categories (Compress, Metadata, Split,
                // Redact), so the category decides which module owns the work.
                Ok(match category {
                    Category::Images => match tool {
                        ToolId::Metadata => Route::Metadata,
                        ToolId::Redact => Route::Redact,
                        _ => Route::Image,
                    },
                    Category::Video | Category::Audio => match tool {
                        ToolId::Metadata => Route::Metadata,
                        ToolId::Redact => Route::Redact,
                        _ => Route::Av,
                    },
                    Category::Pdf => match tool {
                        ToolId::Metadata => Route::Metadata,
                        _ => Route::Pdf,
                    },
                    Category::Archives => Route::Archive,
                    Category::Documents | Category::Epub => Route::Docs,
                    Category::Slides | Category::Spreadsheets => Route::Docs,
                    Category::Subtitles => Route::Docs,
                })
            }
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum Route {
    Image,
    Av,
    Pdf,
    Docs,
    Subtitle,
    Archive,
    Metadata,
    Redact,
}

impl EngineRunner for Dispatcher {
    fn validate(&self, job: &Job) -> Result<(), JobError> {
        self.route(job)?;
        // Per-tool parameter checks live with their module; they are cheap.
        if job.intent.tool_id().is_some() {
            let input = job
                .input_format
                .or_else(|| Registry::detect(&job.source).map(|f| f.id))
                .ok_or_else(|| JobError::new(ErrorCode::Unsupported, "unknown input format"))?;
            if let Some(category) = Registry::get(input).map(|f| f.category) {
                av::validate(job, category)?;
                image::validate(job, category)?;
            }
        }
        Ok(())
    }

    fn run(&self, job: &Job, work: &WorkDir, ctx: &JobCtx) -> Result<Vec<PathBuf>, JobError> {
        let cx = Context {
            tools: &self.tools,
            work,
            ctx,
        };
        let route = self.route(job)?;
        let artifacts = match route {
            Route::Image => image::run(job, &cx)?,
            Route::Av => av::run(job, &cx)?,
            Route::Pdf => pdf::run(job, &cx)?,
            Route::Docs => docs::run(job, &cx)?,
            Route::Subtitle => docs::subtitle(job, &cx)?,
            Route::Archive => archives::run(job, &cx)?,
            Route::Metadata => metadata::run(job, &cx)?,
            Route::Redact => redact::run(job, &cx)?,
        };
        if artifacts.is_empty() {
            return Err(JobError::new(
                ErrorCode::EngineFailed,
                "the engine produced no output",
            ));
        }
        Ok(artifacts)
    }
}

/// Human name of a format for filenames and labels.
pub fn label_of(id: FormatId) -> &'static str {
    Registry::get(id).map(|f| f.label).unwrap_or(id.0)
}

/// Extension for a target format.
pub fn ext_of(id: FormatId) -> &'static str {
    Registry::get(id)
        .map(|f| f.canonical_ext())
        .unwrap_or("bin")
}

pub(crate) fn ensure_file(path: &Path, what: &str) -> Result<(), JobError> {
    match std::fs::metadata(path) {
        Ok(meta) if meta.len() > 0 => Ok(()),
        Ok(_) => Err(JobError::new(
            ErrorCode::EngineFailed,
            format!("{what} produced an empty file"),
        )),
        Err(e) => Err(JobError::io(format!("{what} did not produce {path:?}"), &e)),
    }
}
