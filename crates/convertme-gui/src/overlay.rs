//! The wheel window and the full-screen drop overlay.
//!
//! One extra window, created once at startup and kept hidden, because window
//! creation is the expensive part of "the wheel appeared late". On Wayland a client
//! cannot position a window, so the wheel always runs full-screen there and the
//! segment ring is drawn around the pointer position reported by the drop; on
//! Windows and X11 the wheel window is a small borderless layer positioned at the
//! pointer.

use convertme_drag::Backend;
use tauri::{Emitter, Manager, PhysicalPosition, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

pub const WHEEL_LABEL: &str = "wheel";
const WHEEL_SIZE: f64 = 640.0;

/// Create the wheel window if it is not there yet. Idempotent.
pub fn ensure_wheel_window(app: &tauri::AppHandle) -> tauri::Result<WebviewWindow> {
    if let Some(window) = app.get_webview_window(WHEEL_LABEL) {
        return Ok(window);
    }
    WebviewWindowBuilder::new(app, WHEEL_LABEL, WebviewUrl::App("index.html".into()))
        .title("")
        .inner_size(WHEEL_SIZE, WHEEL_SIZE)
        .decorations(false)
        .transparent(true)
        .shadow(false)
        .resizable(false)
        .always_on_top(true)
        .skip_taskbar(true)
        .visible(false)
        .focused(false)
        .accept_first_mouse(true)
        .build()
}

/// Load the wheel's page once, without the user seeing it.
///
/// A hidden WebKit view does not run its page, so the first key press after launch
/// would pay for the load — which is exactly the 50 ms budget. On X11 and Windows the
/// window can be shown far outside every monitor for a moment; Wayland gives a client
/// no position, so there the first open is honestly slower and we say so in the log.
pub fn prewarm(app: &tauri::AppHandle) {
    if matches!(backend(app), Backend::Wayland | Backend::Headless) {
        logln!("wheel prewarm skipped: this compositor decides where a window goes");
        return;
    }
    let window = match ensure_wheel_window(app) {
        Ok(window) => window,
        Err(e) => {
            crate::logging::log(format!("wheel prewarm: {e}"));
            return;
        }
    };
    let _ = window.set_position(PhysicalPosition::new(-20000, -20000));
    let _ = window.show();
    let handle = app.clone();
    std::thread::spawn(move || {
        std::thread::sleep(std::time::Duration::from_millis(900));
        let for_main = handle.clone();
        let _ = handle.run_on_main_thread(move || {
            let _ = hide_wheel(&for_main);
            crate::logging::log("wheel prewarmed off-screen");
        });
    });
}

fn backend(app: &tauri::AppHandle) -> Backend {
    app.try_state::<crate::state::AppState>()
        .map(|s| s.backend)
        .unwrap_or(Backend::Headless)
}

/// Show the wheel. `x`/`y` are physical screen coordinates of the pointer when the
/// platform can provide them.
pub fn show_wheel(app: &tauri::AppHandle, x: Option<f64>, y: Option<f64>) -> Result<(), String> {
    let window = ensure_wheel_window(app).map_err(|e| e.to_string())?;
    match backend(app) {
        Backend::Wayland | Backend::Headless => {
            window.set_fullscreen(true).map_err(|e| e.to_string())?;
        }
        Backend::Windows | Backend::X11 => {
            window.set_fullscreen(false).map_err(|e| e.to_string())?;
            let (px, py) = match (x, y) {
                (Some(x), Some(y)) => (x, y),
                _ => pointer_position(app).unwrap_or((400.0, 400.0)),
            };
            #[cfg(windows)]
            {
                let monitor = window
                    .monitor_from_point(px, py)
                    .map_err(|e| e.to_string())?
                    .ok_or_else(|| "no monitor at pointer position".to_string())?;
                let area = monitor.work_area();
                let size = ((WHEEL_SIZE * monitor.scale_factor()).round() as u32)
                    .min(area.size.width)
                    .min(area.size.height);
                if size == 0 {
                    return Err("pointer monitor has an empty work area".into());
                }
                window
                    .set_size(tauri::PhysicalSize::new(size, size))
                    .map_err(|e| e.to_string())?;
                let left = area.position.x as i64;
                let top = area.position.y as i64;
                let x = ((px - f64::from(size) / 2.0).round() as i64)
                    .clamp(left, left + i64::from(area.size.width - size));
                let y = ((py - f64::from(size) / 2.0).round() as i64)
                    .clamp(top, top + i64::from(area.size.height - size));
                window
                    .set_position(PhysicalPosition::new(x as i32, y as i32))
                    .map_err(|e| e.to_string())?;
            }
            #[cfg(not(windows))]
            {
                let size = window.outer_size().map_err(|e| e.to_string())?;
                let pos = PhysicalPosition::new(
                    (px - size.width as f64 / 2.0).max(0.0) as i32,
                    (py - size.height as f64 / 2.0).max(0.0) as i32,
                );
                window.set_position(pos).map_err(|e| e.to_string())?;
                window
                    .set_size(tauri::PhysicalSize::new(
                        WHEEL_SIZE as u32,
                        WHEEL_SIZE as u32,
                    ))
                    .map_err(|e| e.to_string())?;
            }
        }
    }
    window.show().map_err(|e| e.to_string())?;
    window.set_focus().ok();
    let _ = app.emit_to(WHEEL_LABEL, "wheel-shown", ());
    Ok(())
}

pub fn hide_wheel(app: &tauri::AppHandle) -> Result<(), String> {
    if let Some(window) = app.get_webview_window(WHEEL_LABEL) {
        window.set_fullscreen(false).ok();
        window.hide().map_err(|e| e.to_string())?;
        let _ = app.emit_to(WHEEL_LABEL, "wheel-hidden", ());
    }
    Ok(())
}

/// The full-screen "drop here" overlay used on Wayland, where no drag can be
/// observed from outside the app.
pub fn open_overlay(app: &tauri::AppHandle) -> Result<(), String> {
    let window = ensure_wheel_window(app).map_err(|e| e.to_string())?;
    window.set_fullscreen(true).map_err(|e| e.to_string())?;
    window.show().map_err(|e| e.to_string())?;
    window.set_focus().ok();
    let _ = app.emit_to(WHEEL_LABEL, "overlay-open", ());
    Ok(())
}

pub fn close_overlay(app: &tauri::AppHandle) -> Result<(), String> {
    hide_wheel(app)
}

/// Current pointer position, when the platform lets us ask.
pub fn pointer_position(_app: &tauri::AppHandle) -> Option<(f64, f64)> {
    cursor_position()
}

#[cfg(windows)]
pub fn cursor_position() -> Option<(f64, f64)> {
    use windows::Win32::Foundation::POINT;
    use windows::Win32::UI::WindowsAndMessaging::GetCursorPos;
    let mut point = POINT::default();
    // SAFETY: GetCursorPos writes into a stack-allocated POINT.
    unsafe { GetCursorPos(&mut point).ok()? };
    Some((point.x as f64, point.y as f64))
}

#[cfg(all(unix, not(target_os = "macos")))]
pub fn cursor_position() -> Option<(f64, f64)> {
    use std::os::raw::{c_int, c_ulong};
    let xlib = x11_dl::xlib::Xlib::open().ok()?;
    // SAFETY: a short-lived display connection, closed below.
    unsafe {
        let display = (xlib.XOpenDisplay)(std::ptr::null());
        if display.is_null() {
            return None;
        }
        let mut root: c_ulong = 0;
        let mut child: c_ulong = 0;
        let mut rx: c_int = 0;
        let mut ry: c_int = 0;
        let mut wx: c_int = 0;
        let mut wy: c_int = 0;
        let mut mask: std::os::raw::c_uint = 0;
        let ok = (xlib.XQueryPointer)(
            display,
            (xlib.XDefaultRootWindow)(display),
            &mut root,
            &mut child,
            &mut rx,
            &mut ry,
            &mut wx,
            &mut wy,
            &mut mask,
        );
        (xlib.XCloseDisplay)(display);
        if ok == 0 {
            return None;
        }
        Some((rx as f64, ry as f64))
    }
}

#[cfg(not(any(windows, all(unix, not(target_os = "macos")))))]
pub fn cursor_position() -> Option<(f64, f64)> {
    None
}
