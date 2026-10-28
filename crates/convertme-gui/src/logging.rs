//! Logging.
//!
//! Off by default (an idle app should not touch the disk), enabled by
//! `CONVERTME_LOG=1` and always on when the perf harness is driving, so a
//! measurement run explains itself. Everything goes to stderr, which is what a
//! service manager, a terminal and the harness all capture.

use std::sync::atomic::{AtomicBool, Ordering};

static ENABLED: AtomicBool = AtomicBool::new(false);
static START: std::sync::OnceLock<std::time::Instant> = std::sync::OnceLock::new();

/// Turn logging on when the environment asks for it.
pub fn init() {
    let asked = std::env::var_os(convertme_core::identity::ENV_LOG).is_some()
        || std::env::var_os("CONVERTME_PERF").is_some();
    if asked {
        ENABLED.store(true, Ordering::Relaxed);
        START.set(std::time::Instant::now()).ok();
        log("logging enabled");
    }
}

pub fn enabled() -> bool {
    ENABLED.load(Ordering::Relaxed)
}

/// Print one line, timestamped from process start.
pub fn log(message: impl AsRef<str>) {
    if !enabled() {
        return;
    }
    let at = START
        .get()
        .map(|start| start.elapsed().as_secs_f64() * 1000.0)
        .unwrap_or(0.0);
    eprintln!("[{at:>8.1} ms] {}", message.as_ref());
}

/// `log!("x = {}", value)` without pulling in a logging framework.
#[macro_export]
macro_rules! logln {
    ($($arg:tt)*) => {
        if $crate::logging::enabled() {
            $crate::logging::log(format!($($arg)*));
        }
    };
}
