//! The registry snapshot: an accidental format removal must be a visible diff, not
//! a silently smaller wheel.
//!
//! Regenerate with `cargo xtask snapshot` after an intentional registry change, and
//! review the diff the way you would review a public API change.

use convertme_core::formats::{Category, FORMATS, PAIR_OVERRIDES, Registry, TOOLS};
use serde_json::Value;

fn snapshot() -> Result<Value, String> {
    let formats: Vec<Value> = FORMATS
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
    let overrides: Vec<Value> = PAIR_OVERRIDES
        .iter()
        .map(|(from, to, engine)| serde_json::json!([from.0, to.0, format!("{engine:?}")]))
        .collect();
    let tools: Vec<Value> = TOOLS
        .iter()
        .map(|t| {
            serde_json::json!({
                "id": t.id.as_str(),
                "category": t.category.key(),
                "label": t.label,
            })
        })
        .collect();
    let wheels: Value = Category::ALL
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

    Ok(serde_json::json!({
        "formats": formats,
        "overrides": overrides,
        "tools": tools,
        "wheels": wheels,
        "pairs": Registry::pairs().count(),
    }))
}

#[test]
fn planner_matches_registry_snapshot() {
    let path = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/formats.snapshot.json");
    let raw = std::fs::read_to_string(&path).unwrap_or_else(|e| {
        panic!(
            "cannot read {}: {e}; run `cargo xtask snapshot`",
            path.display()
        )
    });
    let expected: Value = serde_json::from_str(&raw).expect("the snapshot is not valid JSON");
    let actual = snapshot().expect("building the snapshot");
    assert_eq!(
        actual, expected,
        "the format registry changed; review the diff and run `cargo xtask snapshot`"
    );
}

#[test]
fn every_format_is_reachable_from_its_own_wheel_or_is_read_only() {
    for format in FORMATS {
        if format.write {
            assert!(
                format.is_listed_in(format.category),
                "{} is writable but not on its own wheel",
                format.id.0
            );
        }
    }
}

#[test]
fn pair_engine_never_names_an_unregisterable_pair() {
    for (from, to, _engine) in PAIR_OVERRIDES {
        let source = Registry::get(*from).unwrap_or_else(|| panic!("{from} is not registered"));
        let target = Registry::get(*to).unwrap_or_else(|| panic!("{to} is not registered"));
        assert!(source.read, "{from} is write-only but appears as an input");
        assert!(target.write, "{to} is read-only but appears as a target");
        assert!(
            target.is_listed_in(source.category),
            "{from} → {to} is not on the {} wheel",
            source.category
        );
    }
}

#[test]
fn every_tool_belongs_to_an_existing_category() {
    for tool in TOOLS {
        assert!(
            Registry::tools_for(tool.category)
                .iter()
                .any(|t| t.id == tool.id),
            "{} is not listed for {}",
            tool.id.as_str(),
            tool.category
        );
        assert!(!tool.label.is_empty() && !tool.blurb.is_empty());
    }
}
