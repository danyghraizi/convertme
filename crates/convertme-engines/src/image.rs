//! Images category: everything that runs through ImageMagick, plus the
//! in-process image → PDF writer.
//!
//! ImageMagick gets argv arrays only; source files are read, never rewritten.
//! Tools that would otherwise change pixels in place (compress, crop, annotate,
//! background, photo adjustments) write a new artifact into `work/out`, from
//! which the queue publishes it.

use crate::proc::{self, Budget, Cmd};
use crate::{Context, ensure_file};
use convertme_core::capability::ToolKey;
use convertme_core::error::{ErrorCode, JobError};
use convertme_core::formats::{Category, Format, FormatId, Registry, ToolId};
use convertme_core::job::{Intent, Job, ToolParams};
use printpdf::{
    DictItem, ExternalStream, ExternalXObject, Mm, Op, PdfDocument, PdfPage, PdfSaveOptions, Pt,
    Px, RawImage, RawImageData, RawImageFormat, XObjectId, XObjectTransform,
};
use serde_json::Value;
use std::collections::BTreeMap;
use std::ffi::{OsStr, OsString};
use std::path::{Path, PathBuf};

/// A4 portrait, in PostScript points. The page is always A4 for image → PDF;
/// each image is fitted inside it without changing its aspect ratio.
const PAGE_WIDTH_PT: f32 = 595.276;
const PAGE_HEIGHT_PT: f32 = 841.890;

/// Parameter validation, called before a job is queued.
pub fn validate(job: &Job, category: Category) -> Result<(), JobError> {
    if category != Category::Images {
        return Ok(());
    }
    match &job.intent {
        Intent::Convert { target } => {
            if target.0 == "SVG" {
                return Err(unsupported("SVG cannot be a conversion target"));
            }
            let fmt = Registry::get(*target)
                .ok_or_else(|| unsupported(format!("unknown target format {target}")))?;
            let writable =
                fmt.id == FormatId("PDF") || (fmt.write && fmt.category == Category::Images);
            if !writable {
                return Err(unsupported(format!(
                    "{target} cannot be written from an image"
                )));
            }
            Ok(())
        }
        Intent::Tool { tool, params } => validate_tool(*tool, params),
    }
}

fn validate_tool(tool: ToolId, params: &ToolParams) -> Result<(), JobError> {
    match tool {
        ToolId::Compress => {
            check_params(params, &["quality", "scale", "percent", "target_bytes"])?;
            let quality = unsigned(params, "quality")?;
            let scale = number(params, "scale")?;
            let percent = number(params, "percent")?;
            // Zero means "no target": the field is optional and starts empty.
            let _ = unsigned(params, "target_bytes")?;
            if scale.is_some() && percent.is_some() {
                return Err(unsupported("choose either `scale` or `percent`, not both"));
            }
            if let Some(q) = quality
                && !(1..=100).contains(&q)
            {
                return Err(unsupported("`quality` must be between 1 and 100"));
            }
            if let Some(s) = scale
                && !(5.0..=100.0).contains(&s)
            {
                return Err(unsupported("`scale` must be between 5 and 100"));
            }
            if let Some(p) = percent
                && !(1.0..=95.0).contains(&p)
            {
                return Err(unsupported("`percent` must be between 1 and 95"));
            }
            Ok(())
        }
        ToolId::Crop => {
            check_params(params, &["x", "y", "w", "h", "aspect"])?;
            let rect = crop_params(params)?;
            // `free` means "keep the drawn rectangle as it is", not an unknown ratio.
            let aspect = string(params, "aspect")?.filter(|a| !a.eq_ignore_ascii_case("free"));
            if rect.is_some() && aspect.is_some() {
                return Err(unsupported(
                    "use either a crop rectangle or an aspect ratio, not both",
                ));
            }
            if let Some(a) = aspect {
                parse_aspect(a)?;
            }
            Ok(())
        }
        ToolId::Annotate => {
            check_params(
                params,
                &["text", "shapes", "boxes", "arrow", "pointsize", "color"],
            )?;
            let text = string(params, "text")?.unwrap_or("").trim();
            let shapes = parse_shapes(params)?;
            if text.is_empty() && shapes.is_empty() {
                return Err(unsupported(
                    "nothing to draw: supply `text`, `shapes` or `arrow`",
                ));
            }
            if let Some(size) = number(params, "pointsize")?
                && !(1.0..=1000.0).contains(&size)
            {
                return Err(unsupported("`pointsize` must be between 1 and 1000"));
            }
            if let Some(color) = string(params, "color")?
                && color.trim().is_empty()
            {
                return Err(unsupported("`color` cannot be empty"));
            }
            Ok(())
        }
        ToolId::AddBackground => {
            check_params(
                params,
                &[
                    "color",
                    "gradient",
                    "image",
                    "aspect",
                    "padding",
                    "radius",
                    "padding_pct",
                    "radius_pct",
                    "shadow_pct",
                    "shadow_angle",
                    "bg_zoom",
                    "bg_x",
                    "bg_y",
                ],
            )?;
            if let Some(color) = string(params, "color")?
                && color.trim().is_empty()
            {
                return Err(unsupported("`color` cannot be empty"));
            }
            gradient_value(params)?
                .map(|value| crate::gradient::parse(&value))
                .transpose()?;
            if let Some(image) = string(params, "image")?
                && image.trim().is_empty()
            {
                return Err(unsupported("`image` cannot be empty"));
            }
            // "free" is the dialog's word for "keep the photo's own ratio".
            if let Some(aspect) = string(params, "aspect")?
                && !aspect.eq_ignore_ascii_case("free")
                && !aspect.is_empty()
            {
                parse_aspect(aspect)?;
            }
            for key in ["bg_zoom", "bg_x", "bg_y"] {
                if let Some(v) = number(params, key)?
                    && !v.is_finite()
                {
                    return Err(unsupported(format!("`{key}` must be a number")));
                }
            }
            for key in ["padding", "radius"] {
                if let Some(v) = number(params, key)?
                    && (!v.is_finite() || v < 0.0 || v.fract() != 0.0)
                {
                    return Err(unsupported(format!(
                        "`{key}` must be a non-negative whole number of pixels"
                    )));
                }
            }
            if let Some(v) = number(params, "padding_pct")?
                && (!v.is_finite() || !(0.0..=100.0).contains(&v))
            {
                return Err(unsupported("`padding_pct` is a percentage from 0 to 100"));
            }
            if let Some(v) = number(params, "shadow_pct")?
                && (!v.is_finite() || !(0.0..=20.0).contains(&v))
            {
                return Err(unsupported("`shadow_pct` is a percentage from 0 to 20"));
            }
            if let Some(v) = number(params, "shadow_angle")?
                && (!v.is_finite() || !(0.0..=360.0).contains(&v))
            {
                return Err(unsupported("`shadow_angle` is degrees from 0 to 360"));
            }
            if let Some(v) = number(params, "radius_pct")?
                && (!v.is_finite() || !(0.0..=100.0).contains(&v))
            {
                // 100% is a circle; the radius stops there.
                return Err(unsupported("`radius_pct` is a percentage from 0 to 100"));
            }
            Ok(())
        }
        ToolId::EditPhoto => {
            check_params(
                params,
                &[
                    "exposure",
                    "brightness",
                    "contrast",
                    "saturation",
                    "hue",
                    "temperature",
                    "gamma",
                ],
            )?;
            let brightness = number(params, "brightness")?.unwrap_or(0.0);
            let contrast = number(params, "contrast")?.unwrap_or(0.0);
            let saturation = number(params, "saturation")?.unwrap_or(0.0);
            let temperature = number(params, "temperature")?.unwrap_or(0.0);
            let hue = number(params, "hue")?.unwrap_or(0.0);
            let exposure = number(params, "exposure")?.unwrap_or(0.0);
            for (key, value) in [
                ("brightness", brightness),
                ("contrast", contrast),
                ("saturation", saturation),
                ("temperature", temperature),
                ("hue", hue),
            ] {
                if !(-100.0..=100.0).contains(&value) {
                    return Err(unsupported(format!("`{key}` must be between -100 and 100")));
                }
            }
            if !(-3.0..=3.0).contains(&exposure) {
                return Err(unsupported("`exposure` must be between -3 and 3 stops"));
            }
            if let Some(gamma) = number(params, "gamma")?
                && !(0.1..=10.0).contains(&gamma)
            {
                return Err(unsupported("`gamma` must be between 0.1 and 10"));
            }
            if brightness == 0.0
                && contrast == 0.0
                && saturation == 0.0
                && temperature == 0.0
                && hue == 0.0
                && exposure == 0.0
                && number(params, "gamma")?.is_none()
            {
                return Err(unsupported("no photo adjustment was requested"));
            }
            Ok(())
        }
        // Metadata and Redact have their own modules; nothing image-specific here.
        _ => Ok(()),
    }
}

/// Run one image job.
pub fn run(job: &Job, cx: &Context<'_>) -> Result<Vec<PathBuf>, JobError> {
    match &job.intent {
        Intent::Convert { target } => {
            if target.0 == "SVG" {
                return Err(unsupported("SVG cannot be a conversion target"));
            }
            if target.0 == "PDF" {
                images_to_pdf(job, cx)
            } else {
                convert(job, cx, *target)
            }
        }
        Intent::Tool { tool, params } => run_tool(job, cx, *tool, params),
    }
}

fn run_tool(
    job: &Job,
    cx: &Context<'_>,
    tool: ToolId,
    params: &ToolParams,
) -> Result<Vec<PathBuf>, JobError> {
    validate_tool(tool, params)?;
    match tool {
        ToolId::Compress => compress(job, cx, params),
        ToolId::Crop => crop(job, cx, params),
        ToolId::Annotate => annotate(job, cx, params),
        ToolId::AddBackground => add_background(job, cx, params),
        ToolId::EditPhoto => edit_photo(job, cx, params),
        _ => Err(unsupported(format!(
            "{tool} is not handled by the image engine"
        ))),
    }
}

// ---------------------------------------------------------------- conversions

fn convert(job: &Job, cx: &Context<'_>, target: FormatId) -> Result<Vec<PathBuf>, JobError> {
    let magick = magick_path(cx)?;
    let input = input_format(job)?;
    let out = cx.out(&format!("{}.{}", stem(job), crate::ext_of(target)));

    let mut args: Vec<OsString> = Vec::new();
    if input.id == FormatId("SVG") {
        // A drawing has no pixel grid; rasterise it at print resolution.
        args.push(os("-density"));
        args.push(os("384"));
    }
    args.push(os(&job.source));
    if is_lossy(target.0) {
        args.push(os("-quality"));
        args.push(os("92"));
    }
    args.push(os(&out));

    cx.ctx.set_stage("converting the image");
    cx.ctx.set_progress(0.05);
    run_magick(&magick, args, cx, Budget::Short)?;
    ensure_file(&out, "the image encoder")?;
    cx.ctx.set_progress(1.0);
    Ok(vec![out])
}

// -------------------------------------------------------------------- compress

fn compress(job: &Job, cx: &Context<'_>, params: &ToolParams) -> Result<Vec<PathBuf>, JobError> {
    let magick = magick_path(cx)?;
    let ext = output_ext(job);
    let out = tool_out(job, cx, ToolId::Compress);
    let lossy = is_lossy_ext(ext);

    // Three independent dials, as the tool promises: encode quality, a size cap, and
    // an optional resize. `percent` is the old spelling of "scale down by this much".
    let quality = unsigned(params, "quality")?.map(|q| q.clamp(1, 100) as u32);
    let scale = match number(params, "scale")? {
        Some(scale) => Some(scale),
        None => number(params, "percent")?.map(|percent| (100.0 - percent).max(5.0)),
    };
    let target = unsigned(params, "target_bytes")?.filter(|t| *t > 0);

    if let Some(target) = target {
        cx.ctx.set_stage("compressing to the target size");
        let produced = compress_to_target(&magick, &job.source, target, ext, lossy, scale, cx)?;
        move_into_place(&produced, &out)?;
    } else {
        cx.ctx.set_stage("compressing the image");
        cx.ctx.set_progress(0.05);
        let mut args = vec![os(&job.source)];
        if let Some(scale) = scale {
            args.push(os("-resize"));
            args.push(os(format!("{scale:.2}%")));
        }
        if lossy {
            args.push(os("-quality"));
            args.push(os(quality.unwrap_or(92).to_string()));
        }
        args.push(os(&out));
        run_magick(&magick, args, cx, Budget::Short)?;
    }

    ensure_file(&out, "the image encoder")?;
    cx.ctx.set_progress(1.0);
    Ok(vec![out])
}

/// Quality search then downscale, at most three rounds. The smallest image the
/// engine produced is returned even when the target is unreachable — shrinking
/// as far as the format allows is still the useful result.
fn compress_to_target(
    magick: &Path,
    source: &Path,
    target: u64,
    ext: &str,
    lossy: bool,
    scale: Option<f64>,
    cx: &Context<'_>,
) -> Result<PathBuf, JobError> {
    let mut current = source.to_path_buf();
    if let Some(scale) = scale {
        // Resize once, then search quality on the smaller image.
        let resized = cx.scratch(&format!("compress-scaled.{ext}"));
        run_magick(
            magick,
            vec![
                os(source),
                os("-resize"),
                os(format!("{scale:.2}%")),
                os(&resized),
            ],
            cx,
            Budget::Short,
        )?;
        current = resized;
    }
    let mut smallest: Option<PathBuf> = None;
    let mut smallest_len = u64::MAX;
    let mut probe = 0usize;

    for round in 0..3 {
        cx.ctx.checkpoint()?;

        if !lossy {
            // A lossless format ignores `-quality`, so the only levers are the palette
            // and the dimensions. A photograph that has to fit in a few hundred
            // kilobytes needs both, and this ladder is what made PNG targets reachable
            // at all.
            for colors in [256u32, 128, 64, 32, 16] {
                let path = cx.scratch(&format!("compress-palette-{probe}.{ext}"));
                let args = vec![
                    os(&current),
                    os("-colors"),
                    os(colors.to_string()),
                    os("-depth"),
                    os("8"),
                    os("-define"),
                    os("png:compression-level=9"),
                    os(&path),
                ];
                run_magick(magick, args, cx, Budget::Short)?;
                probe += 1;
                let len = file_len(&path)?;
                track_smallest(&path, len, &mut smallest, &mut smallest_len);
                if len <= target {
                    return Ok(path);
                }
            }
        }

        if lossy {
            let path = cx.scratch(&format!("compress-probe-{probe}.{ext}"));
            let len = probe_quality(magick, &current, 95, &path, cx)?;
            probe += 1;
            cx.ctx
                .set_progress((probe as f64 / 21.0).max((round as f64) * 0.25));
            if len <= target {
                return Ok(path);
            }
            track_smallest(&path, len, &mut smallest, &mut smallest_len);

            let mut low = 20i64;
            let mut high = 94i64;
            let mut best: Option<PathBuf> = None;
            let mut probes = 1usize;
            while low <= high && probes < 6 {
                let quality = (low + high) / 2;
                let path = cx.scratch(&format!("compress-probe-{probe}.{ext}"));
                let len = probe_quality(magick, &current, quality as u32, &path, cx)?;
                probe += 1;
                probes += 1;
                cx.ctx
                    .set_progress((probe as f64 / 21.0).max((round as f64) * 0.25));
                if len <= target {
                    best = Some(path);
                    low = quality + 1;
                } else {
                    track_smallest(&path, len, &mut smallest, &mut smallest_len);
                    high = quality - 1;
                }
            }
            if let Some(path) = best {
                return Ok(path);
            }
        }

        if round == 2 {
            break;
        }

        let scaled = cx.scratch(&format!("compress-scale-{round}.{ext}"));
        let mut args = vec![os(&current), os("-resize"), os("85%")];
        if lossy {
            args.push(os("-quality"));
            args.push(os("95"));
        }
        args.push(os(&scaled));
        run_magick(magick, args, cx, Budget::Short)?;
        ensure_file(&scaled, "the image encoder")?;
        let len = file_len(&scaled)?;
        track_smallest(&scaled, len, &mut smallest, &mut smallest_len);
        current = scaled;
    }

    Ok(smallest.unwrap_or(current))
}

fn probe_quality(
    magick: &Path,
    input: &Path,
    quality: u32,
    out: &Path,
    cx: &Context<'_>,
) -> Result<u64, JobError> {
    cx.ctx.set_stage("probing the compression quality");
    let args = vec![os(input), os("-quality"), os(quality.to_string()), os(out)];
    run_magick(magick, args, cx, Budget::Short)?;
    file_len(out)
}

fn track_smallest(path: &Path, len: u64, smallest: &mut Option<PathBuf>, size: &mut u64) {
    if len < *size {
        *size = len;
        *smallest = Some(path.to_path_buf());
    }
}

// ------------------------------------------------------------------------ crop

fn crop(job: &Job, cx: &Context<'_>, params: &ToolParams) -> Result<Vec<PathBuf>, JobError> {
    let magick = magick_path(cx)?;
    let (width, height) = image_size(&magick, &job.source, cx)?;
    let (x, y, w, h) = crop_rect(params, width, height)?;

    let out = tool_out(job, cx, ToolId::Crop);
    let args = vec![
        os(&job.source),
        os("-crop"),
        os(format!("{w}x{h}+{x}+{y}")),
        os("+repage"),
        os(&out),
    ];
    cx.ctx.set_stage("cropping the image");
    cx.ctx.set_progress(0.05);
    run_magick(&magick, args, cx, Budget::Short)?;
    ensure_file(&out, "the image encoder")?;
    cx.ctx.set_progress(1.0);
    Ok(vec![out])
}

/// Pixel rectangle for an explicit fraction box, an aspect ratio, or the
/// centred square used when neither is supplied.
fn crop_rect(
    params: &ToolParams,
    width: u32,
    height: u32,
) -> Result<(u32, u32, u32, u32), JobError> {
    if let Some((x, y, w, h)) = crop_params(params)? {
        let x = (x * width as f32).round() as u32;
        let y = (y * height as f32).round() as u32;
        let x = x.min(width - 1);
        let y = y.min(height - 1);
        let w = ((w * width as f32).round() as u32).clamp(1, width - x);
        let h = ((h * height as f32).round() as u32).clamp(1, height - y);
        return Ok((x, y, w, h));
    }

    if let Some(aspect) =
        string(params, "aspect")?.filter(|a| !a.eq_ignore_ascii_case("free") && !a.is_empty())
    {
        let (a, b) = parse_aspect(aspect)?;
        let (w, h) = if (width as f32) / (height as f32) > a / b {
            let w = ((height as f32) * a / b).round() as u32;
            (w.clamp(1, width), height)
        } else {
            let h = ((width as f32) * b / a).round() as u32;
            (width, h.clamp(1, height))
        };
        let x = (width - w) / 2;
        let y = (height - h) / 2;
        return Ok((x, y, w, h));
    }

    let side = width.min(height);
    Ok(((width - side) / 2, (height - side) / 2, side, side))
}

fn crop_params(params: &ToolParams) -> Result<Option<(f32, f32, f32, f32)>, JobError> {
    let x = number(params, "x")?;
    let y = number(params, "y")?;
    let w = number(params, "w")?;
    let h = number(params, "h")?;
    let present = [x, y, w, h].iter().filter(|v| v.is_some()).count();
    if present == 0 {
        return Ok(None);
    }
    let (Some(x), Some(y), Some(w), Some(h)) = (x, y, w, h) else {
        return Err(unsupported(
            "crop needs all of `x`, `y`, `w` and `h` together",
        ));
    };
    if ![x, y, w, h].iter().all(|v| v.is_finite()) {
        return Err(unsupported("crop coordinates must be finite numbers"));
    }
    if x < 0.0 || y < 0.0 || w <= 0.0 || h <= 0.0 {
        return Err(unsupported("crop `x`/`y` must be >= 0 and `w`/`h` > 0"));
    }
    if x + w > 1.0001 || y + h > 1.0001 {
        return Err(unsupported(
            "crop rectangle must lie inside the image (fractions 0..1)",
        ));
    }
    Ok(Some((x as f32, y as f32, w as f32, h as f32)))
}

// -------------------------------------------------------------------- annotate

fn annotate(job: &Job, cx: &Context<'_>, params: &ToolParams) -> Result<Vec<PathBuf>, JobError> {
    let magick = magick_path(cx)?;
    let (width, height) = image_size(&magick, &job.source, cx)?;
    let text = string(params, "text")?.unwrap_or("").trim().to_string();
    let shapes = parse_shapes(params)?;
    let pointsize = number(params, "pointsize")?.unwrap_or(32.0);
    let color = string(params, "color")?.unwrap_or("#FFFFFF").to_string();

    let out = tool_out(job, cx, ToolId::Annotate);
    let mut args = vec![os(&job.source)];

    if !text.is_empty() {
        args.push(os("-gravity"));
        args.push(os("SouthWest"));
        args.push(os("-pointsize"));
        args.push(os(format!("{pointsize:.0}")));
        args.push(os("-fill"));
        args.push(os(&color));
        args.push(os("-annotate"));
        args.push(os("+24+24"));
        args.push(os(&text));
    }

    if !shapes.is_empty() {
        args.push(os("-gravity"));
        args.push(os("NorthWest"));
        args.push(os("-fill"));
        args.push(os("none"));
        args.push(os("-stroke"));
        args.push(os(&color));
        args.push(os("-strokewidth"));
        args.push(os("4"));
        for shape in &shapes {
            args.push(os("-draw"));
            args.push(os(draw_command(shape, width, height)));
        }
    }

    args.push(os(&out));
    cx.ctx.set_stage("annotating the image");
    cx.ctx.set_progress(0.05);
    run_magick(&magick, args, cx, Budget::Short)?;
    ensure_file(&out, "the image encoder")?;
    cx.ctx.set_progress(1.0);
    Ok(vec![out])
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ShapeKind {
    Rect,
    Arrow,
}

#[derive(Debug, Clone, Copy)]
struct DrawShape {
    kind: ShapeKind,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
}

/// `shapes` and `boxes` are arrays of `{x,y,w,h}`; `arrow` is one such object.
/// All coordinates are fractions of the image, with the origin top-left.
fn parse_shapes(params: &ToolParams) -> Result<Vec<DrawShape>, JobError> {
    let mut shapes = Vec::new();

    for key in ["shapes", "boxes"] {
        match params.get(key) {
            None | Some(Value::Null) => {}
            Some(Value::Array(items)) => {
                for item in items {
                    let object = item
                        .as_object()
                        .ok_or_else(|| unsupported(format!("`{key}` entries must be objects")))?;
                    let kind = object.get("kind").and_then(Value::as_str).unwrap_or("rect");
                    shapes.push(shape_from_object(object, kind)?);
                }
            }
            Some(_) => {
                return Err(unsupported(format!("`{key}` must be an array of shapes")));
            }
        }
    }

    if let Some(value) = params.get("arrow") {
        match value {
            Value::Null => {}
            Value::Object(object) => shapes.push(shape_from_object(object, "arrow")?),
            _ => return Err(unsupported("`arrow` must be a shape object")),
        }
    }

    Ok(shapes)
}

fn shape_from_object(
    object: &serde_json::Map<String, Value>,
    kind: &str,
) -> Result<DrawShape, JobError> {
    let kind = match kind {
        "rect" | "rectangle" => ShapeKind::Rect,
        "arrow" => ShapeKind::Arrow,
        other => {
            return Err(unsupported(format!(
                "unknown shape kind `{other}`; use `rect` or `arrow`"
            )));
        }
    };
    let read = |key: &str| -> Result<f32, JobError> {
        object
            .get(key)
            .and_then(Value::as_f64)
            .map(|v| v as f32)
            .filter(|v| v.is_finite())
            .ok_or_else(|| unsupported(format!("shape `{key}` must be a finite number")))
    };
    let shape = DrawShape {
        kind,
        x: read("x")?,
        y: read("y")?,
        w: read("w")?,
        h: read("h")?,
    };
    let inside = |v: f32| (0.0..=1.0).contains(&v);
    if !inside(shape.x) || !inside(shape.y) {
        return Err(unsupported(
            "shape `x`/`y` must be fractions between 0 and 1",
        ));
    }
    match kind {
        ShapeKind::Rect => {
            if shape.w <= 0.0 || shape.h <= 0.0 {
                return Err(unsupported("rectangle `w`/`h` must be > 0"));
            }
            if !inside(shape.x + shape.w) || !inside(shape.y + shape.h) {
                return Err(unsupported("rectangle must lie inside the image"));
            }
        }
        ShapeKind::Arrow => {
            if !inside(shape.x + shape.w) || !inside(shape.y + shape.h) {
                return Err(unsupported("arrow end point must lie inside the image"));
            }
        }
    }
    Ok(shape)
}

fn draw_command(shape: &DrawShape, width: u32, height: u32) -> String {
    let x1 = shape.x * width as f32;
    let y1 = shape.y * height as f32;
    let x2 = (shape.x + shape.w) * width as f32;
    let y2 = (shape.y + shape.h) * height as f32;
    match shape.kind {
        ShapeKind::Rect => format!("rectangle {x1:.2},{y1:.2} {x2:.2},{y2:.2}"),
        ShapeKind::Arrow => {
            let angle = (y2 - y1).atan2(x2 - x1);
            let head = (0.03 * width.min(height) as f32).clamp(8.0, 48.0);
            let ax = x2 - head * (angle + 0.45).cos();
            let ay = y2 - head * (angle + 0.45).sin();
            let bx = x2 - head * (angle - 0.45).cos();
            let by = y2 - head * (angle - 0.45).sin();
            format!(
                "line {x1:.2},{y1:.2} {x2:.2},{y2:.2} \
                 line {x2:.2},{y2:.2} {ax:.2},{ay:.2} \
                 line {x2:.2},{y2:.2} {bx:.2},{by:.2}"
            )
        }
    }
}

// -------------------------------------------------------------- add background

fn add_background(
    job: &Job,
    cx: &Context<'_>,
    params: &ToolParams,
) -> Result<Vec<PathBuf>, JobError> {
    let magick = magick_path(cx)?;
    let (width, height) = image_size(&magick, &job.source, cx)?;
    // The dialog sends percentages of the shorter side: a fixed pixel count means
    // nothing on a 5000 px photo, where 400 px is an 8% border.
    let shortest = width.min(height).max(1) as f64;
    let padding = match number(params, "padding_pct")? {
        Some(pct) => (pct.clamp(0.0, 100.0) / 100.0 * shortest).round() as u32,
        None => number(params, "padding")?.unwrap_or(0.0).round() as u32,
    };
    // 100% of the slider is a circle, so the radius runs to half the shorter side.
    let radius = match number(params, "radius_pct")? {
        Some(pct) => (pct.clamp(0.0, 100.0) / 100.0 * shortest / 2.0).round() as u32,
        None => number(params, "radius")?.unwrap_or(0.0).round() as u32,
    };
    let color = string(params, "color")?.unwrap_or("#101418").to_string();
    let gradient = gradient_value(params)?
        .map(|value| crate::gradient::parse(&value))
        .transpose()?;
    let background_image = string(params, "image")?
        .map(PathBuf::from)
        .filter(|path| path.is_file());

    let mut canvas_w = width + padding.saturating_mul(2);
    let mut canvas_h = height + padding.saturating_mul(2);
    // Without an explicit ratio the canvas keeps the source aspect: padding the
    // two axes equally would otherwise stretch a non-square photo.
    // Only an explicit ratio changes the canvas: with "free" the canvas is the
    // photo plus the same padding on all four sides, which is what the dialog shows.
    if let Some(aspect) = string(params, "aspect")?
        .filter(|a| !a.eq_ignore_ascii_case("free"))
        .filter(|a| !a.is_empty())
    {
        let (a, b) = parse_aspect(aspect)?;
        if (canvas_w as f32) / (canvas_h as f32) > a / b {
            canvas_h = ((canvas_w as f32) * b / a).round().max(1.0) as u32;
        } else {
            canvas_w = ((canvas_h as f32) * a / b).round().max(1.0) as u32;
        }
    }
    let size = format!("{canvas_w}x{canvas_h}");

    let out = tool_out(job, cx, ToolId::AddBackground);

    // Rounding belongs to the photo, not to the canvas: masking the finished composition
    // cut the corners off the background, which is not what the dialog shows.
    let photo: OsString = if radius > 0 {
        let rounded = cx.scratch("photo-rounded.png");
        let r = radius.min(width.min(height) / 2).max(1).to_string();
        let mask = format!(
            "roundrectangle 0,0 {},{} {},{}",
            width.saturating_sub(1),
            height.saturating_sub(1),
            r,
            r
        );
        let args = vec![
            os("-depth"),
            os("8"),
            os(&job.source),
            os("("),
            os("-size"),
            os(format!("{width}x{height}")),
            os("xc:black"),
            os("-fill"),
            os("white"),
            os("-draw"),
            os(&mask),
            os(")"),
            os("-alpha"),
            os("off"),
            os("-compose"),
            os("CopyOpacity"),
            os("-composite"),
            os(&rounded),
        ];
        run_magick(&magick, args, cx, Budget::Short)?;
        ensure_file(&rounded, "the rounded photo")?;
        rounded.into_os_string()
    } else {
        job.source.clone().into_os_string()
    };

    // A soft drop shadow, built from an explicit shape mask: the photo may be opaque
    // (no alpha to extract), and this ImageMagick ignores `-shadow`'s own offset, so the
    // mask is generated, blurred at reduced resolution, shifted, and scaled back.
    let shadow: Option<PathBuf> = {
        let shadow_pct = number(params, "shadow_pct")?
            .unwrap_or(0.0)
            .clamp(0.0, 20.0);
        if shadow_pct <= 0.0 {
            None
        } else {
            let angle = number(params, "shadow_angle")?
                .unwrap_or(180.0)
                .to_radians();
            let sigma = (shadow_pct / 100.0 * shortest * 0.15).max(1.0);
            let offset = sigma * 2.6;
            // 0 degrees points up, 180 down: the shadow falls opposite the light.
            let dx = angle.sin() * offset;
            let dy = -angle.cos() * offset;
            let scale = (900.0 / f64::from(width.max(height))).min(1.0);
            let small_w = (f64::from(width) * scale).round().max(1.0);
            let small_h = (f64::from(height) * scale).round().max(1.0);
            let sigma_s = sigma * scale;
            let dx_s = dx * scale;
            let dy_s = dy * scale;
            let margin_x = dx_s.abs().ceil() + (sigma_s * 3.0).ceil();
            let margin_y = dy_s.abs().ceil() + (sigma_s * 3.0).ceil();
            let canvas_w = small_w + margin_x * 2.0;
            let canvas_h = small_h + margin_y * 2.0;
            let place_x = (margin_x + dx_s).round();
            let place_y = (margin_y + dy_s).round();

            // White inside the photo's shape, black outside; blurring this gives the
            // shadow's alpha directly.
            let shape = cx.scratch("photo-shape.png");
            let mut shape_args = vec![
                os("-depth"),
                os("8"),
                os("-size"),
                os(format!("{small_w}x{small_h}")),
                os("xc:black"),
            ];
            if radius > 0 {
                let r = f64::from(radius) * scale;
                shape_args.extend([
                    os("-fill"),
                    os("white"),
                    os("-draw"),
                    os(format!(
                        "roundrectangle 0,0 {},{},{r:.1},{r:.1}",
                        small_w - 1.0,
                        small_h - 1.0
                    )),
                ]);
            } else {
                shape_args.extend([
                    os("-fill"),
                    os("white"),
                    os("-draw"),
                    os(format!("rectangle 0,0 {},{}", small_w - 1.0, small_h - 1.0)),
                ]);
            }
            shape_args.push(os(&shape));
            run_magick(&magick, shape_args, cx, Budget::Short)?;

            let path = cx.scratch("photo-shadow.png");
            let args = vec![
                os("-depth"),
                os("8"),
                // Transparent black, not opaque: CopyOpacity leaves the area the mask
                // does not cover untouched, and an opaque canvas there became a slab.
                os("-size"),
                os(format!("{canvas_w}x{canvas_h}")),
                os("xc:none"),
                os("("),
                os(&shape),
                os("-blur"),
                os(format!("0x{sigma_s:.1}")),
                os("-evaluate"),
                os("multiply"),
                os("0.55"),
                os(")"),
                os("-geometry"),
                os(format!("{place_x:+.0}{place_y:+.0}")),
                os("-compose"),
                os("CopyOpacity"),
                os("-composite"),
                os("-resize"),
                os(format!("{:.3}%", 100.0 / scale)),
                os(&path),
            ];
            run_magick(&magick, args, cx, Budget::Short)?;
            ensure_file(&path, "the shadow")?;
            Some(path)
        }
    };

    cx.ctx.set_stage("composing the background");
    cx.ctx.set_progress(0.4);
    // Q16-HDRI doubles the pixel cache for no visible gain in a composed PNG/JPEG.
    let mut args: Vec<OsString> = vec![os("-depth"), os("8"), os("-size"), os(&size)];
    if let Some(spec) = &gradient {
        let paint = cx.scratch("background-gradient.ppm");
        crate::gradient::write_ppm(&paint, canvas_w, canvas_h, spec)
            .map_err(|e| convertme_core::error::JobError::io("writing the gradient", &e))?;
        args.push(os(&paint));
    } else if let Some(background) = background_image {
        // The background is a picture to place, not just a texture. Crop the source
        // window first and scale it to the canvas: upscaling before cropping is what
        // exhausted ImageMagick's pixel cache on large photos.
        let zoom = number(params, "bg_zoom")?.unwrap_or(1.0).clamp(1.0, 3.0);
        let (bg_w, bg_h) = image_size(&magick, &background, cx)?;
        let target = f64::from(canvas_w) / f64::from(canvas_h);
        let src_aspect = f64::from(bg_w) / f64::from(bg_h);
        let (base_w, base_h) = if src_aspect > target {
            (f64::from(bg_h) * target, f64::from(bg_h))
        } else {
            (f64::from(bg_w), f64::from(bg_w) / target)
        };
        let vis_w = (base_w / zoom).round().clamp(1.0, f64::from(bg_w));
        let vis_h = (base_h / zoom).round().clamp(1.0, f64::from(bg_h));
        let free_x = (base_w - vis_w).max(0.0);
        let free_y = (base_h - vis_h).max(0.0);
        let shift_x =
            (number(params, "bg_x")?.unwrap_or(0.0).clamp(-100.0, 100.0) / 100.0) * free_x / 2.0;
        let shift_y =
            (number(params, "bg_y")?.unwrap_or(0.0).clamp(-100.0, 100.0) / 100.0) * free_y / 2.0;
        let crop_x = (free_x / 2.0 - shift_x).round().clamp(0.0, free_x) as u32;
        let crop_y = (free_y / 2.0 - shift_y).round().clamp(0.0, free_y) as u32;
        args.push(os("("));
        args.push(os(&background));
        args.push(os("-crop"));
        args.push(os(format!(
            "{}x{}+{crop_x}+{crop_y}",
            vis_w.round(),
            vis_h.round()
        )));
        args.push(os("+repage"));
        args.push(os("-resize"));
        args.push(os(format!("{size}!")));
        args.push(os(")"));
    } else {
        args.push(os(format!("xc:{color}")));
    }
    if let Some(shadow) = &shadow {
        args.push(os(shadow));
        args.push(os("-gravity"));
        args.push(os("center"));
        args.push(os("-composite"));
    }
    args.push(os(&photo));
    args.push(os("-gravity"));
    args.push(os("center"));
    args.push(os("-composite"));
    args.push(os(&out));
    run_magick(&magick, args, cx, Budget::Short)?;
    ensure_file(&out, "the image encoder")?;

    cx.ctx.set_progress(1.0);
    Ok(vec![out])
}

/// The gradient as the dialog sends it: a single string, so the plan, the history
/// row and the CLI all carry it unchanged. Parsing lives in `crate::gradient`.
fn gradient_value(params: &ToolParams) -> Result<Option<String>, JobError> {
    match params.get("gradient") {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => {
            if value.trim().is_empty() {
                return Err(unsupported("`gradient` cannot be empty"));
            }
            Ok(Some(value.clone()))
        }
        Some(Value::Array(items)) if items.len() >= 2 => {
            let mut parts = Vec::with_capacity(items.len());
            for item in items {
                let colour = item
                    .as_str()
                    .ok_or_else(|| unsupported("`gradient` colours must be strings"))?;
                parts.push(colour.to_string());
            }
            Ok(Some(parts.join(",")))
        }
        Some(_) => Err(unsupported(
            "`gradient` must be a colour list or at least two colours",
        )),
    }
}

// ------------------------------------------------------------------ edit photo

fn edit_photo(job: &Job, cx: &Context<'_>, params: &ToolParams) -> Result<Vec<PathBuf>, JobError> {
    let magick = magick_path(cx)?;
    let brightness = number(params, "brightness")?.unwrap_or(0.0) as f32;
    let contrast = number(params, "contrast")?.unwrap_or(0.0) as f32;
    let saturation = number(params, "saturation")?.unwrap_or(0.0) as f32;
    let exposure = number(params, "exposure")?.unwrap_or(0.0) as f32;
    let hue = number(params, "hue")?.unwrap_or(0.0) as f32;
    let temperature = number(params, "temperature")?.unwrap_or(0.0) as f32;
    let gamma = number(params, "gamma")?;

    let out = tool_out(job, cx, ToolId::EditPhoto);
    let mut args = vec![os(&job.source)];
    if exposure != 0.0 {
        // Stops of light, applied as a linear multiply before anything tonal.
        let factor = 2f32.powf(exposure).clamp(0.05, 20.0);
        args.push(os("-evaluate"));
        args.push(os("multiply"));
        args.push(os(format!("{factor:.4}")));
    }
    if temperature != 0.0 {
        // Warm or cool: lift red and drop blue, and the reverse, without clipping.
        let warm = (temperature / 300.0).clamp(-0.33, 0.33);
        args.push(os("+channel"));
        args.push(os("-channel"));
        args.push(os("R"));
        args.push(os("-evaluate"));
        args.push(os("multiply"));
        args.push(os(format!("{:.4}", 1.0 + warm)));
        args.push(os("-channel"));
        args.push(os("B"));
        args.push(os("-evaluate"));
        args.push(os("multiply"));
        args.push(os(format!("{:.4}", 1.0 - warm)));
        args.push(os("+channel"));
    }
    if brightness != 0.0 || saturation != 0.0 || hue != 0.0 {
        let bright = (100.0 + brightness).clamp(0.0, 200.0);
        let sat = (100.0 + saturation).clamp(0.0, 200.0);
        // `-modulate`'s third value is a hue percentage: 100 is unchanged.
        let hue_shift = (100.0 + hue / 3.6).clamp(0.0, 200.0);
        args.push(os("-modulate"));
        args.push(os(format!("{bright:.2},{sat:.2},{hue_shift:.2}")));
    }
    if contrast != 0.0 {
        args.push(os("-brightness-contrast"));
        args.push(os(format!("0x{contrast:.0}")));
    }
    if let Some(gamma) = gamma {
        args.push(os("-gamma"));
        args.push(os(format!("{gamma:.3}")));
    }
    args.push(os(&out));

    cx.ctx.set_stage("adjusting the photo");
    cx.ctx.set_progress(0.05);
    run_magick(&magick, args, cx, Budget::Short)?;
    ensure_file(&out, "the image encoder")?;
    cx.ctx.set_progress(1.0);
    Ok(vec![out])
}

// ------------------------------------------------------------------ image → PDF

fn images_to_pdf(job: &Job, cx: &Context<'_>) -> Result<Vec<PathBuf>, JobError> {
    let magick = magick_path(cx)?;
    let inputs: &[PathBuf] = if job.inputs.is_empty() {
        std::slice::from_ref(&job.source)
    } else {
        &job.inputs
    };

    let mut doc = PdfDocument::new(&stem(job));
    let budget = if inputs.len() > 1 {
        Budget::Long
    } else {
        Budget::Short
    };

    for (index, input) in inputs.iter().enumerate() {
        cx.ctx.checkpoint()?;
        cx.ctx.set_stage("building the PDF");
        cx.ctx
            .set_progress(index as f64 / inputs.len() as f64 * 0.8);
        add_pdf_page(&mut doc, input, index, &magick, cx, budget)?;
    }
    cx.ctx.set_progress(0.85);

    let mut warnings = Vec::new();
    let bytes = doc.save(&PdfSaveOptions::default(), &mut warnings);
    if bytes.is_empty() {
        return Err(JobError::new(
            ErrorCode::EngineFailed,
            "the PDF writer produced no data",
        ));
    }

    let out = cx.out(&format!("{}.pdf", stem(job)));
    std::fs::write(&out, &bytes).map_err(|e| JobError::io("writing the PDF", &e))?;
    ensure_file(&out, "the PDF writer")?;
    cx.ctx.set_progress(1.0);
    Ok(vec![out])
}

fn add_pdf_page(
    doc: &mut PdfDocument,
    input: &Path,
    index: usize,
    magick: &Path,
    cx: &Context<'_>,
    budget: Budget,
) -> Result<(), JobError> {
    let bytes = std::fs::read(input)
        .map_err(|e| JobError::io(format!("reading {}", input.display()), &e))?;

    let ext = input
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or("")
        .to_ascii_lowercase();

    // JPEG is embedded as a DCTDecode XObject, byte for byte.
    if matches!(ext.as_str(), "jpg" | "jpeg")
        && let Some((width, height, components)) = jpeg_info(&bytes)
    {
        let xobject = jpeg_xobject(bytes, width, height, components);
        let id = doc.add_xobject(&xobject);
        push_image_page(doc, id, width, height);
        return Ok(());
    }

    let image = match load_image(&bytes) {
        Ok(image) => image,
        Err(_) => {
            // SVG, AVIF and HEIC have no decoder in the `image` crate; use the
            // image engine to rasterise them, then embed the PNG.
            let png = cx.scratch(&format!("pdf-page-{index}.png"));
            rasterise(magick, input, &png, cx, budget)?;
            let png_bytes =
                std::fs::read(&png).map_err(|e| JobError::io("reading the rasterised page", &e))?;
            load_image(&png_bytes).map_err(|e| {
                JobError::new(
                    ErrorCode::EngineFailed,
                    format!("decoding the page image: {e}"),
                )
            })?
        }
    };

    let rgba = image.to_rgba8();
    let (width, height) = rgba.dimensions();
    let raw = RawImage {
        pixels: RawImageData::U8(rgba.into_raw()),
        width: width as usize,
        height: height as usize,
        data_format: RawImageFormat::RGBA8,
        tag: Vec::new(),
    };
    let id = doc.add_image(&raw);
    push_image_page(doc, id, width, height);
    Ok(())
}

fn load_image(bytes: &[u8]) -> Result<::image::DynamicImage, ::image::ImageError> {
    ::image::load_from_memory(bytes)
}

fn push_image_page(doc: &mut PdfDocument, id: XObjectId, width: u32, height: u32) {
    let image_w = width as f32;
    let image_h = height as f32;
    let scale = (PAGE_WIDTH_PT * 0.97 / image_w).min(PAGE_HEIGHT_PT * 0.97 / image_h);
    let translate_x = (PAGE_WIDTH_PT - image_w * scale) / 2.0;
    let translate_y = (PAGE_HEIGHT_PT - image_h * scale) / 2.0;

    let ops = vec![Op::UseXobject {
        id,
        transform: XObjectTransform {
            translate_x: Some(Pt(translate_x)),
            translate_y: Some(Pt(translate_y)),
            rotate: None,
            scale_x: Some(scale),
            scale_y: Some(scale),
            dpi: Some(72.0),
            no_auto_scale: false,
        },
    }];
    doc.pages.push(PdfPage::new(Mm(210.0), Mm(297.0), ops));
}

fn jpeg_xobject(bytes: Vec<u8>, width: u32, height: u32, components: u8) -> ExternalXObject {
    let mut dict = BTreeMap::new();
    dict.insert("Type".to_string(), DictItem::Name(b"XObject".to_vec()));
    dict.insert("Subtype".to_string(), DictItem::Name(b"Image".to_vec()));
    dict.insert("Width".to_string(), DictItem::Int(width as i64));
    dict.insert("Height".to_string(), DictItem::Int(height as i64));
    dict.insert("BitsPerComponent".to_string(), DictItem::Int(8));
    dict.insert("Filter".to_string(), DictItem::Name(b"DCTDecode".to_vec()));

    let color_space: &[u8] = match components {
        1 => &b"DeviceGray"[..],
        4 => &b"DeviceCMYK"[..],
        _ => &b"DeviceRGB"[..],
    };
    dict.insert(
        "ColorSpace".to_string(),
        DictItem::Name(color_space.to_vec()),
    );
    if components == 4 {
        // PDF and Adobe JPEG disagree on the polarity of CMYK samples.
        dict.insert(
            "Decode".to_string(),
            DictItem::Array(vec![
                DictItem::Int(1),
                DictItem::Int(0),
                DictItem::Int(1),
                DictItem::Int(0),
                DictItem::Int(1),
                DictItem::Int(0),
                DictItem::Int(1),
                DictItem::Int(0),
            ]),
        );
    }

    ExternalXObject {
        stream: ExternalStream {
            dict,
            content: bytes,
            // Already DCT-compressed; wrapping it in Flate would only add work.
            compress: false,
        },
        width: Some(Px(width as usize)),
        height: Some(Px(height as usize)),
        dpi: Some(72.0),
    }
}

/// Dimensions and component count from the first JPEG SOF segment, without
/// decoding the pixels.
fn jpeg_info(bytes: &[u8]) -> Option<(u32, u32, u8)> {
    if bytes.len() < 4 || bytes[0] != 0xFF || bytes[1] != 0xD8 {
        return None;
    }
    let mut i = 2;
    while i + 3 < bytes.len() {
        if bytes[i] != 0xFF {
            i += 1;
            continue;
        }
        let mut marker = bytes[i + 1];
        let mut j = i + 2;
        while marker == 0xFF && j < bytes.len() {
            marker = bytes[j];
            j += 1;
        }
        i = j;
        if marker == 0xD8 || marker == 0xD9 || (0xD0..=0xD7).contains(&marker) {
            continue;
        }
        if j + 2 > bytes.len() {
            return None;
        }
        let len = ((bytes[j] as usize) << 8) | bytes[j + 1] as usize;
        if len < 2 || j + len > bytes.len() {
            return None;
        }
        let is_sof =
            (0xC0..=0xCF).contains(&marker) && marker != 0xC4 && marker != 0xC8 && marker != 0xCC;
        if is_sof {
            if len < 8 {
                return None;
            }
            let height = u16::from_be_bytes([bytes[j + 3], bytes[j + 4]]) as u32;
            let width = u16::from_be_bytes([bytes[j + 5], bytes[j + 6]]) as u32;
            let components = bytes[j + 7];
            return Some((width, height, components));
        }
        i = j + len;
    }
    None
}

fn rasterise(
    magick: &Path,
    input: &Path,
    out: &Path,
    cx: &Context<'_>,
    budget: Budget,
) -> Result<(), JobError> {
    let is_svg = input
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| e.eq_ignore_ascii_case("svg"))
        .unwrap_or(false);
    let mut args = Vec::new();
    if is_svg {
        args.push(os("-density"));
        args.push(os("384"));
    }
    args.push(os(input));
    args.push(os(out));
    run_magick(magick, args, cx, budget)?;
    ensure_file(out, "the image decoder")
}

// ---------------------------------------------------------------------- helpers

fn magick_path(cx: &Context<'_>) -> Result<PathBuf, JobError> {
    cx.tools.path(ToolKey::Magick)
}

fn input_format(job: &Job) -> Result<&'static Format, JobError> {
    job.input_format
        .and_then(Registry::get)
        .or_else(|| Registry::detect(&job.source))
        .ok_or_else(|| JobError::new(ErrorCode::Unsupported, "unknown image input format"))
}

/// Source extension for tool output; SVG is read-only, so it becomes PNG.
fn output_ext(job: &Job) -> &'static str {
    match input_format(job) {
        Ok(format) if format.write && format.id != FormatId("SVG") => format.canonical_ext(),
        _ => "png",
    }
}

fn stem(job: &Job) -> String {
    job.source
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .filter(|s| !s.is_empty())
        .unwrap_or_else(|| "image".to_string())
}

fn tool_out(job: &Job, cx: &Context<'_>, tool: ToolId) -> PathBuf {
    cx.out(&format!(
        "{}-{}.{}",
        stem(job),
        tool.as_str(),
        output_ext(job)
    ))
}

fn image_size(magick: &Path, input: &Path, cx: &Context<'_>) -> Result<(u32, u32), JobError> {
    let cmd = Cmd::new(magick)
        .arg("identify")
        .arg("-format")
        .arg("%w %h")
        .arg(input)
        .stdout_piped()
        .budget(Budget::Short);
    let output = proc::run(cmd, cx.ctx)?;
    let output = output.ok()?;
    let mut parts = output.stdout.split_whitespace();
    let width: u32 = parts
        .next()
        .and_then(|v| v.parse().ok())
        .ok_or_else(|| JobError::new(ErrorCode::EngineFailed, "could not read the image width"))?;
    let height: u32 = parts
        .next()
        .and_then(|v| v.parse().ok())
        .ok_or_else(|| JobError::new(ErrorCode::EngineFailed, "could not read the image height"))?;
    if width == 0 || height == 0 {
        return Err(JobError::new(
            ErrorCode::EngineFailed,
            "the image has no pixels",
        ));
    }
    Ok((width, height))
}

fn run_magick(
    magick: &Path,
    args: Vec<OsString>,
    cx: &Context<'_>,
    budget: Budget,
) -> Result<(), JobError> {
    let output = proc::run(Cmd::new(magick).args(args).budget(budget), cx.ctx)?;
    output.ok()?;
    Ok(())
}

fn file_len(path: &Path) -> Result<u64, JobError> {
    std::fs::metadata(path)
        .map(|meta| meta.len())
        .map_err(|e| JobError::io(format!("reading the size of {}", path.display()), &e))
}

fn move_into_place(from: &Path, to: &Path) -> Result<(), JobError> {
    std::fs::rename(from, to)
        .or_else(|e| {
            if e.kind() == std::io::ErrorKind::CrossesDevices {
                std::fs::copy(from, to)
                    .and_then(|_| std::fs::remove_file(from))
                    .map(|_| ())
            } else {
                Err(e)
            }
        })
        .map_err(|e| JobError::io(format!("placing {}", to.display()), &e))
}

fn is_lossy(id: &str) -> bool {
    matches!(id, "JPG" | "WEBP" | "AVIF" | "HEIC")
}

fn is_lossy_ext(ext: &str) -> bool {
    matches!(ext, "jpg" | "jpeg" | "webp" | "avif" | "heic" | "heif")
}

fn parse_aspect(value: &str) -> Result<(f32, f32), JobError> {
    let (a, b) = value
        .split_once(':')
        .ok_or_else(|| unsupported(format!("unknown aspect ratio `{value}`")))?;
    let a: f32 = a
        .trim()
        .parse()
        .map_err(|_| unsupported(format!("unknown aspect ratio `{value}`")))?;
    let b: f32 = b
        .trim()
        .parse()
        .map_err(|_| unsupported(format!("unknown aspect ratio `{value}`")))?;
    if !a.is_finite() || !b.is_finite() || a <= 0.0 || b <= 0.0 {
        return Err(unsupported(format!("unknown aspect ratio `{value}`")));
    }
    Ok((a, b))
}

fn check_params(params: &ToolParams, allowed: &[&str]) -> Result<(), JobError> {
    for key in params.keys() {
        if !allowed.contains(&key.as_str()) {
            return Err(unsupported(format!("unknown parameter `{key}`")));
        }
    }
    Ok(())
}

fn number(params: &ToolParams, key: &str) -> Result<Option<f64>, JobError> {
    match params.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_f64()
            .filter(|v| v.is_finite())
            .map(Some)
            .ok_or_else(|| unsupported(format!("`{key}` must be a number"))),
    }
}

fn unsigned(params: &ToolParams, key: &str) -> Result<Option<u64>, JobError> {
    match params.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(value) => value
            .as_u64()
            .map(Some)
            .ok_or_else(|| unsupported(format!("`{key}` must be a whole number"))),
    }
}

fn string<'a>(params: &'a ToolParams, key: &str) -> Result<Option<&'a str>, JobError> {
    match params.get(key) {
        None | Some(Value::Null) => Ok(None),
        Some(Value::String(value)) => Ok(Some(value.as_str())),
        Some(_) => Err(unsupported(format!("`{key}` must be a string"))),
    }
}

fn os(value: impl AsRef<OsStr>) -> OsString {
    value.as_ref().to_os_string()
}

fn unsupported(message: impl Into<String>) -> JobError {
    JobError::new(ErrorCode::Unsupported, message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use convertme_core::job::{JobCtx, WorkDir};
    use serde_json::json;

    struct Fixture {
        _dir: tempfile::TempDir,
        tools: crate::Toolset,
        magick: PathBuf,
    }

    fn fixture() -> Option<Fixture> {
        let tools = crate::Toolset::probe_now();
        let magick = tools.path(ToolKey::Magick).ok()?;
        let dir = tempfile::tempdir().ok()?;
        Some(Fixture {
            _dir: dir,
            tools,
            magick,
        })
    }

    fn make_image(f: &Fixture, name: &str, args: Vec<OsString>) -> PathBuf {
        let path = f._dir.path().join(name);
        let mut cmd = Cmd::new(&f.magick);
        for arg in args {
            cmd = cmd.arg(arg);
        }
        let output = proc::run(cmd.arg(&path), &JobCtx::new()).expect("run magick");
        assert!(output.success, "magick failed: {}", output.stderr_tail);
        path
    }

    fn red_8x8(f: &Fixture, name: &str) -> PathBuf {
        make_image(f, name, vec![os("-size"), os("8x8"), os("xc:red")])
    }

    fn red_100x80(f: &Fixture, name: &str) -> PathBuf {
        make_image(f, name, vec![os("-size"), os("100x80"), os("xc:red")])
    }

    fn noisy_400(f: &Fixture, name: &str) -> PathBuf {
        make_image(
            f,
            name,
            vec![
                os("-size"),
                os("400x400"),
                os("xc:gray"),
                os("+noise"),
                os("Random"),
            ],
        )
    }

    fn run_engine(f: &Fixture, job: &Job) -> Result<Vec<PathBuf>, JobError> {
        let work = WorkDir::create(f._dir.path().join(format!("work-{}", job.id)))
            .expect("create the work dir");
        let ctx = JobCtx::new();
        let cx = Context {
            tools: &f.tools,
            work: &work,
            ctx: &ctx,
        };
        run(job, &cx)
    }

    fn params(value: Value) -> ToolParams {
        value.as_object().cloned().expect("object params")
    }

    #[test]
    fn incomplete_crop_is_rejected() {
        let job = Job::new(
            PathBuf::from("/tmp/image.png"),
            Intent::Tool {
                tool: ToolId::Crop,
                params: params(json!({ "w": 0.5 })),
            },
        );
        let err = validate(&job, Category::Images).expect_err("must reject");
        assert_eq!(err.code, ErrorCode::Unsupported);
    }

    #[test]
    fn unknown_parameter_is_rejected() {
        let job = Job::new(
            PathBuf::from("/tmp/image.png"),
            Intent::Tool {
                tool: ToolId::Crop,
                params: params(json!({ "width": 10 })),
            },
        );
        let err = validate(&job, Category::Images).expect_err("must reject");
        assert_eq!(err.code, ErrorCode::Unsupported);
    }

    /// A pixel as 0..255 channels, sampled with the same tool the engine uses.
    fn pixel(f: &Fixture, path: &Path, x: u32, y: u32) -> (u8, u8, u8) {
        let cmd = Cmd::new(&f.magick)
            .arg(format!("{}[1x1+{x}+{y}]", path.display()))
            .arg("-format")
            .arg("%[fx:int(255*r)] %[fx:int(255*g)] %[fx:int(255*b)]")
            .arg("info:")
            .stdout_piped()
            .budget(Budget::Short);
        let output = proc::run(cmd, &JobCtx::new()).expect("run magick");
        assert!(output.success, "magick failed: {}", output.stderr_tail);
        let mut parts = output.stdout.split_whitespace();
        (
            parts.next().unwrap_or("0").parse().unwrap_or(0),
            parts.next().unwrap_or("0").parse().unwrap_or(0),
            parts.next().unwrap_or("0").parse().unwrap_or(0),
        )
    }

    fn solid(f: &Fixture, name: &str, size: &str, colour: &str) -> PathBuf {
        make_image(
            f,
            name,
            vec![os("-size"), os(size), os(format!("xc:{colour}"))],
        )
    }

    #[test]
    fn add_background_padding_and_radius_are_proportional() {
        let Some(f) = fixture() else { return };
        let input = solid(&f, "pad.png", "400x300", "red");
        let job = Job::new(
            input,
            Intent::Tool {
                tool: ToolId::AddBackground,
                params: params(json!({
                    "color": "#00ff00",
                    "padding_pct": 10,
                    "radius_pct": 0,
                })),
            },
        );
        let artifacts = run_engine(&f, &job).expect("compose");
        let (w, h) = image_size(
            &f.magick,
            &artifacts[0],
            &Context {
                tools: &f.tools,
                work: &WorkDir::create(f._dir.path().join("size-work")).expect("work dir"),
                ctx: &JobCtx::new(),
            },
        )
        .expect("size");
        // 10% of the shorter side (300) on every side.
        assert_eq!((w, h), (460, 360));
        assert_eq!(
            pixel(&f, &artifacts[0], 2, 2),
            (0, 255, 0),
            "canvas corner is the background"
        );
    }

    #[test]
    fn add_background_rounds_the_photo_not_the_canvas() {
        let Some(f) = fixture() else { return };
        let input = solid(&f, "round.png", "400x300", "red");
        let job = Job::new(
            input,
            Intent::Tool {
                tool: ToolId::AddBackground,
                params: params(json!({
                    "color": "#00ff00",
                    "padding_pct": 20,
                    "radius_pct": 100,
                })),
            },
        );
        let artifacts = run_engine(&f, &job).expect("compose");
        // Canvas corner: background, not cut away.
        assert_eq!(pixel(&f, &artifacts[0], 2, 2), (0, 255, 0));
        // Photo corner (inside the rounded shape): background shows through.
        assert_eq!(pixel(&f, &artifacts[0], 62, 62), (0, 255, 0));
        // Photo centre: the photo.
        let (r, g, _b) = pixel(&f, &artifacts[0], 230, 180);
        assert!(r > 200 && g < 60, "centre should be the photo, got {r}/{g}");
    }

    #[test]
    fn add_background_image_stays_inside_the_frame_at_any_placement() {
        let Some(f) = fixture() else { return };
        let input = solid(&f, "shot.png", "400x300", "red");
        let backdrop = solid(&f, "backdrop.png", "100x100", "blue");
        for (x, y) in [(0, 0), (-100, -100), (100, 100)] {
            let job = Job::new(
                input.clone(),
                Intent::Tool {
                    tool: ToolId::AddBackground,
                    params: params(json!({
                        "image": backdrop.to_string_lossy(),
                        "bg_zoom": 2,
                        "bg_x": x,
                        "bg_y": y,
                        "padding_pct": 10,
                    })),
                },
            );
            let artifacts = run_engine(&f, &job).expect("compose");
            // Opaque everywhere: a clipped crop used to leave transparent wedges.
            let (r, g, b) = pixel(&f, &artifacts[0], 1, 1);
            assert!(
                r > 0 || g > 0 || b > 0,
                "corner is transparent at {x}/{y}: {r}/{g}/{b}"
            );
        }
    }

    #[test]
    fn add_background_shadow_darkens_the_band_below_the_photo() {
        let Some(f) = fixture() else { return };
        let input = solid(&f, "shadow.png", "400x300", "red");
        let compose = |shadow: f64| {
            let job = Job::new(
                input.clone(),
                Intent::Tool {
                    tool: ToolId::AddBackground,
                    params: params(json!({
                        "color": "#ffffff",
                        "padding_pct": 20,
                        "shadow_pct": shadow,
                    })),
                },
            );
            run_engine(&f, &job).expect("compose")
        };
        let plain = compose(0.0);
        let shadowed = compose(10.0);
        // Just below the photo, in the padding band (padding is 60 px, the photo ends
        // at y=360 in a 420 px canvas).
        // A few pixels under the photo edge: the shadow fades before the band ends.
        let (pr, _, _) = pixel(&f, &plain[0], 260, 366);
        let (sr, _, _) = pixel(&f, &shadowed[0], 260, 366);
        assert_eq!(pr, 255, "the band starts as the background");
        assert!(sr < pr - 5, "the shadow should darken it: {sr} vs {pr}");
    }

    #[test]
    fn png_to_webp() {
        let Some(f) = fixture() else { return };
        let input = red_8x8(&f, "swatch.png");
        let job = Job::new(
            input,
            Intent::Convert {
                target: FormatId("WEBP"),
            },
        );
        let artifacts = run_engine(&f, &job).expect("convert");
        assert_eq!(artifacts.len(), 1);
        let bytes = std::fs::read(&artifacts[0]).expect("read output");
        assert_eq!(&bytes[0..4], b"RIFF");
        assert_eq!(&bytes[8..12], b"WEBP");
    }

    #[test]
    fn png_to_pdf_starts_with_pdf_header() {
        let Some(f) = fixture() else { return };
        let input = red_8x8(&f, "page.png");
        let job = Job::new(
            input,
            Intent::Convert {
                target: FormatId("PDF"),
            },
        );
        let artifacts = run_engine(&f, &job).expect("write pdf");
        assert_eq!(artifacts.len(), 1);
        let bytes = std::fs::read(&artifacts[0]).expect("read output");
        assert!(bytes.starts_with(b"%PDF-"), "missing PDF header");
    }

    #[test]
    fn multi_image_pdf_has_one_page_per_input() {
        let Some(f) = fixture() else { return };
        let first = red_8x8(&f, "one.png");
        let second = red_100x80(&f, "two.png");
        let job = Job::with_inputs(
            vec![first, second],
            Intent::Convert {
                target: FormatId("PDF"),
            },
        );
        let artifacts = run_engine(&f, &job).expect("write pdf");
        assert_eq!(artifacts.len(), 1);
        let bytes = std::fs::read(&artifacts[0]).expect("read output");
        assert!(bytes.starts_with(b"%PDF-"));
        let parsed = lopdf::Document::load(&artifacts[0]).expect("parse the PDF");
        assert_eq!(parsed.get_pages().len(), 2);
    }

    #[test]
    fn compress_target_bytes_shrinks() {
        let Some(f) = fixture() else { return };
        let input = noisy_400(&f, "noise.png");
        let original = std::fs::metadata(&input).expect("metadata").len();
        let target = (original / 4).max(1_000);
        let job = Job::new(
            input,
            Intent::Tool {
                tool: ToolId::Compress,
                params: params(json!({ "target_bytes": target })),
            },
        );
        let artifacts = run_engine(&f, &job).expect("compress");
        assert_eq!(artifacts.len(), 1);
        let shrunk = std::fs::metadata(&artifacts[0]).expect("metadata").len();
        assert!(
            shrunk < original,
            "compressed {shrunk} is not smaller than {original}"
        );
    }

    #[test]
    fn crop_changes_dimensions() {
        let Some(f) = fixture() else { return };
        let input = red_100x80(&f, "wide.png");
        let job = Job::new(
            input,
            Intent::Tool {
                tool: ToolId::Crop,
                params: params(json!({ "x": 0.1, "y": 0.1, "w": 0.5, "h": 0.5 })),
            },
        );
        let artifacts = run_engine(&f, &job).expect("crop");
        assert_eq!(artifacts.len(), 1);
        let (width, height) = ::image::image_dimensions(&artifacts[0]).expect("decode");
        assert_eq!((width, height), (50, 40));
    }

    #[test]
    fn hostile_filename_converts() {
        let Some(f) = fixture() else { return };
        // Windows forbids a double quote in a file name; a single quote still
        // exercises the quoting path on that platform.
        let name = if cfg!(windows) {
            "sp ace 'quote'.png"
        } else {
            "sp ace \"quote\".png"
        };
        let input = red_8x8(&f, name);
        let job = Job::new(
            input,
            Intent::Convert {
                target: FormatId("WEBP"),
            },
        );
        let artifacts = run_engine(&f, &job).expect("convert hostile filename");
        assert_eq!(artifacts.len(), 1);
        assert!(artifacts[0].is_file());
    }
}
