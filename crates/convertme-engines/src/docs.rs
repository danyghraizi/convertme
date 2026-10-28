//! Documents, slides, spreadsheets, EPUB and subtitles.
//!
//! Text-shaped document conversions go through Pandoc. Anything that needs a
//! page layout — Office → PDF, DOCX ↔ ODT, document or slide pages as images —
//! goes through LibreOffice with a private profile per job, and multi-page image
//! export renders the generated PDF with Poppler. Spreadsheets are read with
//! `calamine`, written with `rust_xlsxwriter` (or LibreOffice for ODS/PDF), and
//! flat CSV/TSV never leaves the process. Subtitles are pure Rust.
//!
//! No function here ever writes outside [`Context::work`]; every published path
//! is a file this module produced, checked with [`crate::ensure_file`].

use crate::proc::{self, Budget, Cmd};
use crate::{Context, ensure_file, ext_of};
use calamine::{Data, Reader, open_workbook_auto};
use convertme_core::capability::ToolKey;
use convertme_core::error::{ErrorCode, JobError};
use convertme_core::formats::{Category, FormatId, Registry};
use convertme_core::job::{Intent, Job};
use convertme_core::subtitle::Subtitle;
use std::fs::{self, File};
use std::io::{BufRead, BufReader, BufWriter, Read, Write};
use std::path::{Path, PathBuf};

/// Convert a document, slide deck or spreadsheet. Subtitles take
/// [`subtitle`] instead, but a subtitle job arriving here is rejected cleanly.
pub fn run(job: &Job, cx: &Context<'_>) -> Result<Vec<PathBuf>, JobError> {
    let from = input_format(job)?;
    let target = match &job.intent {
        Intent::Convert { target } => *target,
        Intent::Tool { .. } => {
            return Err(JobError::new(
                ErrorCode::Unsupported,
                "document engines only perform conversions",
            ));
        }
    };
    let category = Registry::get(from).map(|f| f.category).ok_or_else(|| {
        JobError::new(
            ErrorCode::Unsupported,
            format!("{from} is not a known format"),
        )
    })?;
    // `engine_for` is the pair table the dispatcher already validated; it also
    // admits the EPUB → PDF override, which the generic category rule misses.
    if Registry::engine_for(from, target).is_none() {
        return Err(JobError::unsupported(from.0, target.0));
    }
    cx.ctx.checkpoint()?;

    match category {
        Category::Documents | Category::Epub => document(job, cx, from, target),
        Category::Slides => slides(job, cx, from, target),
        Category::Spreadsheets => spreadsheet(job, cx, from, target),
        other => Err(JobError::new(
            ErrorCode::Unsupported,
            format!("{other} conversions are not handled by the document engine"),
        )),
    }
}

/// SRT ↔ VTT ↔ TXT. Pure Rust: no tool can be missing here.
pub fn subtitle(job: &Job, cx: &Context<'_>) -> Result<Vec<PathBuf>, JobError> {
    let target = job.intent.target().ok_or_else(|| {
        JobError::new(ErrorCode::Unsupported, "subtitles only perform conversions")
    })?;
    let ext = ext_of(target);
    if !matches!(ext, "srt" | "vtt" | "txt") {
        return Err(JobError::unsupported("subtitles", target.0));
    }
    cx.ctx.set_stage("converting the subtitles");
    cx.ctx.checkpoint()?;
    let bytes = fs::read(&job.source)
        .map_err(|e| JobError::io(format!("reading {}", job.source.display()), &e))?;
    let parsed = Subtitle::parse(&String::from_utf8_lossy(&bytes))?;
    let rendered = parsed.to_document(ext)?;
    let dest = cx.out(&format!("{}.{ext}", stem_of(job)));
    fs::write(&dest, rendered.as_bytes())
        .map_err(|e| JobError::io("writing the converted subtitles", &e))?;
    ensure_file(&dest, "the subtitle writer")?;
    cx.ctx.set_progress(1.0);
    Ok(vec![dest])
}

// ------------------------------------------------------------------ documents

fn document(
    job: &Job,
    cx: &Context<'_>,
    from: FormatId,
    target: FormatId,
) -> Result<Vec<PathBuf>, JobError> {
    let target_ext = ext_of(target);
    match target.0 {
        "PDF" => {
            let pdf = document_pdf(job, cx, from)?;
            let dest = cx.out(&format!("{}.pdf", stem_of(job)));
            move_file(&pdf, &dest)?;
            ensure_file(&dest, "LibreOffice")?;
            cx.ctx.set_progress(1.0);
            Ok(vec![dest])
        }
        "JPG" | "PNG" => document_images(job, cx, from, target_ext),
        // DOCX ↔ ODT keeps tables and page structure only through the layout
        // engine; every other text target is Pandoc's job.
        _ if office_pair(from, target) => {
            let staged = stage_source(job, cx, from)?;
            let produced = soffice_convert(
                cx,
                &staged,
                &out_dir(cx),
                writer_filter(target_ext)?,
                target_ext,
            )?;
            let dest = publish(job, cx, &produced, target_ext, "LibreOffice")?;
            cx.ctx.set_progress(1.0);
            Ok(vec![dest])
        }
        _ => pandoc_convert(job, cx, from, target, target_ext),
    }
}

/// True for the pairs whose layout only survives a real Office renderer.
fn office_pair(from: FormatId, target: FormatId) -> bool {
    matches!((from.0, target.0), ("DOCX", "ODT") | ("ODT", "DOCX"))
}

/// Produce a PDF in the work dir for a document. Text-shaped inputs get an
/// intermediate HTML page because Pandoc cannot emit PDF without a TeX engine.
fn document_pdf(job: &Job, cx: &Context<'_>, from: FormatId) -> Result<PathBuf, JobError> {
    if matches!(from.0, "DOCX" | "ODT") {
        let staged = stage_source(job, cx, from)?;
        return soffice_convert(cx, &staged, &out_dir(cx), writer_filter("pdf")?, "pdf");
    }
    let html = pandoc_html(job, cx, from)?;
    soffice_convert(cx, &html, &out_dir(cx), writer_filter("pdf")?, "pdf")
}

fn document_images(
    job: &Job,
    cx: &Context<'_>,
    from: FormatId,
    target_ext: &str,
) -> Result<Vec<PathBuf>, JobError> {
    cx.ctx.set_stage("rendering the document");
    let pdf = document_pdf(job, cx, from)?;
    let pages = pdf_page_count(&pdf)?;

    // A one-page Office document can be exported directly; anything longer (or
    // any text-shaped input) needs the PDF route so no page is silently lost.
    if pages <= 1 && matches!(from.0, "DOCX" | "ODT") {
        let staged = stage_source(job, cx, from)?;
        let produced = soffice_convert(
            cx,
            &staged,
            &out_dir(cx),
            writer_filter(target_ext)?,
            target_ext,
        )?;
        let dest = publish(job, cx, &produced, target_ext, "LibreOffice")?;
        cx.ctx.set_progress(1.0);
        return Ok(vec![dest]);
    }
    render_pdf_pages(cx, &pdf, target_ext, &stem_of(job))
}

fn pandoc_convert(
    job: &Job,
    cx: &Context<'_>,
    from: FormatId,
    target: FormatId,
    target_ext: &str,
) -> Result<Vec<PathBuf>, JobError> {
    let pandoc = cx.tools.path(ToolKey::Pandoc)?;
    let dest = cx.out(&format!("{}.{target_ext}", stem_of(job)));
    let mut cmd = Cmd::new(&pandoc);
    if let Some(reader) = pandoc_reader(from) {
        cmd = cmd.args(["-f", reader]);
    }
    cmd = cmd.arg(&job.source);
    cmd = match target.0 {
        "MD" => cmd.args(["-t", "gfm"]),
        "HTML" => cmd.args(["-t", "html5", "--standalone"]),
        "TXT" => cmd.args(["-t", "plain"]),
        "EPUB" => cmd.args(["-t", "epub3"]),
        "DOCX" => cmd.args(["-t", "docx"]),
        "ODT" => cmd.args(["-t", "odt"]),
        other => return Err(JobError::unsupported(from.0, other)),
    };
    cmd = cmd.arg("-o").arg(&dest).budget(Budget::Short);
    cx.ctx.set_stage("converting the document");
    proc::run(cmd, cx.ctx)?.ok()?;
    ensure_file(&dest, "Pandoc")?;
    cx.ctx.set_progress(1.0);
    Ok(vec![dest])
}

/// Pandoc's reader name. Explicit, because a `.txt` file is not a format Pandoc
/// recognises from its extension and a `.md` file may be renamed.
fn pandoc_reader(from: FormatId) -> Option<&'static str> {
    match from.0 {
        "DOCX" => Some("docx"),
        "ODT" => Some("odt"),
        "MD" | "TXT" => Some("markdown"),
        "HTML" => Some("html"),
        "EPUB" => Some("epub"),
        _ => None,
    }
}

/// Pandoc → standalone HTML in the work dir, with images embedded so the layout
/// engine sees them. `--embed-resources` needs Pandoc 2.19; older builds get a
/// retry without it rather than a hard failure.
fn pandoc_html(job: &Job, cx: &Context<'_>, from: FormatId) -> Result<PathBuf, JobError> {
    let pandoc = cx.tools.path(ToolKey::Pandoc)?;
    let out = cx.scratch("intermediate.html");
    let build = |embed: bool| {
        let mut cmd = Cmd::new(&pandoc);
        if let Some(reader) = pandoc_reader(from) {
            cmd = cmd.args(["-f", reader]);
        }
        cmd = cmd.arg(&job.source).args(["-t", "html5", "--standalone"]);
        if embed {
            cmd = cmd.arg("--embed-resources");
        }
        cmd.arg("-o").arg(&out).budget(Budget::Short)
    };

    cx.ctx.set_stage("preparing the page layout");
    match proc::run(build(true), cx.ctx) {
        Ok(output) if output.success => {}
        Ok(_) => {
            proc::run(build(false), cx.ctx)?.ok()?;
        }
        Err(e) => return Err(e),
    }
    ensure_file(&out, "Pandoc")?;
    Ok(out)
}

// --------------------------------------------------------------------- slides

fn slides(
    job: &Job,
    cx: &Context<'_>,
    from: FormatId,
    target: FormatId,
) -> Result<Vec<PathBuf>, JobError> {
    let staged = stage_source(job, cx, from)?;
    match target.0 {
        "PDF" => {
            let produced =
                soffice_convert(cx, &staged, &out_dir(cx), impress_filter("pdf")?, "pdf")?;
            let dest = publish(job, cx, &produced, "pdf", "LibreOffice")?;
            cx.ctx.set_progress(1.0);
            Ok(vec![dest])
        }
        // LibreOffice's Impress image filter writes only the first slide; going
        // through the PDF is what guarantees every slide becomes an artifact.
        "JPG" | "PNG" => {
            let ext = ext_of(target);
            let pdf = soffice_convert(cx, &staged, &out_dir(cx), impress_filter("pdf")?, "pdf")?;
            render_pdf_pages(cx, &pdf, ext, &stem_of(job))
        }
        other => Err(JobError::unsupported(from.0, other)),
    }
}

// --------------------------------------------------------------- spreadsheets

fn spreadsheet(
    job: &Job,
    cx: &Context<'_>,
    from: FormatId,
    target: FormatId,
) -> Result<Vec<PathBuf>, JobError> {
    match target.0 {
        "CSV" => write_delimited_sheets(job, cx, from, "csv", ','),
        "TSV" => write_delimited_sheets(job, cx, from, "tsv", '\t'),
        "XLSX" => {
            cx.ctx.set_stage("writing the workbook");
            let dest = cx.out(&format!("{}.xlsx", stem_of(job)));
            write_workbook(from, &job.source, &dest)?;
            cx.ctx.set_progress(1.0);
            Ok(vec![dest])
        }
        "ODS" | "PDF" => {
            let staged = spreadsheet_office_input(job, cx, from)?;
            let (filter, out_ext) = if target.0 == "ODS" {
                (calc_filter("ods")?, "ods")
            } else {
                (calc_filter("pdf")?, "pdf")
            };
            let produced = soffice_convert(cx, &staged, &out_dir(cx), filter, out_ext)?;
            let dest = publish(job, cx, &produced, out_ext, "LibreOffice")?;
            cx.ctx.set_progress(1.0);
            Ok(vec![dest])
        }
        other => Err(JobError::unsupported(from.0, other)),
    }
}

/// A workbook LibreOffice can open. Flat files are materialised as XLSX first,
/// because `--convert-to` cannot import CSV/TSV on its own without filter options.
fn spreadsheet_office_input(
    job: &Job,
    cx: &Context<'_>,
    from: FormatId,
) -> Result<PathBuf, JobError> {
    let dir = cx.scratch("office-in");
    fs::create_dir_all(&dir).map_err(|e| JobError::io("creating the scratch directory", &e))?;
    match from.0 {
        "XLSX" | "XLS" | "ODS" => {
            let staged = dir.join(format!("input.{}", ext_of(from)));
            fs::copy(&job.source, &staged)
                .map_err(|e| JobError::io(format!("staging {}", job.source.display()), &e))?;
            Ok(staged)
        }
        _ => {
            let staged = dir.join("input.xlsx");
            write_workbook(from, &job.source, &staged)?;
            Ok(staged)
        }
    }
}

/// One cell in a spreadsheet-independent form. CSV/TSV fields are typed by
/// `Cell::infer` so writing XLSX does not turn every number into text.
#[derive(Debug, Clone, PartialEq)]
enum Cell {
    Empty,
    Text(String),
    Number(f64),
    Bool(bool),
}

impl Cell {
    fn infer(value: &str) -> Self {
        if value.is_empty() {
            return Cell::Empty;
        }
        // Leading zeros are significant in identifiers ("007"), and a leading
        // plus is not a spreadsheet number, so both stay text.
        let numeric = !value.starts_with('+')
            && !(value.starts_with('0') && value.len() > 1 && !value.starts_with("0."))
            && value.parse::<f64>().map(|n| n.is_finite()).unwrap_or(false);
        if numeric {
            Cell::Number(value.parse().unwrap_or(0.0))
        } else {
            Cell::Text(value.to_string())
        }
    }
}

fn cell_from_data(data: &Data) -> Cell {
    match data {
        Data::Empty => Cell::Empty,
        Data::String(s) => Cell::Text(s.clone()),
        Data::Float(f) => Cell::Number(*f),
        Data::Int(i) => Cell::Number(*i as f64),
        Data::Bool(b) => Cell::Bool(*b),
        Data::DateTime(dt) => Cell::Text(format_excel_serial(dt.as_f64())),
        Data::DateTimeIso(s) => Cell::Text(s.clone()),
        Data::DurationIso(s) => Cell::Text(s.clone()),
        Data::Error(e) => Cell::Text(format!("#ERROR({e:?})")),
    }
}

/// Visit every worksheet in order. At most one sheet is resident at a time, so
/// a workbook with hundreds of sheets does not multiply peak memory.
fn each_sheet(
    from: FormatId,
    path: &Path,
    mut visit: impl FnMut(&str, Vec<Vec<Cell>>) -> Result<(), JobError>,
) -> Result<(), JobError> {
    if from.0 == "CSV" || from.0 == "TSV" {
        let delimiter = if from.0 == "CSV" { b',' } else { b'\t' };
        let rows = read_delimited(path, delimiter)?;
        let name = path
            .file_stem()
            .and_then(|s| s.to_str())
            .unwrap_or("Sheet1")
            .to_string();
        return visit(&name, rows);
    }

    let mut workbook = open_workbook_auto(path).map_err(|e| spreadsheet_error(path, &e))?;
    let names = workbook.sheet_names().to_vec();
    if names.is_empty() {
        return Err(JobError::new(
            ErrorCode::EngineFailed,
            format!("{} contains no worksheets", path.display()),
        ));
    }
    for name in names {
        let range = workbook
            .worksheet_range(&name)
            .map_err(|e| spreadsheet_error(path, &e))?;
        let rows: Vec<Vec<Cell>> = range
            .rows()
            .map(|row| row.iter().map(cell_from_data).collect())
            .collect();
        visit(&name, rows)?;
    }
    Ok(())
}

fn spreadsheet_error(path: &Path, error: &impl std::fmt::Display) -> JobError {
    JobError::new(
        ErrorCode::EngineFailed,
        format!("reading {} as a spreadsheet: {error}", path.display()),
    )
}

fn write_workbook(from: FormatId, source: &Path, dest: &Path) -> Result<(), JobError> {
    let mut workbook = rust_xlsxwriter::Workbook::new();
    let mut used_names: Vec<String> = Vec::new();
    each_sheet(from, source, |name, rows| {
        let sheet_name = unique_sheet_name(name, &mut used_names);
        let sheet = workbook.add_worksheet();
        sheet.set_name(sheet_name.as_str()).map_err(xlsx_error)?;
        write_rows(sheet, &rows)
    })?;
    workbook.save(dest).map_err(xlsx_error)?;
    ensure_file(dest, "the spreadsheet writer")
}

fn write_rows(sheet: &mut rust_xlsxwriter::Worksheet, rows: &[Vec<Cell>]) -> Result<(), JobError> {
    for (r, cells) in rows.iter().enumerate() {
        let row = u32::try_from(r).map_err(|_| {
            JobError::new(
                ErrorCode::EngineFailed,
                "the sheet has too many rows to write",
            )
        })?;
        for (c, cell) in cells.iter().enumerate() {
            let col = u16::try_from(c).map_err(|_| {
                JobError::new(
                    ErrorCode::EngineFailed,
                    "the sheet has too many columns to write",
                )
            })?;
            match cell {
                Cell::Empty => {}
                Cell::Text(text) => {
                    sheet
                        .write_string(row, col, text.as_str())
                        .map_err(xlsx_error)?;
                }
                Cell::Number(n) if n.is_finite() => {
                    sheet.write_number(row, col, *n).map_err(xlsx_error)?;
                }
                Cell::Number(n) => {
                    sheet
                        .write_string(row, col, n.to_string())
                        .map_err(xlsx_error)?;
                }
                Cell::Bool(b) => {
                    sheet.write_boolean(row, col, *b).map_err(xlsx_error)?;
                }
            }
        }
    }
    Ok(())
}

fn xlsx_error(error: rust_xlsxwriter::XlsxError) -> JobError {
    JobError::new(
        ErrorCode::EngineFailed,
        format!("writing the workbook: {error}"),
    )
}

fn unique_sheet_name(raw: &str, used: &mut Vec<String>) -> String {
    let mut base: String = raw
        .chars()
        .map(|c| match c {
            '[' | ']' | ':' | '*' | '?' | '/' | '\\' => '_',
            c if c.is_control() => '_',
            c => c,
        })
        .collect();
    base = base.trim_matches('\'').trim().to_string();
    if base.is_empty() {
        base = "Sheet".to_string();
    }
    base = base.chars().take(31).collect();

    let mut candidate = base.clone();
    let mut suffix = 2;
    while used
        .iter()
        .any(|name| name.eq_ignore_ascii_case(&candidate))
    {
        let tail = format!("-{suffix}");
        let keep = 31usize.saturating_sub(tail.chars().count());
        candidate = format!("{}{}", base.chars().take(keep).collect::<String>(), tail);
        suffix += 1;
    }
    used.push(candidate.clone());
    candidate
}

fn write_delimited_sheets(
    job: &Job,
    cx: &Context<'_>,
    from: FormatId,
    ext: &str,
    delimiter: char,
) -> Result<Vec<PathBuf>, JobError> {
    let stem = stem_of(job);
    let mut outputs = Vec::new();
    let mut ordinal = 1usize;
    cx.ctx.set_stage("writing the spreadsheet");
    each_sheet(from, &job.source, |_name, rows| {
        cx.ctx.checkpoint()?;
        let file_name = if ordinal == 1 {
            format!("{stem}.{ext}")
        } else {
            format!("{stem}-{ordinal}.{ext}")
        };
        let dest = cx.out(&file_name);
        write_delimited(&dest, &rows, delimiter)?;
        ensure_file(&dest, "the spreadsheet writer")?;
        outputs.push(dest);
        ordinal += 1;
        cx.ctx.set_progress(0.9);
        Ok(())
    })?;
    cx.ctx.set_progress(1.0);
    Ok(outputs)
}

fn write_delimited(path: &Path, rows: &[Vec<Cell>], delimiter: char) -> Result<(), JobError> {
    let file = File::create(path).map_err(|e| JobError::io("creating the spreadsheet", &e))?;
    let mut writer = BufWriter::new(file);
    for row in rows {
        let mut line = String::new();
        for (index, cell) in row.iter().enumerate() {
            if index > 0 {
                line.push(delimiter);
            }
            push_field(&mut line, &cell_text(cell), delimiter);
        }
        line.push('\n');
        writer
            .write_all(line.as_bytes())
            .map_err(|e| JobError::io("writing the spreadsheet", &e))?;
    }
    writer
        .flush()
        .map_err(|e| JobError::io("writing the spreadsheet", &e))
}

fn cell_text(cell: &Cell) -> String {
    match cell {
        Cell::Empty => String::new(),
        Cell::Text(text) => text.clone(),
        Cell::Number(n) => format_number(*n),
        Cell::Bool(b) => if *b { "TRUE" } else { "FALSE" }.to_string(),
    }
}

fn format_number(value: f64) -> String {
    if value.fract() == 0.0 && value.abs() < 1e15 {
        format!("{}", value as i64)
    } else {
        format!("{value}")
    }
}

fn push_field(out: &mut String, value: &str, delimiter: char) {
    let quote = value.contains(delimiter)
        || value.contains('"')
        || value.contains('\n')
        || value.contains('\r');
    if quote {
        out.push('"');
        out.push_str(&value.replace('"', "\"\""));
        out.push('"');
    } else {
        out.push_str(value);
    }
}

/// RFC 4180 reader that streams over the file. Quoted fields may contain the
/// delimiter and newlines; a doubled quote inside a quoted field is a literal
/// quote. A leading UTF-8 BOM is discarded.
fn read_delimited(path: &Path, delimiter: u8) -> Result<Vec<Vec<Cell>>, JobError> {
    let file =
        File::open(path).map_err(|e| JobError::io(format!("opening {}", path.display()), &e))?;
    let mut reader = BufReader::new(file);
    {
        let head = reader
            .fill_buf()
            .map_err(|e| JobError::io("reading the spreadsheet", &e))?;
        if head.starts_with(&[0xEF, 0xBB, 0xBF]) {
            reader.consume(3);
        }
    }
    let mut rows: Vec<Vec<Cell>> = Vec::new();
    let mut row: Vec<Cell> = Vec::new();
    let mut field: Vec<u8> = Vec::new();
    let mut quoted = false;
    let mut quote_closed = false;
    let mut touched = false;

    for byte in reader.bytes() {
        let byte = byte.map_err(|e| JobError::io("reading the spreadsheet", &e))?;
        if quoted {
            if byte == b'"' {
                quoted = false;
                quote_closed = true;
            } else {
                field.push(byte);
            }
            continue;
        }
        // A doubled quote inside a quoted field is a literal quote.
        if byte == b'"' && quote_closed {
            field.push(b'"');
            quoted = true;
            quote_closed = false;
            touched = true;
            continue;
        }
        if byte == b'"' && field.is_empty() && !quote_closed {
            quoted = true;
            touched = true;
            continue;
        }
        match byte {
            b if b == delimiter => {
                row.push(Cell::infer(&decode_field(&field)));
                field.clear();
                quote_closed = false;
                touched = true;
            }
            b'\n' => {
                row.push(Cell::infer(&decode_field(&field)));
                field.clear();
                quote_closed = false;
                if touched {
                    rows.push(std::mem::take(&mut row));
                }
                row.clear();
                touched = false;
            }
            // CR of a CRLF pair; a lone CR inside a field is not valid RFC 4180.
            b'\r' => {}
            b => {
                field.push(b);
                touched = true;
            }
        }
    }
    if touched || !field.is_empty() || !row.is_empty() {
        row.push(Cell::infer(&decode_field(&field)));
        rows.push(row);
    }
    Ok(rows)
}

fn decode_field(bytes: &[u8]) -> String {
    String::from_utf8_lossy(bytes).into_owned()
}

fn format_excel_serial(serial: f64) -> String {
    if !serial.is_finite() || serial < 0.0 {
        return format_number(serial);
    }
    let days = serial.floor() as i64;
    let mut seconds = ((serial - serial.floor()) * 86_400.0).round() as i64;
    let mut unix_days = days - 25_569; // 1970-01-01 is Excel serial 25569
    if seconds >= 86_400 {
        seconds -= 86_400;
        unix_days += 1;
    }
    let (year, month, day) = civil_from_days(unix_days);
    let (hour, minute, second) = (seconds / 3600, (seconds % 3600) / 60, seconds % 60);
    if seconds == 0 {
        format!("{year:04}-{month:02}-{day:02}")
    } else {
        format!("{year:04}-{month:02}-{day:02} {hour:02}:{minute:02}:{second:02}")
    }
}

/// Howard Hinnant's civil-from-days, days counted from 1970-01-01.
fn civil_from_days(days: i64) -> (i64, u32, u32) {
    let z = days + 719_468;
    let era = if z >= 0 { z } else { z - 146_096 } / 146_097;
    let doe = (z - era * 146_097) as u64;
    let yoe = (doe - doe / 1_460 + doe / 36_524 - doe / 146_096) / 365;
    let year = yoe as i64 + era * 400;
    let doy = doe - (365 * yoe + yoe / 4 - yoe / 100);
    let mp = (5 * doy + 2) / 153;
    let day = (doy - (153 * mp + 2) / 5 + 1) as u32;
    let month = if mp < 10 {
        (mp + 3) as u32
    } else {
        (mp - 9) as u32
    };
    (if month <= 2 { year + 1 } else { year }, month, day)
}

// ------------------------------------------------------------------ LibreOffice

/// Run one `--convert-to` and return the file it wrote. Every invocation gets its
/// own user profile under the work dir, so parallel jobs cannot collide.
fn soffice_convert(
    cx: &Context<'_>,
    input: &Path,
    outdir: &Path,
    filter: &str,
    out_ext: &str,
) -> Result<PathBuf, JobError> {
    fs::create_dir_all(outdir)
        .map_err(|e| JobError::io("creating the LibreOffice output directory", &e))?;
    let mut invocation = cx.tools.soffice_invocation()?.into_iter();
    let program = invocation
        .next()
        .ok_or_else(|| JobError::missing_tool("LibreOffice"))?;
    let profile = file_url(&cx.scratch("lo-profile"))?;
    let cmd = Cmd::new(program)
        .args(invocation)
        .arg(format!("-env:UserInstallation={profile}"))
        .arg("--headless")
        .arg("--norestore")
        .arg("--nofirststartwizard")
        .arg("--convert-to")
        .arg(filter)
        .arg("--outdir")
        .arg(outdir)
        .arg(input)
        .budget(Budget::Long);
    cx.ctx.set_stage("converting with LibreOffice");
    proc::run(cmd, cx.ctx)?.ok()?;

    let stem = input
        .file_stem()
        .and_then(|s| s.to_str())
        .unwrap_or("input");
    let produced = outdir.join(format!("{stem}.{out_ext}"));
    ensure_file(&produced, "LibreOffice")?;
    Ok(produced)
}

fn writer_filter(target_ext: &str) -> Result<&'static str, JobError> {
    match target_ext {
        "pdf" => Ok("pdf:writer_pdf_Export"),
        "odt" => Ok("odt:writer8"),
        "docx" => Ok("docx:MS Word 2007 XML"),
        "jpg" => Ok("jpg:writer_jpg_Export"),
        "png" => Ok("png:writer_png_Export"),
        other => Err(JobError::new(
            ErrorCode::Unsupported,
            format!("LibreOffice cannot write {other} from a text document"),
        )),
    }
}

fn impress_filter(target_ext: &str) -> Result<&'static str, JobError> {
    match target_ext {
        "pdf" => Ok("pdf:impress_pdf_Export"),
        "jpg" => Ok("jpg:impress_jpg_Export"),
        "png" => Ok("png:impress_png_Export"),
        other => Err(JobError::new(
            ErrorCode::Unsupported,
            format!("LibreOffice cannot write {other} from a presentation"),
        )),
    }
}

fn calc_filter(target_ext: &str) -> Result<&'static str, JobError> {
    match target_ext {
        "pdf" => Ok("pdf:calc_pdf_Export"),
        "ods" => Ok("ods:calc8"),
        "xlsx" => Ok("xlsx:Calc MS Excel 2007 XML"),
        other => Err(JobError::new(
            ErrorCode::Unsupported,
            format!("LibreOffice cannot write {other} from a spreadsheet"),
        )),
    }
}

/// A `file:` URL LibreOffice accepts for `-env:UserInstallation`; every byte
/// outside the unreserved set is percent-encoded, so spaces and emoji survive.
fn file_url(path: &Path) -> Result<String, JobError> {
    let absolute = std::path::absolute(path)
        .map_err(|e| JobError::io("resolving the LibreOffice profile path", &e))?;
    let text = absolute.to_string_lossy().replace('\\', "/");
    let mut url = String::from("file://");
    if !text.starts_with('/') {
        url.push('/');
    }
    for byte in text.bytes() {
        match byte {
            b'A'..=b'Z' | b'a'..=b'z' | b'0'..=b'9' | b'-' | b'.' | b'_' | b'~' | b'/' | b':' => {
                url.push(byte as char);
            }
            other => url.push_str(&format!("%{other:02X}")),
        }
    }
    Ok(url)
}

// --------------------------------------------------------------------- helpers

fn input_format(job: &Job) -> Result<FormatId, JobError> {
    job.input_format
        .or_else(|| Registry::detect(&job.source).map(|f| f.id))
        .ok_or_else(|| JobError::new(ErrorCode::Unsupported, "unknown input format"))
}

fn stem_of(job: &Job) -> String {
    let stem = job
        .source
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    if stem.is_empty() {
        "output".to_string()
    } else {
        stem
    }
}

fn out_dir(cx: &Context<'_>) -> PathBuf {
    cx.scratch("soffice-out")
}

/// Copy the source into the work dir under a predictable name, so the output
/// LibreOffice writes can be found without guessing at a hostile filename.
fn stage_source(job: &Job, cx: &Context<'_>, from: FormatId) -> Result<PathBuf, JobError> {
    let dir = cx.scratch("office-in");
    fs::create_dir_all(&dir).map_err(|e| JobError::io("creating the scratch directory", &e))?;
    let staged = dir.join(format!("input.{}", ext_of(from)));
    fs::copy(&job.source, &staged)
        .map_err(|e| JobError::io(format!("staging {}", job.source.display()), &e))?;
    Ok(staged)
}

fn publish(
    job: &Job,
    cx: &Context<'_>,
    produced: &Path,
    ext: &str,
    producer: &str,
) -> Result<PathBuf, JobError> {
    let dest = cx.out(&format!("{}.{ext}", stem_of(job)));
    move_file(produced, &dest)?;
    ensure_file(&dest, producer)?;
    Ok(dest)
}

fn move_file(from: &Path, to: &Path) -> Result<(), JobError> {
    match fs::rename(from, to) {
        Ok(()) => Ok(()),
        Err(_) => {
            fs::copy(from, to)
                .map_err(|e| JobError::io(format!("moving the result to {}", to.display()), &e))?;
            fs::remove_file(from)
                .map_err(|e| JobError::io("removing the intermediate file", &e))?;
            Ok(())
        }
    }
}

fn pdf_page_count(path: &Path) -> Result<usize, JobError> {
    let document = lopdf::Document::load(path).map_err(|e| {
        JobError::new(
            ErrorCode::EngineFailed,
            format!("reading the generated PDF: {e}"),
        )
    })?;
    Ok(document.get_pages().len())
}

/// Render every page at 150 DPI into its own artifact, numbered in page order.
fn render_pdf_pages(
    cx: &Context<'_>,
    pdf: &Path,
    ext: &str,
    stem: &str,
) -> Result<Vec<PathBuf>, JobError> {
    let pdftoppm = cx.tools.path(ToolKey::Pdftoppm).map_err(|_| {
        JobError::new(
            ErrorCode::MissingTool,
            "Poppler (pdftoppm) is required to save every page as a separate image",
        )
    })?;
    let dir = cx.scratch("pages");
    if dir.exists() {
        fs::remove_dir_all(&dir)
            .map_err(|e| JobError::io("clearing the page scratch directory", &e))?;
    }
    fs::create_dir_all(&dir).map_err(|e| JobError::io("creating the page directory", &e))?;

    let mode = if ext == "png" { "-png" } else { "-jpeg" };
    let prefix = dir.join("page");
    cx.ctx.set_stage("rendering the pages");
    let cmd = Cmd::new(&pdftoppm)
        .args(["-r", "150", mode])
        .arg(pdf)
        .arg(&prefix)
        .budget(Budget::Long);
    proc::run(cmd, cx.ctx)?.ok()?;

    let wanted_ext = if ext == "png" { "png" } else { "jpg" };
    let mut files: Vec<PathBuf> = fs::read_dir(&dir)
        .map_err(|e| JobError::io("listing the rendered pages", &e))?
        .filter_map(|entry| entry.ok().map(|e| e.path()))
        .filter(|path| path.extension().and_then(|e| e.to_str()) == Some(wanted_ext))
        .collect();
    files.sort_by_key(|path| page_number(path));
    if files.is_empty() {
        return Err(JobError::new(
            ErrorCode::EngineFailed,
            "Poppler produced no page images",
        ));
    }

    let mut outputs = Vec::with_capacity(files.len());
    for (index, file) in files.iter().enumerate() {
        let dest = cx.out(&format!("{stem}-{}.{ext}", index + 1));
        move_file(file, &dest)?;
        ensure_file(&dest, "Poppler")?;
        outputs.push(dest);
        cx.ctx
            .set_progress(0.5 + 0.5 * ((index + 1) as f64 / files.len() as f64));
    }
    Ok(outputs)
}

fn page_number(path: &Path) -> u32 {
    path.file_stem()
        .and_then(|s| s.to_str())
        .and_then(|s| s.rsplit('-').next())
        .and_then(|n| n.parse().ok())
        .unwrap_or(0)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Toolset;
    use convertme_core::job::{JobCtx, WorkDir};

    struct Fixture {
        dir: tempfile::TempDir,
        work: WorkDir,
        ctx: JobCtx,
        tools: Toolset,
    }

    impl Fixture {
        fn new() -> Self {
            let dir = tempfile::tempdir().expect("temp dir");
            let work = WorkDir::create(dir.path().join("work")).expect("work dir");
            Self {
                dir,
                work,
                ctx: JobCtx::new(),
                tools: Toolset::probe_now(),
            }
        }

        fn context(&self) -> Context<'_> {
            Context {
                tools: &self.tools,
                work: &self.work,
                ctx: &self.ctx,
            }
        }

        fn source(&self, name: &str, content: &[u8]) -> PathBuf {
            let path = self.dir.path().join(name);
            std::fs::write(&path, content).expect("fixture");
            path
        }
    }

    fn job(source: PathBuf, target: &str) -> Job {
        Job::new(source, Intent::convert(target).expect("known target"))
    }

    #[test]
    fn markdown_to_html_contains_a_heading() {
        let fixture = Fixture::new();
        if !fixture.tools.has(ToolKey::Pandoc) {
            return;
        }
        let source = fixture.source("notes.md", b"# Title\n\nBody text.\n");
        let cx = fixture.context();
        let out = run(&job(source, "HTML"), &cx).expect("pandoc html");
        assert_eq!(out.len(), 1);
        let html = std::fs::read_to_string(&out[0]).expect("html");
        assert!(html.contains("<h1"), "{html}");
    }

    #[test]
    fn markdown_to_docx_is_a_zip_with_document_xml() {
        let fixture = Fixture::new();
        if !fixture.tools.has(ToolKey::Pandoc) {
            return;
        }
        let source = fixture.source("notes.md", b"# Title\n\nBody text.\n");
        let cx = fixture.context();
        let out = run(&job(source, "DOCX"), &cx).expect("pandoc docx");
        let bytes = std::fs::read(&out[0]).expect("docx");
        assert_eq!(&bytes[..2], b"PK", "DOCX must be a zip container");
        assert!(
            bytes
                .windows(b"word/document.xml".len())
                .any(|w| w == b"word/document.xml"),
            "DOCX must carry word/document.xml"
        );
    }

    #[test]
    fn text_to_epub_is_a_zip_with_container_xml() {
        let fixture = Fixture::new();
        if !fixture.tools.has(ToolKey::Pandoc) {
            return;
        }
        let source = fixture.source("story.txt", b"Once upon a time.\n");
        let cx = fixture.context();
        let out = run(&job(source, "EPUB"), &cx).expect("pandoc epub");
        let bytes = std::fs::read(&out[0]).expect("epub");
        assert_eq!(&bytes[..2], b"PK");
        assert!(
            bytes
                .windows(b"META-INF/container.xml".len())
                .any(|w| w == b"META-INF/container.xml"),
            "EPUB must carry META-INF/container.xml"
        );
    }

    #[test]
    fn csv_to_tsv_roundtrip_keeps_three_columns() {
        let fixture = Fixture::new();
        let source = fixture.source("table.csv", b"a,b,c\n1,2,3\n");
        let cx = fixture.context();
        let tsv = run(&job(source, "TSV"), &cx).expect("csv to tsv");
        assert_eq!(tsv.len(), 1);
        let text = std::fs::read_to_string(&tsv[0]).expect("tsv");
        let first: Vec<&str> = text.lines().next().unwrap().split('\t').collect();
        assert_eq!(first, ["a", "b", "c"]);

        let back = run(&job(tsv[0].clone(), "CSV"), &cx).expect("tsv to csv");
        let rows = read_delimited(&back[0], b',').expect("csv parse");
        assert_eq!(rows.len(), 2);
        assert_eq!(rows[0].len(), 3);
        assert_eq!(rows[1].len(), 3);
        assert_eq!(cell_text(&rows[1][2]), "3");
    }

    #[test]
    fn csv_to_tsv_quotes_embedded_delimiters() {
        let fixture = Fixture::new();
        let source = fixture.source("quoted.csv", b"name,note\nAda,\"one, two\"\n");
        let cx = fixture.context();
        let tsv = run(&job(source, "TSV"), &cx).expect("csv to tsv");
        let text = std::fs::read_to_string(&tsv[0]).expect("tsv");
        assert!(text.contains("one, two"), "{text}");
    }

    #[test]
    fn xlsx_roundtrips_cell_values_through_calamine() {
        let fixture = Fixture::new();
        let source = fixture.source("cells.csv", b"name,count\nwidget,42\n");
        let cx = fixture.context();
        let out = run(&job(source, "XLSX"), &cx).expect("csv to xlsx");

        let mut workbook = open_workbook_auto(&out[0]).expect("calamine opens what we wrote");
        let names = workbook.sheet_names().to_vec();
        let range = workbook.worksheet_range(&names[0]).expect("first sheet");
        let rows: Vec<Vec<Data>> = range.rows().map(|row| row.to_vec()).collect();
        assert_eq!(rows.len(), 2);
        match &rows[0][0] {
            Data::String(text) => assert_eq!(text, "name"),
            other => panic!("expected a string header, got {other:?}"),
        }
        match &rows[1][1] {
            Data::Float(value) => assert!((*value - 42.0).abs() < 1e-9),
            Data::Int(value) => assert_eq!(*value, 42),
            other => panic!("expected a number, got {other:?}"),
        }
    }

    #[test]
    fn srt_to_vtt_and_back_preserves_cues() {
        let fixture = Fixture::new();
        let srt =
            b"1\n00:00:01,000 --> 00:00:02,500\nHello\n\n2\n00:00:03,000 --> 00:00:04,000\nWorld\n";
        let source = fixture.source("captions.srt", srt);
        let cx = fixture.context();
        let vtt = subtitle(&job(source, "VTT"), &cx).expect("srt to vtt");
        let body = std::fs::read_to_string(&vtt[0]).expect("vtt");
        assert!(body.starts_with("WEBVTT"));
        assert!(body.contains("00:00:01.000 --> 00:00:02.500"));

        let back = subtitle(&job(vtt[0].clone(), "SRT"), &cx).expect("vtt to srt");
        let body = std::fs::read_to_string(&back[0]).expect("srt");
        assert!(body.contains("00:00:01,000 --> 00:00:02,500"));
        assert!(body.contains("Hello"));
        assert_eq!(body.matches("-->").count(), 2);
    }

    #[test]
    fn docx_to_pdf_runs_through_libreoffice() {
        let fixture = Fixture::new();
        if !fixture.tools.has(ToolKey::Pandoc) || !fixture.tools.has(ToolKey::Soffice) {
            return;
        }
        let source = fixture.source("brief.md", b"# Brief\n\nOne paragraph.\n");
        let cx = fixture.context();
        let docx = run(&job(source, "DOCX"), &cx).expect("pandoc docx");
        let pdf = run(&job(docx[0].clone(), "PDF"), &cx).expect("soffice pdf");
        let bytes = std::fs::read(&pdf[0]).expect("pdf");
        assert!(bytes.starts_with(b"%PDF"), "output must be a PDF");
    }

    #[test]
    fn sheet_names_are_unique_and_xlsx_legal() {
        let mut used = Vec::new();
        assert_eq!(unique_sheet_name("Sheet1", &mut used), "Sheet1");
        assert_eq!(unique_sheet_name("Sheet1", &mut used), "Sheet1-2");
        let long = "x".repeat(80);
        let name = unique_sheet_name(&long, &mut used);
        assert!(name.chars().count() <= 31);
        assert_eq!(unique_sheet_name("a/b:c", &mut used), "a_b_c");
    }
}
