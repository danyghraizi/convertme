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

/// The tag names each format family's writer accepts.
///
/// This is the browser's list *and* the writer's contract: a name here must be
/// settable and (where `supports_remove` says so) removable. Camera tags such as
/// `ExposureTime` are deliberately absent — the readers show them, but parsing
/// "1/250" into an EXIF rational is not something this writer does, and a row the
/// writer rejects is a control that fails the job.
pub fn editable_tags(format: FormatId) -> &'static [&'static str] {
    match format.0 {
        "JPG" | "HEIC" | "TIFF" => &[
            "Title",
            "Description",
            "Artist",
            "Copyright",
            "Make",
            "Model",
            "LensModel",
            "Orientation",
        ],
        // No EXIF writer for these containers: ImageMagick sets the four tags it can
        // and clears whole profiles, so nothing here is individually removable.
        "PNG" | "WEBP" => &["Title", "Description", "Artist", "Copyright"],
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

/// Whether this format's writer can delete a single tag.
///
/// A writer that can only clear everything (the ImageMagick EXIF path, which
/// strips whole profiles) reports `false`: the metadata browser then draws no minus
/// button for its rows, and `Remove All` stays the supported route. Quietly
/// accepting `remove` and doing nothing would look like a working control.
pub fn supports_remove(format: FormatId) -> bool {
    matches!(
        format.0,
        "JPG"
            | "HEIC"
            | "TIFF"
            | "MP3"
            | "M4A"
            | "FLAC"
            | "OGG"
            | "OPUS"
            | "WAV"
            | "AIFF"
            | "WMA"
            | "MP4"
            | "MOV"
            | "M4V"
            | "PDF"
    )
}

/// `params.set` (object of tag → value), `params.remove` (array of tags) and
/// `params.strip` (bool).
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

    let remove: Vec<String> = params
        .get("remove")
        .and_then(|v| v.as_array())
        .map(|items| {
            items
                .iter()
                .filter_map(|v| v.as_str().map(str::to_string))
                .collect()
        })
        .unwrap_or_default();

    if !strip && set.is_empty() && remove.is_empty() {
        return Err(JobError::new(
            ErrorCode::Unsupported,
            "nothing to change: give `set` tags, `remove` tags or `strip: true`",
        ));
    }

    let format = job
        .input_format
        .or_else(|| Registry::detect(&job.source).map(|f| f.id))
        .ok_or_else(|| JobError::new(ErrorCode::Unsupported, "unknown input format"))?;

    // Every key must be one the writer knows; otherwise a typo silently edits nothing.
    let writable = editable_tags(format);
    for key in set.keys().chain(remove.iter()) {
        if !writable.iter().any(|t| t.eq_ignore_ascii_case(key)) {
            return Err(JobError::new(
                ErrorCode::Unsupported,
                format!("{key} is not a writable tag for {}", format.0),
            ));
        }
    }
    if !remove.is_empty() && !supports_remove(format) {
        let key = &remove[0];
        return Err(JobError::new(
            ErrorCode::Unsupported,
            format!(
                "{key} cannot be removed from {}; strip every tag instead",
                format.0
            ),
        ));
    }

    let mut written = Vec::new();
    for (index, source) in job.inputs.iter().enumerate() {
        cx.ctx.checkpoint()?;
        let staged = cx.copy_of(index, job)?;
        apply(&staged, format, &set, &remove, strip, cx)?;
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

/// The friendly key for an EXIF tag, matching `editable_tags`' names.
fn friendly_exif(name: &str) -> Option<&'static str> {
    match name {
        "ImageDescription" => Some("Title"),
        "Artist" => Some("Artist"),
        "Copyright" => Some("Copyright"),
        "Make" => Some("Make"),
        "Model" => Some("Model"),
        "LensModel" => Some("LensModel"),
        "Orientation" => Some("Orientation"),
        _ => None,
    }
}

/// Read a camera's EXIF block and translate it to the form's tag names.
fn read_exif_tags(path: &Path, tags: &mut BTreeMap<String, String>) {
    let Ok(file) = std::fs::File::open(path) else {
        return;
    };
    let mut buf = std::io::BufReader::new(file);
    if let Ok(exif) = exif::Reader::new().read_from_container(&mut buf) {
        insert_exif(&exif, tags);
    }
}

/// The form's rows are the friendly names `editable_tags` advertises; a raw
/// `ImageDescription` row cannot be edited and leaves the "Title" row looking empty
/// on a file that has one. `Title` and `Description` are backed by the same EXIF
/// field (`ImageDescription`), so both rows show the stored value.
fn insert_exif(exif: &exif::Exif, tags: &mut BTreeMap<String, String>) {
    for field in exif.fields() {
        let name = field.tag.to_string();
        // Orientation's display value is prose ("row 0 at top…"); the form's input is
        // editable and the writer parses 1..8, so show the number it accepts.
        // `display_value()` quotes an ASCII field ("A title"); the form wants the
        // text itself, not the debug rendering.
        let value = if name == "Orientation" {
            field.value.get_uint(0).map(|n| n.to_string())
        } else if let exif::Value::Ascii(values) = &field.value {
            values
                .first()
                .map(|bytes| {
                    String::from_utf8_lossy(bytes)
                        .trim_end_matches('\0')
                        .to_string()
                })
                .filter(|text| !text.is_empty())
        } else {
            Some(field.display_value().with_unit(exif).to_string())
        };
        let Some(value) = value else {
            continue;
        };
        if name == "ImageDescription" {
            tags.entry("Title".to_string())
                .or_insert_with(|| value.clone());
            tags.entry("Description".to_string()).or_insert(value);
        } else if let Some(key) = friendly_exif(&name) {
            tags.entry(key.to_string()).or_insert(value);
        }
    }
}

/// WEBP metadata lives in RIFF chunks. ImageMagick writes the writable tags into
/// XMP (`dc:title`, `dc:description`, `dc:creator`, `dc:rights`); an EXIF block,
/// when present, is scanned separately so a chunk past the first few kilobytes is
/// still read.
fn webp_tags(path: &Path, tags: &mut BTreeMap<String, String>) {
    let Ok(bytes) = std::fs::read(path) else {
        return;
    };
    if !bytes.starts_with(b"RIFF") || bytes.get(8..12) != Some(&b"WEBP"[..]) {
        return;
    }
    let mut at = 12;
    while at + 8 <= bytes.len() {
        let kind = &bytes[at..at + 4];
        let len = u32::from_le_bytes([bytes[at + 4], bytes[at + 5], bytes[at + 6], bytes[at + 7]])
            as usize;
        let start = at + 8;
        let end = start.saturating_add(len).min(bytes.len());
        let data = &bytes[start..end];
        match kind {
            b"EXIF" => {
                let raw = data.strip_prefix(b"Exif\0\0").unwrap_or(data);
                if let Ok(exif) = exif::Reader::new().read_raw(raw.to_vec()) {
                    insert_exif(&exif, tags);
                }
            }
            b"XMP " => xmp_tags(data, tags),
            _ => {}
        }
        // Chunks are padded to an even length.
        at = end.saturating_add(len & 1);
    }
}

fn xmp_tags(data: &[u8], tags: &mut BTreeMap<String, String>) {
    let Ok(text) = std::str::from_utf8(data) else {
        return;
    };
    for (element, key) in [
        ("dc:title", "Title"),
        ("dc:description", "Description"),
        ("dc:creator", "Artist"),
        ("dc:rights", "Copyright"),
    ] {
        if let Some(value) = xmp_element(text, element) {
            tags.entry(key.to_string()).or_insert(value);
        }
    }
}

/// The text of one XMP element. XMP wraps repeated/translated values in
/// `<rdf:Alt><rdf:li>…</rdf:li></rdf:Alt>`, so take the first `rdf:li`.
fn xmp_element(text: &str, name: &str) -> Option<String> {
    let open = format!("<{name}");
    let close = format!("</{name}>");
    let open = text.find(open.as_str())?;
    let rest = &text[open..];
    let body_start = rest.find('>')? + 1;
    let body_end = rest.find(close.as_str())?;
    let body = &rest[body_start..body_end];
    let value = match body.find("<rdf:li") {
        Some(li) => {
            let after = &body[li..];
            let start = after.find('>')? + 1;
            let end = after.find("</rdf:li>")?;
            &after[start..end]
        }
        None => body,
    };
    let value = value.trim();
    (!value.is_empty()).then(|| {
        value
            .replace("&lt;", "<")
            .replace("&gt;", ">")
            .replace("&quot;", "\"")
            .replace("&apos;", "'")
            .replace("&amp;", "&")
    })
}

/// PNG text chunks (`tEXt`/`iTXt`): ImageMagick writes Title as `Comment`, and the
/// other writable tags under their own keyword (`Description`, `Artist`, `Copyright`).
fn png_text_tags(path: &Path, tags: &mut BTreeMap<String, String>) {
    const SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";
    let Ok(bytes) = std::fs::read(path) else {
        return;
    };
    if !bytes.starts_with(SIGNATURE) {
        return;
    }
    let mut at = SIGNATURE.len();
    while at + 8 <= bytes.len() {
        let len =
            u32::from_be_bytes([bytes[at], bytes[at + 1], bytes[at + 2], bytes[at + 3]]) as usize;
        let kind = &bytes[at + 4..at + 8];
        let start = at + 8;
        let end = start.saturating_add(len).min(bytes.len());
        let data = &bytes[start..end];
        match kind {
            b"tEXt" => {
                if let Some(nul) = data.iter().position(|b| *b == 0) {
                    let key = String::from_utf8_lossy(&data[..nul]).into_owned();
                    let value = String::from_utf8_lossy(&data[nul + 1..]).into_owned();
                    insert_text(tags, &key, value);
                }
            }
            b"iTXt" => {
                // keyword\0 compression flag\0 compression method\0 language\0
                // translated keyword\0 text (UTF-8).
                let mut parts = data.split(|b| *b == 0);
                let key = parts.next().unwrap_or_default();
                let _flag = parts.next();
                let _method = parts.next();
                let _language = parts.next();
                let _translated = parts.next();
                if let Some(text) = parts.next() {
                    insert_text(
                        tags,
                        &String::from_utf8_lossy(key),
                        String::from_utf8_lossy(text).into_owned(),
                    );
                }
            }
            b"IEND" => break,
            _ => {}
        }
        at = end.saturating_add(4);
    }
}

fn insert_text(tags: &mut BTreeMap<String, String>, key: &str, value: String) {
    let value = value.trim();
    if value.is_empty() {
        return;
    }
    // ImageMagick writes its bookkeeping into the same chunks (`date:create`,
    // `date:modify`, `Software`); those are not tags a user edits, and listing them
    // would bury the four that matter.
    let lower = key.to_ascii_lowercase();
    if lower.contains(':') || lower.starts_with("date") || lower == "software" {
        return;
    }
    // The keyword's case is the writer's business: ImageMagick writes `comment`.
    // A real `Description` keyword is its own writable tag; `comment` is what this
    // writer uses for Title.
    let friendly = match lower.as_str() {
        "comment" | "imagedescription" | "title" => "Title",
        "description" => "Description",
        "artist" => "Artist",
        "copyright" => "Copyright",
        other => other,
    };
    // This writer's Title is a `comment` chunk. A file edited by it can still carry
    // an older `Title` keyword from another tool, and that chunk may come first, so
    // `comment` replaces rather than yields.
    if lower == "comment" {
        tags.insert(friendly.to_string(), value.to_string());
    } else {
        tags.entry(friendly.to_string())
            .or_insert_with(|| value.to_string());
    }
}

/// Read the tags the UI shows in the edit form.
pub fn read(path: &Path) -> BTreeMap<String, String> {
    let Some(format) = Registry::detect(path).map(|f| f.id) else {
        return BTreeMap::new();
    };
    let mut tags = BTreeMap::new();
    match format.0 {
        "JPG" | "HEIC" | "TIFF" => read_exif_tags(path, &mut tags),
        // ImageMagick writes the four writable image tags as PNG text chunks (Title
        // as `comment`), so the reader has to look there.
        "PNG" => png_text_tags(path, &mut tags),
        "WEBP" => {
            // A WEBP carries EXIF and XMP inside RIFF chunks. The container reader
            // only looks at the first few kilobytes, and ImageMagick stores these
            // tags in XMP, so scan the chunks themselves.
            webp_tags(path, &mut tags);
            read_exif_tags(path, &mut tags);
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
                        (
                            "AlbumArtist",
                            tag.get_string(lofty::tag::ItemKey::AlbumArtist)
                                .map(str::to_string),
                        ),
                        ("TrackNumber", tag.track().map(|n| n.to_string())),
                        ("DiscNumber", tag.disk().map(|n| n.to_string())),
                        ("Comment", tag.comment().map(|c| c.to_string())),
                        (
                            "Year",
                            tag.get_string(lofty::tag::ItemKey::Year)
                                .map(str::to_string),
                        ),
                        ("Genre", tag.genre().map(|c| c.to_string())),
                        (
                            "Composer",
                            tag.get_string(lofty::tag::ItemKey::Composer)
                                .map(str::to_string),
                        ),
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
                    ("Year", mp4.year().map(str::to_string)),
                    ("Comment", mp4.comment().map(str::to_string)),
                    ("Genre", mp4.genre().map(str::to_string)),
                ] {
                    if let Some(v) = value {
                        tags.insert(name.to_string(), v);
                    }
                }
            }
        }
        "PDF" => {
            if let Ok(doc) = lopdf::Document::load(path)
                && let Some(info) = pdf_info_dict(&doc)
            {
                for (k, v) in info.iter() {
                    if let Some(value) = pdf_string(&doc, v) {
                        tags.insert(String::from_utf8_lossy(k).into_owned(), value);
                    }
                }
            }
        }
        _ => {}
    }
    tags
}

/// The document information dictionary. Producers store `/Info` as an indirect
/// reference, and `Object::as_dict` does not follow references, so a plain
/// `trailer.get("Info")` returned nothing on every real-world PDF.
fn pdf_info_dict(doc: &lopdf::Document) -> Option<&lopdf::Dictionary> {
    let info = doc.trailer.get(b"Info").ok()?;
    let (_, resolved) = doc.dereference(info).ok()?;
    resolved.as_dict().ok()
}

/// One information value as text. PDF strings are PDFDocEncoding or UTF-16BE with a
/// BOM; decoding them as UTF-8 turned every non-ASCII title into mojibake.
fn pdf_string(doc: &lopdf::Document, object: &lopdf::Object) -> Option<String> {
    let (_, resolved) = doc.dereference(object).ok()?;
    lopdf::decode_text_string(resolved).ok()
}

fn apply(
    path: &Path,
    format: FormatId,
    set: &BTreeMap<String, String>,
    remove: &[String],
    strip: bool,
    cx: &Context<'_>,
) -> Result<(), JobError> {
    match format.0 {
        "JPG" | "HEIC" | "TIFF" => exif_direct(path, set, remove, strip),
        // No EXIF writer for these containers: ImageMagick sets what it can and
        // clears profiles wholesale, which is also why they cannot remove one tag.
        "PNG" | "WEBP" => exif_via_magick(path, set, strip, cx),
        "MP3" | "M4A" | "FLAC" | "OGG" | "OPUS" | "WAV" | "AIFF" | "WMA" => {
            audio_tags(path, set, remove, strip)
        }
        "MP4" | "MOV" | "M4V" => mp4_tags(path, set, remove, strip),
        "PDF" => pdf_info(path, set, remove, strip),
        _ => Err(JobError::new(
            ErrorCode::Unsupported,
            format!("{} has no writable metadata", format.0),
        )),
    }
}

fn exif_direct(
    path: &Path,
    set: &BTreeMap<String, String>,
    remove: &[String],
    strip: bool,
) -> Result<(), JobError> {
    use little_exif::exif_tag::ExifTag;
    use little_exif::metadata::Metadata;

    let mut metadata = if strip {
        Metadata::new()
    } else {
        // A JPEG straight out of a camera-less pipeline has no EXIF block at all;
        // `new_from_path` treats that as an error, but "no tags yet" is exactly the
        // file this writer must be able to add a title to.
        Metadata::new_from_path(path).unwrap_or_else(|_| Metadata::new())
    };

    if strip {
        // Empty metadata replaces the block wholesale once written.
        metadata
            .write_to_file(path)
            .map_err(|e| JobError::new(ErrorCode::EngineFailed, format!("stripping EXIF: {e}")))?;
        return Ok(());
    }

    // Deletions first: setting and then deleting the same tag must end deleted.
    for key in remove {
        if let Some(tag) = exif_tag_for(key, "") {
            metadata.remove_tag(tag);
        }
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

/// The little_exif tag a UI tag name maps to, for deletion.
fn exif_tag_for(key: &str, value: &str) -> Option<little_exif::exif_tag::ExifTag> {
    use little_exif::exif_tag::ExifTag;
    Some(match key {
        "Title" | "Description" => ExifTag::ImageDescription(value.to_string()),
        "Artist" => ExifTag::Artist(value.to_string()),
        "Copyright" => ExifTag::Copyright(value.to_string()),
        "Make" => ExifTag::Make(value.to_string()),
        "Model" => ExifTag::Model(value.to_string()),
        "LensModel" => ExifTag::LensModel(value.to_string()),
        "Orientation" => ExifTag::Orientation(vec![value.parse().unwrap_or(1)]),
        _ => return None,
    })
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
            // ImageMagick's PNG writer stores `comment` as a tEXt chunk; that is the
            // chunk the reader calls Title. Description gets its own keyword, so a
            // value set on that row comes back on that row.
            "Title" => {
                cmd = cmd.arg("-set").arg("comment").arg(value);
            }
            "Description" => {
                cmd = cmd.arg("-set").arg("Description").arg(value);
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

fn audio_tags(
    path: &Path,
    set: &BTreeMap<String, String>,
    remove: &[String],
    strip: bool,
) -> Result<(), JobError> {
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
    for key in remove {
        use lofty::tag::ItemKey;
        let item = match key.as_str() {
            "Title" => ItemKey::TrackTitle,
            "Artist" => ItemKey::TrackArtist,
            "Album" => ItemKey::AlbumTitle,
            "AlbumArtist" => ItemKey::AlbumArtist,
            "TrackNumber" => ItemKey::TrackNumber,
            "DiscNumber" => ItemKey::DiscNumber,
            "Year" => ItemKey::Year,
            "Genre" => ItemKey::Genre,
            "Comment" => ItemKey::Comment,
            "Composer" => ItemKey::Composer,
            other => {
                return Err(JobError::new(
                    ErrorCode::Unsupported,
                    format!("{other} is not removable for audio"),
                ));
            }
        };
        tag.remove_key(item);
    }
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

fn mp4_tags(
    path: &Path,
    set: &BTreeMap<String, String>,
    remove: &[String],
    strip: bool,
) -> Result<(), JobError> {
    let mut tag = mp4ameta::Tag::read_from_path(path)
        .map_err(|e| JobError::new(ErrorCode::EngineFailed, format!("reading MP4 tags: {e}")))?;
    // Every tag `editable_tags` offers for MP4 is handled here; a name the writer
    // cannot set would show in the browser and then fail the job, so the two lists
    // are kept in step by this match.
    // `take_*` hand back the value they removed; the value is not what is reported.
    let take = |tag: &mut mp4ameta::Tag, key: &str| {
        let _ = match key {
            "Title" => tag.take_title(),
            "Artist" => tag.take_artist(),
            "Album" => tag.take_album(),
            "Comment" => tag.take_comment(),
            "Year" => tag.take_year(),
            "Genre" => tag.take_genre(),
            _ => None,
        };
    };
    if strip {
        for key in ["Title", "Artist", "Album", "Comment", "Year", "Genre"] {
            take(&mut tag, key);
        }
    }
    for key in remove {
        take(&mut tag, key);
    }
    for (key, value) in set {
        match key.as_str() {
            "Title" => tag.set_title(value.clone()),
            "Artist" => tag.set_artist(value.clone()),
            "Album" => tag.set_album(value.clone()),
            "Comment" => tag.set_comment(value.clone()),
            "Year" => tag.set_year(value.clone()),
            "Genre" => tag.set_genre(value.clone()),
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

fn pdf_info(
    path: &Path,
    set: &BTreeMap<String, String>,
    remove: &[String],
    strip: bool,
) -> Result<(), JobError> {
    use lopdf::{Dictionary, Document};

    let mut doc = Document::load(path)
        .map_err(|e| JobError::new(ErrorCode::EngineFailed, format!("reading the PDF: {e}")))?;
    let mut info = Dictionary::new();
    if strip {
        // `strip` means every information value goes, including keys this build does
        // not model: the dictionary is replaced wholesale.
    } else {
        // Keep what the file already says, then edit that dictionary: replacing it
        // outright dropped Producer and CreationDate on every single edit. `/Info` is
        // normally an indirect reference, which `as_dict` does not follow.
        if let Some(existing) = pdf_info_dict(&doc) {
            info = existing.clone();
        }
        for key in remove {
            info.remove(key.as_bytes());
        }
    }
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
            info.set(key.as_str(), lopdf::text_string(value));
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
            preview: None,
        }
    }

    /// The smallest valid one-page PDF, for the information-dictionary tests.
    fn one_page_doc() -> lopdf::Document {
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
        doc
    }

    #[test]
    fn pdf_info_roundtrips_through_lopdf() {
        let dir = tempfile::tempdir().unwrap();
        let pdf = dir.path().join("doc.pdf");
        one_page_doc().save(&pdf).unwrap();

        let mut set = BTreeMap::new();
        set.insert("Title".to_string(), "Hello".to_string());
        set.insert("Author".to_string(), "Nobody".to_string());
        pdf_info(&pdf, &set, &[], false).unwrap();
        let tags = read(&pdf);
        assert_eq!(tags.get("Title").map(String::as_str), Some("Hello"));

        // Removing one key leaves the other alone: replacing the whole dictionary
        // would have dropped it.
        pdf_info(&pdf, &BTreeMap::new(), &["Author".to_string()], false).unwrap();
        let tags = read(&pdf);
        assert!(!tags.contains_key("Author"));
        assert_eq!(tags.get("Title").map(String::as_str), Some("Hello"));

        pdf_info(&pdf, &BTreeMap::new(), &[], true).unwrap();
        assert!(!read(&pdf).contains_key("Title"));
    }

    #[test]
    fn pdf_info_behind_an_indirect_reference_is_read_and_kept() {
        // Ghostscript and most producers store `/Info` as an indirect object. Reading
        // the trailer entry as a dictionary returned nothing, so every PDF opened with
        // empty rows even though it carried all six keys.
        let dir = tempfile::tempdir().unwrap();
        let pdf = dir.path().join("indirect.pdf");
        let mut doc = one_page_doc();
        let mut info = lopdf::Dictionary::new();
        info.set("Title", lopdf::text_string("Tïtle"));
        info.set("Author", lopdf::text_string("Ada"));
        info.set("Subject", lopdf::text_string("Subj"));
        info.set("Keywords", lopdf::text_string("k1, k2"));
        info.set("Creator", lopdf::text_string("Creator"));
        info.set("Producer", lopdf::text_string("Producer"));
        let info_id = doc.add_object(lopdf::Object::Dictionary(info));
        doc.trailer.set("Info", lopdf::Object::Reference(info_id));
        doc.save(&pdf).unwrap();

        // The file must keep `/Info` indirect, or the test would not exercise the bug.
        let saved = lopdf::Document::load(&pdf).unwrap();
        assert!(matches!(
            saved.trailer.get(b"Info").unwrap(),
            lopdf::Object::Reference(_)
        ));

        let tags = read(&pdf);
        assert_eq!(tags.get("Title").map(String::as_str), Some("Tïtle"));
        assert_eq!(tags.get("Author").map(String::as_str), Some("Ada"));
        assert_eq!(tags.get("Subject").map(String::as_str), Some("Subj"));
        assert_eq!(tags.get("Keywords").map(String::as_str), Some("k1, k2"));
        assert_eq!(tags.get("Creator").map(String::as_str), Some("Creator"));
        assert_eq!(tags.get("Producer").map(String::as_str), Some("Producer"));

        // An edit keeps the keys it was not asked to change.
        let mut set = BTreeMap::new();
        set.insert("Subject".to_string(), "New subject".to_string());
        pdf_info(&pdf, &set, &[], false).unwrap();
        let tags = read(&pdf);
        assert_eq!(tags.get("Title").map(String::as_str), Some("Tïtle"));
        assert_eq!(tags.get("Subject").map(String::as_str), Some("New subject"));
    }

    #[test]
    fn png_text_chunks_are_read_as_the_forms_own_keys() {
        // A PNG whose only tag is the `Comment` chunk ImageMagick writes for Title:
        // the form must show it under "Title", not as a raw "Comment" row.
        let dir = tempfile::tempdir().unwrap();
        let png = dir.path().join("tagged.png");
        let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
        let chunk = |kind: &[u8], data: &[u8]| {
            let mut out = Vec::new();
            out.extend_from_slice(&(data.len() as u32).to_be_bytes());
            out.extend_from_slice(kind);
            out.extend_from_slice(data);
            out.extend_from_slice(&[0, 0, 0, 0]);
            out
        };
        // ImageMagick writes the keyword in lower case and adds bookkeeping chunks.
        bytes.extend(chunk(b"tEXt", b"comment\0Hello"));
        bytes.extend(chunk(b"tEXt", b"Description\0A description"));
        bytes.extend(chunk(b"tEXt", b"Artist\0Ada"));
        bytes.extend(chunk(b"tEXt", "Copyright\0© 2026 Ada".as_bytes()));
        bytes.extend(chunk(b"tEXt", b"date:create\x002026-10-31T16:20:16+00:00"));
        bytes.extend(chunk(b"IEND", b""));
        std::fs::write(&png, &bytes).unwrap();

        let tags = read(&png);
        assert_eq!(tags.get("Title").map(String::as_str), Some("Hello"));
        assert_eq!(
            tags.get("Description").map(String::as_str),
            Some("A description")
        );
        assert_eq!(tags.get("Artist").map(String::as_str), Some("Ada"));
        assert_eq!(
            tags.get("Copyright").map(String::as_str),
            Some("© 2026 Ada")
        );
        assert!(!tags.contains_key("Comment"));
        assert!(!tags.keys().any(|key| key.contains(':')));

        // A literal `Title` keyword (ImageMagick writes one for `-set Title`) also
        // lands on the Title row.
        let mut tags = BTreeMap::new();
        let titled = dir.path().join("titled.png");
        let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
        bytes.extend(chunk(b"tEXt", b"Title\0Direct title"));
        bytes.extend(chunk(b"IEND", b""));
        std::fs::write(&titled, &bytes).unwrap();
        png_text_tags(&titled, &mut tags);
        assert_eq!(tags.get("Title").map(String::as_str), Some("Direct title"));

        // This writer sets Title as `comment`. An older `Title` keyword from another
        // tool can sit before that chunk in the file; the comment is the value the
        // app wrote, so it must win.
        let mut tags = BTreeMap::new();
        let rewritten = dir.path().join("rewritten.png");
        let mut bytes = b"\x89PNG\r\n\x1a\n".to_vec();
        bytes.extend(chunk(b"tEXt", b"Title\0Old title"));
        bytes.extend(chunk(b"tEXt", b"comment\0New title"));
        bytes.extend(chunk(b"IEND", b""));
        std::fs::write(&rewritten, &bytes).unwrap();
        png_text_tags(&rewritten, &mut tags);
        assert_eq!(tags.get("Title").map(String::as_str), Some("New title"));
    }

    #[test]
    fn exif_reader_prefills_every_writable_image_tag() {
        // A real JPEG with the EXIF block the writer produces. The form must show the
        // stored value under both names ImageDescription backs, and Orientation as the
        // number the writer can parse again.
        let dir = tempfile::tempdir().unwrap();
        let jpg = dir.path().join("tagged.jpg");
        image::RgbImage::new(8, 8).save(&jpg).unwrap();

        let mut set = BTreeMap::new();
        set.insert("Title".to_string(), "A title".to_string());
        set.insert("Artist".to_string(), "Ada".to_string());
        set.insert("Copyright".to_string(), "Copyright 2026 Ada".to_string());
        set.insert("Make".to_string(), "Acme".to_string());
        set.insert("Model".to_string(), "X1".to_string());
        set.insert("LensModel".to_string(), "Lens 50mm".to_string());
        set.insert("Orientation".to_string(), "6".to_string());
        exif_direct(&jpg, &set, &[], false).unwrap();

        let tags = read(&jpg);
        assert_eq!(tags.get("Title").map(String::as_str), Some("A title"));
        assert_eq!(tags.get("Description").map(String::as_str), Some("A title"));
        assert_eq!(tags.get("Artist").map(String::as_str), Some("Ada"));
        assert_eq!(
            tags.get("Copyright").map(String::as_str),
            Some("Copyright 2026 Ada")
        );
        assert_eq!(tags.get("Make").map(String::as_str), Some("Acme"));
        assert_eq!(tags.get("Model").map(String::as_str), Some("X1"));
        assert_eq!(tags.get("LensModel").map(String::as_str), Some("Lens 50mm"));
        assert_eq!(tags.get("Orientation").map(String::as_str), Some("6"));
    }

    #[test]
    fn a_tag_the_writer_cannot_delete_is_refused_not_ignored() {
        // The metadata browser hides the minus for these, so a `remove` that still
        // arrives is a contract violation and must say so.
        assert!(supports_remove(FormatId("PDF")));
        assert!(supports_remove(FormatId("JPG")));
        assert!(supports_remove(FormatId("MP3")));
        assert!(!supports_remove(FormatId("PNG")));
        assert!(!supports_remove(FormatId("WEBP")));
        assert!(!supports_remove(FormatId("MKV")));
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
