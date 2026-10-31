//! Normalising the parameters a surface collected into the shape the engines expect.
//!
//! The GUI's modal and the CLI's `--params` both produce a flat map; range-like
//! fields arrive as the text the user typed ("0-10, 10-20") and become arrays of
//! `{start, end}` here, so no engine has to parse prose.

use crate::formats::{Category, ToolId};
use crate::job::ToolParams;
use serde_json::Value;

/// Turn the modal's flat fields into the shape the engines expect.
pub fn normalize(
    _category: Category,
    _tool: ToolId,
    params: serde_json::Map<String, Value>,
) -> Result<ToolParams, String> {
    let mut out = ToolParams::new();
    let mut set = serde_json::Map::new();

    for (key, value) in params {
        if let Some(tag) = key.strip_prefix("set.") {
            if value.as_str().map(|s| !s.is_empty()).unwrap_or(false) {
                set.insert(tag.to_string(), value);
            }
            continue;
        }
        match key.as_str() {
            // "0-10, 10-20" → [{"start":0,"end":10}, …]; same shape for the video
            // split sections and the audio bleep ranges. The dialog sends the
            // structured array directly (the timeline is the editor), so a value
            // that is already typed must pass through untouched.
            "sections" | "ranges" => {
                if value.is_array() {
                    out.insert(key, value);
                } else {
                    out.insert(
                        key.clone(),
                        Value::Array(parse_ranges(value.as_str().unwrap_or_default())?),
                    );
                }
            }
            "times" => {
                if value.is_array() {
                    out.insert(key, value);
                } else {
                    let text = value.as_str().unwrap_or_default();
                    let arr: Result<Vec<f64>, _> = text
                        .split(',')
                        .map(str::trim)
                        .filter(|p| !p.is_empty())
                        .map(|p| p.parse::<f64>())
                        .collect();
                    out.insert(
                        "times".into(),
                        serde_json::json!(arr.map_err(|_| "times must be numbers")?),
                    );
                }
            }
            "order" => {
                if value.is_array() {
                    out.insert(key, value);
                } else {
                    let text = value.as_str().unwrap_or_default();
                    let arr: Result<Vec<u32>, _> = text
                        .split(',')
                        .map(str::trim)
                        .filter(|p| !p.is_empty())
                        .map(|p| p.parse::<u32>())
                        .collect();
                    out.insert(
                        "order".into(),
                        serde_json::json!(arr.map_err(|_| "order must be page numbers")?),
                    );
                }
            }
            // The page organizer sends the object it already holds; only the CLI's
            // prose form ("1:90,2:180") needs parsing. Turning an object into an empty
            // map is how rotation was silently dropped.
            "rotate" if value.is_object() => {
                out.insert("rotate".into(), value);
            }
            "rotate" => {
                let text = value.as_str().unwrap_or_default();
                let mut map = serde_json::Map::new();
                for part in text.split(',').map(str::trim).filter(|p| !p.is_empty()) {
                    let (page, deg) = part
                        .split_once(':')
                        .ok_or_else(|| format!("{part:?} is not page:degrees"))?;
                    map.insert(
                        page.trim().to_string(),
                        serde_json::json!(
                            deg.trim()
                                .parse::<i64>()
                                .map_err(|_| "degrees must be a number")?
                        ),
                    );
                }
                out.insert("rotate".into(), Value::Object(map));
            }
            _ => {
                out.insert(key, value);
            }
        }
    }

    if !set.is_empty() {
        out.insert("set".into(), Value::Object(set));
    }
    Ok(out)
}

/// Parse `"0-10, 12.5-20"` into `[{"start":…,"end":…}]`.
fn parse_ranges(text: &str) -> Result<Vec<Value>, String> {
    let mut ranges = Vec::new();
    for part in text.split(',').map(str::trim).filter(|p| !p.is_empty()) {
        let (start, end) = part
            .split_once('-')
            .ok_or_else(|| format!("{part:?} is not a start-end range"))?;
        let start: f64 = start
            .trim()
            .parse()
            .map_err(|_| format!("{start:?} is not a number"))?;
        let end: f64 = end
            .trim()
            .parse()
            .map_err(|_| format!("{end:?} is not a number"))?;
        if end <= start {
            return Err(format!("range {part:?} ends before it starts"));
        }
        ranges.push(serde_json::json!({"start": start, "end": end}));
    }
    if ranges.is_empty() {
        return Err("give at least one start-end range".into());
    }
    Ok(ranges)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn ranges_become_objects() {
        let params = serde_json::json!({"sections": "0-10, 10.5-20"});
        let map = params.as_object().unwrap().clone();
        let out = normalize(Category::Video, ToolId::Split, map).unwrap();
        let sections = out.get("sections").unwrap().as_array().unwrap();
        assert_eq!(sections.len(), 2);
        assert_eq!(sections[1]["start"], 10.5);
    }

    #[test]
    fn structured_values_pass_through() {
        let map = serde_json::json!({
            "sections": [{"start": 0, "end": 2.5}, {"start": 2.5, "end": 5}],
            "times": [1.0, 3.0],
            "order": [1, 0],
        })
        .as_object()
        .unwrap()
        .clone();
        let out = normalize(Category::Video, ToolId::Split, map).unwrap();
        assert_eq!(
            out.get("sections").unwrap(),
            &serde_json::json!([{"start": 0, "end": 2.5}, {"start": 2.5, "end": 5}])
        );
        assert_eq!(out.get("times").unwrap(), &serde_json::json!([1.0, 3.0]));
        assert_eq!(out.get("order").unwrap(), &serde_json::json!([1, 0]));
    }

    #[test]
    fn bad_ranges_are_rejected() {
        let map = serde_json::json!({"ranges": "10-5"})
            .as_object()
            .unwrap()
            .clone();
        assert!(normalize(Category::Audio, ToolId::Bleep, map).is_err());
        let map = serde_json::json!({"ranges": "nonsense"})
            .as_object()
            .unwrap()
            .clone();
        assert!(normalize(Category::Audio, ToolId::Bleep, map).is_err());
    }

    #[test]
    fn metadata_keys_are_collected_into_set() {
        let map = serde_json::json!({"set.Title": "Hello", "set.Artist": "", "strip": false})
            .as_object()
            .unwrap()
            .clone();
        let out = normalize(Category::Images, ToolId::Metadata, map).unwrap();
        let set = out.get("set").unwrap().as_object().unwrap();
        assert_eq!(set.len(), 1, "empty values are dropped");
        assert_eq!(set["Title"], "Hello");
    }

    #[test]
    fn page_order_and_rotation_are_typed() {
        let map = serde_json::json!({"order": "3,1,2", "rotate": "1:90"})
            .as_object()
            .unwrap()
            .clone();
        let out = normalize(Category::Pdf, ToolId::Organize, map).unwrap();
        assert_eq!(out.get("order").unwrap(), &serde_json::json!([3, 1, 2]));
        assert_eq!(out.get("rotate").unwrap()["1"], 90);
        // The dialog sends the object it holds; normalising it must not empty it.
        let mut structured = serde_json::Map::new();
        structured.insert("rotate".into(), serde_json::json!({"2": 270}));
        let out = normalize(Category::Pdf, ToolId::Organize, structured).unwrap();
        assert_eq!(out.get("rotate").unwrap()["2"], 270);
    }
}
