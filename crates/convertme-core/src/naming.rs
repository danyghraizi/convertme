//! Output naming. This module is the whole of the "never touch the source"
//! naming rule, in one place, with no I/O other than the writability probe the
//! caller asks for.
//!
//! Rules (from the product invariant):
//! * `<stem>.<ext>` normally;
//! * if that lands on the source path (case-folded) → `<stem>-converted.<ext>`,
//!   so `PNG → PNG` on `logo.png` yields `logo-converted.png`;
//! * tool output is `<stem>-<tool>.<ext>`;
//! * an existing file is never replaced under [`Overwrite::Never`]; ` (2)`,
//!   ` (3)` … is appended instead.

use std::path::{Path, PathBuf};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum Overwrite {
    /// Default: never replace an existing output.
    #[default]
    Never,
    /// Ask the user, which the queue resolves by treating it as `Never` until a
    /// surface answers; a UI that asks passes `Replace` when the user agrees.
    Ask,
    Replace,
}

impl std::str::FromStr for Overwrite {
    type Err = std::convert::Infallible;

    fn from_str(s: &str) -> Result<Self, Self::Err> {
        Ok(match s {
            "replace" => Overwrite::Replace,
            "ask" => Overwrite::Ask,
            _ => Overwrite::Never,
        })
    }
}

impl Overwrite {
    /// Lenient parse for settings and CLI flags: anything unknown means `never`.
    pub fn parse(s: &str) -> Self {
        <Self as std::str::FromStr>::from_str(s).unwrap_or(Overwrite::Never)
    }

    pub fn as_str(self) -> &'static str {
        match self {
            Overwrite::Never => "never",
            Overwrite::Ask => "ask",
            Overwrite::Replace => "replace",
        }
    }
}

/// Where `path`'s siblings go when their own directory is not writable.
pub fn fallback_dir() -> PathBuf {
    crate::identity::downloads_dir()
}

/// Resolve the publish path for a conversion artifact.
///
/// `source` is only ever read for its directory and stem.
pub fn resolve_output(
    source: &Path,
    ext: &str,
    policy: Overwrite,
    suffix: Option<&str>,
) -> PathBuf {
    resolve_with(source, ext, policy, suffix, |p| p.exists())
}

/// Same as [`resolve_output`] but with an injectable existence test, so the
/// collision rules are unit-testable without touching the filesystem.
pub fn resolve_with(
    source: &Path,
    ext: &str,
    policy: Overwrite,
    suffix: Option<&str>,
    exists: impl Fn(&Path) -> bool,
) -> PathBuf {
    let dir = source
        .parent()
        .map(Path::to_path_buf)
        .unwrap_or_else(|| PathBuf::from("."));
    let stem = source
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "output".to_string());
    let ext = ext.trim_start_matches('.').to_ascii_lowercase();
    let base = match suffix {
        Some(s) if !s.is_empty() => format!("{stem}-{s}"),
        _ => stem,
    };

    let source_name = source
        .file_name()
        .map(|n| n.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_default();

    let mut candidate_stem = base.clone();
    if format!("{candidate_stem}.{ext}").to_ascii_lowercase() == source_name {
        candidate_stem = format!("{base}-converted");
    }

    let mut candidate = dir.join(format!("{candidate_stem}.{ext}"));
    if policy == Overwrite::Replace {
        return candidate;
    }
    let mut n = 2u32;
    while exists(&candidate) {
        // Never, and no policy at all, may select the source path.
        candidate = dir.join(format!("{candidate_stem} ({n}).{ext}"));
        n += 1;
        if n > 9999 {
            break;
        }
    }
    candidate
}

/// True when `candidate` is the same file as `source` (by name, case-folded).
pub fn same_path(a: &Path, b: &Path) -> bool {
    if let (Ok(ca), Ok(cb)) = (std::fs::canonicalize(a), std::fs::canonicalize(b)) {
        return ca == cb;
    }
    a.to_string_lossy().to_ascii_lowercase() == b.to_string_lossy().to_ascii_lowercase()
}

#[cfg(test)]
mod tests {
    use super::*;

    fn none(_: &Path) -> bool {
        false
    }

    #[test]
    fn png_to_png_becomes_converted() {
        let out = resolve_with(
            Path::new("/d/logo.png"),
            "png",
            Overwrite::Never,
            None,
            none,
        );
        assert_eq!(out, Path::new("/d/logo-converted.png"));
    }

    #[test]
    fn different_extension_keeps_the_stem() {
        let out = resolve_with(
            Path::new("/d/photo.jpg"),
            "webp",
            Overwrite::Never,
            None,
            none,
        );
        assert_eq!(out, Path::new("/d/photo.webp"));
    }

    #[test]
    fn naming_collision_appends_suffix() {
        // `photo.webp` exists → `photo (2).webp`; `photo (2).webp` exists too → `(3)`.
        let existing = ["/d/photo.webp", "/d/photo (2).webp"];
        let out = resolve_with(
            Path::new("/d/photo.jpg"),
            "webp",
            Overwrite::Never,
            None,
            |p| existing.contains(&p.to_str().unwrap()),
        );
        assert_eq!(out, Path::new("/d/photo (3).webp"));
    }

    #[test]
    fn replace_policy_overwrites_existing_output_only() {
        let out = resolve_with(
            Path::new("/d/photo.jpg"),
            "webp",
            Overwrite::Replace,
            None,
            |_| true,
        );
        assert_eq!(out, Path::new("/d/photo.webp"));
        // …and still refuses the source path.
        let same = resolve_with(
            Path::new("/d/photo.webp"),
            "webp",
            Overwrite::Replace,
            None,
            |_| true,
        );
        assert_eq!(same, Path::new("/d/photo-converted.webp"));
    }

    #[test]
    fn tool_suffix_is_applied() {
        let out = resolve_with(
            Path::new("/d/clip.mp4"),
            "mp4",
            Overwrite::Never,
            Some("compressed"),
            none,
        );
        assert_eq!(out, Path::new("/d/clip-compressed.mp4"));
    }

    #[test]
    fn case_insensitive_source_match_is_treated_as_the_source() {
        let out = resolve_with(
            Path::new("/d/Photo.PNG"),
            "png",
            Overwrite::Never,
            None,
            none,
        );
        assert_eq!(out, Path::new("/d/Photo-converted.png"));
    }

    #[test]
    fn all_collision_suffixes_are_never_the_source() {
        let source = Path::new("/d/a.png");
        for policy in [Overwrite::Never, Overwrite::Ask, Overwrite::Replace] {
            let out = resolve_with(source, "png", policy, None, |_| false);
            assert!(!same_path(source, &out), "{policy:?} → {out:?}");
        }
    }
}
