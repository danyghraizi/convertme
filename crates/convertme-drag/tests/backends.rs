//! Platform backend smoke tests: starting and stopping each mechanism must be safe,
//! and a refused mechanism must degrade to a signal rather than a panic.

use convertme_core::settings::ModifierKey;
use convertme_drag::{Backend, Signal, WheelKeys, spawn};
use std::time::{Duration, Instant};

fn keys() -> WheelKeys {
    WheelKeys {
        convert: ModifierKey::Shift,
        tools: ModifierKey::Control,
        actions: ModifierKey::Alt,
    }
}

#[test]
fn headless_backend_reports_unavailable_and_stops_cleanly() {
    let (tx, rx) = crossbeam_channel::bounded(8);
    let mut surface = spawn(Backend::Headless, keys(), tx);
    surface.start().expect("headless start");
    let first = rx.recv_timeout(Duration::from_secs(2)).expect("a signal");
    assert!(matches!(first, Signal::Unavailable { .. }));
    surface.stop();
    assert_eq!(surface.backend(), Backend::Headless);
}

#[cfg(all(unix, not(target_os = "macos")))]
#[test]
fn x11_backend_starts_and_stops_without_panicking() {
    if std::env::var_os("DISPLAY").is_none() {
        return;
    }
    let (tx, rx) = crossbeam_channel::bounded(64);
    let mut surface = spawn(Backend::X11, keys(), tx);
    surface
        .start()
        .expect("the X11 poller starts when DISPLAY is set");
    let deadline = Instant::now() + Duration::from_millis(500);
    while Instant::now() < deadline && rx.try_recv().is_err() {
        std::thread::sleep(Duration::from_millis(20));
    }
    surface.stop();
    while rx.try_recv().is_ok() {}
}

#[cfg(all(unix, not(target_os = "macos")))]
#[test]
fn wayland_backend_either_registers_or_refuses_but_never_panics() {
    if std::env::var_os("WAYLAND_DISPLAY").is_none() {
        return;
    }
    let (tx, rx) = crossbeam_channel::bounded(64);
    let mut surface = spawn(Backend::Wayland, keys(), tx);
    surface.start().expect("starting the portal thread");
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut refusal = None;
    while Instant::now() < deadline {
        if let Ok(Signal::Unavailable { reason }) = rx.recv_timeout(Duration::from_millis(200)) {
            refusal = Some(reason);
            break;
        }
    }
    surface.stop();
    if let Some(reason) = refusal {
        // Documented outcome on sessions without a GlobalShortcuts portal.
        assert!(!reason.is_empty());
    }
}
