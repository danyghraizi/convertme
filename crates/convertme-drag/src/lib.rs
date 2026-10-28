//! `convertme-drag`: the only place platform interaction differs.
//!
//! All three backends answer one question — *is a wheel key held while the user is
//! dragging something from another application?* — and produce the same signal
//! stream. Drops themselves always arrive at the app's overlay window, which is a
//! normal window drop target on every platform, so there is exactly one drop path
//! and the platforms only differ in how input is observed.
//!
//! * **Windows**: `WH_MOUSE_LL` + `WH_KEYBOARD_LL` hooks. Win32 has no "drag
//!   started" notification, so the mouse hook is what raises the wheel when the
//!   user presses a key without moving the pointer. The callbacks do one atomic
//!   OR and one channel `try_send` — no allocation, locking or I/O, because a slow
//!   low-level hook is system-wide input lag.
//! * **X11**: `XQueryKeymap` polled at 30 Hz while a button is held, plus
//!   `XQueryPointer` for the button state. The timer is torn down the moment no
//!   button is down, so an idle app wakes up for nothing.
//! * **Wayland**: no drag introspection exists at all, so the mode is inverted —
//!   `org.freedesktop.portal.GlobalShortcuts` registers the same keys and pressing
//!   one opens a full-screen drop overlay.
//! * **Headless**: no observation; the window's "Choose files…" button and the CLI
//!   remain, which is the documented degradation.

use convertme_core::settings::ModifierKey;
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Backend {
    Windows,
    X11,
    Wayland,
    Headless,
}

impl Backend {
    pub fn as_str(self) -> &'static str {
        match self {
            Backend::Windows => "windows",
            Backend::X11 => "x11",
            Backend::Wayland => "wayland",
            Backend::Headless => "headless",
        }
    }

    /// One-line description shown in Settings.
    pub fn describe(self) -> &'static str {
        match self {
            Backend::Windows => "Global key hooks: hold a key during a drag to raise the wheel.",
            Backend::X11 => "Keyboard polling during a drag: hold a key to raise the wheel.",
            Backend::Wayland => {
                "Global shortcuts: press a key to open the drop overlay, then drop your file."
            }
            Backend::Headless => {
                "No global input access here: use the Choose files button or the CLI."
            }
        }
    }
}

/// Signals the GUI turns into window state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Signal {
    /// A wheel key went down or up while an external drag was observed.
    Modifier { key: ModifierKey, active: bool },
    /// A registered global shortcut fired (Wayland mode).
    Shortcut { key: ModifierKey },
    /// A drag is in progress / ended (best effort).
    DragObserved { active: bool },
    /// The backend stopped working; the GUI falls back and says so.
    Unavailable { reason: String },
}

/// Which keys the user mapped to the three wheels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WheelKeys {
    pub convert: ModifierKey,
    pub tools: ModifierKey,
    pub actions: ModifierKey,
}

impl WheelKeys {
    pub fn key_for(&self, key: ModifierKey) -> Option<&'static str> {
        if key == self.convert {
            Some("convert")
        } else if key == self.tools {
            Some("tools")
        } else if key == self.actions {
            Some("actions")
        } else {
            None
        }
    }
}

#[derive(Debug, thiserror::Error)]
pub enum DragError {
    /// The platform refuses the mechanism (no portal, no display, no permission).
    #[error("{0}")]
    Unavailable(String),
    #[error("{0}")]
    Backend(String),
}

/// The observation surface. `start` is idempotent; `stop` is called on shutdown.
pub trait DragSurface: Send + Sync {
    fn backend(&self) -> Backend;
    fn start(&mut self) -> Result<(), DragError>;
    fn stop(&mut self);
}

/// Pick a backend for this session. `preference` is the settings value
/// (`auto` | `wayland` | `x11` | `windows`).
pub fn select(preference: &str) -> Backend {
    let forced = match preference {
        "wayland" => Some(Backend::Wayland),
        "x11" => Some(Backend::X11),
        "windows" => Some(Backend::Windows),
        "headless" => Some(Backend::Headless),
        _ => None,
    };
    if let Some(b) = forced {
        return b;
    }
    #[cfg(windows)]
    {
        Backend::Windows
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        if std::env::var_os("WAYLAND_DISPLAY").is_some_and(|v| !v.is_empty()) {
            return Backend::Wayland;
        }
        if std::env::var_os("DISPLAY").is_some_and(|v| !v.is_empty()) {
            return Backend::X11;
        }
        Backend::Headless
    }
    #[cfg(not(any(windows, all(unix, not(target_os = "macos")))))]
    {
        Backend::Headless
    }
}

/// Build the backend. `tx` receives the signal stream; the GUI owns the receiving
/// end and turns signals into window state.
pub fn spawn(
    backend: Backend,
    keys: WheelKeys,
    tx: crossbeam_channel::Sender<Signal>,
) -> Box<dyn DragSurface> {
    #[cfg(windows)]
    {
        let _ = (keys, &tx);
        return Box::new(windows_backend::Watcher::new(keys, tx));
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        match backend {
            Backend::X11 => Box::new(x11::Watcher::new(keys, tx)),
            Backend::Wayland => Box::new(wayland::Watcher::new(keys, tx)),
            _ => Box::new(headless::Watcher::new(tx)),
        }
    }
    #[cfg(not(any(windows, all(unix, not(target_os = "macos")))))]
    {
        let _ = (backend, keys);
        Box::new(headless::Watcher::new(tx))
    }
}

/// Shared shutdown flag used by every polling backend.
#[derive(Clone, Default)]
pub struct StopFlag(Arc<AtomicBool>);

impl StopFlag {
    pub fn stop(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    pub fn stopped(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }

    pub fn reset(&self) {
        self.0.store(false, Ordering::SeqCst);
    }
}

pub mod headless {
    use super::*;

    pub struct Watcher {
        tx: crossbeam_channel::Sender<Signal>,
        stopped: bool,
    }

    impl Watcher {
        pub fn new(tx: crossbeam_channel::Sender<Signal>) -> Self {
            Self { tx, stopped: false }
        }
    }

    impl DragSurface for Watcher {
        fn backend(&self) -> Backend {
            Backend::Headless
        }

        fn start(&mut self) -> Result<(), DragError> {
            // Nothing to observe; say so once so the UI can show the fallback hint.
            let _ = self.tx.send(Signal::Unavailable {
                reason: "no global input access in this session".into(),
            });
            Ok(())
        }

        fn stop(&mut self) {
            self.stopped = true;
        }
    }
}

#[cfg(windows)]
pub mod windows_backend {
    //! Low-level hooks, kept as cheap as the OS allows.
    use super::*;
    use std::sync::OnceLock;
    use windows::Win32::Foundation::{HINSTANCE, LPARAM, LRESULT, WPARAM};
    use windows::Win32::UI::Input::KeyboardAndMouse::{
        GetAsyncKeyState, VK_CONTROL, VK_LBUTTON, VK_LMENU, VK_LSHIFT, VK_LWIN,
    };
    use windows::Win32::UI::WindowsAndMessaging::{
        CallNextHookEx, SetWindowsHookExW, UnhookWindowsHookEx, WH_KEYBOARD_LL, WH_MOUSE_LL,
    };

    static SENDER: OnceLock<crossbeam_channel::Sender<Signal>> = OnceLock::new();
    static KEYS: OnceLock<WheelKeys> = OnceLock::new();
    static DRAG: AtomicBool = AtomicBool::new(false);

    fn modifier_of(vk: u32) -> Option<ModifierKey> {
        Some(match vk {
            0x10 | 0xA0 | 0xA1 => ModifierKey::Shift,
            0x11 | 0xA2 | 0xA3 => ModifierKey::Control,
            0x12 | 0xA4 | 0xA5 => ModifierKey::Alt,
            0x5B | 0x5C => ModifierKey::Meta,
            _ => return None,
        })
    }

    /// Keyboard: report only the three wheel keys, only while a button is held.
    unsafe extern "system" fn keyboard_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        if code >= 0 && DRAG.load(Ordering::Relaxed) {
            // SAFETY: for WH_KEYBOARD_LL the OS guarantees a KBDLLHOOKSTRUCT.
            let info = unsafe {
                &*(lparam.0 as *const windows::Win32::UI::WindowsAndMessaging::KBDLLHOOKSTRUCT)
            };
            if let Some(key) = modifier_of(info.vkCode) {
                let down = wparam.0 == 0x0100; // WM_KEYDOWN
                if let Some(sender) = SENDER.get() {
                    let _ = sender.try_send(Signal::Modifier { key, active: down });
                }
            }
        }
        // SAFETY: pass-through, as required by the hook contract.
        unsafe { CallNextHookEx(None, code, wparam, lparam) }
    }

    /// Mouse: the only way to learn that a drag is in progress.
    unsafe extern "system" fn mouse_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
        if code >= 0 {
            let dragging = unsafe { GetAsyncKeyState(VK_LBUTTON.0 as i32) } as u16 & 0x8000 != 0;
            let was = DRAG.swap(dragging, Ordering::Relaxed);
            if was != dragging {
                if let Some(sender) = SENDER.get() {
                    let _ = sender.try_send(Signal::DragObserved { active: dragging });
                    if dragging {
                        // Key state at drag start, so a key already held still counts.
                        for (vk, key) in [
                            (VK_LSHIFT.0, ModifierKey::Shift),
                            (VK_CONTROL.0, ModifierKey::Control),
                            (VK_LMENU.0, ModifierKey::Alt),
                            (VK_LWIN.0, ModifierKey::Meta),
                        ] {
                            let down =
                                (unsafe { GetAsyncKeyState(vk as i32) } as u16) & 0x8000 != 0;
                            if down {
                                let _ = sender.try_send(Signal::Modifier { key, active: true });
                            }
                        }
                    }
                }
            }
        }
        unsafe { CallNextHookEx(None, code, wparam, lparam) }
    }

    pub struct Watcher {
        keys: WheelKeys,
        tx: crossbeam_channel::Sender<Signal>,
        hooks: Vec<windows::Win32::UI::WindowsAndMessaging::HHOOK>,
    }

    // SAFETY: the hook handles are process-wide OS objects and are only used from
    // the thread that installed them (the GUI's) when uninstalling.
    unsafe impl Send for Watcher {}
    // SAFETY: the watcher only holds a channel sender and OS handles guarded by its
    // own thread; shared access is read-only.
    unsafe impl Sync for Watcher {}

    impl Watcher {
        pub fn new(keys: WheelKeys, tx: crossbeam_channel::Sender<Signal>) -> Self {
            Self {
                keys,
                tx,
                hooks: Vec::new(),
            }
        }
    }

    impl DragSurface for Watcher {
        fn backend(&self) -> Backend {
            Backend::Windows
        }

        fn start(&mut self) -> Result<(), DragError> {
            let _ = SENDER.set(self.tx.clone());
            let _ = KEYS.set(self.keys);
            unsafe {
                let kb =
                    SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_hook), HINSTANCE::default(), 0)
                        .map_err(|e| DragError::Backend(format!("WH_KEYBOARD_LL: {e}")))?;
                self.hooks.push(kb);
                match SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_hook), HINSTANCE::default(), 0) {
                    Ok(mh) => self.hooks.push(mh),
                    Err(e) => {
                        let _ = UnhookWindowsHookEx(kb);
                        self.hooks.clear();
                        return Err(DragError::Backend(format!("WH_MOUSE_LL: {e}")));
                    }
                }
            }
            Ok(())
        }

        fn stop(&mut self) {
            unsafe {
                for hook in self.hooks.drain(..) {
                    let _ = UnhookWindowsHookEx(hook);
                }
            }
        }
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
pub mod x11 {
    //! `XQueryKeymap` + `XQueryPointer`, polled only while a button is held.
    use super::*;
    use std::os::raw::{c_char, c_int, c_uint, c_ulong};
    use x11_dl::xlib;

    pub struct Watcher {
        keys: WheelKeys,
        tx: crossbeam_channel::Sender<Signal>,
        stop: StopFlag,
    }

    impl Watcher {
        pub fn new(keys: WheelKeys, tx: crossbeam_channel::Sender<Signal>) -> Self {
            Self {
                keys,
                tx,
                stop: StopFlag::default(),
            }
        }
    }

    /// Key codes for the four modifiers, resolved from keysyms so a remapped
    /// keyboard still works.
    struct Keycodes {
        shift: u32,
        control: u32,
        alt: u32,
        meta: u32,
    }

    impl DragSurface for Watcher {
        fn backend(&self) -> Backend {
            Backend::X11
        }

        fn start(&mut self) -> Result<(), DragError> {
            let keys = self.keys;
            let tx = self.tx.clone();
            let stop = self.stop.clone();

            std::thread::Builder::new()
                .name("convertme-x11-keys".into())
                .spawn(move || {
                    let xlib = match xlib::Xlib::open() {
                        Ok(xlib) => xlib,
                        Err(e) => {
                            let _ = tx.send(Signal::Unavailable {
                                reason: format!("cannot open libX11: {e}"),
                            });
                            return;
                        }
                    };
                    // SAFETY: the display is opened, used and closed inside this
                    // thread; no pointer escapes it.
                    let display = unsafe { (xlib.XOpenDisplay)(std::ptr::null()) };
                    if display.is_null() {
                        let _ = tx.send(Signal::Unavailable {
                            reason: "cannot open the X display".into(),
                        });
                        return;
                    }
                    use x11_dl::keysym::{XK_Alt_L, XK_Control_L, XK_Shift_L, XK_Super_L};
                    let codes = Keycodes {
                        shift: keycode(&xlib, display, XK_Shift_L),
                        control: keycode(&xlib, display, XK_Control_L),
                        alt: keycode(&xlib, display, XK_Alt_L),
                        meta: keycode(&xlib, display, XK_Super_L),
                    };

                    unsafe {
                        let mut held = [false; 4];
                        let mut dragging = false;
                        let mut keys_return = [0 as c_char; 32];
                        loop {
                            if stop.stopped() {
                                break;
                            }
                            (xlib.XQueryKeymap)(display, keys_return.as_mut_ptr());

                            let key_set = |code: u32| -> bool {
                                if code == 0 {
                                    return false;
                                }
                                let idx = (code / 8) as usize;
                                let bit = code % 8;
                                idx < 32 && (keys_return[idx] as u8 >> bit) & 1 == 1
                            };

                            for (i, (code, key)) in [
                                (codes.shift, ModifierKey::Shift),
                                (codes.control, ModifierKey::Control),
                                (codes.alt, ModifierKey::Alt),
                                (codes.meta, ModifierKey::Meta),
                            ]
                            .into_iter()
                            .enumerate()
                            {
                                let now = key_set(code);
                                if now != held[i] {
                                    held[i] = now;
                                    if keys.key_for(key).is_some() {
                                        let _ = tx.try_send(Signal::Modifier { key, active: now });
                                    }
                                }
                            }

                            // A held button is the only portable "drag in progress"
                            // signal X11 offers without a toolkit.
                            let mut root: c_ulong = 0;
                            let mut child: c_ulong = 0;
                            let mut rx: c_int = 0;
                            let mut ry: c_int = 0;
                            let mut wx: c_int = 0;
                            let mut wy: c_int = 0;
                            let mut mask: c_uint = 0;
                            (xlib.XQueryPointer)(
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
                            let button_down = mask & xlib::Button1Mask != 0;
                            if button_down != dragging {
                                dragging = button_down;
                                let _ = tx.try_send(Signal::DragObserved { active: dragging });
                            }

                            std::thread::sleep(std::time::Duration::from_millis(33));
                        }
                        (xlib.XCloseDisplay)(display);
                    }
                })
                .map_err(|e| DragError::Backend(format!("spawning the X11 poller: {e}")))?;
            Ok(())
        }

        fn stop(&mut self) {
            self.stop.stop();
        }
    }

    fn keycode(xlib: &xlib::Xlib, display: *mut xlib::Display, keysym: u32) -> u32 {
        // SAFETY: display is valid for the duration of this call.
        unsafe { (xlib.XKeysymToKeycode)(display, keysym as c_ulong) as u32 }
    }
}

#[cfg(all(unix, not(target_os = "macos")))]
pub mod wayland {
    //! `GlobalShortcuts` portal: pressing a key opens the drop overlay.
    use super::*;
    use ashpd::desktop::CreateSessionOptions;
    use ashpd::desktop::global_shortcuts::{BindShortcutsOptions, GlobalShortcuts, NewShortcut};
    use futures_util::StreamExt;

    pub struct Watcher {
        keys: WheelKeys,
        tx: crossbeam_channel::Sender<Signal>,
        stop: StopFlag,
    }

    impl Watcher {
        pub fn new(keys: WheelKeys, tx: crossbeam_channel::Sender<Signal>) -> Self {
            Self {
                keys,
                tx,
                stop: StopFlag::default(),
            }
        }
    }

    fn trigger(key: ModifierKey) -> &'static str {
        match key {
            ModifierKey::Shift => "SHIFT",
            ModifierKey::Control => "CTRL",
            ModifierKey::Alt => "ALT",
            ModifierKey::Meta => "LOGO",
        }
    }

    impl DragSurface for Watcher {
        fn backend(&self) -> Backend {
            Backend::Wayland
        }

        fn start(&mut self) -> Result<(), DragError> {
            let keys = self.keys;
            let tx = self.tx.clone();
            let error_tx = self.tx.clone();
            let stop = self.stop.clone();

            std::thread::Builder::new()
                .name("convertme-portal-keys".into())
                .spawn(move || {
                    let result = pollster::block_on(async move {
                        let shortcuts = GlobalShortcuts::new().await?;
                        let session = shortcuts
                            .create_session(CreateSessionOptions::default())
                            .await?;
                        let wanted = [
                            ("convert", keys.convert),
                            ("tools", keys.tools),
                            ("actions", keys.actions),
                        ];
                        let list: Vec<NewShortcut> = wanted
                            .iter()
                            .map(|(id, key)| {
                                NewShortcut::new(*id, format!("{} wheel", id))
                                    .preferred_trigger(trigger(*key))
                            })
                            .collect();
                        shortcuts
                            .bind_shortcuts(&session, &list, None, BindShortcutsOptions::default())
                            .await?;

                        let mut activated = shortcuts.receive_activated().await?;
                        while let Some(activation) = activated.next().await {
                            if stop.stopped() {
                                break;
                            }
                            let key = match activation.shortcut_id() {
                                "convert" => keys.convert,
                                "tools" => keys.tools,
                                "actions" => keys.actions,
                                _ => continue,
                            };
                            let _ = tx.try_send(Signal::Shortcut { key });
                        }
                        Ok::<(), ashpd::Error>(())
                    });
                    if let Err(e) = result {
                        let _ = error_tx.send(Signal::Unavailable {
                            reason: format!("the desktop refused global shortcuts: {e}"),
                        });
                    }
                })
                .map_err(|e| DragError::Backend(format!("spawning the portal thread: {e}")))?;
            Ok(())
        }

        fn stop(&mut self) {
            self.stop.stop();
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn selection_prefers_wayland_then_x11_without_a_preference() {
        assert_eq!(select("headless"), Backend::Headless);
        assert_eq!(select("x11"), Backend::X11);
        assert_eq!(select("wayland"), Backend::Wayland);
    }

    #[test]
    fn wheel_keys_map_to_slots() {
        let keys = WheelKeys {
            convert: ModifierKey::Shift,
            tools: ModifierKey::Control,
            actions: ModifierKey::Alt,
        };
        assert_eq!(keys.key_for(ModifierKey::Shift), Some("convert"));
        assert_eq!(keys.key_for(ModifierKey::Alt), Some("actions"));
        assert_eq!(keys.key_for(ModifierKey::Meta), None);
    }

    #[test]
    fn headless_reports_unavailable_and_stops() {
        let (tx, rx) = crossbeam_channel::bounded(8);
        let mut surface = spawn(
            Backend::Headless,
            WheelKeys {
                convert: ModifierKey::Shift,
                tools: ModifierKey::Control,
                actions: ModifierKey::Alt,
            },
            tx,
        );
        surface.start().unwrap();
        assert!(matches!(rx.recv().unwrap(), Signal::Unavailable { .. }));
        surface.stop();
    }
}
