//! Shared application state for the Tauri commands and the event loops.

use convertme_core::capability::Capabilities;
use convertme_core::formats::Category;
use convertme_core::job::JobId;
use convertme_core::queue::{Event, Queue};
use convertme_core::settings::Settings;
use convertme_drag::{Backend, DragSurface, WheelKeys};
use parking_lot::Mutex;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

pub struct AppState {
    pub queue: Arc<Queue>,
    pub tools: convertme_engines::Toolset,
    pub caps: Arc<parking_lot::RwLock<Capabilities>>,
    pub settings: Mutex<Settings>,
    pub settings_path: PathBuf,
    pub db_path: PathBuf,
    pub cache: convertme_engines::preview::PreviewCache,
    pub backend: Backend,
    /// Started with `--background`: no window until the app is launched again.
    pub background: bool,
    /// Paths that arrived on the command line or over IPC before the UI asked.
    pub pending: Mutex<Vec<PathBuf>>,
    /// The selection the wheel was opened for.
    pub selection: Mutex<Selection>,
    /// Live progress per running job, so a window that opens late still shows it.
    pub progress: Mutex<HashMap<JobId, (f64, String)>>,
    pub previews: Mutex<HashMap<PathBuf, bool>>,
    /// The dialog previews in flight: starting one cancels the one before it.
    pub preview_guard: convertme_engines::preview::PreviewGuard,
    pub shutting_down: AtomicBool,
    pub drag: Mutex<Option<Box<dyn DragSurface>>>,
    /// Receiving end of the queue's event channel, taken once by the bridge thread.
    pub events: Mutex<Option<crossbeam_channel::Receiver<Event>>>,
    /// The one controllable audio player; a second Play replaces the first.
    pub audio: crate::audio::AudioSink,
}

#[derive(Debug, Clone, Default)]
pub struct Selection {
    pub paths: Vec<PathBuf>,
    pub category: Option<Category>,
    pub format: Option<String>,
}

impl Selection {
    pub fn set(&mut self, paths: Vec<PathBuf>) {
        self.paths = paths;
        self.category = self
            .paths
            .iter()
            .find_map(|p| convertme_core::formats::Registry::category_of(p));
        self.format = self
            .paths
            .iter()
            .find_map(|p| convertme_core::formats::Registry::detect(p).map(|f| f.id.0.to_string()));
    }
}

impl AppState {
    pub fn keys(&self) -> WheelKeys {
        let s = self.settings.lock();
        WheelKeys {
            convert: s.convert_key,
            tools: s.tools_key,
            actions: s.actions_key,
        }
    }

    pub fn request_shutdown(&self) {
        self.shutting_down.store(true, Ordering::SeqCst);
    }
}
