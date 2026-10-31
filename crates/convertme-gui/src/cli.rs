//! The CLI. Same pipeline as the GUI: same planner, same queue, same engines, same
//! publish rules — it just has no wheel. This is also the documented degradation
//! path on desktops where global input access is refused.

use convertme_core::formats::{Category, Registry};
use convertme_core::job::{Intent, Job};
use convertme_core::naming::Overwrite;
use convertme_core::queue::{Queue, QueueConfig};
use convertme_core::store::Store;
use convertme_core::{paths, plan};
use std::path::PathBuf;
use std::time::{Duration, Instant};

const USAGE: &str = "\
ConvertMe — offline file conversion

Usage:
  convertme convert <file>... --to <FORMAT> [--overwrite never|ask|replace]
  convertme tool <file>... --tool <TOOL> [--params '<json>'] [--category <CATEGORY>]
  convertme preview <file> --kind <thumb|filmstrip|frames|waveform|pdf-pages|photo|compress>
  convertme pdf-pages <file>
  convertme probe [--tools|--fields]
  convertme history [--limit N]
  convertme sweep
  convertme --version

Formats and tools are the ones on the wheel; `convertme probe` lists what is
installed, and `--help` on a subcommand explains its flags.

convert:
  convertme convert <file>... --to <FORMAT> [--overwrite never|ask|replace]

tool:
  convertme tool <file>... --tool <TOOL> [--params '<json>']
                  [--category <CATEGORY>] [--overwrite never|ask|replace]

pdf-pages:
  convertme pdf-pages <file>

history:
  convertme history [--limit N]
";

/// The verbs that make this a command-line run rather than a window launch.
///
/// One list, used by `main` to decide which front end to start: a verb that exists
/// in `run` but not here silently opens the window instead of doing the work (the
/// `preview` subcommand did exactly that and hung a headless harness).
pub const CLI_VERBS: &[&str] = &[
    "convert",
    "tool",
    "preview",
    "pdf-pages",
    "probe",
    "history",
    "sweep",
    "help",
    "--help",
    "-h",
    "--version",
    "-V",
    "version",
];

pub fn is_cli_command(verb: &str) -> bool {
    CLI_VERBS.contains(&verb)
}

pub fn run(args: &[String]) -> i32 {
    let Some(command) = args.first().map(String::as_str) else {
        print!("{USAGE}");
        return 0;
    };
    match command {
        "--version" | "-V" | "version" => {
            println!(
                "{} {}",
                convertme_core::identity::BIN,
                env!("CARGO_PKG_VERSION")
            );
            0
        }
        "--help" | "-h" | "help" => {
            print!("{USAGE}");
            0
        }
        "convert" => convert(&args[1..]),
        "tool" => tool(&args[1..]),
        "preview" => preview(&args[1..]),
        "pdf-pages" => pdf_pages(&args[1..]),
        "probe" => probe(&args[1..]),
        "history" => history(&args[1..]),
        "sweep" => {
            let dirs = convertme_core::queue::sweep_temp_dirs();
            println!("removed {dirs} stale work directories");
            // A crashed run leaves `running` rows behind; reconciling here makes the
            // command the one-stop recovery step.
            let now = std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0);
            match Store::open(&paths::history_db()).and_then(|store| store.reconcile(now)) {
                Ok(n) => println!("marked {n} interrupted job(s)"),
                Err(e) => eprintln!("could not reconcile the history: {e}"),
            }
            0
        }
        other => {
            eprintln!("unknown command {other:?}\n\n{USAGE}");
            2
        }
    }
}

/// One subcommand's arguments: the values of the flags it declares, plus its paths.
///
/// Parsing is strict on purpose. The permissive version silently skipped any `--foo`
/// and the word after it, so a mistyped flag ate the path that followed and the job
/// ran on the wrong selection. Anything undeclared is now an error.
#[derive(Debug)]
struct Args {
    values: Vec<(String, String)>,
    files: Vec<PathBuf>,
}

impl Args {
    fn parse(args: &[String], known: &[&str]) -> Result<Self, String> {
        let mut values = Vec::new();
        let mut files = Vec::new();
        let mut iter = args.iter();
        while let Some(arg) = iter.next() {
            let Some(rest) = arg.strip_prefix("--") else {
                if arg.starts_with('-') && arg.len() > 1 {
                    return Err(format!("unexpected argument {arg:?}"));
                }
                files.push(PathBuf::from(arg));
                continue;
            };
            let (name, inline) = match rest.split_once('=') {
                Some((name, value)) => (format!("--{name}"), Some(value.to_string())),
                None => (arg.clone(), None),
            };
            if !known.contains(&name.as_str()) {
                return Err(format!("unexpected argument {name:?}"));
            }
            if name == "--help" {
                values.push((name, String::new()));
                continue;
            }
            let value = match inline {
                Some(value) => value,
                None => iter
                    .next()
                    .ok_or_else(|| format!("{name} needs a value"))?
                    .clone(),
            };
            values.push((name, value));
        }
        Ok(Self { values, files })
    }

    fn has(&self, name: &str) -> bool {
        self.values.iter().any(|(flag, _)| flag == name)
    }

    fn value(&self, name: &str) -> Option<&str> {
        self.values
            .iter()
            .find(|(flag, _)| flag == name)
            .map(|(_, value)| value.as_str())
    }

    fn overwrite(&self) -> Overwrite {
        Overwrite::parse(self.value("--overwrite").unwrap_or("never"))
    }
}

fn usage_error(message: &str) -> i32 {
    eprintln!("{message}");
    eprint!("{USAGE}");
    2
}

/// Expand directory arguments into their files, sorted, so a folder drop is stable.
fn expand_inputs(paths: Vec<PathBuf>) -> Vec<PathBuf> {
    let mut out = Vec::new();
    for path in paths {
        if path.is_dir() {
            let mut entries: Vec<PathBuf> = std::fs::read_dir(&path)
                .map(|read| {
                    read.flatten()
                        .map(|e| e.path())
                        .filter(|p| p.is_file())
                        .collect()
                })
                .unwrap_or_default();
            entries.sort();
            out.extend(entries);
        } else {
            out.push(path);
        }
    }
    out
}

/// Group a mixed drop by category. A selection of images and PDFs is two wheels'
/// worth of work, not an error.
fn group_by_category(files: Vec<PathBuf>) -> Vec<(Category, Vec<PathBuf>)> {
    let mut groups: Vec<(Category, Vec<PathBuf>)> = Vec::new();
    for file in files {
        match Registry::category_of(&file) {
            Some(category) => match groups.iter_mut().find(|(c, _)| *c == category) {
                Some((_, list)) => list.push(file),
                None => groups.push((category, vec![file])),
            },
            None => eprintln!("skipped {}: unknown file type", file.display()),
        }
    }
    groups
}

fn convert(args: &[String]) -> i32 {
    let parsed = match Args::parse(args, &["--to", "--overwrite", "--help"]) {
        Ok(parsed) => parsed,
        Err(message) => return usage_error(&message),
    };
    if parsed.has("--help") {
        print!("{USAGE}");
        return 0;
    }
    let Some(target) = parsed.value("--to") else {
        return usage_error("convert needs --to <FORMAT>");
    };
    let files = expand_inputs(parsed.files.clone());
    if files.is_empty() {
        return usage_error("convert needs at least one file");
    }
    let Some(intent) = Intent::convert(target) else {
        eprintln!("unknown format {target:?}");
        return 2;
    };
    let policy = parsed.overwrite();
    let groups = group_by_category(files);
    if groups.is_empty() {
        eprintln!("could not tell what kind of file that is");
        return 2;
    }
    let mut code = 0;
    for (category, group) in groups {
        let status = execute(&group, category, intent.clone(), policy);
        code = code.max(status);
    }
    code
}

fn tool(args: &[String]) -> i32 {
    let parsed = match Args::parse(
        args,
        &["--tool", "--params", "--category", "--overwrite", "--help"],
    ) {
        Ok(parsed) => parsed,
        Err(message) => return usage_error(&message),
    };
    if parsed.has("--help") {
        print!("{USAGE}");
        return 0;
    }
    let Some(tool_name) = parsed.value("--tool") else {
        return usage_error("tool needs --tool <TOOL>");
    };
    let Some(tool) = convertme_core::formats::ToolId::parse(tool_name) else {
        eprintln!("unknown tool {tool_name:?}");
        return 2;
    };
    let files = expand_inputs(parsed.files.clone());
    if files.is_empty() {
        return usage_error("tool needs at least one file");
    }
    let params = match parsed.value("--params") {
        Some(json) => match serde_json::from_str::<serde_json::Value>(json) {
            Ok(serde_json::Value::Object(map)) => map,
            Ok(_) => {
                eprintln!("--params must be a JSON object");
                return 2;
            }
            Err(e) => {
                eprintln!("--params is not valid JSON: {e}");
                return 2;
            }
        },
        None => Default::default(),
    };
    let category = match parsed.value("--category") {
        Some(name) => match Category::parse(name) {
            Some(c) => c,
            None => {
                eprintln!("unknown category {name:?}");
                return 2;
            }
        },
        None => match files.iter().find_map(|p| Registry::category_of(p)) {
            Some(c) => c,
            None => {
                eprintln!("could not tell what kind of file that is");
                return 2;
            }
        },
    };
    let params = match convertme_core::params::normalize(category, tool, params) {
        Ok(params) => params,
        Err(e) => {
            eprintln!("{e}");
            return 2;
        }
    };
    execute(
        &files,
        category,
        Intent::Tool { tool, params },
        parsed.overwrite(),
    )
}

/// Render one of the previews a dialog asks for and print what it produced.
///
/// This exists so the performance harness can measure the exact code the dialogs
/// call (`PreviewCache`, the shared filter chain, the two-slot probe pool); the
/// wheel itself never shells out to this. It writes only into the preview cache.
fn preview(args: &[String]) -> i32 {
    let parsed = match Args::parse(
        args,
        &[
            "--kind", "--size", "--frames", "--fps", "--params", "--help",
        ],
    ) {
        Ok(parsed) => parsed,
        Err(message) => return usage_error(&message),
    };
    if parsed.has("--help") {
        print!("{USAGE}");
        return 0;
    }
    let Some(kind) = parsed.value("--kind") else {
        return usage_error("preview needs --kind");
    };
    let Some(source) = parsed.files.first().cloned() else {
        return usage_error("preview needs a file");
    };
    // The dialogs never re-probe the host: the capability cache is written at startup
    // and reused. Probing here would add ten process launches to every measurement and
    // report a number the app never sees.
    let tools = {
        let caps = convertme_core::capability::Capabilities::load(&paths::capabilities_json());
        convertme_engines::Toolset::new(
            std::sync::Arc::new(parking_lot::RwLock::new(caps)),
            convertme_engines::probe::ProbeOptions::from_env(),
        )
    };
    let settings = paths::settings_json();
    let cap = convertme_core::settings::Settings::load(&settings).preview_cache_bytes;
    let cache = convertme_engines::preview::PreviewCache::new(paths::previews_dir(), cap);
    let params: convertme_core::job::ToolParams = match parsed.value("--params") {
        Some(json) => match serde_json::from_str::<serde_json::Value>(json) {
            Ok(serde_json::Value::Object(map)) => map.into_iter().collect(),
            _ => {
                eprintln!("--params must be a JSON object");
                return 2;
            }
        },
        None => Default::default(),
    };
    let size: u32 = parsed
        .value("--size")
        .and_then(|v| v.parse().ok())
        .unwrap_or(512);
    let frames: usize = parsed
        .value("--frames")
        .and_then(|v| v.parse().ok())
        .unwrap_or(12);
    let fps: u32 = parsed
        .value("--fps")
        .and_then(|v| v.parse().ok())
        .unwrap_or(10);

    let report = |paths: Vec<String>| {
        let bytes: u64 = paths
            .iter()
            .filter_map(|p| std::fs::metadata(p).ok())
            .map(|m| m.len())
            .sum();
        println!(
            "{}",
            serde_json::json!({ "kind": kind, "paths": paths, "bytes": bytes })
        );
    };

    let ctx = convertme_core::job::JobCtx::new();
    match kind {
        "thumb" => match cache.thumbnail(&source, size, &ctx) {
            Ok(path) => report(vec![path.to_string_lossy().into_owned()]),
            Err(e) => return engine_error(e),
        },
        "filmstrip" => match cache.filmstrip(&source, frames, &ctx) {
            Ok(paths) => report(
                paths
                    .iter()
                    .map(|p| p.to_string_lossy().into_owned())
                    .collect(),
            ),
            Err(e) => return engine_error(e),
        },
        "frames" => match cache.preview_frames(&source, fps, frames, &ctx) {
            Ok(paths) => report(
                paths
                    .iter()
                    .map(|p| p.to_string_lossy().into_owned())
                    .collect(),
            ),
            Err(e) => return engine_error(e),
        },
        "waveform" => match cache.waveform(&source, &ctx) {
            Ok(w) => {
                println!(
                    "{}",
                    serde_json::json!({
                        "kind": kind,
                        "duration_ms": w.duration_ms,
                        "peaks": w.peaks.len(),
                        "bytes": 0,
                    })
                );
            }
            Err(e) => return engine_error(e),
        },
        "channels" => match cache.waveform_channels(&source, &ctx) {
            Ok((left, right)) => println!(
                "{}",
                serde_json::json!({
                    "kind": kind,
                    "left": left.peaks.len(),
                    "right": right.peaks.len(),
                    "bytes": 0,
                })
            ),
            Err(e) => return engine_error(e),
        },
        "pdf-pages" => match cache.pdf_pages(&source, &ctx) {
            Ok(pages) => {
                let paths: Vec<String> = pages
                    .iter()
                    .filter_map(|p| p.thumb.as_ref())
                    .map(|p| p.to_string_lossy().into_owned())
                    .collect();
                println!(
                    "{}",
                    serde_json::json!({
                        "kind": kind,
                        "pages": pages.len(),
                        "thumbs": paths.len(),
                        "paths": paths,
                        "bytes": 0,
                    })
                );
            }
            Err(e) => return engine_error(e),
        },
        "photo" => {
            let key = match cache.artifact_key(&source, "photo", "cli", "jpg") {
                Some(key) => key,
                None => {
                    eprintln!("cannot stat {}", source.display());
                    return 2;
                }
            };
            match convertme_engines::image::photo_preview(
                &tools, &cache, &source, &params, &key, size,
            ) {
                Ok(()) => {
                    cache.enforce_cap();
                    report(vec![key.to_string_lossy().into_owned()]);
                }
                Err(e) => return engine_error(e),
            }
        }
        "compress" => {
            // The dialog's own function: normalize → encode → cache → numbers.
            let params = serde_json::Value::Object(params);
            match crate::commands::preview_compress_blocking(
                &tools,
                cache.root().to_path_buf(),
                cap,
                &source,
                params,
            ) {
                Ok(preview) => println!(
                    "{}",
                    serde_json::json!({
                        "kind": kind,
                        "paths": [preview.path],
                        "bytes": preview.bytes,
                    })
                ),
                Err(message) => {
                    eprintln!("{message}");
                    return 1;
                }
            }
        }
        other => {
            eprintln!("unknown preview kind {other:?}");
            return 2;
        }
    }
    0
}

fn engine_error(error: convertme_core::error::JobError) -> i32 {
    eprintln!("{error}");
    1
}

fn execute(files: &[PathBuf], category: Category, intent: Intent, policy: Overwrite) -> i32 {
    let tools = convertme_engines::Toolset::probe_now();
    let caps = tools.capabilities();

    let plan = match plan::plan_with_caps(files, category, intent, &caps) {
        Ok(plan) => plan,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };

    let db_path = paths::history_db();
    let store = match Store::open(&db_path) {
        Ok(store) => store,
        Err(e) => {
            eprintln!("{}: {e}", convertme_core::identity::APP_NAME);
            return 1;
        }
    };

    let (tx, rx) = crossbeam_channel::bounded(4096);
    let queue = Queue::start(
        QueueConfig {
            overwrite: policy,
            ..Default::default()
        },
        std::sync::Arc::new(convertme_engines::Dispatcher::new(tools)),
        store,
        tx,
    );

    let mut ids = Vec::new();
    for planned in plan.jobs {
        let job = Job::with_inputs(planned.inputs, planned.intent);
        match queue.submit(job) {
            Ok((id, _)) => ids.push(id),
            Err(e) => {
                // `JobError` already prints its code; prefixing it again read
                // "unsupported: unsupported: …".
                eprintln!("{e}");
                queue.shutdown();
                return 1;
            }
        }
    }
    for skipped in &plan.skipped {
        eprintln!(
            "skipped {}: {}",
            skipped.input.display(),
            skipped.error.message
        );
    }

    // Wait for the queue to drain. The CLI reports as jobs finish.
    let deadline = Instant::now() + Duration::from_secs(2 * 60 * 60);
    let mut failures = 0;
    while !ids.is_empty() && Instant::now() < deadline {
        match rx.recv_timeout(Duration::from_millis(250)) {
            Ok(convertme_core::queue::Event::Finished {
                id,
                outputs,
                fallback_dir,
            }) => {
                for output in outputs {
                    if fallback_dir {
                        println!(
                            "wrote {} (the source folder is not writable)",
                            output.display()
                        );
                    } else {
                        println!("wrote {}", output.display());
                    }
                }
                ids.retain(|i| *i != id);
            }
            Ok(convertme_core::queue::Event::Failed { id, error, .. }) => {
                eprintln!("failed: {error}");
                if let Some(tail) = &error.stderr_tail {
                    eprintln!("{tail}");
                }
                failures += 1;
                ids.retain(|i| *i != id);
            }
            Ok(convertme_core::queue::Event::Cancelled { id }) => {
                eprintln!("cancelled");
                failures += 1;
                ids.retain(|i| *i != id);
            }
            Ok(_) => {}
            Err(crossbeam_channel::RecvTimeoutError::Timeout) => {
                if queue.pending() == 0 && queue.running() == 0 && ids.is_empty() {
                    break;
                }
            }
            Err(crossbeam_channel::RecvTimeoutError::Disconnected) => break,
        }
    }

    queue.shutdown();
    if !ids.is_empty() {
        eprintln!("timed out waiting for {} job(s)", ids.len());
        return 1;
    }
    if failures == 0 { 0 } else { 1 }
}

/// Print a PDF's page count and the pages it rendered.
///
/// The organizer's dialog calls the same `PreviewCache::pdf_pages`. This verb exists
/// so the page count can be checked headlessly: as a top-level command it is in
/// `is_cli_command`, so it never falls through to a window launch.
fn pdf_pages(args: &[String]) -> i32 {
    let parsed = match Args::parse(args, &["--help"]) {
        Ok(parsed) => parsed,
        Err(message) => return usage_error(&message),
    };
    if parsed.has("--help") {
        print!("{USAGE}");
        return 0;
    }
    let Some(source) = parsed.files.first().cloned() else {
        return usage_error("pdf-pages needs a file");
    };
    let cap = convertme_core::settings::Settings::load(&paths::settings_json()).preview_cache_bytes;
    let cache = convertme_engines::preview::PreviewCache::new(paths::previews_dir(), cap);
    let ctx = convertme_core::job::JobCtx::new();
    match cache.pdf_pages(&source, &ctx) {
        Ok(pages) => {
            let thumbs: Vec<String> = pages
                .iter()
                .filter_map(|page| page.thumb.as_ref())
                .map(|path| path.to_string_lossy().into_owned())
                .collect();
            println!(
                "{}",
                serde_json::json!({
                    "kind": "pdf-pages",
                    "pages": pages.len(),
                    "thumbs": thumbs.len(),
                    "paths": thumbs,
                    "bytes": 0,
                })
            );
            0
        }
        Err(e) => engine_error(e),
    }
}

fn probe(args: &[String]) -> i32 {
    // `--tools` prints the tool table as JSON. The harnesses assert the dialog's copy
    // against it, so they read the Rust table itself instead of keeping a fourth copy
    // of the same strings.
    if args.iter().any(|arg| arg == "--fields") {
        println!("{}", crate::commands::all_tool_fields());
        return 0;
    }
    if args.iter().any(|arg| arg == "--tools") {
        let tools: Vec<serde_json::Value> = convertme_core::formats::TOOLS
            .iter()
            .map(|tool| {
                serde_json::json!({
                    "id": tool.id.as_str(),
                    "category": tool.category.key(),
                    "label": tool.label,
                    "title": tool.title,
                    "action": tool.action,
                    "benefits": tool.benefits,
                    "eyebrow": tool.eyebrow,
                })
            })
            .collect();
        println!("{}", serde_json::json!({ "tools": tools }));
        return 0;
    }
    let opts = convertme_engines::probe::ProbeOptions::from_env();
    let caps = convertme_engines::probe::probe_all(&opts);
    let cache = paths::capabilities_json();
    if let Err(e) = caps.save(&cache) {
        eprintln!("could not write {}: {e}", cache.display());
    }
    for key in convertme_core::capability::ToolKey::ALL {
        match caps.get(key) {
            Some(cap) if cap.ok => {
                println!(
                    "{:<14} ok      {:<40} {}",
                    key.as_str(),
                    cap.path
                        .as_ref()
                        .map(|p| p.display().to_string())
                        .unwrap_or_default(),
                    cap.version.clone().unwrap_or_default()
                );
            }
            _ => println!("{:<14} missing {}", key.as_str(), key.install_hint()),
        }
    }
    println!(
        "\n{} conversion options, {} file tools",
        convertme_core::conversion_option_count(),
        convertme_core::file_tool_count()
    );
    0
}

fn history(args: &[String]) -> i32 {
    let parsed = match Args::parse(args, &["--limit", "--help"]) {
        Ok(parsed) => parsed,
        Err(message) => return usage_error(&message),
    };
    if parsed.has("--help") {
        print!("{USAGE}");
        return 0;
    }
    let limit: usize = parsed
        .value("--limit")
        .and_then(|v| v.parse().ok())
        .unwrap_or(20);
    let store = match Store::open(&paths::history_db()) {
        Ok(store) => store,
        Err(e) => {
            eprintln!("{e}");
            return 1;
        }
    };
    match store.history(limit) {
        Ok(rows) => {
            for row in rows {
                let output = row
                    .output
                    .map(|p| format!(" → {}", p.display()))
                    .unwrap_or_default();
                println!(
                    "{:<11} {}{}",
                    row.status.as_str(),
                    row.input.display(),
                    output
                );
                if let Some(code) = row.error_code {
                    println!("            {code}");
                }
            }
            0
        }
        Err(e) => {
            eprintln!("{e}");
            1
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn args(values: &[&str]) -> Vec<String> {
        values.iter().map(|v| v.to_string()).collect()
    }

    #[test]
    fn the_dispatch_verbs_are_all_cli_verbs() {
        // Every verb `run` dispatches must be in the shared list: otherwise the binary
        // launches the window instead of doing the work (LESSONS 42).
        for verb in [
            "convert",
            "tool",
            "preview",
            "pdf-pages",
            "probe",
            "history",
            "sweep",
            "help",
            "--help",
            "-h",
            "--version",
            "-V",
            "version",
        ] {
            assert!(is_cli_command(verb), "{verb} is not a CLI verb");
        }
        assert!(!is_cli_command("open"));
    }

    #[test]
    fn flag_values_are_accepted_inline_and_separate() {
        let parsed = Args::parse(&args(&["--to", "WEBP"]), &["--to"]).unwrap();
        assert_eq!(parsed.value("--to"), Some("WEBP"));
        let parsed = Args::parse(&args(&["--to=WEBP"]), &["--to"]).unwrap();
        assert_eq!(parsed.value("--to"), Some("WEBP"));
    }

    #[test]
    fn flag_values_are_not_taken_as_files() {
        let parsed = Args::parse(&args(&["a.png", "--to", "WEBP", "b.png"]), &["--to"]).unwrap();
        assert_eq!(
            parsed.files,
            vec![PathBuf::from("a.png"), PathBuf::from("b.png")]
        );
    }

    #[test]
    fn unknown_flags_are_rejected_instead_of_swallowing_the_next_word() {
        // The bug this pins: `--out /tmp/x` used to be skipped, taking the path with it.
        let error = Args::parse(&args(&["--out", "/tmp/x", "a.png"]), &["--to"]).unwrap_err();
        assert!(error.contains("--out"), "{error}");
        let error = Args::parse(&args(&["-x", "a.png"]), &["--to"]).unwrap_err();
        assert!(error.contains("-x"), "{error}");
    }

    #[test]
    fn a_flag_without_a_value_is_an_error() {
        let error = Args::parse(&args(&["a.png", "--to"]), &["--to"]).unwrap_err();
        assert!(error.contains("--to"), "{error}");
    }
}
