//! Free-space accounting, checked *before* a job starts so a 20-minute encode
//! cannot die at 90 % with a full disk.

use std::path::{Path, PathBuf};

/// Bytes free on the filesystem that holds `path`, or `None` when it cannot be
/// determined (then the pre-check is skipped rather than failing the job).
pub fn available_bytes(path: &Path) -> Option<u64> {
    let target = nearest_existing(path)?;
    imp::available(&target)
}

fn nearest_existing(path: &Path) -> Option<PathBuf> {
    let mut cur = path.to_path_buf();
    loop {
        if cur.exists() {
            return Some(cur);
        }
        if !cur.pop() {
            return None;
        }
    }
}

#[cfg(unix)]
mod imp {
    use super::*;
    use std::os::unix::ffi::OsStrExt;

    pub fn available(path: &Path) -> Option<u64> {
        let c = std::ffi::CString::new(path.as_os_str().as_bytes()).ok()?;
        // SAFETY: `statvfs` only writes into the provided struct; the path is a
        // valid NUL-terminated C string owned by `c` for the duration of the call.
        unsafe {
            let mut st: libc::statvfs = std::mem::zeroed();
            if libc::statvfs(c.as_ptr(), &mut st) != 0 {
                return None;
            }
            Some(st.f_bavail as u64 * st.f_frsize as u64)
        }
    }
}

#[cfg(windows)]
mod imp {
    use super::*;
    use std::os::windows::ffi::OsStrExt;
    use windows::Win32::Storage::FileSystem::GetDiskFreeSpaceExW;
    use windows::core::PCWSTR;

    pub fn available(path: &Path) -> Option<u64> {
        let mut wide: Vec<u16> = path.as_os_str().encode_wide().collect();
        wide.push(0);
        let mut free: u64 = 0;
        // SAFETY: the wide string is NUL-terminated and outlives the call; the
        // out parameter is a valid `*mut u64`.
        unsafe {
            GetDiskFreeSpaceExW(
                PCWSTR(wide.as_ptr()),
                Some(&mut free as *mut u64 as *mut _),
                None,
                None,
            )
            .ok()?;
        }
        Some(free)
    }
}

/// Refuse the job when either the work dir or the target dir cannot hold the
/// estimated output plus a small margin.
pub fn precheck(
    work_dir: &Path,
    target_dir: &Path,
    estimated: u64,
) -> Result<(), crate::error::JobError> {
    let needed = estimated.saturating_add(8 * 1024 * 1024);
    for dir in [work_dir, target_dir] {
        if let Some(free) = available_bytes(dir)
            && free < needed
        {
            return Err(crate::error::JobError::new(
                crate::error::ErrorCode::DiskFull,
                format!(
                    "{} has {:.1} MB free, this job needs about {:.1} MB",
                    dir.display(),
                    free as f64 / 1e6,
                    needed as f64 / 1e6
                ),
            ));
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn reads_free_space_for_an_existing_dir() {
        let dir = tempfile::tempdir().unwrap();
        let free = available_bytes(dir.path()).expect("statvfs should work");
        assert!(free > 0);
    }

    #[test]
    fn resolves_the_nearest_existing_ancestor() {
        let dir = tempfile::tempdir().unwrap();
        let deep = dir.path().join("a/b/c");
        assert!(available_bytes(&deep).is_some());
    }
}
