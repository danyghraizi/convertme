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

/// Keep helper processes from creating a console window on Windows.
pub fn configure_background(command: &mut Command) {
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(windows::Win32::System::Threading::CREATE_NO_WINDOW.0);
    }
    #[cfg(not(windows))]
    {
        let _ = command;
    }
}

/// Point portable helper processes at the data trees shipped beside them.
pub fn configure_tool_environment(command: &mut Command, program: &Path) {
    #[cfg(windows)]
    {
        let Some(root) = program
            .ancestors()
            .find(|dir| dir.join("manifest.json").is_file())
        else {
            return;
        };
        let Ok(manifest) = std::fs::read(root.join("manifest.json")) else {
            return;
        };
        let Ok(value) = serde_json::from_slice::<serde_json::Value>(&manifest) else {
            return;
        };
        let Some(resources) = value.get("resource_dirs").and_then(|v| v.as_object()) else {
            return;
        };
        let resource = |name: &str| -> Option<PathBuf> {
            let relative = resources.get(name)?.as_str()?;
            let path = root.join(relative);
            path.is_dir().then_some(path)
        };
        if let Some(path) = resource("tessdata") {
            command.env("TESSDATA_PREFIX", path);
        }
        if let Some(home) = resource("imagemagick_home") {
            command.env("MAGICK_HOME", &home);
            let configure = home.join("etc");
            if configure.is_dir() {
                command.env("MAGICK_CONFIGURE_PATH", configure);
            }
            let coders = home.join("modules").join("coders");
            if coders.is_dir() {
                command.env("MAGICK_CODER_MODULE_PATH", coders);
            }
        }
        if let Some(dirs) = resources.get("ghostscript_lib").and_then(|v| v.as_array()) {
            let paths: Vec<PathBuf> = dirs
                .iter()
                .filter_map(|value| value.as_str())
                .map(|relative| root.join(relative))
                .filter(|path| path.is_dir())
                .collect();
            if !paths.is_empty() {
                command.env("GS_LIB", std::env::join_paths(paths).unwrap_or_default());
            }
        }
    }
    #[cfg(target_os = "linux")]
    if let Ok(program_path) = std::fs::canonicalize(program) {
        if let Some(root) = program_path
            .ancestors()
            .find(|dir| dir.join("manifest.json").is_file())
        {
            configure_linux_tool_environment(command, &program_path, root);
        }
    }
    #[cfg(all(not(windows), not(target_os = "linux")))]
    {
        let _ = (command, program);
    }
}

#[cfg(target_os = "linux")]
fn configure_linux_tool_environment(command: &mut Command, program: &Path, root: &Path) {
    let Ok(root) = std::fs::canonicalize(root) else {
        return;
    };
    let Ok(program_path) = std::fs::canonicalize(program) else {
        return;
    };
    let Ok(bytes) = std::fs::read(root.join("manifest.json")) else {
        return;
    };
    let Ok(manifest) = serde_json::from_slice::<serde_json::Value>(&bytes) else {
        return;
    };
    if manifest.get("schema_version").and_then(|v| v.as_u64()) != Some(1)
        || manifest.get("helper_arch").and_then(|v| v.as_str()) != Some("x86_64")
    {
        return;
    }
    let Some(executables) = manifest.get("executables").and_then(|v| v.as_object()) else {
        return;
    };
    let Some((key, _)) = executables.iter().find(|(_, value)| {
        value
            .as_str()
            .and_then(|p| std::fs::canonicalize(root.join(p)).ok())
            .is_some_and(|p| p.starts_with(&root) && p == program_path)
    }) else {
        return;
    };
    let Some(dirs) = manifest
        .get("library_dirs")
        .and_then(|v| v.get(key))
        .and_then(|v| v.as_array())
    else {
        return;
    };
    let mut libraries = Vec::with_capacity(dirs.len());
    for dir in dirs {
        let Some(relative) = dir.as_str() else { return };
        let path = Path::new(relative);
        if path.as_os_str().is_empty()
            || path
                .components()
                .any(|c| !matches!(c, std::path::Component::Normal(_)))
        {
            return;
        }
        let Ok(path) = std::fs::canonicalize(root.join(path)) else {
            return;
        };
        if !path.starts_with(&root) || !path.is_dir() {
            return;
        }
        libraries.push(path);
    }
    let Ok(library_path) = std::env::join_paths(libraries) else {
        return;
    };
    command
        .env_remove("LD_PRELOAD")
        .env_remove("LD_LIBRARY_PATH");
    if !library_path.is_empty() {
        command.env("LD_LIBRARY_PATH", library_path);
    }
    let Some(name) = program.file_name().and_then(OsStr::to_str) else {
        return;
    };
    let share = root.join("runtime-data/usr/share");
    match name {
        "gs" => {
            let mut paths = Vec::new();
            if let Ok(versions) = std::fs::read_dir(share.join("ghostscript")) {
                for entry in versions.flatten() {
                    let version = entry.path();
                    if !version.is_dir() {
                        continue;
                    }
                    for subdir in ["Resource", "lib"] {
                        let path = version.join(subdir);
                        if path.is_dir() {
                            paths.push(path);
                        }
                    }
                }
            }
            let fonts = share.join("fonts/type1/urw-base35");
            if fonts.is_dir() {
                paths.push(fonts);
            }
            if !paths.is_empty()
                && let Ok(paths) = std::env::join_paths(paths)
            {
                command.env("GS_LIB", paths);
            }
        }
        "tesseract" => {
            let mut data_dirs = Vec::new();
            if let Ok(versions) = std::fs::read_dir(share.join("tesseract-ocr")) {
                for entry in versions.flatten() {
                    let data = entry.path().join("tessdata");
                    if data.join("eng.traineddata").is_file() {
                        data_dirs.push(data);
                    }
                }
            }
            if data_dirs.len() == 1 {
                command.env("TESSDATA_PREFIX", &data_dirs[0]);
            }
        }
        "pdftoppm" | "pdftotext" | "pdfinfo" => {
            let data = share.join("poppler");
            if data.is_dir() {
                command.env("POPPLER_DATADIR", data);
            }
        }
        _ => {}
    }
}

fn build(cmd: &Cmd) -> Command {
    let mut command = Command::new(verbatim_path(&cmd.program));
    configure_background(&mut command);
    configure_tool_environment(&mut command, &cmd.program);
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
    // Modest by design: this app must run on a 4 GB laptop. The ceilings keep a runaway
    // job from taking the machine down; anything that needs more than this has to be
    // made cheaper (see the note in docs/LESSONS.md), not given a bigger allowance.
    // 1 GiB of RAM + 2 GiB of mapping + 16 GiB of disk spill covers a 24 Mpx photo
    // comfortably, which is the common case.
    command.env("MAGICK_MEMORY_LIMIT", "1GiB");
    command.env("MAGICK_MAP_LIMIT", "2GiB");
    command.env("MAGICK_DISK_LIMIT", "16GiB");
    // Keep tool output machine-readable regardless of the user's locale.
    #[cfg(unix)]
    {
        command.env("LC_ALL", "C.UTF-8");
        command.env("LANG", "C.UTF-8");
    }
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
        let system_root =
            std::env::var_os("SystemRoot").unwrap_or_else(|| OsString::from(r"C:\Windows"));
        let taskkill = PathBuf::from(system_root)
            .join("System32")
            .join("taskkill.exe");
        let mut command = Command::new(taskkill);
        configure_background(&mut command);
        let _ = command
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

    fn child(mode: &str) -> Cmd {
        static CHILD: std::sync::LazyLock<(tempfile::TempDir, PathBuf)> =
            std::sync::LazyLock::new(|| {
                let dir = tempfile::tempdir().unwrap();
                let executable = dir.path().join(if cfg!(windows) {
                    "process_child.exe"
                } else {
                    "process_child"
                });
                let source =
                    Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/fixtures/process_child.rs");
                let status =
                    Command::new(std::env::var_os("RUSTC").unwrap_or_else(|| "rustc".into()))
                        .arg(source)
                        .arg("-o")
                        .arg(&executable)
                        .status()
                        .expect("starting native rustc");
                assert!(
                    status.success(),
                    "compiling process fixture failed: {status}"
                );
                (dir, executable)
            });
        Cmd::new(&CHILD.1).arg(mode)
    }

    #[test]
    fn hostile_filenames_survive_argv() {
        // The whole point: a filename is one argv element, never re-parsed.
        let dir = tempfile::tempdir().unwrap();
        let names = [
            "sp ace.txt",
            #[cfg(unix)]
            "q\"uote.txt",
            #[cfg(unix)]
            "line\nbreak.txt",
            "émoji🎬.txt",
            "amp&semi;colon.txt",
            &format!("{}.txt", "x".repeat(200)),
        ];
        for name in names {
            let path = dir.path().join(name);
            std::fs::write(&path, b"payload").unwrap();
            let out = run(child("echo-file").arg(&path).stdout_piped(), &JobCtx::new()).unwrap();
            assert_eq!(out.stdout.trim_end(), "payload", "failed for {name:?}");
        }
    }

    #[test]
    fn nonzero_exit_carries_the_stderr_tail() {
        let ctx = JobCtx::new();
        let err = run(child("stderr-exit"), &ctx).unwrap_err();
        assert_eq!(err.code, ErrorCode::EngineFailed);
        assert!(err.message.contains('3'));
        assert!(err.stderr_tail.unwrap().contains("boom"));
    }

    #[test]
    fn cancellation_kills_the_process_and_reports_cancelled() {
        let command = child("sleep");
        let ctx = JobCtx::new();
        let flag = ctx.clone();
        std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(300));
            flag.cancel();
        });
        let start = Instant::now();
        let err = run(command, &ctx).unwrap_err();
        assert_eq!(err.code, ErrorCode::Cancelled);
        assert!(
            start.elapsed() < Duration::from_secs(5),
            "cancel was not prompt"
        );
    }

    #[test]
    fn watchdog_fires_and_reports_timed_out() {
        let ctx = JobCtx::new();
        let command = child("sleep").budget_duration(Duration::from_millis(300));
        // A 300 ms budget stands in for the 5-minute one, same code path.
        let start = Instant::now();
        let err = run(command, &ctx).unwrap_err();
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
        let out = run_with_lines(child("lines").stdout_piped(), &JobCtx::new(), |line| {
            lines.push(line.to_string())
        })
        .unwrap();
        assert_eq!(lines, vec!["a", "b", "c"]);
        assert_eq!(out.stdout.trim(), "a\nb\nc");
    }
    #[test]
    fn cancellation_terminates_descendants() {
        let dir = tempfile::tempdir().unwrap();
        let marker = dir.path().join("descendant-alive");
        let command = child("spawn-descendant").arg(&marker).stdout_piped();
        let ctx = JobCtx::new();
        let cancel = ctx.clone();
        let mut observed_pid = false;
        let err = run_with_lines(command, &ctx, |line| {
            observed_pid = line.parse::<u32>().is_ok();
            cancel.cancel();
        })
        .unwrap_err();
        assert_eq!(err.code, ErrorCode::Cancelled);
        assert!(observed_pid, "child did not report its descendant");
        std::thread::sleep(Duration::from_secs(4));
        assert!(!marker.exists(), "descendant survived cancellation");
    }
    #[cfg(target_os = "linux")]
    #[test]
    fn linux_runtime_data_sets_tool_specific_paths() {
        let root = tempfile::tempdir().unwrap();
        let share = root.path().join("runtime-data/usr/share");
        let resource = share.join("ghostscript/10.02/Resource");
        let gs_lib = share.join("ghostscript/10.02/lib");
        let fonts = share.join("fonts/type1/urw-base35");
        let tessdata = share.join("tesseract-ocr/5/tessdata");
        let poppler = share.join("poppler");
        for path in [&resource, &gs_lib, &fonts, &tessdata, &poppler] {
            std::fs::create_dir_all(path).unwrap();
        }
        std::fs::write(tessdata.join("eng.traineddata"), b"data").unwrap();
        std::fs::create_dir_all(root.path().join("bin")).unwrap();
        std::fs::create_dir_all(root.path().join("lib")).unwrap();
        for name in ["gs", "tesseract", "pdftotext"] {
            std::fs::write(root.path().join("bin").join(name), b"tool").unwrap();
        }
        std::fs::write(root.path().join("manifest.json"), r#"{"schema_version":1,"helper_arch":"x86_64","executables":{"gs":"bin/gs","tesseract":"bin/tesseract","pdftotext":"bin/pdftotext"},"library_dirs":{"gs":["lib"],"tesseract":["lib"],"pdftotext":["lib"]}}"#).unwrap();

        let mut gs = Command::new("gs");
        configure_linux_tool_environment(&mut gs, &root.path().join("bin/gs"), root.path());
        let envs: Vec<_> = gs.get_envs().collect();
        assert!(
            envs.iter()
                .any(|(key, value)| *key == OsStr::new("LD_PRELOAD") && value.is_none())
        );
        assert!(
            envs.iter()
                .any(|(key, value)| *key == OsStr::new("LD_LIBRARY_PATH")
                    && value == &Some(root.path().join("lib").as_os_str()))
        );
        let gs_paths = gs
            .get_envs()
            .find_map(|(name, value)| {
                (name == OsStr::new("GS_LIB")).then(|| value.unwrap().to_os_string())
            })
            .unwrap();
        assert_eq!(
            std::env::split_paths(&gs_paths).collect::<Vec<_>>(),
            vec![resource, gs_lib, fonts]
        );

        let mut tesseract = Command::new("tesseract");
        configure_linux_tool_environment(
            &mut tesseract,
            &root.path().join("bin/tesseract"),
            root.path(),
        );
        assert_eq!(
            tesseract
                .get_envs()
                .find_map(|(name, value)| {
                    (name == OsStr::new("TESSDATA_PREFIX")).then(|| value.unwrap().to_os_string())
                })
                .unwrap(),
            tessdata.into_os_string()
        );

        let mut pdftotext = Command::new("pdftotext");
        configure_linux_tool_environment(
            &mut pdftotext,
            &root.path().join("bin/pdftotext"),
            root.path(),
        );
        assert_eq!(
            pdftotext
                .get_envs()
                .find_map(|(name, value)| {
                    (name == OsStr::new("POPPLER_DATADIR")).then(|| value.unwrap().to_os_string())
                })
                .unwrap(),
            poppler.into_os_string()
        );
    }
}
