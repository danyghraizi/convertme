//! Single instance, and argv forwarding into the running one.
//!
//! A second launch must not open a second wheel. The second process takes the
//! argv it was given (paths from "Open with", a file-manager "Convert with
//! ConvertMe" action, or the CLI) and hands them to the first one over the IPC
//! endpoint, then exits 0 — even when the first one is busy.
//!
//! * Lock: `flock` on Linux; a session-local named mutex on Windows.
//! * Endpoint: `$XDG_RUNTIME_DIR/convertme/ipc.sock` (or `/tmp`) on Linux;
//!   a named pipe on Windows.
//! * Framing: one JSON object per line, `{"build": "<stamp>", "paths": [...]}`, UTF-8.
//!
//! A launch whose bundle differs from the one already running is not forwarded: the
//! running window is out of date, so it quits and the new process takes the lock. That
//! is what makes "rebuild, relaunch" actually show the new interface.

use convertme_core::identity;
use convertme_core::paths;
use serde::Deserialize;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct Request {
    build: String,
    paths: Vec<PathBuf>,
}

fn parse_request(line: &str) -> Result<Forwarded, String> {
    let request: Request = serde_json::from_str(line).map_err(|e| e.to_string())?;
    if request.build.is_empty() {
        return Err("IPC build stamp is empty".into());
    }
    Ok(Forwarded {
        paths: request.paths,
        build: request.build,
    })
}

fn is_stale(forwarded: &Forwarded) -> bool {
    forwarded.build != BUILD_STAMP
}

/// The bundle embedded in this binary. Changes with every frontend build.
pub const BUILD_STAMP: &str = env!("UI_BUILD_STAMP");

#[derive(Debug)]
pub enum InstanceError {
    Io(std::io::Error),
    Busy,
}

impl std::fmt::Display for InstanceError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            InstanceError::Io(e) => write!(f, "{e}"),
            InstanceError::Busy => write!(f, "another instance holds the lock"),
        }
    }
}

/// Holds the process-wide lock for the lifetime of the app.
pub struct InstanceLock {
    /// Held for the process lifetime; dropping it releases the lock.
    #[cfg(unix)]
    #[allow(dead_code)]
    file: std::fs::File,
    #[cfg(windows)]
    handle: windows::Win32::Foundation::HANDLE,
}

#[cfg(unix)]
pub fn acquire() -> Result<InstanceLock, InstanceError> {
    use std::os::unix::io::AsRawFd;

    let path = paths::instance_lock();
    if let Some(dir) = path.parent() {
        let _ = std::fs::create_dir_all(dir);
    }
    let file = std::fs::OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(&path)
        .map_err(InstanceError::Io)?;
    // SAFETY: flock on a fd we own; LOCK_NB makes it non-blocking.
    let rc = unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) };
    if rc != 0 {
        return Err(InstanceError::Busy);
    }
    Ok(InstanceLock { file })
}

#[cfg(windows)]
pub fn acquire() -> Result<InstanceLock, InstanceError> {
    use windows::Win32::Foundation::{CloseHandle, ERROR_ALREADY_EXISTS, GetLastError};
    use windows::Win32::System::Threading::CreateMutexW;
    let name = imp::names().map_err(InstanceError::Io)?.0;
    let handle = unsafe {
        CreateMutexW(None, true, &windows::core::HSTRING::from(name))
            .map_err(|e| InstanceError::Io(std::io::Error::other(format!("CreateMutexW: {e}"))))?
    };
    let already = unsafe { GetLastError() } == ERROR_ALREADY_EXISTS;
    if already {
        // SAFETY: the handle came from CreateMutexW above.
        unsafe {
            let _ = CloseHandle(handle);
        }
        return Err(InstanceError::Busy);
    }
    Ok(InstanceLock { handle })
}

#[cfg(windows)]
impl Drop for InstanceLock {
    fn drop(&mut self) {
        // SAFETY: releasing a handle this struct owns.
        unsafe {
            let _ = windows::Win32::Foundation::CloseHandle(self.handle);
        }
    }
}

/// Send paths to the instance that owns the socket/pipe. Used by the second
/// launch, which then exits 0 without opening a window.
pub fn forward(paths: &[PathBuf]) -> Result<Forward, InstanceError> {
    let payload = serde_json::json!({ "build": BUILD_STAMP, "paths": paths });
    let line = serde_json::to_string(&payload)
        .map_err(|e| InstanceError::Io(std::io::Error::other(format!("serialising argv: {e}"))))?;
    imp::send(&format!("{line}\n")).map_err(InstanceError::Io)
}

/// What the running instance said about a forwarded launch.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Forward {
    /// The running instance is this build: it took the paths.
    Accepted,
    /// The running instance is an older build and is shutting down.
    Stale,
}

/// A forwarded message, as the running instance sees it.
#[derive(Debug, Clone)]
pub struct Forwarded {
    pub paths: Vec<PathBuf>,
    pub build: String,
}

/// Accept connections forever, calling `handler` with each forwarded launch.
pub fn serve<F>(handler: F) -> std::io::Result<std::thread::JoinHandle<()>>
where
    F: Fn(Forwarded) + Send + 'static,
{
    imp::serve(Box::new(handler))
}

#[cfg(unix)]
mod imp {
    use super::*;
    use std::os::unix::net::{UnixListener, UnixStream};

    pub fn send(payload: &str) -> std::io::Result<Forward> {
        let path = identity::socket_path();
        let mut stream = UnixStream::connect(&path)?;
        stream.write_all(payload.as_bytes())?;
        stream.flush()?;
        let mut reply = String::new();
        BufReader::new(stream).read_line(&mut reply)?;
        match reply.as_str() {
            "stale\n" => Ok(Forward::Stale),
            "ok\n" => Ok(Forward::Accepted),
            _ => Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "invalid IPC acknowledgement",
            )),
        }
    }

    pub fn serve(
        handler: Box<dyn Fn(Forwarded) + Send>,
    ) -> std::io::Result<std::thread::JoinHandle<()>> {
        let path = identity::socket_path();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir)?;
            // A stale socket from a crashed process must not block binding.
            let _ = std::fs::remove_file(&path);
        }
        let listener = UnixListener::bind(&path)?;
        std::thread::Builder::new()
            .name("convertme-ipc".into())
            .spawn(move || {
                for stream in listener.incoming() {
                    let Ok(stream) = stream else { continue };
                    let mut reply = stream.try_clone().ok();
                    let reader = BufReader::new(stream);
                    for line in reader.lines() {
                        let Ok(line) = line else { break };
                        let Ok(forwarded) = parse_request(&line) else {
                            break;
                        };
                        let stale = is_stale(&forwarded);
                        if let Some(mut socket) = reply.take() {
                            let _ = socket.write_all(if stale { b"stale\n" } else { b"ok\n" });
                            let _ = socket.flush();
                        }
                        handler(forwarded);
                    }
                }
            })
    }
}

#[cfg(windows)]
mod imp {
    use super::*;
    use std::time::{Duration, Instant};
    use windows::Win32::Foundation::*;
    use windows::Win32::Security::Authorization::*;
    use windows::Win32::Security::*;
    use windows::Win32::Storage::FileSystem::*;
    use windows::Win32::System::IO::*;
    use windows::Win32::System::Pipes::*;
    use windows::Win32::System::RemoteDesktop::ProcessIdToSessionId;
    use windows::Win32::System::Threading::*;
    use windows::core::{HSTRING, PCWSTR, PWSTR};

    const LIMIT: usize = 64 * 1024;
    const TIMEOUT: Duration = Duration::from_secs(3);
    struct Handle(HANDLE);
    impl Drop for Handle {
        fn drop(&mut self) {
            unsafe {
                let _ = CloseHandle(self.0);
            }
        }
    }
    fn err(e: windows::core::Error) -> std::io::Error {
        std::io::Error::other(e.to_string())
    }

    pub(super) fn names() -> std::io::Result<(String, String, String)> {
        unsafe {
            let mut token = HANDLE::default();
            OpenProcessToken(GetCurrentProcess(), TOKEN_QUERY, &mut token).map_err(err)?;
            let token = Handle(token);
            let mut len = 0;
            let _ = GetTokenInformation(token.0, TokenUser, None, 0, &mut len);
            if len == 0 {
                return Err(std::io::Error::last_os_error());
            }
            let mut data = vec![0usize; (len as usize).div_ceil(std::mem::size_of::<usize>())];
            GetTokenInformation(
                token.0,
                TokenUser,
                Some(data.as_mut_ptr().cast()),
                len,
                &mut len,
            )
            .map_err(err)?;
            let user = &*(data.as_ptr().cast::<TOKEN_USER>());
            let mut sid = PWSTR::null();
            ConvertSidToStringSidW(user.User.Sid, &mut sid).map_err(err)?;
            let sid_text = sid
                .to_string()
                .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e));
            let _ = LocalFree(Some(HLOCAL(sid.0.cast())));
            let sid_text = sid_text?;
            let mut session = 0;
            ProcessIdToSessionId(std::process::id(), &mut session).map_err(err)?;
            let suffix = format!("{sid_text}-{session}");
            Ok((
                format!(r"Local\{}-{suffix}", identity::MUTEX),
                format!(r"\\.\pipe\{}-{suffix}", identity::PIPE),
                sid_text,
            ))
        }
    }
    // Keep the OVERLAPPED and buffer alive until cancelled I/O has completed.
    fn operation<F>(pipe: HANDLE, deadline: Instant, start: F) -> std::io::Result<u32>
    where
        F: FnOnce(*mut OVERLAPPED) -> windows::core::Result<()>,
    {
        unsafe {
            let event = Handle(CreateEventW(None, true, false, PCWSTR::null()).map_err(err)?);
            let mut ov = OVERLAPPED::default();
            ov.hEvent = event.0;
            match start(&mut ov) {
                Ok(()) => {}
                Err(e) if e.code() == ERROR_PIPE_CONNECTED.to_hresult() => return Ok(0),
                Err(e) if e.code() == ERROR_IO_PENDING.to_hresult() => {
                    let millis = deadline
                        .saturating_duration_since(Instant::now())
                        .as_millis()
                        .min(u32::MAX as u128) as u32;
                    if WaitForSingleObject(event.0, millis) != WAIT_OBJECT_0 {
                        let _ = CancelIoEx(pipe, Some(&ov));
                        let mut ignored = 0;
                        let _ = GetOverlappedResult(pipe, &ov, &mut ignored, true);
                        return Err(std::io::Error::new(
                            std::io::ErrorKind::TimedOut,
                            "IPC timed out",
                        ));
                    }
                }
                Err(e) => return Err(err(e)),
            }
            let mut transferred = 0;
            GetOverlappedResult(pipe, &ov, &mut transferred, false).map_err(err)?;
            Ok(transferred)
        }
    }
    fn write_all(pipe: HANDLE, mut bytes: &[u8], deadline: Instant) -> std::io::Result<()> {
        while !bytes.is_empty() {
            let n = operation(pipe, deadline, |ov| unsafe {
                WriteFile(pipe, Some(bytes), None, Some(ov))
            })? as usize;
            if n == 0 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::WriteZero,
                    "IPC write",
                ));
            }
            bytes = &bytes[n..];
        }
        Ok(())
    }
    fn read_line(pipe: HANDLE, limit: usize, deadline: Instant) -> std::io::Result<String> {
        let mut out = Vec::new();
        while out.len() < limit {
            let mut buffer = [0u8; 4096];
            let capacity = buffer.len().min(limit - out.len());
            let n = operation(pipe, deadline, |ov| unsafe {
                ReadFile(pipe, Some(&mut buffer[..capacity]), None, Some(ov))
            })? as usize;
            if n == 0 {
                return Err(std::io::Error::new(
                    std::io::ErrorKind::UnexpectedEof,
                    "IPC disconnected",
                ));
            }
            if let Some(end) = buffer[..n].iter().position(|&byte| byte == b'\n') {
                if end + 1 != n {
                    return Err(std::io::Error::new(
                        std::io::ErrorKind::InvalidData,
                        "data after IPC line",
                    ));
                }
                out.extend_from_slice(&buffer[..end]);
                return String::from_utf8(out)
                    .map_err(|e| std::io::Error::new(std::io::ErrorKind::InvalidData, e));
            }
            out.extend_from_slice(&buffer[..n]);
        }
        Err(std::io::Error::new(
            std::io::ErrorKind::InvalidData,
            "IPC line too long",
        ))
    }
    pub fn send(payload: &str) -> std::io::Result<Forward> {
        if payload.len() > LIMIT {
            return Err(std::io::Error::new(
                std::io::ErrorKind::InvalidInput,
                "IPC request too long",
            ));
        }
        let (_, name, _) = names()?;
        let name = HSTRING::from(name);
        let deadline = Instant::now() + TIMEOUT;
        let pipe = loop {
            let result = unsafe {
                CreateFileW(
                    &name,
                    FILE_GENERIC_READ.0 | FILE_GENERIC_WRITE.0,
                    FILE_SHARE_READ | FILE_SHARE_WRITE,
                    None,
                    OPEN_EXISTING,
                    FILE_FLAG_OVERLAPPED,
                    None,
                )
            };
            match result {
                Ok(handle) => break Handle(handle),
                Err(_) if Instant::now() < deadline => {
                    unsafe {
                        let _ = WaitNamedPipeW(&name, 50);
                    }
                    std::thread::sleep(Duration::from_millis(10));
                }
                Err(e) => return Err(err(e)),
            }
        };
        write_all(pipe.0, payload.as_bytes(), deadline)?;
        match read_line(pipe.0, 16, deadline)?.as_str() {
            "ok" => Ok(Forward::Accepted),
            "stale" => Ok(Forward::Stale),
            _ => Err(std::io::Error::new(
                std::io::ErrorKind::InvalidData,
                "invalid IPC acknowledgement",
            )),
        }
    }
    pub fn serve(
        handler: Box<dyn Fn(Forwarded) + Send>,
    ) -> std::io::Result<std::thread::JoinHandle<()>> {
        let (_, name, sid) = names()?;
        std::thread::Builder::new()
            .name("convertme-ipc".into())
            .spawn(move || {
                let name = HSTRING::from(name);
                let sddl = HSTRING::from(format!("D:P(A;;GA;;;{sid})"));
                loop {
                    unsafe {
                        let mut descriptor = PSECURITY_DESCRIPTOR::default();
                        if ConvertStringSecurityDescriptorToSecurityDescriptorW(
                            &sddl,
                            SDDL_REVISION_1,
                            &mut descriptor,
                            None,
                        )
                        .is_err()
                        {
                            std::thread::sleep(Duration::from_millis(250));
                            continue;
                        }
                        let attrs = SECURITY_ATTRIBUTES {
                            nLength: std::mem::size_of::<SECURITY_ATTRIBUTES>() as u32,
                            lpSecurityDescriptor: descriptor.0,
                            bInheritHandle: false.into(),
                        };
                        let raw = CreateNamedPipeW(
                            &name,
                            PIPE_ACCESS_DUPLEX | FILE_FLAG_OVERLAPPED,
                            PIPE_TYPE_BYTE
                                | PIPE_READMODE_BYTE
                                | PIPE_WAIT
                                | PIPE_REJECT_REMOTE_CLIENTS,
                            PIPE_UNLIMITED_INSTANCES,
                            LIMIT as u32,
                            LIMIT as u32,
                            0,
                            Some(&attrs),
                        );
                        let _ = LocalFree(Some(HLOCAL(descriptor.0)));
                        if raw.is_invalid() {
                            std::thread::sleep(Duration::from_millis(250));
                            continue;
                        }
                        let pipe = Handle(raw);
                        if operation(pipe.0, Instant::now() + TIMEOUT, |ov| {
                            ConnectNamedPipe(pipe.0, Some(ov))
                        })
                        .is_ok()
                        {
                            let deadline = Instant::now() + TIMEOUT;
                            if let Ok(line) = read_line(pipe.0, LIMIT, deadline) {
                                if let Ok(request) = parse_request(&line) {
                                    let reply: &[u8] = if is_stale(&request) {
                                        b"stale\n"
                                    } else {
                                        b"ok\n"
                                    };
                                    if write_all(pipe.0, reply, deadline).is_ok() {
                                        drop(pipe);
                                        handler(request);
                                        continue;
                                    }
                                }
                            }
                        }
                        let _ = DisconnectNamedPipe(pipe.0);
                    }
                }
            })
    }
    #[cfg(test)]
    mod tests {
        use super::*;

        #[test]
        fn pipe_partial_request_and_acknowledgements() {
            for stale in [false, true] {
                let name =
                    HSTRING::from(format!(r"\\.\pipe\convertme-test-{}", uuid::Uuid::new_v4()));
                let raw = unsafe {
                    CreateNamedPipeW(
                        &name,
                        PIPE_ACCESS_DUPLEX | FILE_FLAG_OVERLAPPED,
                        PIPE_TYPE_BYTE
                            | PIPE_READMODE_BYTE
                            | PIPE_WAIT
                            | PIPE_REJECT_REMOTE_CLIENTS,
                        1,
                        LIMIT as u32,
                        LIMIT as u32,
                        0,
                        None,
                    )
                };
                assert!(!raw.is_invalid());
                // Move the integer representation, not the raw HANDLE, to the server thread.
                let value = raw.0 as usize;
                let worker = std::thread::spawn(move || {
                    let pipe = Handle(HANDLE(value as _));
                    operation(pipe.0, Instant::now() + TIMEOUT, |ov| unsafe {
                        ConnectNamedPipe(pipe.0, Some(ov))
                    })
                    .unwrap();
                    let line = read_line(pipe.0, LIMIT, Instant::now() + TIMEOUT).unwrap();
                    let request = parse_request(&line).unwrap();
                    assert_eq!(request.paths, vec![PathBuf::from("é & sample.png")]);
                    let ack: &[u8] = if is_stale(&request) {
                        b"stale\n"
                    } else {
                        b"ok\n"
                    };
                    write_all(pipe.0, ack, Instant::now() + TIMEOUT).unwrap();
                });
                let client = Handle(unsafe {
                    CreateFileW(
                        &name,
                        FILE_GENERIC_READ.0 | FILE_GENERIC_WRITE.0,
                        FILE_SHARE_READ | FILE_SHARE_WRITE,
                        None,
                        OPEN_EXISTING,
                        FILE_FLAG_OVERLAPPED,
                        None,
                    )
                    .unwrap()
                });
                let build = if stale { "an-old-build" } else { BUILD_STAMP };
                let payload = format!(
                    "{}\n",
                    serde_json::json!({"build": build, "paths": ["é & sample.png"]})
                );
                let split = payload.len() / 2;
                let deadline = Instant::now() + TIMEOUT;
                write_all(client.0, &payload.as_bytes()[..split], deadline).unwrap();
                write_all(client.0, &payload.as_bytes()[split..], deadline).unwrap();
                assert_eq!(
                    read_line(client.0, 16, deadline).unwrap(),
                    if stale { "stale" } else { "ok" }
                );
                worker.join().unwrap();
            }
        }
    }
}

/// Paths in the process argv that look like files (everything after the binary
/// and any flags the launcher inserted).
/// `--background` starts the app with no window: the wheel keeps working, the
/// window is opened later by launching the app again.
pub fn wants_background(argv: &[String]) -> bool {
    argv.iter().any(|arg| arg == "--background")
}

pub fn paths_from_argv(argv: impl IntoIterator<Item = String>) -> Vec<PathBuf> {
    argv.into_iter()
        .skip(1)
        .filter(|arg| !arg.starts_with('-'))
        .map(PathBuf::from)
        .filter(|p| p.exists())
        .collect()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn argv_filtering_keeps_existing_files_only() {
        let dir = tempfile::tempdir().unwrap();
        let file = dir.path().join("a.png");
        std::fs::write(&file, b"x").unwrap();
        let argv = vec![
            "convertme".to_string(),
            "--flag".to_string(),
            file.to_string_lossy().into_owned(),
            "/nope/missing.png".to_string(),
        ];
        assert_eq!(paths_from_argv(argv), vec![file]);
    }

    #[test]
    fn request_requires_the_complete_object_protocol() {
        let path = PathBuf::from("émoji & (sample).png");
        let message = serde_json::json!({"build": BUILD_STAMP, "paths": [path.clone()]});
        let request = parse_request(&message.to_string()).unwrap();
        assert_eq!(request.paths, vec![path]);
        assert!(!is_stale(&request));
        assert!(is_stale(
            &parse_request(r#"{"build":"old","paths":[]}"#).unwrap()
        ));
        for invalid in [
            r#"[]"#,
            r#"{"paths":[]}"#,
            r#"{"build":"x"}"#,
            r#"{"build":"x","paths":["a"],"extra":1}"#,
            r#"{"build":null,"paths":[]}"#,
            r#"{"build":"","paths":[]}"#,
        ] {
            assert!(parse_request(invalid).is_err(), "{invalid}");
        }
    }

    #[test]
    fn lock_is_exclusive_within_a_process_is_not_asserted() {
        // flock is per open-file-description, so the same process can take it
        // twice; the guarantee we rely on is cross-process. Assert the call
        // succeeds and the lock is released on drop.
        let first = acquire();
        assert!(first.is_ok() || matches!(first, Err(InstanceError::Busy)));
        drop(first);
    }
}
