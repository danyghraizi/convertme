//! Job history: SQLite in WAL mode with one writer (the queue), and the startup
//! reconciliation that turns a crashed run's `running` rows into `interrupted`.
//!
//! The table is display-only; nothing in the conversion path reads it back.

use crate::error::{ErrorCode, JobError};
use crate::job::{Intent, Job, JobId};
use rusqlite::{Connection, OptionalExtension, params};
use std::path::{Path, PathBuf};
use std::time::Duration;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Status {
    Queued,
    Running,
    Done,
    Failed,
    Cancelled,
    Interrupted,
}

impl Status {
    pub fn as_str(self) -> &'static str {
        match self {
            Status::Queued => "queued",
            Status::Running => "running",
            Status::Done => "done",
            Status::Failed => "failed",
            Status::Cancelled => "cancelled",
            Status::Interrupted => "interrupted",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Some(match s {
            "queued" => Status::Queued,
            "running" => Status::Running,
            "done" => Status::Done,
            "failed" => Status::Failed,
            "cancelled" => Status::Cancelled,
            "interrupted" => Status::Interrupted,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone)]
pub struct JobRow {
    pub id: JobId,
    pub input: PathBuf,
    pub intent: String,
    pub output: Option<PathBuf>,
    pub status: Status,
    pub started_at: Option<i64>,
    pub ended_at: Option<i64>,
    pub error_code: Option<ErrorCode>,
    pub error_tail: Option<String>,
}

pub struct Store {
    conn: Connection,
}

impl Store {
    pub fn open(path: &Path) -> Result<Self, JobError> {
        if let Some(parent) = path.parent() {
            std::fs::create_dir_all(parent)
                .map_err(|e| JobError::io("creating the data dir", &e))?;
        }
        let conn = Connection::open(path).map_err(|e| {
            JobError::new(ErrorCode::Io, format!("opening the history database: {e}"))
        })?;
        conn.busy_timeout(Duration::from_secs(5)).ok();
        conn.pragma_update(None, "journal_mode", "WAL").ok();
        conn.pragma_update(None, "synchronous", "NORMAL").ok();
        let store = Self { conn };
        store.migrate()?;
        Ok(store)
    }

    pub fn in_memory() -> Result<Self, JobError> {
        let conn = Connection::open_in_memory()
            .map_err(|e| JobError::new(ErrorCode::Io, format!("opening sqlite: {e}")))?;
        let store = Self { conn };
        store.migrate()?;
        Ok(store)
    }

    fn migrate(&self) -> Result<(), JobError> {
        self.conn
            .execute_batch(
                "CREATE TABLE IF NOT EXISTS jobs (
                    id TEXT PRIMARY KEY,
                    input TEXT NOT NULL,
                    intent TEXT NOT NULL,
                    output TEXT,
                    status TEXT NOT NULL,
                    started_at INTEGER,
                    ended_at INTEGER,
                    error_code TEXT,
                    error_tail TEXT
                );
                CREATE INDEX IF NOT EXISTS jobs_ended ON jobs(ended_at DESC);",
            )
            .map_err(|e| {
                JobError::new(
                    ErrorCode::Io,
                    format!("migrating the history database: {e}"),
                )
            })
    }

    pub fn enqueue(&self, job: &Job, now: i64) -> Result<(), JobError> {
        let intent = serde_json::to_string(&job.intent).unwrap_or_default();
        self.conn
            .execute(
                "INSERT OR REPLACE INTO jobs (id, input, intent, output, status, started_at, ended_at, error_code, error_tail)
                 VALUES (?1, ?2, ?3, NULL, 'queued', NULL, NULL, NULL, NULL)",
                params![job.id, job.source.to_string_lossy(), intent],
            )
            .map_err(db_err)?;
        let _ = now;
        Ok(())
    }

    pub fn mark_running(&self, id: &JobId, now: i64) -> Result<(), JobError> {
        self.conn
            .execute(
                "UPDATE jobs SET status='running', started_at=?2 WHERE id=?1",
                params![id, now],
            )
            .map_err(db_err)?;
        Ok(())
    }

    pub fn finish(
        &self,
        id: &JobId,
        status: Status,
        output: Option<&Path>,
        error: Option<&JobError>,
        now: i64,
    ) -> Result<(), JobError> {
        self.conn
            .execute(
                "UPDATE jobs SET status=?2, output=?3, ended_at=?4, error_code=?5, error_tail=?6 WHERE id=?1",
                params![
                    id,
                    status.as_str(),
                    output.map(|p| p.to_string_lossy().into_owned()),
                    now,
                    error.map(|e| e.code.as_str()),
                    error.and_then(|e| e.stderr_tail.clone()),
                ],
            )
            .map_err(db_err)?;
        Ok(())
    }

    /// Every `running` row from a previous process becomes `interrupted`.
    pub fn reconcile(&self, now: i64) -> Result<usize, JobError> {
        let n = self
            .conn
            .execute(
                "UPDATE jobs SET status='interrupted', ended_at=?1, error_code='interrupted'
                 WHERE status IN ('running','queued')",
                params![now],
            )
            .map_err(db_err)?;
        Ok(n)
    }

    pub fn row(&self, id: &JobId) -> Result<Option<JobRow>, JobError> {
        let row = self
            .conn
            .query_row(
                "SELECT id, input, intent, output, status, started_at, ended_at, error_code, error_tail
                 FROM jobs WHERE id=?1",
                params![id],
                map_row,
            )
            .optional()
            .map_err(db_err)?;
        Ok(row)
    }

    /// Most recent first.
    pub fn history(&self, limit: usize) -> Result<Vec<JobRow>, JobError> {
        let mut stmt = self
            .conn
            .prepare(
                "SELECT id, input, intent, output, status, started_at, ended_at, error_code, error_tail
                 FROM jobs ORDER BY COALESCE(ended_at, started_at, 0) DESC, rowid DESC LIMIT ?1",
            )
            .map_err(db_err)?;
        let rows = stmt
            .query_map(params![limit as i64], map_row)
            .map_err(db_err)?
            .collect::<Result<Vec<_>, _>>()
            .map_err(db_err)?;
        Ok(rows)
    }

    /// Keep the newest `keep` rows; returns how many were removed.
    pub fn prune(&self, keep: usize) -> Result<usize, JobError> {
        let n = self
            .conn
            .execute(
                "DELETE FROM jobs WHERE id NOT IN (
                    SELECT id FROM jobs ORDER BY COALESCE(ended_at, started_at, 0) DESC, rowid DESC LIMIT ?1
                 )",
                params![keep as i64],
            )
            .map_err(db_err)?;
        Ok(n)
    }

    pub fn count(&self) -> Result<usize, JobError> {
        let n: i64 = self
            .conn
            .query_row("SELECT COUNT(*) FROM jobs", [], |r| r.get(0))
            .map_err(db_err)?;
        Ok(n as usize)
    }
}

fn db_err(e: rusqlite::Error) -> JobError {
    JobError::new(ErrorCode::Io, format!("history database: {e}"))
}

fn map_row(r: &rusqlite::Row<'_>) -> rusqlite::Result<JobRow> {
    let status: String = r.get(4)?;
    let error_code: Option<String> = r.get(7)?;
    Ok(JobRow {
        id: r.get(0)?,
        input: PathBuf::from(r.get::<_, String>(1)?),
        intent: r.get(2)?,
        output: r.get::<_, Option<String>>(3)?.map(PathBuf::from),
        status: Status::parse(&status).unwrap_or(Status::Failed),
        started_at: r.get(5)?,
        ended_at: r.get(6)?,
        error_code: error_code.as_deref().and_then(ErrorCode::parse),
        error_tail: r.get(8)?,
    })
}

impl JobRow {
    pub fn intent_value(&self) -> Option<Intent> {
        serde_json::from_str(&self.intent).ok()
    }
}

/// Map a finished job to the row status the UI shows.
pub fn status_for(error: Option<&JobError>) -> Status {
    match error {
        None => Status::Done,
        Some(e) => match e.code {
            ErrorCode::Cancelled => Status::Cancelled,
            ErrorCode::Interrupted => Status::Interrupted,
            _ => Status::Failed,
        },
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::job::{Intent, Job};

    #[test]
    fn rows_roundtrip_and_reconcile() {
        let store = Store::in_memory().unwrap();
        let job = Job::new(
            PathBuf::from("/tmp/a.png"),
            Intent::convert("WEBP").unwrap(),
        );
        store.enqueue(&job, 1).unwrap();
        store.mark_running(&job.id, 2).unwrap();
        let row = store.row(&job.id).unwrap().unwrap();
        assert_eq!(row.status, Status::Running);
        assert_eq!(row.intent_value(), Some(Intent::convert("WEBP").unwrap()));

        // A crash leaves the row running; the next launch reconciles it.
        assert_eq!(store.reconcile(3).unwrap(), 1);
        let row = store.row(&job.id).unwrap().unwrap();
        assert_eq!(row.status, Status::Interrupted);
        assert_eq!(row.error_code, Some(ErrorCode::Interrupted));
        assert_eq!(row.ended_at, Some(3));
    }

    #[test]
    fn history_is_newest_first_and_prunable() {
        let store = Store::in_memory().unwrap();
        for i in 0..10 {
            let job = Job::new(
                PathBuf::from(format!("/tmp/{i}.png")),
                Intent::convert("WEBP").unwrap(),
            );
            store.enqueue(&job, i).unwrap();
            store.mark_running(&job.id, i).unwrap();
            store
                .finish(
                    &job.id,
                    Status::Done,
                    Some(Path::new("/tmp/out.webp")),
                    None,
                    i,
                )
                .unwrap();
        }
        let hist = store.history(4).unwrap();
        assert_eq!(hist.len(), 4);
        assert_eq!(hist[0].ended_at, Some(9));
        assert_eq!(store.count().unwrap(), 10);
        assert_eq!(store.prune(5).unwrap(), 5);
        assert_eq!(store.count().unwrap(), 5);
    }

    #[test]
    fn failures_keep_their_code_and_stderr_tail() {
        let store = Store::in_memory().unwrap();
        let job = Job::new(
            PathBuf::from("/tmp/a.pdf"),
            Intent::convert("DOCX").unwrap(),
        );
        store.enqueue(&job, 0).unwrap();
        let err =
            JobError::new(ErrorCode::EngineFailed, "sidecar crashed").with_stderr("Traceback…");
        store
            .finish(&job.id, status_for(Some(&err)), None, Some(&err), 5)
            .unwrap();
        let row = store.row(&job.id).unwrap().unwrap();
        assert_eq!(row.status, Status::Failed);
        assert_eq!(row.error_code, Some(ErrorCode::EngineFailed));
        assert_eq!(row.error_tail.as_deref(), Some("Traceback…"));
    }
}
