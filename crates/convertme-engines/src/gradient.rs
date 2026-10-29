//! Background gradients, computed here rather than by a delegate.
//!
//! ImageMagick's `gradient:` pseudo-format takes exactly two colours, so a
//! multi-stop gradient with an angle is painted into a PPM (one row at a time,
//! never a whole image in memory) and composited like any other background.
//!
//! The value the dialog sends is a single string so it survives the plan and the
//! history table unchanged:
//!
//! ```text
//! linear|<angle>|#0b1020@0,#4b5563@35,#fbbf24@100
//! radial||#f6c26b@0,#e4572e@100
//! ```
//!
//! Positions are optional and default to even spacing; `#a,#b` alone is still
//! accepted so an older request keeps working.

use convertme_core::error::JobError;
use std::fs::File;
use std::io::{BufWriter, Write};
use std::path::Path;

pub const MAX_STOPS: usize = 8;
pub const MIN_STOPS: usize = 2;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GradientKind {
    Linear,
    Radial,
}

#[derive(Debug, Clone)]
pub struct GradientSpec {
    pub kind: GradientKind,
    /// Degrees clockwise from "up", as CSS `linear-gradient` means them.
    /// Ignored by [`GradientKind::Radial`].
    pub angle: f32,
    /// Colours with their position in 0..=1, sorted and spanning both ends.
    pub stops: Vec<([f32; 3], f32)>,
}

fn unsupported(message: impl Into<String>) -> JobError {
    JobError::new(
        convertme_core::error::ErrorCode::Unsupported,
        message.into(),
    )
}

/// `#rgb` / `#rrggbb`, which is what the picker emits.
fn parse_colour(value: &str) -> Result<[f32; 3], JobError> {
    let hex = value.trim().trim_start_matches('#');
    let expand = |s: &str| -> Result<u8, JobError> {
        u8::from_str_radix(s, 16).map_err(|_| unsupported(format!("`{value}` is not a colour")))
    };
    let (r, g, b) = match hex.len() {
        3 => {
            let mut chars = hex.chars();
            let mut next = || {
                let c = chars.next().unwrap_or('0');
                format!("{c}{c}")
            };
            let (r, g, b) = (next(), next(), next());
            (expand(&r)?, expand(&g)?, expand(&b)?)
        }
        6 => (
            expand(&hex[0..2])?,
            expand(&hex[2..4])?,
            expand(&hex[4..6])?,
        ),
        _ => return Err(unsupported(format!("`{value}` is not a colour"))),
    };
    Ok([
        f32::from(r) / 255.0,
        f32::from(g) / 255.0,
        f32::from(b) / 255.0,
    ])
}

/// Parse the field's value into a paintable spec.
pub fn parse(value: &str) -> Result<GradientSpec, JobError> {
    let text = value.trim();
    if text.is_empty() {
        return Err(unsupported("`gradient` cannot be empty"));
    }
    // `type|angle|stops`: the last part is the stop list, everything before it is
    // the type and the angle, in any order.
    let parts: Vec<&str> = text.split('|').collect();
    let stops_text = parts[parts.len() - 1];
    let head: Vec<&str> = if parts.len() > 1 {
        parts[..parts.len() - 1].to_vec()
    } else {
        Vec::new()
    };

    let mut kind = GradientKind::Linear;
    let mut angle = 0.0_f32;
    if !head.is_empty() {
        for part in head {
            let part = part.trim();
            if part.is_empty() {
                continue;
            }
            if let Ok(degrees) = part.parse::<f32>() {
                if !degrees.is_finite() || !(-360.0..=360.0).contains(&degrees) {
                    return Err(unsupported("`gradient` angle must be between -360 and 360"));
                }
                angle = degrees;
            } else if part.eq_ignore_ascii_case("linear") {
                kind = GradientKind::Linear;
            } else if part.eq_ignore_ascii_case("radial") {
                kind = GradientKind::Radial;
            } else {
                return Err(unsupported(format!(
                    "`{part}` is not a gradient type or angle"
                )));
            }
        }
    }

    let mut stops: Vec<([f32; 3], Option<f32>)> = Vec::new();
    for entry in stops_text.split(',') {
        let entry = entry.trim();
        if entry.is_empty() {
            continue;
        }
        let (colour, position) = match entry.split_once('@') {
            Some((colour, position)) => {
                let position: f32 = position
                    .trim()
                    .trim_end_matches('%')
                    .parse()
                    .map_err(|_| unsupported(format!("`{entry}` has no readable position")))?;
                if !position.is_finite() || !(0.0..=100.0).contains(&position) {
                    return Err(unsupported("`gradient` positions are percentages"));
                }
                (colour, Some(position / 100.0))
            }
            None => (entry, None),
        };
        stops.push((parse_colour(colour)?, position));
    }

    if !(MIN_STOPS..=MAX_STOPS).contains(&stops.len()) {
        return Err(unsupported(format!(
            "`gradient` takes {MIN_STOPS} to {MAX_STOPS} colours"
        )));
    }

    // Even spacing when the dialog does not send positions, then force the ends.
    if stops.iter().all(|(_, position)| position.is_none()) {
        let last = (stops.len() - 1) as f32;
        for (index, stop) in stops.iter_mut().enumerate() {
            stop.1 = Some(index as f32 / last);
        }
    } else {
        let last = (stops.len() - 1) as f32;
        for (index, stop) in stops.iter_mut().enumerate() {
            stop.1 = Some(stop.1.unwrap_or(index as f32 / last));
        }
    }
    let mut stops: Vec<([f32; 3], f32)> = stops
        .into_iter()
        .map(|(colour, position)| (colour, position.unwrap_or(0.0)))
        .collect();
    stops.sort_by(|a, b| a.1.total_cmp(&b.1));
    stops[0].1 = 0.0;
    if let Some(last) = stops.last_mut() {
        last.1 = 1.0;
    }

    Ok(GradientSpec { kind, angle, stops })
}

fn sample(stops: &[([f32; 3], f32)], t: f32) -> [f32; 3] {
    let t = t.clamp(0.0, 1.0);
    if t <= stops[0].1 {
        return stops[0].0;
    }
    for pair in stops.windows(2) {
        let (from, to) = (&pair[0], &pair[1]);
        if t <= to.1 {
            let span = (to.1 - from.1).max(f32::EPSILON);
            let k = (t - from.1) / span;
            return [
                from.0[0] + (to.0[0] - from.0[0]) * k,
                from.0[1] + (to.0[1] - from.0[1]) * k,
                from.0[2] + (to.0[2] - from.0[2]) * k,
            ];
        }
    }
    stops[stops.len() - 1].0
}

/// Write the gradient as a binary PPM, one row at a time.
pub fn write_ppm(path: &Path, width: u32, height: u32, spec: &GradientSpec) -> std::io::Result<()> {
    let mut out = BufWriter::with_capacity(64 * 1024, File::create(path)?);
    writeln!(out, "P6\n{width} {height}\n255")?;
    let mut row = Vec::with_capacity(width as usize * 3);
    let (angle, cx, cy) = (
        spec.angle.to_radians(),
        width as f32 / 2.0,
        height as f32 / 2.0,
    );

    // The dialog speaks CSS: 0° points up, 90° points right, clockwise. Screen
    // y grows downward, so the direction is (sin, -cos) and the preview is truthful.
    let (dx, dy) = (angle.sin(), -angle.cos());
    // Pixel-centre projections of the four corners: the first and last pixel of the
    // axis land exactly on 0 and 1 whatever the angle is.
    let projection = |x: f32, y: f32| x * dx + y * dy;
    let corners = [
        projection(0.5, 0.5),
        projection(width as f32 - 0.5, 0.5),
        projection(0.5, height as f32 - 0.5),
        projection(width as f32 - 0.5, height as f32 - 0.5),
    ];
    let min_projection = corners.iter().copied().fold(f32::INFINITY, f32::min);
    let max_projection = corners.iter().copied().fold(f32::NEG_INFINITY, f32::max);
    let span = (max_projection - min_projection).max(1.0);
    let radius = (cx * cx + cy * cy).sqrt().max(1.0);

    for y in 0..height {
        row.clear();
        for x in 0..width {
            let (px, py) = (x as f32 + 0.5, y as f32 + 0.5);
            let t = match spec.kind {
                GradientKind::Linear => (projection(px, py) - min_projection) / span,
                GradientKind::Radial => {
                    let (ox, oy) = (px - cx, py - cy);
                    (ox * ox + oy * oy).sqrt() / radius
                }
            };
            let colour = sample(&spec.stops, t);
            row.push((colour[0] * 255.0).round().clamp(0.0, 255.0) as u8);
            row.push((colour[1] * 255.0).round().clamp(0.0, 255.0) as u8);
            row.push((colour[2] * 255.0).round().clamp(0.0, 255.0) as u8);
        }
        out.write_all(&row)?;
    }
    out.flush()
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn two_colours_are_still_accepted() {
        let spec = parse("#000000,#ffffff").unwrap();
        assert_eq!(spec.stops.len(), 2);
        assert_eq!(spec.kind, GradientKind::Linear);
        assert_eq!(spec.angle, 0.0);
        assert_eq!(spec.stops[0].1, 0.0);
        assert_eq!(spec.stops[1].1, 1.0);
    }

    #[test]
    fn stops_and_angle_round_trip() {
        let spec = parse("linear|135|#0b1020@0,#4b5563@35,#fbbf24@100").unwrap();
        assert_eq!(spec.stops.len(), 3);
        assert_eq!(spec.angle, 135.0);
        assert!((spec.stops[1].1 - 0.35).abs() < 1e-6);
    }

    #[test]
    fn radial_ignores_the_angle_but_keeps_the_stops() {
        let spec = parse("radial|90|#ffffff@0,#000000@100").unwrap();
        assert_eq!(spec.kind, GradientKind::Radial);
        assert_eq!(spec.stops.len(), 2);
    }

    #[test]
    fn too_few_or_too_many_stops_are_refused() {
        assert!(parse("#000000").is_err());
        let many = (0..9)
            .map(|i| format!("#00000{i}"))
            .collect::<Vec<_>>()
            .join(",");
        assert!(parse(&many).is_err());
    }

    #[test]
    fn a_missing_position_falls_back_to_even_spacing() {
        let spec = parse("#000000@0,#808080,#ffffff@100").unwrap();
        assert!((spec.stops[1].1 - 0.5).abs() < 1e-6);
    }

    #[test]
    fn the_angle_follows_the_css_convention() {
        // 90° points right, so the left edge is the first colour.
        let dir = std::env::temp_dir().join(format!("convertme-angle-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("a.ppm");
        let spec = parse("linear|90|#000000@0,#ffffff@100").unwrap();
        write_ppm(&path, 8, 1, &spec).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        let needle: &[u8] = b"\n255\n";
        let body = bytes
            .windows(needle.len())
            .position(|w| w == needle)
            .map(|at| &bytes[at + needle.len()..])
            .unwrap();
        assert!(body[0] < 10);
        assert!(body[body.len() - 3] > 245);
        let _ = std::fs::remove_dir_all(&dir);
    }

    #[test]
    fn the_written_gradient_starts_and_ends_at_the_named_colours() {
        let dir = std::env::temp_dir().join(format!("convertme-gradient-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("g.ppm");
        // Horizontal on purpose: a 4x1 image has no vertical extent to show an
        // upward gradient on.
        let spec = parse("linear|90|#000000,#ffffff").unwrap();
        write_ppm(&path, 4, 1, &spec).unwrap();
        let bytes = std::fs::read(&path).unwrap();
        let needle: &[u8] = b"\n255\n";
        let body = bytes
            .windows(needle.len())
            .position(|w| w == needle)
            .map(|at| &bytes[at + needle.len()..])
            .unwrap();
        assert_eq!(body.len(), 4 * 3);
        assert!(body[0] < 10, "left edge should be the first colour");
        assert!(body[9] > 245, "right edge should be the last colour");
        let _ = std::fs::remove_dir_all(&dir);
    }
}
