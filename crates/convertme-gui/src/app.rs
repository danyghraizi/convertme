//! Application wiring: one queue, one capability cache, one probe thread, one drag
//! watcher, one IPC endpoint.

use crate::commands;
use crate::instance;
use crate::state::{AppState, Selection};
use convertme_core::capability::Capabilities;
use convertme_core::identity;
use convertme_core::paths;
use convertme_core::queue::{Event, Queue, QueueConfig};
use convertme_core::settings::Settings;
use convertme_core::store::Store;
use convertme_drag::{Backend, Signal, WheelKeys};
use parking_lot::Mutex;
use serde_json::json;
use std::path::PathBuf;
use std::sync::Arc;
use tauri::{Emitter, Listener, Manager};

/// Entry point for the GUI. Returns the process exit code.
pub fn run() -> i32 {
    crate::logging::init();
    logln!("{} starting", identity::APP_NAME);
    let argv: Vec<String> = std::env::args().collect();
    let initial_paths = instance::paths_from_argv(argv.clone());
    let background = instance::wants_background(&argv);

    if let Err(e) = paths::ensure_dirs() {
        eprintln!(
            "{}: cannot create its data directories: {e}",
            identity::APP_NAME
        );
        return 1;
    }

    // A crashed run must not leave work directories behind.
    convertme_core::queue::sweep_temp_dirs();

    // Second launch: hand the paths over and exit without a window — unless the running
    // window is an older build, in which case it closes and this one takes over.
    match instance::acquire() {
        Ok(lock) => std::mem::forget(lock), // held for the process lifetime
        Err(instance::InstanceError::Busy) => {
            // No files on the command line is the user asking for the window, not a
            // no-op: forwarding an empty list tells the running instance to show and
            // focus itself.
            logln!(
                "another instance owns the lock; forwarding {} path(s)",
                initial_paths.len()
            );
            match instance::forward(&initial_paths) {
                Ok(instance::Forward::Accepted) => return 0,
                Ok(instance::Forward::Stale) => {
                    logln!("the running window is an older build; taking over");
                    let mut taken = false;
                    for _ in 0..60 {
                        std::thread::sleep(std::time::Duration::from_millis(50));
                        if let Ok(lock) = instance::acquire() {
                            std::mem::forget(lock);
                            taken = true;
                            break;
                        }
                    }
                    if !taken {
                        eprintln!(
                            "{}: the older window did not close; close it and try again",
                            identity::APP_NAME
                        );
                        return 1;
                    }
                }
                Err(e) => {
                    eprintln!(
                        "{}: the running instance did not accept the files: {e}",
                        identity::APP_NAME
                    );
                    return 1;
                }
            }
        }
        Err(e) => {
            eprintln!("{}: {e}", identity::APP_NAME);
            return 1;
        }
    }

    let mut state = build_state(initial_paths);
    state.background = background;

    match tauri::Builder::default()
        .manage(state)
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_clipboard_manager::init())
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .invoke_handler(tauri::generate_handler![
            commands::boot,
            commands::image_size,
            commands::take_pending_paths,
            commands::analyze,
            commands::wheels,
            commands::run,
            commands::tool_fields,
            commands::history,
            commands::cancel_job,
            commands::cancel_all,
            commands::retry,
            commands::save_settings,
            commands::set_autostart,
            commands::autostart_status,
            commands::preview_thumb,
            commands::detect_boxes,
            commands::drag_status,
            commands::wheel_show,
            commands::wheel_hide,
            commands::overlay_open,
            commands::overlay_close,
            commands::wheel_mode,
            commands::preview_wheel,
            commands::run_action,
            commands::open_path,
            commands::reveal,
            commands::copy_path,
            commands::open_terminal,
            commands::toggle_preview,
        ])
        .setup(setup)
        .run(tauri::generate_context!())
    {
        Ok(()) => 0,
        Err(e) => {
            eprintln!("{}: {e}", identity::APP_NAME);
            1
        }
    }
}

/// Build the shared state once. Kept separate so tests can construct the same
/// state without a window.
pub fn build_state(initial_paths: Vec<PathBuf>) -> AppState {
    let settings_path = paths::settings_json();
    let settings = Settings::load(&settings_path);
    let db_path = paths::history_db();

    let cache_path = paths::capabilities_json();
    let mut caps = Capabilities::load(&cache_path);
    caps.drop_stale();

    let probe_opts = convertme_engines::probe::ProbeOptions::from_env();
    let tools = convertme_engines::Toolset::new(
        Arc::new(parking_lot::RwLock::new(caps.clone())),
        probe_opts.clone(),
    );

    let store = Store::open(&db_path).unwrap_or_else(|e| {
        eprintln!(
            "{}: history is unavailable ({e}); continuing without it",
            identity::APP_NAME
        );
        Store::in_memory().expect("in-memory history")
    });

    let (tx, rx) = crossbeam_channel::bounded::<Event>(4096);
    let queue = Queue::start(
        QueueConfig {
            overwrite: settings.overwrite_policy(),
            history_limit: settings.history_limit,
            ..Default::default()
        },
        Arc::new(convertme_engines::Dispatcher::new(tools.clone())),
        store,
        tx,
    );

    let backend = convertme_drag::select(&settings.drag_backend);
    let mut selection = Selection::default();
    selection.set(initial_paths.clone());

    AppState {
        queue,
        tools,
        caps: Arc::new(parking_lot::RwLock::new(caps)),
        settings: Mutex::new(settings),
        settings_path,
        db_path,
        cache: convertme_engines::preview::PreviewCache::new(
            paths::previews_dir(),
            256 * 1024 * 1024,
        ),
        backend,
        background: false,
        pending: Mutex::new(initial_paths),
        selection: Mutex::new(selection),
        progress: Mutex::new(Default::default()),
        previews: Mutex::new(Default::default()),
        shutting_down: std::sync::atomic::AtomicBool::new(false),
        drag: Mutex::new(None),
        events: Mutex::new(Some(rx)),
    }
}

fn setup(app: &mut tauri::App) -> Result<(), Box<dyn std::error::Error>> {
    let handle = app.handle().clone();
    logln!(
        "setup: windows = {:?}",
        app.webview_windows().keys().collect::<Vec<_>>()
    );
    start_perf_hooks(&handle);
    start_show_wheel_hook(&handle);
    start_show_main_hook(&handle);

    // The wheel window exists from startup, hidden: the first key press must not
    // pay window-creation cost.
    let app_for_window = handle.clone();
    handle.run_on_main_thread(move || {
        match crate::overlay::ensure_wheel_window(&app_for_window) {
            Ok(_) => {
                logln!("wheel window ready");
                crate::overlay::prewarm(&app_for_window);
            }
            Err(e) => crate::logging::log(format!("wheel window failed: {e}")),
        }
    })?;

    start_event_bridge(&handle);
    start_probe(handle.clone());
    start_drag_watcher(handle.clone());
    start_ipc(handle.clone());
    Ok(())
}

/// Dev aid: `CONVERTME_SHOW_WHEEL=1 convertme <paths>` opens the wheel with those
/// paths and leaves it open, so the wheel can be inspected (or screenshotted)
/// without a real drag.
fn start_show_wheel_hook(app: &tauri::AppHandle) {
    if std::env::var_os("CONVERTME_SHOW_WHEEL").is_none() {
        return;
    }
    let handle = app.clone();
    app.listen("ui-ready", move |_| {
        let Some(state) = handle.try_state::<AppState>() else {
            return;
        };
        let paths: Vec<String> = state
            .pending
            .lock()
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect();
        let handle_for_main = handle.clone();
        let _ = handle.run_on_main_thread(move || {
            let _ = crate::overlay::show_wheel(&handle_for_main, None, None);
        });
        let _ = handle.emit_to(crate::overlay::WHEEL_LABEL, "wheel-mode", "convert");
        if !paths.is_empty() {
            let _ = handle.emit_to(crate::overlay::WHEEL_LABEL, "open-paths", paths);
        }
    });
}

/// Show the window once the UI is ready, unless `--background` asked for none, and
/// make closing it hide rather than quit in that mode: the wheel must survive the
/// window.
fn start_show_main_hook(app: &tauri::AppHandle) {
    let background = app.state::<AppState>().background;
    if let Some(main) = app.get_webview_window("main") {
        let handle = app.clone();
        main.on_window_event(move |event| {
            if let tauri::WindowEvent::CloseRequested { api, .. } = event {
                let bg = handle
                    .try_state::<AppState>()
                    .map(|state| state.background)
                    .unwrap_or(false);
                if bg {
                    api.prevent_close();
                    if let Some(window) = handle.get_webview_window("main") {
                        let _ = window.hide();
                    }
                    return;
                }
                // Closing the window must end the process. The prewarmed wheel window
                // is still alive, so Tauri would otherwise keep running with no window
                // at all — and the next launch would find a lock owner with nothing to
                // show, which reads as "the app did not start".
                api.prevent_close();
                let for_exit = handle.clone();
                std::thread::spawn(move || {
                    std::thread::sleep(std::time::Duration::from_millis(50));
                    logln!("main window closed; exiting");
                    for_exit.exit(0);
                });
            }
        });
    }
    if background {
        logln!("background mode: the window stays hidden until the app is launched again");
        return;
    }
    // Show it now, not on `ui-ready`: a hidden WebKit view does not load its page, so
    // waiting for `ui-ready` before showing it would wait forever.
    if let Some(window) = app.get_webview_window("main") {
        let _ = window.show();
        let _ = window.set_focus();
    }
}

/// Measurement hooks for `cargo xtask perf`. Inert unless the app is launched by
/// that harness, and they cost nothing when it is not.
fn start_perf_hooks(app: &tauri::AppHandle) {
    let started = std::time::Instant::now();
    let want_start = std::env::var("CONVERTME_PERF_START").ok();
    let want_wheel = std::env::var("CONVERTME_PERF_WHEEL").ok();
    let idle = std::env::var("CONVERTME_PERF_IDLE").is_ok();
    if want_start.is_none() && want_wheel.is_none() && !idle {
        return;
    }
    crate::logging::log(format!(
        "perf hooks armed (start={:?}, wheel={:?}, idle={idle})",
        want_start.is_some(),
        want_wheel.is_some()
    ));
    // Report window state once, so a run that never paints says why.
    {
        let probe = app.clone();
        std::thread::spawn(move || {
            std::thread::sleep(std::time::Duration::from_secs(3));
            let windows: Vec<(String, bool, Option<String>)> = probe
                .webview_windows()
                .iter()
                .map(|(label, w)| {
                    (
                        label.clone(),
                        w.is_visible().unwrap_or(false),
                        w.url().ok().map(|u| u.to_string()),
                    )
                })
                .collect();
            crate::logging::log(format!("window state after 3 s: {windows:?}"));
        });
    }

    let (pong_tx, pong_rx) = crossbeam_channel::unbounded::<u64>();
    let app_for_pong = app.clone();
    app.listen("perf-pong", move |event| {
        let seq: u64 = serde_json::from_str(event.payload()).unwrap_or(0);
        let _ = pong_tx.send(seq);
    });
    let _ = app_for_pong;

    let handle = app.clone();
    app.listen("ui-ready", move |_| {
        crate::logging::log("ui-ready received");
        if want_start.is_some() {
            println!("PERF start={:.1}", started.elapsed().as_secs_f64() * 1000.0);
            std::process::exit(0);
        }
        let Some(runs) = want_wheel.as_ref().and_then(|v| v.parse::<u64>().ok()) else {
            // Idle measurement: stay alive, touch nothing.
            return;
        };
        let handle = handle.clone();
        let pong_rx = pong_rx.clone();
        std::thread::spawn(move || {
            // Let startup finish (the tool probe spawns a dozen `--version` calls) so the
            // measurement is about the wheel, not about the machine being busy booting.
            std::thread::sleep(std::time::Duration::from_millis(1800));
            let mut samples = Vec::with_capacity(runs as usize);
            for seq in 0..runs {
                let t0 = std::time::Instant::now();
                let for_main = handle.clone();
                let _ = handle.run_on_main_thread(move || {
                    let _ = crate::overlay::show_wheel(&for_main, None, None);
                });
                let _ = handle.emit_to(crate::overlay::WHEEL_LABEL, "perf-ping", seq);
                let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
                loop {
                    if let Ok(got) = pong_rx.recv_timeout(std::time::Duration::from_millis(500))
                        && got == seq
                    {
                        samples.push(t0.elapsed().as_secs_f64() * 1000.0);
                        break;
                    }
                    if std::time::Instant::now() > deadline {
                        break;
                    }
                }
                let for_main = handle.clone();
                let _ = handle.run_on_main_thread(move || {
                    let _ = crate::overlay::hide_wheel(&for_main);
                });
                std::thread::sleep(std::time::Duration::from_millis(60));
            }
            if samples.is_empty() {
                eprintln!("perf: the wheel window never answered");
                std::process::exit(2);
            }
            for value in &samples {
                println!("PERF wheel={value:.1}");
            }
            std::process::exit(0);
        });
    });
}

fn start_event_bridge(app: &tauri::AppHandle) {
    let Some(rx) = app
        .try_state::<AppState>()
        .and_then(|state| state.events.lock().take())
    else {
        return;
    };
    let app = app.clone();
    std::thread::Builder::new()
        .name("convertme-events".into())
        .spawn(move || {
            while let Ok(event) = rx.recv() {
                let state = app.try_state::<AppState>();
                let payload = match &event {
                    Event::Queued { id, input } => {
                        json!({"kind":"queued","id":id,"input":input.to_string_lossy()})
                    }
                    Event::Started { id, input } => {
                        json!({"kind":"started","id":id,"input":input.to_string_lossy()})
                    }
                    Event::Progress { id, fraction, stage } => {
                        if let Some(state) = &state {
                            state.progress.lock().insert(id.clone(), (*fraction, stage.clone()));
                        }
                        json!({"kind":"progress","id":id,"fraction":fraction,"stage":stage})
                    }
                    Event::Finished { id, outputs, fallback_dir } => {
                        if let Some(state) = &state {
                            state.progress.lock().remove(id);
                            state.previews.lock().clear();
                        }
                        json!({
                            "kind":"finished",
                            "id":id,
                            "outputs": outputs.iter().map(|p| p.to_string_lossy().into_owned()).collect::<Vec<_>>(),
                            "fallbackDir": fallback_dir,
                        })
                    }
                    Event::Failed { id, error, outputs } => {
                        if let Some(state) = &state {
                            state.progress.lock().remove(id);
                        }
                        json!({
                            "kind":"failed",
                            "id":id,
                            "outputs": outputs.iter().map(|p| p.to_string_lossy().into_owned()).collect::<Vec<_>>(),
                            "error": {"code": error.code.as_str(), "message": error.message, "tail": error.stderr_tail},
                        })
                    }
                    Event::Cancelled { id } => {
                        if let Some(state) = &state {
                            state.progress.lock().remove(id);
                        }
                        json!({"kind":"cancelled","id":id})
                    }
                    Event::Idle => json!({"kind":"idle"}),
                };
                let _ = app.emit("job-event", payload);
            }
        })
        .expect("spawn the queue event bridge");
}

/// Probe off-thread; first paint never waits for a `--version`.
fn start_probe(app: tauri::AppHandle) {
    let opts = convertme_engines::probe::ProbeOptions::from_env();
    let cache_path = paths::capabilities_json();
    convertme_engines::probe::probe_background(opts, cache_path, move |caps| {
        if let Some(state) = app.try_state::<AppState>() {
            *state.caps.write() = caps.clone();
        }
        let _ = app.emit("capabilities-changed", json!(null));
    });
}

fn start_drag_watcher(app: tauri::AppHandle) {
    let Some(state) = app.try_state::<AppState>() else {
        return;
    };
    let backend: Backend = state.backend;
    let keys: WheelKeys = state.keys();
    let (tx, rx) = crossbeam_channel::bounded::<Signal>(256);
    let mut surface = convertme_drag::spawn(backend, keys, tx);
    if let Err(e) = surface.start() {
        let _ = app.emit(
            "drag-signal",
            json!({"kind":"unavailable","reason": e.to_string()}),
        );
    }
    *state.drag.lock() = Some(surface);

    let app = app.clone();
    std::thread::Builder::new()
        .name("convertme-drag".into())
        .spawn(move || {
            for signal in rx {
                match signal {
                    Signal::Modifier { key, active } => {
                        let _ = app.emit(
                            "drag-signal",
                            json!({"kind":"modifier","key":key.as_str(),"active":active}),
                        );
                        if active {
                            let for_main = app.clone();
                            let _ = app.clone().run_on_main_thread(move || {
                                let (x, y) = crate::overlay::cursor_position().unzip();
                                let _ = crate::overlay::show_wheel(&for_main, x, y);
                            });
                            let _ = app.emit("wheel-mode", key.as_str());
                        }
                    }
                    Signal::Shortcut { key } => {
                        let _ =
                            app.emit("drag-signal", json!({"kind":"shortcut","key":key.as_str()}));
                        let for_main = app.clone();
                        let _ = app.clone().run_on_main_thread(move || {
                            let _ = crate::overlay::open_overlay(&for_main);
                        });
                        let _ = app.emit("wheel-mode", key.as_str());
                    }
                    Signal::DragObserved { active } => {
                        let _ = app.emit("drag-signal", json!({"kind":"drag","active":active}));
                    }
                    Signal::Unavailable { reason } => {
                        let _ =
                            app.emit("drag-signal", json!({"kind":"unavailable","reason":reason}));
                    }
                }
            }
        })
        .expect("spawn the drag watcher");
}

fn start_ipc(app: tauri::AppHandle) {
    let handle = app.clone();
    logln!(
        "ipc endpoint at {}",
        convertme_core::identity::socket_path().display()
    );
    let served = instance::serve(move |forwarded| {
        let paths = forwarded.paths;
        // An older window must not keep serving a newer build: release the lock and
        // let the process that was just launched take over.
        if !forwarded.build.is_empty() && forwarded.build != instance::BUILD_STAMP {
            logln!(
                "a newer build was launched ({} != {}); closing this window",
                forwarded.build,
                instance::BUILD_STAMP
            );
            handle.exit(0);
            return;
        }
        if let Some(state) = handle.try_state::<AppState>() {
            state.selection.lock().set(paths.clone());
            state.pending.lock().extend(paths.clone());
        }
        let list: Vec<String> = paths
            .iter()
            .map(|p| p.to_string_lossy().into_owned())
            .collect();
        logln!("second launch forwarded {} path(s)", list.len());
        let with_paths = !list.is_empty();
        if with_paths {
            let _ = handle.emit("open-paths", list);
        }
        // Bring the window up either way; a forwarded drop also raises the wheel,
        // exactly as an OS drop would.
        let for_main = handle.clone();
        let _ = handle.run_on_main_thread(move || {
            match for_main.get_webview_window("main") {
                Some(window) => {
                    let _ = window.unminimize();
                    let _ = window.show();
                    let _ = window.set_focus();
                }
                None => {
                    // A lock owner with no window can never show one again; exiting
                    // lets the launch that asked for it start a clean process.
                    logln!("no window to raise; exiting so the next launch starts fresh");
                    for_main.exit(0);
                    return;
                }
            }
            if with_paths {
                let _ = crate::overlay::show_wheel(&for_main, None, None);
            }
        });
        if with_paths {
            let _ = handle.emit_to(crate::overlay::WHEEL_LABEL, "wheel-mode", "convert");
        }
    });
    match served {
        Ok(_) => logln!("ipc listener running"),
        Err(e) => crate::logging::log(format!("ipc listener failed: {e}")),
    }
}
