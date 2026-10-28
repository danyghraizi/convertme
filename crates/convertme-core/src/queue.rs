//! The job queue: two worker pools, atomic publish, dedupe, cancellation, crash
//! reconciliation and the temp-dir sweeper.
//!
//! Guarantees, all of them tested elsewhere:
//! * the UI thread never owns a worker and never does file I/O;
//! * a source file is only ever read (`job.source` is passed to engines as a path
//!   to read; writes go to `WorkDir`);
//! * the published path is never the source path and never an existing file under
//!   the default overwrite policy;
//! * a cancelled or failed job leaves no partial artifact and no work dir.

use crate::engine::EngineRunner;
use crate::error::{ErrorCode, JobError};
use crate::freespace;
use crate::job::{Job, JobCtx, JobId, Pool, WorkDir};
use crate::naming::{self, Overwrite};
use crate::publish;
use crate::store::{Status, Store, status_for};
use crossbeam_channel::{Receiver, Sender, bounded};
use parking_lot::Mutex;
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::time::{Duration, SystemTime};

/// Events surfaced to the UI. Sent on a bounded channel; a slow UI drops progress
/// updates rather than blocking a worker.
#[derive(Debug, Clone)]
pub enum Event {
    Queued {
        id: JobId,
        input: PathBuf,
    },
    Started {
        id: JobId,
        input: PathBuf,
    },
    Progress {
        id: JobId,
        fraction: f64,
        stage: String,
    },
    Finished {
        id: JobId,
        outputs: Vec<PathBuf>,
        fallback_dir: bool,
    },
    Failed {
        id: JobId,
        error: JobError,
        outputs: Vec<PathBuf>,
    },
    Cancelled {
        id: JobId,
    },
    /// Nothing is running or queued any more.
    Idle,
}

#[derive(Debug, Clone)]
pub struct QueueConfig {
    pub cpu_workers: usize,
    pub io_workers: usize,
    pub overwrite: Overwrite,
    pub history_limit: usize,
    /// Overrides `std::env::temp_dir()`; used by the perf harness and tests.
    pub temp_root: Option<PathBuf>,
}

impl Default for QueueConfig {
    fn default() -> Self {
        let cpus = std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(4);
        Self {
            cpu_workers: (cpus / 2).max(1),
            io_workers: 2,
            overwrite: Overwrite::Never,
            history_limit: 500,
            temp_root: None,
        }
    }
}

struct Shared {
    cfg: QueueConfig,
    runner: Arc<dyn EngineRunner>,
    store: Mutex<Store>,
    events: Sender<Event>,
    live: Mutex<HashMap<JobId, JobCtx>>,
    dedupe: Mutex<HashMap<String, JobId>>,
    queued: AtomicU64,
    running: AtomicU64,
    shutdown: AtomicBool,
    seq: AtomicU64,
}

impl Shared {
    fn emit(&self, event: Event) {
        // `try_send` keeps a stalled UI from back-pressuring a worker.
        let _ = self.events.try_send(event);
    }

    fn now() -> i64 {
        SystemTime::now()
            .duration_since(SystemTime::UNIX_EPOCH)
            .map(|d| d.as_secs() as i64)
            .unwrap_or(0)
    }

    fn work_root(&self, seq: u64) -> PathBuf {
        let base = self
            .cfg
            .temp_root
            .clone()
            .unwrap_or_else(std::env::temp_dir);
        base.join(format!(
            "{}-{}-{seq}",
            crate::identity::TMP_PREFIX,
            std::process::id()
        ))
    }
}

pub struct Queue {
    shared: Arc<Shared>,
    /// Held so `shutdown` can drop them, which is what wakes the workers. Keeping
    /// the workers on a blocking `recv` (not a poll) is what makes an idle app cost
    /// zero wakeups.
    cpu_tx: Mutex<Option<Sender<Job>>>,
    io_tx: Mutex<Option<Sender<Job>>>,
    workers: Mutex<Vec<std::thread::JoinHandle<()>>>,
}

impl Queue {
    /// Start the pools. `store` is moved behind the queue's single-writer mutex.
    pub fn start(
        cfg: QueueConfig,
        runner: Arc<dyn EngineRunner>,
        store: Store,
        events: Sender<Event>,
    ) -> Arc<Self> {
        let shared = Arc::new(Shared {
            cfg: cfg.clone(),
            runner,
            store: Mutex::new(store),
            events,
            live: Mutex::new(HashMap::new()),
            dedupe: Mutex::new(HashMap::new()),
            queued: AtomicU64::new(0),
            running: AtomicU64::new(0),
            shutdown: AtomicBool::new(false),
            seq: AtomicU64::new(0),
        });

        // Reconcile the previous process before any new job can start.
        if let Err(e) = shared.store.lock().reconcile(Shared::now()) {
            shared.emit(Event::Failed {
                id: "startup".into(),
                error: e,
                outputs: vec![],
            });
        }

        let (cpu_tx, cpu_rx) = bounded::<Job>(256);
        let (io_tx, io_rx) = bounded::<Job>(256);
        let mut workers = Vec::new();

        for n in 0..cfg.cpu_workers.max(1) {
            let rx: Receiver<Job> = cpu_rx.clone();
            let sh = shared.clone();
            workers.push(
                std::thread::Builder::new()
                    .name(format!("convertme-cpu-{n}"))
                    .spawn(move || worker_loop(sh, rx))
                    .expect("spawn cpu worker"),
            );
        }
        for n in 0..cfg.io_workers.max(1) {
            let rx: Receiver<Job> = io_rx.clone();
            let sh = shared.clone();
            workers.push(
                std::thread::Builder::new()
                    .name(format!("convertme-io-{n}"))
                    .spawn(move || worker_loop(sh, rx))
                    .expect("spawn io worker"),
            );
        }

        Arc::new(Self {
            shared,
            cpu_tx: Mutex::new(Some(cpu_tx)),
            io_tx: Mutex::new(Some(io_tx)),
            workers: Mutex::new(workers),
        })
    }

    /// Queue a job. A repeat submit of a job that is still live returns the
    /// existing id instead of queueing a twin.
    pub fn submit(&self, job: Job) -> Result<(JobId, bool), JobError> {
        let signature = job.signature();
        {
            let dedupe = self.shared.dedupe.lock();
            if let Some(existing) = dedupe.get(&signature) {
                return Ok((existing.clone(), true));
            }
        }

        self.shared.runner.validate(&job)?;

        let now = Shared::now();
        self.shared.store.lock().enqueue(&job, now)?;

        let ctx = JobCtx::new();
        self.shared.live.lock().insert(job.id.clone(), ctx);
        self.shared.dedupe.lock().insert(signature, job.id.clone());
        self.shared.queued.fetch_add(1, Ordering::SeqCst);
        self.shared.emit(Event::Queued {
            id: job.id.clone(),
            input: job.source.clone(),
        });

        let id = job.id.clone();
        let tx = match job.pool {
            Pool::Cpu => self.cpu_tx.lock().clone(),
            Pool::Io => self.io_tx.lock().clone(),
        }
        .ok_or_else(|| JobError::new(ErrorCode::Cancelled, "the queue is shutting down"))?;
        match tx.send(job) {
            Ok(()) => Ok((id, false)),
            Err(e) => {
                self.shared.live.lock().remove(&id);
                self.shared.dedupe.lock().retain(|_, v| *v != id);
                self.shared.queued.fetch_sub(1, Ordering::SeqCst);
                let err = JobError::new(
                    ErrorCode::Io,
                    format!("the queue is not accepting jobs: {e}"),
                );
                self.shared.store.lock().finish(
                    &id,
                    Status::Failed,
                    None,
                    Some(&err),
                    Shared::now(),
                )?;
                Err(err)
            }
        }
    }

    /// Cancel one job. A queued job is dropped by the worker when it dequeues; a
    /// running job is killed by the engine's watchdog on `ctx.is_cancelled`.
    pub fn cancel(&self, id: &str) {
        if let Some(ctx) = self.shared.live.lock().get(id) {
            ctx.cancel();
        }
    }

    pub fn cancel_all(&self) {
        for ctx in self.shared.live.lock().values() {
            ctx.cancel();
        }
    }

    pub fn is_running(&self, id: &str) -> bool {
        self.shared.live.lock().contains_key(id)
    }

    pub fn progress(&self, id: &str) -> Option<(f64, String)> {
        self.shared
            .live
            .lock()
            .get(id)
            .map(|c| (c.progress(), c.stage()))
    }

    pub fn pending(&self) -> u64 {
        self.shared.queued.load(Ordering::SeqCst)
    }

    pub fn running(&self) -> u64 {
        self.shared.running.load(Ordering::SeqCst)
    }

    /// Stop accepting jobs, cancel what runs, and join the workers.
    pub fn shutdown(&self) {
        self.shared.shutdown.store(true, Ordering::SeqCst);
        self.cancel_all();
        // Dropping the senders disconnects the channels, which is what releases the
        // workers from their blocking `recv`.
        *self.cpu_tx.lock() = None;
        *self.io_tx.lock() = None;
        let mut workers = self.workers.lock();
        for handle in workers.drain(..) {
            let _ = handle.join();
        }
    }
}

fn worker_loop(shared: Arc<Shared>, rx: Receiver<Job>) {
    // A blocking receive: an idle queue costs nothing at all.
    while let Ok(job) = rx.recv() {
        if shared.shutdown.load(Ordering::SeqCst) {
            return;
        }
        execute(&shared, job);
    }
}

fn execute(shared: &Arc<Shared>, job: Job) {
    let id = job.id.clone();
    let input = job.source.clone();

    let ctx = shared.live.lock().get(&id).cloned().unwrap_or_default();
    if ctx.is_cancelled() {
        finish_cancelled(shared, &job);
        return;
    }

    shared.queued.fetch_sub(1, Ordering::SeqCst);
    shared.running.fetch_add(1, Ordering::SeqCst);
    shared.emit(Event::Started {
        id: id.clone(),
        input: input.clone(),
    });
    let _ = shared.store.lock().mark_running(&id, Shared::now());

    // Progress monitor: one short-lived thread per running job, none when idle.
    let progress_done = Arc::new(AtomicBool::new(false));
    let monitor = {
        let shared = shared.clone();
        let ctx = ctx.clone();
        let id = id.clone();
        let done = progress_done.clone();
        std::thread::Builder::new()
            .name("convertme-progress".into())
            .spawn(move || {
                let mut last = -1.0f64;
                while !done.load(Ordering::SeqCst) {
                    let p = ctx.progress();
                    if (p - last).abs() > 0.004 {
                        last = p;
                        shared.emit(Event::Progress {
                            id: id.clone(),
                            fraction: p,
                            stage: ctx.stage(),
                        });
                    }
                    std::thread::sleep(Duration::from_millis(120));
                }
            })
            .ok()
    };

    let result = run_job(shared, &job, &ctx);

    progress_done.store(true, Ordering::SeqCst);
    if let Some(handle) = monitor {
        let _ = handle.join();
    }

    shared.running.fetch_sub(1, Ordering::SeqCst);
    shared.live.lock().remove(&id);
    shared.dedupe.lock().retain(|_, v| *v != id);

    match result {
        Ok(outputs) => {
            let fallback = outputs.iter().any(|p| {
                p.parent()
                    .map(|d| d == naming::fallback_dir())
                    .unwrap_or(false)
            });
            let primary = outputs.first().cloned();
            let _ = shared.store.lock().finish(
                &id,
                Status::Done,
                primary.as_deref(),
                None,
                Shared::now(),
            );
            shared.emit(Event::Finished {
                id: id.clone(),
                outputs,
                fallback_dir: fallback,
            });
        }
        Err(error) => {
            let status = status_for(Some(&error));
            let _ = shared
                .store
                .lock()
                .finish(&id, status, None, Some(&error), Shared::now());
            if error.code == ErrorCode::Cancelled {
                shared.emit(Event::Cancelled { id: id.clone() });
            } else {
                shared.emit(Event::Failed {
                    id,
                    error,
                    outputs: vec![],
                });
            }
        }
    }

    if shared.queued.load(Ordering::SeqCst) == 0 && shared.running.load(Ordering::SeqCst) == 0 {
        shared.emit(Event::Idle);
    }
}

fn finish_cancelled(shared: &Arc<Shared>, job: &Job) {
    let error = JobError::new(ErrorCode::Cancelled, "cancelled before it started");
    let _ = shared.store.lock().finish(
        &job.id,
        Status::Cancelled,
        None,
        Some(&error),
        Shared::now(),
    );
    shared.queued.fetch_sub(1, Ordering::SeqCst);
    shared.live.lock().remove(&job.id);
    shared.dedupe.lock().retain(|_, v| *v != job.id);
    shared.emit(Event::Cancelled { id: job.id.clone() });
}

/// The whole lifecycle of one job, minus bookkeeping.
fn run_job(shared: &Arc<Shared>, job: &Job, ctx: &JobCtx) -> Result<Vec<PathBuf>, JobError> {
    ctx.checkpoint()?;

    // Free-space pre-check before anything is created.
    let estimated = shared.runner.estimate_output_bytes(job);
    let (dest_dir, _fallback) = publish::publish_dir_for(&job.source);
    let work_root = shared.work_root(shared.seq.fetch_add(1, Ordering::SeqCst));
    freespace::precheck(
        work_root.parent().unwrap_or(Path::new(".")),
        &dest_dir,
        estimated,
    )?;

    let work = WorkDir::create(work_root.clone())
        .map_err(|e| JobError::io("creating the work directory", &e))?;

    let result = (|| -> Result<Vec<PathBuf>, JobError> {
        let artifacts = shared.runner.run(job, &work, ctx)?;
        ctx.checkpoint()?;
        publish_artifacts(job, &work, artifacts, shared.cfg.overwrite, ctx)
    })();

    // The work dir never outlives the job, whatever happened.
    work.cleanup();
    result
}

fn publish_artifacts(
    job: &Job,
    work: &WorkDir,
    artifacts: Vec<PathBuf>,
    overwrite: Overwrite,
    ctx: &JobCtx,
) -> Result<Vec<PathBuf>, JobError> {
    let out_root = work.out();
    let (dest_dir, _) = publish::publish_dir_for(&job.source);
    let suffix = job.intent.tool_id().map(|t| t.as_str());
    let mut published = Vec::new();

    for artifact in artifacts {
        ctx.checkpoint()?;
        // Refuse to publish anything that is not the engine's own output.
        if !artifact.starts_with(&out_root) {
            return Err(JobError::new(
                ErrorCode::EngineFailed,
                format!(
                    "engine returned a path outside its work dir: {}",
                    artifact.display()
                ),
            ));
        }
        let is_dir = std::fs::metadata(&artifact)
            .map(|m| m.is_dir())
            .unwrap_or(false);
        if is_dir {
            let stem = job
                .source
                .file_stem()
                .map(|s| s.to_string_lossy().into_owned())
                .unwrap_or_else(|| "output".into());
            let name = match suffix {
                Some(s) => format!("{stem}-{s}"),
                None => format!("{stem}-out"),
            };
            let published_path = publish::publish_dir(&artifact, &dest_dir.join(name))?;
            published.push(published_path);
        } else {
            let ext = artifact
                .extension()
                .map(|e| e.to_string_lossy().to_ascii_lowercase())
                .or_else(|| job.intent.output_ext().map(|e| e.to_string()))
                .unwrap_or_else(|| "bin".into());
            let desired = naming::resolve_output(&job.source, &ext, overwrite, suffix);
            let desired = if desired.parent() != Some(dest_dir.as_path()) {
                dest_dir.join(desired.file_name().unwrap_or_default())
            } else {
                desired
            };
            // Belt and braces: a published path may never be the source.
            if naming::same_path(&desired, &job.source) {
                return Err(JobError::new(
                    ErrorCode::Io,
                    "refusing to publish over the source file",
                ));
            }
            published.push(publish::publish_file(&artifact, &desired, overwrite)?);
        }
    }

    if published.is_empty() {
        return Err(JobError::new(
            ErrorCode::EngineFailed,
            "the engine produced no output",
        ));
    }
    Ok(published)
}

/// Delete `<tmp>/convertme-*` dirs whose owning process is gone. Dirs belonging
/// to a live process are left alone.
pub fn sweep_temp_dirs() -> usize {
    sweep_temp_dirs_in(&std::env::temp_dir())
}

/// Sweep a specific temp root (the perf harness and tests use this).
pub fn sweep_temp_dirs_in(root: &Path) -> usize {
    let Ok(entries) = std::fs::read_dir(root) else {
        return 0;
    };
    let mut removed = 0;
    let own_pid = std::process::id();
    for entry in entries.flatten() {
        let name = entry.file_name().to_string_lossy().into_owned();
        let Some(rest) = name.strip_prefix(&format!("{}-", crate::identity::TMP_PREFIX)) else {
            continue;
        };
        let Some(pid) = rest.split('-').next().and_then(|p| p.parse::<u32>().ok()) else {
            continue;
        };
        if pid == own_pid || process_alive(pid) {
            continue;
        }
        let path = entry.path();
        if std::fs::remove_dir_all(&path).is_ok() {
            removed += 1;
        }
    }
    removed
}

#[cfg(unix)]
fn process_alive(pid: u32) -> bool {
    // SAFETY: signal 0 performs the existence check without delivering a signal.
    unsafe { libc::kill(pid as i32, 0) == 0 }
}

#[cfg(windows)]
fn process_alive(pid: u32) -> bool {
    // A dead pid's dir is swept; without a handle API in this crate, treat
    // unknown pids as alive so a live process is never robbed of its work dir.
    let _ = pid;
    true
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::job::Intent;
    use std::sync::Barrier;
    use std::sync::atomic::AtomicUsize;

    struct Fake {
        fail: bool,
        hits: Arc<AtomicUsize>,
    }

    impl EngineRunner for Fake {
        fn run(&self, _job: &Job, work: &WorkDir, _ctx: &JobCtx) -> Result<Vec<PathBuf>, JobError> {
            self.hits.fetch_add(1, Ordering::SeqCst);
            if self.fail {
                return Err(JobError::new(ErrorCode::EngineFailed, "boom").with_stderr("tail"));
            }
            let out = work.out().join("result.webp");
            std::fs::write(&out, b"artifact").unwrap();
            Ok(vec![out])
        }
    }

    fn queue_with(runner: Arc<dyn EngineRunner>, temp: PathBuf) -> (Arc<Queue>, Receiver<Event>) {
        let (tx, rx) = bounded(1024);
        let cfg = QueueConfig {
            cpu_workers: 2,
            io_workers: 1,
            temp_root: Some(temp),
            ..Default::default()
        };
        let q = Queue::start(cfg, runner, Store::in_memory().unwrap(), tx);
        (q, rx)
    }

    fn wait_for<F: Fn(&Event) -> bool>(rx: &Receiver<Event>, pred: F) -> Event {
        let deadline = std::time::Instant::now() + Duration::from_secs(20);
        while std::time::Instant::now() < deadline {
            if let Ok(ev) = rx.recv_timeout(Duration::from_millis(500))
                && pred(&ev)
            {
                return ev;
            }
        }
        panic!("event never arrived");
    }

    #[test]
    fn publishes_next_to_the_source_without_touching_it() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("photo.jpg");
        std::fs::write(&src, b"source-bytes").unwrap();
        let (q, rx) = queue_with(
            Arc::new(Fake {
                fail: false,
                hits: Arc::new(AtomicUsize::new(0)),
            }),
            dir.path().join("tmp"),
        );
        let job = Job::new(src.clone(), Intent::convert("WEBP").unwrap());
        q.submit(job).unwrap();
        let ev = wait_for(&rx, |e| matches!(e, Event::Finished { .. }));
        let Event::Finished { outputs, .. } = ev else {
            unreachable!()
        };
        assert_eq!(outputs.len(), 1);
        assert_eq!(outputs[0], dir.path().join("photo.webp"));
        assert_eq!(std::fs::read(&src).unwrap(), b"source-bytes");
        assert_eq!(std::fs::read(&outputs[0]).unwrap(), b"artifact");
        // Work dir removed.
        let leftover = std::fs::read_dir(dir.path().join("tmp")).unwrap().count();
        assert_eq!(leftover, 0);
        q.shutdown();
    }

    #[test]
    fn duplicate_submit_returns_the_live_id() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("a.png");
        std::fs::write(&src, b"x").unwrap();
        let hits = Arc::new(AtomicUsize::new(0));
        let (q, rx) = queue_with(
            Arc::new(Fake {
                fail: false,
                hits: hits.clone(),
            }),
            dir.path().join("tmp"),
        );
        // Hold both submissions before the first finishes by using a barrier-free
        // approach: submit twice in a row.
        let (id1, dup1) = q
            .submit(Job::new(src.clone(), Intent::convert("WEBP").unwrap()))
            .unwrap();
        let (id2, dup2) = q
            .submit(Job::new(src.clone(), Intent::convert("WEBP").unwrap()))
            .unwrap();
        assert!(!dup1);
        assert!(dup2, "second submit must be recognised as a duplicate");
        assert_eq!(id1, id2);
        wait_for(&rx, |e| matches!(e, Event::Finished { .. }));
        assert_eq!(hits.load(Ordering::SeqCst), 1);
        // After completion the same intent can run again.
        let (_, dup3) = q
            .submit(Job::new(src, Intent::convert("WEBP").unwrap()))
            .unwrap();
        assert!(!dup3);
        q.shutdown();
    }

    #[test]
    fn failure_is_reported_with_code_and_tail() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("a.png");
        std::fs::write(&src, b"x").unwrap();
        let (q, rx) = queue_with(
            Arc::new(Fake {
                fail: true,
                hits: Arc::new(AtomicUsize::new(0)),
            }),
            dir.path().join("tmp"),
        );
        q.submit(Job::new(src, Intent::convert("WEBP").unwrap()))
            .unwrap();
        let Event::Failed { error, .. } = wait_for(&rx, |e| matches!(e, Event::Failed { .. }))
        else {
            unreachable!()
        };
        assert_eq!(error.code, ErrorCode::EngineFailed);
        assert_eq!(error.stderr_tail.as_deref(), Some("tail"));
        q.shutdown();
    }

    struct Slow {
        started: Arc<AtomicUsize>,
    }

    impl EngineRunner for Slow {
        fn run(&self, _job: &Job, work: &WorkDir, ctx: &JobCtx) -> Result<Vec<PathBuf>, JobError> {
            self.started.fetch_add(1, Ordering::SeqCst);
            let partial = work.out().join("half.webp");
            std::fs::write(&partial, b"partial").unwrap();
            for _ in 0..600 {
                ctx.checkpoint()?;
                std::thread::sleep(Duration::from_millis(10));
            }
            Ok(vec![partial])
        }
    }

    #[test]
    fn cancelling_mid_run_leaves_no_artifact_and_no_work_dir() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("slow.png");
        std::fs::write(&src, b"x").unwrap();
        let started = Arc::new(AtomicUsize::new(0));
        let temp_root = dir.path().join("tmp");
        let (q, rx) = queue_with(
            Arc::new(Slow {
                started: started.clone(),
            }),
            temp_root.clone(),
        );
        let (id, _) = q
            .submit(Job::new(src.clone(), Intent::convert("WEBP").unwrap()))
            .unwrap();

        // Wait until the engine is actually running, then cancel.
        let deadline = std::time::Instant::now() + Duration::from_secs(10);
        while started.load(Ordering::SeqCst) == 0 && std::time::Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert_eq!(started.load(Ordering::SeqCst), 1);
        q.cancel(&id);

        wait_for(&rx, |e| matches!(e, Event::Cancelled { .. }));
        assert!(
            !dir.path().join("slow.webp").exists(),
            "a cancelled job must publish nothing"
        );
        assert!(!dir.path().join("slow-converted.webp").exists());
        let leftover: Vec<_> = std::fs::read_dir(&temp_root)
            .map(|read| read.flatten().map(|e| e.path()).collect())
            .unwrap_or_default();
        assert!(leftover.is_empty(), "work dirs left behind: {leftover:?}");
        assert_eq!(
            std::fs::read(&src).unwrap(),
            b"x",
            "the source is untouched"
        );
        q.shutdown();
    }

    #[test]
    fn concurrent_jobs_all_complete() {
        let dir = tempfile::tempdir().unwrap();
        let barrier = Arc::new(Barrier::new(2));
        let _ = barrier;
        let hits = Arc::new(AtomicUsize::new(0));
        let (q, rx) = queue_with(
            Arc::new(Fake {
                fail: false,
                hits: hits.clone(),
            }),
            dir.path().join("tmp"),
        );
        for i in 0..6 {
            let src = dir.path().join(format!("f{i}.png"));
            std::fs::write(&src, b"x").unwrap();
            q.submit(Job::new(src, Intent::convert("WEBP").unwrap()))
                .unwrap();
        }
        let mut done = 0;
        while done < 6 {
            if let Ok(Event::Finished { .. }) = rx.recv_timeout(Duration::from_secs(20)) {
                done += 1;
            }
        }
        assert_eq!(hits.load(Ordering::SeqCst), 6);
        q.shutdown();
    }

    #[test]
    fn sweep_removes_dead_process_dirs_and_keeps_live_ones() {
        let root = tempfile::tempdir().unwrap();
        let dead = root
            .path()
            .join(format!("{}-999999-0", crate::identity::TMP_PREFIX));
        std::fs::create_dir_all(dead.join("src")).unwrap();
        let live = root.path().join(format!(
            "{}-{}-0",
            crate::identity::TMP_PREFIX,
            std::process::id()
        ));
        std::fs::create_dir_all(live.join("src")).unwrap();
        let unrelated = root.path().join("other-dir");
        std::fs::create_dir_all(&unrelated).unwrap();

        assert_eq!(sweep_temp_dirs_in(root.path()), 1);
        assert!(!dead.exists(), "a dead process's work dir must be swept");
        assert!(live.exists(), "a live process's work dir must be kept");
        assert!(unrelated.exists(), "unrelated entries must be left alone");
    }
}
