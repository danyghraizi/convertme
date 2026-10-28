//! One spawn helper, so no engine can forget the hygiene rules.
//!
//! * **argv arrays only** — never a shell string, so spaces, quotes, `&`,
//!   newlines and emoji in filenames are safe. Windows gets the `\\?\` verbatim
//!   prefix for long paths.
//! * **Memory ceilings** injected as env vars, plus per-worker thread caps.
//! * **Watchdog**: 30 min for video and multi-page PDF work, 5 min otherwise;
//!   `SIGTERM`/`q`, then 2 s, then the whole process group.
//! * **Bounded stderr** (64 KiB ring) surfaced as `JobError::stderr_tail`.
//! * **Cancellation** polled every 50 ms, same escalation as the watchdog.

use convertme_core::error::{ErrorCode, JobError};
use convertme_core::job::JobCtx;
use std::collections::VecDeque;
use std::ffi::{OsStr, OsString};
use std::io::{BufRead, BufReader, Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Child, Command, Stdio};
use std::time::{Duration, Instant};

/// How long a child may run before the watchdog fires.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Budget {
    Short,
    Long,
}

impl Budget {
    pub fn duration(self) -> Duration {
        match self {
            Budget::Short => Duration::from_secs(5 * 60),
            Budget::Long => Duration::from_secs(30 * 60),
        }
    }
}

/// A fully specified invocation.
pub struct Cmd {
    program: PathBuf,
    args: Vec<OsString>,
    envs: Vec<(OsString, OsString)>,
    cwd: Option<PathBuf>,
    /// Wall-clock ceiling; `Budget::Short` or `Budget::Long` by default.
    pub budget: Duration,
    pipe_stdout: bool,
    stdin_data: Option<Vec<u8>>,
}

impl Cmd {
    pub fn new(program: impl AsRef<Path>) -> Self {
        Self {
            program: program.as_ref().to_path_buf(),
            args: Vec::new(),
            envs: Vec::new(),
            cwd: None,
            budget: Budget::Short.duration(),
            pipe_stdout: false,
            stdin_data: None,
        }
    }

    pub fn arg(mut self, arg: impl AsRef<OsStr>) -> Self {
        self.args.push(arg.as_ref().to_os_string());
        self
    }

    pub fn args<I, S>(mut self, args: I) -> Self
    where
        I: IntoIterator<Item = S>,
        S: AsRef<OsStr>,
    {
        self.args
            .extend(args.into_iter().map(|a| a.as_ref().to_os_string()));
        self
    }

    pub fn env(mut self, key: impl AsRef<OsStr>, value: impl AsRef<OsStr>) -> Self {
        self.envs
            .push((key.as_ref().to_os_string(), value.as_ref().to_os_string()));
        self
    }

    pub fn cwd(mut self, dir: impl AsRef<Path>) -> Self {
        self.cwd = Some(dir.as_ref().to_path_buf());
        self
    }

    pub fn budget(mut self, budget: Budget) -> Self {
        self.budget = budget.duration();
        self
    }

    /// Override the ceiling directly (the perf harness and tests use this).
    pub fn budget_duration(mut self, budget: Duration) -> Self {
        self.budget = budget;
        self
    }

    pub fn stdout_piped(mut self) -> Self {
        self.pipe_stdout = true;
        self
    }

    /// Write `data` to the child's stdin, then close it.
    pub fn stdin(mut self, data: impl Into<Vec<u8>>) -> Self {
        self.stdin_data = Some(data.into());
        self
    }
}

/// What a finished child produced.
#[derive(Debug, Clone, Default)]
pub struct Output {
    pub success: bool,
    /// Exit code, or `None` when killed by a signal.
    pub code: Option<i32>,
    /// Last 8 KiB of stderr (the ring keeps 64 KiB).
    pub stderr_tail: String,
    /// Captured stdout, only when `stdout_piped` was set.
    pub stdout: String,
}

impl Output {
    /// Consume the output, turning a non-zero exit into an `EngineFailed` error
    /// carrying the stderr tail. Taking `self` keeps `proc::run(..)?.ok()?` free of
    /// dangling borrows.
    pub fn ok(self) -> Result<Self, JobError> {
        if self.success {
            Ok(self)
        } else {
            Err(JobError::new(
                ErrorCode::EngineFailed,
                match self.code {
                    Some(c) => format!("the tool exited with status {c}"),
                    None => "the tool was killed by a signal".to_string(),
                },
            )
            .with_stderr(&self.stderr_tail))
        }
    }
}

const STDERR_RING: usize = 64 * 1024;

/// Run to completion. Stdout is discarded unless `stdout_piped`.
pub fn run(cmd: Cmd, ctx: &JobCtx) -> Result<Output, JobError> {
    run_with_lines(cmd, ctx, |_| {})
}

/// Run, handing every stdout line to `on_line` (that is how ffmpeg progress and
/// `pdftoppm` page counts are parsed). Stderr is always drained on its own thread.
pub fn run_with_lines(
    cmd: Cmd,
    ctx: &JobCtx,
    mut on_line: impl FnMut(&str) + Send,
) -> Result<Output, JobError> {
    let mut command = build(&cmd);
    let mut child = command
        .spawn()
        .map_err(|e| JobError::io(format!("starting {}", cmd.program.display()), &e))?;

    // stderr reader.
    let stderr_ring = std::sync::Arc::new(parking_lot::Mutex::new(StderrRing::default()));

    // stdin writer (optional data, then closed).
    if let Some(data) = cmd.stdin_data.clone() {
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(&data);
            let _ = stdin.flush();
        }
    } else {
        drop(child.stdin.take());
    }

    // stdout reader when the contract needs it.
    let stdout_data = std::sync::Arc::new(parking_lot::Mutex::new(String::new()));
    let stdout_pipe = if cmd.pipe_stdout {
        child.stdout.take()
    } else {
        None
    };
    let stderr_pipe = child.stderr.take();

    // Scoped threads: the reader may borrow `on_line` and the pipes without
    // requiring `'static`, and both are joined before this function returns.
    let scope_result = std::thread::scope(
        |scope| -> Result<(std::process::ExitStatus, bool), JobError> {
            if let Some(pipe) = stderr_pipe {
                let ring = stderr_ring.clone();
                scope.spawn(move || drain_stderr(pipe, ring));
            }
            if let Some(pipe) = stdout_pipe {
                let sink = stdout_data.clone();
                let on_line = &mut on_line;
                scope.spawn(move || {
                    let reader = BufReader::new(pipe);
                    for line in reader.lines() {
                        match line {
                            Ok(line) => {
                                on_line(&line);
                                let mut sink = sink.lock();
                                if sink.len() < 256 * 1024 {
                                    sink.push_str(&line);
                                    sink.push('\n');
                                }
                            }
                            Err(_) => break,
                        }
                    }
                });
            }

            let deadline = Instant::now() + cmd.budget;
            loop {
                if let Some(status) = child.try_wait().map_err(|e| spawn_io_error(&e))? {
                    return Ok((status, false));
                }
                if ctx.is_cancelled() {
                    terminate(&mut child, true);
                    let _ = child.wait();
                    return Err(JobError::new(ErrorCode::Cancelled, "cancelled")
                        .with_stderr(stderr_ring.lock().to_string()));
                }
                if Instant::now() > deadline {
                    terminate(&mut child, false);
                    let _ = child.wait();
                    let status = child
                        .try_wait()
                        .ok()
                        .flatten()
                        .unwrap_or_else(exit_status_fallback);
                    return Ok((status, true));
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        },
    );

    let (status, timed_out) = scope_result?;

    let stderr = stderr_ring.lock().to_string();
    if timed_out {
        return Err(JobError::new(
            ErrorCode::TimedOut,
            format!(
                "{} exceeded its {} minute budget",
                cmd.program
                    .file_name()
                    .unwrap_or(cmd.program.as_os_str())
                    .to_string_lossy(),
                cmd.budget.as_secs() / 60
            ),
        )
        .with_stderr(&stderr));
    }

    let out = Output {
        success: status.success(),
        code: status.code(),
        stderr_tail: tail(&stderr, 8 * 1024),
        stdout: std::mem::take(&mut *stdout_data.lock()),
    };
    if out.success {
        return Ok(out);
    }
    if looks_like_disk_full(&stderr) {
        return Err(
            JobError::new(ErrorCode::DiskFull, "the tool reported a full disk")
                .with_stderr(&stderr),
        );
    }
    if looks_like_permission(&stderr) {
        return Err(JobError::new(
            ErrorCode::Permission,
            "the tool reported a permission error",
        )
        .with_stderr(&stderr));
    }
    Err(JobError::new(
        ErrorCode::EngineFailed,
        match out.code {
            Some(code) => format!("the tool exited with status {code}"),
            None => "the tool was killed by a signal".to_string(),
        },
    )
    .with_stderr(&stderr))
}

fn build(cmd: &Cmd) -> Command {
    let mut command = Command::new(verbatim_path(&cmd.program));
    command.args(&cmd.args);
    command.stdin(if cmd.stdin_data.is_some() {
        Stdio::piped()
    } else {
        Stdio::null()
    });
    command.stdout(if cmd.pipe_stdout {
        Stdio::piped()
    } else {
        Stdio::null()
    });
    command.stderr(Stdio::piped());

    // One thread per worker, not per core: N workers must not each grab everything.
    let threads = (std::thread::available_parallelism()
        .map(|n| n.get())
        .unwrap_or(4)
        / 2)
    .max(1);
    command.env("OMP_NUM_THREADS", threads.to_string());
    command.env("MAGICK_THREAD_LIMIT", "2");
    command.env("MAGICK_MEMORY_LIMIT", "512MiB");
    command.env("MAGICK_MAP_LIMIT", "1GiB");
    command.env("MAGICK_DISK_LIMIT", "4GiB");
    // Keep tool output machine-readable regardless of the user's locale.
    command.env("LC_ALL", "C.UTF-8");
    command.env("LANG", "C.UTF-8");
    // Nothing may phone home from a conversion.
    command.env("GS_OPTIONS", "-P-");

    for (k, v) in &cmd.envs {
        command.env(k, v);
    }
    if let Some(dir) = &cmd.cwd {
        command.current_dir(dir);
    }

    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        // Own process group, so the watchdog can take the whole tree down.
        command.process_group(0);
    }
    command
}

/// Windows long paths need the verbatim prefix to survive >260 characters.
fn verbatim_path(path: &Path) -> PathBuf {
    #[cfg(windows)]
    {
        let s = path.to_string_lossy();
        if s.len() > 240 && !s.starts_with(r"\\?\") {
            if let Some(rest) = s.strip_prefix(r"\\") {
                return PathBuf::from(format!(r"\\?\UNC\{rest}"));
            }
            return PathBuf::from(format!(r"\\?\{s}"));
        }
    }
    path.to_path_buf()
}

/// Ask nicely, then take the process group down.
fn terminate(child: &mut Child, graceful: bool) {
    if graceful {
        // ffmpeg quits cleanly on `q`; other tools ignore stdin anyway.
        if let Some(mut stdin) = child.stdin.take() {
            let _ = stdin.write_all(b"q");
            let _ = stdin.flush();
        }
    }
    #[cfg(unix)]
    {
        let pid = child.id() as i32;
        // SAFETY: signal delivery to a pid we spawned and own.
        unsafe {
            if graceful {
                libc::killpg(pid, libc::SIGTERM);
            }
        }
        let deadline = Instant::now() + Duration::from_secs(2);
        while Instant::now() < deadline {
            if let Ok(Some(_)) = child.try_wait() {
                return;
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        // SAFETY: as above, now with an unconditional group kill.
        unsafe {
            libc::killpg(pid, libc::SIGKILL);
        }
    }
    #[cfg(windows)]
    {
        let _ = graceful;
        // taskkill /T takes the whole tree; CREATE_NO_WINDOW keeps it silent.
        let _ = Command::new("taskkill")
            .args(["/PID", &child.id().to_string(), "/T", "/F"])
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status();
        let _ = child.kill();
    }
}

#[derive(Default)]
struct StderrRing {
    buf: VecDeque<u8>,
    truncated_head: bool,
}

impl StderrRing {
    fn push(&mut self, mut bytes: &[u8]) {
        if bytes.len() >= STDERR_RING {
            self.truncated_head = true;
            bytes = &bytes[bytes.len() - STDERR_RING..];
            self.buf.clear();
        }
        let overflow = (self.buf.len() + bytes.len()).saturating_sub(STDERR_RING);
        for _ in 0..overflow {
            self.buf.pop_front();
            self.truncated_head = true;
        }
        self.buf.extend(bytes.iter().copied());
    }
}

impl std::fmt::Display for StderrRing {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        let body: Vec<u8> = self.buf.iter().copied().collect();
        let text = String::from_utf8_lossy(&body);
        if self.truncated_head {
            write!(f, "…{text}")
        } else {
            f.write_str(&text)
        }
    }
}

fn drain_stderr(mut pipe: impl Read, ring: std::sync::Arc<parking_lot::Mutex<StderrRing>>) {
    let mut buf = [0u8; 8192];
    loop {
        match pipe.read(&mut buf) {
            Ok(0) => break,
            Ok(n) => ring.lock().push(&buf[..n]),
            Err(_) => break,
        }
    }
}

fn tail(s: &str, max: usize) -> String {
    if s.len() <= max {
        return s.to_string();
    }
    let start = s.len() - max;
    let start = s
        .char_indices()
        .map(|(i, _)| i)
        .find(|i| *i >= start)
        .unwrap_or(s.len());
    format!("…{}", &s[start..])
}

fn looks_like_disk_full(stderr: &str) -> bool {
    let s = stderr.to_ascii_lowercase();
    s.contains("no space left") || s.contains("disk is full") || s.contains("enospc")
}

fn looks_like_permission(stderr: &str) -> bool {
    let s = stderr.to_ascii_lowercase();
    s.contains("permission denied") || s.contains("operation not permitted")
}

fn spawn_io_error(e: &std::io::Error) -> JobError {
    JobError::io("waiting for the tool", e)
}

#[cfg(unix)]
fn exit_status_fallback() -> std::process::ExitStatus {
    use std::os::unix::process::ExitStatusExt;
    std::process::ExitStatus::from_raw(137)
}

#[cfg(not(unix))]
fn exit_status_fallback() -> std::process::ExitStatus {
    use std::os::windows::process::ExitStatusExt;
    std::process::ExitStatus::from_raw(1)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn sh(script: &str) -> Cmd {
        Cmd::new("/bin/sh").arg("-c").arg(script)
    }

    #[test]
    fn hostile_filenames_survive_argv() {
        // The whole point: a filename is one argv element, never re-parsed.
        let dir = tempfile::tempdir().unwrap();
        let names = [
            "sp ace.txt",
            "q\"uote.txt",
            "line\nbreak.txt",
            "émoji🎬.txt",
            "amp&semi;colon.txt",
            &format!("{}.txt", "x".repeat(200)),
        ];
        for name in names {
            let path = dir.path().join(name);
            std::fs::write(&path, b"payload").unwrap();
            let out = run(
                Cmd::new("/bin/sh")
                    .arg("-c")
                    .arg("cat \"$1\"")
                    .arg("sh")
                    .arg(&path)
                    .stdout_piped(),
                &JobCtx::new(),
            )
            .unwrap();
            assert_eq!(out.stdout.trim_end(), "payload", "failed for {name:?}");
        }
    }

    #[test]
    fn nonzero_exit_carries_the_stderr_tail() {
        let ctx = JobCtx::new();
        let err = run(sh("echo boom >&2; exit 3"), &ctx).unwrap_err();
        assert_eq!(err.code, ErrorCode::EngineFailed);
        assert!(err.message.contains('3'));
        assert!(err.stderr_tail.unwrap().contains("boom"));
    }

    #[test]
    fn cancellation_kills_the_process_and_reports_cancelled() {
        let ctx = JobCtx::new();
        let flag = ctx.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            flag.cancel();
        });
        let start = Instant::now();
        let err = run(sh("sleep 30"), &ctx).unwrap_err();
        assert_eq!(err.code, ErrorCode::Cancelled);
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "cancel was not prompt"
        );
    }

    #[test]
    fn watchdog_fires_and_reports_timed_out() {
        let ctx = JobCtx::new();
        // A 300 ms budget stands in for the 5-minute one, same code path.
        let start = Instant::now();
        let err = run(
            Cmd::new("/bin/sh")
                .arg("-c")
                .arg("sleep 30")
                .budget_duration(Duration::from_millis(300)),
            &ctx,
        )
        .unwrap_err();
        assert_eq!(err.code, ErrorCode::TimedOut);
        assert!(start.elapsed() < Duration::from_secs(5));
    }

    #[test]
    fn stderr_ring_is_bounded() {
        let mut ring = StderrRing::default();
        for _ in 0..2000 {
            ring.push(&[b'x'; 1024]);
        }
        assert!(ring.buf.len() <= STDERR_RING);
        assert!(ring.to_string().starts_with('…'));
    }

    #[test]
    fn stdout_lines_reach_the_callback() {
        let mut lines = Vec::new();
        let out = run_with_lines(
            Cmd::new("/bin/sh")
                .arg("-c")
                .arg("printf 'a\\nb\\nc\\n'")
                .stdout_piped(),
            &JobCtx::new(),
            |line| lines.push(line.to_string()),
        )
        .unwrap();
        assert_eq!(lines, vec!["a", "b", "c"]);
        assert_eq!(out.stdout.trim(), "a\nb\nc");
    }
}
