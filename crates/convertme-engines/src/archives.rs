//! Archives: ZIP ↔ TAR ↔ GZIP conversion and the `Extract` tool, all through
//! 7-Zip.
//!
//! Every job unpacks the source archive into scratch space first and then either
//! repacks that tree or moves it into `out/`. The archive therefore never
//! contains itself as a member, and the user's file is only ever read.

use crate::proc::{self, Budget, Cmd};
use crate::{Context, ensure_file};
use convertme_core::capability::ToolKey;
use convertme_core::error::{ErrorCode, JobError};
use convertme_core::formats::{FormatId, ToolId};
use convertme_core::job::{Intent, Job, JobCtx};
use std::ffi::OsString;
use std::path::{Path, PathBuf};
use walkdir::WalkDir;

/// Entry point used by the dispatcher.
pub fn run(job: &Job, cx: &Context<'_>) -> Result<Vec<PathBuf>, JobError> {
    let seven = cx.tools.path(ToolKey::SevenZip)?;
    match &job.intent {
        Intent::Convert { target } => convert(job, *target, &seven, cx),
        Intent::Tool { tool, .. } if *tool == ToolId::Extract => extract(job, &seven, cx),
        Intent::Tool { tool, .. } => Err(JobError::new(
            ErrorCode::Unsupported,
            format!("{} is not an archive tool", tool.as_str()),
        )),
    }
}

fn convert(
    job: &Job,
    target: FormatId,
    seven: &Path,
    cx: &Context<'_>,
) -> Result<Vec<PathBuf>, JobError> {
    let format = format_for(target)?;
    let unpack_dir = cx.scratch("unpack");
    reset_dir(&unpack_dir)?;

    cx.stage("Unpacking the archive");
    unpack(seven, &job.source, &unpack_dir, cx.ctx, (0.02, 0.55))?;
    let produced = entries(&unpack_dir)?;
    if produced.is_empty() {
        return Err(empty_archive());
    }

    cx.ctx.checkpoint()?;
    cx.ctx.set_progress(0.6);
    cx.stage("Creating the archive");

    let artifact = cx.out(&format!("{}.{}", stem(&job.source), crate::ext_of(target)));
    pack(seven, &unpack_dir, &artifact, format, cx.ctx, (0.6, 0.95))?;
    ensure_file(&artifact, "7-Zip")?;
    cx.ctx.set_progress(1.0);
    Ok(vec![artifact])
}

fn extract(job: &Job, seven: &Path, cx: &Context<'_>) -> Result<Vec<PathBuf>, JobError> {
    let unpack_dir = cx.scratch("unpack");
    reset_dir(&unpack_dir)?;

    cx.stage("Extracting the archive");
    unpack(seven, &job.source, &unpack_dir, cx.ctx, (0.02, 0.9))?;
    let produced = entries(&unpack_dir)?;
    if produced.is_empty() {
        return Err(empty_archive());
    }

    cx.ctx.checkpoint()?;
    cx.ctx.set_progress(0.95);
    cx.stage("Preparing the extracted folder");

    // The queue publishes a directory artifact as `<stem>-extract`.
    let artifact = cx.out(&stem(&job.source));
    std::fs::rename(&unpack_dir, &artifact)
        .map_err(|e| JobError::io("moving the extracted folder into the output directory", &e))?;
    cx.ctx.set_progress(1.0);
    Ok(vec![artifact])
}

/// Extract `archive` below `dest`, reporting 7-Zip's percentage when it prints
/// one. `range` is the slice of the job progress this step may occupy.
fn unpack(
    seven: &Path,
    archive: &Path,
    dest: &Path,
    ctx: &JobCtx,
    range: (f64, f64),
) -> Result<(), JobError> {
    std::fs::create_dir_all(dest).map_err(|e| JobError::io("creating the unpack directory", &e))?;
    let (lo, hi) = range;
    let output = proc::run_with_lines(
        Cmd::new(seven)
            .arg("x")
            .arg(prefixed("-o", dest))
            .arg("-y")
            .arg("-bb1")
            .arg(archive)
            .stdout_piped()
            .budget(Budget::Short),
        ctx,
        |line| {
            if line.contains("Everything is Ok") {
                ctx.set_progress(hi);
            } else if let Some(fraction) = percent(line) {
                ctx.set_progress(lo + (hi - lo) * fraction);
            }
        },
    )?;
    output.ok()?;
    Ok(())
}

/// Add every top-level entry of `dir` (recursively) to `archive`.
fn pack(
    seven: &Path,
    dir: &Path,
    archive: &Path,
    format: &str,
    ctx: &JobCtx,
    range: (f64, f64),
) -> Result<(), JobError> {
    let mut names: Vec<OsString> = std::fs::read_dir(dir)
        .map_err(|e| JobError::io("reading the unpacked archive", &e))?
        .map(|entry| {
            entry
                .map(|e| e.file_name())
                .map_err(|e| JobError::io("reading the unpacked archive", &e))
        })
        .collect::<Result<_, _>>()?;
    names.sort();

    if names.is_empty() {
        return Err(empty_archive());
    }

    let (lo, hi) = range;
    let mut cmd = Cmd::new(seven)
        .arg("a")
        .arg(format!("-t{format}"))
        .arg(archive)
        .arg("-bb1")
        .arg("-r")
        // A file named `-foo` is a member, not a switch.
        .arg("--");
    for name in &names {
        cmd = cmd.arg(name);
    }

    let output = proc::run_with_lines(
        cmd.cwd(dir).stdout_piped().budget(Budget::Short),
        ctx,
        |line| {
            if line.contains("Everything is Ok") {
                ctx.set_progress(hi);
            } else if let Some(fraction) = percent(line) {
                ctx.set_progress(lo + (hi - lo) * fraction);
            }
        },
    )?;
    output.ok()?;
    Ok(())
}

fn format_for(target: FormatId) -> Result<&'static str, JobError> {
    match target.0 {
        "ZIP" => Ok("zip"),
        "TAR" => Ok("tar"),
        "GZIP" => Ok("gzip"),
        "RAR" => Err(JobError::new(
            ErrorCode::Unsupported,
            "RAR is read-only: no RAR encoder is bundled",
        )),
        other => Err(JobError::new(
            ErrorCode::Unsupported,
            format!("{other} is not an archive target"),
        )),
    }
}

/// Every path below `root`, refusing anything that does not stay inside it. A
/// traversal entry would otherwise make `Extract` publish outside the folder.
fn entries(root: &Path) -> Result<Vec<PathBuf>, JobError> {
    let mut found = Vec::new();
    for entry in WalkDir::new(root).min_depth(1) {
        let entry = entry.map_err(|e| {
            JobError::new(
                ErrorCode::EngineFailed,
                format!("reading the extracted archive: {e}"),
            )
        })?;
        let path = entry.path();
        if !path.starts_with(root) {
            return Err(JobError::new(
                ErrorCode::EngineFailed,
                format!(
                    "the archive contains an entry that escapes the output folder: {}",
                    path.display()
                ),
            ));
        }
        found.push(path.to_path_buf());
    }
    Ok(found)
}

fn reset_dir(dir: &Path) -> Result<(), JobError> {
    match std::fs::remove_dir_all(dir) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(e) => return Err(JobError::io("clearing the unpack directory", &e)),
    }
    std::fs::create_dir_all(dir).map_err(|e| JobError::io("creating the unpack directory", &e))
}

fn stem(source: &Path) -> String {
    source
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "archive".to_string())
}

/// 7-Zip draws progress as ` 45%`, often followed by backspaces on a terminal.
fn percent(line: &str) -> Option<f64> {
    let bytes = line.as_bytes();
    let mut found = None;
    let mut i = 0;
    while i < bytes.len() {
        if bytes[i].is_ascii_digit() {
            let start = i;
            while i < bytes.len() && bytes[i].is_ascii_digit() {
                i += 1;
            }
            if i < bytes.len()
                && bytes[i] == b'%'
                && let Ok(value) = line[start..i].parse::<u32>()
            {
                found = Some(value.min(100) as f64 / 100.0);
            }
        } else {
            i += 1;
        }
    }
    found
}

fn prefixed(prefix: &str, path: &Path) -> OsString {
    let mut arg = OsString::from(prefix);
    arg.push(path.as_os_str());
    arg
}

fn empty_archive() -> JobError {
    JobError::new(ErrorCode::EngineFailed, "the archive has no entries")
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Toolset;
    use crate::probe;
    use convertme_core::capability::Capabilities;
    use convertme_core::job::WorkDir;
    use parking_lot::RwLock;
    use std::fs;
    use std::sync::{Arc, LazyLock};

    static TOOLSET: LazyLock<Option<Toolset>> = LazyLock::new(|| {
        let opts = probe::ProbeOptions::from_env();
        let capability = probe::probe_one(ToolKey::SevenZip, &opts);
        if !capability.ok {
            return None;
        }
        let mut caps = Capabilities::default();
        caps.put(capability);
        Some(Toolset::new(Arc::new(RwLock::new(caps)), opts))
    });

    fn toolset() -> Option<Toolset> {
        TOOLSET.clone()
    }

    fn make_zip(seven: &Path, dir: &Path, archive: &Path, names: &[&str]) {
        let output = proc::run(
            Cmd::new(seven)
                .arg("a")
                .arg("-tzip")
                .arg(archive)
                .arg("-r")
                .arg("--")
                .args(names)
                .cwd(dir),
            &JobCtx::new(),
        )
        .expect("starting 7-Zip");
        output.ok().expect("7-Zip creates the fixture zip");
    }

    fn make_gzip(seven: &Path, dir: &Path, archive: &Path, name: &str) {
        let output = proc::run(
            Cmd::new(seven)
                .arg("a")
                .arg("-tgzip")
                .arg(archive)
                .arg("-r")
                .arg("--")
                .arg(name)
                .cwd(dir),
            &JobCtx::new(),
        )
        .expect("starting 7-Zip");
        output.ok().expect("7-Zip creates the fixture gzip");
    }

    fn run_job(
        tools: &Toolset,
        source: &Path,
        intent: Intent,
        root: &Path,
    ) -> Result<Vec<PathBuf>, JobError> {
        let work = WorkDir::create(root.join("work")).expect("work dir");
        let job = Job::new(source.to_path_buf(), intent);
        let job_ctx = JobCtx::new();
        let cx = Context {
            tools,
            work: &work,
            ctx: &job_ctx,
            preview: None,
        };
        run(&job, &cx)
    }

    fn file_names(dir: &Path) -> Vec<String> {
        entries(dir)
            .expect("walk")
            .into_iter()
            .filter(|p| p.is_file())
            .filter_map(|p| p.file_name().map(|n| n.to_string_lossy().into_owned()))
            .collect()
    }

    #[test]
    fn extract_produces_a_folder_with_every_entry() {
        let Some(tools) = toolset() else { return };
        let seven = tools.path(ToolKey::SevenZip).expect("7zz");
        let tmp = tempfile::tempdir().expect("tempdir");
        let src = tmp.path().join("src");
        fs::create_dir_all(&src).expect("src dir");
        fs::write(src.join("one.txt"), b"one").expect("one");
        fs::write(src.join("two.txt"), b"two").expect("two");
        let zip = tmp.path().join("bundle.zip");
        make_zip(&seven, &src, &zip, &["one.txt", "two.txt"]);

        let artifacts = run_job(&tools, &zip, Intent::tool(ToolId::Extract), tmp.path())
            .expect("extract succeeds");
        assert_eq!(artifacts.len(), 1);
        assert!(artifacts[0].is_dir());
        let mut names = file_names(&artifacts[0]);
        names.sort();
        assert_eq!(names, ["one.txt", "two.txt"]);
    }

    #[test]
    fn convert_zip_to_tar_keeps_both_entries() {
        let Some(tools) = toolset() else { return };
        let seven = tools.path(ToolKey::SevenZip).expect("7zz");
        let tmp = tempfile::tempdir().expect("tempdir");
        let src = tmp.path().join("src");
        fs::create_dir_all(&src).expect("src dir");
        fs::write(src.join("one.txt"), b"one").expect("one");
        fs::write(src.join("two.txt"), b"two").expect("two");
        let zip = tmp.path().join("bundle.zip");
        make_zip(&seven, &src, &zip, &["one.txt", "two.txt"]);

        let artifacts = run_job(
            &tools,
            &zip,
            Intent::Convert {
                target: FormatId("TAR"),
            },
            tmp.path(),
        )
        .expect("convert succeeds");
        assert_eq!(artifacts.len(), 1);
        assert_eq!(
            artifacts[0].extension().and_then(|e| e.to_str()),
            Some("tar")
        );

        let listing = proc::run(
            Cmd::new(&seven).arg("l").arg(&artifacts[0]).stdout_piped(),
            &JobCtx::new(),
        )
        .expect("listing the tar");
        let listing = listing.ok().expect("7-Zip reads the produced tar");
        assert!(listing.stdout.contains("one.txt"), "{}", listing.stdout);
        assert!(listing.stdout.contains("two.txt"), "{}", listing.stdout);
    }

    #[test]
    fn gzip_roundtrip_keeps_the_single_file() {
        let Some(tools) = toolset() else { return };
        let seven = tools.path(ToolKey::SevenZip).expect("7zz");
        let tmp = tempfile::tempdir().expect("tempdir");
        let src = tmp.path().join("src");
        fs::create_dir_all(&src).expect("src dir");
        fs::write(src.join("solo.txt"), b"solo").expect("solo");
        let gz = tmp.path().join("solo.gz");
        make_gzip(&seven, &src, &gz, "solo.txt");

        let artifacts = run_job(
            &tools,
            &gz,
            Intent::Convert {
                target: FormatId("GZIP"),
            },
            tmp.path(),
        )
        .expect("gzip roundtrip succeeds");
        assert_eq!(artifacts.len(), 1);
        assert_eq!(
            artifacts[0].extension().and_then(|e| e.to_str()),
            Some("gz")
        );

        let restored = tmp.path().join("restored");
        let output = proc::run(
            Cmd::new(&seven)
                .arg("x")
                .arg(prefixed("-o", &restored))
                .arg("-y")
                .arg(&artifacts[0]),
            &JobCtx::new(),
        )
        .expect("extracting the roundtripped gzip");
        output.ok().expect("7-Zip reads the produced gzip");
        assert_eq!(
            fs::read(restored.join("solo.txt")).expect("restored file"),
            b"solo"
        );
    }

    #[test]
    fn empty_archive_is_engine_failed() {
        let Some(tools) = toolset() else { return };
        let tmp = tempfile::tempdir().expect("tempdir");
        let empty = tmp.path().join("empty.zip");
        // A ZIP end-of-central-directory record with zero entries.
        fs::write(
            &empty,
            b"PK\x05\x06\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00\x00",
        )
        .expect("empty zip");

        let error = run_job(&tools, &empty, Intent::tool(ToolId::Extract), tmp.path())
            .expect_err("an empty archive must not produce an artifact");
        assert_eq!(error.code, ErrorCode::EngineFailed);
        assert!(error.message.contains("no entries"), "{}", error.message);
    }
}
