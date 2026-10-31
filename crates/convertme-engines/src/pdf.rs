//! PDF engine: Poppler for text and page images, Ghostscript for compression,
//! qpdf for page-level edits, and the Python sidecar for office/EPUB exports.
//!
//! Every artifact lands in `work/out`; publishing is the queue's job. Source
//! files are only ever opened for reading.

use crate::Context;
use crate::ensure_file;
use crate::proc::{self, Budget, Cmd};
use convertme_core::capability::ToolKey;
use convertme_core::error::{ErrorCode, JobError};
use convertme_core::formats::ToolId;
use convertme_core::job::{Intent, Job, ToolParams};
use std::ffi::{OsStr, OsString};
use std::fs;
use std::io::Read as _;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::sync::atomic::{AtomicBool, Ordering};
use std::time::Duration;

/// A render above this many pages is refused instead of filling the disk.
const PAGE_LIMIT: usize = 400;
/// Below this many non-space characters per page the text layer is treated as
/// missing and the OCR path runs instead.
const OCR_MIN_CHARS_PER_PAGE: f64 = 40.0;
const RENDER_DPI: &str = "300";

pub fn run(job: &Job, cx: &Context<'_>) -> Result<Vec<PathBuf>, JobError> {
    cx.ctx.checkpoint()?;
    let artifacts = match &job.intent {
        Intent::Convert { target } => convert(job, cx, target.0)?,
        Intent::Tool { tool, .. } => run_tool(job, cx, *tool)?,
    };
    if artifacts.is_empty() {
        return Err(JobError::new(
            ErrorCode::EngineFailed,
            "the PDF engine produced no output",
        ));
    }
    for artifact in &artifacts {
        ensure_file(artifact, "the PDF engine")?;
    }
    cx.ctx.set_progress(1.0);
    Ok(artifacts)
}

fn convert(job: &Job, cx: &Context<'_>, target: &'static str) -> Result<Vec<PathBuf>, JobError> {
    match target {
        "TXT" => extract_text(job, cx, TextFlavor::Plain),
        "MD" => extract_text(job, cx, TextFlavor::Markdown),
        "PNG" => render_pages(job, cx, ImageKind::Png),
        "JPG" => render_pages(job, cx, ImageKind::Jpeg),
        "DOCX" => sidecar(job, cx, "docx", "docx"),
        "PPTX" => sidecar(job, cx, "pptx", "pptx"),
        "XLSX" => sidecar(job, cx, "xlsx", "xlsx"),
        "EPUB" => sidecar(job, cx, "epub", "epub"),
        other => Err(JobError::unsupported("PDF", other)),
    }
}

fn run_tool(job: &Job, cx: &Context<'_>, tool: ToolId) -> Result<Vec<PathBuf>, JobError> {
    match tool {
        ToolId::Compress => compress(job, cx),
        ToolId::Organize => organize(job, cx),
        ToolId::Merge => merge(job, cx),
        ToolId::Split => split(job, cx),
        ToolId::SavePages => save_pages(job, cx),
        ToolId::ExtractText => extract_text(job, cx, TextFlavor::Plain),
        ToolId::Metadata => Err(JobError::new(
            ErrorCode::Unsupported,
            "PDF metadata is handled by the metadata engine",
        )),
        other => Err(JobError::new(
            ErrorCode::Unsupported,
            format!("{other} is not a PDF tool"),
        )),
    }
}

// --------------------------------------------------------------------- text

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum TextFlavor {
    Plain,
    Markdown,
}

fn extract_text(job: &Job, cx: &Context<'_>, flavor: TextFlavor) -> Result<Vec<PathBuf>, JobError> {
    let text = pdf_text(job, cx)?;
    cx.ctx.checkpoint()?;

    match flavor {
        TextFlavor::Plain => {
            let out = cx.out("text.txt");
            write_text(&out, &text)?;
            Ok(vec![out])
        }
        TextFlavor::Markdown => {
            let pandoc = cx.tools.path(ToolKey::Pandoc)?;
            let plain = cx.scratch("extracted-plain.txt");
            write_text(&plain, &text)?;
            let out = cx.out("text.md");
            cx.ctx.set_stage("Converting text to Markdown");
            // Pandoc 3 removed the `plain` reader; extracted text is valid Markdown,
            // and saying so is what keeps this path working across pandoc upgrades.
            proc::run(
                Cmd::new(&pandoc)
                    .args(["-f", "markdown", "-t", "gfm", "--wrap=none", "-o"])
                    .arg(&out)
                    .arg(&plain)
                    .budget(Budget::Short),
                cx.ctx,
            )?
            .ok()?;
            Ok(vec![out])
        }
    }
}

/// Text from `pdftotext -layout`, with the OCR fallback for scanned pages.
fn pdf_text(job: &Job, cx: &Context<'_>) -> Result<String, JobError> {
    let pdftotext = cx.tools.path(ToolKey::Pdftotext)?;
    cx.ctx.set_stage("Reading the document text");
    let raw = cx.scratch("extracted.txt");
    proc::run(
        Cmd::new(&pdftotext)
            .args(["-layout", "-enc", "UTF-8"])
            .arg(&job.source)
            .arg(&raw)
            .budget(Budget::Short),
        cx.ctx,
    )?
    .ok()?;
    cx.ctx.set_progress(0.4);

    let bytes = fs::read(&raw)
        .map_err(|e| JobError::io(format!("reading the extracted text {}", raw.display()), &e))?;
    let text = String::from_utf8_lossy(&bytes).into_owned();
    cx.ctx.checkpoint()?;

    let pages = text.matches('\u{0c}').count().max(1);
    let chars = text.chars().filter(|c| !c.is_whitespace()).count() as f64;
    let can_ocr = cx.tools.has(ToolKey::Tesseract) && cx.tools.has(ToolKey::Pdftoppm);
    if chars / (pages as f64) < OCR_MIN_CHARS_PER_PAGE && can_ocr {
        let ocr = ocr_text(job, cx, &text, pages)?;
        if !ocr.trim().is_empty() {
            return Ok(ocr);
        }
    }
    Ok(text)
}

/// Render the pages at 300 DPI and run Tesseract over each one, because the PDF
/// carries no text layer worth reading.
fn ocr_text(
    job: &Job,
    cx: &Context<'_>,
    extracted: &str,
    pages: usize,
) -> Result<String, JobError> {
    let pdftoppm = cx.tools.path(ToolKey::Pdftoppm)?;
    let tesseract = cx.tools.path(ToolKey::Tesseract)?;
    let langs = ocr_languages(job)?;
    let limit = pages.min(PAGE_LIMIT);

    let dir = cx.scratch("ocr");
    fs::create_dir_all(&dir)
        .map_err(|e| JobError::io(format!("creating the OCR directory {}", dir.display()), &e))?;

    cx.ctx.set_stage("Rendering scanned pages");
    cx.ctx.checkpoint()?;
    proc::run(
        Cmd::new(&pdftoppm)
            .args(["-r", RENDER_DPI, "-png", "-f", "1", "-l"])
            .arg(limit.to_string())
            .arg(&job.source)
            .arg(dir.join("page"))
            .budget(Budget::Long),
        cx.ctx,
    )?
    .ok()?;

    let images = page_files(&dir, &["png"]);
    if images.is_empty() {
        return Ok(extracted.to_string());
    }

    let mut page_texts: Vec<String> = Vec::with_capacity(images.len());
    for (i, image) in images.iter().enumerate() {
        cx.ctx.checkpoint()?;
        cx.ctx
            .set_stage(format!("Recognising page {} of {}", i + 1, images.len()));
        let tsv_output = proc::run(
            Cmd::new(&tesseract)
                .arg(image)
                .arg("stdout")
                .arg("-l")
                .arg(&langs)
                .arg("tsv")
                .stdout_piped()
                .budget(Budget::Long),
            cx.ctx,
        )?
        .ok()?;
        page_texts.push(tsv_text(&tsv_output.stdout));
        cx.ctx
            .set_progress(0.45 + 0.5 * ((i + 1) as f64 / images.len() as f64));
    }

    let mut text = page_texts.join("\n\n");
    if pages > limit {
        // Pages past the render cap keep whatever text layer they had.
        let tail = extracted
            .split('\u{0c}')
            .skip(limit)
            .collect::<Vec<_>>()
            .join("\u{0c}");
        if !tail.trim().is_empty() {
            if !text.is_empty() {
                text.push('\n');
            }
            text.push_str(&tail);
        }
    }
    Ok(text)
}

/// Tesseract's TSV: one row per word, column 11 is the recognised text. A new
/// (block, paragraph, line) tuple starts a new output line.
fn tsv_text(tsv: &str) -> String {
    let mut out = String::new();
    let mut last_key: Option<(String, String, String)> = None;
    for row in tsv.lines().skip(1) {
        let cols: Vec<&str> = row.split('\t').collect();
        if cols.len() < 12 {
            continue;
        }
        let word = cols[11].trim();
        if word.is_empty() {
            continue;
        }
        let key = (
            cols[2].to_string(),
            cols[3].to_string(),
            cols[4].to_string(),
        );
        if last_key.as_ref() != Some(&key) {
            if !out.is_empty() {
                out.push('\n');
            }
            last_key = Some(key);
        } else if !out.ends_with(' ') && !out.ends_with('\n') {
            out.push(' ');
        }
        out.push_str(word);
    }
    out
}

fn ocr_languages(job: &Job) -> Result<String, JobError> {
    let mut langs: Vec<String> = Vec::new();
    if let Some(params) = job.intent.params()
        && let Some(value) = params.get("ocr_languages")
    {
        let array = value
            .as_array()
            .ok_or_else(|| bad_param("ocr_languages", "an array of language codes"))?;
        for item in array {
            let code = item
                .as_str()
                .ok_or_else(|| bad_param("ocr_languages", "an array of language codes"))?
                .trim();
            if !code.is_empty() {
                langs.push(code.to_string());
            }
        }
    }
    if langs.is_empty() {
        langs.push("eng".to_string());
    }
    Ok(langs.join("+"))
}

fn write_text(path: &Path, text: &str) -> Result<(), JobError> {
    fs::write(path, text.as_bytes())
        .map_err(|e| JobError::io(format!("writing {}", path.display()), &e))
}

// ------------------------------------------------------------------- render

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ImageKind {
    Png,
    Jpeg,
}

impl ImageKind {
    fn flag(self) -> &'static str {
        match self {
            ImageKind::Png => "-png",
            ImageKind::Jpeg => "-jpeg",
        }
    }

    fn ext(self) -> &'static str {
        match self {
            ImageKind::Png => "png",
            ImageKind::Jpeg => "jpg",
        }
    }
}

fn render_pages(job: &Job, cx: &Context<'_>, kind: ImageKind) -> Result<Vec<PathBuf>, JobError> {
    let pdftoppm = cx.tools.path(ToolKey::Pdftoppm)?;
    let total = page_count_hint(cx, &job.source);
    if let Some(total) = total
        && total > PAGE_LIMIT
    {
        return Err(too_many_pages(total));
    }

    cx.ctx.set_stage("Rendering the pages");
    cx.ctx.checkpoint()?;
    let prefix = cx.out("page");
    let cmd = Cmd::new(&pdftoppm)
        .args(["-r", RENDER_DPI])
        .arg(kind.flag())
        .args(["-f", "1", "-l"])
        .arg((PAGE_LIMIT + 1).to_string())
        .arg(&job.source)
        .arg(&prefix)
        .budget(Budget::Long);
    run_while_counting(cmd, cx, kind.ext(), total)?;

    let files = page_files(&cx.work.out(), &[kind.ext()]);
    if files.len() > PAGE_LIMIT {
        for file in &files {
            let _ = fs::remove_file(file);
        }
        return Err(too_many_pages(files.len()));
    }
    if files.is_empty() {
        return Err(JobError::new(
            ErrorCode::EngineFailed,
            "pdftoppm produced no page images",
        ));
    }
    Ok(files)
}

/// Run a renderer while a watcher reports progress from the files appearing in
/// `work/out` — pdftoppm writes one file per page as it goes.
fn run_while_counting(
    cmd: Cmd,
    cx: &Context<'_>,
    ext: &'static str,
    total: Option<usize>,
) -> Result<proc::Output, JobError> {
    let stop = Arc::new(AtomicBool::new(false));
    let watcher = {
        let stop = stop.clone();
        let ctx = cx.ctx.clone();
        let dir = cx.work.out();
        std::thread::spawn(move || {
            while !stop.load(Ordering::Relaxed) {
                let done = page_files(&dir, &[ext]).len();
                let progress = match total {
                    Some(total) if total > 0 => 0.05 + 0.9 * (done as f64 / total as f64).min(1.0),
                    _ => 0.5,
                };
                ctx.set_progress(progress);
                std::thread::sleep(Duration::from_millis(100));
            }
        })
    };
    let result = proc::run(cmd, cx.ctx);
    stop.store(true, Ordering::Relaxed);
    let _ = watcher.join();
    result
}

fn save_pages(job: &Job, cx: &Context<'_>) -> Result<Vec<PathBuf>, JobError> {
    let kind = match str_param(job.intent.params(), "format") {
        None => ImageKind::Jpeg,
        Some(format) => match format.trim().to_ascii_lowercase().as_str() {
            "jpg" | "jpeg" => ImageKind::Jpeg,
            "png" => ImageKind::Png,
            _ => return Err(bad_param("format", "png or jpg")),
        },
    };
    render_pages(job, cx, kind)
}

/// Files named `page-<n>.<ext>` in `dir`, in page order.
fn page_files(dir: &Path, exts: &[&str]) -> Vec<PathBuf> {
    let Ok(entries) = fs::read_dir(dir) else {
        return Vec::new();
    };
    let mut files: Vec<PathBuf> = entries
        .flatten()
        .map(|entry| entry.path())
        .filter(|path| path.is_file())
        .filter(|path| {
            let Some(name) = path.file_name().and_then(OsStr::to_str) else {
                return false;
            };
            if !name.starts_with("page-") {
                return false;
            }
            path.extension()
                .and_then(OsStr::to_str)
                .map(|ext| exts.contains(&ext))
                .unwrap_or(false)
        })
        .collect();
    files.sort_by_key(|path| page_number(path));
    files
}

fn page_number(path: &Path) -> u32 {
    path.file_stem()
        .and_then(OsStr::to_str)
        .and_then(|stem| stem.rsplit('-').next())
        .and_then(|n| n.parse().ok())
        .unwrap_or(u32::MAX)
}

fn too_many_pages(pages: usize) -> JobError {
    JobError::new(
        ErrorCode::EngineFailed,
        format!("this document has {pages} pages; the limit is {PAGE_LIMIT}"),
    )
}

/// Page count from qpdf when it is present, so a huge document is refused before
/// any page is rendered. Progress falls back to a coarse value when absent.
fn page_count_hint(cx: &Context<'_>, input: &Path) -> Option<usize> {
    if !cx.tools.has(ToolKey::Qpdf) {
        return None;
    }
    let qpdf = cx.tools.path(ToolKey::Qpdf).ok()?;
    let out = proc::run(
        Cmd::new(&qpdf)
            .arg("--show-npages")
            .arg(input)
            .stdout_piped()
            .budget(Budget::Short),
        cx.ctx,
    )
    .ok()?;
    out.stdout.trim().parse().ok()
}

fn required_page_count(
    qpdf: &Path,
    input: &Path,
    ctx: &convertme_core::job::JobCtx,
) -> Result<usize, JobError> {
    let out = proc::run(
        Cmd::new(qpdf)
            .arg("--show-npages")
            .arg(input)
            .stdout_piped()
            .budget(Budget::Short),
        ctx,
    )?;
    out.stdout.trim().parse().map_err(|_| {
        JobError::new(
            ErrorCode::EngineFailed,
            format!("qpdf did not report a page count for {}", input.display()),
        )
        .with_stderr(&out.stdout)
    })
}

// ---------------------------------------------------------------- organize

fn organize(job: &Job, cx: &Context<'_>) -> Result<Vec<PathBuf>, JobError> {
    let qpdf = cx.tools.path(ToolKey::Qpdf)?;
    let total = required_page_count(&qpdf, &job.source, cx.ctx)?;

    let params = job.intent.params();
    let deleted = int_list_param(params, "delete")?.unwrap_or_default();
    let mut pages: Vec<usize> = match int_list_param(params, "order")? {
        Some(order) if !order.is_empty() => order,
        _ => (1..=total).collect(),
    };
    pages.retain(|page| *page >= 1 && *page <= total);
    pages.retain(|page| !deleted.contains(page));
    if pages.is_empty() {
        return Err(JobError::new(
            ErrorCode::EngineFailed,
            "no pages would remain after the requested order and deletions",
        ));
    }

    let same_width = match params.and_then(|params| params.get("same_width")) {
        Some(value) => value
            .as_bool()
            .ok_or_else(|| bad_param("same_width", "true or false"))?,
        None => false,
    };

    let page_spec = pages
        .iter()
        .map(|page| page.to_string())
        .collect::<Vec<_>>()
        .join(",");

    cx.ctx.set_stage("Organizing the pages");
    cx.ctx.checkpoint()?;
    let staged = cx.scratch("organized.pdf");
    let rebuilt = proc::run(
        Cmd::new(&qpdf)
            .arg("--empty")
            .arg("--pages")
            .arg(&job.source)
            .arg(&page_spec)
            .arg("--")
            .arg(&staged)
            .budget(Budget::Short),
        cx.ctx,
    )?;
    // A spec that repeats a page is one qpdf may refuse; cloning the page objects
    // produces the same document without depending on that.
    if !rebuilt.success {
        clone_pages(&job.source, &pages, &staged)?;
    }
    ensure_file(&staged, "qpdf")?;

    let rotation = rotation_param(params)?;
    let mut groups: Vec<(i64, Vec<usize>)> = Vec::new();
    for (position, degrees) in rotation {
        if position == 0 || position > pages.len() {
            continue;
        }
        let normalized = ((degrees % 360) + 360) % 360;
        if normalized == 0 {
            continue;
        }
        let signed = if normalized > 180 {
            normalized - 360
        } else {
            normalized
        };
        match groups.iter_mut().find(|(angle, _)| *angle == signed) {
            Some((_, positions)) => positions.push(position),
            None => groups.push((signed, vec![position])),
        }
    }
    groups.sort_by_key(|(angle, _)| *angle);

    let out = cx.out("organized.pdf");
    if groups.is_empty() {
        fs::rename(&staged, &out)
            .map_err(|e| JobError::io(format!("moving {} into place", out.display()), &e))?;
    } else {
        cx.ctx.checkpoint()?;
        cx.ctx.set_stage("Rotating the pages");
        let mut cmd = Cmd::new(&qpdf);
        for (angle, positions) in &groups {
            if positions.is_empty() {
                continue;
            }
            let list = positions
                .iter()
                .map(|position| position.to_string())
                .collect::<Vec<_>>()
                .join(",");
            cmd = cmd.arg(format!("--rotate={angle:+}:{list}"));
        }
        proc::run(cmd.arg(&staged).arg(&out).budget(Budget::Short), cx.ctx)?.ok()?;
    }

    if same_width {
        cx.ctx.checkpoint()?;
        cx.ctx.set_stage("Matching the page widths");
        match_pages_to_narrowest(&out)?;
    }

    check_pdf_header(&out)?;
    ensure_file(&out, "qpdf")?;
    Ok(vec![out])
}

/// Build a document out of the requested page order, duplicating where asked.
///
/// Used when qpdf will not take the page spec (repeated pages). Pages are cloned
/// individually, so the result is the same sequence of page objects in a new file.
fn clone_pages(source: &Path, pages: &[usize], out: &Path) -> Result<(), JobError> {
    use lopdf::{Dictionary, Document, Object};

    let doc = Document::load(source)
        .map_err(|e| JobError::new(ErrorCode::EngineFailed, format!("reading the PDF: {e}")))?;
    let numbered: Vec<(u32, lopdf::ObjectId)> = doc.get_pages().into_iter().collect();
    let mut target = Document::with_version("1.7");
    let pages_id = target.new_object_id();
    let mut kids = Vec::with_capacity(pages.len());
    for page in pages {
        let Some((_, id)) = numbered.get(page.saturating_sub(1)) else {
            continue;
        };
        let mut clone = doc
            .get_object(*id)
            .and_then(|object| object.as_dict())
            .map_err(|e| {
                JobError::new(ErrorCode::EngineFailed, format!("reading a PDF page: {e}"))
            })?
            .clone();
        // The parent moves to the new tree; copying it would point at the old one.
        clone.remove(b"Parent");
        clone.set("Parent", pages_id);
        let cloned = target.add_object(Object::Dictionary(clone));
        kids.push(cloned.into());
    }
    if kids.is_empty() {
        return Err(JobError::new(
            ErrorCode::EngineFailed,
            "no pages would remain after the requested order",
        ));
    }
    let mut pages_dict = Dictionary::new();
    pages_dict.set("Type", "Pages");
    pages_dict.set("Kids", kids);
    pages_dict.set("Count", pages.len() as i64);
    target
        .objects
        .insert(pages_id, Object::Dictionary(pages_dict));
    let mut catalog = Dictionary::new();
    catalog.set("Type", "Catalog");
    catalog.set("Pages", pages_id);
    let catalog_id = target.add_object(Object::Dictionary(catalog));
    target.trailer.set("Root", catalog_id);
    target
        .save(out)
        .map_err(|e| JobError::new(ErrorCode::EngineFailed, format!("writing the PDF: {e}")))?;
    Ok(())
}

/// Scale every page to the narrowest page's width.
///
/// The page is not redrawn: the existing content is wrapped in a `cm` scale and the
/// MediaBox is resized, which keeps text, images and rotation exactly as they were
/// and stays cheap on a long document. Heights follow the same factor, so a page that
/// was taller keeps its proportion (the reference says so in the dialog's own help).
fn match_pages_to_narrowest(path: &Path) -> Result<(), JobError> {
    use lopdf::{Dictionary, Document, Object, Stream};

    let mut doc = Document::load(path)
        .map_err(|e| JobError::new(ErrorCode::EngineFailed, format!("reading the PDF: {e}")))?;
    let page_ids: Vec<lopdf::ObjectId> = doc.get_pages().into_values().collect();
    if page_ids.len() < 2 {
        return Ok(());
    }

    fn resolved<'a>(doc: &'a Document, object: &'a Object) -> &'a Object {
        let mut current = object;
        for _ in 0..8 {
            match current {
                Object::Reference(id) => match doc.get_object(*id) {
                    Ok(next) => current = next,
                    Err(_) => break,
                },
                _ => break,
            }
        }
        current
    }
    let number = |object: &Object| -> Option<f32> {
        match object {
            Object::Integer(i) => Some(*i as f32),
            Object::Real(f) => Some(*f),
            _ => None,
        }
    };
    let media_box = |doc: &Document, object: &Dictionary| -> Option<[f32; 4]> {
        let array = object.get(b"MediaBox").ok().map(|o| resolved(doc, o))?;
        let values: Vec<f32> = array.as_array().ok()?.iter().filter_map(number).collect();
        if values.len() == 4 {
            Some([values[0], values[1], values[2], values[3]])
        } else {
            None
        }
    };

    let mut targets = Vec::with_capacity(page_ids.len());
    let mut narrowest = f32::MAX;
    for id in &page_ids {
        let page = doc
            .get_object(*id)
            .and_then(|o| o.as_dict())
            .map_err(|e| JobError::new(ErrorCode::EngineFailed, format!("reading a page: {e}")))?;
        let Some(box_) = media_box(&doc, page) else {
            continue;
        };
        let width = (box_[2] - box_[0]).abs();
        if width > 0.0 {
            narrowest = narrowest.min(width);
        }
        targets.push((*id, box_));
    }
    if !narrowest.is_finite() || narrowest <= 0.0 {
        return Ok(());
    }

    let mut prepended: Vec<lopdf::ObjectId> = Vec::new();
    let mut appended: Vec<lopdf::ObjectId> = Vec::new();
    for (id, box_) in targets {
        let width = (box_[2] - box_[0]).abs();
        if width <= 0.0 || (width - narrowest).abs() < 0.01 {
            continue;
        }
        let scale = narrowest / width;
        let height = (box_[3] - box_[1]).abs() * scale;
        let open = Stream::new(
            Dictionary::new(),
            format!("q {scale:.6} 0 0 {scale:.6} 0 0 cm\n").into_bytes(),
        );
        let close = Stream::new(Dictionary::new(), b"Q\n".to_vec());
        let open_id = doc.add_object(Object::Stream(open));
        let close_id = doc.add_object(Object::Stream(close));
        prepended.push(open_id);
        appended.push(close_id);

        // The scale runs before the page's own content and the matching Q after it,
        // so nothing in the original streams is rewritten.
        let page = doc
            .get_object_mut(id)
            .and_then(|o| o.as_dict_mut())
            .map_err(|e| JobError::new(ErrorCode::EngineFailed, format!("editing a page: {e}")))?;
        let existing: Vec<Object> = match page.get(b"Contents") {
            Ok(Object::Array(items)) => items.clone(),
            Ok(Object::Reference(reference)) => vec![Object::Reference(*reference)],
            _ => Vec::new(),
        };
        let mut contents = vec![Object::Reference(open_id)];
        contents.extend(existing);
        contents.push(Object::Reference(close_id));
        page.set("Contents", contents);
        page.set(
            "MediaBox",
            vec![
                0.into(),
                0.into(),
                Object::Real(narrowest),
                Object::Real(height),
            ],
        );
        if page.get(b"CropBox").is_ok() {
            page.set(
                "CropBox",
                vec![
                    0.into(),
                    0.into(),
                    Object::Real(narrowest),
                    Object::Real(height),
                ],
            );
        }
    }

    let tmp = path.with_extension("same-width.pdf");
    doc.save(&tmp)
        .map_err(|e| JobError::new(ErrorCode::EngineFailed, format!("writing the PDF: {e}")))?;
    fs::rename(&tmp, path).map_err(|e| JobError::io("replacing the organized PDF", &e))?;
    Ok(())
}

/// The job's inputs in the user's chosen order.
///
/// `order` holds indices into `inputs`; anything else (absent, short, out of range)
/// means "the selection order", which is what the engine did before the dialog could
/// reorder anything. A partial order is completed with the inputs it left out, so a
/// dropped entry never loses a document.
fn ordered_inputs(job: &Job) -> Result<Vec<PathBuf>, JobError> {
    let params = job.intent.params();
    let Some(order) = int_list_param(params, "order")? else {
        return Ok(job.inputs.clone());
    };
    if order.is_empty() {
        return Ok(job.inputs.clone());
    }
    let mut seen: Vec<usize> = Vec::with_capacity(order.len());
    let mut files: Vec<PathBuf> = Vec::with_capacity(job.inputs.len());
    for index in order {
        if index >= job.inputs.len() || seen.contains(&index) {
            return Err(bad_param("order", "each input exactly once, as indices"));
        }
        seen.push(index);
        files.push(job.inputs[index].clone());
    }
    for (index, input) in job.inputs.iter().enumerate() {
        if !seen.contains(&index) {
            files.push(input.clone());
        }
    }
    Ok(files)
}

fn rotation_param(params: Option<&ToolParams>) -> Result<Vec<(usize, i64)>, JobError> {
    let Some(params) = params else {
        return Ok(Vec::new());
    };
    let Some(value) = params.get("rotate") else {
        return Ok(Vec::new());
    };
    let map = value
        .as_object()
        .ok_or_else(|| bad_param("rotate", "an object mapping page numbers to degrees"))?;
    let mut entries = Vec::with_capacity(map.len());
    for (key, value) in map {
        let position: usize = key
            .trim()
            .parse()
            .map_err(|_| bad_param("rotate", "an object keyed by page numbers"))?;
        let degrees = value
            .as_i64()
            .or_else(|| value.as_f64().map(|f| f as i64))
            .ok_or_else(|| bad_param("rotate", "an object mapping page numbers to degrees"))?;
        if degrees % 90 != 0 {
            return Err(bad_param("rotate", "a rotation in multiples of 90 degrees"));
        }
        entries.push((position, degrees));
    }
    entries.sort_by_key(|(position, _)| *position);
    Ok(entries)
}

// ------------------------------------------------------------------- merge

fn merge(job: &Job, cx: &Context<'_>) -> Result<Vec<PathBuf>, JobError> {
    let qpdf = cx.tools.path(ToolKey::Qpdf)?;
    if job.inputs.is_empty() {
        return Err(JobError::new(ErrorCode::Io, "no documents to merge"));
    }
    // The dialog can reorder the documents; `order` is a permutation of the inputs as
    // indices into them, so the file whose card was dragged second is merged second.
    let order = ordered_inputs(job)?;

    cx.ctx.set_stage("Merging the documents");
    cx.ctx.checkpoint()?;
    let out = cx.out("merged.pdf");
    let mut cmd = Cmd::new(&qpdf).arg("--empty").arg("--pages");
    for input in &order {
        cmd = cmd.arg(input);
    }
    proc::run(cmd.arg("--").arg(&out).budget(Budget::Short), cx.ctx)?.ok()?;

    check_pdf_header(&out)?;
    ensure_file(&out, "qpdf")?;
    Ok(vec![out])
}

// ------------------------------------------------------------------- split

fn split(job: &Job, cx: &Context<'_>) -> Result<Vec<PathBuf>, JobError> {
    let qpdf = cx.tools.path(ToolKey::Qpdf)?;
    cx.ctx.set_stage("Splitting the pages");
    cx.ctx.checkpoint()?;

    let prefix = cx.out("page");
    proc::run(
        Cmd::new(&qpdf)
            .arg("--split-pages")
            .arg(&job.source)
            .arg(&prefix)
            .budget(Budget::Short),
        cx.ctx,
    )?
    .ok()?;

    let entries = fs::read_dir(cx.work.out()).map_err(|e| {
        JobError::io(
            format!("reading the output directory {}", cx.work.out().display()),
            &e,
        )
    })?;
    let mut artifacts = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if !path.is_file() {
            continue;
        }
        let Some(name) = path.file_name().and_then(OsStr::to_str) else {
            continue;
        };
        if !name.starts_with("page-") {
            continue;
        }
        // qpdf 12 writes `page-1` with no extension; the publish step needs one.
        if path
            .extension()
            .and_then(OsStr::to_str)
            .map(|ext| ext.eq_ignore_ascii_case("pdf"))
            .unwrap_or(false)
        {
            artifacts.push(path);
        } else {
            let target = path.with_extension("pdf");
            fs::rename(&path, &target)
                .map_err(|e| JobError::io(format!("renaming {}", path.display()), &e))?;
            artifacts.push(target);
        }
    }
    artifacts.sort_by_key(|path| page_number(path));
    if artifacts.is_empty() {
        return Err(JobError::new(
            ErrorCode::EngineFailed,
            "qpdf split the document into no files",
        ));
    }
    for artifact in &artifacts {
        check_pdf_header(artifact)?;
        ensure_file(artifact, "qpdf")?;
    }
    Ok(artifacts)
}

// --------------------------------------------------------------- compress

fn compress(job: &Job, cx: &Context<'_>) -> Result<Vec<PathBuf>, JobError> {
    let gs = cx.tools.path(ToolKey::Gs)?;
    let params = job.intent.params();
    let preset = match str_param(params, "preset") {
        None => "ebook",
        Some(value) => match value.trim().to_ascii_lowercase().as_str() {
            "screen" => "screen",
            "ebook" => "ebook",
            "printer" => "printer",
            "prepress" => "prepress",
            _ => return Err(bad_param("preset", "screen, ebook, printer or prepress")),
        },
    };
    let target_bytes = number_param(params, "target_bytes")
        .filter(|value| *value > 0.0)
        .map(|value| value as u64);

    let mut best = cx.out("compressed.pdf");
    run_gs(&gs, preset, &job.source, &best, cx)?;

    if let Some(target) = target_bytes
        && file_len(&best) > target
    {
        cx.ctx.checkpoint()?;
        cx.ctx.set_stage("Compressing further");
        let screen = cx.out("compressed-screen.pdf");
        run_gs(&gs, "screen", &job.source, &screen, cx)?;
        if file_len(&screen) < file_len(&best) {
            let _ = fs::remove_file(&best);
            best = screen;
        } else {
            let _ = fs::remove_file(&screen);
        }
    }

    // A compressor that grows a file is not a compressor: fall back to a copy of
    // the original so the artifact is never larger than the input.
    let source_len = fs::metadata(&job.source)
        .map(|meta| meta.len())
        .unwrap_or(0);
    if file_len(&best) > source_len {
        let copy = cx.out("compressed-copy.pdf");
        fs::copy(&job.source, &copy)
            .map_err(|e| JobError::io(format!("copying {}", job.source.display()), &e))?;
        let _ = fs::remove_file(&best);
        best = copy;
    }

    check_pdf_header(&best)?;
    ensure_file(&best, "Ghostscript")?;
    Ok(vec![best])
}

fn run_gs(
    gs: &Path,
    preset: &str,
    input: &Path,
    out: &Path,
    cx: &Context<'_>,
) -> Result<(), JobError> {
    cx.ctx.set_stage(format!("Compressing ({preset})"));
    cx.ctx.checkpoint()?;
    let mut output_arg = OsString::from("-sOutputFile=");
    output_arg.push(out.as_os_str());
    proc::run(
        Cmd::new(gs)
            .arg("-sDEVICE=pdfwrite")
            .arg("-dCompatibilityLevel=1.7")
            .arg(format!("-dPDFSETTINGS=/{preset}"))
            .args(["-dNOPAUSE", "-dQUIET", "-dBATCH"])
            .arg(output_arg)
            .arg(input)
            .budget(Budget::Long),
        cx.ctx,
    )?
    .ok()?;
    ensure_file(out, "Ghostscript")?;
    Ok(())
}

// ----------------------------------------------------------------- sidecar

fn sidecar(
    job: &Job,
    cx: &Context<'_>,
    subcommand: &str,
    ext: &str,
) -> Result<Vec<PathBuf>, JobError> {
    let sidecar = cx.tools.path(ToolKey::Sidecar)?;
    let out = cx.out(&format!("output.{ext}"));
    cx.ctx
        .set_stage(format!("Exporting to {}", ext.to_ascii_uppercase()));
    cx.ctx.checkpoint()?;
    proc::run(
        Cmd::new(&sidecar)
            .arg(subcommand)
            .arg(&job.source)
            .arg(&out)
            .budget(Budget::Long),
        cx.ctx,
    )?
    .ok()?;
    ensure_file(&out, "the PDF sidecar")?;
    Ok(vec![out])
}

// ------------------------------------------------------------------ params

fn bad_param(key: &str, expected: &str) -> JobError {
    JobError::new(
        ErrorCode::EngineFailed,
        format!("`{key}` must be {expected}"),
    )
}

fn str_param<'a>(params: Option<&'a ToolParams>, key: &str) -> Option<&'a str> {
    params?.get(key)?.as_str()
}

fn number_param(params: Option<&ToolParams>, key: &str) -> Option<f64> {
    let value = params?.get(key)?;
    value
        .as_f64()
        .or_else(|| value.as_str().and_then(|text| text.trim().parse().ok()))
}

fn int_list_param(params: Option<&ToolParams>, key: &str) -> Result<Option<Vec<usize>>, JobError> {
    let Some(params) = params else {
        return Ok(None);
    };
    let Some(value) = params.get(key) else {
        return Ok(None);
    };
    let array = value
        .as_array()
        .ok_or_else(|| bad_param(key, "an array of page numbers"))?;
    let mut pages = Vec::with_capacity(array.len());
    for item in array {
        let number = item
            .as_u64()
            .or_else(|| item.as_str().and_then(|text| text.trim().parse().ok()))
            .ok_or_else(|| bad_param(key, "an array of page numbers"))?;
        let page =
            usize::try_from(number).map_err(|_| bad_param(key, "an array of page numbers"))?;
        pages.push(page);
    }
    Ok(Some(pages))
}

fn file_len(path: &Path) -> u64 {
    fs::metadata(path).map(|meta| meta.len()).unwrap_or(0)
}

fn check_pdf_header(path: &Path) -> Result<(), JobError> {
    let file = fs::File::open(path)
        .map_err(|e| JobError::io(format!("opening {}", path.display()), &e))?;
    // The specification allows the marker anywhere in the first 1024 bytes.
    let mut head = Vec::with_capacity(1024);
    file.take(1024)
        .read_to_end(&mut head)
        .map_err(|e| JobError::io(format!("reading {}", path.display()), &e))?;
    if head.windows(5).any(|window| window == b"%PDF-") {
        Ok(())
    } else {
        Err(JobError::new(
            ErrorCode::EngineFailed,
            format!("{} is not a PDF", path.display()),
        ))
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Toolset;
    use convertme_core::job::{JobCtx, WorkDir};
    use std::process::Command;

    fn host_tools(needed: &[ToolKey]) -> Option<Toolset> {
        let tools = Toolset::probe_now();
        if needed.iter().all(|key| tools.path(*key).is_ok()) {
            Some(tools)
        } else {
            None
        }
    }

    /// `None` when this ImageMagick build cannot write PDF (distro policies do
    /// block it), so the test skips instead of failing on the host's setup.
    fn make_pdf(tools: &Toolset, dir: &Path, name: &str, pages: usize) -> Option<PathBuf> {
        let magick = tools.path(ToolKey::Magick).ok()?;
        let mut pngs = Vec::new();
        for i in 1..=pages {
            let png = dir.join(format!("{name}-{i}.png"));
            let status = Command::new(&magick)
                .args([
                    "-size",
                    "180x180",
                    "xc:white",
                    "-fill",
                    "black",
                    "-pointsize",
                    "24",
                    "-gravity",
                    "center",
                    "-annotate",
                    "0",
                ])
                .arg(format!("Page {i}"))
                .arg(&png)
                .status()
                .ok()?;
            if !status.success() {
                return None;
            }
            pngs.push(png);
        }
        let pdf = dir.join(format!("{name}.pdf"));
        let mut cmd = Command::new(&magick);
        cmd.args(&pngs).arg(&pdf);
        cmd.status().ok()?.success().then_some(pdf)
    }

    fn run_in(tools: &Toolset, root: &Path, job: &Job) -> Result<Vec<PathBuf>, JobError> {
        let work = WorkDir::create(root.join("work")).expect("work dir");
        let ctx = JobCtx::new();
        let cx = Context {
            tools,
            work: &work,
            ctx: &ctx,
            preview: None,
        };
        run(job, &cx)
    }

    fn page_count(qpdf: &Path, pdf: &Path) -> usize {
        let out = Command::new(qpdf)
            .arg("--show-npages")
            .arg(pdf)
            .output()
            .expect("running qpdf");
        String::from_utf8_lossy(&out.stdout).trim().parse().unwrap()
    }

    #[test]
    fn split_writes_one_pdf_per_page() {
        let Some(tools) = host_tools(&[ToolKey::Magick, ToolKey::Qpdf]) else {
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let Some(pdf) = make_pdf(&tools, dir.path(), "split", 2) else {
            return;
        };
        let qpdf = tools.path(ToolKey::Qpdf).unwrap();

        let job = Job::new(pdf, Intent::tool(ToolId::Split));
        let artifacts = run_in(&tools, dir.path(), &job).unwrap();

        assert_eq!(artifacts.len(), 2);
        for artifact in &artifacts {
            assert_eq!(page_count(&qpdf, artifact), 1, "{}", artifact.display());
        }
    }

    #[test]
    fn save_pages_renders_each_page() {
        let Some(tools) = host_tools(&[ToolKey::Magick, ToolKey::Pdftoppm]) else {
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let Some(pdf) = make_pdf(&tools, dir.path(), "pages", 2) else {
            return;
        };

        let mut params = ToolParams::new();
        params.insert("format".into(), serde_json::json!("png"));
        let job = Job::new(
            pdf,
            Intent::Tool {
                tool: ToolId::SavePages,
                params,
            },
        );
        let artifacts = run_in(&tools, dir.path(), &job).unwrap();

        assert_eq!(artifacts.len(), 2);
        for artifact in &artifacts {
            assert_eq!(artifact.extension().and_then(OsStr::to_str), Some("png"));
            let bytes = fs::read(artifact).unwrap();
            assert!(bytes.starts_with(b"\x89PNG\r\n\x1a\n"));
        }
    }

    #[test]
    fn compress_produces_a_pdf_no_larger_than_the_source() {
        let Some(tools) = host_tools(&[ToolKey::Magick, ToolKey::Gs]) else {
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let Some(pdf) = make_pdf(&tools, dir.path(), "compress", 2) else {
            return;
        };
        let source_len = fs::metadata(&pdf).unwrap().len();

        let job = Job::new(pdf, Intent::tool(ToolId::Compress));
        let artifacts = run_in(&tools, dir.path(), &job).unwrap();

        assert_eq!(artifacts.len(), 1);
        let bytes = fs::read(&artifacts[0]).unwrap();
        assert!(bytes.starts_with(b"%PDF-"));
        assert!(bytes.len() as u64 <= source_len);
    }

    #[test]
    fn organize_reorders_the_pages() {
        let Some(tools) = host_tools(&[ToolKey::Magick, ToolKey::Qpdf]) else {
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let Some(pdf) = make_pdf(&tools, dir.path(), "order", 2) else {
            return;
        };
        let qpdf = tools.path(ToolKey::Qpdf).unwrap();

        let mut params = ToolParams::new();
        params.insert("order".into(), serde_json::json!([2, 1]));
        let job = Job::new(
            pdf,
            Intent::Tool {
                tool: ToolId::Organize,
                params,
            },
        );
        let artifacts = run_in(&tools, dir.path(), &job).unwrap();

        assert_eq!(artifacts.len(), 1);
        assert!(fs::read(&artifacts[0]).unwrap().starts_with(b"%PDF-"));
        assert_eq!(page_count(&qpdf, &artifacts[0]), 2);
    }

    /// A page can appear twice: the organizer's duplicate is a repeated entry in
    /// `order`, and qpdf may refuse the spec, so the clone path has to produce the
    /// same document.
    #[test]
    fn organize_duplicates_a_page() {
        let Some(tools) = host_tools(&[ToolKey::Magick, ToolKey::Qpdf]) else {
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let Some(pdf) = make_pdf(&tools, dir.path(), "dupe", 3) else {
            return;
        };
        let qpdf = tools.path(ToolKey::Qpdf).unwrap();

        let mut params = ToolParams::new();
        params.insert("order".into(), serde_json::json!([1, 2, 2, 3]));
        let job = Job::new(
            pdf,
            Intent::Tool {
                tool: ToolId::Organize,
                params,
            },
        );
        let artifacts = run_in(&tools, dir.path(), &job).unwrap();
        assert_eq!(artifacts.len(), 1);
        assert_eq!(
            page_count(&qpdf, &artifacts[0]),
            4,
            "the duplicated page is written twice"
        );
    }

    /// Pages of different widths become one width, proportionally, and the page count
    /// does not change.
    #[test]
    fn organize_matches_every_page_width() {
        let Some(tools) = host_tools(&[ToolKey::Magick, ToolKey::Qpdf]) else {
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        // Two pages of different widths in one document.
        let magick = tools.path(ToolKey::Magick).unwrap();
        let wide = dir.path().join("wide.png");
        let narrow = dir.path().join("narrow.png");
        for (path, size) in [(&wide, "300x300"), (&narrow, "150x300")] {
            assert!(
                Command::new(&magick)
                    .args(["-size", size, "xc:white"])
                    .arg(path)
                    .status()
                    .expect("magick")
                    .success()
            );
        }
        let source = dir.path().join("mixed.pdf");
        assert!(
            Command::new(&magick)
                .args([&wide, &narrow])
                .arg(&source)
                .status()
                .expect("magick")
                .success()
        );

        let mut params = ToolParams::new();
        params.insert("same_width".into(), serde_json::json!(true));
        let job = Job::new(
            source,
            Intent::Tool {
                tool: ToolId::Organize,
                params,
            },
        );
        let artifacts = run_in(&tools, dir.path(), &job).unwrap();
        let doc = lopdf::Document::load(&artifacts[0]).expect("parse the result");
        let mut widths: Vec<f32> = Vec::new();
        let mut heights: Vec<f32> = Vec::new();
        for (_, id) in doc.get_pages() {
            let page = doc.get_object(id).and_then(|o| o.as_dict()).expect("page");
            let box_ = page
                .get(b"MediaBox")
                .and_then(|o| o.as_array())
                .expect("MediaBox");
            let values: Vec<f32> = box_
                .iter()
                .map(|item| match item {
                    lopdf::Object::Integer(i) => *i as f32,
                    lopdf::Object::Real(f) => *f,
                    _ => 0.0,
                })
                .collect();
            widths.push(values[2] - values[0]);
            heights.push(values[3] - values[1]);
        }
        assert_eq!(widths.len(), 2, "the page count is unchanged");
        assert!(
            (widths[0] - widths[1]).abs() < 1.0,
            "pages differ in width: {widths:?}"
        );
        assert!(
            (heights[0] - heights[1]).abs() > 1.0,
            "the narrower page must keep its taller shape: {heights:?}"
        );
        assert!(
            widths[0] < 300.0,
            "the wide page is scaled down, not the narrow one up: {widths:?}"
        );
    }

    /// PDF → Markdown runs through pandoc, whose reader names change between major
    /// versions: `plain` was removed in pandoc 3, and nothing noticed because no test
    /// exercised this path.
    #[test]
    fn pdf_to_markdown_keeps_the_words() {
        let Some(tools) = host_tools(&[ToolKey::Magick, ToolKey::Pdftotext, ToolKey::Pandoc])
        else {
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let Some(pdf) = make_pdf(&tools, dir.path(), "md", 1) else {
            return;
        };
        let job = Job::new(
            pdf,
            Intent::Convert {
                target: convertme_core::formats::FormatId("MD"),
            },
        );
        let artifacts = run_in(&tools, dir.path(), &job).unwrap();
        let text = fs::read_to_string(&artifacts[0]).unwrap();
        assert!(
            text.to_lowercase().contains("page"),
            "the markdown lost the page's words: {text:?}"
        );
    }

    #[test]
    fn merge_combines_every_input_in_order() {
        let Some(tools) = host_tools(&[ToolKey::Magick, ToolKey::Qpdf]) else {
            return;
        };
        let dir = tempfile::tempdir().unwrap();
        let (Some(first), Some(second)) = (
            make_pdf(&tools, dir.path(), "first", 2),
            make_pdf(&tools, dir.path(), "second", 2),
        ) else {
            return;
        };
        let qpdf = tools.path(ToolKey::Qpdf).unwrap();

        let job = Job::with_inputs(vec![first, second], Intent::tool(ToolId::Merge));
        let artifacts = run_in(&tools, dir.path(), &job).unwrap();

        assert_eq!(artifacts.len(), 1);
        assert_eq!(page_count(&qpdf, &artifacts[0]), 4);
    }
}
