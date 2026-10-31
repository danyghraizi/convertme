//! Job identity, intent, work directory and the progress/cancel handle.
//!
//! The type system here enforces the immutability invariant: an engine only ever
//! receives a [`Job`] (read-only `source` path plus intent) and a [`WorkDir`] (the
//! only writable place it can name). Publishing into the user's directory is the
//! queue's job, never the engine's.

use crate::error::{ErrorCode, JobError};
use crate::formats::{FormatId, ToolId};
use serde::{Deserialize, Serialize};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU32, Ordering};

/// Stable job identifier: a v4 UUID string, also the SQLite primary key.
pub type JobId = String;

/// Which worker pool a job belongs to. Two pools, so a 40-file selection of
/// extracts cannot starve a running video encode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum Pool {
    Cpu,
    Io,
}

/// Free-form tool parameters, validated by the tool module that owns them.
pub type ToolParams = serde_json::Map<String, serde_json::Value>;

/// What the user asked for.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "lowercase")]
pub enum Intent {
    Convert {
        target: FormatId,
    },
    Tool {
        tool: ToolId,
        #[serde(default)]
        params: ToolParams,
    },
}

impl Intent {
    /// Convert to a registered format, by canonical id. Returns `None` for an
    /// unknown id so no `&'static str` is ever manufactured at runtime.
    pub fn convert(target: &str) -> Option<Self> {
        crate::formats::Registry::by_id(target).map(|f| Intent::Convert { target: f.id })
    }

    pub fn tool(tool: ToolId) -> Self {
        Intent::Tool {
            tool,
            params: ToolParams::new(),
        }
    }

    pub fn target(&self) -> Option<FormatId> {
        match self {
            Intent::Convert { target } => Some(*target),
            Intent::Tool { .. } => None,
        }
    }

    pub fn tool_id(&self) -> Option<ToolId> {
        match self {
            Intent::Tool { tool, .. } => Some(*tool),
            Intent::Convert { .. } => None,
        }
    }

    pub fn params(&self) -> Option<&ToolParams> {
        match self {
            Intent::Tool { params, .. } => Some(params),
            Intent::Convert { .. } => None,
        }
    }

    /// Stable hash input for the dedupe key: canonical path + this string.
    pub fn dedupe_signature(&self) -> String {
        match self {
            Intent::Convert { target } => format!("convert:{}", target.0),
            Intent::Tool { tool, params } => {
                let mut keys: Vec<_> = params.iter().collect();
                keys.sort_by(|a, b| a.0.cmp(b.0));
                let rendered: Vec<String> =
                    keys.into_iter().map(|(k, v)| format!("{k}={v}")).collect();
                format!("tool:{tool}:{}", rendered.join(","))
            }
        }
    }

    /// What to show a person: never the serialized form.
    pub fn describe(&self) -> String {
        match self {
            Intent::Convert { target } => format!("to {}", target.0),
            Intent::Tool { tool, .. } => tool.as_str().replace('-', " "),
        }
    }

    /// Extension of the artifact this intent produces, when it is known up front.
    pub fn output_ext(&self) -> Option<&'static str> {
        match self {
            Intent::Convert { target } => {
                crate::formats::Registry::get(*target).map(|f| f.canonical_ext())
            }
            Intent::Tool { tool, .. } => tool_default_ext(*tool),
        }
    }
}

/// Default artifact extension per tool. Parameter overrides (e.g. `format: png`)
/// are honoured by the engine; this is what the publish name uses when the tool
/// does not say otherwise.
pub fn tool_default_ext(tool: ToolId) -> Option<&'static str> {
    match tool {
        // These keep the source container: the publish name reuses the source
        // extension, resolved by the queue from the produced artifact's path.
        ToolId::Compress
        | ToolId::Crop
        | ToolId::Annotate
        | ToolId::Collage
        | ToolId::AddBackground
        | ToolId::EditPhoto
        | ToolId::Metadata
        | ToolId::Redact
        | ToolId::Trim
        | ToolId::Speed
        | ToolId::Mute
        | ToolId::Join
        | ToolId::Split
        | ToolId::Bleep
        | ToolId::Normalize
        | ToolId::Channels
        | ToolId::Organize
        | ToolId::Merge
        | ToolId::Extract => None,
        ToolId::Snapshot | ToolId::SavePages => Some("png"),
        ToolId::Visualize => Some("mp4"),
        ToolId::ExtractText => Some("txt"),
    }
}

/// One unit of work.
#[derive(Debug, Clone)]
pub struct Job {
    pub id: JobId,
    /// The user's file. Read-only by contract.
    pub source: PathBuf,
    /// Every input, in selection order. Multi-input tools (join, merge) use all of
    /// them; single-input jobs carry exactly one, equal to `source`.
    pub inputs: Vec<PathBuf>,
    pub intent: Intent,
    pub pool: Pool,
    pub input_format: Option<FormatId>,
}

impl Job {
    pub fn new(source: PathBuf, intent: Intent) -> Self {
        let input_format = crate::formats::Registry::detect(&source).map(|f| f.id);
        let pool = match intent.tool_id() {
            Some(ToolId::Extract) => Pool::Io,
            _ => Pool::Cpu,
        };
        let inputs = vec![source.clone()];
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            source,
            inputs,
            intent,
            pool,
            input_format,
        }
    }

    /// A job over several inputs, ordered. `source` is the first one, which is
    /// what the history table and the publish name refer to.
    pub fn with_inputs(inputs: Vec<PathBuf>, intent: Intent) -> Self {
        let source = inputs.first().cloned().unwrap_or_default();
        let input_format = crate::formats::Registry::detect(&source).map(|f| f.id);
        let pool = match intent.tool_id() {
            Some(ToolId::Extract) => Pool::Io,
            _ => Pool::Cpu,
        };
        Self {
            id: uuid::Uuid::new_v4().to_string(),
            source,
            inputs,
            intent,
            pool,
            input_format,
        }
    }

    pub fn signature(&self) -> String {
        format!(
            "{}|{}",
            canonical_or_self(&self.source).display(),
            self.intent.dedupe_signature()
        )
    }
}

pub fn canonical_or_self(path: &Path) -> PathBuf {
    std::fs::canonicalize(path).unwrap_or_else(|_| path.to_path_buf())
}

/// The only writable directory an engine ever sees:
/// `<tmp>/convertme-<pid>-<seq>/{src,out}`.
#[derive(Debug, Clone)]
pub struct WorkDir {
    root: PathBuf,
}

impl WorkDir {
    pub fn create(root: PathBuf) -> std::io::Result<Self> {
        std::fs::create_dir_all(root.join("src"))?;
        std::fs::create_dir_all(root.join("out"))?;
        Ok(Self { root })
    }

    pub fn root(&self) -> &Path {
        &self.root
    }

    pub fn src(&self) -> PathBuf {
        self.root.join("src")
    }

    pub fn out(&self) -> PathBuf {
        self.root.join("out")
    }

    /// Copy the source in, keeping its name. Engines whose semantics are
    /// in-place operate on this copy.
    pub fn copy_in(&self, source: &Path) -> Result<PathBuf, JobError> {
        let name = source
            .file_name()
            .ok_or_else(|| JobError::new(ErrorCode::Io, "input has no file name"))?;
        let dst = self.src().join(name);
        std::fs::copy(source, &dst)
            .map_err(|e| JobError::io("copying the source into the work dir", &e))?;
        Ok(dst)
    }

    /// A path inside the work dir for an intermediate artifact.
    pub fn scratch(&self, name: &str) -> PathBuf {
        self.root.join(name)
    }

    /// Remove the whole work dir, ignoring errors (the sweeper will catch strays).
    pub fn cleanup(&self) {
        let _ = std::fs::remove_dir_all(&self.root);
    }
}

/// Progress (monotonic 0..1) and cancellation for one running job.
#[derive(Clone, Default)]
pub struct JobCtx {
    progress_milli: Arc<AtomicU32>,
    cancelled: Arc<AtomicBool>,
    stage: Arc<parking_lot::Mutex<String>>,
}

impl JobCtx {
    pub fn new() -> Self {
        Self::default()
    }

    /// Report progress. Values below the current one are ignored so a
    /// second pass or a resumed file cannot move the bar backwards.
    pub fn set_progress(&self, value: f64) {
        let milli = (value.clamp(0.0, 1.0) * 1000.0) as u32;
        let _ = self
            .progress_milli
            .try_update(Ordering::Relaxed, Ordering::Relaxed, |cur| {
                Some(cur.max(milli))
            });
    }

    pub fn progress(&self) -> f64 {
        self.progress_milli.load(Ordering::Relaxed) as f64 / 1000.0
    }

    pub fn set_stage(&self, stage: impl Into<String>) {
        *self.stage.lock() = stage.into();
    }

    pub fn stage(&self) -> String {
        self.stage.lock().clone()
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::SeqCst);
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::SeqCst)
    }

    /// Bail out with `cancelled` if the user asked to stop.
    pub fn checkpoint(&self) -> Result<(), JobError> {
        if self.is_cancelled() {
            Err(JobError::new(ErrorCode::Cancelled, "cancelled by the user"))
        } else {
            Ok(())
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn progress_is_monotonic() {
        let ctx = JobCtx::new();
        ctx.set_progress(0.6);
        ctx.set_progress(0.2);
        assert!((ctx.progress() - 0.6).abs() < 0.002);
        ctx.set_progress(2.0);
        assert_eq!(ctx.progress(), 1.0);
    }

    #[test]
    fn workdir_isolates_writes() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("in.txt");
        std::fs::write(&src, b"hello").unwrap();
        let wd = WorkDir::create(dir.path().join("work")).unwrap();
        let copy = wd.copy_in(&src).unwrap();
        assert!(copy.starts_with(wd.src()));
        std::fs::write(&copy, b"changed").unwrap();
        assert_eq!(std::fs::read(&src).unwrap(), b"hello");
        assert!(wd.out().is_dir());
        wd.cleanup();
        assert!(!wd.root().exists());
    }

    #[test]
    fn signature_distinguishes_intents() {
        let a = Job::new(
            PathBuf::from("/tmp/a.png"),
            Intent::convert("WEBP").unwrap(),
        );
        let b = Job::new(PathBuf::from("/tmp/a.png"), Intent::convert("JPG").unwrap());
        assert_ne!(a.signature(), b.signature());
        assert_eq!(
            a.signature(),
            Job::new(
                PathBuf::from("/tmp/a.png"),
                Intent::convert("WEBP").unwrap()
            )
            .signature()
        );
        assert!(Intent::convert("NOTAFORMAT").is_none());
    }
}
