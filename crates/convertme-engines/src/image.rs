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
use convertme_core::job::{Intent, Job, JobCtx, ToolParams};
use printpdf::{
    DictItem, ExternalStream, ExternalXObject, Mm, Op, PdfDocument, PdfPage, PdfSaveOptions, Pt,
    Px, RawImage, RawImageData, RawImageFormat, XObjectId, XObjectTransform,
};
use serde::Serialize;
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
            check_params(
                params,
                &["quality", "scale", "percent", "target_bytes", "colours"],
            )?;
            let quality = unsigned(params, "quality")?;
            let scale = number(params, "scale")?;
            let percent = number(params, "percent")?;
            // Zero means "no target": the field is optional and starts empty.
            let _ = unsigned(params, "target_bytes")?;
            let colours = unsigned(params, "colours")?;
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
            if let Some(c) = colours
                && !(2..=256).contains(&c)
            {
                return Err(unsupported("`colours` must be between 2 and 256"));
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
        ToolId::Collage => {
            check_params(
                params,
                &[
                    "style",
                    "rows",
                    "columns",
                    "aspect",
                    "width",
                    "fit",
                    "spacing_pct",
                    "radius_pct",
                    "background",
                    "order",
                    "cells",
                ],
            )?;
            let style = string(params, "style")?.unwrap_or("grid");
            if !["grid", "row", "column", "featured"].contains(&style) {
                return Err(unsupported("`style` must be grid, row, column or featured"));
            }
            if let Some(rows) = unsigned(params, "rows")?
                && rows > 6
            {
                return Err(unsupported("`rows` must be between 0 and 6"));
            }
            if let Some(columns) = unsigned(params, "columns")?
                && !(1..=6).contains(&columns)
            {
                return Err(unsupported("`columns` must be between 1 and 6"));
            }
            if let Some(aspect) = string(params, "aspect")?
                && !aspect.eq_ignore_ascii_case("auto")
                && !aspect.is_empty()
            {
                parse_aspect(aspect)?;
            }
            if let Some(width) = unsigned(params, "width")?
                && !(512..=8192).contains(&width)
            {
                return Err(unsupported("`width` must be between 512 and 8192"));
            }
            if let Some(fit) = string(params, "fit")?
                && !["crop", "contain"].contains(&fit)
            {
                return Err(unsupported("`fit` must be crop or contain"));
            }
            for key in ["spacing_pct", "radius_pct"] {
                if let Some(value) = number(params, key)?
                    && !(0.0..=100.0).contains(&value)
                {
                    return Err(unsupported(format!("`{key}` must be between 0 and 100")));
                }
            }
            collage_transform_entries(params)?;
            if let Some(background) = string(params, "background")?
                && background.trim().is_empty()
            {
                return Err(unsupported("`background` cannot be empty"));
            }
            if let Some(value) = params.get("order")
                && !value.is_array()
                && !value.is_null()
            {
                return Err(unsupported("`order` must be an array of indices"));
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
                    "bg_blur",
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
            if let Some(v) = number(params, "bg_blur")?
                && (!v.is_finite() || !(0.0..=50.0).contains(&v))
            {
                return Err(unsupported("`bg_blur` is pixels from 0 to 50"));
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
                && (!v.is_finite() || !(0.0..=100.0).contains(&v))
            {
                return Err(unsupported("`shadow_pct` is a percentage from 0 to 100"));
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
                    "highlights",
                    "shadows",
                    "whites",
                    "blacks",
                    "tint",
                    "vibrance",
                    "clarity",
                    "dehaze",
                    "grain",
                    "noise",
                    "sharpness",
                ],
            )?;
            let exposure = number(params, "exposure")?.unwrap_or(0.0);
            if !(-5.0..=5.0).contains(&exposure) {
                return Err(unsupported("`exposure` must be between -5 and 5 stops"));
            }
            for key in [
                "brightness",
                "contrast",
                "saturation",
                "temperature",
                "highlights",
                "shadows",
                "whites",
                "blacks",
                "tint",
                "vibrance",
                "clarity",
                "dehaze",
            ] {
                if let Some(value) = number(params, key)?
                    && !(-100.0..=100.0).contains(&value)
                {
                    return Err(unsupported(format!("`{key}` must be between -100 and 100")));
                }
            }
            // Hue is the one colour control in degrees, and the field spans a half
            // turn: rejecting +120 here made half the slider dead.
            if let Some(value) = number(params, "hue")?
                && !(-180.0..=180.0).contains(&value)
            {
                return Err(unsupported("`hue` must be between -180 and 180 degrees"));
            }
            for key in ["grain", "noise", "sharpness"] {
                if let Some(value) = number(params, key)?
                    && !(0.0..=100.0).contains(&value)
                {
                    return Err(unsupported(format!("`{key}` must be between 0 and 100")));
                }
            }
            if let Some(gamma) = number(params, "gamma")?
                && !(0.1..=10.0).contains(&gamma)
            {
                return Err(unsupported("`gamma` must be between 0.1 and 10"));
            }
            let neutral = |key: &str| -> Result<bool, JobError> {
                Ok(number(params, key)?.unwrap_or(0.0) == 0.0)
            };
            let mut touched = exposure != 0.0;
            for key in [
                "brightness",
                "contrast",
                "saturation",
                "hue",
                "temperature",
                "highlights",
                "shadows",
                "whites",
                "blacks",
                "tint",
                "vibrance",
                "clarity",
                "dehaze",
                "grain",
                "noise",
                "sharpness",
            ] {
                touched = touched || !neutral(key)?;
            }
            if !touched && number(params, "gamma")?.is_none() {
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
        ToolId::Collage => collage(job, cx, params),
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
    let ext = output_ext(job);
    let out = tool_out(job, cx, ToolId::Compress);

    // The dialog encodes the exact same request for its preview; reuse those bytes
    // instead of compressing the file twice.
    if let Some(hit) = compress_cache_hit(cx, &job.source, params, ext) {
        cx.ctx.set_stage("using the previewed result");
        copy_file(&hit, &out)?;
        ensure_file(&out, "the image encoder")?;
        cx.ctx.set_progress(1.0);
        return Ok(vec![out]);
    }

    let produced = produce_compress(job, cx, params, ext)?;
    if let Some(cache) = cx.preview {
        // A cache that cannot be written must not fail a job that already succeeded.
        let _ = cache.store_artifact(
            &job.source,
            "compress",
            &compress_signature(params, ext),
            ext,
            &produced,
        );
    }
    move_into_place(&produced, &out)?;
    ensure_file(&out, "the image encoder")?;
    cx.ctx.set_progress(1.0);
    Ok(vec![out])
}

/// Bump when the encoder output for the same request changes: the key otherwise only
/// covers the source file and the parameters, so an old preview would be reused.
/// v5: PNG target searches use a dedicated palette quantizer, and a small palette falls back to a flat
/// remap when dithering made it larger than the lossless encode.
/// v6: the optimizer runs before the never-grow comparison, and a resize is its own
/// artifact — both change the bytes a given request produces.
/// v7: the palette quantizer and error-diffusion implementation changed.
const COMPRESS_CACHE_VERSION: &str = "v7";

/// One cache identity per compression request: the output extension plus every
/// parameter, sorted so the same request always renders the same string.
fn compress_signature(params: &ToolParams, ext: &str) -> String {
    let mut keys: Vec<_> = params.iter().collect();
    keys.sort_by(|a, b| a.0.cmp(b.0));
    let rendered: Vec<String> = keys
        .into_iter()
        .map(|(key, value)| format!("{key}={value}"))
        .collect();
    format!("{COMPRESS_CACHE_VERSION}|{ext}|{}", rendered.join(","))
}

/// The artifact the dialog already encoded for this exact request, when it is still
/// on disk. The key carries the source's mtime and size, so an edited file misses.
fn compress_cache_hit(
    cx: &Context<'_>,
    source: &Path,
    params: &ToolParams,
    ext: &str,
) -> Option<PathBuf> {
    let cache = cx.preview?;
    let path = cache.artifact_key(source, "compress", &compress_signature(params, ext), ext)?;
    path.is_file().then_some(path)
}

/// Encode one compression request into the work dir and return the artifact.
fn encode_compress(job: &Job, cx: &Context<'_>, params: &ToolParams) -> Result<PathBuf, JobError> {
    let magick = magick_path(cx)?;
    let ext = output_ext(job);
    let lossy = is_lossy_ext(ext);

    // Three independent dials, as the tool promises: encode quality, a size cap, and
    // an optional resize. `percent` is the old spelling of "scale down by this much".
    let quality = unsigned(params, "quality")?.map(|q| q.clamp(1, 100) as u32);
    let scale = match number(params, "scale")? {
        Some(scale) => Some(scale),
        None => number(params, "percent")?.map(|percent| (100.0 - percent).max(5.0)),
    };
    let target = unsigned(params, "target_bytes")?.filter(|t| *t > 0);
    let colours = unsigned(params, "colours")?.map(|c| c.clamp(2, 256) as u32);

    if let Some(target) = target {
        cx.ctx.set_stage("compressing to the target size");
        return TargetSearch {
            magick: &magick,
            source: &job.source,
            target,
            ext,
            lossy,
            scale,
            colours,
        }
        .run(cx);
    }

    cx.ctx.set_stage("compressing the image");
    cx.ctx.set_progress(0.05);
    let out = cx.scratch(&format!("compress-result.{ext}"));
    let mut args = vec![os(&job.source)];
    if let Some(scale) = scale {
        args.push(os("-resize"));
        args.push(os(format!("{scale:.2}%")));
    }
    if lossy {
        args.push(os("-quality"));
        args.push(os(quality.unwrap_or(92).to_string()));
    } else {
        // A plain re-encode uses the encoder defaults and often changes nothing. Ask
        // for the format's best lossless settings: PNG zlib level 9 (~2–3 % on an
        // ordinary PNG), TIFF LZW (~half of an uncompressed TIFF). BMP has no
        // lossless compression to ask for.
        match ext {
            "png" => args.extend([os("-define"), os("png:compression-level=9")]),
            "tif" | "tiff" => args.extend([os("-compress"), os("lzw")]),
            _ => {}
        }
    }
    args.push(os(&out));
    run_magick(&magick, args, cx, Budget::Short)?;
    ensure_file(&out, "the image encoder")?;
    Ok(out)
}

/// One compression pipeline for both the preview and the job, so the two cannot
/// disagree: encode, optionally reduce the palette, refuse to grow the file, then let
/// a real PNG optimizer try the row filters and deflate strategies.
fn produce_compress(
    job: &Job,
    cx: &Context<'_>,
    params: &ToolParams,
    ext: &str,
) -> Result<PathBuf, JobError> {
    let colours = unsigned(params, "colours")?;
    if colours.is_some() && ext != "png" {
        return Err(unsupported("reducing colours needs a PNG output"));
    }
    let target = unsigned(params, "target_bytes")?.filter(|t| *t > 0);

    let encoded = encode_compress(job, cx, params)?;
    // The target search already quantizes PNGs: it must, or a 1 KB target would be
    // judged against ImageMagick's weak `-colors` and return a larger file than a
    // plain palette pass. Re-quantizing that result would only lose quality.
    let reduced = if target.is_some() {
        encoded
    } else {
        reduce_colours(encoded, ext, params, cx)?
    };
    // Optimize *before* the never-grow comparison. The other order compared the raw
    // encoder output with the source, and oxipng's lossless pass then ran on whatever
    // survived — so a screenshot whose unoptimized encode was larger than the source
    // was replaced by the source even though row filters would have halved it.
    let optimized = optimize_png(reduced, ext, cx)?;
    prefer_smaller(job, optimized, ext, cx)
}

/// Lossy palette reduction with the MIT-licensed NeuQuant implementation, writing an
/// indexed PNG. Absent `colours` leaves the encoder output untouched: the default stays
/// lossless. ImageMagick's own `-colors` is deliberately not used.
fn reduce_colours(
    path: PathBuf,
    ext: &str,
    params: &ToolParams,
    cx: &Context<'_>,
) -> Result<PathBuf, JobError> {
    let Some(colours) = unsigned(params, "colours")?.map(|c| c.clamp(2, 256) as u32) else {
        return Ok(path);
    };
    if ext != "png" {
        return Err(unsupported("reducing colours needs a PNG output"));
    }

    let baseline = file_len(&path)?;
    let dithered = cx.scratch("compress-reduced.png");
    quantize_png(&path, colours, 1.0, &dithered, cx)?;
    if file_len(&dithered)? < baseline {
        return Ok(dithered);
    }
    // Dithering is what makes a small palette exceed the lossless encode on a smooth
    // gradient (16 colours on the poster did). A flat remap is worse to look at but
    // actually smaller, which is the point of a compressor. Keep whichever is smaller.
    let flat = cx.scratch("compress-reduced-flat.png");
    quantize_png(&path, colours, 0.0, &flat, cx)?;
    Ok(if file_len(&flat)? < file_len(&dithered)? {
        flat
    } else {
        dithered
    })
}

/// Quantize a PNG to an indexed palette and write it to `out`. `dithering` is 0–1;
/// 1.0 applies Floyd–Steinberg error diffusion and 0.0 picks the nearest palette colour.
fn quantize_png(
    input: &Path,
    colours: u32,
    dithering: f32,
    out: &Path,
    cx: &Context<'_>,
) -> Result<(), JobError> {
    cx.ctx.checkpoint()?;
    cx.ctx.set_stage("reducing the palette");
    let bytes = std::fs::read(input)
        .map_err(|e| JobError::io(format!("reading {}", input.display()), &e))?;
    let decoded = load_image(&bytes).map_err(|e| {
        JobError::new(
            ErrorCode::EngineFailed,
            format!("decoding the PNG for colour reduction: {e}"),
        )
    })?;
    // Palette quantization works on 8-bit RGBA; a 16-bit source is reduced here.
    let rgba = decoded.to_rgba8();
    encode_indexed_png(
        rgba.width(),
        rgba.height(),
        rgba.as_raw(),
        colours,
        dithering,
        out,
    )
}

fn encode_indexed_png(
    width: u32,
    height: u32,
    pixels: &[u8],
    colours: u32,
    dithering: f32,
    out: &Path,
) -> Result<(), JobError> {
    let quantizer = color_quant::NeuQuant::new(4, colours as usize, pixels);
    let palette = quantizer.color_map_rgba();
    let indices = remap_rgba(&quantizer, &palette, pixels, width, height, dithering);

    let mut rgb = Vec::with_capacity(palette.len() / 4 * 3);
    let mut alpha = Vec::with_capacity(palette.len() / 4);
    for entry in palette.as_chunks::<4>().0 {
        rgb.extend([entry[0], entry[1], entry[2]]);
        alpha.push(entry[3]);
    }
    // tRNS may stop at the last non-opaque entry; the rest is implicitly opaque.
    let transparency = alpha
        .iter()
        .rposition(|a| *a != 255)
        .map_or(0, |last| last + 1);

    let mut file = std::fs::File::create(out)
        .map_err(|e| JobError::io(format!("creating {}", out.display()), &e))?;
    let mut encoder = png::Encoder::new(&mut file, width, height);
    encoder.set_color(png::ColorType::Indexed);
    encoder.set_depth(png::BitDepth::Eight);
    encoder.set_palette(rgb);
    if transparency > 0 {
        encoder.set_trns(alpha[..transparency].to_vec());
    }
    encoder.set_compression(png::Compression::High);
    // Palette values retain the decoded sRGB channel values.
    encoder.set_source_srgb(png::SrgbRenderingIntent::Perceptual);
    let write_error = |e: png::EncodingError| {
        JobError::new(
            ErrorCode::EngineFailed,
            format!("writing the indexed PNG: {e}"),
        )
    };
    let mut writer = encoder.write_header().map_err(write_error)?;
    writer.write_image_data(&indices).map_err(write_error)?;
    writer.finish().map_err(write_error)?;
    Ok(())
}

/// Map RGBA pixels to a trained palette. Error diffusion runs in RGB so transparent
/// pixels do not spread hidden colour into their neighbours.
fn remap_rgba(
    quantizer: &color_quant::NeuQuant,
    palette: &[u8],
    pixels: &[u8],
    width: u32,
    height: u32,
    dithering: f32,
) -> Vec<u8> {
    let mut indices = Vec::with_capacity(width as usize * height as usize);
    if dithering <= 0.0 {
        indices.extend(
            pixels
                .as_chunks::<4>()
                .0
                .iter()
                .map(|pixel| quantizer.index_of(pixel) as u8),
        );
        return indices;
    }

    let mut current = vec![[0.0_f32; 3]; width as usize + 2];
    let mut next = vec![[0.0_f32; 3]; width as usize + 2];
    for y in 0..height as usize {
        for x in 0..width as usize {
            let offset = (y * width as usize + x) * 4;
            let source = &pixels[offset..offset + 4];
            let adjusted = [
                (source[0] as f32 + current[x + 1][0])
                    .round()
                    .clamp(0.0, 255.0) as u8,
                (source[1] as f32 + current[x + 1][1])
                    .round()
                    .clamp(0.0, 255.0) as u8,
                (source[2] as f32 + current[x + 1][2])
                    .round()
                    .clamp(0.0, 255.0) as u8,
                source[3],
            ];
            let index = quantizer.index_of(&adjusted) as u8;
            indices.push(index);

            if source[3] == 0 {
                continue;
            }
            let palette_offset = index as usize * 4;
            let error = [
                (adjusted[0] as f32 - palette[palette_offset] as f32) * dithering,
                (adjusted[1] as f32 - palette[palette_offset + 1] as f32) * dithering,
                (adjusted[2] as f32 - palette[palette_offset + 2] as f32) * dithering,
            ];
            for channel in 0..3 {
                current[x + 2][channel] += error[channel] * (7.0 / 16.0);
                next[x][channel] += error[channel] * (3.0 / 16.0);
                next[x + 1][channel] += error[channel] * (5.0 / 16.0);
                next[x + 2][channel] += error[channel] * (1.0 / 16.0);
            }
        }
        std::mem::swap(&mut current, &mut next);
        next.fill([0.0; 3]);
    }
    indices
}

/// What the dialog shows before the user commits: the encoded file itself, with its
/// real size and dimensions. The artifact is stored in the shared cache, so the job
/// with the identical request copies it instead of encoding a second time.
pub struct CompressPreview {
    pub path: PathBuf,
    pub bytes: u64,
    pub width: u32,
    pub height: u32,
    pub source_bytes: u64,
}

pub fn preview_compress(job: &Job, cx: &Context<'_>) -> Result<CompressPreview, JobError> {
    let Intent::Tool { tool, params } = &job.intent else {
        return Err(unsupported("a compress preview needs a compress job"));
    };
    if *tool != ToolId::Compress {
        return Err(unsupported("only compress has a size preview"));
    }
    validate_tool(*tool, params)?;
    let cache = cx
        .preview
        .ok_or_else(|| JobError::new(ErrorCode::Io, "previews are not configured"))?;
    let ext = output_ext(job);
    let source_bytes = file_len(&job.source)?;

    let path = match compress_cache_hit(cx, &job.source, params, ext) {
        Some(hit) => hit,
        None => {
            let produced = produce_compress(job, cx, params, ext)?;
            cache.store_artifact(
                &job.source,
                "compress",
                &compress_signature(params, ext),
                ext,
                &produced,
            )?
        }
    };

    let magick = magick_path(cx)?;
    let (width, height) = image_size(&magick, &path, cx)?;
    let bytes = file_len(&path)?;
    Ok(CompressPreview {
        path,
        bytes,
        width,
        height,
        source_bytes,
    })
}

/// Quality search then downscale, at most three rounds. The smallest image the
/// engine produced is returned even when the target is unreachable — shrinking
/// as far as the format allows is still the useful result.
struct TargetSearch<'a> {
    magick: &'a Path,
    source: &'a Path,
    target: u64,
    ext: &'a str,
    lossy: bool,
    scale: Option<f64>,
    colours: Option<u32>,
}

impl TargetSearch<'_> {
    /// The smallest artifact this format can produce for this source.
    ///
    /// It is the ladder below restricted to its last helper — one palette-2 encode per
    /// downscale round instead of seven — because that is the artifact the ladder would
    /// end on. Reaching the same bytes by the shortest path is what makes an unreachable
    /// target cheap without changing a reachable one.
    fn floor_candidate(&self, cx: &Context<'_>) -> Result<Option<PathBuf>, JobError> {
        let Self {
            magick,
            source,
            ext,
            lossy,
            scale,
            ..
        } = *self;
        let mut current = source.to_path_buf();
        if let Some(scale) = scale {
            let resized = cx.scratch(&format!("compress-floor-scaled.{ext}"));
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
        for round in 0..3 {
            cx.ctx.checkpoint()?;
            let path = if ext == "png" {
                let path = cx.scratch(&format!("compress-floor-{round}.png"));
                quantize_png(&current, 2, 0.0, &path, cx)?;
                optimize_png(path, ext, cx)?
            } else if lossy {
                let path = cx.scratch(&format!("compress-floor-{round}.{ext}"));
                probe_quality(magick, &current, 1, &path, cx)?;
                path
            } else {
                current.clone()
            };
            let len = file_len(&path)?;
            track_smallest(&path, len, &mut smallest, &mut smallest_len);
            if round == 2 {
                break;
            }
            let scaled = cx.scratch(&format!("compress-floor-scale-{round}.{ext}"));
            let mut args = vec![os(&current), os("-resize"), os("85%")];
            if lossy {
                args.push(os("-quality"));
                args.push(os("95"));
            }
            args.push(os(&scaled));
            run_magick(magick, args, cx, Budget::Short)?;
            ensure_file(&scaled, "the image encoder")?;
            current = scaled;
        }
        Ok(smallest)
    }

    fn run(&self, cx: &Context<'_>) -> Result<PathBuf, JobError> {
        let Self {
            magick,
            source,
            target,
            ext,
            lossy,
            scale,
            colours,
        } = *self;
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
        // Floor first. The ladder below walks palette 256 → 2 across three downscales to
        // find the smallest image the format can make, which took thirteen seconds on a
        // poster to prove that a 1,000-byte target is unreachable. One pass at the floor
        // answers the same question: if even that is above the target, nothing in the
        // ladder can be below it, and the floor is the result.
        if let Some(floor) = self.floor_candidate(cx)? {
            let floor_len = file_len(&floor)?;
            if floor_len > target {
                return Ok(floor);
            }
        }

        let mut smallest: Option<PathBuf> = None;
        let mut smallest_len = u64::MAX;
        let mut probe = 0usize;

        for round in 0..3 {
            cx.ctx.checkpoint()?;
            // The lossless encoding of this round is a candidate too: a target above it
            // needs no palette at all, and it keeps the search from returning a lossy file
            // larger than the plain encode.
            let current_len = file_len(&current)?;
            track_smallest(&current, current_len, &mut smallest, &mut smallest_len);
            if current_len <= target {
                return Ok(current);
            }

            if !lossy {
                // A lossless format ignores `-quality`, so the only levers are the palette
                // and the dimensions. PNG gets the real quantizer, starting at the palette
                // the user chose (or 256). The palette pass also lets the target search
                // reduce colour count after lossless encoders have reached their floor.
                if ext == "png" {
                    let mut palette = colours.unwrap_or(256).clamp(2, 256);
                    loop {
                        let path = cx.scratch(&format!("compress-palette-{probe}.png"));
                        quantize_png(&current, palette, 0.0, &path, cx)?;
                        let path = optimize_png(path, ext, cx)?;
                        probe += 1;
                        let len = file_len(&path)?;
                        track_smallest(&path, len, &mut smallest, &mut smallest_len);
                        if len <= target {
                            return Ok(path);
                        }
                        if palette <= 2 {
                            break;
                        }
                        palette = (palette / 2).max(2);
                    }
                } else {
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

// --------------------------------------------------------------------- collage

/// How the cells are arranged.
#[derive(Clone, Copy, PartialEq, Eq)]
enum CollageStyle {
    Grid,
    Row,
    Column,
    Featured,
}

/// One placed cell: the source index, where it sits, and how many cell units it spans.
/// `input` indexes `inputs` (the source), while placement follows `order`.
struct CollageCell {
    input: usize,
    row: usize,
    col: usize,
    row_span: usize,
    col_span: usize,
}

/// One cell of the arrangement, in output pixels.
#[derive(Serialize, Clone, Debug, PartialEq, Eq)]
pub struct CollageCellOut {
    /// The source index (into the selection), so the dialog can match a cell to a file.
    pub input: usize,
    pub row: usize,
    pub col: usize,
    pub row_span: usize,
    pub col_span: usize,
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

/// The arrangement the composer will write. The dialog draws its hit areas from
/// these rectangles, so the preview and the job cannot disagree about the layout.
#[derive(Serialize, Clone, Debug)]
pub struct CollageLayout {
    pub width: u32,
    pub height: u32,
    pub cols: usize,
    pub rows: usize,
    pub spacing: u32,
    pub cells: Vec<CollageCellOut>,
}

/// How one picture sits inside its cell: `zoom` scales it before the crop and
/// `x`/`y` choose the window, both as percentages.
#[derive(Clone, Copy, Default)]
struct CollageTransform {
    zoom: f64,
    x: f64,
    y: f64,
}

fn collage_style(params: &ToolParams) -> Result<CollageStyle, JobError> {
    Ok(match string(params, "style")?.unwrap_or("grid") {
        "grid" => CollageStyle::Grid,
        "row" => CollageStyle::Row,
        "column" => CollageStyle::Column,
        "featured" => CollageStyle::Featured,
        other => {
            return Err(unsupported(format!(
                "unknown collage style `{other}`; use grid, row, column or featured"
            )));
        }
    })
}

/// The inputs in the order the dialog lists them. An untouched list is the selection
/// order; indices outside the selection and repeats are dropped, never applied.
fn collage_order(params: &ToolParams, count: usize) -> Result<Vec<usize>, JobError> {
    let mut order: Vec<usize> = Vec::new();
    match params.get("order") {
        None | Some(Value::Null) => {}
        Some(Value::Array(items)) => {
            for item in items {
                let Some(index) = item.as_u64().map(|value| value as usize) else {
                    return Err(unsupported("`order` entries must be indices"));
                };
                if index < count && !order.contains(&index) {
                    order.push(index);
                }
            }
        }
        Some(_) => return Err(unsupported("`order` must be an array of indices")),
    }
    if order.is_empty() {
        order.extend(0..count);
    }
    Ok(order)
}

fn collage_ratio(
    params: &ToolParams,
    magick: &Path,
    first: &Path,
    ctx: &JobCtx,
) -> Result<f64, JobError> {
    let aspect = string(params, "aspect")?.unwrap_or("auto");
    if aspect.eq_ignore_ascii_case("auto") || aspect.is_empty() {
        // The composer resizes after `-auto-orient`; the canvas ratio has to be the
        // picture's displayed ratio, or a rotated phone photo gets a portrait canvas
        // around a landscape picture.
        let (w, h) = oriented_size_with(magick, first, ctx)?;
        return Ok(f64::from(w.max(1)) / f64::from(h.max(1)));
    }
    let (a, b) = parse_aspect(aspect)?;
    Ok(f64::from(a) / f64::from(b))
}

/// The picture size after EXIF orientation, which is what the composer's
/// `-auto-orient -resize` sees. Pan offsets are a fraction of the real overflow.
fn oriented_size_with(magick: &Path, input: &Path, ctx: &JobCtx) -> Result<(u32, u32), JobError> {
    let cmd = Cmd::new(magick)
        .arg("identify")
        .arg("-auto-orient")
        .arg("-format")
        .arg("%w %h")
        .arg(input)
        .stdout_piped()
        .budget(Budget::Short);
    let output = proc::run(cmd, ctx)?;
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
    Ok((width.max(1), height.max(1)))
}

/// The cell grid. `row` is one row of N columns, `column` one column of N rows;
/// `featured` lets the first picture take a 2×2 block and fills the rest around it.
///
/// `rows` is the explicit row count (0 = automatic). It is honoured when it is
/// larger than the pictures need — the spare cells stay background — and it grows
/// when the pictures would not otherwise fit.
fn collage_cells(
    style: CollageStyle,
    columns: usize,
    rows: usize,
    order: &[usize],
) -> (usize, usize, Vec<CollageCell>) {
    let count = order.len();
    let columns = columns.clamp(1, 6);
    let requested = rows.min(6);
    let featured = style == CollageStyle::Featured && count >= 3 && columns >= 2;
    let (cols, rows) = if featured {
        let fit = ((count + 3).div_ceil(columns)).max(2);
        (columns, fit.max(requested))
    } else {
        match style {
            CollageStyle::Row => (count.max(1), 1),
            CollageStyle::Column => (1, count.max(1)),
            CollageStyle::Grid | CollageStyle::Featured => {
                let fit = count.div_ceil(columns).max(1);
                (columns, fit.max(requested).max(1))
            }
        }
    };

    let mut cells = Vec::with_capacity(count);
    if featured {
        cells.push(CollageCell {
            input: order[0],
            row: 0,
            col: 0,
            row_span: 2,
            col_span: 2,
        });
        let mut next = 1;
        for row in 0..rows {
            for col in 0..cols {
                if next >= count {
                    break;
                }
                if row < 2 && col < 2 {
                    continue;
                }
                cells.push(CollageCell {
                    input: order[next],
                    row,
                    col,
                    row_span: 1,
                    col_span: 1,
                });
                next += 1;
            }
        }
    } else {
        for (position, &input) in order.iter().enumerate() {
            cells.push(CollageCell {
                input,
                row: position / cols,
                col: position % cols,
                row_span: 1,
                col_span: 1,
            });
        }
    }
    (cols, rows, cells)
}

/// The gutter in pixels: a percentage of the smaller cell side, computed before the
/// gutter is taken out. 100% leaves one pixel per cell rather than swallowing them.
fn collage_spacing(
    params: &ToolParams,
    width: u32,
    height: u32,
    cols: usize,
    rows: usize,
) -> Result<u32, JobError> {
    let pct = number(params, "spacing_pct")?
        .unwrap_or(0.0)
        .clamp(0.0, 100.0);
    if pct <= 0.0 {
        return Ok(0);
    }
    let base_w = width / cols.max(1) as u32;
    let base_h = height / rows.max(1) as u32;
    let wanted = (f64::from(base_w.min(base_h)) * pct / 100.0).round() as u32;
    let room_w = width.saturating_sub(cols as u32) / (cols as u32 + 1);
    let room_h = height.saturating_sub(rows as u32) / (rows as u32 + 1);
    Ok(wanted.min(room_w).min(room_h))
}

/// The corner radius in pixels: 100% is half the shorter cell side (a circle on a
/// square cell), so the slider means the same thing at every output size.
fn collage_radius(params: &ToolParams, cell_w: u32, cell_h: u32) -> Result<u32, JobError> {
    let pct = number(params, "radius_pct")?
        .unwrap_or(0.0)
        .clamp(0.0, 100.0);
    if pct <= 0.0 {
        return Ok(0);
    }
    Ok((f64::from(cell_w.min(cell_h)) * pct / 200.0).round() as u32)
}

/// The `cells` array: zoom and pan per source index, validated but not yet applied.
fn collage_transform_entries(
    params: &ToolParams,
) -> Result<Vec<(usize, CollageTransform)>, JobError> {
    let Some(value) = params.get("cells") else {
        return Ok(Vec::new());
    };
    if value.is_null() {
        return Ok(Vec::new());
    }
    let Some(items) = value.as_array() else {
        return Err(unsupported("`cells` must be an array"));
    };
    let mut entries = Vec::with_capacity(items.len());
    for item in items {
        let Some(object) = item.as_object() else {
            return Err(unsupported("`cells` entries must be objects"));
        };
        let index = object
            .get("index")
            .and_then(Value::as_u64)
            .ok_or_else(|| unsupported("`cells` entries need an `index`"))?
            as usize;
        let lookup = |key: &str| -> Result<f64, JobError> {
            match object.get(key) {
                None | Some(Value::Null) => Ok(0.0),
                Some(value) => value
                    .as_f64()
                    .filter(|v| v.is_finite())
                    .ok_or_else(|| unsupported(format!("`{key}` must be a number"))),
            }
        };
        let zoom = match object.get("zoom") {
            None | Some(Value::Null) => 1.0,
            Some(value) => value
                .as_f64()
                .filter(|v| v.is_finite())
                .ok_or_else(|| unsupported("`zoom` must be a number"))?,
        };
        let x = lookup("x")?;
        let y = lookup("y")?;
        if !(1.0..=4.0).contains(&zoom) {
            return Err(unsupported("`zoom` must be between 1 and 4"));
        }
        if !(-100.0..=100.0).contains(&x) || !(-100.0..=100.0).contains(&y) {
            return Err(unsupported("`x` and `y` must be between -100 and 100"));
        }
        entries.push((index, CollageTransform { zoom, x, y }));
    }
    Ok(entries)
}

/// Transforms indexed by source index; entries for pictures that are not selected
/// are ignored, so the dialog can keep a pan while the selection changes.
fn collage_transforms(
    params: &ToolParams,
    count: usize,
) -> Result<Vec<CollageTransform>, JobError> {
    let mut transforms = vec![CollageTransform::default(); count];
    for (index, transform) in collage_transform_entries(params)? {
        if index < count {
            transforms[index] = transform;
        }
    }
    Ok(transforms)
}

/// The arrangement at `width` output pixels. Shared by the composer, the 512 px
/// preview and the dialog's hit areas: one formula, so they cannot drift.
pub fn collage_layout(
    tools: &crate::Toolset,
    inputs: &[PathBuf],
    params: &ToolParams,
    width: u32,
) -> Result<CollageLayout, JobError> {
    let magick = tools.path(ToolKey::Magick)?;
    collage_layout_with(&magick, inputs, params, width, &JobCtx::new())
}

fn collage_layout_with(
    magick: &Path,
    inputs: &[PathBuf],
    params: &ToolParams,
    width: u32,
    ctx: &JobCtx,
) -> Result<CollageLayout, JobError> {
    if inputs.is_empty() {
        return Err(unsupported("a collage needs at least one image"));
    }
    let order = collage_order(params, inputs.len())?;
    if order.is_empty() {
        return Err(unsupported("a collage needs at least one image"));
    }
    let style = collage_style(params)?;
    let columns = unsigned(params, "columns")?.unwrap_or(2).clamp(1, 6) as usize;
    let rows_param = unsigned(params, "rows")?.unwrap_or(0).min(6) as usize;
    let requested = width.clamp(512, 8192);
    let width = requested - requested % 2;
    let ratio = collage_ratio(params, magick, &inputs[order[0]], ctx)?;
    if !ratio.is_finite() || ratio <= 0.0 {
        return Err(unsupported("the canvas ratio must be positive"));
    }
    let height = ((f64::from(width) / ratio).round() as u32).max(2);
    let height = height - height % 2;

    let (cols, rows, cells) = collage_cells(style, columns, rows_param, &order);
    let spacing = collage_spacing(params, width, height, cols, rows)?;
    let cell_w =
        ((i64::from(width) - i64::from(spacing) * (cols as i64 + 1)) / cols as i64).max(1) as u32;
    let cell_h =
        ((i64::from(height) - i64::from(spacing) * (rows as i64 + 1)) / rows as i64).max(1) as u32;

    let cells = cells
        .into_iter()
        .map(|cell| {
            let span_w = cell_w * cell.col_span as u32 + spacing * (cell.col_span as u32 - 1);
            let span_h = cell_h * cell.row_span as u32 + spacing * (cell.row_span as u32 - 1);
            let x = spacing + cell.col as u32 * (cell_w + spacing);
            let y = spacing + cell.row as u32 * (cell_h + spacing);
            CollageCellOut {
                input: cell.input,
                row: cell.row,
                col: cell.col,
                row_span: cell.row_span,
                col_span: cell.col_span,
                x,
                y,
                w: span_w,
                h: span_h,
            }
        })
        .collect();

    Ok(CollageLayout {
        width,
        height,
        cols,
        rows,
        spacing,
        cells,
    })
}

/// One `magick` invocation composes the whole canvas; the dialog's preview calls the
/// same function at 512 px, so what it shows is what the job writes.
fn compose_collage(
    magick: &Path,
    inputs: &[PathBuf],
    params: &ToolParams,
    width_override: Option<u32>,
    out: &Path,
    ctx: &JobCtx,
) -> Result<(), JobError> {
    let requested = width_override
        .or_else(|| unsigned(params, "width").ok().flatten().map(|w| w as u32))
        .unwrap_or(2048);
    // One layout, shared with the dialog: the composer cannot disagree with the
    // rectangles the preview draws over it.
    let layout = collage_layout_with(magick, inputs, params, requested, ctx)?;
    let fit = string(params, "fit")?.unwrap_or("crop");
    if !["crop", "contain"].contains(&fit) {
        return Err(unsupported("`fit` must be crop or contain"));
    }
    let background = string(params, "background")?
        .unwrap_or("#ffffff")
        .to_string();
    if background.trim().is_empty() {
        return Err(unsupported("`background` cannot be empty"));
    }
    let transforms = collage_transforms(params, inputs.len())?;
    // Source sizes are probed once per picture: the pan range is a fraction of the
    // real overflow, and `-resize …^` overflows by more than the target when the
    // picture's aspect does not match the cell's.
    let mut sizes: Vec<Option<(u32, u32)>> = vec![None; inputs.len()];

    let mut args: Vec<OsString> = vec![
        os("-size"),
        os(format!("{}x{}", layout.width, layout.height)),
        os(format!("xc:{background}")),
    ];
    ctx.set_stage("composing the collage");
    let total = layout.cells.len().max(1);
    for (position, cell) in layout.cells.iter().enumerate() {
        let span_w = cell.w;
        let span_h = cell.h;
        let transform = transforms.get(cell.input).copied().unwrap_or_default();
        let radius = collage_radius(params, span_w, span_h)?;

        args.push(os("("));
        args.push(os("("));
        args.push(os(&inputs[cell.input]));
        args.push(os("-auto-orient"));
        if fit == "crop"
            && (transform.zoom - 1.0).abs() < 1e-9
            && transform.x == 0.0
            && transform.y == 0.0
        {
            // The default cell: a centred cover. No source-size probe is needed, which
            // keeps an all-default preview at four Magick processes fewer.
            args.push(os("-resize"));
            args.push(os(format!("{span_w}x{span_h}^")));
            args.push(os("-gravity"));
            args.push(os("center"));
            args.push(os("-extent"));
            args.push(os(format!("{span_w}x{span_h}")));
        } else if fit == "crop" {
            // Zoom enlarges the picture before the crop; pan picks which part of the
            // overflow the cell shows. Both are percentages, so they mean the same
            // thing at any output size.
            let zoom = transform.zoom.clamp(1.0, 4.0);
            let resized_w = ((f64::from(span_w) * zoom).round() as u32).max(span_w);
            let resized_h = ((f64::from(span_h) * zoom).round() as u32).max(span_h);
            let (source_w, source_h) = match sizes[cell.input] {
                Some(size) => size,
                None => {
                    let size = oriented_size_with(magick, &inputs[cell.input], ctx)?;
                    sizes[cell.input] = Some(size);
                    size
                }
            };
            // `-resize …^` keeps the aspect ratio and covers the target, so the real
            // size is the target scaled up by the larger ratio (rounded the same way).
            let scale = (f64::from(resized_w) / f64::from(source_w))
                .max(f64::from(resized_h) / f64::from(source_h));
            let covered_w = ((f64::from(source_w) * scale).round() as u32).max(resized_w);
            let covered_h = ((f64::from(source_h) * scale).round() as u32).max(resized_h);
            let offset = |pan: f64, extra: u32| -> u32 {
                let half = f64::from(extra) / 2.0;
                (half + (pan.clamp(-100.0, 100.0) / 100.0) * half)
                    .round()
                    .clamp(0.0, f64::from(extra)) as u32
            };
            let ox = offset(transform.x, covered_w - span_w);
            let oy = offset(transform.y, covered_h - span_h);
            args.push(os("-resize"));
            args.push(os(format!("{resized_w}x{resized_h}^")));
            args.push(os("-gravity"));
            args.push(os("NorthWest"));
            args.push(os("-crop"));
            args.push(os(format!("{span_w}x{span_h}+{ox}+{oy}")));
            args.push(os("+repage"));
        } else {
            // Contain shows the whole picture: pan and zoom cannot reveal anything
            // that is not already visible, so the cell keeps the plain fit.
            args.push(os("-resize"));
            args.push(os(format!("{span_w}x{span_h}")));
            args.push(os("-background"));
            args.push(os(&background));
            args.push(os("-gravity"));
            args.push(os("center"));
            args.push(os("-extent"));
            args.push(os(format!("{span_w}x{span_h}")));
        }
        args.push(os(")"));
        if radius > 0 {
            // The mask belongs to the picture, not to the canvas: only the photo's
            // corners are cut, and the background shows through them.
            let r = radius.min(span_w.min(span_h) / 2).max(1);
            args.push(os("("));
            args.push(os("-size"));
            args.push(os(format!("{span_w}x{span_h}")));
            args.push(os("xc:black"));
            args.push(os("-fill"));
            args.push(os("white"));
            args.push(os("-draw"));
            args.push(os(format!(
                "roundrectangle 0,0 {},{} {r},{r}",
                span_w.saturating_sub(1),
                span_h.saturating_sub(1)
            )));
            args.push(os(")"));
            args.push(os("-alpha"));
            args.push(os("off"));
            args.push(os("-compose"));
            args.push(os("CopyOpacity"));
            args.push(os("-composite"));
        }
        args.push(os(")"));
        // `-gravity` is a persistent setting: the cell used `center` for its extent,
        // and leaving it set made `-geometry` place the overlay in the middle of the
        // canvas instead of at the cell's corner.
        args.push(os("-gravity"));
        args.push(os("NorthWest"));
        args.push(os("-compose"));
        args.push(os("Over"));
        args.push(os("-geometry"));
        args.push(os(format!("+{}+{}", cell.x, cell.y)));
        args.push(os("-composite"));
        ctx.set_progress((position + 1) as f64 / total as f64);
        if ctx.is_cancelled() {
            return Err(JobError::new(
                ErrorCode::Cancelled,
                "the collage was cancelled",
            ));
        }
    }
    args.push(os("-quality"));
    args.push(os("90"));
    args.push(os(out));

    let output = proc::run(Cmd::new(magick).args(args).budget(Budget::Long), ctx)?;
    output.ok()?;
    ensure_file(out, "the collage composer")?;
    ctx.set_progress(1.0);
    Ok(())
}

fn collage(job: &Job, cx: &Context<'_>, params: &ToolParams) -> Result<Vec<PathBuf>, JobError> {
    let magick = magick_path(cx)?;
    let inputs = if job.inputs.is_empty() {
        vec![job.source.clone()]
    } else {
        job.inputs.clone()
    };
    let out = tool_out(job, cx, ToolId::Collage);
    compose_collage(&magick, &inputs, params, None, &out, cx.ctx)?;
    Ok(vec![out])
}

/// The composition at preview size, rendered by the real engine into the cache.
pub fn collage_preview(
    tools: &crate::Toolset,
    cache: &crate::preview::PreviewCache,
    inputs: &[PathBuf],
    params: &ToolParams,
    out: &Path,
    width: u32,
) -> Result<(), JobError> {
    let magick = tools.path(ToolKey::Magick)?;
    let ctx = JobCtx::new();
    let _permit = cache.preview_permit();
    compose_collage(&magick, inputs, params, Some(width), out, &ctx)
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
        // Font family names are machine-specific: resolve the UI's generic families to
        // the actual faces this ImageMagick knows, once per process.
        let fonts = font_table(&magick, cx);
        // `-gravity` is a persistent setting and the caption above may have left it at
        // SouthWest. With an explicit corner gravity ImageMagick treats the `text`
        // primitive's y as the top of the line; resetting to the default keeps it a
        // baseline, which is what the normalised geometry assumes.
        args.push(os("-gravity"));
        args.push(os("Undefined"));
        args.push(os("-fill"));
        args.push(os("none"));
        args.push(os("-stroke"));
        args.push(os(&color));
        args.push(os("-strokewidth"));
        args.push(os("4"));
        for shape in &shapes {
            args.push(os("-draw"));
            args.push(os(draw_command(shape, width, height, &color, fonts)));
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
    Ellipse,
    Line,
    Highlight,
    Number,
    Text,
}

impl ShapeKind {
    fn parse(name: &str) -> Option<Self> {
        Some(match name {
            "rect" | "rectangle" => ShapeKind::Rect,
            "arrow" => ShapeKind::Arrow,
            "ellipse" | "circle" => ShapeKind::Ellipse,
            "line" | "pen" => ShapeKind::Line,
            "highlight" => ShapeKind::Highlight,
            "number" => ShapeKind::Number,
            "text" => ShapeKind::Text,
            _ => return None,
        })
    }
}

#[derive(Debug, Clone)]
struct DrawShape {
    kind: ShapeKind,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    /// Freehand and polyline points, normalised like `x`/`y`.
    points: Vec<(f32, f32)>,
    text: Option<String>,
    color: Option<String>,
    /// Stroke width in output pixels for drawn shapes, font size for `text`.
    size: Option<f32>,
    font: Option<FontFamily>,
}

/// The three families the dialog offers. The actual face is resolved from the machine's
/// ImageMagick font list; an unknown family is a parameter error, a missing face is not.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum FontFamily {
    Sans,
    Serif,
    Mono,
}

impl FontFamily {
    fn parse(name: &str) -> Option<Self> {
        Some(match name.trim().to_ascii_lowercase().as_str() {
            "sans" | "sans-serif" => FontFamily::Sans,
            "serif" => FontFamily::Serif,
            "mono" | "monospace" => FontFamily::Mono,
            _ => return None,
        })
    }

    /// The word a plain face of this family carries in ImageMagick's font names.
    fn marker(self) -> &'static str {
        match self {
            FontFamily::Sans => "sans",
            FontFamily::Serif => "serif",
            FontFamily::Mono => "mono",
        }
    }
}

/// Faces whose name carries one of these words are not plain: a badge or a caption
/// should look like the family, not like a weight variant of it.
const FONT_VARIANTS: &[&str] = &[
    "bold",
    "italic",
    "oblique",
    "light",
    "thin",
    "black",
    "medium",
    "semibold",
    "extrabold",
    "extra",
    "condensed",
];

#[derive(Debug, Default)]
struct FontTable {
    sans: Option<String>,
    serif: Option<String>,
    mono: Option<String>,
}

impl FontTable {
    fn name(&self, family: FontFamily) -> Option<&str> {
        match family {
            FontFamily::Sans => self.sans.as_deref(),
            FontFamily::Serif => self.serif.as_deref(),
            FontFamily::Mono => self.mono.as_deref(),
        }
    }
}

/// `magick -list font` prints one `Font: Name` line per installed face. The table is
/// built once: spawning `magick` on every preview would cost more than the draw.
static FONTS: std::sync::OnceLock<FontTable> = std::sync::OnceLock::new();

fn font_table(magick: &Path, cx: &Context<'_>) -> &'static FontTable {
    FONTS.get_or_init(|| load_fonts(magick, cx.ctx).unwrap_or_default())
}

fn load_fonts(magick: &Path, ctx: &JobCtx) -> Option<FontTable> {
    let output = proc::run(
        Cmd::new(magick)
            .arg("-list")
            .arg("font")
            .stdout_piped()
            .budget(Budget::Short),
        ctx,
    )
    .ok()?
    .ok()
    .ok()?;
    let names: Vec<String> = output
        .stdout
        .lines()
        .filter_map(|line| {
            line.strip_prefix("  Font: ")
                .or_else(|| line.strip_prefix("Font: "))
        })
        .map(|name| name.trim().to_string())
        .filter(|name| !name.is_empty())
        .collect();

    let pick = |family: FontFamily| -> Option<String> {
        let marker = family.marker();
        let mut plain: Vec<&String> = names
            .iter()
            .filter(|name| {
                let lower = name.to_ascii_lowercase();
                lower.contains(marker)
                    // "DejaVu-Sans-Mono" is a monospace face, not the plain sans.
                    && (family == FontFamily::Mono || !lower.contains("mono"))
                    && !FONT_VARIANTS.iter().any(|bad| lower.contains(bad))
            })
            .collect();
        // Keep the choice deterministic: the shortest name is the least decorated one.
        plain.sort_by_key(|name| (name.len(), (*name).clone()));
        plain.first().map(|name| (*name).clone())
    };

    Some(FontTable {
        sans: pick(FontFamily::Sans),
        serif: pick(FontFamily::Serif),
        mono: pick(FontFamily::Mono),
    })
}

/// `shapes` and `boxes` are arrays of shape objects; `arrow` is one such object.
/// All coordinates are fractions of the image, with the origin top-left, so the same
/// value draws correctly at preview size and at full resolution.
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

    if shapes.len() > 200 {
        return Err(unsupported("at most 200 shapes can be drawn"));
    }
    Ok(shapes)
}

fn shape_from_object(
    object: &serde_json::Map<String, Value>,
    kind: &str,
) -> Result<DrawShape, JobError> {
    let kind = ShapeKind::parse(kind).ok_or_else(|| {
        unsupported(format!(
            "unknown shape kind `{kind}`; use `rect`, `arrow`, `ellipse`, `line`, \
             `highlight`, `number` or `text`"
        ))
    })?;
    let read = |key: &str| -> Result<f32, JobError> {
        object
            .get(key)
            .and_then(Value::as_f64)
            .map(|v| v as f32)
            .filter(|v| v.is_finite())
            .ok_or_else(|| unsupported(format!("shape `{key}` must be a finite number")))
    };
    let optional = |key: &str| -> Option<f32> {
        object
            .get(key)
            .and_then(Value::as_f64)
            .map(|v| v as f32)
            .filter(|v| v.is_finite())
    };
    let inside = |v: f32| (0.0..=1.0).contains(&v);

    let x = read("x")?;
    let y = read("y")?;
    let mut w = optional("w").unwrap_or(0.0);
    let mut h = optional("h").unwrap_or(0.0);
    if !inside(x) || !inside(y) {
        return Err(unsupported(
            "shape `x`/`y` must be fractions between 0 and 1",
        ));
    }

    let mut points = Vec::new();
    if let Some(Value::Array(items)) = object.get("points") {
        for item in items {
            let pair = item
                .as_array()
                .filter(|pair| pair.len() >= 2)
                .ok_or_else(|| unsupported("shape `points` must be [x, y] pairs"))?;
            let px = pair[0]
                .as_f64()
                .map(|v| v as f32)
                .filter(|v| v.is_finite())
                .ok_or_else(|| unsupported("shape `points` must be numbers"))?;
            let py = pair[1]
                .as_f64()
                .map(|v| v as f32)
                .filter(|v| v.is_finite())
                .ok_or_else(|| unsupported("shape `points` must be numbers"))?;
            if !inside(px) || !inside(py) {
                return Err(unsupported(
                    "shape `points` must be fractions between 0 and 1",
                ));
            }
            points.push((px, py));
        }
    }

    let text = match object.get("text") {
        None | Some(Value::Null) => None,
        Some(Value::String(value)) => Some(value.clone()),
        Some(_) => return Err(unsupported("shape `text` must be a string")),
    };
    let color = match object.get("color") {
        None | Some(Value::Null) => None,
        Some(Value::String(value)) if !value.trim().is_empty() => Some(value.clone()),
        Some(_) => return Err(unsupported("shape `color` must be a colour string")),
    };
    let font = match object.get("font") {
        None | Some(Value::Null) => None,
        Some(Value::String(value)) => Some(FontFamily::parse(value).ok_or_else(|| {
            unsupported(format!(
                "unknown shape `font` `{value}`; use `sans`, `serif` or `mono`"
            ))
        })?),
        Some(_) => return Err(unsupported("shape `font` must be a string")),
    };
    let size = optional("size");
    // A stroke wider than 64 px is a fill, not a stroke. A caption is free-form: the
    // dialog offers sizes up to 2000 so a title can span a large photo. Number badges
    // derive their size from their box.
    let size_max = match kind {
        ShapeKind::Text => 2000.0,
        ShapeKind::Number => 200.0,
        _ => 64.0,
    };
    if let Some(size) = size
        && !(1.0..=size_max).contains(&size)
    {
        return Err(unsupported(format!(
            "shape `size` must be between 1 and {size_max:.0}"
        )));
    }

    match kind {
        ShapeKind::Rect | ShapeKind::Ellipse | ShapeKind::Highlight => {
            if w <= 0.0 || h <= 0.0 {
                return Err(unsupported(format!("{kind:?} `w`/`h` must be > 0")));
            }
            if !inside(x + w) || !inside(y + h) {
                return Err(unsupported("shape must lie inside the image"));
            }
        }
        ShapeKind::Number => {
            if w <= 0.0 || h <= 0.0 {
                return Err(unsupported("number `w`/`h` must be > 0"));
            }
            if !inside(x + w) || !inside(y + h) {
                return Err(unsupported("number must lie inside the image"));
            }
            if text.as_deref().map(str::trim).unwrap_or("").is_empty() {
                return Err(unsupported("a number shape needs `text`"));
            }
        }
        ShapeKind::Arrow => {
            if !inside(x + w) || !inside(y + h) {
                return Err(unsupported("arrow end point must lie inside the image"));
            }
        }
        ShapeKind::Line => {
            if points.len() >= 2 {
                // A polyline carries its own points; the box is a bounding hint.
            } else if !inside(x + w) || !inside(y + h) {
                return Err(unsupported("line end point must lie inside the image"));
            }
        }
        ShapeKind::Text => {
            let Some(value) = text.as_deref().map(str::trim) else {
                return Err(unsupported("a text shape needs `text`"));
            };
            if value.is_empty() {
                return Err(unsupported("a text shape needs `text`"));
            }
            if w < 0.0 || h < 0.0 {
                w = 0.0;
                h = 0.0;
            }
            if !inside(x + w) || !inside(y + h) {
                return Err(unsupported("text must lie inside the image"));
            }
        }
    }

    Ok(DrawShape {
        kind,
        x,
        y,
        w,
        h,
        points,
        text,
        color,
        size,
        font,
    })
}

/// One `-draw` string per shape. Fill, stroke and width are set inside the string so
/// each shape can carry its own colour without leaking into the next.
fn draw_command(
    shape: &DrawShape,
    width: u32,
    height: u32,
    default: &str,
    fonts: &FontTable,
) -> String {
    let colour = shape.color.as_deref().unwrap_or(default);
    let stroke = shape
        .size
        .filter(|_| shape.kind != ShapeKind::Text)
        .unwrap_or(4.0);
    // The face is optional: when the machine has no plain face for the family the
    // command is omitted and ImageMagick's default face draws the text anyway.
    let font = fonts
        .name(shape.font.unwrap_or(FontFamily::Sans))
        .map(|name| format!("font '{}' ", escape_draw(name)))
        .unwrap_or_default();
    let x1 = shape.x * width as f32;
    let y1 = shape.y * height as f32;
    let x2 = (shape.x + shape.w) * width as f32;
    let y2 = (shape.y + shape.h) * height as f32;
    match shape.kind {
        ShapeKind::Rect => format!(
            "fill none stroke {colour} stroke-width {stroke:.1} rectangle {x1:.2},{y1:.2} {x2:.2},{y2:.2}"
        ),
        ShapeKind::Ellipse => {
            let cx = (x1 + x2) / 2.0;
            let cy = (y1 + y2) / 2.0;
            let rx = (x2 - x1).abs() / 2.0;
            let ry = (y2 - y1).abs() / 2.0;
            format!(
                "fill none stroke {colour} stroke-width {stroke:.1} ellipse {cx:.2},{cy:.2} {rx:.2},{ry:.2} 0,360"
            )
        }
        ShapeKind::Highlight => {
            format!("fill {colour}80 stroke none rectangle {x1:.2},{y1:.2} {x2:.2},{y2:.2}")
        }
        ShapeKind::Arrow => {
            let angle = (y2 - y1).atan2(x2 - x1);
            // The head scales with the stroke: a fixed 48 px head is shorter than the
            // stroke itself at wide settings, and butt-capped lines meeting at a point
            // leave wedge-shaped gaps there. Round caps and joins close the tip at any
            // width. Bounded by the picture and by the arrow's own length.
            let length = ((x2 - x1).powi(2) + (y2 - y1).powi(2)).sqrt();
            let ceiling = (width.min(height) as f32 * 0.25).max(1.0);
            let head = (stroke * 3.5)
                .min(ceiling)
                .max(1.0)
                .min((length * 0.8).max(1.0));
            let ax = x2 - head * (angle + 0.45).cos();
            let ay = y2 - head * (angle + 0.45).sin();
            let bx = x2 - head * (angle - 0.45).cos();
            let by = y2 - head * (angle - 0.45).sin();
            format!(
                "fill none stroke {colour} stroke-width {stroke:.1} \
                 stroke-linecap round stroke-linejoin round \
                 line {x1:.2},{y1:.2} {x2:.2},{y2:.2} \
                 line {x2:.2},{y2:.2} {ax:.2},{ay:.2} \
                 line {x2:.2},{y2:.2} {bx:.2},{by:.2}"
            )
        }
        ShapeKind::Line => {
            if shape.points.len() >= 2 {
                let mut path = String::new();
                for (index, (px, py)) in shape.points.iter().enumerate() {
                    let command = if index == 0 { 'M' } else { 'L' };
                    path.push_str(&format!(
                        "{command} {:.2},{:.2} ",
                        px * width as f32,
                        py * height as f32
                    ));
                }
                format!(
                    "fill none stroke {colour} stroke-width {stroke:.1} path '{}'",
                    path.trim_end()
                )
            } else {
                format!(
                    "fill none stroke {colour} stroke-width {stroke:.1} line {x1:.2},{y1:.2} {x2:.2},{y2:.2}"
                )
            }
        }
        ShapeKind::Number => {
            let box_w = (x2 - x1).abs();
            let box_h = (y2 - y1).abs();
            let cx = (x1 + x2) / 2.0;
            let cy = (y1 + y2) / 2.0;
            let r = (box_w.min(box_h) / 2.0).max(1.0);
            // A badge is sized by its box, so resizing the box keeps the digit legible.
            // 0.55 of the diameter leaves the same breathing room at every size.
            let size = (box_w.min(box_h) * 0.55).clamp(8.0, 200.0);
            // Under `gravity center` the `text` primitive centres the whole text box on
            // the canvas centre plus the given offset. That centres one digit and two
            // digits alike, without guessing a font's ascent or digit advance.
            let tx = cx - width as f32 / 2.0;
            let ty = cy - height as f32 / 2.0;
            let label = shape.text.as_deref().unwrap_or("1");
            format!(
                "fill {colour} stroke none circle {cx:.2},{cy:.2} {:.2},{cy:.2} \
                 fill white stroke none {font}font-size {size:.1} \
                 gravity center text {tx:.2},{ty:.2} '{}' gravity NorthWest",
                cx + r,
                escape_draw(label)
            )
        }
        ShapeKind::Text => {
            // A caption is not a badge: it may be larger than the picture's short side,
            // and the dialog lets the user type any size up to this bound.
            let size = shape.size.unwrap_or(32.0).clamp(1.0, 2000.0);
            // `y` is the top of the line box; the glyph baseline sits 0.8 em below it.
            let ty = y1 + 0.8 * size;
            format!(
                "fill {colour} stroke none {font}font-size {size:.1} text {x1:.2},{ty:.2} '{}'",
                escape_draw(shape.text.as_deref().unwrap_or(""))
            )
        }
    }
}

/// Text goes inside a single-quoted `-draw` argument: the quote and the backslash are
/// the only two characters ImageMagick would read as syntax.
fn escape_draw(text: &str) -> String {
    text.replace('\\', "\\\\").replace('\'', "\\'")
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

    // A soft drop shadow, built from an explicit shape mask. The mask is drawn on a
    // canvas padded by three sigmas, so the blur has room: drawing it at the photo's own
    // size cut the tail and left a hard line with a halo around it.
    let shadow: Option<PathBuf> = {
        let shadow_pct = number(params, "shadow_pct")?
            .unwrap_or(0.0)
            .clamp(0.0, 100.0);
        // A shadow layer the size of a huge canvas does not fit the memory budget on
        // modest hardware. Dropping it keeps the job working; tiling is the real fix.
        let too_big = canvas_w as u64 * canvas_h as u64 > 60_000_000;
        if shadow_pct <= 0.0 || too_big {
            None
        } else {
            let angle = number(params, "shadow_angle")?
                .unwrap_or(180.0)
                .to_radians();
            // The shadow lives in the padding, so its size is relative to that: scaling it
            // by the photo made every percentage saturate the same narrow band.
            // Bounded: padding can be 100% of the shorter side, and a 500 px blur on a
            // large canvas took minutes. The mask is also built at most 1200 px wide.
            let sigma = (shadow_pct / 100.0 * f64::from(padding) * 0.8)
                .min(shortest * 0.20)
                .max(1.0);
            let offset = sigma * 1.0;
            // 0 degrees points up, 180 down: the shadow falls opposite the light.
            let dx = angle.sin() * offset;
            let dy = -angle.cos() * offset;
            // Half resolution keeps the falloff smooth (2x upscale of a wide blur) while
            // cutting the blur cost by four.
            // The shadow is a wide, soft gradient: 600 px on its longest side is plenty,
            // and it caps the blur cost regardless of padding or percentage.
            let scale = (600.0 / f64::from(width.max(height) + 2 * padding)).min(1.0);
            let small_w = f64::from(width) * scale;
            let small_h = f64::from(height) * scale;
            let sigma_s = sigma * scale;
            let dx_s = dx * scale;
            let dy_s = dy * scale;
            let reach = (sigma_s * 3.0).ceil();
            let canvas_w = small_w + 2.0 * (reach + dx_s.abs());
            let canvas_h = small_h + 2.0 * (reach + dy_s.abs());
            let x0 = (canvas_w - small_w) / 2.0 + dx_s;
            let y0 = (canvas_h - small_h) / 2.0 + dy_s;
            let x1 = x0 + small_w - 1.0;
            let y1 = y0 + small_h - 1.0;
            let r_s = f64::from(radius) * scale;
            let shape = if radius > 0 {
                format!("roundrectangle {x0:.0},{y0:.0} {x1:.0},{y1:.0} {r_s},{r_s}")
            } else {
                format!("rectangle {x0:.0},{y0:.0} {x1:.0},{y1:.0}")
            };

            let path = cx.scratch("photo-shadow.png");
            let args = vec![
                os("-depth"),
                os("16"),
                os("-size"),
                os(format!("{canvas_w:.0}x{canvas_h:.0}")),
                os("xc:black"),
                os("-fill"),
                os("white"),
                os("-draw"),
                os(&shape),
                os("-blur"),
                os(format!("0x{sigma_s:.1}")),
                // Density follows the slider as well as the spread, otherwise 2% and
                // 100% differ only in how far the same faint grey reaches.
                os("-evaluate"),
                os("multiply"),
                // Density has to be visible against a mid-grey background as well: at
                // 100% the core reaches 0.8 alpha.
                os(format!("{:.3}", 0.45 + 0.55 * shadow_pct / 100.0)),
                // The blurred grey has to become the alpha channel: CopyOpacity copies the
                // source's alpha, not its intensity.
                os("-alpha"),
                os("copy"),
                os("-channel"),
                os("RGB"),
                os("-fill"),
                os("black"),
                os("-colorize"),
                os("100"),
                os("+channel"),
                // 8 bits before the upscale: the big canvas is the memory cost, and a
                // smooth 16-bit gradient survives the conversion.
                os("-depth"),
                os("8"),
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
        // The cover window is centred in the picture: a 4:3 picture on a square canvas
        // throws away the left and right edges, not just the right one. `bg_x`/`bg_y`
        // slide inside that window, so the crop starts at the window's own origin.
        let (base_w, base_h, base_x, base_y) = if src_aspect > target {
            (
                f64::from(bg_h) * target,
                f64::from(bg_h),
                (f64::from(bg_w) - f64::from(bg_h) * target) / 2.0,
                0.0,
            )
        } else {
            (
                f64::from(bg_w),
                f64::from(bg_w) / target,
                0.0,
                (f64::from(bg_h) - f64::from(bg_w) / target) / 2.0,
            )
        };
        let vis_w = (base_w / zoom).round().clamp(1.0, f64::from(bg_w));
        let vis_h = (base_h / zoom).round().clamp(1.0, f64::from(bg_h));
        let free_x = (base_w - vis_w).max(0.0);
        let free_y = (base_h - vis_h).max(0.0);
        let shift_x =
            (number(params, "bg_x")?.unwrap_or(0.0).clamp(-100.0, 100.0) / 100.0) * free_x / 2.0;
        let shift_y =
            (number(params, "bg_y")?.unwrap_or(0.0).clamp(-100.0, 100.0) / 100.0) * free_y / 2.0;
        let crop_x =
            base_x.round() as u32 + (free_x / 2.0 - shift_x).round().clamp(0.0, free_x) as u32;
        let crop_y =
            base_y.round() as u32 + (free_y / 2.0 - shift_y).round().clamp(0.0, free_y) as u32;
        args.push(os("("));
        args.push(os(&background));
        args.push(os("-crop"));
        args.push(os(format!(
            "{}x{}+{crop_x}+{crop_y}",
            vis_w.round(),
            vis_h.round()
        )));
        args.push(os("+repage"));
        let blur = number(params, "bg_blur")?.unwrap_or(0.0).clamp(0.0, 50.0);
        if blur > 0.0 {
            args.push(os("-blur"));
            args.push(os(format!("0x{blur:.1}")));
        }
        args.push(os("-resize"));
        args.push(os(format!("{size}!")));
        args.push(os(")"));
    } else {
        args.push(os(format!("xc:{color}")));
    }
    // A gray background picture (a black-and-white photograph, say) put the whole
    // composite in Gray, and the colour photo came out white instead of red. The
    // canvas is a colour surface whatever the picture is, so normalise it before the
    // photo lands on top. Geometry is untouched.
    args.push(os("-colorspace"));
    args.push(os("sRGB"));
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

/// The photo adjustments as ImageMagick arguments, in one place.
///
/// The full-resolution job and the dialog's preview both call this: two copies of
/// the same chain drift within a week, and then the preview stops being a preview.
fn photo_filter_args(params: &ToolParams) -> Result<Vec<OsString>, JobError> {
    let brightness = number(params, "brightness")?.unwrap_or(0.0) as f32;
    let contrast = number(params, "contrast")?.unwrap_or(0.0) as f32;
    let saturation = number(params, "saturation")?.unwrap_or(0.0) as f32;
    let exposure = number(params, "exposure")?.unwrap_or(0.0) as f32;
    let hue = number(params, "hue")?.unwrap_or(0.0) as f32;
    let temperature = number(params, "temperature")?.unwrap_or(0.0) as f32;
    let tint = number(params, "tint")?.unwrap_or(0.0) as f32;
    let whites = number(params, "whites")?.unwrap_or(0.0) as f32;
    let blacks = number(params, "blacks")?.unwrap_or(0.0) as f32;
    let shadows = number(params, "shadows")?.unwrap_or(0.0) as f32;
    let highlights = number(params, "highlights")?.unwrap_or(0.0) as f32;
    let vibrance = number(params, "vibrance")?.unwrap_or(0.0) as f32;
    let clarity = number(params, "clarity")?.unwrap_or(0.0) as f32;
    let dehaze = number(params, "dehaze")?.unwrap_or(0.0) as f32;
    let grain = number(params, "grain")?.unwrap_or(0.0) as f32;
    let noise = number(params, "noise")?.unwrap_or(0.0) as f32;
    let sharpness = number(params, "sharpness")?.unwrap_or(0.0) as f32;
    let gamma = number(params, "gamma")?;

    let mut args: Vec<OsString> = Vec::new();
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
    if tint != 0.0 {
        // Tint is the green/magenta axis: green down and the picture reads magenta.
        let green = (1.0 - tint / 400.0).clamp(0.05, 2.0);
        args.push(os("-channel"));
        args.push(os("G"));
        args.push(os("-evaluate"));
        args.push(os("multiply"));
        args.push(os(format!("{green:.4}")));
        args.push(os("+channel"));
    }
    if whites != 0.0 {
        // `-level black%,white%` rescales so the white point becomes 255. Raising it
        // above 100% therefore *darkens*; a positive Whites must lower it and clip
        // toward white, and a negative Whites must raise it to dim the top end.
        args.push(os("-level"));
        args.push(os(format!(
            "0%,{:.2}%",
            (100.0 - whites * 0.25).clamp(5.0, 200.0)
        )));
    }
    if blacks != 0.0 {
        // The mirror of whites: a black point below 0% lifts 0 to a positive value
        // (`-level -25%,100%` writes 51 at black), a point above 0% crushes shadows.
        args.push(os("-level"));
        args.push(os(format!(
            "{:.2}%,100%",
            (-blacks * 0.25).clamp(-50.0, 50.0)
        )));
    }
    if shadows != 0.0 {
        // ImageMagick's gamma is the inverse exponent: `-gamma 2` brightens and
        // `-gamma 0.5` darkens. A shadow lift is therefore gamma above one.
        let g = (1.0 + shadows / 100.0).clamp(0.1, 4.0);
        args.push(os("-gamma"));
        args.push(os(format!("{g:.3}")));
    }
    if highlights != 0.0 {
        // The sigmoid midpoint decides which end moves: below 50% lifts the upper
        // range and pins black at 0, above 50% recovers (darkens) the bright end.
        let midpoint = (50.0 - highlights * 0.3).clamp(0.0, 100.0);
        args.push(os("-sigmoidal-contrast"));
        args.push(os(format!("3,{midpoint:.1}%")));
    }
    if brightness != 0.0 || saturation != 0.0 || hue != 0.0 {
        let bright = (100.0 + brightness).clamp(0.0, 200.0);
        let sat = (100.0 + saturation).clamp(0.0, 200.0);
        // `-modulate`'s third value is a hue percentage: 100 is unchanged and 200 is
        // +180°, so one degree is 1/1.8 of a percent.
        let hue_shift = (100.0 + hue / 1.8).clamp(0.0, 200.0);
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
    if vibrance != 0.0 {
        let sat = (100.0 + vibrance * 0.5).clamp(0.0, 200.0);
        args.push(os("-modulate"));
        args.push(os(format!("100,{sat:.2},100")));
    }
    if clarity > 0.0 {
        args.push(os("-unsharp"));
        args.push(os(format!("0x1+{:.3}", clarity / 100.0)));
    } else if clarity < 0.0 {
        args.push(os("-blur"));
        args.push(os(format!("0x{:.2}", -clarity / 50.0)));
    }
    if dehaze > 0.0 {
        // A real magnitude, not one fixed push: 10 % is a gentle 1.2 curve and 100 %
        // is a strong 3.0. The midpoint stays put so the exposure does not shift.
        let contrast = 1.0 + dehaze / 50.0;
        args.push(os("-sigmoidal-contrast"));
        args.push(os(format!("{contrast:.3},50%")));
    } else if dehaze < 0.0 {
        // Haze is a veil: flatten the tone curve and soften a little, both scaled.
        let drop = -dehaze;
        args.push(os("-brightness-contrast"));
        args.push(os(format!("0x-{:.0}", drop * 0.5)));
        args.push(os("-blur"));
        args.push(os(format!("0x{:.2}", drop / 100.0)));
    }
    if noise > 0.0 {
        // Each pass removes another layer of speckle; 25 is one pass.
        let passes = (noise / 25.0).ceil().clamp(1.0, 4.0) as usize;
        for _ in 0..passes {
            args.push(os("-despeckle"));
        }
    }
    if sharpness > 0.0 {
        args.push(os("-sharpen"));
        args.push(os(format!("0x{:.3}", sharpness / 50.0)));
    }
    if grain > 0.0 {
        args.push(os("-attenuate"));
        args.push(os(format!("{:.3}", grain / 100.0)));
        args.push(os("+noise"));
        args.push(os("Gaussian"));
    }
    Ok(args)
}

fn edit_photo(job: &Job, cx: &Context<'_>, params: &ToolParams) -> Result<Vec<PathBuf>, JobError> {
    let magick = magick_path(cx)?;
    let out = tool_out(job, cx, ToolId::EditPhoto);
    let mut args = vec![os(&job.source)];
    args.extend(photo_filter_args(params)?);
    args.push(os(&out));

    cx.ctx.set_stage("adjusting the photo");
    cx.ctx.set_progress(0.05);
    run_magick(&magick, args, cx, Budget::Short)?;
    ensure_file(&out, "the image encoder")?;
    cx.ctx.set_progress(1.0);
    Ok(vec![out])
}

/// The photo with its adjustments applied, at `size` on the long edge.
///
/// The source is never decoded at full resolution for a preview: a 63 MP photo
/// costs ~250 MB as a bitmap and the dialog only ever shows 1024 px.
pub fn photo_preview(
    tools: &crate::Toolset,
    cache: &crate::preview::PreviewCache,
    source: &Path,
    params: &ToolParams,
    out: &Path,
    size: u32,
) -> Result<(), JobError> {
    let magick = tools.path(ToolKey::Magick)?;
    let ctx = JobCtx::new();
    let mut args: Vec<OsString> = Vec::new();
    // A JPEG can be decoded at a fraction of its size: `jpeg:size` asks the decoder for
    // a DCT-scaled version, which is the difference between decoding twelve megapixels
    // and decoding two. The full job never does this — its output is the real file.
    let jpeg = source
        .extension()
        .and_then(|e| e.to_str())
        .map(|e| matches!(e.to_ascii_lowercase().as_str(), "jpg" | "jpeg"))
        .unwrap_or(false);
    if jpeg {
        args.push(os("-define"));
        args.push(os(format!("jpeg:size={}x{}", size * 2, size * 2)));
    }
    args.push(os(source));
    args.push(os("-auto-orient"));
    args.push(os("-thumbnail"));
    args.push(os(format!("{size}x{size}>")));
    args.extend(photo_filter_args(params)?);
    args.push(os("-quality"));
    args.push(os("86"));
    args.push(os(out));
    let _permit = cache.preview_permit();
    let output = proc::run(Cmd::new(magick).args(args).budget(Budget::Short), &ctx)?;
    output.ok()?;
    ensure_file(out, "the photo preview")?;
    Ok(())
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
    image_size_with(magick, input, cx.ctx)
}

/// Dimensions with a bare `JobCtx`, so a preview (which has no work directory) can
/// measure the source the same way a job does.
fn image_size_with(magick: &Path, input: &Path, ctx: &JobCtx) -> Result<(u32, u32), JobError> {
    let cmd = Cmd::new(magick)
        .arg("identify")
        .arg("-format")
        .arg("%w %h")
        .arg(input)
        .stdout_piped()
        .budget(Budget::Short);
    let output = proc::run(cmd, ctx)?;
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

fn copy_file(from: &Path, to: &Path) -> Result<(), JobError> {
    std::fs::copy(from, to)
        .map(|_| ())
        .map_err(|e| JobError::io(format!("copying {}", to.display()), &e))
}

/// Keep the original when the encoder produced something larger. A compressor that
/// grows a file is not a compressor; the AV and PDF tools use the same rule.
fn prefer_smaller(
    job: &Job,
    produced: PathBuf,
    ext: &str,
    cx: &Context<'_>,
) -> Result<PathBuf, JobError> {
    // SVG rasterises to PNG: comparing the two by size is meaningless, and copying the
    // SVG under a .png name produced an artifact no viewer could open.
    let same_format = input_format(job)
        .map(|format| format.canonical_ext() == ext)
        .unwrap_or(false);
    if !same_format || file_len(&produced)? < file_len(&job.source)? {
        return Ok(produced);
    }
    // A resize is an explicit request for other dimensions, so "never grow" cannot
    // mean "hand back the original": the dialog said 800 × 500. The original is only
    // the right answer when the shape did not change.
    if crate::image_dimensions(&produced) != crate::image_dimensions(&job.source) {
        return Ok(produced);
    }
    let copy = cx.scratch(&format!("compress-original.{ext}"));
    copy_file(&job.source, &copy)?;
    Ok(copy)
}

/// A real PNG optimizer, run after the encoder. ImageMagick's PNG stream leaves row
/// filters and the deflate strategy on the table; oxipng tries them losslessly, which
/// matters most for screenshots and flat graphics. A failure keeps the encoder output.
fn optimize_png(path: PathBuf, ext: &str, cx: &Context<'_>) -> Result<PathBuf, JobError> {
    if ext != "png" {
        return Ok(path);
    }
    cx.ctx.checkpoint()?;
    let Ok(bytes) = std::fs::read(&path) else {
        return Ok(path);
    };
    // Bound memory and time: this is an improvement, not worth stalling a huge job.
    if bytes.len() > 64 * 1024 * 1024 {
        return Ok(path);
    }
    cx.ctx.set_stage("optimising the PNG");
    let mut opts = oxipng::Options::from_preset(2);
    opts.timeout = Some(std::time::Duration::from_secs(4));
    opts.strip = oxipng::StripChunks::None;
    opts.scale_16 = false;
    let Ok(optimized) = oxipng::optimize_from_memory(&bytes, &opts) else {
        return Ok(path);
    };
    if optimized.len() < bytes.len() {
        std::fs::write(&path, optimized)
            .map_err(|e| JobError::io("writing the optimised PNG", &e))?;
    }
    Ok(path)
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
    use std::collections::BTreeSet;

    #[test]
    fn palette_quantizer_bounds_colours_and_preserves_transparent_pixels() {
        let width = 400_u32;
        let height = 400_u32;
        let mut pixels = Vec::with_capacity((width * height * 4) as usize);
        let mut random = 0xA5C3_17D9_u32;
        for _y in 0..height {
            for x in 0..width {
                if x >= width / 2 {
                    pixels.extend([255, 0, 255, 0]);
                } else {
                    random ^= random << 13;
                    random ^= random >> 17;
                    random ^= random << 5;
                    let gray = (random >> 24) as u8;
                    pixels.extend([gray, gray, gray, 255]);
                }
            }
        }

        let temp = tempfile::tempdir().unwrap();
        let mut lossless = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut lossless, width, height);
            encoder.set_color(png::ColorType::Rgba);
            encoder.set_depth(png::BitDepth::Eight);
            encoder.set_compression(png::Compression::High);
            encoder
                .write_header()
                .unwrap()
                .write_image_data(&pixels)
                .unwrap();
        }
        let output = temp.path().join("quantized.png");
        encode_indexed_png(width, height, &pixels, 16, 1.0, &output).unwrap();
        assert!(
            std::fs::metadata(&output).unwrap().len() < lossless.len() as u64,
            "indexed output should be smaller than the lossless source"
        );
        let encoded = std::fs::read(&output).unwrap();
        let reader = png::Decoder::new(std::io::Cursor::new(&encoded))
            .read_info()
            .unwrap();
        let info = reader.info();
        assert_eq!(info.color_type, png::ColorType::Indexed);
        assert_eq!((info.width, info.height), (width, height));
        let palette_len = info.palette.as_ref().unwrap().len() / 3;
        assert!(
            (2..=16).contains(&palette_len),
            "palette has {palette_len} entries"
        );
        let decoded = ::image::load_from_memory(&encoded).unwrap().to_rgba8();
        let used: BTreeSet<_> = decoded.pixels().map(|p| [p[0], p[1], p[2], p[3]]).collect();
        assert!(
            used.len() <= 16,
            "decoded output has {} unique colors",
            used.len()
        );
        assert!(
            decoded
                .pixels()
                .enumerate()
                .all(|(i, p)| { i % (width as usize) < (width / 2) as usize || p[3] == 0 }),
            "transparent pixels stay transparent"
        );

        let mut squared_error = 0.0_f64;
        let mut samples = 0_u64;
        for (pixel, output) in pixels.as_chunks::<4>().0.iter().zip(decoded.pixels()) {
            if pixel[3] == 0 {
                continue;
            }
            for channel in 0..3 {
                let difference = pixel[channel] as f64 - output[channel] as f64;
                squared_error += difference * difference;
                samples += 1;
            }
        }
        let rmse = (squared_error / samples as f64).sqrt();
        assert!(rmse < 42.0, "gradient RGB RMSE was {rmse:.2}");

        let two_colour_output = temp.path().join("quantized-2.png");
        encode_indexed_png(width, height, &pixels, 2, 0.0, &two_colour_output).unwrap();
        let two_colour = ::image::open(&two_colour_output).unwrap().to_rgba8();
        let two_colour_set: BTreeSet<_> = two_colour
            .pixels()
            .map(|p| [p[0], p[1], p[2], p[3]])
            .collect();
        assert!(
            two_colour_set.len() <= 2,
            "2-colour output has {} unique colors",
            two_colour_set.len()
        );
        assert_eq!(two_colour.dimensions(), (width, height));
    }

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
        run_engine_with(f, job, None)
    }

    fn run_engine_with(
        f: &Fixture,
        job: &Job,
        preview: Option<&crate::preview::PreviewCache>,
    ) -> Result<Vec<PathBuf>, JobError> {
        let work = WorkDir::create(f._dir.path().join(format!("work-{}", job.id)))
            .expect("create the work dir");
        let ctx = JobCtx::new();
        let cx = Context {
            tools: &f.tools,
            work: &work,
            ctx: &ctx,
            preview,
        };
        run(job, &cx)
    }

    fn preview_engine(
        f: &Fixture,
        job: &Job,
        cache: &crate::preview::PreviewCache,
    ) -> Result<CompressPreview, JobError> {
        let work = WorkDir::create(f._dir.path().join(format!("preview-{}", job.id)))
            .expect("create the preview work dir");
        let ctx = JobCtx::new();
        let cx = Context {
            tools: &f.tools,
            work: &work,
            ctx: &ctx,
            preview: Some(cache),
        };
        let result = preview_compress(job, &cx);
        work.cleanup();
        result
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

    /// Mean luma of the horizontal strip `x .. x + w`, 0..255.
    fn strip_mean(f: &Fixture, path: &Path, x: u32, w: u32) -> f64 {
        let cmd = Cmd::new(&f.magick)
            .arg(format!("{}[{w}x1+{x}+0]", path.display()))
            .arg("-format")
            .arg("%[fx:mean*255]")
            .arg("info:")
            .stdout_piped()
            .budget(Budget::Short);
        let output = proc::run(cmd, &JobCtx::new()).expect("run magick");
        assert!(output.success, "magick failed: {}", output.stderr_tail);
        output
            .stdout
            .trim()
            .parse()
            .unwrap_or_else(|_| panic!("unreadable mean `{}`", output.stdout.trim()))
    }

    /// Standard deviation of the whole image, 0..255: how textured it is.
    fn deviation(f: &Fixture, path: &Path) -> f64 {
        let cmd = Cmd::new(&f.magick)
            .arg(path)
            .arg("-colorspace")
            .arg("Gray")
            .arg("-format")
            .arg("%[fx:standard_deviation*255]")
            .arg("info:")
            .stdout_piped()
            .budget(Budget::Short);
        let output = proc::run(cmd, &JobCtx::new()).expect("run magick");
        assert!(output.success, "magick failed: {}", output.stderr_tail);
        output.stdout.trim().parse().unwrap_or(0.0)
    }

    /// HSV hue of an RGB triple, in degrees.
    fn hue_degrees((r, g, b): (u8, u8, u8)) -> f64 {
        let (r, g, b) = (r as f64, g as f64, b as f64);
        let max = r.max(g).max(b);
        let min = r.min(g).min(b);
        let delta = max - min;
        if delta == 0.0 {
            return 0.0;
        }
        let raw = if max == r {
            60.0 * ((g - b) / delta)
        } else if max == g {
            60.0 * ((b - r) / delta + 2.0)
        } else {
            60.0 * ((r - g) / delta + 4.0)
        };
        raw.rem_euclid(360.0)
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
                preview: None,
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

    /// The background picture is cropped from the centre of its cover window: the crop
    /// starts at `(base_x, base_y)` and `bg_x`/`bg_y` slide inside that window. Cropping
    /// from source (0, 0) instead showed the wrong part of a picture whose aspect differs
    /// from the canvas, so the dialog preview and the saved file disagreed.
    #[test]
    fn add_background_image_crop_starts_at_the_cover_window() {
        let Some(f) = fixture() else { return };
        let input = solid(&f, "cover-shot.png", "100x100", "green");
        // Red everywhere except a blue band from x=40 to x=89. On a square canvas this
        // 200x100 picture's cover window is 100x100 starting at source x=50: blue. The
        // source origin, which is where the crop used to start, is red.
        let backdrop = make_image(
            &f,
            "cover-backdrop.png",
            vec![
                os("-size"),
                os("200x100"),
                os("xc:red"),
                os("-fill"),
                os("blue"),
                os("-draw"),
                os("rectangle 40,0 89,99"),
            ],
        );
        let compose = |zoom: f64, x: f64| {
            let job = Job::new(
                input.clone(),
                Intent::Tool {
                    tool: ToolId::AddBackground,
                    params: params(json!({
                        "image": backdrop.to_string_lossy(),
                        // 100% of the 100 px shorter side: a 200x200 canvas, so the
                        // background is visible around the photo. With no padding the
                        // photo covers the canvas and there is no background pixel left.
                        "padding_pct": 100,
                        "aspect": "1:1",
                        "bg_zoom": zoom,
                        "bg_x": x,
                        "bg_y": 0,
                    })),
                },
            );
            run_engine(&f, &job).expect("compose")[0].clone()
        };
        // Cover window at source (50, 0): its first pixel is the blue band.
        assert_eq!(
            pixel(&f, &compose(1.0, 0.0), 1, 1),
            (0, 0, 255),
            "the cover window must start at source x=50, not at the source origin"
        );
        // Zoom 2 leaves 50 px of room inside the window. A fully positive bg_x keeps
        // the crop at the window's origin (blue); a fully negative one slides it to
        // source x=100, back into the red field.
        assert_eq!(
            pixel(&f, &compose(2.0, 100.0), 1, 1),
            (0, 0, 255),
            "bg_x=100 must not move the crop outside the cover window"
        );
        assert_eq!(
            pixel(&f, &compose(2.0, -100.0), 1, 1),
            (255, 0, 0),
            "bg_x=-100 must slide the crop to the window's far edge"
        );
    }

    #[test]
    fn background_blur_changes_only_the_background_layer() {
        let Some(f) = fixture() else { return };
        let input = solid(&f, "blur-photo.png", "400x300", "red");
        // A black-and-white checkerboard: a flat colour blurs to itself, so only a
        // pattern can show whether the sigma reached the background and nothing else.
        let backdrop = make_image(
            &f,
            "blur-backdrop.png",
            vec![
                os("-size"),
                os("400x400"),
                os("pattern:checkerboard"),
                os("-threshold"),
                os("50%"),
            ],
        );
        let compose = |blur: f64| {
            let job = Job::new(
                input.clone(),
                Intent::Tool {
                    tool: ToolId::AddBackground,
                    params: params(json!({
                        "image": backdrop.to_string_lossy(),
                        "bg_zoom": 1,
                        "padding_pct": 25,
                        "bg_blur": blur,
                    })),
                },
            );
            run_engine(&f, &job).expect("compose")[0].clone()
        };
        let sharp = compose(0.0);
        let soft = compose(35.0);
        // 25% padding on a 400x300 photo is 75 px: the canvas is 550x450 and the
        // photo starts at x=75, so x=37 is background at every height.
        let mut biggest = 0i32;
        for y in [40u32, 120, 200, 280, 360] {
            let (sr, sg, sb) = pixel(&f, &sharp, 37, y);
            let (br, bg, bb) = pixel(&f, &soft, 37, y);
            let delta = (i32::from(sr) - i32::from(br))
                .abs()
                .max((i32::from(sg) - i32::from(bg)).abs())
                .max((i32::from(sb) - i32::from(bb)).abs());
            biggest = biggest.max(delta);
        }
        assert!(
            biggest > 30,
            "blur must change the background layer, biggest channel delta was {biggest}"
        );
        // The photo is composited after the background: blurring the backdrop must not
        // touch it. Centre of the 550x450 canvas.
        let (pr, pg, pb) = pixel(&f, &sharp, 275, 225);
        let (qr, qg, qb) = pixel(&f, &soft, 275, 225);
        // The checkerboard is gray: without a colour canvas the photo used to come
        // back white, so this also guards the sRGB normalisation.
        assert!(
            pr > 240 && pg < 12 && pb < 12,
            "the photo centre should be red, got {pr}/{pg}/{pb}"
        );
        let same = |a: u8, b: u8| (i32::from(a) - i32::from(b)).abs() <= 2;
        assert!(
            same(pr, qr) && same(pg, qg) && same(pb, qb),
            "the photo must not change with the background blur: \
             {pr}/{pg}/{pb} vs {qr}/{qg}/{qb}"
        );
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

    fn png_dims(path: &Path) -> (u32, u32) {
        ::image::image_dimensions(path).expect("read the PNG dimensions")
    }

    #[test]
    fn collage_quadrants_follow_their_sources_and_the_order() {
        let Some(f) = fixture() else { return };
        let inputs = vec![
            solid(&f, "c1.png", "100x100", "red"),
            solid(&f, "c2.png", "100x100", "green"),
            solid(&f, "c3.png", "100x100", "blue"),
            solid(&f, "c4.png", "100x100", "white"),
        ];
        let compose = |order: Value, extra: Value| {
            let mut map = serde_json::json!({
                "width": 2048,
                "spacing_pct": 1.5625,
                "background": "#000000",
                "order": order,
            });
            if let (Some(map), Some(extra)) = (map.as_object_mut(), extra.as_object()) {
                for (key, value) in extra {
                    map.insert(key.clone(), value.clone());
                }
            }
            let job = Job::with_inputs(
                inputs.clone(),
                Intent::Tool {
                    tool: ToolId::Collage,
                    params: params(map),
                },
            );
            run_engine(&f, &job).expect("compose the collage")
        };

        let artifacts = compose(serde_json::json!([0, 1, 2, 3]), serde_json::json!({}));
        assert_eq!(png_dims(&artifacts[0]), (2048, 2048));
        // cell_w = (2048 - 16*3) / 2 = 1000, first cell at 16.
        assert_eq!(pixel(&f, &artifacts[0], 516, 516), (255, 0, 0));
        assert_eq!(pixel(&f, &artifacts[0], 1532, 516), (0, 128, 0));
        assert_eq!(pixel(&f, &artifacts[0], 516, 1532), (0, 0, 255));
        assert_eq!(pixel(&f, &artifacts[0], 1532, 1532), (255, 255, 255));
        // The gutter is the background, not a stretched cell.
        assert_eq!(pixel(&f, &artifacts[0], 1024, 516), (0, 0, 0));

        // Reversing the order swaps the quadrants, not the layout.
        let reversed = compose(serde_json::json!([3, 2, 1, 0]), serde_json::json!({}));
        assert_eq!(pixel(&f, &reversed[0], 516, 516), (255, 255, 255));
        assert_eq!(pixel(&f, &reversed[0], 1532, 1532), (255, 0, 0));

        // A rounded corner of the first cell is the background.
        let rounded = compose(
            serde_json::json!([0, 1, 2, 3]),
            serde_json::json!({ "radius_pct": 50 }),
        );
        assert_eq!(pixel(&f, &rounded[0], 17, 17), (0, 0, 0));
    }

    #[test]
    fn collage_contain_leaves_the_background_at_the_cell_edges() {
        let Some(f) = fixture() else { return };
        let wide = solid(&f, "wide-collage.png", "200x100", "blue");
        let second = solid(&f, "square-collage.png", "100x100", "red");
        let job = Job::with_inputs(
            vec![wide, second],
            Intent::Tool {
                tool: ToolId::Collage,
                params: params(serde_json::json!({
                    "width": 2048,
                    "spacing_pct": 1.5625,
                    "background": "#000000",
                    "fit": "contain",
                })),
            },
        );
        let artifacts = run_engine(&f, &job).expect("compose");
        let (w, h) = png_dims(&artifacts[0]);
        assert_eq!((w, h), (2048, 1024), "auto ratio follows the first picture");
        // The wide picture is letterboxed inside its square-ish cell.
        assert_eq!(pixel(&f, &artifacts[0], 516, 100), (0, 0, 0));
        assert_eq!(pixel(&f, &artifacts[0], 516, 516), (0, 0, 255));
    }

    #[test]
    fn collage_rows_columns_make_the_requested_grid() {
        let Some(f) = fixture() else { return };
        let colours = ["red", "green", "blue", "white", "yellow", "magenta"];
        let inputs: Vec<PathBuf> = colours
            .iter()
            .enumerate()
            .map(|(index, colour)| solid(&f, &format!("grid-{index}.png"), "100x100", colour))
            .collect();
        let case = |rows: u32, columns: u32| {
            let params = params(serde_json::json!({
                "width": 2048,
                "aspect": "1:1",
                "rows": rows,
                "columns": columns,
                "background": "#000000",
                "radius_pct": 100,
            }));
            let layout =
                collage_layout(&f.tools, &inputs, &params, 2048).expect("compute the layout");
            let job = Job::with_inputs(
                inputs.clone(),
                Intent::Tool {
                    tool: ToolId::Collage,
                    params,
                },
            );
            let artifacts = run_engine(&f, &job).expect("compose");
            (layout, artifacts)
        };
        let expected = [
            (255, 0, 0),
            (0, 128, 0),
            (0, 0, 255),
            (255, 255, 255),
            (255, 255, 0),
            (255, 0, 255),
        ];
        let rect = |cell: &CollageCellOut| (cell.x, cell.y, cell.w, cell.h);

        // 3 rows × 2 columns: cells are 1024 wide and 682 tall.
        let (layout, composed) = case(3, 2);
        assert_eq!((layout.width, layout.height), (2048, 2048));
        assert_eq!((layout.cols, layout.rows), (2, 3));
        assert_eq!(layout.cells.len(), 6);
        assert_eq!(rect(&layout.cells[0]), (0, 0, 1024, 682));
        assert_eq!(rect(&layout.cells[1]), (1024, 0, 1024, 682));
        assert_eq!(rect(&layout.cells[5]), (1024, 1364, 1024, 682));
        for (cell, colour) in layout.cells.iter().zip(expected) {
            assert_eq!(
                pixel(&f, &composed[0], cell.x + cell.w / 2, cell.y + cell.h / 2),
                colour,
                "cell {} is the wrong picture",
                cell.input
            );
        }
        // radius_pct 100 rounds the picture, so its corner is the canvas.
        assert_eq!(pixel(&f, &composed[0], 2, 2), (0, 0, 0));

        // 2 rows × 3 columns: the same pictures, a visibly different grid.
        let (layout, composed) = case(2, 3);
        assert_eq!((layout.cols, layout.rows), (3, 2));
        assert_eq!(rect(&layout.cells[0]), (0, 0, 682, 1024));
        assert_eq!(rect(&layout.cells[1]), (682, 0, 682, 1024));
        assert_eq!(rect(&layout.cells[5]), (1364, 1024, 682, 1024));
        for (cell, colour) in layout.cells.iter().zip(expected) {
            assert_eq!(
                pixel(&f, &composed[0], cell.x + cell.w / 2, cell.y + cell.h / 2),
                colour,
                "cell {} is the wrong picture",
                cell.input
            );
        }
        assert_eq!(pixel(&f, &composed[0], 2, 2), (0, 0, 0));
    }

    #[test]
    fn collage_cell_pan_and_zoom_move_the_picture() {
        let Some(f) = fixture() else { return };
        // Left half red, right half blue: a pan or zoom changes which half fills the
        // sampled point.
        let source = make_image(
            &f,
            "pan-source.png",
            vec![
                os("-size"),
                os("200x100"),
                os("xc:red"),
                os("-fill"),
                os("blue"),
                os("-draw"),
                os("rectangle 100,0 199,99"),
            ],
        );
        let compose = |cells: Value| {
            let job = Job::with_inputs(
                vec![source.clone()],
                Intent::Tool {
                    tool: ToolId::Collage,
                    params: params(serde_json::json!({
                        "width": 2048,
                        "background": "#000000",
                        "cells": cells,
                    })),
                },
            );
            run_engine(&f, &job).expect("compose the collage")
        };

        // One cell at 0,0 is 1024×1024; zoom 2 shows a 2×2 copy of the picture.
        let centred = compose(serde_json::json!([{ "index": 0, "zoom": 2, "x": 0, "y": 0 }]));
        let panned = compose(serde_json::json!([{ "index": 0, "zoom": 2, "x": 100, "y": 0 }]));
        assert_eq!(pixel(&f, &centred[0], 256, 512), (255, 0, 0));
        assert_eq!(pixel(&f, &panned[0], 256, 512), (0, 0, 255));
    }

    #[test]
    fn annotate_draws_every_shape_kind_inside_its_box() {
        let Some(f) = fixture() else { return };
        let input = solid(&f, "annotate.png", "600x400", "red");
        let job = Job::new(
            input.clone(),
            Intent::Tool {
                tool: ToolId::Annotate,
                params: params(serde_json::json!({
                    "color": "#ff5a36",
                    "shapes": [
                        {"kind": "rect", "x": 0.1, "y": 0.1, "w": 0.5, "h": 0.3, "color": "#0000ff"},
                        {"kind": "ellipse", "x": 0.1, "y": 0.55, "w": 0.2, "h": 0.2},
                        {"kind": "arrow", "x": 0.4, "y": 0.5, "w": 0.2, "h": 0.1},
                        {"kind": "line", "x": 0.4, "y": 0.7, "w": 0.1, "h": 0.1,
                         "points": [[0.4, 0.7], [0.5, 0.8], [0.6, 0.72]]},
                        {"kind": "highlight", "x": 0.65, "y": 0.1, "w": 0.2, "h": 0.2, "color": "#00ff00"},
                        {"kind": "number", "x": 0.65, "y": 0.55, "w": 0.1, "h": 0.1, "text": "1"},
                        {"kind": "text", "x": 0.5, "y": 0.9, "w": 0.0, "h": 0.0, "text": "Hi", "size": 20}
                    ]
                })),
            },
        );
        let artifacts = run_engine(&f, &job).expect("annotate");
        assert_eq!(png_dims(&artifacts[0]), (600, 400));
        // The rectangle's left border is blue; its interior and the corner are not.
        let border = pixel(&f, &artifacts[0], 60, 100);
        assert!(
            border.2 > 150 && border.0 < 120,
            "rect border was {border:?}"
        );
        let inside = pixel(&f, &artifacts[0], 200, 100);
        assert_eq!(inside, (255, 0, 0), "the rectangle has no fill");
        assert_eq!(pixel(&f, &artifacts[0], 2, 2), (255, 0, 0));
        // The highlight changes the picture under it without hiding the source.
        let lit = pixel(&f, &artifacts[0], 450, 80);
        assert_ne!(lit, (255, 0, 0), "the highlight did not draw");
    }

    #[test]
    fn annotate_number_and_text_are_centred_and_legible() {
        let Some(f) = fixture() else { return };
        let input = solid(&f, "annotate-centre.png", "400x300", "black");
        let annotate = |font: &str| -> PathBuf {
            let job = Job::new(
                input.clone(),
                Intent::Tool {
                    tool: ToolId::Annotate,
                    params: params(json!({
                        "shapes": [
                            // 40 x 40 px badge centred on (200, 140): the circle fills
                            // the box, so a background pixel at the centre would mean
                            // the digit was drawn at the edge instead of centred.
                            {"kind": "number", "x": 0.45, "y": 0.4, "w": 0.1, "h": 0.133333,
                             "text": "7", "color": "#0055ff"},
                            // The same box with two digits: width must not shift the glyph
                            // off the centre.
                            {"kind": "number", "x": 0.2, "y": 0.4, "w": 0.1, "h": 0.133333,
                             "text": "10", "color": "#0055ff"},
                            {"kind": "text", "x": 0.05, "y": 0.05, "w": 0.5, "h": 0.2,
                             "text": "MMMM", "size": 40, "color": "#ff2200", "font": font}
                        ]
                    })),
                },
            );
            run_engine(&f, &job).expect("annotate")[0].clone()
        };

        let sans = annotate("sans");
        // The badge body is blue and the digit is white. Sampling a small grid around
        // the centre accepts any digit shape: a stroke of the 7 crosses that area.
        let mut white_near_centre = false;
        let mut blue_in_box = false;
        for (dx, dy) in [(-5, -5), (-2, -3), (0, -2), (1, 1), (2, -1), (3, 0), (0, 0)] {
            let (r, g, b) = pixel(&f, &sans, (200i64 + dx) as u32, (140i64 + dy) as u32);
            if r > 150 && g > 150 && b > 150 {
                white_near_centre = true;
            }
            if b > 150 && r < 100 {
                blue_in_box = true;
            }
        }
        assert!(blue_in_box, "the number badge did not fill its box");
        assert!(
            white_near_centre,
            "the digit is not near the centre of the badge"
        );

        // Two digits: the whole text box is centred, so its glyphs surround (100, 140).
        let mut two_digit_white = false;
        for (dx, dy) in [
            (-10, -4),
            (-8, -8),
            (-6, 0),
            (-2, -1),
            (2, -1),
            (4, -6),
            (7, 0),
            (10, 4),
            (0, 0),
        ] {
            let (r, g, b) = pixel(&f, &sans, (100i64 + dx) as u32, (140i64 + dy) as u32);
            if r > 150 && g > 150 && b > 150 {
                two_digit_white = true;
            }
        }
        assert!(
            two_digit_white,
            "the two-digit badge is not centred on its box"
        );

        // The caption is red somewhere inside its box, not only under its rim.
        let mut red_inside = false;
        for (x, y) in [(25, 30), (45, 35), (70, 40), (95, 42)] {
            let (r, _g, _b) = pixel(&f, &sans, x, y);
            if r > 120 {
                red_inside = true;
            }
        }
        assert!(red_inside, "the text shape did not draw inside its box");

        // If the machine has both a plain sans and a plain serif, swapping the family
        // must change the pixels: this is what proves the family reaches the draw string.
        let table = load_fonts(&f.magick, &JobCtx::new()).unwrap_or_default();
        if table.name(FontFamily::Sans).is_some() && table.name(FontFamily::Serif).is_some() {
            let serif = annotate("serif");
            let a = std::fs::read(&sans).expect("read sans output");
            let b = std::fs::read(&serif).expect("read serif output");
            assert_ne!(a, b, "sans and serif produced identical bytes");
        }
    }

    #[test]
    fn annotate_arrow_uses_round_caps_and_a_large_text_is_allowed() {
        let arrow = DrawShape {
            kind: ShapeKind::Arrow,
            x: 0.1,
            y: 0.1,
            w: 0.5,
            h: 0.5,
            points: Vec::new(),
            text: None,
            color: None,
            size: Some(40.0),
            font: None,
        };
        let command = draw_command(&arrow, 1000, 800, "#ff0000", &FontTable::default());
        assert!(
            command.contains("stroke-linecap round"),
            "the arrow tip must be closed at wide strokes: {command}"
        );
        assert!(command.contains("stroke-linejoin round"));
        // A 40 px stroke gets a 140 px head, not the old fixed 48 px.
        let angle = (0.5f32 * 800.0).atan2(0.5 * 1000.0);
        let head_x = 0.6 * 1000.0 - 140.0 * (angle + 0.45).cos();
        assert!(
            command.contains(&format!("{head_x:.2}")),
            "the head did not scale with the stroke: {command}"
        );

        let text = DrawShape {
            kind: ShapeKind::Text,
            x: 0.1,
            y: 0.2,
            w: 0.0,
            h: 0.0,
            points: Vec::new(),
            text: Some("Hi".into()),
            color: None,
            size: Some(600.0),
            font: None,
        };
        let command = draw_command(&text, 1000, 800, "#ff0000", &FontTable::default());
        assert!(
            command.contains("font-size 600.0"),
            "a caption above 200 px was clamped: {command}"
        );
    }

    #[test]
    fn annotate_accepts_a_text_above_the_old_cap() {
        let Some(f) = fixture() else { return };
        let input = solid(&f, "annotate-large-text.png", "1000x800", "red");
        let job = Job::new(
            input,
            Intent::Tool {
                tool: ToolId::Annotate,
                params: params(json!({
                    "shapes": [
                        {"kind": "text", "x": 0.1, "y": 0.1, "w": 0.0, "h": 0.0,
                         "text": "Big", "size": 600}
                    ]
                })),
            },
        );
        let artifacts = run_engine(&f, &job).expect("a 600 px caption is valid");
        assert!(
            deviation(&f, &artifacts[0]) > 10.0,
            "the 600 px caption never reached the picture"
        );
    }

    #[test]
    fn annotate_rejects_a_shape_outside_the_image() {
        let job = Job::new(
            PathBuf::from("/tmp/image.png"),
            Intent::Tool {
                tool: ToolId::Annotate,
                params: params(serde_json::json!({
                    "shapes": [{"kind": "rect", "x": 0.8, "y": 0.1, "w": 0.5, "h": 0.3}]
                })),
            },
        );
        let err = validate(&job, Category::Images).expect_err("must reject");
        assert_eq!(err.code, ErrorCode::Unsupported);
    }

    #[test]
    fn redact_solid_and_blur_change_only_the_box() {
        let Some(f) = fixture() else { return };
        let run_redact = |source: &Path, params: Value| {
            let job = Job::new(
                source.to_path_buf(),
                Intent::Tool {
                    tool: ToolId::Redact,
                    params: params.as_object().cloned().expect("object params"),
                },
            );
            let work = WorkDir::create(f._dir.path().join(format!("redact-{}", job.id)))
                .expect("work dir");
            let ctx = JobCtx::new();
            let cx = Context {
                tools: &f.tools,
                work: &work,
                ctx: &ctx,
                preview: None,
            };
            crate::redact::run(&job, &cx)
        };

        let plain = solid(&f, "redact.png", "200x200", "white");
        let solid_out = run_redact(
            &plain,
            serde_json::json!({
                "boxes": [{"x": 0.25, "y": 0.25, "w": 0.5, "h": 0.5}],
                "style": "solid",
                "color": "#ff0000",
            }),
        )
        .expect("solid redaction");
        assert_eq!(pixel(&f, &solid_out[0], 100, 100), (255, 0, 0));
        assert_eq!(pixel(&f, &solid_out[0], 5, 5), (255, 255, 255));

        let noisy = noisy_400(&f, "redact-noise.png");
        let blur_out = run_redact(
            &noisy,
            serde_json::json!({
                "boxes": [{"x": 0.25, "y": 0.25, "w": 0.5, "h": 0.5}],
                "style": "blur",
            }),
        )
        .expect("blurred redaction");
        let before = pixel(&f, &noisy, 200, 200);
        let after = pixel(&f, &blur_out[0], 200, 200);
        assert_ne!(before, after, "the blur did not change the region");
        assert_eq!(pixel(&f, &blur_out[0], 5, 5), pixel(&f, &noisy, 5, 5));
    }

    #[test]
    fn redact_rejects_an_unknown_style() {
        let Some(f) = fixture() else { return };
        let plain = solid(&f, "redact-style.png", "100x100", "white");
        let job = Job::new(
            plain,
            Intent::Tool {
                tool: ToolId::Redact,
                params: params(serde_json::json!({
                    "boxes": [{"x": 0.1, "y": 0.1, "w": 0.2, "h": 0.2}],
                    "style": "sparkle",
                })),
            },
        );
        let work = WorkDir::create(f._dir.path().join("redact-style-work")).expect("work dir");
        let ctx = JobCtx::new();
        let cx = Context {
            tools: &f.tools,
            work: &work,
            ctx: &ctx,
            preview: None,
        };
        let err = crate::redact::run(&job, &cx).expect_err("must reject");
        assert_eq!(err.code, ErrorCode::Unsupported);
    }

    #[test]
    fn photo_adjustments_reach_the_engine() {
        let Some(f) = fixture() else { return };
        let grey = solid(&f, "grey.png", "100x100", "gray");
        let job = Job::new(
            grey.clone(),
            Intent::Tool {
                tool: ToolId::EditPhoto,
                params: params(serde_json::json!({ "exposure": 1 })),
            },
        );
        let bright = run_engine(&f, &job).expect("adjust");
        let before = pixel(&f, &grey, 50, 50);
        let after = pixel(&f, &bright[0], 50, 50);
        assert!(after.0 > before.0 + 30, "exposure did not lift the picture");

        let grain = Job::new(
            grey.clone(),
            Intent::Tool {
                tool: ToolId::EditPhoto,
                params: params(serde_json::json!({ "grain": 100 })),
            },
        );
        assert!(run_engine(&f, &grain).is_ok(), "grain must run");

        let nothing = Job::new(
            grey,
            Intent::Tool {
                tool: ToolId::EditPhoto,
                params: params(serde_json::json!({ "clarity": 20, "noise": 30 })),
            },
        );
        assert!(run_engine(&f, &nothing).is_ok(), "detail controls must run");
    }

    /// Every slider must move the picture the way its label says. A 256 px black→white
    /// ramp makes the tone controls measurable; solids cover colour and detail.
    #[test]
    fn photo_sliders_have_the_expected_direction() {
        let Some(f) = fixture() else { return };
        let edit = |source: &Path, value: Value| -> PathBuf {
            let job = Job::new(
                source.to_path_buf(),
                Intent::Tool {
                    tool: ToolId::EditPhoto,
                    params: params(value),
                },
            );
            run_engine(&f, &job).expect("edit photo")[0].clone()
        };

        let ramp = make_image(
            &f,
            "photo-ramp.png",
            vec![os("-size"), os("256x1"), os("gradient:black-white")],
        );
        let neutral = strip_mean(&f, &ramp, 0, 256);
        let top = |path: &Path| strip_mean(&f, path, 205, 51);
        let bottom = |path: &Path| strip_mean(&f, path, 0, 26);
        let (neutral_top, neutral_bottom) = (top(&ramp), bottom(&ramp));

        // Highlights: + lifts the upper range and leaves black alone; − recovers it.
        let highlights_up = edit(&ramp, json!({ "highlights": 60 }));
        assert!(
            top(&highlights_up) > neutral_top + 6.0,
            "highlights +60 must brighten the top 20%: {neutral_top:.1} -> {:.1}",
            top(&highlights_up)
        );
        assert!(
            top(&highlights_up) - neutral_top > (bottom(&highlights_up) - neutral_bottom).abs(),
            "highlights +60 must move the top more than the bottom: top +{:.1}, bottom {:.1}",
            top(&highlights_up) - neutral_top,
            bottom(&highlights_up) - neutral_bottom
        );
        let highlights_down = edit(&ramp, json!({ "highlights": -60 }));
        assert!(
            top(&highlights_down) < neutral_top - 0.5,
            "highlights -60 must darken the top 20%: {neutral_top:.1} -> {:.1}",
            top(&highlights_down)
        );
        assert_eq!(
            pixel(&f, &highlights_down, 0, 0).0,
            0,
            "highlights must leave black at black"
        );

        // The endpoints keep the direction: the round-4 report said the sliders read
        // inverted only at ±100, so the extremes are measured, not just ±60.
        for (value, should_rise) in [(100.0, true), (-100.0, false)] {
            let edge = edit(&ramp, json!({ "highlights": value }));
            let moved = top(&edge) - neutral_top;
            if should_rise {
                assert!(
                    moved > 6.0,
                    "highlights +100 must brighten the top 20%: {neutral_top:.1} -> {:.1}",
                    top(&edge)
                );
            } else {
                assert!(
                    moved < -0.5,
                    "highlights -100 must darken the top 20%: {neutral_top:.1} -> {:.1}",
                    top(&edge)
                );
            }
            assert_eq!(
                pixel(&f, &edge, 0, 0).0,
                0,
                "highlights {value} must leave black at black"
            );
        }

        // Whites: + clips toward white (brighter), − pulls the white point down.
        let whites_up = edit(&ramp, json!({ "whites": 60 }));
        assert!(
            strip_mean(&f, &whites_up, 0, 256) > neutral + 15.0,
            "whites +60 must brighten: {neutral:.1} -> {:.1}",
            strip_mean(&f, &whites_up, 0, 256)
        );
        let whites_down = edit(&ramp, json!({ "whites": -60 }));
        assert!(
            pixel(&f, &whites_down, 255, 0).0 < 240,
            "whites -60 must dim the white end: white is {}",
            pixel(&f, &whites_down, 255, 0).0
        );
        let whites_max = edit(&ramp, json!({ "whites": 100 }));
        assert!(
            strip_mean(&f, &whites_max, 0, 256) > neutral + 15.0,
            "whites +100 must brighten: {neutral:.1} -> {:.1}",
            strip_mean(&f, &whites_max, 0, 256)
        );
        let whites_min = edit(&ramp, json!({ "whites": -100 }));
        assert!(
            pixel(&f, &whites_min, 255, 0).0 < 240,
            "whites -100 must dim the white end: white is {}",
            pixel(&f, &whites_min, 255, 0).0
        );

        // Blacks: + lifts the shadows, − crushes them.
        let blacks_up = edit(&ramp, json!({ "blacks": 60 }));
        assert!(
            pixel(&f, &blacks_up, 0, 0).0 > 25,
            "blacks +60 must lift black: 0 -> {}",
            pixel(&f, &blacks_up, 0, 0).0
        );
        let blacks_down = edit(&ramp, json!({ "blacks": -60 }));
        assert_eq!(
            pixel(&f, &blacks_down, 32, 0).0,
            0,
            "blacks -60 must crush the low end"
        );

        // Shadows: + lifts the dark end and keeps white; − crushes it.
        let shadows_up = edit(&ramp, json!({ "shadows": 60 }));
        assert!(
            pixel(&f, &shadows_up, 64, 0).0 > 90,
            "shadows +60 must lift the dark end: 64 -> {}",
            pixel(&f, &shadows_up, 64, 0).0
        );
        assert_eq!(
            pixel(&f, &shadows_up, 255, 0).0,
            255,
            "shadows must keep white"
        );
        let shadows_down = edit(&ramp, json!({ "shadows": -60 }));
        assert!(
            pixel(&f, &shadows_down, 64, 0).0 < 40,
            "shadows -60 must crush the dark end: 64 -> {}",
            pixel(&f, &shadows_down, 64, 0).0
        );

        // A near-neutral *sRGB* solid: `xc:gray` is a Grayscale image, so a channel
        // edit would be flattened back to grey when the PNG is written.
        let grey = solid(&f, "photo-grey.png", "8x8", "rgb(130,128,126)");
        let grey_value = pixel(&f, &grey, 4, 4).0;
        let brighter = edit(&grey, json!({ "brightness": 60 }));
        assert!(
            pixel(&f, &brighter, 4, 4).0 > grey_value + 20,
            "brightness +60 must brighten"
        );
        let darker = edit(&grey, json!({ "brightness": -60 }));
        assert!(
            pixel(&f, &darker, 4, 4).0 < grey_value - 20,
            "brightness -60 must darken"
        );
        let stop_up = edit(&grey, json!({ "exposure": 1 }));
        assert!(
            pixel(&f, &stop_up, 4, 4).0 >= 250,
            "exposure +1 must double the light"
        );
        let stop_down = edit(&grey, json!({ "exposure": -1 }));
        assert!(
            pixel(&f, &stop_down, 4, 4).0 < grey_value - 50,
            "exposure -1 must halve the light"
        );

        // Contrast: + spreads the ramp, − flattens it.
        let more_contrast = edit(&ramp, json!({ "contrast": 60 }));
        assert!(
            pixel(&f, &more_contrast, 64, 0).0 < 40 && pixel(&f, &more_contrast, 192, 0).0 > 240,
            "contrast +60 must spread the ramp"
        );
        let less_contrast = edit(&ramp, json!({ "contrast": -60 }));
        assert!(
            pixel(&f, &less_contrast, 64, 0).0 > 90 && pixel(&f, &less_contrast, 192, 0).0 < 170,
            "contrast -60 must flatten the ramp"
        );

        // Gamma: 1.0 is neutral, above 1 brightens and below 1 darkens.
        let gamma_up = edit(&grey, json!({ "gamma": 2.0 }));
        assert!(
            pixel(&f, &gamma_up, 4, 4).0 > grey_value + 20,
            "gamma above 1 must brighten"
        );
        let gamma_down = edit(&grey, json!({ "gamma": 0.5 }));
        assert!(
            pixel(&f, &gamma_down, 4, 4).0 < grey_value - 20,
            "gamma below 1 must darken"
        );

        // Temperature: + warmer (red up, blue down), − cooler.
        let (r0, _, b0) = pixel(&f, &grey, 4, 4);
        let warm = edit(&grey, json!({ "temperature": 60 }));
        let (r, _, b) = pixel(&f, &warm, 4, 4);
        assert!(
            r > b + 20 && b < b0 && r > r0,
            "temperature +60 must warm: {r} {b}"
        );
        let cool = edit(&grey, json!({ "temperature": -60 }));
        let (r, _, b) = pixel(&f, &cool, 4, 4);
        assert!(
            b > r + 20 && r < r0 && b > b0,
            "temperature -60 must cool: {r} {b}"
        );

        // Tint: + magenta (green down), − green.
        let magenta = edit(&grey, json!({ "tint": 60 }));
        let (r, g, b) = pixel(&f, &magenta, 4, 4);
        assert!(
            g + 12 < r && g + 12 < b,
            "tint +60 must move toward magenta: {r} {g} {b}"
        );
        let green = edit(&grey, json!({ "tint": -60 }));
        let (r, g, b) = pixel(&f, &green, 4, 4);
        assert!(
            g > r + 12 && g > b + 12,
            "tint -60 must move toward green: {r} {g} {b}"
        );

        // Hue: the slider is in degrees, so 60° is 60° and 180° is a half turn.
        let swatch = solid(&f, "photo-swatch.png", "8x8", "rgb(200,60,40)");
        let base_hue = hue_degrees(pixel(&f, &swatch, 4, 4));
        let turned = edit(&swatch, json!({ "hue": 60 }));
        let turned_hue = hue_degrees(pixel(&f, &turned, 4, 4));
        assert!(
            (turned_hue - (base_hue + 60.0)).abs() < 4.0,
            "hue +60 must rotate 60°: {base_hue:.1} -> {turned_hue:.1}"
        );
        let half = edit(&swatch, json!({ "hue": 180 }));
        let half_hue = hue_degrees(pixel(&f, &half, 4, 4));
        assert!(
            (half_hue - (base_hue + 180.0) % 360.0).abs() < 4.0,
            "hue +180 must be a half turn: {base_hue:.1} -> {half_hue:.1}"
        );

        // Saturation and vibrance: + more colour, − less.
        let muted = solid(&f, "photo-muted.png", "8x8", "rgb(160,120,120)");
        let spread = |pixel: (u8, u8, u8)| {
            pixel.0.max(pixel.1).max(pixel.2) - pixel.0.min(pixel.1).min(pixel.2)
        };
        let base_spread = spread(pixel(&f, &muted, 4, 4));
        let saturated = edit(&muted, json!({ "saturation": 60 }));
        assert!(
            spread(pixel(&f, &saturated, 4, 4)) > base_spread,
            "saturation +60 must add colour"
        );
        let vibrant = edit(&muted, json!({ "vibrance": 60 }));
        assert!(
            spread(pixel(&f, &vibrant, 4, 4)) > base_spread,
            "vibrance +60 must add colour"
        );
        let desaturated = edit(&muted, json!({ "saturation": -60 }));
        assert!(
            spread(pixel(&f, &desaturated, 4, 4)) < base_spread,
            "saturation -60 must remove colour"
        );

        // Dehaze: positive adds contrast, scaled by the value; negative veils.
        let dehaze_low = edit(&ramp, json!({ "dehaze": 10 }));
        let dehaze_high = edit(&ramp, json!({ "dehaze": 100 }));
        let (low, high) = (
            pixel(&f, &dehaze_low, 64, 0).0,
            pixel(&f, &dehaze_high, 64, 0).0,
        );
        assert!(
            high < low && low < 64,
            "dehaze must scale and add contrast at 64: 64 -> {low} -> {high}"
        );
        assert!(
            pixel(&f, &dehaze_high, 192, 0).0 > 195,
            "dehaze must lift the bright side near white"
        );
        let haze = edit(&ramp, json!({ "dehaze": -60 }));
        assert!(
            bottom(&haze) > neutral_bottom + 5.0 && top(&haze) < neutral_top - 2.0,
            "dehaze -60 must veil the picture: bottom {:.1}, top {:.1}",
            bottom(&haze),
            top(&haze)
        );

        // Clarity and sharpness sharpen the edge; negative clarity softens it.
        let step = make_image(
            &f,
            "photo-step.png",
            vec![
                os("-size"),
                os("64x16"),
                os("xc:rgb(102,102,102)"),
                os("-fill"),
                os("rgb(153,153,153)"),
                os("-draw"),
                os("rectangle 32,0 63,15"),
            ],
        );
        let (left, right) = (pixel(&f, &step, 31, 8).0, pixel(&f, &step, 32, 8).0);
        let clarity_up = edit(&step, json!({ "clarity": 60 }));
        assert!(
            pixel(&f, &clarity_up, 31, 8).0 < left && pixel(&f, &clarity_up, 32, 8).0 > right,
            "clarity +60 must sharpen the edge"
        );
        let clarity_down = edit(&step, json!({ "clarity": -60 }));
        assert!(
            pixel(&f, &clarity_down, 31, 8).0 > left + 5
                && pixel(&f, &clarity_down, 32, 8).0 < right - 5,
            "clarity -60 must soften the edge"
        );
        let sharper = edit(&step, json!({ "sharpness": 60 }));
        assert!(
            pixel(&f, &sharper, 31, 8).0 < left && pixel(&f, &sharper, 32, 8).0 > right,
            "sharpness +60 must sharpen the edge"
        );

        // Noise reduction removes texture; grain adds it.
        let noisy = make_image(
            &f,
            "photo-noise.png",
            vec![
                os("-seed"),
                os("42"),
                os("-size"),
                os("64x64"),
                os("xc:gray"),
                os("+noise"),
                os("Random"),
            ],
        );
        let before = deviation(&f, &noisy);
        let denoised = edit(&noisy, json!({ "noise": 100 }));
        let after = deviation(&f, &denoised);
        assert!(
            after < before * 0.9,
            "noise +100 must reduce texture: {before:.1} -> {after:.1}"
        );
        let grainy = edit(&grey, json!({ "grain": 100 }));
        assert!(
            deviation(&f, &grainy) > 10.0,
            "grain +100 must add texture: {:.1}",
            deviation(&f, &grainy)
        );
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
    #[cfg(unix)]
    fn compress_preview_matches_the_job_and_is_reused() {
        let Some(f) = fixture() else { return };
        let input = make_image(
            &f,
            "photo.jpg",
            vec![
                os("-size"),
                os("400x400"),
                os("xc:gray"),
                os("+noise"),
                os("Random"),
            ],
        );
        let job = Job::new(
            input.clone(),
            Intent::Tool {
                tool: ToolId::Compress,
                params: params(json!({ "quality": 60 })),
            },
        );
        let cache = crate::preview::PreviewCache::new(f._dir.path().join("previews"), 64 << 20);
        let preview = preview_engine(&f, &job, &cache).expect("preview");
        assert!(preview.path.starts_with(cache.root()));
        assert_eq!(
            preview.source_bytes,
            std::fs::metadata(&input).expect("metadata").len()
        );
        let (width, height) = ::image::image_dimensions(&preview.path).expect("decode preview");
        assert_eq!((preview.width, preview.height), (width, height));

        // The same request must resolve to the same artifact.
        let again = preview_engine(&f, &job, &cache).expect("second preview");
        assert_eq!(again.path, preview.path);

        // Prove the job reads the cached artifact instead of re-encoding: the source is
        // unreadable at this point, so only the reuse path can finish.
        use std::os::unix::fs::PermissionsExt;
        let original = std::fs::metadata(&input).expect("metadata").permissions();
        let mut locked = original.clone();
        locked.set_mode(0o000);
        std::fs::set_permissions(&input, locked).expect("lock the source");
        let readable = std::fs::File::open(&input).is_ok();
        let artifacts = run_engine_with(&f, &job, Some(&cache));
        std::fs::set_permissions(&input, original).expect("unlock the source");
        if readable {
            // Running as root: the permission lock says nothing, so this run only proves
            // the bytes match.
            let artifacts = artifacts.expect("compress");
            assert_eq!(
                std::fs::read(&artifacts[0]).expect("read"),
                std::fs::read(&preview.path).expect("read preview")
            );
            return;
        }
        let artifacts = artifacts.expect("the job must reuse the preview");
        assert_eq!(artifacts.len(), 1);
        assert_eq!(
            std::fs::read(&artifacts[0]).expect("read output"),
            std::fs::read(&preview.path).expect("read preview")
        );
    }

    #[test]
    fn lossy_extension_list_matches_the_dialog() {
        // The dialog hides the quality slider for exactly the extensions this returns
        // false for (`ui/src/format.ts`); keep the two lists in step.
        for ext in ["jpg", "jpeg", "webp", "avif", "heic", "heif"] {
            assert!(is_lossy_ext(ext), "{ext} should accept a quality");
        }
        for ext in ["png", "bmp", "tif", "tiff", "svg", "ico", "gif"] {
            assert!(!is_lossy_ext(ext), "{ext} ignores quality");
        }
    }

    #[test]
    fn compress_preview_changes_with_the_parameters() {
        let Some(f) = fixture() else { return };
        let input = make_image(
            &f,
            "photo2.jpg",
            vec![
                os("-size"),
                os("300x300"),
                os("xc:gray"),
                os("+noise"),
                os("Random"),
            ],
        );
        let cache = crate::preview::PreviewCache::new(f._dir.path().join("previews"), 64 << 20);
        let make = |quality: u32| {
            let job = Job::new(
                input.clone(),
                Intent::Tool {
                    tool: ToolId::Compress,
                    params: params(json!({ "quality": quality })),
                },
            );
            preview_engine(&f, &job, &cache).expect("preview")
        };
        let high = make(90);
        let low = make(20);
        assert_ne!(high.path, low.path);
        assert!(
            low.bytes < high.bytes,
            "quality 20 ({} B) is not smaller than quality 90 ({} B)",
            low.bytes,
            high.bytes
        );
    }

    /// PNG/BMP/TIFF are lossless: compress may only re-encode the same pixels more
    /// efficiently (or keep the original), never change the image.
    #[test]
    fn lossless_compression_keeps_every_pixel() {
        let Some(f) = fixture() else { return };
        let png = make_image(
            &f,
            "gradient.png",
            vec![os("-size"), os("600x400"), os("gradient:#2ea8a0-#12161b")],
        );
        let tiff = make_image(
            &f,
            "gradient.tif",
            vec![os("-size"), os("600x400"), os("gradient:#2ea8a0-#12161b")],
        );
        for (source, name) in [(png, "png"), (tiff, "tiff")] {
            let before = std::fs::metadata(&source).expect("metadata").len();
            let job = Job::new(
                source.clone(),
                Intent::Tool {
                    tool: ToolId::Compress,
                    params: params(json!({})),
                },
            );
            let artifacts = run_engine(&f, &job).expect("compress");
            let after = std::fs::metadata(&artifacts[0]).expect("metadata").len();
            assert!(after <= before, "{name}: {before} bytes grew to {after}");

            let original = ::image::open(&source).expect("decode source").to_rgb8();
            let output = ::image::open(&artifacts[0])
                .expect("decode output")
                .to_rgb8();
            assert_eq!(original.dimensions(), output.dimensions(), "{name}");
            assert!(
                original.as_raw() == output.as_raw(),
                "{name}: lossless compression changed pixels"
            );
        }
    }

    /// The TIFF encoder defaults to no compression: asking for LZW is the whole point.
    #[test]
    fn tiff_compress_actually_shrinks() {
        let Some(f) = fixture() else { return };
        let source = make_image(
            &f,
            "smooth.tif",
            vec![os("-size"), os("800x600"), os("gradient:#2ea8a0-#12161b")],
        );
        let before = std::fs::metadata(&source).expect("metadata").len();
        let job = Job::new(
            source,
            Intent::Tool {
                tool: ToolId::Compress,
                params: params(json!({})),
            },
        );
        let artifacts = run_engine(&f, &job).expect("compress");
        let after = std::fs::metadata(&artifacts[0]).expect("metadata").len();
        assert!(
            after < before,
            "an uncompressed TIFF was not compressed: {before} -> {after}"
        );
    }

    /// A scaled screenshot must be the *scaled* result, not a copy of the source.
    ///
    /// The never-grow rule once compared the raw encoder output with the source, before
    /// oxipng ran. A 16-bit screenshot's unoptimized encode was larger than the original,
    /// so the tool published the untouched source — at the source's dimensions — while
    /// the dialog said "600 × 400". Optimizing first and comparing after is the fix.
    #[test]
    fn a_scaled_screenshot_is_scaled_and_smaller() {
        let Some(f) = fixture() else { return };
        let source = make_image(
            &f,
            "screenshot.png",
            vec![
                os("-size"),
                os("1600x1000"),
                os("xc:#f4f4f2"),
                os("-fill"),
                os("#202020"),
                os("-draw"),
                os("rectangle 40,60 1560,120"),
                os("-draw"),
                os("rectangle 40,200 900,260"),
                os("-depth"),
                os("16"),
            ],
        );
        let job = Job::new(
            source,
            Intent::Tool {
                tool: ToolId::Compress,
                params: params(json!({ "scale": 50 })),
            },
        );
        let artifacts = run_engine(&f, &job).expect("compress");
        let (width, height) =
            crate::image_dimensions(&artifacts[0]).expect("the artifact is an image");
        assert_eq!(
            (width, height),
            (800, 500),
            "scale 50 halves the dimensions"
        );
    }

    /// q100 asks for more fidelity than the source has; the re-encode is larger, so the
    /// tool must return the original rather than inflate the file.
    #[test]
    fn compression_never_grows_a_jpeg() {
        let Some(f) = fixture() else { return };
        let source = make_image(
            &f,
            "small.jpg",
            vec![
                os("-size"),
                os("400x400"),
                os("xc:gray"),
                os("+noise"),
                os("Random"),
                os("-quality"),
                os("60"),
            ],
        );
        let original = std::fs::read(&source).expect("read source");
        let job = Job::new(
            source,
            Intent::Tool {
                tool: ToolId::Compress,
                params: params(json!({ "quality": 100 })),
            },
        );
        let artifacts = run_engine(&f, &job).expect("compress");
        let result = std::fs::read(&artifacts[0]).expect("read output");
        assert!(
            result.len() <= original.len(),
            "q100 grew {} bytes into {}",
            original.len(),
            result.len()
        );
        assert_eq!(
            result, original,
            "the result must be the original, not a larger encode"
        );
    }

    /// ImageMagick's encoder leaves a lot on the table; a stored-quality PNG must shrink
    /// without touching a pixel.
    #[test]
    fn png_optimizer_fixes_an_uncompressed_file() {
        let Some(f) = fixture() else { return };
        let source = make_image(
            &f,
            "stored.png",
            vec![
                os("-size"),
                os("800x600"),
                os("gradient:#2ea8a0-#12161b"),
                os("-define"),
                os("png:compression-level=0"),
            ],
        );
        let before = std::fs::metadata(&source).expect("metadata").len();
        let job = Job::new(
            source.clone(),
            Intent::Tool {
                tool: ToolId::Compress,
                params: params(json!({})),
            },
        );
        let artifacts = run_engine(&f, &job).expect("compress");
        let after = std::fs::metadata(&artifacts[0]).expect("metadata").len();
        assert!(
            after * 4 < before,
            "stored PNG {before} bytes only reached {after}"
        );
        let original = ::image::open(&source).expect("decode source").to_rgb8();
        let output = ::image::open(&artifacts[0])
            .expect("decode output")
            .to_rgb8();
        assert!(
            original.as_raw() == output.as_raw(),
            "PNG optimisation changed pixels"
        );
    }

    /// Colour type, palette length and dimensions of a PNG, read without decoding
    /// pixels: the indexed writer must produce a palette, not an RGB re-encode.
    fn indexed_png(path: &Path) -> (png::ColorType, usize, (u32, u32)) {
        let file = std::fs::File::open(path).expect("open the PNG");
        let decoder = png::Decoder::new(std::io::BufReader::new(file));
        let reader = decoder.read_info().expect("read the PNG info");
        let info = reader.info();
        (
            info.color_type,
            info.palette.as_ref().map_or(0, |palette| palette.len() / 3),
            (info.width, info.height),
        )
    }

    /// "Reduce colours" is opt-in and lossy: it must write an indexed PNG that is
    /// smaller than the lossless encode, keep the dimensions, and actually change pixels.
    #[test]
    fn reducing_colours_writes_a_smaller_indexed_png() {
        let Some(f) = fixture() else { return };
        let source = noisy_400(&f, "palette-source.png");
        let compress = |extra: Value| {
            let mut all = json!({});
            all.as_object_mut()
                .expect("object")
                .extend(extra.as_object().cloned().unwrap_or_default());
            let job = Job::new(
                source.clone(),
                Intent::Tool {
                    tool: ToolId::Compress,
                    params: params(all),
                },
            );
            run_engine(&f, &job).expect("compress")
        };

        let lossless = compress(json!({}));
        let reduced = compress(json!({ "colours": 16 }));
        let lossless_bytes = std::fs::metadata(&lossless[0]).expect("metadata").len();
        let reduced_bytes = std::fs::metadata(&reduced[0]).expect("metadata").len();
        assert!(
            reduced_bytes < lossless_bytes,
            "16 colours ({reduced_bytes} B) did not beat lossless ({lossless_bytes} B)"
        );

        let (colour_type, palette_len, dims) = indexed_png(&reduced[0]);
        assert_eq!(colour_type, png::ColorType::Indexed);
        assert!(
            (1..=16).contains(&palette_len),
            "palette has {palette_len} entries"
        );
        assert_eq!(dims, (400, 400), "palette reduction resized the image");

        let before = ::image::open(&source).expect("decode source").to_rgb8();
        let after = ::image::open(&reduced[0]).expect("decode output").to_rgb8();
        assert!(
            before.as_raw() != after.as_raw(),
            "a 16-colour palette must lose some colour: the request was lossy"
        );
    }

    /// A smooth gradient is where dithering makes a small palette larger than the
    /// lossless encode. It must fall back to a flat remap rather than to the source:
    /// choosing 16 colours has to change the file, not reproduce the lossless result.
    #[test]
    fn sixteen_colours_beat_the_lossless_fallback_on_a_gradient() {
        let Some(f) = fixture() else { return };
        let source = make_image(
            &f,
            "flat-gradient.png",
            vec![os("-size"), os("600x400"), os("gradient:#2ea8a0-#12161b")],
        );
        let compress = |extra: Value| {
            let job = Job::new(
                source.clone(),
                Intent::Tool {
                    tool: ToolId::Compress,
                    params: params(extra),
                },
            );
            run_engine(&f, &job).expect("compress")
        };
        let lossless = compress(json!({}));
        let reduced = compress(json!({ "colours": 16 }));
        let lossless_bytes = std::fs::metadata(&lossless[0]).expect("metadata").len();
        let reduced_bytes = std::fs::metadata(&reduced[0]).expect("metadata").len();
        assert!(
            reduced_bytes < lossless_bytes,
            "16 colours ({reduced_bytes} B) did not beat lossless ({lossless_bytes} B)"
        );
        let (colour_type, palette_len, dims) = indexed_png(&reduced[0]);
        assert_eq!(colour_type, png::ColorType::Indexed);
        assert!((2..=16).contains(&palette_len), "palette {palette_len}");
        assert_eq!(dims, (600, 400));
    }

    /// Target size and Reduce colours are one search, not two stages: the target ladder
    /// quantizes and downscales, so "both options" cannot return a file
    /// larger than either option alone.
    #[test]
    fn target_and_colours_are_searched_together() {
        let Some(f) = fixture() else { return };
        let source = noisy_400(&f, "combined.png");
        let compress = |extra: Value| {
            let job = Job::new(
                source.clone(),
                Intent::Tool {
                    tool: ToolId::Compress,
                    params: params(extra),
                },
            );
            run_engine(&f, &job).expect("compress")
        };
        let reduce_only = compress(json!({ "colours": 256 }));
        let target_only = compress(json!({ "target_bytes": 1_000 }));
        let both = compress(json!({ "colours": 256, "target_bytes": 1_000 }));
        let len = |paths: &[PathBuf]| std::fs::metadata(&paths[0]).expect("metadata").len();
        let (reduce_bytes, target_bytes, both_bytes) =
            (len(&reduce_only), len(&target_only), len(&both));
        assert!(
            both_bytes < reduce_bytes,
            "the target search ({both_bytes} B) did not beat palette-only ({reduce_bytes} B)"
        );
        assert!(
            both_bytes <= target_bytes,
            "choosing the same 256 colours made the target result larger ({both_bytes} B vs {target_bytes} B)"
        );
        let (colour_type, palette_len, _) = indexed_png(&both[0]);
        assert_eq!(colour_type, png::ColorType::Indexed);
        assert!(palette_len <= 256, "palette {palette_len}");

        // The dialog's preview is the same search. It must report the bytes the job
        // publishes, or "Run" looks like it ignored the options.
        let job = Job::new(
            source.clone(),
            Intent::Tool {
                tool: ToolId::Compress,
                params: params(json!({ "colours": 256, "target_bytes": 1_000 })),
            },
        );
        let cache = crate::preview::PreviewCache::new(f._dir.path().join("previews"), 64 << 20);
        let preview = preview_engine(&f, &job, &cache).expect("preview");
        assert_eq!(preview.bytes, both_bytes, "preview and job disagree");
        let artifacts = run_engine_with(&f, &job, Some(&cache)).expect("compress");
        assert_eq!(
            std::fs::read(&artifacts[0]).expect("read output"),
            std::fs::read(&preview.path).expect("read preview"),
            "the job must publish the previewed result"
        );
    }

    /// The preview and the job share `produce_compress`; for the palette path that also
    /// means the quantization must be deterministic, or the job would re-encode under
    /// the preview's key and produce different bytes.
    #[test]
    fn reduced_colour_preview_is_reused_by_the_job() {
        let Some(f) = fixture() else { return };
        let source = noisy_400(&f, "palette-preview.png");
        let job = Job::new(
            source,
            Intent::Tool {
                tool: ToolId::Compress,
                params: params(json!({ "colours": 64 })),
            },
        );
        let cache = crate::preview::PreviewCache::new(f._dir.path().join("previews"), 64 << 20);
        let preview = preview_engine(&f, &job, &cache).expect("preview");
        let again = preview_engine(&f, &job, &cache).expect("second preview");
        assert_eq!(
            again.path, preview.path,
            "the same request must hit the cache"
        );

        let artifacts = run_engine_with(&f, &job, Some(&cache)).expect("compress");
        assert_eq!(
            std::fs::read(&artifacts[0]).expect("read output"),
            std::fs::read(&preview.path).expect("read preview"),
            "the job must publish the previewed palette artifact"
        );
    }

    /// The indexed writer is the only place that emits tRNS; a transparency bug here
    /// would turn a transparent corner opaque.
    #[test]
    fn reducing_colours_keeps_transparency() {
        let Some(f) = fixture() else { return };
        let source = make_image(
            &f,
            "cutout.png",
            vec![
                os("-size"),
                os("200x200"),
                os("xc:none"),
                os("-fill"),
                os("#2ea8a0"),
                os("-draw"),
                os("circle 100,100 100,20"),
            ],
        );
        let job = Job::new(
            source,
            Intent::Tool {
                tool: ToolId::Compress,
                params: params(json!({ "colours": 16 })),
            },
        );
        let artifacts = run_engine(&f, &job).expect("compress");
        let (colour_type, _, _) = indexed_png(&artifacts[0]);
        assert_eq!(colour_type, png::ColorType::Indexed);
        let output = ::image::open(&artifacts[0])
            .expect("decode output")
            .to_rgba8();
        assert_eq!(output.get_pixel(0, 0)[3], 0, "the corner lost its alpha");
        assert_eq!(
            output.get_pixel(100, 100)[3],
            255,
            "the disc became transparent"
        );
    }

    /// The dialog hides the controls for non-PNG selections; the engine rejects the
    /// parameter rather than writing a palette PNG under a .jpg name.
    #[test]
    fn reducing_colours_refuses_a_non_png_output() {
        let Some(f) = fixture() else { return };
        let source = make_image(
            &f,
            "photo.jpg",
            vec![os("-size"), os("64x64"), os("xc:red")],
        );
        let job = Job::new(
            source,
            Intent::Tool {
                tool: ToolId::Compress,
                params: params(json!({ "colours": 16 })),
            },
        );
        let err = run_engine(&f, &job).expect_err("must reject");
        assert_eq!(err.code, ErrorCode::Unsupported);
    }

    /// SVG is read-only and rasterises to PNG; the result must be a real PNG, not a copy
    /// of the SVG under a .png name (that was the broken preview).
    #[test]
    fn compressing_an_svg_produces_a_png() {
        let Some(f) = fixture() else { return };
        let source = f._dir.path().join("drawing.svg");
        std::fs::write(
            &source,
            br##"<svg xmlns="http://www.w3.org/2000/svg" width="120" height="80"><rect width="120" height="80" fill="#2ea8a0"/></svg>"##,
        )
        .expect("write svg");
        let job = Job::new(
            source,
            Intent::Tool {
                tool: ToolId::Compress,
                params: params(json!({})),
            },
        );
        let artifacts = run_engine(&f, &job).expect("compress");
        assert_eq!(
            artifacts[0].extension().and_then(|e| e.to_str()),
            Some("png")
        );
        let bytes = std::fs::read(&artifacts[0]).expect("read output");
        assert!(
            bytes.starts_with(b"\x89PNG\r\n\x1a\n"),
            "the artifact is not a PNG: {:?}",
            String::from_utf8_lossy(&bytes[..bytes.len().min(40)])
        );
        let (width, height) = ::image::image_dimensions(&artifacts[0]).expect("decode output");
        assert_eq!((width, height), (120, 80));
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
