//! File actions. These do not convert anything.
//!
//! `Copy` and `Move` are the only operations that touch a source path, and they run
//! only when the user picks them from the actions wheel — never from a conversion,
//! a tool or a batch. Everything else here is read-only.

use convertme_core::error::{ErrorCode, JobError};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};

pub fn run(
    app: &tauri::AppHandle,
    action: &str,
    paths: &[PathBuf],
    dest: Option<&Path>,
) -> Result<(), String> {
    if paths.is_empty() {
        return Err("nothing selected".into());
    }
    match action {
        "preview" => toggle_preview(app, &paths[0]),
        "copy-path" => {
            use tauri_plugin_clipboard_manager::ClipboardExt;
            let text = paths
                .iter()
                .map(|p| p.to_string_lossy().into_owned())
                .collect::<Vec<_>>()
                .join("\n");
            app.clipboard().write_text(text).map_err(|e| e.to_string())
        }
        "terminal" => open_terminal(&paths[0]).map_err(|e| e.to_string()),
        "copy" => {
            let dest = dest.ok_or_else(|| "choose a destination folder first".to_string())?;
            copy_many(paths, dest).map(|_| ()).map_err(|e| e.message)
        }
        "move" => {
            let dest = dest.ok_or_else(|| "choose a destination folder first".to_string())?;
            move_many(paths, dest).map(|_| ()).map_err(|e| e.message)
        }
        "share" => share(app, paths),
        other => Err(format!("unknown action {other:?}")),
    }
}

pub fn open_with_default(path: &Path) -> std::io::Result<()> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows::Win32::UI::Shell::ShellExecuteW;
        use windows::Win32::UI::WindowsAndMessaging::SW_SHOWNORMAL;
        use windows::core::{PCWSTR, w};
        let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        let result = unsafe {
            ShellExecuteW(
                None,
                w!("open"),
                PCWSTR(wide.as_ptr()),
                PCWSTR::null(),
                PCWSTR::null(),
                SW_SHOWNORMAL,
            )
        };
        if result.0 as isize <= 32 {
            Err(std::io::Error::other(format!(
                "ShellExecuteW failed: {}",
                result.0 as isize
            )))
        } else {
            Ok(())
        }
    }
    #[cfg(unix)]
    {
        Command::new("xdg-open")
            .arg(path)
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .spawn()?;
        Ok(())
    }
}

pub fn reveal(path: &Path) -> std::io::Result<()> {
    #[cfg(windows)]
    {
        use std::os::windows::ffi::OsStrExt;
        use windows::Win32::System::Com::CoTaskMemFree;
        use windows::Win32::UI::Shell::{SHOpenFolderAndSelectItems, SHParseDisplayName};
        use windows::core::PCWSTR;
        let wide: Vec<u16> = path.as_os_str().encode_wide().chain(Some(0)).collect();
        unsafe {
            let mut pidl = std::ptr::null_mut();
            SHParseDisplayName(PCWSTR(wide.as_ptr()), None, &mut pidl, 0, None)
                .map_err(|e| std::io::Error::other(e.to_string()))?;
            let result = SHOpenFolderAndSelectItems(pidl, None, 0)
                .map_err(|e| std::io::Error::other(e.to_string()));
            CoTaskMemFree(Some(pidl.cast()));
            result
        }
    }
    #[cfg(unix)]
    {
        let dir = path.parent().unwrap_or(path);
        open_with_default(dir)
    }
}

/// Open a terminal in the file's folder, using the user's own terminal when set.
pub fn open_terminal(path: &Path) -> std::io::Result<()> {
    let dir = path.parent().unwrap_or(Path::new("."));
    #[cfg(windows)]
    {
        if Command::new("wt").arg("-d").arg(dir).spawn().is_ok() {
            return Ok(());
        }
        Command::new("powershell")
            .args(["-NoExit", "-NoProfile"])
            .current_dir(dir)
            .spawn()?;
        Ok(())
    }
    #[cfg(unix)]
    {
        let candidates: Vec<String> = std::env::var("TERMINAL")
            .ok()
            .into_iter()
            .chain(
                [
                    "x-terminal-emulator",
                    "gnome-terminal",
                    "konsole",
                    "alacritty",
                    "kitty",
                    "xterm",
                ]
                .into_iter()
                .map(str::to_string),
            )
            .collect();
        for term in candidates {
            let mut command = Command::new(&term);
            match term.as_str() {
                "gnome-terminal" => {
                    command.arg("--working-directory").arg(dir);
                }
                _ => {
                    command.current_dir(dir);
                }
            }
            if command
                .stdout(Stdio::null())
                .stderr(Stdio::null())
                .spawn()
                .is_ok()
            {
                return Ok(());
            }
        }
        Err(std::io::Error::other("no terminal emulator found"))
    }
}

/// Copy into `dest`, never replacing anything there.
pub fn copy_many(paths: &[PathBuf], dest: &Path) -> Result<Vec<PathBuf>, JobError> {
    std::fs::create_dir_all(dest).map_err(|e| JobError::io("creating the destination", &e))?;
    let mut written = Vec::new();
    for path in paths {
        let target = unique_target(dest, path);
        if path.is_dir() {
            copy_dir(path, &target)?;
        } else {
            // Copy through a temp name so an interrupted copy leaves no partial file.
            let tmp = target.with_extension("convertme-partial");
            std::fs::copy(path, &tmp).map_err(|e| JobError::io("copying", &e))?;
            std::fs::rename(&tmp, &target).map_err(|e| JobError::io("finishing the copy", &e))?;
        }
        written.push(target);
    }
    Ok(written)
}

/// Move into `dest`. Same cross-device fallback as publishing.
pub fn move_many(paths: &[PathBuf], dest: &Path) -> Result<Vec<PathBuf>, JobError> {
    std::fs::create_dir_all(dest).map_err(|e| JobError::io("creating the destination", &e))?;
    let mut moved = Vec::new();
    for path in paths {
        let target = unique_target(dest, path);
        match std::fs::rename(path, &target) {
            Ok(()) => {}
            Err(e) if e.raw_os_error() == Some(cross_device_code()) => {
                if path.is_dir() {
                    copy_dir(path, &target)?;
                } else {
                    std::fs::copy(path, &target)
                        .map_err(|e| JobError::io("copying across filesystems", &e))?;
                }
                // Only remove the source once the copy is complete.
                if path.is_dir() {
                    std::fs::remove_dir_all(path)
                        .map_err(|e| JobError::io("removing the source folder", &e))?;
                } else {
                    std::fs::remove_file(path)
                        .map_err(|e| JobError::io("removing the source", &e))?;
                }
            }
            Err(e) => return Err(JobError::io("moving", &e)),
        }
        moved.push(target);
    }
    Ok(moved)
}

fn cross_device_code() -> i32 {
    #[cfg(unix)]
    {
        libc::EXDEV
    }
    #[cfg(windows)]
    {
        17
    }
    #[cfg(not(any(unix, windows)))]
    {
        0
    }
}

/// `name (2).ext` when the destination name is taken.
pub fn unique_target(dest: &Path, source: &Path) -> PathBuf {
    let name = source
        .file_name()
        .map(|n| n.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut candidate = dest.join(&name);
    if !candidate.exists() {
        return candidate;
    }
    let stem = source
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let ext = source.extension().map(|e| e.to_string_lossy().into_owned());
    let mut n = 2;
    loop {
        let name = match &ext {
            Some(ext) => format!("{stem} ({n}).{ext}"),
            None => format!("{stem} ({n})"),
        };
        candidate = dest.join(name);
        if !candidate.exists() {
            return candidate;
        }
        n += 1;
        if n > 9999 {
            return candidate;
        }
    }
}

fn copy_dir(from: &Path, to: &Path) -> Result<(), JobError> {
    std::fs::create_dir_all(to).map_err(|e| JobError::io("creating a folder", &e))?;
    for entry in walkdir::WalkDir::new(from).min_depth(1) {
        let entry =
            entry.map_err(|e| JobError::new(ErrorCode::Io, format!("walking {from:?}: {e}")))?;
        let rel = entry
            .path()
            .strip_prefix(from)
            .map_err(|_| JobError::new(ErrorCode::Io, "path escaped the source folder"))?;
        let target = to.join(rel);
        if entry.file_type().is_dir() {
            std::fs::create_dir_all(&target).map_err(|e| JobError::io("creating a folder", &e))?;
        } else {
            std::fs::copy(entry.path(), &target).map_err(|e| JobError::io("copying a file", &e))?;
        }
    }
    Ok(())
}

/// Always-on-top preview window. Reuses one window so repeated floats do not pile up.
pub fn toggle_preview(app: &tauri::AppHandle, path: &Path) -> Result<(), String> {
    use tauri::{Manager, WebviewUrl, WebviewWindowBuilder};
    let url = format!(
        "index.html?view=preview&path={}",
        percent_encode(&path.to_string_lossy())
    );
    if let Some(window) = app.get_webview_window("preview") {
        if window.is_visible().unwrap_or(false) {
            // Hiding a webview does not run its cleanup; a clip started here must not
            // keep playing behind a hidden preview window.
            if let Some(state) = app.try_state::<crate::state::AppState>() {
                state.audio.stop();
            }
            window.hide().map_err(|e| e.to_string())?;
            return Ok(());
        }
        if let Some(state) = app.try_state::<crate::state::AppState>() {
            state.audio.stop();
        }
        window
            .navigate(
                tauri::Url::parse(&format!("tauri://localhost/{url}"))
                    .map_err(|e| e.to_string())?,
            )
            .map_err(|e| e.to_string())?;
        window.show().map_err(|e| e.to_string())?;
        return Ok(());
    }
    WebviewWindowBuilder::new(app, "preview", WebviewUrl::App(url.into()))
        .title(
            path.file_name()
                .map(|n| n.to_string_lossy().into_owned())
                .unwrap_or_default(),
        )
        .inner_size(760.0, 560.0)
        .always_on_top(true)
        .decorations(true)
        .build()
        .map(|_| ())
        .map_err(|e| e.to_string())
}

fn percent_encode(s: &str) -> String {
    let mut out = String::with_capacity(s.len());
    for byte in s.as_bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'_' | b'.' | b'~' | b'/' => {
                out.push(*byte as char)
            }
            _ => out.push_str(&format!("%{byte:02X}")),
        }
    }
    out
}

/// Hand the selection to another application.
///
/// Windows: the file list goes on the clipboard as `CF_HDROP`, which is what a
/// receiving app (mail, chat, file manager) pastes. Linux has no equivalent
/// system-wide share sheet, so the action is omitted there.
#[cfg(windows)]
pub fn share(app: &tauri::AppHandle, paths: &[PathBuf]) -> Result<(), String> {
    use tauri::Manager;
    let window = app
        .get_webview_window("main")
        .or_else(|| app.webview_windows().into_values().next())
        .ok_or_else(|| "no window available to own the clipboard".to_string())?;
    let hwnd = window.hwnd().map_err(|e| e.to_string())?;
    clipboard_files::set(hwnd, paths)
}

#[cfg(not(windows))]
pub fn share(_app: &tauri::AppHandle, _paths: &[PathBuf]) -> Result<(), String> {
    Err("sharing is a Windows-only action".into())
}

#[cfg(windows)]
mod clipboard_files {
    use std::path::PathBuf;
    use windows::Win32::Foundation::{GlobalFree, HANDLE, HGLOBAL, HWND};
    use windows::Win32::System::DataExchange::{
        CloseClipboard, EmptyClipboard, OpenClipboard, SetClipboardData,
    };
    use windows::Win32::System::Memory::{GMEM_MOVEABLE, GlobalAlloc, GlobalLock, GlobalUnlock};
    use windows::Win32::System::Ole::CF_HDROP;
    use windows::Win32::UI::Shell::DROPFILES;

    pub fn set(hwnd: HWND, paths: &[PathBuf]) -> Result<(), String> {
        // The clipboard DROPFILES payload: header, then a double-NUL-terminated
        // list of wide paths.
        let mut list: Vec<u16> = Vec::new();
        for path in paths {
            use std::os::windows::ffi::OsStrExt;
            list.extend(path.as_os_str().encode_wide());
            list.push(0);
        }
        list.push(0);

        let header = std::mem::size_of::<DROPFILES>();
        let bytes = header + list.len() * 2;
        // SAFETY: all Win32 clipboard calls are serialised through OpenClipboard;
        // the allocation is handed to the OS by SetClipboardData on success.
        unsafe {
            let mem = GlobalAlloc(GMEM_MOVEABLE, bytes).map_err(|e| e.to_string())?;
            struct Allocation(HGLOBAL);
            impl Drop for Allocation {
                fn drop(&mut self) {
                    unsafe {
                        let _ = GlobalFree(Some(self.0));
                    }
                }
            }
            let allocation = Allocation(mem);
            let ptr = GlobalLock(mem);
            if ptr.is_null() {
                return Err("GlobalLock failed".into());
            }
            // The block is not zeroed by the allocator, and the header has fields we
            // do not set; a garbage `fNC`/`pt` would travel to the receiving app.
            std::ptr::write_bytes(ptr as *mut u8, 0, header);
            let drop = ptr as *mut DROPFILES;
            (*drop).pFiles = header as u32;
            (*drop).fWide = true.into();
            let dest = (ptr as *mut u8).add(header) as *mut u16;
            std::ptr::copy_nonoverlapping(list.as_ptr(), dest, list.len());
            let _ = GlobalUnlock(mem);

            OpenClipboard(Some(hwnd)).map_err(|e| e.to_string())?;
            let result = (|| -> Result<(), String> {
                EmptyClipboard().map_err(|e| e.to_string())?;
                SetClipboardData(CF_HDROP.0 as u32, Some(HANDLE(mem.0)))
                    .map_err(|e| e.to_string())?;
                std::mem::forget(allocation);
                Ok(())
            })();
            let _ = CloseClipboard();
            result
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn copy_never_replaces_and_keeps_directories_intact() {
        let dir = tempfile::tempdir().unwrap();
        let src_dir = dir.path().join("src");
        std::fs::create_dir_all(src_dir.join("nested")).unwrap();
        let a = src_dir.join("a.txt");
        std::fs::write(&a, b"one").unwrap();
        std::fs::write(src_dir.join("nested/b.txt"), b"two").unwrap();
        let dest = dir.path().join("dest");
        std::fs::create_dir_all(&dest).unwrap();
        std::fs::write(dest.join("a.txt"), b"existing").unwrap();

        let written = copy_many(std::slice::from_ref(&a), &dest).unwrap();
        assert_eq!(written[0].file_name().unwrap(), "a (2).txt");
        assert_eq!(std::fs::read(&a).unwrap(), b"one");
        assert_eq!(std::fs::read(&written[0]).unwrap(), b"one");

        let written = copy_many(std::slice::from_ref(&src_dir), &dest).unwrap();
        assert!(written[0].join("nested/b.txt").is_file());
        // Nothing partial survived.
        assert!(!dest.join("a.convertme-partial").exists());
    }

    #[test]
    fn move_relocates_and_leaves_nothing_behind() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("x.txt");
        std::fs::write(&src, b"payload").unwrap();
        let dest = dir.path().join("out");
        let moved = move_many(std::slice::from_ref(&src), &dest).unwrap();
        assert!(!src.exists());
        assert_eq!(std::fs::read(&moved[0]).unwrap(), b"payload");
    }

    #[test]
    fn percent_encoding_survives_spaces_and_unicode() {
        assert_eq!(percent_encode("/a b/é"), "/a%20b/%C3%A9");
    }
}
