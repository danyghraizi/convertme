//! `convertme-gui`: the Tauri shell, the wheel window, the actions and the CLI.
//!
//! The window is deliberately thin. Everything that decides *what* to do lives in
//! `convertme-core`, everything that *does* it lives in `convertme-engines`, and
//! everything platform-specific about observing input lives in `convertme-drag`.
//!
//! `logging` is declared first and with `#[macro_use]` so `logln!` is available to
//! every module below it.

#[macro_use]
pub mod logging;

pub mod actions;
pub mod app;
pub mod autostart;
pub mod cli;
pub mod commands;
pub mod instance;
pub mod overlay;
pub mod state;

pub use app::run;
