//! Metadata editing and stripping, for every format family that has a real
//! writer. Read-only when the format has none: the job fails with `Unsupported`
//! instead of pretending.
//!
//! All engines in this family are in-place by nature, so they run on a staged copy
//! (`cx.copy_of`) and the original is structurally out of reach.

use crate::proc::{self, Cmd};
use crate::{Context, ensure_file};
use convertme_core::error::{ErrorCode, JobError};
use convertme_core::formats::{FormatId, Registry, ToolId};
use convertme_core::job::Job;
use std::collections::BTreeMap;
use std::path::{Path, PathBuf};

/// The tag names each format family accepts in `params.set`.
pub fn editable_tags(format: FormatId) -> &'static [&'static str] {
    match format.0 {
        "JPG" | "HEIC" => &[
            "Title",
            "Description",
            "Artist",
            "Copyright",
            "DateTimeOriginal",
            "Make",
            "Model",
            "LensModel",
            "ExposureTime",
            "FNumber",
            "ISO",
            "FocalLength",
            "Orientation",
            "GPSLatitude",
            "GPSLongitude",
            "GPSAltitude",
        ],
        "MP3" | "M4A" | "FLAC" | "OGG" | "OPUS" | "WAV" | "AIFF" | "WMA" => &[
            "Title",
            "Artist",
            "Album",
            "AlbumArtist",
            "TrackNumber",
            "DiscNumber",
            "Year",
            "Genre",
            "Comment",
            "Composer",
        ],
        "MP4" | "MOV" | "M4V" => &["Title", "Artist", "Album", "Year", "Comment", "Genre"],
        "PDF" => &[
            "Title", "Author", "Subject", "Keywords", "Creator", "Producer",
        ],
        _ => &[],
    }
}

/// `params.set` (object of tag → value) and `params.strip` (bool).
pub fn run(job: &Job, cx: &Context<'_>) -> Result<Vec<PathBuf>, JobError> {
    if job.intent.tool_id() != Some(ToolId::Metadata) {
        return Err(JobError::new(ErrorCode::Unsupported, "not a metadata job"));
    }
    let params = job.intent.params().cloned().unwrap_or_default();
    let strip = params
        .get("strip")
        .and_then(|v| v.as_bool())
        .unwrap_or(false);
    let set: BTreeMap<String, String> = params
        .get("set")
        .and_then(|v| v.as_object())
        .map(|obj| {
            obj.iter()
                .filter_map(|(k, v)| v.as_str().map(|s| (k.clone(), s.to_string())))
                .collect()
        })
        .unwrap_or_default();

    if !strip && set.is_empty() {
        return Err(JobError::new(
            ErrorCode::Unsupported,
            "nothing to change: give `set` tags or `strip: true`",
        ));
    }

    let format = job
        .input_format
        .or_else(|| Registry::detect(&job.source).map(|f| f.id))
        .ok_or_else(|| JobError::new(ErrorCode::Unsupported, "unknown input format"))?;

    let mut written = Vec::new();
    for (index, source) in job.inputs.iter().enumerate() {
        cx.ctx.checkpoint()?;
        let staged = cx.copy_of(index, job)?;
        apply(&staged, format, &set, strip, cx)?;
        let ext = source
            .extension()
            .map(|e| e.to_string_lossy().to_ascii_lowercase())
            .unwrap_or_else(|| crate::ext_of(format).to_string());
        let out = cx.out(&format!("{}-metadata.{ext}", stem_of(source)));
        std::fs::rename(&staged, &out)
            .map_err(|e| JobError::io("publishing the metadata copy", &e))?;
        ensure_file(&out, "the metadata writer")?;
        written.push(out);
        cx.ctx
            .set_progress((index + 1) as f64 / job.inputs.len() as f64);
    }
    Ok(written)
}

fn stem_of(path: &Path) -> String {
    path.file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_else(|| "file".into())
}

/// Read the tags the UI shows in the edit form.
pub fn read(path: &Path) -> BTreeMap<String, String> {
    let Some(format) = Registry::detect(path).map(|f| f.id) else {
        return BTreeMap::new();
    };
    let mut tags = BTreeMap::new();
    match format.0 {
        "JPG" | "HEIC" | "TIFF" => {
            if let Ok(file) = std::fs::File::open(path) {
                let mut buf = std::io::BufReader::new(file);
                if let Ok(exif) = exif::Reader::new().read_from_container(&mut buf) {
                    for f in exif.fields() {
                        tags.insert(
                            f.tag.to_string(),
                            f.display_value().with_unit(&exif).to_string(),
                        );
                    }
                }
            }
        }
        "MP3" | "M4A" | "FLAC" | "OGG" | "OPUS" | "WAV" | "AIFF" | "WMA" => {
            if let Ok(tagged) = lofty::read_from_path(path) {
                use lofty::prelude::{Accessor, TaggedFileExt};
                let tag = tagged.primary_tag().or_else(|| tagged.first_tag());
                if let Some(tag) = tag {
                    for (name, value) in [
                        ("Title", tag.title().map(|c| c.to_string())),
                        ("Artist", tag.artist().map(|c| c.to_string())),
                        ("Album", tag.album().map(|c| c.to_string())),
                        ("Comment", tag.comment().map(|c| c.to_string())),
                        (
                            "Year",
                            tag.get_string(lofty::tag::ItemKey::Year)
                                .map(str::to_string),
                        ),
                        ("Genre", tag.genre().map(|c| c.to_string())),
                    ] {
                        if let Some(v) = value {
                            tags.insert(name.to_string(), v);
                        }
                    }
                }
            }
        }
        "MP4" | "MOV" => {
            if let Ok(mp4) = mp4ameta::Tag::read_from_path(path) {
                for (name, value) in [
                    ("Title", mp4.title().map(str::to_string)),
                    ("Artist", mp4.artist().map(str::to_string)),
                    ("Album", mp4.album().map(str::to_string)),
                    ("Comment", mp4.comment().map(str::to_string)),
                ] {
                    if let Some(v) = value {
                        tags.insert(name.to_string(), v);
                    }
                }
            }
        }
        "PDF" => {
            if let Ok(doc) = lopdf::Document::load(path)
                && let Ok(info) = doc.trailer.get(b"Info").and_then(|o| o.as_dict())
            {
                for (k, v) in info.iter() {
                    if let Ok(s) = v.as_str() {
                        tags.insert(
                            String::from_utf8_lossy(k).into_owned(),
                            String::from_utf8_lossy(s).into_owned(),
                        );
                    }
                }
            }
        }
        _ => {}
    }
    tags
}

fn apply(
    path: &Path,
    format: FormatId,
    set: &BTreeMap<String, String>,
    strip: bool,
    cx: &Context<'_>,
) -> Result<(), JobError> {
    match format.0 {
        "JPG" | "HEIC" | "TIFF" | "PNG" | "WEBP" => {
            if editable_tags(format).is_empty() {
                // No EXIF writer for this container: fall back to ImageMagick,
                // which can also clear the profile wholesale.
                return exif_via_magick(path, set, strip, cx);
            }
            exif_direct(path, set, strip)
        }
        "MP3" | "M4A" | "FLAC" | "OGG" | "OPUS" | "WAV" | "AIFF" | "WMA" => {
            audio_tags(path, set, strip)
        }
        "MP4" | "MOV" | "M4V" => mp4_tags(path, set, strip),
        "PDF" => pdf_info(path, set, strip),
        _ => Err(JobError::new(
            ErrorCode::Unsupported,
            format!("{} has no writable metadata", format.0),
        )),
    }
}

fn exif_direct(path: &Path, set: &BTreeMap<String, String>, strip: bool) -> Result<(), JobError> {
    use little_exif::exif_tag::ExifTag;
    use little_exif::metadata::Metadata;

    let mut metadata = if strip {
        Metadata::new()
    } else {
        Metadata::new_from_path(path)
            .map_err(|e| JobError::new(ErrorCode::EngineFailed, format!("reading EXIF: {e}")))?
    };

    if strip {
        // Empty metadata replaces the block wholesale once written.
        metadata
            .write_to_file(path)
            .map_err(|e| JobError::new(ErrorCode::EngineFailed, format!("stripping EXIF: {e}")))?;
        return Ok(());
    }

    for (key, value) in set {
        let tag = match key.as_str() {
            "Title" => ExifTag::ImageDescription(value.clone()),
            "Description" => ExifTag::ImageDescription(value.clone()),
            "Artist" => ExifTag::Artist(value.clone()),
            "Copyright" => ExifTag::Copyright(value.clone()),
            "Make" => ExifTag::Make(value.clone()),
            "Model" => ExifTag::Model(value.clone()),
            "LensModel" => ExifTag::LensModel(value.clone()),
            "Orientation" => {
                let n: u16 = value.parse().map_err(|_| {
                    JobError::new(ErrorCode::Unsupported, "Orientation must be 1..8")
                })?;
                ExifTag::Orientation(vec![n])
            }
            other => {
                return Err(JobError::new(
                    ErrorCode::Unsupported,
                    format!("{other} is not writable for this format"),
                ));
            }
        };
        metadata.set_tag(tag);
    }
    metadata
        .write_to_file(path)
        .map_err(|e| JobError::new(ErrorCode::EngineFailed, format!("writing EXIF: {e}")))?;
    Ok(())
}

fn exif_via_magick(
    path: &Path,
    set: &BTreeMap<String, String>,
    strip: bool,
    cx: &Context<'_>,
) -> Result<(), JobError> {
    let magick = cx.tools.path(convertme_core::capability::ToolKey::Magick)?;
    let ext = path
        .extension()
        .map(|e| e.to_string_lossy().to_ascii_lowercase())
        .unwrap_or_else(|| "png".into());
    let staged_out = cx.scratch(&format!("meta-out.{ext}"));

    let mut cmd = Cmd::new(&magick).arg(path);
    if strip {
        cmd = cmd.arg("+profile").arg("*");
    }
    for (key, value) in set {
        match key.as_str() {
            "Title" | "Description" => {
                cmd = cmd.arg("-set").arg("comment").arg(value);
            }
            "Artist" => {
                cmd = cmd.arg("-set").arg("Artist").arg(value);
            }
            "Copyright" => {
                cmd = cmd.arg("-set").arg("Copyright").arg(value);
            }
            other => {
                return Err(JobError::new(
                    ErrorCode::Unsupported,
                    format!("{other} is not writable for this format"),
                ));
            }
        }
    }
    cmd = cmd.arg(&staged_out);
    proc::run(cmd, cx.ctx)?.ok()?;
    ensure_file(&staged_out, "the metadata writer")?;
    std::fs::rename(&staged_out, path)
        .map_err(|e| JobError::io("replacing the staged copy", &e))?;
    Ok(())
}

fn audio_tags(path: &Path, set: &BTreeMap<String, String>, strip: bool) -> Result<(), JobError> {
    use lofty::config::WriteOptions;
    use lofty::prelude::{Accessor, TagExt, TaggedFileExt};
    use lofty::probe::Probe;
    use lofty::tag::Tag;

    let mut tagged = Probe::open(path)
        .map_err(|e| JobError::new(ErrorCode::EngineFailed, format!("opening audio tags: {e}")))?
        .read()
        .map_err(|e| JobError::new(ErrorCode::EngineFailed, format!("reading audio tags: {e}")))?;

    let tag_type = tagged.primary_tag_type();
    if strip {
        tagged.remove(tag_type);
        let tag = Tag::new(tag_type);
        tag.save_to_path(path, WriteOptions::default())
            .map_err(|e| JobError::new(ErrorCode::EngineFailed, format!("stripping tags: {e}")))?;
        return Ok(());
    }

    let mut tag = tagged
        .remove(tag_type)
        .unwrap_or_else(|| Tag::new(tag_type));
    for (key, value) in set {
        match key.as_str() {
            "Title" => tag.set_title(value.clone()),
            "Artist" => tag.set_artist(value.clone()),
            "Album" => tag.set_album(value.clone()),
            "Comment" => tag.set_comment(value.clone()),
            "Genre" => tag.set_genre(value.clone()),
            "Year" => {
                if value.parse::<u32>().is_err() {
                    return Err(JobError::new(
                        ErrorCode::Unsupported,
                        "Year must be a number",
                    ));
                }
                tag.insert_text(lofty::tag::ItemKey::Year, value.clone());
            }
            "TrackNumber" => {
                let n: u32 = value.parse().map_err(|_| {
                    JobError::new(ErrorCode::Unsupported, "TrackNumber must be a number")
                })?;
                tag.set_track(n);
            }
            "DiscNumber" => {
                let n: u32 = value.parse().map_err(|_| {
                    JobError::new(ErrorCode::Unsupported, "DiscNumber must be a number")
                })?;
                tag.set_disk(n);
            }
            "AlbumArtist" => {
                use lofty::tag::ItemKey;
                tag.insert_text(ItemKey::AlbumArtist, value.clone());
            }
            "Composer" => {
                use lofty::tag::ItemKey;
                tag.insert_text(ItemKey::Composer, value.clone());
            }
            other => {
                return Err(JobError::new(
                    ErrorCode::Unsupported,
                    format!("{other} is not writable for audio"),
                ));
            }
        }
    }
    tag.save_to_path(path, WriteOptions::default())
        .map_err(|e| JobError::new(ErrorCode::EngineFailed, format!("writing audio tags: {e}")))?;
    Ok(())
}

fn mp4_tags(path: &Path, set: &BTreeMap<String, String>, strip: bool) -> Result<(), JobError> {
    let mut tag = mp4ameta::Tag::read_from_path(path)
        .map_err(|e| JobError::new(ErrorCode::EngineFailed, format!("reading MP4 tags: {e}")))?;
    if strip {
        tag.take_title();
        tag.take_artist();
        tag.take_album();
        tag.take_comment();
    }
    for (key, value) in set {
        match key.as_str() {
            "Title" => tag.set_title(value.clone()),
            "Artist" => tag.set_artist(value.clone()),
            "Album" => tag.set_album(value.clone()),
            "Comment" => tag.set_comment(value.clone()),
            "Year" => tag.set_year(value.clone()),
            other => {
                return Err(JobError::new(
                    ErrorCode::Unsupported,
                    format!("{other} is not writable for MP4"),
                ));
            }
        }
    }
    tag.write_to_path(path)
        .map_err(|e| JobError::new(ErrorCode::EngineFailed, format!("writing MP4 tags: {e}")))?;
    Ok(())
}

fn pdf_info(path: &Path, set: &BTreeMap<String, String>, strip: bool) -> Result<(), JobError> {
    use lopdf::{Dictionary, Document, Object};

    let mut doc = Document::load(path)
        .map_err(|e| JobError::new(ErrorCode::EngineFailed, format!("reading the PDF: {e}")))?;
    let mut info = Dictionary::new();
    if !strip {
        for (key, value) in set {
            let allowed = [
                "Title", "Author", "Subject", "Keywords", "Creator", "Producer",
            ];
            if !allowed.contains(&key.as_str()) {
                return Err(JobError::new(
                    ErrorCode::Unsupported,
                    format!("{key} is not a PDF information key"),
                ));
            }
            info.set(key.as_str(), Object::string_literal(value.as_str()));
        }
    }
    doc.trailer.set("Info", info);
    doc.save(path)
        .map_err(|e| JobError::new(ErrorCode::EngineFailed, format!("writing the PDF: {e}")))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use convertme_core::job::Intent;
    use convertme_core::job::Job;

    fn ctx<'a>(
        tools: &'a crate::Toolset,
        work: &'a convertme_core::job::WorkDir,
        jc: &'a convertme_core::job::JobCtx,
    ) -> Context<'a> {
        Context {
            tools,
            work,
            ctx: jc,
        }
    }

    #[test]
    fn pdf_info_roundtrips_through_lopdf() {
        let dir = tempfile::tempdir().unwrap();
        let pdf = dir.path().join("doc.pdf");
        // Smallest valid PDF with one empty page.
        let mut doc = lopdf::Document::with_version("1.5");
        let pages_id = doc.new_object_id();
        let mut page = lopdf::Dictionary::new();
        page.set("Type", "Page");
        page.set("Parent", pages_id);
        page.set("MediaBox", vec![0.into(), 0.into(), 200.into(), 200.into()]);
        let page_id = doc.add_object(lopdf::Object::Dictionary(page));

        let mut pages = lopdf::Dictionary::new();
        pages.set("Type", "Pages");
        pages.set("Kids", vec![page_id.into()]);
        pages.set("Count", 1);
        doc.objects
            .insert(pages_id, lopdf::Object::Dictionary(pages));

        let mut catalog = lopdf::Dictionary::new();
        catalog.set("Type", "Catalog");
        catalog.set("Pages", pages_id);
        doc.trailer.set("Root", catalog);
        doc.save(&pdf).unwrap();

        let mut set = BTreeMap::new();
        set.insert("Title".to_string(), "Hello".to_string());
        pdf_info(&pdf, &set, false).unwrap();
        let tags = read(&pdf);
        assert_eq!(tags.get("Title").map(String::as_str), Some("Hello"));
        pdf_info(&pdf, &BTreeMap::new(), true).unwrap();
        assert!(!read(&pdf).contains_key("Title"));
    }

    #[test]
    fn unknown_format_has_no_editable_tags() {
        assert!(editable_tags(FormatId("MKV")).is_empty());
        assert!(!editable_tags(FormatId("JPG")).is_empty());
    }

    #[test]
    fn metadata_job_needs_something_to_do() {
        let dir = tempfile::tempdir().unwrap();
        let png = dir.path().join("a.png");
        std::fs::write(&png, b"x").unwrap();
        let tools = crate::Toolset::probe_now();
        let work = convertme_core::job::WorkDir::create(dir.path().join("w")).unwrap();
        let jc = convertme_core::job::JobCtx::new();
        let cx = ctx(&tools, &work, &jc);
        let job = Job::new(
            png.clone(),
            Intent::Tool {
                tool: ToolId::Metadata,
                params: Default::default(),
            },
        );
        let err = run(&job, &cx).unwrap_err();
        assert_eq!(err.code, ErrorCode::Unsupported);
    }
}
