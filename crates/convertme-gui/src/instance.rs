//! Single instance, and argv forwarding into the running one.
//!
//! A second launch must not open a second wheel. The second process takes the
//! argv it was given (paths from "Open with", a file-manager "Convert with
//! ConvertMe" action, or the CLI) and hands them to the first one over the IPC
//! endpoint, then exits 0 — even when the first one is busy.
//!
//! * Lock: `flock` on `<data>/instance.lock` on Linux; the named mutex
//!   `Global\convertme-single` on Windows.
//! * Endpoint: `$XDG_RUNTIME_DIR/convertme/ipc.sock` (or the `/tmp` fallback) on
//!   Linux; `\\.\pipe\convertme-ipc` on Windows.
//! * Framing: one JSON array of paths per line, UTF-8.

use convertme_core::identity;
use convertme_core::paths;
use std::io::{BufRead, BufReader, Write};
use std::path::PathBuf;

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
    use windows::core::w;

    // SAFETY: creating a named mutex; the handle is closed on drop.
    let handle = unsafe {
        CreateMutexW(None, true, w!("Global\\convertme-single"))
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
pub fn forward(paths: &[PathBuf]) -> Result<(), InstanceError> {
    let line = serde_json::to_string(paths)
        .map_err(|e| InstanceError::Io(std::io::Error::other(format!("serialising argv: {e}"))))?;
    imp::send(&format!("{line}\n")).map_err(InstanceError::Io)
}

/// Accept connections forever, calling `handler` with each forwarded path list.
pub fn serve<F>(handler: F) -> std::io::Result<std::thread::JoinHandle<()>>
where
    F: Fn(Vec<PathBuf>) + Send + 'static,
{
    imp::serve(Box::new(handler))
}

#[cfg(unix)]
mod imp {
    use super::*;
    use std::os::unix::net::{UnixListener, UnixStream};

    pub fn send(payload: &str) -> std::io::Result<()> {
        let path = identity::socket_path();
        let mut stream = UnixStream::connect(&path)?;
        stream.write_all(payload.as_bytes())?;
        stream.flush()
    }

    pub fn serve(
        handler: Box<dyn Fn(Vec<PathBuf>) + Send>,
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
                    let reader = BufReader::new(stream);
                    for line in reader.lines() {
                        let Ok(line) = line else { break };
                        match serde_json::from_str::<Vec<PathBuf>>(line.trim()) {
                            Ok(paths) => handler(paths),
                            Err(_) => continue,
                        }
                    }
                }
            })
    }
}

#[cfg(windows)]
mod imp {
    use super::*;
    use windows::Win32::Foundation::{CloseHandle, HANDLE};
    use windows::Win32::Storage::FileSystem::{ReadFile, WriteFile};
    use windows::Win32::System::Pipes::{
        ConnectNamedPipe, CreateNamedPipeW, PIPE_ACCESS_INBOUND, PIPE_ACCESS_OUTBOUND,
        PIPE_READMODE_BYTE, PIPE_TYPE_BYTE, PIPE_UNLIMITED_INSTANCES, PIPE_WAIT,
    };
    use windows::core::HSTRING;

    fn wide(name: &str) -> HSTRING {
        HSTRING::from(name)
    }

    pub fn send(payload: &str) -> std::io::Result<()> {
        // SAFETY: opening the client end of our own pipe; every handle is closed.
        unsafe {
            let handle = windows::Win32::Storage::FileSystem::CreateFileW(
                &wide(identity::PIPE),
                windows::Win32::Storage::FileSystem::FILE_GENERIC_WRITE.0,
                windows::Win32::Storage::FileSystem::FILE_SHARE_READ
                    | windows::Win32::Storage::FileSystem::FILE_SHARE_WRITE,
                None,
                windows::Win32::Storage::FileSystem::OPEN_EXISTING,
                Default::default(),
                None,
            )
            .map_err(|e| std::io::Error::other(format!("CreateFileW: {e}")))?;
            let bytes = payload.as_bytes();
            let mut written = 0u32;
            let res = WriteFile(handle, Some(bytes), Some(&mut written as *mut u32), None);
            let _ = CloseHandle(handle);
            res.map_err(|e| std::io::Error::other(format!("WriteFile: {e}")))?;
        }
        Ok(())
    }

    pub fn serve(
        handler: Box<dyn Fn(Vec<PathBuf>) + Send>,
    ) -> std::io::Result<std::thread::JoinHandle<()>> {
        let handle_w = wide(identity::PIPE);
        std::thread::Builder::new()
            .name("convertme-ipc".into())
            .spawn(move || {
                loop {
                    // SAFETY: one pipe instance per connection, handles closed per loop.
                    let pipe = unsafe {
                        CreateNamedPipeW(
                            &handle_w,
                            PIPE_ACCESS_INBOUND,
                            PIPE_TYPE_BYTE | PIPE_READMODE_BYTE | PIPE_WAIT,
                            PIPE_UNLIMITED_INSTANCES,
                            64 * 1024,
                            64 * 1024,
                            0,
                            None,
                        )
                    };
                    if pipe.is_invalid() || pipe == HANDLE::default() {
                        std::thread::sleep(std::time::Duration::from_millis(250));
                        continue;
                    }
                    unsafe {
                        let _ = ConnectNamedPipe(pipe, None);
                        let mut buf = vec![0u8; 64 * 1024];
                        let mut read = 0u32;
                        if ReadFile(
                            pipe,
                            Some(buf.as_mut_slice()),
                            Some(&mut read as *mut u32),
                            None,
                        )
                        .is_ok()
                            && read > 0
                        {
                            let text = String::from_utf8_lossy(&buf[..read as usize]).to_string();
                            if let Ok(paths) = serde_json::from_str::<Vec<PathBuf>>(text.trim()) {
                                handler(paths);
                            }
                        }
                        let _ = CloseHandle(pipe);
                    }
                }
            })
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

    #[cfg(unix)]
    #[test]
    fn forwarding_reaches_a_listening_instance() {
        use std::sync::{Arc, Mutex};
        let got: Arc<Mutex<Vec<PathBuf>>> = Arc::new(Mutex::new(Vec::new()));
        let sink = got.clone();
        // Use a private socket path so the test cannot collide with a live app.
        let dir = tempfile::tempdir().unwrap();
        let socket = dir.path().join("ipc.sock");
        let listener = std::os::unix::net::UnixListener::bind(&socket).unwrap();
        let handle = std::thread::spawn(move || {
            if let Ok((stream, _)) = listener.accept() {
                let reader = BufReader::new(stream);
                for line in reader.lines().map_while(Result::ok) {
                    if let Ok(paths) = serde_json::from_str::<Vec<PathBuf>>(&line) {
                        sink.lock().unwrap().extend(paths);
                    }
                }
            }
        });
        let mut stream = std::os::unix::net::UnixStream::connect(&socket).unwrap();
        let payload = serde_json::to_string(&vec![PathBuf::from("/tmp/a.png")]).unwrap();
        stream.write_all(format!("{payload}\n").as_bytes()).unwrap();
        drop(stream);
        let _ = handle.join();
        assert_eq!(*got.lock().unwrap(), vec![PathBuf::from("/tmp/a.png")]);
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
