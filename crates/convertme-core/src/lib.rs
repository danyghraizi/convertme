//! `convertme-core`: identity, the format registry, the planner, the job queue and
//! everything that must not depend on a UI toolkit or a subprocess.
//!
//! Layering: `core` knows *what* a job is; `convertme-engines` knows *how* to run
//! one; `convertme-drag` knows how a drop arrives; `convertme-gui` wires them up.

pub mod capability;
pub mod engine;
pub mod error;
pub mod formats;
pub mod freespace;
pub mod identity;
pub mod job;
pub mod naming;
pub mod params;
pub mod paths;
pub mod plan;
pub mod publish;
pub mod queue;
pub mod settings;
pub mod store;
pub mod subtitle;

pub use capability::{Capabilities, Capability, ToolKey};
pub use error::{ErrorCode, JobError, PlanError};
pub use formats::{Category, Format, FormatId, Registry, Tool, ToolId};
pub use job::{Intent, Job, JobCtx, JobId, Pool, WorkDir};
pub use plan::{Plan, PlannedJob, Skipped, plan, plan_with_caps};
pub use queue::{Event, Queue, QueueConfig};
pub use settings::Settings;
pub use store::{JobRow, Status, Store};

/// The derived conversion-option count. Never hardcoded in copy; the About screen
/// and the marketing number both read this.
pub fn conversion_option_count() -> usize {
    Registry::pairs().count()
}

/// The derived file-tool count.
pub fn file_tool_count() -> usize {
    formats::TOOLS.len()
}
