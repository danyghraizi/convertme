//! Starting at login.
//!
//! The wheel needs a process to hold the keyboard hook or the portal shortcut, so
//! "use the wheel without opening the app" means "an invisible instance runs from
//! login". That is one autostart entry pointing at `convertme --background`; the
//! window opens when the user launches the app again.

use convertme_core::identity;
use std::path::PathBuf;

/// Write or remove the autostart entry. Returns whether it is now in place.
pub fn set(enabled: bool) -> Result<bool, String> {
    if enabled {
        imp::install()?;
    } else {
        imp::remove()?;
    }
    Ok(enabled)
}

/// Whether an autostart entry exists right now.
pub fn is_enabled() -> bool {
    imp::present()
}

/// The binary to point at: the running executable, not a guessed path.
fn executable() -> PathBuf {
    std::env::current_exe().unwrap_or_else(|_| PathBuf::from(identity::BIN))
}

#[cfg(all(unix, not(target_os = "macos")))]
mod imp {
    use super::*;
    use std::io::Write;

    pub(super) fn entry() -> PathBuf {
        let config = std::env::var_os("XDG_CONFIG_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| {
                PathBuf::from(std::env::var_os("HOME").unwrap_or_default()).join(".config")
            });
        config.join("autostart").join(identity::DESKTOP_FILE)
    }

    pub fn install() -> Result<(), String> {
        let path = entry();
        if let Some(dir) = path.parent() {
            std::fs::create_dir_all(dir).map_err(|e| format!("{}: {e}", dir.display()))?;
        }
        let exe = executable();
        let body = format!(
            "[Desktop Entry]\n\
             Type=Application\n\
             Name={}\n\
             Comment=The ConvertMe wheel, ready without a window\n\
             Exec={} --background\n\
             Terminal=false\n\
             NoDisplay=true\n\
             X-GNOME-Autostart-enabled=true\n",
            identity::APP_NAME,
            exe.display()
        );
        let mut file =
            std::fs::File::create(&path).map_err(|e| format!("{}: {e}", path.display()))?;
        file.write_all(body.as_bytes())
            .map_err(|e| format!("{}: {e}", path.display()))
    }

    pub fn remove() -> Result<(), String> {
        match std::fs::remove_file(entry()) {
            Ok(()) => Ok(()),
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(e) => Err(format!("{}: {e}", entry().display())),
        }
    }

    pub fn present() -> bool {
        entry().is_file()
    }
}

#[cfg(windows)]
mod imp {
    use super::*;
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::Foundation::ERROR_FILE_NOT_FOUND;
    use windows::Win32::System::Registry::*;
    use windows::core::{PCWSTR, w};

    struct Key(HKEY);
    impl Drop for Key {
        fn drop(&mut self) {
            unsafe {
                let _ = RegCloseKey(self.0);
            }
        }
    }
    fn open(access: REG_SAM_FLAGS) -> Result<Key, windows::Win32::Foundation::WIN32_ERROR> {
        let mut key = HKEY::default();
        let status = unsafe {
            RegOpenKeyExW(
                HKEY_CURRENT_USER,
                w!("Software\\Microsoft\\Windows\\CurrentVersion\\Run"),
                None,
                access,
                &mut key,
            )
        };
        if status.0 == 0 {
            Ok(Key(key))
        } else {
            Err(status)
        }
    }
    fn name() -> Vec<u16> {
        std::ffi::OsStr::new(identity::APP_NAME)
            .encode_wide()
            .chain(Some(0))
            .collect()
    }
    pub fn install() -> Result<(), String> {
        let key = open(KEY_SET_VALUE).map_err(|e| format!("open Run key: {e:?}"))?;
        let exe = std::env::current_exe().map_err(|e| e.to_string())?;
        let mut value: Vec<u16> = std::ffi::OsStr::new("\"").encode_wide().collect();
        value.extend(exe.as_os_str().encode_wide());
        value.extend(std::ffi::OsStr::new("\" --background").encode_wide());
        value.push(0);
        let bytes =
            unsafe { std::slice::from_raw_parts(value.as_ptr().cast::<u8>(), value.len() * 2) };
        let name = name();
        let status =
            unsafe { RegSetValueExW(key.0, PCWSTR(name.as_ptr()), None, REG_SZ, Some(bytes)) };
        if status.0 == 0 {
            Ok(())
        } else {
            Err(format!("set Run value: {status:?}"))
        }
    }
    pub fn remove() -> Result<(), String> {
        let key = match open(KEY_SET_VALUE) {
            Ok(key) => key,
            Err(e) if e == ERROR_FILE_NOT_FOUND => return Ok(()),
            Err(e) => return Err(format!("open Run key: {e:?}")),
        };
        let name = name();
        let status = unsafe { RegDeleteValueW(key.0, PCWSTR(name.as_ptr())) };
        if status.0 == 0 || status == ERROR_FILE_NOT_FOUND {
            Ok(())
        } else {
            Err(format!("delete Run value: {status:?}"))
        }
    }
    pub fn present() -> bool {
        let Ok(key) = open(KEY_QUERY_VALUE) else {
            return false;
        };
        let name = name();
        unsafe { RegQueryValueExW(key.0, PCWSTR(name.as_ptr()), None, None, None, None).0 == 0 }
    }
}

#[cfg(target_os = "macos")]
mod imp {
    pub fn install() -> Result<(), String> {
        Err("autostart is not wired on macOS".into())
    }
    pub fn remove() -> Result<(), String> {
        Ok(())
    }
    pub fn present() -> bool {
        false
    }
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;

    #[test]
    fn the_entry_is_written_and_removed() {
        let dir = tempfile::tempdir().unwrap();
        // This process is the only one reading XDG_CONFIG_HOME in this test binary.
        // SAFETY: single-threaded test body; nothing else reads the environment here.
        unsafe { std::env::set_var("XDG_CONFIG_HOME", dir.path()) };
        assert!(!is_enabled());
        set(true).unwrap();
        assert!(is_enabled());
        let text = std::fs::read_to_string(super::imp::entry()).unwrap();
        assert!(text.contains("Exec="), "{text}");
        assert!(text.contains("--background"), "{text}");
        assert!(
            text.contains(&format!("Name={}", identity::APP_NAME)),
            "{text}"
        );
        set(false).unwrap();
        assert!(!is_enabled());
    }
}
