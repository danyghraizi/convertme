//! The planner: pure, no I/O beyond `stat`, no subprocesses.
//!
//! Given the dropped inputs, the category the wheel was opened for and the intent
//! the user picked, it produces the exact list of jobs the queue should run, plus
//! a per-file `skipped` list. One unreadable file in a 20-file selection must not
//! abort the batch — that is why skipping is a first-class outcome, not an error.

use crate::capability::Capabilities;
use crate::error::{ErrorCode, JobError, PlanError};
use crate::formats::{Category, FormatId, Registry, ToolId};
use crate::job::{Intent, Pool};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct PlannedJob {
    /// Selection order preserved.
    pub inputs: Vec<PathBuf>,
    pub intent: Intent,
    pub pool: Pool,
    pub input_format: Option<FormatId>,
}

impl PlannedJob {
    pub fn primary(&self) -> &Path {
        &self.inputs[0]
    }
}

#[derive(Debug, Clone)]
pub struct Skipped {
    pub input: PathBuf,
    pub error: JobError,
}

#[derive(Debug, Clone, Default)]
pub struct Plan {
    pub category: Category,
    pub jobs: Vec<PlannedJob>,
    pub skipped: Vec<Skipped>,
}

impl Plan {
    pub fn is_empty(&self) -> bool {
        self.jobs.is_empty()
    }

    pub fn input_count(&self) -> usize {
        self.jobs.iter().map(|j| j.inputs.len()).sum()
    }
}

/// Plan with every tool assumed present. Used by the CLI and by tests.
pub fn plan(inputs: &[PathBuf], category: Category, intent: Intent) -> Result<Plan, PlanError> {
    plan_with_caps(inputs, category, intent, &Capabilities::assume_all())
}

/// Plan against a real capability cache.
pub fn plan_with_caps(
    inputs: &[PathBuf],
    category: Category,
    intent: Intent,
    caps: &Capabilities,
) -> Result<Plan, PlanError> {
    if inputs.is_empty() {
        return Err(PlanError::Empty);
    }

    // Tools that consume the whole selection as one job.
    let multi_input = matches!(
        intent.tool_id(),
        Some(ToolId::Join) | Some(ToolId::Merge) | Some(ToolId::Collage)
    );

    // Validate the intent against the registry before touching any file.
    match &intent {
        Intent::Convert { target } => {
            if !Registry::get(*target).map(|f| f.write).unwrap_or(false) {
                return Err(PlanError::Unsupported(JobError::unsupported(
                    "input", target.0,
                )));
            }
            if !Registry::get(*target)
                .map(|f| f.is_listed_in(category))
                .unwrap_or(false)
            {
                return Err(PlanError::Unsupported(JobError::new(
                    ErrorCode::Unsupported,
                    format!("{} cannot be produced from the {category} wheel", target.0),
                )));
            }
        }
        Intent::Tool { tool, .. } => {
            let Some(spec) = Registry::tool(category, *tool) else {
                return Err(PlanError::Unsupported(JobError::new(
                    ErrorCode::Unsupported,
                    format!("{tool} is not a {category} tool"),
                )));
            };
            if !caps.has_engine(spec.engine) {
                return Err(PlanError::MissingTool(JobError::missing_tool(
                    spec.engine.label(),
                )));
            }
        }
    }

    let mut out = Plan {
        category,
        ..Default::default()
    };
    let mut seen: Vec<PathBuf> = Vec::new();
    let mut batch: Vec<(PathBuf, FormatId)> = Vec::new();

    for raw in inputs {
        let canonical = crate::job::canonical_or_self(raw);
        if seen.iter().any(|s| s == &canonical) {
            continue;
        }
        seen.push(canonical.clone());

        let meta = match std::fs::metadata(&canonical) {
            Ok(m) => m,
            Err(e) => {
                out.skipped.push(Skipped {
                    input: raw.clone(),
                    error: JobError::io("cannot read the input", &e),
                });
                continue;
            }
        };
        if meta.is_dir() {
            out.skipped.push(Skipped {
                input: raw.clone(),
                error: JobError::new(
                    ErrorCode::Unsupported,
                    "folders have no conversion of their own",
                ),
            });
            continue;
        }
        if meta.len() == 0 {
            out.skipped.push(Skipped {
                input: raw.clone(),
                error: JobError::new(ErrorCode::EngineFailed, "the file is empty (0 bytes)"),
            });
            continue;
        }

        let Some(format) = Registry::detect(&canonical) else {
            out.skipped.push(Skipped {
                input: raw.clone(),
                error: JobError::new(ErrorCode::Unsupported, "unknown file type"),
            });
            continue;
        };

        match &intent {
            Intent::Convert { target } => {
                let engine = Registry::engine_for(format.id, *target);
                let Some(engine) = engine else {
                    out.skipped.push(Skipped {
                        input: raw.clone(),
                        error: JobError::unsupported(format.id.0, target.0),
                    });
                    continue;
                };
                if !caps.has_engine(engine) {
                    out.skipped.push(Skipped {
                        input: raw.clone(),
                        error: JobError::missing_tool(engine.label()),
                    });
                    continue;
                }
                if !format.read {
                    out.skipped.push(Skipped {
                        input: raw.clone(),
                        error: JobError::unsupported(format.id.0, target.0),
                    });
                    continue;
                }
            }
            Intent::Tool { tool, .. } => {
                // A tool only accepts formats it is defined for: the source must
                // belong to the wheel's category.
                if format.category != category {
                    out.skipped.push(Skipped {
                        input: raw.clone(),
                        error: JobError::new(
                            ErrorCode::Unsupported,
                            format!("{} is not a {category} file for {tool}", format.id.0),
                        ),
                    });
                    continue;
                }
            }
        }

        batch.push((raw.clone(), format.id));
        if !multi_input {
            out.jobs.push(PlannedJob {
                inputs: vec![raw.clone()],
                intent: intent.clone(),
                pool: pool_for(&intent),
                input_format: Some(format.id),
            });
        }
    }

    if multi_input {
        if batch.len() >= 2 {
            out.jobs.push(PlannedJob {
                inputs: batch.iter().map(|(p, _)| p.clone()).collect(),
                intent: intent.clone(),
                pool: Pool::Cpu,
                input_format: batch.first().map(|(_, f)| *f),
            });
        } else if let Some((path, _)) = batch.first() {
            out.skipped.push(Skipped {
                input: path.clone(),
                error: JobError::new(ErrorCode::Unsupported, "this tool needs at least two files"),
            });
        }
    }

    if out.jobs.is_empty() {
        // Nothing runnable: surface the most specific reason instead of an empty plan.
        let first = out
            .skipped
            .first()
            .map(|s| s.error.clone())
            .unwrap_or_else(|| JobError::new(ErrorCode::Unsupported, "no input could be planned"));
        return Err(match first.code {
            ErrorCode::MissingTool => PlanError::MissingTool(first),
            _ => PlanError::Unsupported(first),
        });
    }

    Ok(out)
}

fn pool_for(intent: &Intent) -> Pool {
    match intent.tool_id() {
        Some(ToolId::Extract) => Pool::Io,
        _ => Pool::Cpu,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::fs;

    fn touch(dir: &Path, name: &str, bytes: &[u8]) -> PathBuf {
        let p = dir.join(name);
        fs::write(&p, bytes).unwrap();
        p
    }

    #[test]
    fn plan_rejects_svg_input_to_svg() {
        let dir = tempfile::tempdir().unwrap();
        let svg = touch(
            dir.path(),
            "drawing.svg",
            b"<svg xmlns='http://www.w3.org/2000/svg'/>",
        );
        let err = plan(&[svg], Category::Images, Intent::convert("SVG").unwrap()).unwrap_err();
        match err {
            PlanError::Unsupported(e) => assert_eq!(e.code, ErrorCode::Unsupported),
            other => panic!("unexpected {other:?}"),
        }
    }

    #[test]
    fn plan_dedupes_and_preserves_order() {
        let dir = tempfile::tempdir().unwrap();
        let a = touch(dir.path(), "a.png", b"x");
        let b = touch(dir.path(), "b.png", b"y");
        let p = plan(
            &[b.clone(), a.clone(), b.clone()],
            Category::Images,
            Intent::convert("WEBP").unwrap(),
        )
        .unwrap();
        assert_eq!(p.jobs.len(), 2);
        assert_eq!(p.jobs[0].primary(), b);
        assert_eq!(p.jobs[1].primary(), a);
    }

    #[test]
    fn unreadable_input_is_skipped_not_fatal() {
        let dir = tempfile::tempdir().unwrap();
        let real = touch(dir.path(), "real.png", b"x");
        let ghost = dir.path().join("ghost.png");
        let p = plan(
            &[ghost, real.clone()],
            Category::Images,
            Intent::convert("WEBP").unwrap(),
        )
        .unwrap();
        assert_eq!(p.jobs.len(), 1);
        assert_eq!(p.skipped.len(), 1);
        assert!(matches!(
            p.skipped[0].error.code,
            ErrorCode::Io | ErrorCode::Permission
        ));
    }

    #[test]
    fn empty_file_is_reported_as_engine_failure() {
        let dir = tempfile::tempdir().unwrap();
        let empty = touch(dir.path(), "empty.pdf", b"");
        let p = plan(
            std::slice::from_ref(&empty),
            Category::Pdf,
            Intent::convert("DOCX").unwrap(),
        );
        assert!(p.is_err());
        if let Err(PlanError::Unsupported(e)) = p {
            assert_eq!(e.code, ErrorCode::EngineFailed);
            assert!(!e.message.is_empty());
        }
    }

    #[test]
    fn missing_tool_is_reported_before_starting() {
        let dir = tempfile::tempdir().unwrap();
        let png = touch(dir.path(), "p.png", b"x");
        let mut caps = Capabilities::assume_all();
        caps.put(crate::capability::Capability::missing(
            crate::capability::ToolKey::Magick,
        ));
        let err = plan_with_caps(
            &[png],
            Category::Images,
            Intent::convert("WEBP").unwrap(),
            &caps,
        )
        .unwrap_err();
        assert!(matches!(err, PlanError::MissingTool(_)));
    }

    #[test]
    fn join_needs_two_files_and_makes_one_job() {
        let dir = tempfile::tempdir().unwrap();
        let a = touch(dir.path(), "a.mp4", b"x");
        let b = touch(dir.path(), "b.mp4", b"y");
        let one = plan(
            std::slice::from_ref(&a),
            Category::Video,
            Intent::tool(ToolId::Join),
        );
        assert!(one.is_err());
        let two = plan(
            &[a.clone(), b.clone()],
            Category::Video,
            Intent::tool(ToolId::Join),
        )
        .unwrap();
        assert_eq!(two.jobs.len(), 1);
        assert_eq!(two.jobs[0].inputs, vec![a, b]);
    }

    #[test]
    fn tool_rejects_a_file_from_another_category() {
        let dir = tempfile::tempdir().unwrap();
        let png = touch(dir.path(), "p.png", b"x");
        let err = plan(&[png], Category::Video, Intent::tool(ToolId::Trim));
        assert!(err.is_err());
    }

    #[test]
    fn convert_targets_outside_the_category_are_rejected() {
        let dir = tempfile::tempdir().unwrap();
        let png = touch(dir.path(), "p.png", b"x");
        let err = plan(&[png], Category::Images, Intent::convert("MKV").unwrap());
        assert!(matches!(err, Err(PlanError::Unsupported(_))));
    }
}
