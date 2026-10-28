//! Face boxes for the redaction preview, from a small ONNX detector.
//!
//! The model is **not** shipped: it is an optional download into the app data dir
//! (`models/face_detection_yunet_2023mar.onnx`). When it is absent, face detection
//! reports that plainly and text redaction keeps working.
//!
//! Why YuNet and not the SCRFD weights from the original sketch: YuNet is
//! Apache-2.0 and redistributable, while the InsightFace model zoo is licensed for
//! non-commercial research only, which a shipped GPL binary cannot honour.

use crate::redact::{Detected, Rect};
use crate::{Context, JobError};
use convertme_core::error::ErrorCode;
use ort::session::Session;
use ort::value::Tensor;
use std::path::Path;

/// The detector's fixed input size and score threshold.
const INPUT: u32 = 320;
const SCORE_THRESHOLD: f32 = 0.6;
const NMS_THRESHOLD: f32 = 0.3;
const STRIDES: [usize; 3] = [8, 16, 32];

pub fn detect(image: &Path, model: &Path, cx: &Context<'_>) -> Result<Vec<Detected>, JobError> {
    let decoded = image::ImageReader::open(image)
        .map_err(|e| JobError::new(ErrorCode::Io, format!("opening the image: {e}")))?
        .with_guessed_format()
        .map_err(|e| JobError::new(ErrorCode::Io, format!("reading the image header: {e}")))?
        .decode()
        .map_err(|e| JobError::new(ErrorCode::EngineFailed, format!("decoding the image: {e}")))?
        .to_rgb8();

    let resized = image::imageops::resize(
        &decoded,
        INPUT,
        INPUT,
        image::imageops::FilterType::Triangle,
    );
    let mut data = vec![0f32; 3 * (INPUT * INPUT) as usize];
    let plane = (INPUT * INPUT) as usize;
    for (x, y, pixel) in resized.enumerate_pixels() {
        let index = y as usize * INPUT as usize + x as usize;
        data[index] = pixel[0] as f32;
        data[plane + index] = pixel[1] as f32;
        data[2 * plane + index] = pixel[2] as f32;
    }

    let mut session = Session::builder(environment()?)
        .map_err(model_error)?
        .commit_from_file(model)
        .map_err(model_error)?;

    let tensor = Tensor::from_array(([1usize, 3, INPUT as usize, INPUT as usize], data))
        .map_err(model_error)?;
    let outputs = session.run(ort::inputs![tensor]).map_err(model_error)?;

    cx.ctx.checkpoint()?;
    let mut candidates: Vec<(Rect, f32)> = Vec::new();
    for stride in STRIDES {
        let cols = (INPUT as usize) / stride;
        let rows = cols;
        let scores = output(&outputs, &format!("cls_{stride}"))?;
        let objects = output(&outputs, &format!("obj_{stride}"))?;
        let boxes = output(&outputs, &format!("bbox_{stride}"))?;
        let count = scores.len().min(objects.len()).min(boxes.len() / 4);
        for i in 0..count {
            let score = (scores[i] * objects[i]).max(0.0).sqrt();
            if score < SCORE_THRESHOLD {
                continue;
            }
            let col = (i % cols) as f32;
            let row = (i / rows.max(1)) as f32;
            let base = i * 4;
            let cx = (col + boxes[base]) * stride as f32;
            let cy = (row + boxes[base + 1]) * stride as f32;
            let w = boxes[base + 2].exp() * stride as f32;
            let h = boxes[base + 3].exp() * stride as f32;
            let rect = Rect {
                x: ((cx - w / 2.0) / INPUT as f32) as f64,
                y: ((cy - h / 2.0) / INPUT as f32) as f64,
                w: (w / INPUT as f32) as f64,
                h: (h / INPUT as f32) as f64,
            };
            candidates.push((rect, score));
        }
    }

    let kept = nms(candidates);
    Ok(kept
        .into_iter()
        .map(|(rect, score)| Detected {
            rect: Rect {
                x: rect.x.clamp(0.0, 1.0),
                y: rect.y.clamp(0.0, 1.0),
                w: rect.w.clamp(0.0, 1.0 - rect.x.clamp(0.0, 1.0)),
                h: rect.h.clamp(0.0, 1.0 - rect.y.clamp(0.0, 1.0)),
            },
            kind: "face",
            detail: format!("face {:.0}%", score * 100.0),
        })
        .collect())
}

fn output(outputs: &ort::session::SessionOutputs<'_>, name: &str) -> Result<Vec<f32>, JobError> {
    let value = outputs.get(name).ok_or_else(|| {
        JobError::new(
            ErrorCode::EngineFailed,
            format!("the model has no output {name}"),
        )
    })?;
    let (_shape, data) = value.try_extract_tensor::<f32>().map_err(model_error)?;
    Ok(data.to_vec())
}

fn nms(mut candidates: Vec<(Rect, f32)>) -> Vec<(Rect, f32)> {
    candidates.sort_by(|a, b| b.1.partial_cmp(&a.1).unwrap_or(std::cmp::Ordering::Equal));
    let mut kept: Vec<(Rect, f32)> = Vec::new();
    for (rect, score) in candidates {
        if kept
            .iter()
            .all(|(other, _)| iou(&rect, other) < NMS_THRESHOLD)
        {
            kept.push((rect, score));
        }
    }
    kept
}

fn iou(a: &Rect, b: &Rect) -> f32 {
    let ax2 = a.x + a.w;
    let ay2 = a.y + a.h;
    let bx2 = b.x + b.w;
    let by2 = b.y + b.h;
    let ix = (ax2.min(bx2) - a.x.max(b.x)).max(0.0);
    let iy = (ay2.min(by2) - a.y.max(b.y)).max(0.0);
    let inter = ix * iy;
    let union = a.w * a.h + b.w * b.h - inter;
    if union <= 0.0 {
        0.0
    } else {
        (inter / union) as f32
    }
}

/// One ONNX environment per process; building it twice is an error, not a leak.
fn environment() -> Result<&'static ort::environment::Environment, JobError> {
    static ENV: std::sync::OnceLock<ort::environment::Environment> = std::sync::OnceLock::new();
    if let Some(env) = ENV.get() {
        return Ok(env);
    }
    let built = ort::init().build().map_err(model_error)?;
    let _ = ENV.set(built);
    ENV.get().ok_or_else(|| {
        JobError::new(
            ErrorCode::EngineFailed,
            "the ONNX environment is unavailable",
        )
    })
}

fn model_error(e: impl std::fmt::Display) -> JobError {
    JobError::new(ErrorCode::EngineFailed, format!("face detector: {e}"))
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn iou_of_identical_boxes_is_one() {
        let a = Rect {
            x: 0.1,
            y: 0.1,
            w: 0.2,
            h: 0.2,
        };
        assert!((iou(&a, &a) - 1.0).abs() < 1e-6);
    }

    #[test]
    fn nms_keeps_the_highest_score_of_an_overlap() {
        let a = Rect {
            x: 0.10,
            y: 0.10,
            w: 0.20,
            h: 0.20,
        };
        let b = Rect {
            x: 0.11,
            y: 0.11,
            w: 0.20,
            h: 0.20,
        };
        let c = Rect {
            x: 0.70,
            y: 0.70,
            w: 0.10,
            h: 0.10,
        };
        let kept = nms(vec![(b, 0.7), (a, 0.9), (c, 0.8)]);
        assert_eq!(kept.len(), 2);
        assert!((kept[0].1 - 0.9).abs() < 1e-6);
    }

    #[test]
    fn disjoint_boxes_survive() {
        let kept = nms(vec![
            (
                Rect {
                    x: 0.0,
                    y: 0.0,
                    w: 0.1,
                    h: 0.1,
                },
                0.9,
            ),
            (
                Rect {
                    x: 0.5,
                    y: 0.5,
                    w: 0.1,
                    h: 0.1,
                },
                0.8,
            ),
        ]);
        assert_eq!(kept.len(), 2);
    }
}
