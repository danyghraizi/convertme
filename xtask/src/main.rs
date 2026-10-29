//! Build tasks. `cargo xtask <task>`.
//!
//! * `gen-conf`   – regenerate `crates/convertme-gui/tauri.conf.json`
//! * `snapshot`   – rewrite the registry snapshot used by the planner test
//! * `fixtures`   – (re)generate one tiny file per input format
//! * `vendor`     – copy the bundled tools into `vendor/bin/<triple>`
//! * `debrand`    – run the identity gate over the tree and the artifacts
//! * `perf`       – measure the budgets from the plan and fail when they regress
//! * `release`    – gate, build, measure the bundle

use std::path::Path;
use std::process::{Command, ExitCode};
use std::time::{Duration, Instant};

// The single implementation of the Tauri config generator. It is included, not
// duplicated, so the app id in the bundle cannot drift from `identity.rs`.
include!("../../scripts/gen_tauri_conf.rs");

fn repo_root() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR"))
        .parent()
        .map(Path::to_path_buf)
        .expect("xtask lives one level below the workspace root")
}

fn main() -> ExitCode {
    let args: Vec<String> = std::env::args().skip(1).collect();
    let task = args.first().map(String::as_str).unwrap_or("help");
    let rest = if args.len() > 1 { &args[1..] } else { &[][..] };
    let result = match task {
        "gen-conf" => gen_conf(),
        "snapshot" => snapshot(),
        "fixtures" => fixtures(),
        "vendor" => vendor(),
        "debrand" => debrand(),
        "perf" => perf(rest),
        "release" => release(),
        "help" | "--help" | "-h" => {
            println!(
                "cargo xtask <gen-conf|snapshot|fixtures|vendor|debrand|perf|release>\n\
                 perf: cargo xtask perf <wheel|start|idle> [--runs N] [--seconds N]"
            );
            Ok(())
        }
        other => Err(format!("unknown task {other:?}")),
    };
    match result {
        Ok(()) => ExitCode::SUCCESS,
        Err(e) => {
            eprintln!("xtask: {e}");
            ExitCode::FAILURE
        }
    }
}

fn gen_conf() -> Result<(), String> {
    let root = repo_root();
    let manifest_dir = root.join("crates/convertme-gui");
    let path = manifest_dir.join("tauri.conf.json");
    std::fs::write(&path, tauri_conf_json()).map_err(|e| format!("writing {path:?}: {e}"))?;
    println!("wrote {}", path.display());
    Ok(())
}

fn snapshot() -> Result<(), String> {
    use convertme_core::formats::{Category, FORMATS, PAIR_OVERRIDES, Registry, TOOLS};
    let root = repo_root();
    let path = root.join("crates/convertme-core/tests/formats.snapshot.json");

    let formats: Vec<serde_json::Value> = FORMATS
        .iter()
        .map(|f| {
            serde_json::json!({
                "id": f.id.0,
                "label": f.label,
                "category": f.category.key(),
                "mime": f.mime,
                "exts": f.exts,
                "read": f.read,
                "write": f.write,
                "wheels": f.wheels.iter().map(|c| c.key()).collect::<Vec<_>>(),
            })
        })
        .collect();
    let overrides: Vec<serde_json::Value> = PAIR_OVERRIDES
        .iter()
        .map(|(from, to, engine)| serde_json::json!([from.0, to.0, format!("{engine:?}")]))
        .collect();
    let tools: Vec<serde_json::Value> = TOOLS
        .iter()
        .map(|t| {
            serde_json::json!({
                "id": t.id.as_str(),
                "category": t.category.key(),
                "label": t.label,
            })
        })
        .collect();
    let wheels: serde_json::Value = Category::ALL
        .into_iter()
        .map(|c| {
            (
                c.key().to_string(),
                serde_json::json!(
                    Registry::wheel_targets(c, None)
                        .iter()
                        .map(|f| f.id.0)
                        .collect::<Vec<_>>()
                ),
            )
        })
        .collect::<serde_json::Map<_, _>>()
        .into();

    let snapshot = serde_json::json!({
        "formats": formats,
        "overrides": overrides,
        "tools": tools,
        "wheels": wheels,
        "pairs": Registry::pairs().count(),
    });
    std::fs::create_dir_all(path.parent().unwrap()).map_err(|e| e.to_string())?;
    std::fs::write(
        &path,
        serde_json::to_string_pretty(&snapshot).map_err(|e| e.to_string())?,
    )
    .map_err(|e| format!("writing {path:?}: {e}"))?;
    println!("wrote {}", path.display());
    Ok(())
}

// ---------------------------------------------------------------------------
// Fixtures
// ---------------------------------------------------------------------------

fn have(binary: &str) -> bool {
    convertme_engines_probe(binary)
}

fn convertme_engines_probe(binary: &str) -> bool {
    let path = std::env::var_os("PATH").unwrap_or_default();
    std::env::split_paths(&path).any(|dir| {
        let candidate = dir.join(if cfg!(windows) {
            format!("{binary}.exe")
        } else {
            binary.to_string()
        });
        candidate.is_file()
    })
}

fn run_tool(program: &str, args: &[&str]) -> Result<(), String> {
    let status = Command::new(program)
        .args(args)
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::piped())
        .output()
        .map_err(|e| format!("running {program}: {e}"))?;
    if !status.status.success() {
        return Err(format!(
            "{program} failed: {}",
            String::from_utf8_lossy(&status.stderr).trim()
        ));
    }
    Ok(())
}

fn fixtures() -> Result<(), String> {
    let root = repo_root();
    let dir = root.join("fixtures");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;
    let path = |name: &str| dir.join(name);
    let mut missing: Vec<&str> = Vec::new();

    // Images: one small photo and one flat logo-ish image.
    if have("magick") {
        run_tool(
            "magick",
            &[
                "-size",
                "1200x800",
                "gradient:#2ea8a0-#12161b",
                "-quality",
                "88",
                path("photo.jpg").to_str().unwrap(),
            ],
        )?;
        run_tool(
            "magick",
            &[
                path("photo.jpg").to_str().unwrap(),
                path("photo.png").to_str().unwrap(),
            ],
        )?;
        run_tool(
            "magick",
            &[
                path("photo.jpg").to_str().unwrap(),
                path("photo.webp").to_str().unwrap(),
            ],
        )?;
        run_tool(
            "magick",
            &[
                path("photo.jpg").to_str().unwrap(),
                path("photo.tiff").to_str().unwrap(),
            ],
        )?;
        run_tool(
            "magick",
            &[
                path("photo.jpg").to_str().unwrap(),
                path("photo.bmp").to_str().unwrap(),
            ],
        )?;
        for (name, supported) in [("photo.avif", "AVIF"), ("photo.heic", "HEIC")] {
            if run_tool(
                "magick",
                &[
                    path("photo.jpg").to_str().unwrap(),
                    path(name).to_str().unwrap(),
                ],
            )
            .is_err()
            {
                missing.push(supported);
            }
        }
        run_tool(
            "magick",
            &["-size", "8x8", "xc:red", path("tiny.png").to_str().unwrap()],
        )?;
    } else {
        missing.extend(["JPG", "PNG", "WEBP", "TIFF", "BMP", "AVIF", "HEIC"]);
    }
    std::fs::write(
        path("drawing.svg"),
        br##"<svg xmlns="http://www.w3.org/2000/svg" width="64" height="64"><rect width="64" height="64" fill="#2ea8a0"/></svg>"##,
    )
    .map_err(|e| e.to_string())?;
    // A 0-byte file and a hostile-name set for the adversarial sweep.
    std::fs::write(path("empty.pdf"), b"").map_err(|e| e.to_string())?;
    let hostile = dir.join("hostile");
    std::fs::create_dir_all(&hostile).map_err(|e| e.to_string())?;
    let hostile_names = [
        "sp ace.png",
        "q\"uote.png",
        "line\nbreak.png",
        "émoji🎬.png",
        &format!("{}.png", "x".repeat(200)),
    ];
    for name in hostile_names {
        std::fs::copy(path("tiny.png"), hostile.join(name)).map_err(|e| e.to_string())?;
    }
    std::fs::write(hostile.join("empty.pdf"), b"").map_err(|e| e.to_string())?;

    // Video and audio.
    if have("ffmpeg") {
        let clip = path("clip.mp4");
        run_tool(
            "ffmpeg",
            &[
                "-y",
                "-f",
                "lavfi",
                "-i",
                "testsrc=size=320x240:rate=24:duration=2",
                "-f",
                "lavfi",
                "-i",
                "sine=frequency=440:duration=2",
                "-shortest",
                "-c:v",
                "libx264",
                "-preset",
                "ultrafast",
                "-c:a",
                "aac",
                clip.to_str().unwrap(),
            ],
        )?;
        for (name, vcodec, acodec) in [
            ("clip.mkv", "libx264", "aac"),
            ("clip.mov", "libx264", "aac"),
            ("clip.webm", "libvpx-vp9", "libopus"),
            ("clip.avi", "mpeg4", "mp3"),
            ("clip.wmv", "wmv2", "wmav2"),
        ] {
            let out = path(name);
            run_tool(
                "ffmpeg",
                &[
                    "-y",
                    "-i",
                    clip.to_str().unwrap(),
                    "-c:v",
                    vcodec,
                    "-c:a",
                    acodec,
                    out.to_str().unwrap(),
                ],
            )
            .map_err(|e| format!("{name}: {e}"))?;
        }
        run_tool(
            "ffmpeg",
            &[
                "-y",
                "-i",
                clip.to_str().unwrap(),
                "-vf",
                "fps=10,scale=160:-2",
                path("anim.gif").to_str().unwrap(),
            ],
        )?;
        for (name, codec) in [
            ("tone.mp3", "libmp3lame"),
            ("tone.m4a", "aac"),
            ("tone.wav", "pcm_s16le"),
            ("tone.flac", "flac"),
            ("tone.ogg", "libvorbis"),
            ("tone.opus", "libopus"),
            ("tone.aiff", "pcm_s16be"),
            ("tone.wma", "wmav2"),
        ] {
            run_tool(
                "ffmpeg",
                &[
                    "-y",
                    "-f",
                    "lavfi",
                    "-i",
                    "sine=frequency=440:duration=1",
                    "-c:a",
                    codec,
                    path(name).to_str().unwrap(),
                ],
            )
            .map_err(|e| format!("{name}: {e}"))?;
        }
    } else {
        missing.extend([
            "MP4", "MOV", "MKV", "WEBM", "AVI", "WMV", "GIF", "MP3", "M4A", "WAV", "FLAC", "OGG",
            "OPUS", "AIFF", "WMA",
        ]);
    }

    // A real PDF with selectable text, written by hand so fixtures need no tool.
    std::fs::write(
        path("notes.pdf"),
        minimal_pdf(&["ConvertMe fixtures", "Second line of selectable text"]),
    )
    .map_err(|e| e.to_string())?;

    // Text-shaped documents.
    let markdown =
        "# ConvertMe fixtures\n\nA paragraph with **bold** text and a list:\n\n- one\n- two\n";
    std::fs::write(path("notes.md"), markdown).map_err(|e| e.to_string())?;
    std::fs::write(
        path("notes.txt"),
        "ConvertMe fixtures\nA paragraph of plain text.\n",
    )
    .map_err(|e| e.to_string())?;
    std::fs::write(
        path("notes.html"),
        "<!doctype html><title>ConvertMe fixtures</title><h1>ConvertMe fixtures</h1><p>Text.</p>\n",
    )
    .map_err(|e| e.to_string())?;
    if have("pandoc") {
        for (name, target) in [
            ("notes.docx", "docx"),
            ("notes.odt", "odt"),
            ("notes.epub", "epub3"),
        ] {
            run_tool(
                "pandoc",
                &[
                    path("notes.md").to_str().unwrap(),
                    "-t",
                    target,
                    "-o",
                    path(name).to_str().unwrap(),
                ],
            )
            .map_err(|e| format!("{name}: {e}"))?;
        }
    } else {
        missing.extend(["DOCX", "ODT", "EPUB"]);
    }

    // Spreadsheets and subtitles are text or minimal containers.
    std::fs::write(
        path("sheet.csv"),
        "name,value,note\nalpha,1,first\nbeta,2,second\n",
    )
    .map_err(|e| e.to_string())?;
    std::fs::write(
        path("sheet.tsv"),
        "name\tvalue\tnote\nalpha\t1\tfirst\nbeta\t2\tsecond\n",
    )
    .map_err(|e| e.to_string())?;
    std::fs::write(path("cues.srt"), "1\n00:00:01,000 --> 00:00:03,000\nHello there\n\n2\n00:00:03,500 --> 00:00:05,000\nSecond cue\n").map_err(|e| e.to_string())?;
    std::fs::write(path("cues.vtt"), "WEBVTT\n\n00:00:01.000 --> 00:00:03.000\nHello there\n\n00:00:03.500 --> 00:00:05.000\nSecond cue\n").map_err(|e| e.to_string())?;

    // Office containers that only need a tiny zip.
    write_minimal_xlsx(&path("sheet.xlsx")).map_err(|e| format!("sheet.xlsx: {e}"))?;
    write_minimal_pptx(&path("deck.pptx")).map_err(|e| format!("deck.pptx: {e}"))?;
    write_minimal_ods(&path("sheet.ods")).map_err(|e| format!("sheet.ods: {e}"))?;

    // Archives.
    if have("7zz") {
        let src = dir.join("archive-src");
        std::fs::create_dir_all(src.join("nested")).map_err(|e| e.to_string())?;
        std::fs::write(src.join("a.txt"), b"a").map_err(|e| e.to_string())?;
        std::fs::write(src.join("nested/b.txt"), b"b").map_err(|e| e.to_string())?;
        run_tool(
            "7zz",
            &[
                "a",
                "-tzip",
                path("bundle.zip").to_str().unwrap(),
                src.join("a.txt").to_str().unwrap(),
                src.join("nested").to_str().unwrap(),
            ],
        )?;
        run_tool(
            "7zz",
            &[
                "a",
                "-ttar",
                path("bundle.tar").to_str().unwrap(),
                src.join("a.txt").to_str().unwrap(),
            ],
        )?;
        run_tool(
            "7zz",
            &[
                "a",
                "-tgzip",
                path("bundle.gz").to_str().unwrap(),
                src.join("a.txt").to_str().unwrap(),
            ],
        )?;
    } else {
        missing.extend(["ZIP", "TAR", "GZIP"]);
    }

    // Legacy formats with no open encoder on this platform.
    missing.extend(["PPT", "XLS", "RAR"]);

    println!("fixtures in {}", dir.display());
    if !missing.is_empty() {
        println!(
            "not generated (no encoder on this host): {}",
            missing.join(", ")
        );
    }
    Ok(())
}

/// Smallest useful multi-page-less PDF with real text.
fn minimal_pdf(lines: &[&str]) -> Vec<u8> {
    let mut content = String::from("BT /F1 18 Tf 72 720 Td 20 TL\n");
    for line in lines {
        let escaped = line
            .replace('\\', r"\\")
            .replace('(', r"\(")
            .replace(')', r"\)");
        content.push_str(&format!("({escaped}) Tj T*\n"));
    }
    content.push_str("ET\n");

    let objects = [
        "<< /Type /Catalog /Pages 2 0 R >>".to_string(),
        "<< /Type /Pages /Kids [3 0 R] /Count 1 >>".to_string(),
        "<< /Type /Page /Parent 2 0 R /MediaBox [0 0 595 842] /Resources << /Font << /F1 4 0 R >> >> /Contents 5 0 R >>".to_string(),
        "<< /Type /Font /Subtype /Type1 /BaseFont /Helvetica >>".to_string(),
        format!("<< /Length {} >>\nstream\n{content}endstream", content.len()),
    ];

    let mut out = String::from("%PDF-1.4\n");
    let mut offsets = Vec::new();
    for (i, obj) in objects.iter().enumerate() {
        offsets.push(out.len());
        out.push_str(&format!("{} 0 obj\n{obj}\nendobj\n", i + 1));
    }
    let xref_at = out.len();
    out.push_str(&format!(
        "xref\n0 {}\n0000000000 65535 f \n",
        objects.len() + 1
    ));
    for offset in &offsets {
        out.push_str(&format!("{offset:010} 00000 n \n"));
    }
    out.push_str(&format!(
        "trailer\n<< /Size {} /Root 1 0 R >>\nstartxref\n{xref_at}\n%%EOF\n",
        objects.len() + 1
    ));
    out.into_bytes()
}

fn write_zip(path: &Path, entries: &[(&str, String)]) -> Result<(), String> {
    use std::io::Write;
    let file = std::fs::File::create(path).map_err(|e| e.to_string())?;
    let mut zip = zip::ZipWriter::new(file);
    let options: zip::write::SimpleFileOptions = zip::write::SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated);
    for (name, body) in entries {
        zip.start_file(*name, options).map_err(|e| e.to_string())?;
        zip.write_all(body.as_bytes()).map_err(|e| e.to_string())?;
    }
    zip.finish().map_err(|e| e.to_string())?;
    Ok(())
}

fn write_minimal_xlsx(path: &Path) -> Result<(), String> {
    let entries = [
        ("[Content_Types].xml", r#"<?xml version="1.0" encoding="UTF-8"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/xl/workbook.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.sheet.main+xml"/><Override PartName="/xl/worksheets/sheet1.xml" ContentType="application/vnd.openxmlformats-officedocument.spreadsheetml.worksheet+xml"/></Types>"#.to_string()),
        ("_rels/.rels", r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="xl/workbook.xml"/></Relationships>"#.to_string()),
        ("xl/workbook.xml", r#"<?xml version="1.0" encoding="UTF-8"?><workbook xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><sheets><sheet name="Sheet1" sheetId="1" r:id="rId1"/></sheets></workbook>"#.to_string()),
        ("xl/_rels/workbook.xml.rels", r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/worksheet" Target="worksheets/sheet1.xml"/></Relationships>"#.to_string()),
        ("xl/worksheets/sheet1.xml", r#"<?xml version="1.0" encoding="UTF-8"?><worksheet xmlns="http://schemas.openxmlformats.org/spreadsheetml/2006/main"><sheetData><row r="1"><c r="A1" t="inlineStr"><is><t>name</t></is></c><c r="B1" t="inlineStr"><is><t>value</t></is></c></row><row r="2"><c r="A2" t="inlineStr"><is><t>alpha</t></is></c><c r="B2"><v>1</v></c></row><row r="3"><c r="A3" t="inlineStr"><is><t>beta</t></is></c><c r="B3"><v>2</v></c></row></sheetData></worksheet>"#.to_string()),
    ];
    write_zip(path, &entries)
}

fn write_minimal_pptx(path: &Path) -> Result<(), String> {
    let entries = [
        ("[Content_Types].xml", r#"<?xml version="1.0" encoding="UTF-8"?><Types xmlns="http://schemas.openxmlformats.org/package/2006/content-types"><Default Extension="rels" ContentType="application/vnd.openxmlformats-package.relationships+xml"/><Default Extension="xml" ContentType="application/xml"/><Override PartName="/ppt/presentation.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.presentation.main+xml"/><Override PartName="/ppt/slides/slide1.xml" ContentType="application/vnd.openxmlformats-officedocument.presentationml.slide+xml"/></Types>"#.to_string()),
        ("_rels/.rels", r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/officeDocument" Target="ppt/presentation.xml"/></Relationships>"#.to_string()),
        ("ppt/presentation.xml", r#"<?xml version="1.0" encoding="UTF-8"?><p:presentation xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main" xmlns:r="http://schemas.openxmlformats.org/officeDocument/2006/relationships"><p:sldIdLst><p:sldId id="256" r:id="rId1"/></p:sldIdLst></p:presentation>"#.to_string()),
        ("ppt/_rels/presentation.xml.rels", r#"<?xml version="1.0" encoding="UTF-8"?><Relationships xmlns="http://schemas.openxmlformats.org/package/2006/relationships"><Relationship Id="rId1" Type="http://schemas.openxmlformats.org/officeDocument/2006/relationships/slide" Target="slides/slide1.xml"/></Relationships>"#.to_string()),
        ("ppt/slides/slide1.xml", r#"<?xml version="1.0" encoding="UTF-8"?><p:sld xmlns:a="http://schemas.openxmlformats.org/drawingml/2006/main" xmlns:p="http://schemas.openxmlformats.org/presentationml/2006/main"><p:cSld><p:spTree><p:sp><p:txBody><a:bodyPr/><a:p><a:r><a:t>ConvertMe fixtures</a:t></a:r></a:p></p:txBody></p:sp></p:spTree></p:cSld></p:sld>"#.to_string()),
    ];
    write_zip(path, &entries)
}

fn write_minimal_ods(path: &Path) -> Result<(), String> {
    let entries = [
        ("mimetype", "application/vnd.oasis.opendocument.spreadsheet".to_string()),
        ("META-INF/manifest.xml", r#"<?xml version="1.0" encoding="UTF-8"?><manifest:manifest xmlns:manifest="urn:oasis:names:tc:opendocument:xmlns:manifest:1.0" manifest:version="1.2"><manifest:file-entry manifest:full-path="/" manifest:media-type="application/vnd.oasis.opendocument.spreadsheet"/><manifest:file-entry manifest:full-path="content.xml" manifest:media-type="text/xml"/></manifest:manifest>"#.to_string()),
        ("content.xml", r#"<?xml version="1.0" encoding="UTF-8"?><office:document-content xmlns:office="urn:oasis:names:tc:opendocument:xmlns:office:1.0" xmlns:table="urn:oasis:names:tc:opendocument:xmlns:table:1.0" xmlns:text="urn:oasis:names:tc:opendocument:xmlns:text:1.0" office:version="1.2"><office:body><office:spreadsheet><table:table table:name="Sheet1"><table:table-row><table:table-cell office:value-type="string"><text:p>name</text:p></table:table-cell><table:table-cell office:value-type="string"><text:p>value</text:p></table:table-cell></table:table-row><table:table-row><table:table-cell office:value-type="string"><text:p>alpha</text:p></table:table-cell><table:table-cell office:value-type="float" office:value="1"><text:p>1</text:p></table:table-cell></table:table-row></table:table></office:spreadsheet></office:body></office:document-content>"#.to_string()),
    ];
    write_zip(path, &entries)
}

// ---------------------------------------------------------------------------
// Vendoring
// ---------------------------------------------------------------------------

fn host_triple() -> String {
    let out = Command::new("rustc")
        .args(["--print", "host-tuple"])
        .output()
        .or_else(|_| Command::new("rustc").args(["-vV"]).output())
        .expect("rustc must be on PATH");
    let text = String::from_utf8_lossy(&out.stdout).into_owned();
    text.lines()
        .find_map(|line| {
            line.strip_prefix("host: ").or(if text.contains("host:") {
                None
            } else {
                Some(line.trim())
            })
        })
        .unwrap_or("x86_64-unknown-linux-gnu")
        .trim()
        .to_string()
}

/// `vendor/bin/<tool>-<triple>` is what Tauri's `externalBin` resolves.
fn vendor() -> Result<(), String> {
    let root = repo_root();
    let triple = host_triple();
    let dir = root.join("vendor").join("bin");
    std::fs::create_dir_all(&dir).map_err(|e| e.to_string())?;

    let tools = [
        ("ffmpeg", "ffmpeg"),
        ("ffprobe", "ffprobe"),
        ("magick", "magick"),
        ("gs", "gs"),
        ("qpdf", "qpdf"),
        ("pdftoppm", "pdftoppm"),
        ("pdftotext", "pdftotext"),
        ("pandoc", "pandoc"),
        ("7zz", "7zz"),
        ("tesseract", "tesseract"),
        ("convertme-pdf", "convertme-pdf"),
    ];
    let mut total = 0u64;
    for (name, binary) in tools {
        let Some(source) = which(binary) else {
            println!("skip  {name:<14} not found on PATH");
            continue;
        };
        let target = dir.join(format!(
            "{name}-{triple}{}",
            if cfg!(windows) { ".exe" } else { "" }
        ));
        std::fs::copy(&source, &target)
            .map_err(|e| format!("copying {source:?} → {target:?}: {e}"))?;
        let size = std::fs::metadata(&target).map(|m| m.len()).unwrap_or(0);
        total += size;
        println!(
            "vendor {name:<14} {:>6.1} MB  {}",
            size as f64 / 1e6,
            source.display()
        );
    }
    println!("total bundled tool size: {:.1} MB", total as f64 / 1e6);
    Ok(())
}

fn which(binary: &str) -> Option<PathBuf> {
    let name = if cfg!(windows) {
        format!("{binary}.exe")
    } else {
        binary.to_string()
    };
    let path = std::env::var_os("PATH")?;
    std::env::split_paths(&path)
        .map(|d| d.join(&name))
        .find(|p| p.is_file())
}

// ---------------------------------------------------------------------------
// Gate, perf, release
// ---------------------------------------------------------------------------

fn debrand() -> Result<(), String> {
    let status = Command::new("python3")
        .arg(repo_root().join("scripts/debrand.py"))
        .arg("--all")
        .current_dir(repo_root())
        .status()
        .map_err(|e| format!("running the gate: {e}"))?;
    if status.success() {
        Ok(())
    } else {
        Err("the identity gate failed".into())
    }
}

fn perf(args: &[String]) -> Result<(), String> {
    let which = args.first().map(String::as_str).unwrap_or("wheel");
    let num = |flag: &str, default: u64| -> u64 {
        args.iter()
            .position(|a| a == flag)
            .and_then(|i| args.get(i + 1))
            .and_then(|v| v.parse().ok())
            .unwrap_or(default)
    };
    let budget = args
        .iter()
        .position(|a| a == "--budget")
        .and_then(|i| args.get(i + 1))
        .and_then(|v| v.parse::<f64>().ok());
    match which {
        "wheel" => perf_app("CONVERTME_PERF_WHEEL", num("--runs", 50), "wheel", budget),
        "start" => perf_app("CONVERTME_PERF_START", num("--runs", 20), "start", budget),
        "idle" => perf_idle(num("--seconds", 30)),
        other => Err(format!("unknown perf target {other:?}")),
    }
}

fn release_binary() -> Result<PathBuf, String> {
    let root = repo_root();
    let candidate = root.join("target/release").join(if cfg!(windows) {
        "convertme.exe"
    } else {
        "convertme"
    });
    if candidate.is_file() {
        return Ok(candidate);
    }
    Err("build the release binary first: cargo build -p convertme-gui --release".into())
}

/// The app measures itself (`CONVERTME_PERF_*`) and prints one `PERF …` line per
/// run; this task aggregates and enforces the budget.
fn perf_app(env_var: &str, runs: u64, label: &str, budget: Option<f64>) -> Result<(), String> {
    let binary = release_binary()?;
    // Cold start is one sample per process: each run is a fresh launch. The wheel
    // measurement is many samples inside one process, since it is the *repeat*
    // latency that has to stay under budget.
    let launches = if label == "start" { runs } else { 1 };
    let mut samples: Vec<f64> = Vec::new();
    for run in 0..launches {
        let out = Command::new(&binary)
            .env(env_var, runs.to_string())
            .env("CONVERTME_PERF", "1")
            .output()
            .map_err(|e| format!("running {binary:?}: {e}"))?;
        let stdout = String::from_utf8_lossy(&out.stdout);
        let prefix = format!("PERF {label}=");
        let run_samples: Vec<f64> = stdout
            .lines()
            .filter_map(|line| line.strip_prefix(&prefix))
            .filter_map(|v| v.split_whitespace().next().and_then(|v| v.parse().ok()))
            .collect();
        if run_samples.is_empty() {
            return Err(format!(
                "no measurements on launch {run}; the app must honour {env_var}. stderr: {}",
                String::from_utf8_lossy(&out.stderr)
            ));
        }
        samples.extend(run_samples);
    }
    if samples.is_empty() {
        return Err(format!("no measurements; the app must honour {env_var}"));
    }
    let (p50, p95) = percentiles(&samples);
    println!("{label}: n={} p50={p50:.1}ms p95={p95:.1}ms", samples.len());
    // `--budget` exists for environments whose baseline is not a desktop (a CI
    // container without a GPU starts WebKit in seconds, not milliseconds); the
    // numbers the product promises are the defaults below.
    let ceiling = budget.unwrap_or(match label {
        "wheel" => 50.0,
        "start" => 600.0,
        _ => f64::MAX,
    });
    if p95 > ceiling {
        return Err(format!(
            "{label} p95 {p95:.1} ms exceeds the {ceiling:.0} ms budget"
        ));
    }
    Ok(())
}

fn percentiles(samples: &[f64]) -> (f64, f64) {
    let mut sorted = samples.to_vec();
    sorted.sort_by(|a, b| a.partial_cmp(b).unwrap_or(std::cmp::Ordering::Equal));
    let at = |q: f64| -> f64 {
        if sorted.is_empty() {
            return 0.0;
        }
        let idx = ((sorted.len() as f64 - 1.0) * q).round() as usize;
        sorted[idx.min(sorted.len() - 1)]
    };
    (at(0.5), at(0.95))
}

/// Idle cost: CPU share of the whole process, and wakeups of the threads *this app*
/// created. WebKit's own compositor threads keep a baseline of their own, so the
/// "zero timers" requirement is measured on our threads: `convertme-*`.
fn perf_idle(seconds: u64) -> Result<(), String> {
    let binary = release_binary()?;
    let mut child = Command::new(&binary)
        .env("CONVERTME_PERF_IDLE", "1")
        .stdout(std::process::Stdio::null())
        .stderr(std::process::Stdio::null())
        .spawn()
        .map_err(|e| format!("running {binary:?}: {e}"))?;
    let pid = child.id();

    // Let startup (probing, first paint, capability cache) finish before sampling.
    std::thread::sleep(Duration::from_secs(10));
    let start = read_proc(pid);
    std::thread::sleep(Duration::from_secs(seconds));
    let end = read_proc(pid);
    let _ = child.kill();
    let _ = child.wait();

    let (Some(start), Some(end)) = (start, end) else {
        return Err("could not read /proc for the idle measurement (Linux only)".into());
    };
    let ticks_per_sec = 100.0;
    let cpu_pct = ((end.cpu_ticks.saturating_sub(start.cpu_ticks)) as f64 / ticks_per_sec)
        / seconds as f64
        * 100.0;
    let own_wakeups = end.own_wakeups.saturating_sub(start.own_wakeups) as f64 / seconds as f64;
    println!("idle: {seconds}s window, cpu={cpu_pct:.2}%, own-thread wakeups/s={own_wakeups:.2}");
    if cpu_pct > 0.2 {
        return Err(format!("idle CPU {cpu_pct:.2}% exceeds the 0.2% budget"));
    }
    if own_wakeups > 0.5 {
        return Err(format!(
            "{own_wakeups:.2} wakeups/s on ConvertMe's own threads: something is polling"
        ));
    }
    Ok(())
}

struct ProcSample {
    cpu_ticks: u64,
    own_wakeups: u64,
}

fn read_proc(pid: u32) -> Option<ProcSample> {
    let stat = std::fs::read_to_string(format!("/proc/{pid}/stat")).ok()?;
    let after_comm = stat.rsplit_once(") ")?.1;
    let fields: Vec<&str> = after_comm.split_whitespace().collect();
    // utime and stime are fields 14 and 15 of the full line; after the comm split
    // they are indices 11 and 12.
    let utime: u64 = fields.get(11)?.parse().ok()?;
    let stime: u64 = fields.get(12)?.parse().ok()?;

    // Voluntary context switches of the threads we named ourselves.
    let mut own_wakeups = 0u64;
    let tasks = std::fs::read_dir(format!("/proc/{pid}/task")).ok()?;
    for task in tasks.flatten() {
        let tid = task.file_name();
        let comm = std::fs::read_to_string(task.path().join("comm")).unwrap_or_default();
        if !comm.trim().starts_with("convertme-") {
            continue;
        }
        let status = std::fs::read_to_string(task.path().join("status")).unwrap_or_default();
        let switches = status
            .lines()
            .find_map(|l| l.strip_prefix("voluntary_ctxt_switches:"))
            .and_then(|v| v.trim().parse::<u64>().ok())
            .unwrap_or(0);
        own_wakeups += switches;
        let _ = tid;
    }
    Some(ProcSample {
        cpu_ticks: utime + stime,
        own_wakeups,
    })
}

fn release() -> Result<(), String> {
    debrand()?;
    let status = Command::new("cargo")
        .args([
            "build",
            "--release",
            "-p",
            "convertme-gui",
            "--bin",
            "convertme",
        ])
        .current_dir(repo_root())
        .status()
        .map_err(|e| format!("cargo build: {e}"))?;
    if !status.success() {
        return Err("the release build failed".into());
    }
    let bundle = repo_root().join("target/release/bundle");
    let mut total = 0u64;
    if bundle.is_dir() {
        for entry in walkdir(&bundle) {
            if let Ok(meta) = std::fs::metadata(&entry) {
                total += meta.len();
            }
        }
    }
    println!("bundle size: {:.1} MB", total as f64 / 1e6);
    if total > 320 * 1024 * 1024 {
        return Err(format!(
            "bundle is {:.1} MB, over the 320 MB ceiling",
            total as f64 / 1e6
        ));
    }
    debrand()
}

fn walkdir(root: &Path) -> Vec<PathBuf> {
    let mut out = Vec::new();
    let mut stack = vec![root.to_path_buf()];
    while let Some(dir) = stack.pop() {
        let Ok(entries) = std::fs::read_dir(&dir) else {
            continue;
        };
        for entry in entries.flatten() {
            let path = entry.path();
            if path.is_dir() {
                stack.push(path);
            } else {
                out.push(path);
            }
        }
    }
    out
}

#[allow(dead_code)]
fn elapsed_ms(start: Instant) -> f64 {
    start.elapsed().as_secs_f64() * 1000.0
}
