//! Atomic publish: an artifact produced in the work dir becomes a user-visible
//! file by a rename *inside the destination directory*, never by writing the
//! final path directly. A kill, a full disk or a cancelled job therefore leaves
//! the user's directory exactly as it was.

use crate::error::{ErrorCode, JobError};
use crate::naming::{self, Overwrite};
use std::path::{Path, PathBuf};

/// Publish one file. Returns the path it actually landed on.
pub fn publish_file(
    artifact: &Path,
    desired: &Path,
    policy: Overwrite,
) -> Result<PathBuf, JobError> {
    let dir = desired.parent().unwrap_or_else(|| Path::new("."));
    if !dir_is_writable(dir) {
        return Err(JobError::new(
            ErrorCode::Permission,
            format!("{} is not writable", dir.display()),
        ));
    }
    std::fs::create_dir_all(dir).map_err(|e| JobError::io("creating the output directory", &e))?;

    // Re-resolve inside the destination dir in case the caller's name is stale.
    let final_path = if policy == Overwrite::Replace {
        desired.to_path_buf()
    } else {
        dedupe(desired, policy)
    };

    match std::fs::rename(artifact, &final_path) {
        Ok(()) => {}
        Err(e) if is_cross_device(&e) => {
            // Different mount (network share, bind mount): copy to a temp file in
            // the destination directory, then rename — never a partial final file.
            let tmp = final_path.with_extension(format!(
                "{}.convertme-tmp",
                final_path
                    .extension()
                    .map(|e| e.to_string_lossy().into_owned())
                    .unwrap_or_default()
            ));
            std::fs::copy(artifact, &tmp)
                .map_err(|e| JobError::io("copying across filesystems", &e))?;
            std::fs::rename(&tmp, &final_path).map_err(|e| {
                let _ = std::fs::remove_file(&tmp);
                JobError::io("publishing across filesystems", &e)
            })?;
        }
        Err(e) => return Err(JobError::io("publishing the result", &e)),
    }
    Ok(final_path)
}

/// Publish a directory (archive extraction, split output, page export).
pub fn publish_dir(artifact: &Path, desired: &Path) -> Result<PathBuf, JobError> {
    let dir = desired.parent().unwrap_or_else(|| Path::new("."));
    std::fs::create_dir_all(dir).map_err(|e| JobError::io("creating the output directory", &e))?;
    let mut candidate = desired.to_path_buf();
    let mut n = 2;
    while candidate.exists() {
        candidate = dir.join(format!(
            "{} ({n})",
            desired
                .file_name()
                .map(|f| f.to_string_lossy().into_owned())
                .unwrap_or_default()
        ));
        n += 1;
        if n > 9999 {
            break;
        }
    }
    match std::fs::rename(artifact, &candidate) {
        Ok(()) => Ok(candidate),
        Err(e) if is_cross_device(&e) => {
            copy_dir_all(artifact, &candidate)?;
            Ok(candidate)
        }
        Err(e) => Err(JobError::io("publishing the folder", &e)),
    }
}

fn copy_dir_all(from: &Path, to: &Path) -> Result<(), JobError> {
    std::fs::create_dir_all(to).map_err(|e| JobError::io("creating the output folder", &e))?;
    for entry in walkdir::WalkDir::new(from).min_depth(1) {
        let entry =
            entry.map_err(|e| JobError::new(ErrorCode::Io, format!("walking {from:?}: {e}")))?;
        let rel = entry
            .path()
            .strip_prefix(from)
            .map_err(|_| JobError::new(ErrorCode::Io, "path escaped the work dir"))?;
        let dst = to.join(rel);
        if entry.file_type().is_dir() {
            std::fs::create_dir_all(&dst).map_err(|e| JobError::io("creating a folder", &e))?;
        } else {
            if let Some(parent) = dst.parent() {
                std::fs::create_dir_all(parent)
                    .map_err(|e| JobError::io("creating a folder", &e))?;
            }
            std::fs::copy(entry.path(), &dst).map_err(|e| JobError::io("copying a file", &e))?;
        }
    }
    Ok(())
}

fn dedupe(desired: &Path, policy: Overwrite) -> PathBuf {
    if policy != Overwrite::Never && policy != Overwrite::Ask {
        return desired.to_path_buf();
    }
    if !desired.exists() {
        return desired.to_path_buf();
    }
    let dir = desired.parent().unwrap_or_else(|| Path::new("."));
    let stem = desired
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let ext = desired
        .extension()
        .map(|e| e.to_string_lossy().into_owned())
        .unwrap_or_default();
    let mut n = 2;
    loop {
        let name = if ext.is_empty() {
            format!("{stem} ({n})")
        } else {
            format!("{stem} ({n}).{ext}")
        };
        let candidate = dir.join(name);
        if !candidate.exists() {
            return candidate;
        }
        n += 1;
        if n > 9999 {
            return desired.to_path_buf();
        }
    }
}

fn is_cross_device(e: &std::io::Error) -> bool {
    #[cfg(unix)]
    {
        e.raw_os_error() == Some(libc::EXDEV)
    }
    #[cfg(windows)]
    {
        e.raw_os_error() == Some(17) // ERROR_NOT_SAME_DEVICE
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = e;
        false
    }
}

/// Whether `dir` accepts new files. Probes by creating and removing a temp file;
/// cheaper than parsing mode bits, and correct for ACLs, mounts and Flatpak
/// sandboxes.
pub fn dir_is_writable(dir: &Path) -> bool {
    if !dir.is_dir() {
        return false;
    }
    // Unique per call: two workers probing the same directory at once must not
    // read each other's `create_new` failure as "not writable".
    static PROBE_SEQ: std::sync::atomic::AtomicU64 = std::sync::atomic::AtomicU64::new(0);
    let seq = PROBE_SEQ.fetch_add(1, std::sync::atomic::Ordering::Relaxed);
    let probe = dir.join(format!(".convertme-probe-{}-{seq}", std::process::id()));
    match std::fs::OpenOptions::new()
        .write(true)
        .create_new(true)
        .open(&probe)
    {
        Ok(_) => {
            let _ = std::fs::remove_file(&probe);
            true
        }
        Err(_) => false,
    }
}

/// Pick the publish directory: the source's own directory when writable,
/// otherwise the Downloads fallback (named in the toast by the caller).
pub fn publish_dir_for(source: &Path) -> (PathBuf, bool) {
    let own = source
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    if dir_is_writable(&own) {
        (own, false)
    } else {
        let fallback = naming::fallback_dir();
        let _ = std::fs::create_dir_all(&fallback);
        (fallback, true)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn publish_never_replaces_and_never_leaves_a_partial_file() {
        let dir = tempfile::tempdir().unwrap();
        let src = dir.path().join("out.webp");
        std::fs::write(&src, b"one").unwrap();
        let first = publish_file(&src, &dir.path().join("photo.webp"), Overwrite::Never).unwrap();
        assert_eq!(std::fs::read(&first).unwrap(), b"one");

        let src2 = dir.path().join("out2.webp");
        std::fs::write(&src2, b"two").unwrap();
        let second = publish_file(&src2, &dir.path().join("photo.webp"), Overwrite::Never).unwrap();
        assert_eq!(second.file_name().unwrap(), "photo (2).webp");
        assert_eq!(std::fs::read(&first).unwrap(), b"one");
        assert_eq!(std::fs::read(&second).unwrap(), b"two");
        // No temp files left behind.
        let leftovers: Vec<_> = std::fs::read_dir(dir.path())
            .unwrap()
            .filter_map(|e| e.ok())
            .filter(|e| e.file_name().to_string_lossy().contains("convertme-tmp"))
            .collect();
        assert!(leftovers.is_empty());
    }

    #[test]
    fn read_only_directory_is_reported_and_falls_back() {
        let dir = tempfile::tempdir().unwrap();
        let ro = dir.path().join("ro");
        std::fs::create_dir(&ro).unwrap();
        let mut perms = std::fs::metadata(&ro).unwrap().permissions();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            perms.set_mode(0o555);
        }
        std::fs::set_permissions(&ro, perms).unwrap();

        if !dir_is_writable(&ro) {
            let (target, fallback) = publish_dir_for(&ro.join("x.png"));
            assert!(fallback);
            assert!(!target.starts_with(&ro));
        }
        // Restore so the tempdir can be removed.
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mut p = std::fs::metadata(&ro).unwrap().permissions();
            p.set_mode(0o755);
            std::fs::set_permissions(&ro, p).unwrap();
        }
    }

    #[test]
    fn publish_dir_appends_a_suffix_on_collision() {
        let dir = tempfile::tempdir().unwrap();
        let art = dir.path().join("stage");
        std::fs::create_dir_all(art.join("inner")).unwrap();
        std::fs::write(art.join("inner/a.txt"), b"a").unwrap();
        let target = dir.path().join("photos-extract");
        let once = publish_dir(&art, &target).unwrap();
        assert_eq!(once, target);
        assert_eq!(std::fs::read(target.join("inner/a.txt")).unwrap(), b"a");
    }
}
