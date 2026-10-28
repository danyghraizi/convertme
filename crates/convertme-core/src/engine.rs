//! The one interface the queue has to engines.
//!
//! `convertme-engines` implements this; the queue never knows what a tool is.

use crate::error::JobError;
use crate::job::{Job, JobCtx, WorkDir};

/// What an engine produced: paths inside `work.out()` (files or directories).
/// The queue, not the engine, decides where they are published.
pub type Artifacts = Vec<std::path::PathBuf>;

pub trait EngineRunner: Send + Sync + 'static {
    /// Run one job. All writes stay inside `work`; `job.source` is read-only.
    fn run(&self, job: &Job, work: &WorkDir, ctx: &JobCtx) -> Result<Artifacts, JobError>;

    /// Upper bound on the bytes this job will produce, used by the free-space
    /// pre-check. `input × 1.2` is the default; compress jobs use their target.
    fn estimate_output_bytes(&self, job: &Job) -> u64 {
        let total: u64 = job
            .inputs
            .iter()
            .filter_map(|p| std::fs::metadata(p).ok().map(|m| m.len()))
            .sum();
        if let Some(params) = job.intent.params()
            && let Some(target) = params.get("target_bytes").and_then(|v| v.as_u64())
        {
            return target.saturating_mul(job.inputs.len().max(1) as u64);
        }
        (total as f64 * 1.2) as u64
    }

    /// Cheap parameter validation, run before the job is queued so a bad modal
    /// value is rejected instantly instead of after a 2-minute encode.
    fn validate(&self, _job: &Job) -> Result<(), JobError> {
        Ok(())
    }
}
