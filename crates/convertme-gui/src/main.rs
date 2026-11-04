//! One binary, two front ends: `convertme <subcommand>` is the CLI, and no
//! arguments (or a dropped file) opens the window.
#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

fn main() {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let is_cli = args
        .first()
        .map(|a| convertme_gui::cli::is_cli_command(a))
        .unwrap_or(false);
    #[cfg(windows)]
    if is_cli {
        attach_parent_console();
    }

    let code = if is_cli {
        convertme_gui::cli::run(&args)
    } else {
        convertme_gui::app::run()
    };
    std::process::exit(code);
}

#[cfg(windows)]
fn attach_parent_console() {
    use windows::Win32::Storage::FileSystem::{FILE_TYPE_UNKNOWN, GetFileType};
    use windows::Win32::System::Console::{
        ATTACH_PARENT_PROCESS, AttachConsole, GetStdHandle, STD_ERROR_HANDLE, STD_INPUT_HANDLE,
        STD_OUTPUT_HANDLE, SetStdHandle,
    };
    unsafe {
        let handles = [STD_INPUT_HANDLE, STD_OUTPUT_HANDLE, STD_ERROR_HANDLE].map(|kind| {
            (
                kind,
                GetStdHandle(kind)
                    .ok()
                    .filter(|h| !h.is_invalid() && GetFileType(*h) != FILE_TYPE_UNKNOWN),
            )
        });
        if handles.iter().any(|(_, handle)| handle.is_none())
            && AttachConsole(ATTACH_PARENT_PROCESS).is_ok()
        {
            for (kind, handle) in handles {
                if let Some(handle) = handle {
                    let _ = SetStdHandle(kind, handle);
                }
            }
        }
    }
}
