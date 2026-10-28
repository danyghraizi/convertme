//! Error vocabulary shared by the planner, the queue and every engine.
//!
//! An engine may never return a bare string or panic: everything crosses the job
//! boundary as a [`JobError`] carrying a stable [`ErrorCode`] the UI can switch on,
//! plus the tail of the tool's stderr for the "view details" pane.

use std::fmt;

/// Stable machine-readable failure classes. The string forms are part of the
/// history table and must not change casually.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum ErrorCode {
    /// The pair (source format, target) is not in the registry.
    Unsupported,
    /// A required external tool is not present.
    MissingTool,
    /// Filesystem error unrelated to permissions or space.
    Io,
    /// EACCES/EPERM, including a read-only source directory.
    Permission,
    /// Not enough free space for the estimated output; raised *before* starting.
    DiskFull,
    /// The per-job watchdog fired.
    TimedOut,
    /// The user cancelled.
    Cancelled,
    /// The tool ran and failed (non-zero exit, unparsable output, corrupt input).
    EngineFailed,
    /// The job was running when the process died; reconciled at startup.
    Interrupted,
}

impl ErrorCode {
    pub fn as_str(self) -> &'static str {
        match self {
            ErrorCode::Unsupported => "unsupported",
            ErrorCode::MissingTool => "missing_tool",
            ErrorCode::Io => "io",
            ErrorCode::Permission => "permission",
            ErrorCode::DiskFull => "disk_full",
            ErrorCode::TimedOut => "timed_out",
            ErrorCode::Cancelled => "cancelled",
            ErrorCode::EngineFailed => "engine_failed",
            ErrorCode::Interrupted => "interrupted",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "unsupported" => ErrorCode::Unsupported,
            "missing_tool" => ErrorCode::MissingTool,
            "io" => ErrorCode::Io,
            "permission" => ErrorCode::Permission,
            "disk_full" => ErrorCode::DiskFull,
            "timed_out" => ErrorCode::TimedOut,
            "cancelled" => ErrorCode::Cancelled,
            "engine_failed" => ErrorCode::EngineFailed,
            "interrupted" => ErrorCode::Interrupted,
            _ => return None,
        })
    }

    /// Whether the same job could plausibly succeed if retried as-is.
    pub fn retryable(self) -> bool {
        matches!(
            self,
            ErrorCode::TimedOut | ErrorCode::Io | ErrorCode::DiskFull
        )
    }
}

impl fmt::Display for ErrorCode {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

/// A failed job. `stderr_tail` is capped at 8 KiB by the process helper.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct JobError {
    pub code: ErrorCode,
    pub message: String,
    pub stderr_tail: Option<String>,
}

impl JobError {
    pub fn new(code: ErrorCode, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
            stderr_tail: None,
        }
    }

    pub fn with_stderr(mut self, tail: impl Into<String>) -> Self {
        let tail = tail.into();
        if !tail.trim().is_empty() {
            self.stderr_tail = Some(tail);
        }
        self
    }

    pub fn io(message: impl Into<String>, err: &std::io::Error) -> Self {
        let code = match err.kind() {
            std::io::ErrorKind::PermissionDenied => ErrorCode::Permission,
            _ => ErrorCode::Io,
        };
        if let Some(code2) = err.raw_os_error().and_then(os_error_code) {
            return Self::new(code2, format!("{}: {err}", message.into()));
        }
        Self::new(code, format!("{}: {err}", message.into()))
    }

    pub fn unsupported(from: &str, to: &str) -> Self {
        Self::new(
            ErrorCode::Unsupported,
            format!("{from} → {to} is not supported"),
        )
    }

    pub fn missing_tool(tool: &str) -> Self {
        Self::new(ErrorCode::MissingTool, format!("{tool} is not installed"))
    }
}

impl fmt::Display for JobError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for JobError {}

#[cfg(unix)]
fn os_error_code(raw: i32) -> Option<ErrorCode> {
    match raw {
        libc::ENOSPC | libc::EDQUOT => Some(ErrorCode::DiskFull),
        libc::EACCES | libc::EPERM | libc::EROFS => Some(ErrorCode::Permission),
        _ => None,
    }
}

#[cfg(not(unix))]
fn os_error_code(raw: i32) -> Option<ErrorCode> {
    // ERROR_DISK_FULL, ERROR_HANDLE_DISK_FULL, ERROR_ACCESS_DENIED (incl. EROFS-mapped).
    match raw {
        112 | 39 => Some(ErrorCode::DiskFull),
        5 | 19 => Some(ErrorCode::Permission),
        _ => None,
    }
}

/// Planner failures. Per-file problems during planning are *not* fatal: they are
/// collected into the plan's `skipped` list so one unreadable file cannot abort a
/// 20-file batch.
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PlanError {
    #[error("no input files")]
    Empty,
    #[error("no inputs matched category {0}")]
    NoCategoryMatch(&'static str),
    #[error("{0}")]
    Unsupported(JobError),
    #[error("{0}")]
    MissingTool(JobError),
    #[error("invalid tool parameters: {0}")]
    BadParams(String),
}
