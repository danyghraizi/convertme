//! Video and audio: conversions and the FFmpeg-backed file tools.
//!
//! Every invocation is an argv array (`proc::Cmd`), every output is named in
//! `cx.work.out()`, and the source is only ever read. Encoders are selected
//! explicitly — a target container is never inferred from a temp file name — and
//! an encoder missing from the FFmpeg build is reported as `unsupported` with the
//! encoder's name instead of a cryptic muxer failure.
//!
//! Progress comes from `ffmpeg -progress pipe:1`, mapped against the duration
//! `ffprobe` reported. Long media jobs run on `Budget::Long`.

use crate::proc::{self, Budget, Cmd};
use crate::{Context, ensure_file};
use convertme_core::capability::ToolKey;
use convertme_core::error::{ErrorCode, JobError};
use convertme_core::formats::{Category, FormatId, Registry, ToolId};
use convertme_core::job::{Intent, Job, JobCtx, ToolParams};
use serde_json::Value;
use std::path::{Path, PathBuf};

/// `Snapshot` refuses to write more than this many stills in one job.
const MAX_FRAMES: usize = 200;

/// Remove a silent lead-in and tail, not the pauses inside the take: `stop_periods=1`
/// trims the trailing run only. `silencedetect` on the fixture (1 s each end) leaves
/// 2.02 s of a 4 s file.
const SILENCE_FILTER: &str =
    "silenceremove=start_periods=1:start_threshold=-50dB:stop_periods=1:stop_threshold=-50dB";

/// GIF output never exceeds this width, and never exceeds 15 fps.
const GIF_MAX_WIDTH: u32 = 640;
const GIF_FPS: f64 = 12.0;

/// The FFmpeg encoder set, checked against the probed build features.
struct Spec {
    container: &'static str,
    args: &'static [&'static str],
    needs: &'static [(&'static str, &'static str)],
    audio_only: bool,
}

const MP4_ARGS: &[&str] = &[
    "-c:v",
    "libx264",
    "-preset",
    "veryfast",
    "-crf",
    "20",
    "-pix_fmt",
    "yuv420p",
    "-c:a",
    "aac",
    "-b:a",
    "192k",
    "-movflags",
    "+faststart",
];
const MKV_ARGS: &[&str] = &[
    "-c:v", "libx264", "-preset", "veryfast", "-crf", "20", "-pix_fmt", "yuv420p", "-c:a", "ac3",
    "-b:a", "192k",
];
const WEBM_ARGS: &[&str] = &[
    "-c:v",
    "libvpx-vp9",
    "-b:v",
    "0",
    "-crf",
    "32",
    "-row-mt",
    "1",
    "-pix_fmt",
    "yuv420p",
    "-c:a",
    "libopus",
    "-b:a",
    "128k",
];
const AVI_ARGS: &[&str] = &[
    "-c:v",
    "libx264",
    "-preset",
    "veryfast",
    "-crf",
    "20",
    "-pix_fmt",
    "yuv420p",
    "-c:a",
    "libmp3lame",
    "-q:a",
    "2",
];
const WMV_ARGS: &[&str] = &[
    "-c:v", "wmv2", "-b:v", "2M", "-c:a", "wmav2", "-b:a", "192k",
];
const MP3_ARGS: &[&str] = &["-vn", "-c:a", "libmp3lame", "-q:a", "2"];
const M4A_ARGS: &[&str] = &[
    "-vn",
    "-c:a",
    "aac",
    "-b:a",
    "192k",
    "-movflags",
    "+faststart",
];
const WAV_ARGS: &[&str] = &["-vn", "-c:a", "pcm_s16le"];
const FLAC_ARGS: &[&str] = &["-vn", "-c:a", "flac", "-compression_level", "5"];
const OGG_ARGS: &[&str] = &["-vn", "-c:a", "libvorbis", "-q:a", "5"];
const OPUS_ARGS: &[&str] = &["-vn", "-c:a", "libopus", "-b:a", "128k"];
const AIFF_ARGS: &[&str] = &["-vn", "-c:a", "pcm_s16le"];
const WMA_ARGS: &[&str] = &["-vn", "-c:a", "wmav2", "-b:a", "192k"];
const GIF_ARGS: &[&str] = &[];

/// The explicit encoder recipe for a conversion target, or `None` when the
/// target is not something this engine produces.
fn spec_for(target: FormatId) -> Option<Spec> {
    Some(match target.0 {
        "MP4" => Spec {
            container: "mp4",
            args: MP4_ARGS,
            needs: &[("libx264", "libx264")],
            audio_only: false,
        },
        "MOV" => Spec {
            container: "mov",
            args: MP4_ARGS,
            needs: &[("libx264", "libx264")],
            audio_only: false,
        },
        "MKV" => Spec {
            container: "matroska",
            args: MKV_ARGS,
            needs: &[("libx264", "libx264")],
            audio_only: false,
        },
        "WEBM" => Spec {
            container: "webm",
            args: WEBM_ARGS,
            needs: &[("libvpx", "libvpx-vp9"), ("libopus", "libopus")],
            audio_only: false,
        },
        "AVI" => Spec {
            container: "avi",
            args: AVI_ARGS,
            needs: &[("libx264", "libx264"), ("lame", "libmp3lame")],
            audio_only: false,
        },
        "WMV" => Spec {
            container: "asf",
            args: WMV_ARGS,
            needs: &[],
            audio_only: false,
        },
        "GIF" => Spec {
            container: "gif",
            args: GIF_ARGS,
            needs: &[],
            audio_only: false,
        },
        "MP3" => Spec {
            container: "mp3",
            args: MP3_ARGS,
            needs: &[("lame", "libmp3lame")],
            audio_only: true,
        },
        "M4A" => Spec {
            container: "ipod",
            args: M4A_ARGS,
            needs: &[],
            audio_only: true,
        },
        "WAV" => Spec {
            container: "wav",
            args: WAV_ARGS,
            needs: &[],
            audio_only: true,
        },
        "FLAC" => Spec {
            container: "flac",
            args: FLAC_ARGS,
            needs: &[],
            audio_only: true,
        },
        "OGG" => Spec {
            container: "ogg",
            args: OGG_ARGS,
            needs: &[("libvorbis", "libvorbis")],
            audio_only: true,
        },
        "OPUS" => Spec {
            container: "opus",
            args: OPUS_ARGS,
            needs: &[("libopus", "libopus")],
            audio_only: true,
        },
        "AIFF" => Spec {
            container: "aiff",
            args: AIFF_ARGS,
            needs: &[],
            audio_only: true,
        },
        "WMA" => Spec {
            container: "asf",
            args: WMA_ARGS,
            needs: &[],
            audio_only: true,
        },
        _ => return None,
    })
}

/// Cheap parameter validation, run on submit and again before work starts.
pub fn validate(job: &Job, category: Category) -> Result<(), JobError> {
    match &job.intent {
        Intent::Convert { target } => spec_for(*target)
            .map(|_| ())
            .ok_or_else(|| invalid(format!("{target} is not an audio or video output"))),
        Intent::Tool { tool, .. } => {
            if !matches!(category, Category::Video | Category::Audio) {
                return Ok(());
            }
            match tool {
                ToolId::Compress => validate_compress(job, category),
                ToolId::Trim => {
                    trim_range(job)?;
                    if let Some(params) = job.intent.params()
                        && params.contains_key("trim_silence")
                        && bool_param(Some(params), "trim_silence").is_none()
                    {
                        return Err(invalid("trim_silence must be true or false"));
                    }
                    Ok(())
                }
                ToolId::Crop => crop_spec(job).map(|_| ()),
                ToolId::Speed => speed_factor(job).map(|_| ()),
                ToolId::Mute => Ok(()),
                ToolId::Normalize => normalize_targets(job).map(|_| ()),
                ToolId::Snapshot => validate_snapshot(job),
                ToolId::Join => validate_join(job),
                ToolId::Split => sections_param(job).map(|_| ()),
                ToolId::Channels => channels_param(job).map(|_| ()),
                ToolId::Visualize => validate_visualize(job),
                ToolId::Bleep => ranges_param(job).map(|_| ()),
                // Metadata and Redact are routed to their own modules.
                _ => Ok(()),
            }
        }
    }
}

pub fn run(job: &Job, cx: &Context<'_>) -> Result<Vec<PathBuf>, JobError> {
    let category = category_of(job)?;
    match &job.intent {
        Intent::Convert { target } => convert(job, *target, category, cx),
        Intent::Tool { tool, .. } => {
            validate(job, category)?;
            match tool {
                ToolId::Compress => compress(job, category, cx),
                ToolId::Trim => trim(job, category, cx),
                ToolId::Crop => crop(job, cx),
                ToolId::Speed => speed(job, cx),
                ToolId::Mute => mute(job, cx),
                ToolId::Snapshot => snapshot(job, cx),
                ToolId::Join => join(job, cx),
                ToolId::Split => split(job, cx),
                ToolId::Normalize => normalize(job, cx),
                ToolId::Channels => channels(job, cx),
                ToolId::Visualize => visualize(job, cx),
                ToolId::Bleep => bleep(job, cx),
                other => Err(invalid(format!("{other} is not an audio or video tool"))),
            }
        }
    }
}

// ---------------------------------------------------------------- conversions

fn convert(
    job: &Job,
    target: FormatId,
    category: Category,
    cx: &Context<'_>,
) -> Result<Vec<PathBuf>, JobError> {
    let spec = spec_for(target)
        .ok_or_else(|| invalid(format!("{target} is not an audio or video output")))?;
    for (feature, encoder) in spec.needs {
        require_encoder(cx, feature, encoder)?;
    }
    let ffmpeg = cx.tools.path(ToolKey::Ffmpeg)?;
    let info = probe(cx, &job.source)?;

    if target.as_str() == "GIF" {
        return convert_gif(job, cx, &ffmpeg, &info);
    }
    if spec.audio_only && !info.has_audio {
        return Err(invalid("the input has no audio track"));
    }

    let ext = crate::ext_of(target);
    let output = cx.out(&out_name(job, None, ext));
    cx.ctx.set_stage(format!("converting to {ext}"));

    let mut cmd = ffmpeg_cmd(&ffmpeg);
    cmd = cmd.arg("-i").arg(&job.source);
    for arg in spec.args {
        cmd = cmd.arg(arg);
    }
    cmd = cmd.args(["-f", spec.container]).arg(&output);

    run_ffmpeg(cmd, cx, budget_for(category), info.duration, 0.0, 1.0)?;
    ensure_file(&output, "ffmpeg")?;
    Ok(vec![output])
}

/// GIF needs a generated palette: one pass to build it, one to apply it.
fn convert_gif(
    job: &Job,
    cx: &Context<'_>,
    ffmpeg: &Path,
    info: &MediaInfo,
) -> Result<Vec<PathBuf>, JobError> {
    let output = cx.out(&out_name(job, None, "gif"));
    encode_gif_palette(
        job,
        cx,
        ffmpeg,
        info.duration,
        GIF_FPS,
        GIF_MAX_WIDTH,
        &output,
    )?;
    Ok(vec![output])
}

fn encode_gif_palette(
    job: &Job,
    cx: &Context<'_>,
    ffmpeg: &Path,
    duration: f64,
    fps: f64,
    max_width: u32,
    output: &Path,
) -> Result<(), JobError> {
    let palette = cx.scratch("palette.png");
    let scale = format!("scale='min({max_width},iw)':-1:flags=lanczos");

    cx.ctx.set_stage("building the GIF palette");
    let palette_filter = format!("fps={fps},{scale},palettegen=stats_mode=diff");
    let mut build = ffmpeg_cmd(ffmpeg);
    build = build
        .arg("-i")
        .arg(&job.source)
        .arg("-vf")
        .arg(&palette_filter)
        .args(["-frames:v", "1", "-an"])
        .arg(&palette);
    run_ffmpeg(build, cx, Budget::Long, duration, 0.0, 0.35)?;
    ensure_file(&palette, "the GIF palette pass")?;

    cx.ctx.set_stage("encoding the GIF");
    cx.ctx.checkpoint()?;
    let apply = format!("fps={fps},{scale}[x];[x][1:v]paletteuse=dither=bayer:bayer_scale=5");
    let mut encode = ffmpeg_cmd(ffmpeg);
    encode = encode
        .arg("-i")
        .arg(&job.source)
        .arg("-i")
        .arg(&palette)
        .arg("-filter_complex")
        .arg(&apply)
        .arg("-an")
        .args(["-f", "gif"])
        .arg(output);
    run_ffmpeg(encode, cx, Budget::Long, duration, 0.35, 0.65)?;
    ensure_file(output, "ffmpeg")?;
    Ok(())
}

/// GIF has no bitrate control, so a size target becomes a smaller frame and a
/// lower ceiling, still through the palette passes.
fn compress_gif(
    job: &Job,
    cx: &Context<'_>,
    ffmpeg: &Path,
    info: &MediaInfo,
    target: f64,
) -> Result<Vec<PathBuf>, JobError> {
    let output = cx.out(&out_name(job, Some("compressed"), "gif"));
    let ratio = if info.size > 0 {
        (target / info.size as f64).clamp(0.05, 1.0)
    } else {
        0.5
    };
    let width = ((GIF_MAX_WIDTH as f64 * ratio.sqrt()).round() as u32).clamp(32, GIF_MAX_WIDTH);
    let fps = (GIF_FPS * ratio.sqrt().max(0.6)).clamp(6.0, GIF_FPS);
    encode_gif_palette(
        job,
        cx,
        ffmpeg,
        info.duration,
        fps,
        even(width).max(32),
        &output,
    )?;
    Ok(vec![output])
}

// --------------------------------------------------------------------- tools

fn compress(job: &Job, category: Category, cx: &Context<'_>) -> Result<Vec<PathBuf>, JobError> {
    let info = probe(cx, &job.source)?;
    let ffmpeg = cx.tools.path(ToolKey::Ffmpeg)?;
    let ext = source_ext(job);
    let budget = budget_for(category);
    let artifacts = if matches!(category, Category::Video) {
        compress_video(job, cx, &ffmpeg, &ext, &info, budget)?
    } else {
        compress_audio(job, cx, &ffmpeg, &ext, &info, budget)?
    };

    // A compressor that grows a file is not a compressor (the PDF tool has the same
    // rule). Some encoders cannot reach the requested bitrate on a short clip; the
    // result is then a copy of the original, never a larger file.
    if let Some(out) = artifacts.first() {
        let produced = std::fs::metadata(out).map(|meta| meta.len()).unwrap_or(0);
        if produced >= info.size {
            std::fs::copy(&job.source, out)
                .map_err(|e| JobError::io("keeping the original instead", &e))?;
        }
    }
    Ok(artifacts)
}

fn validate_compress(job: &Job, category: Category) -> Result<(), JobError> {
    let p = job.intent.params();
    let bytes = num_param(p, "target_bytes").filter(|v| *v > 0.0);
    let percent = num_param(p, "percent").filter(|v| *v > 0.0 && *v <= 100.0);
    if matches!(category, Category::Video) {
        if bytes.is_none() && percent.is_none() {
            return Err(invalid(
                "compress needs target_bytes or percent between 0 and 100",
            ));
        }
    } else {
        let bitrate = num_param(p, "bitrate").filter(|v| *v >= 8.0 && *v <= 1024.0);
        if bytes.is_none() && percent.is_none() && bitrate.is_none() {
            return Err(invalid(
                "audio compress needs bitrate (kbps), target_bytes or percent",
            ));
        }
        // WAV/AIFF/FLAC keep every sample; a size or bitrate target cannot apply without
        // changing the format, so refusing beats silently writing the same file back.
        let ext = source_ext(job);
        if is_lossless(&ext) {
            return Err(invalid(format!(
                "{ext} is lossless; convert it to MP3, OGG or Opus to compress it"
            )));
        }
    }
    Ok(())
}

fn compress_video(
    job: &Job,
    cx: &Context<'_>,
    ffmpeg: &Path,
    ext: &str,
    info: &MediaInfo,
    budget: Budget,
) -> Result<Vec<PathBuf>, JobError> {
    let p = job.intent.params();
    let target = match (num_param(p, "target_bytes"), num_param(p, "percent")) {
        (Some(bytes), _) if bytes > 0.0 => bytes,
        (_, Some(percent)) if percent > 0.0 && percent <= 100.0 => {
            info.size as f64 * percent / 100.0
        }
        _ => return Err(invalid("compress needs target_bytes or percent")),
    };
    if !info.has_video {
        return Err(invalid("the input has no video track"));
    }
    if info.duration <= 0.0 {
        return Err(JobError::new(
            ErrorCode::EngineFailed,
            "cannot determine the duration of the input",
        ));
    }
    if ext == "gif" {
        return compress_gif(job, cx, ffmpeg, info, target);
    }
    let audio_kbps = num_param(p, "audio_kbps")
        .filter(|v| *v >= 32.0 && *v <= 512.0)
        .unwrap_or(128.0);
    let total_kbps = target * 8.0 / info.duration / 1000.0;
    // A small target is legitimate (a 2 s clip at 50 %). Give the audio stream what is
    // left rather than refusing the encode; only refuse when even a minimal audio
    // track plus video cannot fit.
    if total_kbps < 48.0 {
        return Err(invalid(format!(
            "a target of {:.1} MB is too small for {:.1} s of video",
            target / 1_000_000.0,
            info.duration
        )));
    }
    let audio_kbps = audio_kbps
        .min((total_kbps * 0.3).max(32.0))
        .min(total_kbps - 16.0);
    let video_kbps = (total_kbps - audio_kbps).floor().max(16.0);

    let (encoder, pass_supported) = match ext {
        "webm" => {
            require_encoder(cx, "libvpx", "libvpx-vp9")?;
            ("libvpx-vp9", true)
        }
        "wmv" => ("wmv2", false),
        _ => {
            if cx.tools.feature(ToolKey::Ffmpeg, "libx264") {
                ("libx264", true)
            } else {
                ("mpeg4", true)
            }
        }
    };
    let x264 = encoder == "libx264";
    let audio_codec = match ext {
        "webm" => "libopus",
        "avi" => "libmp3lame",
        "wmv" => "wmav2",
        _ => "aac",
    };
    if let Some(feature) = encoder_feature(audio_codec) {
        require_encoder(cx, feature, audio_codec)?;
    }

    let bitrate = format!("{}k", video_kbps as u64);
    let audio_bitrate = format!("{}k", audio_kbps as u64);
    let passlog = cx.scratch("ffmpeg-pass");
    let output = cx.out(&out_name(job, Some("compressed"), ext));
    let two_pass = info.duration > 60.0 && pass_supported;
    let muxer = muxer_for_ext(ext);

    if two_pass {
        cx.ctx.set_stage("compressing, pass 1 of 2");
        let mut first = ffmpeg_cmd(ffmpeg);
        first = first.arg("-i").arg(&job.source).args(["-c:v", encoder]);
        if x264 {
            first = first.args(["-preset", "veryfast"]);
        }
        first = first
            .arg("-b:v")
            .arg(&bitrate)
            .args(["-pass", "1", "-passlogfile"])
            .arg(&passlog)
            .args(["-an", "-f", "null"])
            .arg(null_sink());
        run_ffmpeg(first, cx, budget, info.duration, 0.0, 0.4)?;
        cx.ctx.checkpoint()?;
        cx.ctx.set_stage("compressing, pass 2 of 2");
    } else {
        cx.ctx.set_stage("compressing");
    }

    let mut second = ffmpeg_cmd(ffmpeg);
    second = second.arg("-i").arg(&job.source).args(["-c:v", encoder]);
    if x264 {
        second = second.args(["-preset", "veryfast"]);
    }
    second = second.arg("-b:v").arg(&bitrate);
    if two_pass {
        second = second.args(["-pass", "2", "-passlogfile"]).arg(&passlog);
    }
    second = second
        .args(["-c:a", audio_codec])
        .arg("-b:a")
        .arg(&audio_bitrate);
    if let Some(m) = muxer {
        second = second.args(["-f", m]);
    }
    second = second.arg(&output);

    let (base, span) = if two_pass { (0.4, 0.6) } else { (0.0, 1.0) };
    run_ffmpeg(second, cx, budget, info.duration, base, span)?;
    ensure_file(&output, "ffmpeg")?;
    Ok(vec![output])
}

fn compress_audio(
    job: &Job,
    cx: &Context<'_>,
    ffmpeg: &Path,
    ext: &str,
    info: &MediaInfo,
    budget: Budget,
) -> Result<Vec<PathBuf>, JobError> {
    if !info.has_audio {
        return Err(invalid("the input has no audio track"));
    }
    if is_lossless(ext) {
        return Err(invalid(format!(
            "{ext} is lossless; convert it to MP3, OGG or Opus to compress it"
        )));
    }
    let p = job.intent.params();
    let bitrate = num_param(p, "bitrate").filter(|v| *v >= 8.0 && *v <= 1024.0);
    // A relative target is the default the dialog sends: it cannot exceed the source.
    let target = num_param(p, "target_bytes")
        .filter(|v| *v > 0.0)
        .or_else(|| {
            num_param(p, "percent")
                .filter(|v| *v > 0.0 && *v <= 100.0)
                .map(|percent| info.size as f64 * percent / 100.0)
        });
    let codec = lossy_audio_codec(ext);
    let kbps = match (bitrate, target) {
        (Some(b), _) => b,
        (None, Some(target)) if info.duration > 0.0 => (target * 8.0 / info.duration / 1000.0)
            .floor()
            .clamp(8.0, 512.0),
        _ => 128.0,
    };
    // libvorbis (and wmav2) refuse to open the encoder below 32 kbps; a relative
    // target on a small file lands under it. Clamp per codec rather than fail.
    let kbps = kbps.max(min_audio_kbps(codec)).min(512.0);

    let output = cx.out(&out_name(job, Some("compressed"), ext));
    cx.ctx.set_stage("compressing audio");
    let mut cmd = ffmpeg_cmd(ffmpeg);
    cmd = cmd.arg("-i").arg(&job.source).arg("-vn");

    if let Some(feature) = encoder_feature(codec) {
        require_encoder(cx, feature, codec)?;
    }
    cmd = cmd
        .args(["-c:a", codec])
        .arg("-b:a")
        .arg(format!("{}k", kbps as u64));
    if let Some(m) = muxer_for_ext(ext) {
        cmd = cmd.args(["-f", m]);
    }
    cmd = cmd.arg(&output);

    run_ffmpeg(cmd, cx, budget, info.duration, 0.0, 1.0)?;
    ensure_file(&output, "ffmpeg")?;
    Ok(vec![output])
}

fn trim(job: &Job, category: Category, cx: &Context<'_>) -> Result<Vec<PathBuf>, JobError> {
    let (start, end) = trim_range(job)?;
    let ffmpeg = cx.tools.path(ToolKey::Ffmpeg)?;
    let info = probe(cx, &job.source)?;
    let ext = source_ext(job);
    let output = cx.out(&out_name(job, Some("trimmed"), &ext));
    let budget = budget_for(category);
    // Only the audio tool offers it: on a video the picture would keep its silent
    // lead-in while the sound moved, which is a desynchronised clip, not a trim.
    let trim_silence = matches!(category, Category::Audio)
        && bool_param(job.intent.params(), "trim_silence").unwrap_or(false);
    cx.ctx.set_stage("trimming");

    let mut cmd = ffmpeg_cmd(&ffmpeg);
    if start > 0.0 {
        cmd = cmd.arg("-ss").arg(secs(start));
    }
    cmd = cmd.arg("-i").arg(&job.source);
    if let Some(end) = end {
        cmd = cmd.arg("-t").arg(secs((end - start).max(0.0)));
    }
    if trim_silence {
        cmd = cmd.arg("-af").arg(SILENCE_FILTER);
    }

    // A filter means the stream is re-encoded; `-c copy` with `-af` silently drops
    // the filter in some ffmpeg builds and would publish the silence back.
    if is_cut_friendly(&ext) && !trim_silence {
        // `-avoid_negative_ts make_zero` grows a seeked copy on this muxer (2→5 s
    // produced 5.09 s): the default keeps the output at the requested length.
    cmd = cmd.args(["-c", "copy"]);
    } else {
        if matches!(category, Category::Video) {
            for arg in video_encode_args(cx, &ext)? {
                cmd = cmd.arg(arg);
            }
        }
        if info.has_audio
            && let Some(args) = audio_encode_args(cx, &ext)?
        {
            for arg in args {
                cmd = cmd.arg(arg);
            }
        }
    }
    if let Some(m) = muxer_for_ext(&ext) {
        cmd = cmd.args(["-f", m]);
    }
    cmd = cmd.arg(&output);

    let output_duration = match end {
        Some(end) => (end - start).max(0.0),
        None => (info.duration - start).max(0.0),
    };
    run_ffmpeg(cmd, cx, budget, output_duration, 0.0, 1.0)?;
    ensure_file(&output, "ffmpeg")?;
    Ok(vec![output])
}

fn trim_range(job: &Job) -> Result<(f64, Option<f64>), JobError> {
    let p = job.intent.params();
    let start = num_param(p, "start");
    let end = num_param(p, "end");
    if start.is_none() && end.is_none() {
        return Err(invalid("trim needs a start or an end time"));
    }
    let start = start.unwrap_or(0.0);
    if !start.is_finite() || start < 0.0 {
        return Err(invalid(
            "trim start must be a non-negative number of seconds",
        ));
    }
    if let Some(end) = end {
        if !end.is_finite() || end < 0.0 {
            return Err(invalid("trim end must be a non-negative number of seconds"));
        }
        if end <= start {
            return Err(invalid("trim end must be after the start"));
        }
    }
    Ok((start, end))
}

fn mute(job: &Job, cx: &Context<'_>) -> Result<Vec<PathBuf>, JobError> {
    let ffmpeg = cx.tools.path(ToolKey::Ffmpeg)?;
    let info = probe(cx, &job.source)?;
    if !info.has_video {
        return Err(invalid("the input has no video track"));
    }
    let ext = source_ext(job);
    let output = cx.out(&out_name(job, Some("muted"), &ext));
    cx.ctx.set_stage("removing the audio track");

    let mut cmd = ffmpeg_cmd(&ffmpeg);
    cmd = cmd
        .arg("-i")
        .arg(&job.source)
        .args(["-map", "0:v:0", "-c:v", "copy", "-an"]);
    if let Some(m) = muxer_for_ext(&ext) {
        cmd = cmd.args(["-f", m]);
    }
    cmd = cmd.arg(&output);

    run_ffmpeg(cmd, cx, Budget::Long, info.duration, 0.0, 1.0)?;
    ensure_file(&output, "ffmpeg")?;
    Ok(vec![output])
}

fn snapshot(job: &Job, cx: &Context<'_>) -> Result<Vec<PathBuf>, JobError> {
    let ffmpeg = cx.tools.path(ToolKey::Ffmpeg)?;
    let times = times_param(job)?;
    let mut artifacts = Vec::new();

    if !times.is_empty() {
        let total = times.len();
        for (index, time) in times.iter().enumerate() {
            cx.ctx.checkpoint()?;
            cx.ctx
                .set_stage(format!("saving frame {} of {total}", index + 1));
            let output = cx.out(&format!("frame-{:03}.png", index + 1));
            let mut cmd = ffmpeg_cmd(&ffmpeg);
            cmd = cmd
                .arg("-ss")
                .arg(secs(*time))
                .arg("-i")
                .arg(&job.source)
                .args(["-frames:v", "1", "-an", "-f", "image2"])
                .arg(&output);
            let result = proc::run(cmd.budget(Budget::Short).stdout_piped(), cx.ctx)?;
            result.ok()?;
            ensure_file(&output, "ffmpeg")?;
            artifacts.push(output);
            cx.ctx.set_progress((index + 1) as f64 / total as f64);
        }
        return Ok(artifacts);
    }

    let p = job.intent.params();
    let interval = num_param(p, "interval").filter(|v| *v > 0.0);
    let fps = num_param(p, "fps").filter(|v| *v > 0.0);
    let rate = interval
        .map(|i| 1.0 / i)
        .or(fps)
        .map(|r| r.clamp(0.01, 60.0))
        .unwrap_or(1.0);
    let info = probe(cx, &job.source)?;
    let pattern = cx.out("frame-%03d.png");
    cx.ctx.set_stage("saving frames");

    let mut cmd = ffmpeg_cmd(&ffmpeg);
    cmd = cmd
        .arg("-i")
        .arg(&job.source)
        .arg("-vf")
        .arg(format!("fps={rate}"))
        .args(["-frames:v", "200", "-an", "-f", "image2"])
        .arg(&pattern);
    run_ffmpeg(cmd, cx, Budget::Long, info.duration, 0.0, 1.0)?;

    let mut frames: Vec<PathBuf> = std::fs::read_dir(cx.work.out())
        .map_err(|e| JobError::io("listing the snapshot frames", &e))?
        .filter_map(|entry| entry.ok())
        .map(|entry| entry.path())
        .filter(|path| {
            path.file_name()
                .and_then(|name| name.to_str())
                .map(|name| name.starts_with("frame-") && name.ends_with(".png"))
                .unwrap_or(false)
        })
        .collect();
    frames.sort();
    if frames.is_empty() {
        return Err(JobError::new(
            ErrorCode::EngineFailed,
            "ffmpeg wrote no frames",
        ));
    }
    for frame in &frames {
        ensure_file(frame, "ffmpeg")?;
    }
    cx.ctx.set_progress(1.0);
    Ok(frames)
}

fn validate_snapshot(job: &Job) -> Result<(), JobError> {
    let times = times_param(job)?;
    let p = job.intent.params();
    let interval = num_param(p, "interval").filter(|v| *v > 0.0);
    let fps = num_param(p, "fps").filter(|v| *v > 0.0);
    if times.is_empty() && interval.is_none() && fps.is_none() {
        return Err(invalid("snapshot needs times, interval or fps"));
    }
    Ok(())
}

/// The inputs in the dialog's order. The editor sends every index; a missing key (a
/// tool run from the CLI) keeps the selection's order.
fn ordered_inputs(job: &Job) -> Result<Vec<PathBuf>, JobError> {
    let count = job.inputs.len();
    let Some(value) = job.intent.params().and_then(|p| p.get("order")) else {
        return Ok(job.inputs.clone());
    };
    let array = value
        .as_array()
        .ok_or_else(|| invalid("order must be an array of input indices"))?;
    if array.is_empty() {
        return Ok(job.inputs.clone());
    }
    let mut seen = vec![false; count];
    let mut ordered = Vec::with_capacity(count);
    for item in array {
        let index = item
            .as_u64()
            .ok_or_else(|| invalid("order must be an array of input indices"))?
            as usize;
        if index >= count || seen[index] {
            return Err(invalid("order must list every input exactly once"));
        }
        seen[index] = true;
        ordered.push(job.inputs[index].clone());
    }
    if ordered.len() != count {
        return Err(invalid("order must list every input exactly once"));
    }
    Ok(ordered)
}

fn validate_join(job: &Job) -> Result<(), JobError> {
    if job.inputs.len() < 2 {
        return Err(invalid("join needs at least two clips"));
    }
    ordered_inputs(job).map(|_| ())
}

fn join(job: &Job, cx: &Context<'_>) -> Result<Vec<PathBuf>, JobError> {
    let inputs = ordered_inputs(job)?;
    let ffmpeg = cx.tools.path(ToolKey::Ffmpeg)?;
    let ext = source_ext(job);
    let list = cx.scratch("concat.txt");
    let mut body = String::new();
    for input in &inputs {
        body.push_str(&format!("file '{}'\n", concat_escape(input)));
    }
    std::fs::write(&list, body.as_bytes())
        .map_err(|e| JobError::io("writing the concat list", &e))?;

    let mut total = 0.0;
    let mut same_canvas = true;
    let mut first_facts: Option<MediaInfo> = None;
    for input in &inputs {
        let facts = probe(cx, input).ok();
        total += facts.map(|info| info.duration).unwrap_or(0.0);
        if let Some(facts) = facts {
            match first_facts {
                None => first_facts = Some(facts),
                Some(first) => {
                    // The concat demuxer appends packets; it can only be copied when
                    // every clip has the same frame size and audio layout.
                    if facts.width != first.width
                        || facts.height != first.height
                        || facts.has_audio != first.has_audio
                        || facts.audio_channels != first.audio_channels
                        || facts.audio_rate != first.audio_rate
                    {
                        same_canvas = false;
                    }
                }
            }
        }
    }

    let output = cx.out(&out_name(job, Some("joined"), &ext));
    let muxer = muxer_for_ext(&ext);
    let budget = Budget::Long;
    cx.ctx.set_stage("joining clips");

    if same_canvas {
        let mut copy_cmd = ffmpeg_cmd(&ffmpeg);
        copy_cmd = copy_cmd
            .args(["-f", "concat", "-safe", "0", "-i"])
            .arg(&list)
            .args(["-c", "copy"]);
        if let Some(m) = muxer {
            copy_cmd = copy_cmd.args(["-f", m]);
        }
        copy_cmd = copy_cmd.arg(&output);

        match run_ffmpeg(copy_cmd, cx, budget, total, 0.0, 1.0) {
            Ok(()) => {}
            Err(err) if err.code == ErrorCode::EngineFailed => {
                // Streams differ between the clips despite the probe; re-encode.
                cx.ctx.checkpoint()?;
                cx.ctx.set_stage("re-encoding the joined clip");
                join_reencode(cx, &ffmpeg, &inputs, &ext, muxer, total, &output)?;
            }
            Err(err) => return Err(err),
        }
    } else {
        // The canvas differs: re-encode every clip onto the first clip's frame.
        join_reencode(cx, &ffmpeg, &inputs, &ext, muxer, total, &output)?;
    }
    ensure_file(&output, "ffmpeg")?;
    Ok(vec![output])
}

/// The re-encode path: scale and pad each clip onto the first clip's canvas, resample
/// its audio to the first clip's rate, then `concat` the normalised streams.
///
/// The concat *demuxer* appends packets, so a second clip with another frame size
/// cannot be normalised after it — the filter has one input by then. A filter graph
/// with one input per clip can scale before the join.
fn join_reencode(
    cx: &Context<'_>,
    ffmpeg: &Path,
    inputs: &[PathBuf],
    ext: &str,
    muxer: Option<&'static str>,
    total: f64,
    output: &Path,
) -> Result<(), JobError> {
    let first = probe(cx, &inputs[0])?;
    if !first.has_video {
        return Err(invalid("the first clip has no video track"));
    }
    let width = even(first.width.max(2));
    let height = even(first.height.max(2));
    let all_video = inputs.iter().all(|input| {
        probe(cx, input)
            .map(|info| info.has_video)
            .unwrap_or(false)
    });
    if !all_video {
        return Err(invalid("every clip needs a video track to be joined"));
    }
    let audio_all = first.has_audio
        && inputs.iter().all(|input| {
            probe(cx, input)
                .map(|info| info.has_audio)
                .unwrap_or(false)
        });
    let rate = if first.audio_rate > 0 {
        first.audio_rate
    } else {
        48_000
    };

    let mut cmd = ffmpeg_cmd(ffmpeg);
    for input in inputs {
        cmd = cmd.arg("-i").arg(input);
    }

    let mut graph = String::new();
    for (index, _) in inputs.iter().enumerate() {
        graph.push_str(&format!(
            "[{index}:v]scale={width}:{height}:force_original_aspect_ratio=decrease,\
             pad={width}:{height}:(ow-iw)/2:(oh-ih)/2,setsar=1,format=yuv420p[v{index}];"
        ));
        if audio_all {
            graph.push_str(&format!(
                "[{index}:a]aresample={rate},aformat=sample_rates={rate}:\
                 channel_layouts=stereo[a{index}];"
            ));
        }
    }
    let labels: String = (0..inputs.len())
        .map(|index| {
            if audio_all {
                format!("[v{index}][a{index}]")
            } else {
                format!("[v{index}]")
            }
        })
        .collect();
    graph.push_str(&format!(
        "{labels}concat=n={}:v=1:a={}[v]{}",
        inputs.len(),
        if audio_all { 1 } else { 0 },
        if audio_all { "[a]" } else { "" },
    ));
    cmd = cmd.arg("-filter_complex").arg(graph);
    cmd = cmd.args(["-map", "[v]"]);
    if audio_all {
        cmd = cmd.args(["-map", "[a]"]);
    }
    for arg in video_encode_args(cx, ext)? {
        cmd = cmd.arg(arg);
    }
    if audio_all
        && let Some(args) = audio_encode_args(cx, ext)?
    {
        for arg in args {
            cmd = cmd.arg(arg);
        }
    } else if !audio_all {
        cmd = cmd.arg("-an");
    }
    if let Some(m) = muxer {
        cmd = cmd.args(["-f", m]);
    }
    cmd = cmd.arg(output);
    run_ffmpeg(cmd, cx, Budget::Long, total, 0.0, 1.0)
}

fn split(job: &Job, cx: &Context<'_>) -> Result<Vec<PathBuf>, JobError> {
    let sections = sections_param(job)?;
    let ffmpeg = cx.tools.path(ToolKey::Ffmpeg)?;
    let ext = source_ext(job);
    let stem = stem_of(job);
    let muxer = muxer_for_ext(&ext);
    let total = sections.len();
    let mut artifacts = Vec::new();

    for (index, (start, end)) in sections.iter().enumerate() {
        cx.ctx.checkpoint()?;
        cx.ctx
            .set_stage(format!("saving section {} of {total}", index + 1));
        let output = cx.out(&format!("{stem}-part-{:03}.{}", index + 1, ext));
        let mut cmd = ffmpeg_cmd(&ffmpeg);
        cmd = cmd
            .arg("-ss")
            .arg(secs(*start))
            .arg("-i")
            .arg(&job.source)
            .arg("-t")
            .arg(secs(*end - *start))
            // No `-avoid_negative_ts make_zero`: on this muxer it turns a 2.5 s
            // section into a 5 s one by keeping the original offset.
            .args(["-c", "copy"]);
        if let Some(m) = muxer {
            cmd = cmd.args(["-f", m]);
        }
        cmd = cmd.arg(&output);
        let span = 1.0 / total as f64;
        run_ffmpeg(
            cmd,
            cx,
            Budget::Long,
            *end - *start,
            index as f64 * span,
            span,
        )?;
        ensure_file(&output, "ffmpeg")?;
        artifacts.push(output);
    }
    Ok(artifacts)
}

fn sections_param(job: &Job) -> Result<Vec<(f64, f64)>, JobError> {
    let p = job.intent.params();
    let Some(value) = p.and_then(|p| p.get("sections")) else {
        return Err(invalid("split needs a non-empty sections array"));
    };
    let array = value
        .as_array()
        .ok_or_else(|| invalid("sections must be an array of {start, end} objects"))?;
    if array.is_empty() {
        return Err(invalid("split needs at least one section"));
    }
    array
        .iter()
        .map(|item| range_pair(item, "section"))
        .collect()
}

fn ranges_param(job: &Job) -> Result<Vec<(f64, f64)>, JobError> {
    let p = job.intent.params();
    let Some(value) = p.and_then(|p| p.get("ranges")) else {
        return Err(invalid("bleep needs a non-empty ranges array"));
    };
    let array = value
        .as_array()
        .ok_or_else(|| invalid("ranges must be an array of {start, end} objects"))?;
    if array.is_empty() {
        return Err(invalid("bleep needs at least one range"));
    }
    array.iter().map(|item| range_pair(item, "range")).collect()
}

fn range_pair(item: &Value, what: &str) -> Result<(f64, f64), JobError> {
    let object = item
        .as_object()
        .ok_or_else(|| invalid(format!("each {what} must be an object with start and end")))?;
    let value = |key: &str| {
        object.get(key).and_then(|v| {
            v.as_f64()
                .or_else(|| v.as_str().and_then(|s| s.trim().parse::<f64>().ok()))
        })
    };
    let start =
        value("start").ok_or_else(|| invalid(format!("each {what} needs a numeric start")))?;
    let end = value("end").ok_or_else(|| invalid(format!("each {what} needs a numeric end")))?;
    if !start.is_finite() || !end.is_finite() || start < 0.0 || end <= start {
        return Err(invalid(format!("each {what} needs 0 <= start < end")));
    }
    Ok((start, end))
}

fn times_param(job: &Job) -> Result<Vec<f64>, JobError> {
    let p = job.intent.params();
    let Some(value) = p.and_then(|p| p.get("times")) else {
        return Ok(Vec::new());
    };
    let array = value
        .as_array()
        .ok_or_else(|| invalid("times must be an array of seconds"))?;
    if array.len() > MAX_FRAMES {
        return Err(invalid(format!("at most {MAX_FRAMES} frames per snapshot")));
    }
    let mut times = Vec::with_capacity(array.len());
    for item in array {
        let time = item
            .as_f64()
            .or_else(|| item.as_str().and_then(|s| s.trim().parse::<f64>().ok()))
            .ok_or_else(|| invalid("times must be an array of seconds"))?;
        if !time.is_finite() || time < 0.0 {
            return Err(invalid("frame times must be non-negative seconds"));
        }
        times.push(time);
    }
    Ok(times)
}

/// The loudnorm filter for a target. Every number the dialog offers is in the string:
/// a hardcoded target was the "knob the engine ignores" bug.
fn loudnorm_filter(lufs: f64, range: f64, true_peak: f64) -> String {
    format!("loudnorm=I={lufs}:TP={true_peak}:LRA={range}")
}

/// Target loudness, loudness range and true-peak ceiling, validated.
fn normalize_targets(job: &Job) -> Result<(f64, f64, f64), JobError> {
    let p = job.intent.params();
    let lufs = num_param(p, "lufs").unwrap_or(-16.0);
    let range = num_param(p, "range").unwrap_or(11.0);
    let peak = num_param(p, "true_peak").unwrap_or(-1.5);
    if !lufs.is_finite() || !(-30.0..=-5.0).contains(&lufs) {
        return Err(invalid("target loudness must be between -30 and -5 LUFS"));
    }
    if !range.is_finite() || !(1.0..=50.0).contains(&range) {
        return Err(invalid("loudness range must be between 1 and 50 LU"));
    }
    if !peak.is_finite() || !(-9.0..=0.0).contains(&peak) {
        return Err(invalid("true peak limit must be between -9 and 0 dBTP"));
    }
    Ok((lufs, range, peak))
}

fn normalize(job: &Job, cx: &Context<'_>) -> Result<Vec<PathBuf>, JobError> {
    let ffmpeg = cx.tools.path(ToolKey::Ffmpeg)?;
    let info = probe(cx, &job.source)?;
    if !info.has_audio {
        return Err(invalid("the input has no audio track"));
    }
    let (lufs, loudness_range, true_peak) = normalize_targets(job)?;
    let target = loudnorm_filter(lufs, loudness_range, true_peak);
    let ext = source_ext(job);
    let output = cx.out(&out_name(job, Some("normalized"), &ext));
    let muxer = muxer_for_ext(&ext);
    let audio = audio_encode_args(cx, &ext)?;

    // Two passes pay off on anything long enough that a dynamic pass would drift.
    let two_pass = info.duration >= 20.0;
    let mut base = 0.0;
    let mut measured = None;
    if two_pass {
        cx.ctx.set_stage("measuring loudness");
        let mut first = ffmpeg_cmd(&ffmpeg);
        first = first
            .arg("-i")
            .arg(&job.source)
            .arg("-af")
            .arg(format!("{target}:print_format=json"))
            .args(["-f", "null"])
            .arg(null_sink());
        let result = proc::run(first.budget(Budget::Short).stdout_piped(), cx.ctx)?;
        let result = result.ok()?;
        measured = measured_from_json(&result.stderr_tail);
        if measured.is_some() {
            base = 0.5;
        }
        cx.ctx.set_progress(base);
    }

    cx.ctx.checkpoint()?;
    cx.ctx.set_stage("normalizing");
    let filter = match &measured {
        Some(measured) => format!("{target}:{measured}"),
        None => target.to_string(),
    };
    let mut cmd = ffmpeg_cmd(&ffmpeg);
    cmd = cmd
        .arg("-i")
        .arg(&job.source)
        .arg("-af")
        .arg(&filter)
        .arg("-vn");
    if let Some(args) = &audio {
        for arg in args {
            cmd = cmd.arg(arg);
        }
    }
    if let Some(m) = muxer {
        cmd = cmd.args(["-f", m]);
    }
    cmd = cmd.arg(&output);

    run_ffmpeg(
        cmd,
        cx,
        budget_for(Category::Audio),
        info.duration,
        base,
        1.0 - base,
    )?;
    ensure_file(&output, "ffmpeg")?;
    Ok(vec![output])
}

/// The JSON block a filter prints at the end of its stderr.
fn json_block(tail: &str) -> Option<Value> {
    let start = tail.find('{')?;
    let end = tail.rfind('}')?;
    if end <= start {
        return None;
    }
    serde_json::from_str(&tail[start..=end]).ok()
}

/// Pull the loudnorm measurement block out of the first pass's stderr.
pub(crate) fn measured_from_json(tail: &str) -> Option<String> {
    let value = json_block(tail)?;
    let field = |key: &str| {
        value.get(key).and_then(|v| match v {
            Value::String(s) => Some(s.clone()),
            Value::Number(n) => Some(n.to_string()),
            _ => None,
        })
    };
    Some(format!(
        "measured_I={}:measured_TP={}:measured_LRA={}:measured_thresh={}:offset={}",
        field("input_i")?,
        field("input_tp")?,
        field("input_lra")?,
        field("input_thresh")?,
        field("target_offset").unwrap_or_else(|| "0".to_string()),
    ))
}

fn channels(job: &Job, cx: &Context<'_>) -> Result<Vec<PathBuf>, JobError> {
    let (count, left, right) = channels_params(job)?;
    let ffmpeg = cx.tools.path(ToolKey::Ffmpeg)?;
    let info = probe(cx, &job.source)?;
    if !info.has_audio {
        return Err(invalid("the input has no audio track"));
    }
    let ext = source_ext(job);
    let output = cx.out(&out_name(job, Some("channels"), &ext));
    cx.ctx.set_stage(if count == 1 {
        "converting to mono"
    } else {
        "converting to stereo"
    });

    // One `pan` expression carries both the channel layout and the gains. A mono
    // input has only `c0`, so a stereo output duplicates it; a stereo input mixed to
    // mono sums the two weighted channels. Volumes are percentages of the source.
    let pan = match (count, info.audio_channels) {
        (2, 1) => format!(
            "pan=stereo|c0={:.4}*c0|c1={:.4}*c0",
            left / 100.0,
            right / 100.0
        ),
        (2, _) => format!(
            "pan=stereo|c0={:.4}*c0|c1={:.4}*c1",
            left / 100.0,
            right / 100.0
        ),
        (_, 1) => format!("pan=mono|c0={:.4}*c0", left / 100.0),
        _ => format!(
            "pan=mono|c0={:.4}*c0+{:.4}*c1",
            left / 100.0,
            right / 100.0
        ),
    };

    let mut cmd = ffmpeg_cmd(&ffmpeg);
    cmd = cmd.arg("-i").arg(&job.source).arg("-af").arg(pan).arg("-vn");
    if let Some(args) = audio_encode_args(cx, &ext)? {
        for arg in args {
            cmd = cmd.arg(arg);
        }
    }
    if let Some(m) = muxer_for_ext(&ext) {
        cmd = cmd.args(["-f", m]);
    }
    cmd = cmd.arg(&output);

    run_ffmpeg(
        cmd,
        cx,
        budget_for(Category::Audio),
        info.duration,
        0.0,
        1.0,
    )?;
    ensure_file(&output, "ffmpeg")?;
    Ok(vec![output])
}

/// Output count and both channel gains, validated.
fn channels_params(job: &Job) -> Result<(u32, f64, f64), JobError> {
    let p = job.intent.params();
    let count = num_param(p, "channels").unwrap_or(2.0);
    if count != 1.0 && count != 2.0 {
        return Err(invalid("channels must be 1 (mono) or 2 (stereo)"));
    }
    let gain = |key: &str| {
        num_param(p, key)
            .unwrap_or(100.0)
            .clamp(0.0, 200.0)
    };
    Ok((count as u32, gain("left_volume"), gain("right_volume")))
}

fn channels_param(job: &Job) -> Result<u32, JobError> {
    channels_params(job).map(|(count, _, _)| count)
}

/// The canvas the shape names, and the showwaves mode the style names.
fn visualize_shape(value: Option<&str>) -> Option<(u32, u32)> {
    match value {
        None | Some("16:9") => Some((1280, 720)),
        Some("9:16") => Some((720, 1280)),
        Some("1:1") => Some((1080, 1080)),
        Some(_) => None,
    }
}

fn visualize_style(value: Option<&str>) -> Option<&'static str> {
    match value {
        None | Some("waveform") => Some("cline"),
        Some("bars") => Some("p2p"),
        Some("line") => Some("line"),
        Some(_) => None,
    }
}

fn validate_visualize(job: &Job) -> Result<(), JobError> {
    let p = job.intent.params();
    if let Some(shape) = str_param(p, "shape")
        && visualize_shape(Some(shape)).is_none()
    {
        return Err(invalid("shape must be 16:9, 9:16 or 1:1"));
    }
    if let Some(style) = str_param(p, "style")
        && visualize_style(Some(style)).is_none()
    {
        return Err(invalid("style must be waveform, bars or line"));
    }
    if let Some(size) = str_param(p, "size")
        && parse_size(size).is_none()
    {
        return Err(invalid("size must be WIDTHxHEIGHT, e.g. 1280x720"));
    }
    if p.and_then(|p| p.get("background")).is_some() && str_param(p, "background").is_none() {
        return Err(invalid("background must be a file name or path"));
    }
    if let Some(color) = str_param(p, "color") {
        sanitize_color(color)?;
    }
    Ok(())
}

fn visualize(job: &Job, cx: &Context<'_>) -> Result<Vec<PathBuf>, JobError> {
    let ffmpeg = cx.tools.path(ToolKey::Ffmpeg)?;
    let info = probe(cx, &job.source)?;
    if !info.has_audio {
        return Err(invalid("visualize needs an audio track"));
    }
    let p = job.intent.params();
    let (width, height) = str_param(p, "size")
        .and_then(parse_size)
        .or_else(|| visualize_shape(str_param(p, "shape")))
        .ok_or_else(|| invalid("shape must be 16:9, 9:16 or 1:1"))?;
    let width = even(width).max(16);
    let height = even(height).max(16);
    let mode = visualize_style(str_param(p, "style"))
        .ok_or_else(|| invalid("style must be waveform, bars or line"))?;

    let output = cx.out(&out_name(job, Some("visualized"), "mp4"));
    cx.ctx.set_stage("rendering the waveform");

    let mut cmd = ffmpeg_cmd(&ffmpeg);
    cmd = cmd.arg("-i").arg(&job.source);
    if let Some(background) = str_param(p, "background") {
        let path = resolve_background(cx, background)?;
        cmd = cmd.args(["-loop", "1", "-i"]).arg(&path);
    } else {
        let color = sanitize_color(str_param(p, "color").unwrap_or("#101014"))?;
        cmd = cmd
            .args(["-f", "lavfi", "-i"])
            .arg(format!("color=c={color}:s={width}x{height}:r=25"));
    }

    let wave = format!(
        "[0:a]showwaves=s={width}x{height}:mode={mode}:rate=25,format=yuva420p[w]"
    );
    let backdrop = format!(
        "[1:v]scale={width}:{height}:force_original_aspect_ratio=decrease,\
         pad={width}:{height}:(ow-iw)/2:(oh-ih)/2:color=black[bg]"
    );
    let compose = format!("{wave};{backdrop};[bg][w]overlay=shortest=1,format=yuv420p[v]");
    cmd = cmd.arg("-filter_complex").arg(&compose);

    for arg in video_encode_args(cx, "mp4")? {
        cmd = cmd.arg(arg);
    }
    cmd = cmd
        .args([
            "-map",
            "[v]",
            "-map",
            "0:a",
            "-c:a",
            "aac",
            "-b:a",
            "192k",
            "-shortest",
        ])
        .args(["-f", "mp4"])
        .arg(&output);

    run_ffmpeg(cmd, cx, Budget::Long, info.duration, 0.0, 1.0)?;
    ensure_file(&output, "ffmpeg")?;
    Ok(vec![output])
}

fn bleep(job: &Job, cx: &Context<'_>) -> Result<Vec<PathBuf>, JobError> {
    let ranges = ranges_param(job)?;
    let ffmpeg = cx.tools.path(ToolKey::Ffmpeg)?;
    let info = probe(cx, &job.source)?;
    if !info.has_audio {
        return Err(invalid("the input has no audio track"));
    }
    let ext = source_ext(job);
    let output = cx.out(&out_name(job, Some("bleeped"), &ext));
    cx.ctx.set_stage("bleeping the marked ranges");

    let windows: Vec<String> = ranges
        .iter()
        .map(|(start, end)| format!("between(t,{start:.3},{end:.3})"))
        .collect();
    let condition = windows.join("+");
    let filter = format!(
        "[0:a]volume='if({condition},0,1)':eval=frame[main];\
         sine=frequency=1000:sample_rate=48000,\
         volume='if({condition},1,0)':eval=frame[tone];\
         [main][tone]amix=inputs=2:duration=first:normalize=0[a]"
    );

    let mut cmd = ffmpeg_cmd(&ffmpeg);
    cmd = cmd
        .arg("-i")
        .arg(&job.source)
        .arg("-filter_complex")
        .arg(&filter)
        .args(["-map", "[a]", "-vn"]);
    if let Some(args) = audio_encode_args(cx, &ext)? {
        for arg in args {
            cmd = cmd.arg(arg);
        }
    }
    if let Some(m) = muxer_for_ext(&ext) {
        cmd = cmd.args(["-f", m]);
    }
    cmd = cmd.arg(&output);

    run_ffmpeg(
        cmd,
        cx,
        budget_for(Category::Audio),
        info.duration,
        0.0,
        1.0,
    )?;
    ensure_file(&output, "ffmpeg")?;
    Ok(vec![output])
}

// -------------------------------------------------------------------- crop

enum CropSpec {
    Aspect(f64, f64),
    Box { x: f64, y: f64, w: f64, h: f64 },
}

fn crop_spec(job: &Job) -> Result<CropSpec, JobError> {
    let p = job.intent.params();
    if let Some(aspect) = str_param(p, "aspect") {
        let (w, h) = parse_aspect(aspect).ok_or_else(|| invalid("aspect must look like 16:9"))?;
        return Ok(CropSpec::Aspect(w, h));
    }
    let required = |key: &str| {
        num_param(p, key)
            .ok_or_else(|| invalid(format!("crop needs aspect or x, y, w, h (missing {key})")))
    };
    let x = required("x")?;
    let y = required("y")?;
    let w = required("w")?;
    let h = required("h")?;
    for value in [x, y, w, h] {
        if !value.is_finite() {
            return Err(invalid("crop values must be finite fractions"));
        }
    }
    if x < 0.0 || y < 0.0 || w <= 0.0 || h <= 0.0 || x + w > 1.0001 || y + h > 1.0001 {
        return Err(invalid(
            "crop needs 0 <= x,y and 0 < w,h with x+w <= 1 and y+h <= 1",
        ));
    }
    Ok(CropSpec::Box { x, y, w, h })
}

fn crop(job: &Job, cx: &Context<'_>) -> Result<Vec<PathBuf>, JobError> {
    let spec = crop_spec(job)?;
    let ffmpeg = cx.tools.path(ToolKey::Ffmpeg)?;
    let info = probe(cx, &job.source)?;
    if info.width == 0 || info.height == 0 {
        return Err(invalid("the input has no video track"));
    }
    let ext = source_ext(job);
    let (x, y, w, h) = crop_rect(&spec, info.width, info.height);
    let output = cx.out(&out_name(job, Some("cropped"), &ext));
    cx.ctx.set_stage("cropping");

    let mut cmd = ffmpeg_cmd(&ffmpeg);
    cmd = cmd
        .arg("-i")
        .arg(&job.source)
        .arg("-vf")
        .arg(format!("crop={w}:{h}:{x}:{y}"));
    if ext == "gif" {
        cmd = cmd.arg("-an");
    } else {
        for arg in video_encode_args(cx, &ext)? {
            cmd = cmd.arg(arg);
        }
        if info.has_audio {
            cmd = cmd.args(["-c:a", "copy"]);
        }
    }
    if let Some(m) = muxer_for_ext(&ext) {
        cmd = cmd.args(["-f", m]);
    }
    cmd = cmd.arg(&output);

    run_ffmpeg(cmd, cx, Budget::Long, info.duration, 0.0, 1.0)?;
    ensure_file(&output, "ffmpeg")?;
    Ok(vec![output])
}

fn crop_rect(spec: &CropSpec, width: u32, height: u32) -> (u32, u32, u32, u32) {
    let (w, h) = match *spec {
        CropSpec::Aspect(aw, ah) => {
            let ratio = aw / ah;
            let frame = width as f64 / height as f64;
            if frame > ratio {
                (height as f64 * ratio, height as f64)
            } else {
                (width as f64, width as f64 / ratio)
            }
        }
        CropSpec::Box { w, h, .. } => (width as f64 * w, height as f64 * h),
    };
    let cw = even(w.round().max(2.0) as u32).min(even(width).max(2));
    let ch = even(h.round().max(2.0) as u32).min(even(height).max(2));
    let (x_fraction, y_fraction) = match *spec {
        CropSpec::Aspect(..) => (0.5, 0.5),
        CropSpec::Box { x, y, .. } => (x, y),
    };
    let x = even((width as f64 * x_fraction).round() as u32).min(width - cw);
    let y = even((height as f64 * y_fraction).round() as u32).min(height - ch);
    (x, y, cw, ch)
}

// ------------------------------------------------------------------- speed

fn speed(job: &Job, cx: &Context<'_>) -> Result<Vec<PathBuf>, JobError> {
    let factor = speed_factor(job)?;
    let ffmpeg = cx.tools.path(ToolKey::Ffmpeg)?;
    let info = probe(cx, &job.source)?;
    let ext = source_ext(job);
    let output = cx.out(&out_name(job, Some("speed"), &ext));
    cx.ctx.set_stage(format!("changing speed by {factor}x"));

    let mut cmd = ffmpeg_cmd(&ffmpeg);
    cmd = cmd.arg("-i").arg(&job.source);
    if info.has_video {
        let setpts = format!("setpts=PTS/{factor}");
        if info.has_audio {
            let filter = format!("[0:v]{setpts}[v];[0:a]{}[a]", atempo_chain(factor));
            cmd = cmd
                .arg("-filter_complex")
                .arg(filter)
                .args(["-map", "[v]", "-map", "[a]"]);
        } else {
            cmd = cmd.arg("-vf").arg(setpts).arg("-an");
        }
        if ext != "gif" {
            for arg in video_encode_args(cx, &ext)? {
                cmd = cmd.arg(arg);
            }
        }
        if info.has_audio
            && ext != "gif"
            && let Some(args) = audio_encode_args(cx, &ext)?
        {
            for arg in args {
                cmd = cmd.arg(arg);
            }
        }
    } else if info.has_audio {
        cmd = cmd.arg("-af").arg(atempo_chain(factor)).arg("-vn");
        if let Some(args) = audio_encode_args(cx, &ext)? {
            for arg in args {
                cmd = cmd.arg(arg);
            }
        }
    } else {
        return Err(invalid("the input has neither video nor audio"));
    }
    if let Some(m) = muxer_for_ext(&ext) {
        cmd = cmd.args(["-f", m]);
    }
    cmd = cmd.arg(&output);

    let duration = if factor > 0.0 {
        info.duration / factor
    } else {
        info.duration
    };
    run_ffmpeg(cmd, cx, Budget::Long, duration, 0.0, 1.0)?;
    ensure_file(&output, "ffmpeg")?;
    Ok(vec![output])
}

fn speed_factor(job: &Job) -> Result<f64, JobError> {
    let factor = num_param(job.intent.params(), "factor").unwrap_or(1.0);
    if !factor.is_finite() || factor <= 0.0 || factor > 8.0 {
        return Err(invalid("speed factor must be greater than 0 and at most 8"));
    }
    Ok(factor)
}

/// `atempo` only accepts 0.5..2.0, so anything beyond is a chain of stages whose
/// product is the requested factor.
fn atempo_chain(factor: f64) -> String {
    let mut parts: Vec<String> = Vec::new();
    let mut remaining = factor;
    while remaining > 2.0 {
        parts.push("atempo=2.0".to_string());
        remaining /= 2.0;
    }
    while remaining < 0.5 {
        parts.push("atempo=0.5".to_string());
        remaining /= 0.5;
    }
    if (remaining - 1.0).abs() > 1e-9 {
        parts.push(format!("atempo={remaining:.6}"));
    }
    if parts.is_empty() {
        return "atempo=1.0".to_string();
    }
    parts.join(",")
}

// ------------------------------------------------------------------ ffprobe

/// What a dialog needs to know about a media file before it draws anything: the
/// duration on the timeline, the frame size on the crop surface, whether there is an
/// audio track to preview, and what container it is in.
#[derive(Debug, Clone, Default, serde::Serialize)]
pub struct MediaFacts {
    pub duration: f64,
    pub width: u32,
    pub height: u32,
    pub has_audio: bool,
    pub has_video: bool,
    pub format: String,
}

/// The same probe the engines use, callable without a job.
///
/// An image has no duration and its size comes from its own headers, so it never
/// needs an FFprobe run; everything else is one shared argv (see `probe`) so a
/// dialog and a job can never disagree about a clip's length.
pub fn media_info(
    tools: &crate::Toolset,
    path: &Path,
) -> Result<MediaFacts, JobError> {
    if let Some(facts) = image_facts(path) {
        return Ok(facts);
    }
    let ffprobe = tools.path(ToolKey::Ffprobe)?;
    let ctx = JobCtx::new();
    let output = proc::run(probe_argv(&ffprobe, path), &ctx)?;
    let output = output.ok()?;
    let info = parse_probe_json(&output.stdout)?;
    let format = path
        .extension()
        .and_then(|e| e.to_str())
        .unwrap_or_default()
        .to_ascii_lowercase();
    Ok(MediaFacts {
        duration: info.duration,
        width: info.width,
        height: info.height,
        has_audio: info.has_audio,
        has_video: info.has_video,
        format,
    })
}

/// Loudness of a file as measured, in the units the Normalize dialog shows.
#[derive(Debug, Clone, Copy, Default, serde::Serialize)]
pub struct Loudness {
    pub input_i: f64,
    pub input_tp: f64,
    pub input_lra: f64,
}

/// Measure a file the way the normalizer's first pass does.
///
/// The dialog shows these numbers before the job runs, so they must come from the
/// same filter string: `loudnorm`'s own measurement, parsed from the JSON block.
pub fn measure_loudness(tools: &crate::Toolset, path: &Path) -> Result<Loudness, JobError> {
    let ffmpeg = tools.path(ToolKey::Ffmpeg)?;
    let ctx = JobCtx::new();
    let mut cmd = ffmpeg_cmd(&ffmpeg);
    cmd = cmd
        .arg("-i")
        .arg(path)
        .arg("-af")
        .arg("loudnorm=I=-16:TP=-1.5:LRA=11:print_format=json")
        .args(["-f", "null"])
        .arg(null_sink());
    let output = proc::run(cmd.budget(Budget::Short).stdout_piped(), &ctx)?;
    let output = output.ok()?;
    let block = json_block(&output.stderr_tail)
        .ok_or_else(|| JobError::new(ErrorCode::EngineFailed, "ffmpeg printed no loudness measurement"))?;
    let number = |key: &str| match block.get(key) {
        Some(Value::String(s)) => s.parse::<f64>().ok(),
        Some(Value::Number(n)) => n.as_f64(),
        _ => None,
    };
    Ok(Loudness {
        input_i: number("input_i").unwrap_or(0.0),
        input_tp: number("input_tp").unwrap_or(0.0),
        input_lra: number("input_lra").unwrap_or(0.0),
    })
}

/// Dimensions of a still image, from its own headers.
fn image_facts(path: &Path) -> Option<MediaFacts> {
    let ext = path.extension()?.to_str()?.to_ascii_lowercase();
    let still = matches!(
        ext.as_str(),
        "png" | "jpg" | "jpeg" | "webp" | "avif" | "heic" | "tif" | "tiff" | "bmp" | "gif"
    );
    if !still {
        return None;
    }
    let (width, height) = crate::image_dimensions(path)?;
    Some(MediaFacts {
        duration: 0.0,
        width,
        height,
        has_audio: false,
        has_video: false,
        format: ext,
    })
}

#[derive(Debug, Clone, Copy, Default)]
struct MediaInfo {
    duration: f64,
    size: u64,
    has_audio: bool,
    has_video: bool,
    width: u32,
    height: u32,
    /// Input channel count, so the Channels dialog knows whether to duplicate or mix.
    audio_channels: u32,
    /// Input sample rate, used as the join's common rate when it must re-encode.
    audio_rate: u32,
}

fn probe(cx: &Context<'_>, path: &Path) -> Result<MediaInfo, JobError> {
    let ffprobe = cx.tools.path(ToolKey::Ffprobe)?;
    let output = proc::run(probe_argv(&ffprobe, path), cx.ctx)?;
    let output = output.ok()?;
    parse_probe_json(&output.stdout)
}

/// The one ffprobe invocation shape: duration and size of the container, plus the
/// streams and the frame size.
fn probe_argv(ffprobe: &Path, path: &Path) -> Cmd {
    Cmd::new(ffprobe)
        .args([
            "-v",
            "error",
            "-show_entries",
            "format=duration,size:stream=codec_type,channels,sample_rate,width,height",
            "-of",
            "json",
        ])
        .arg(path)
        .stdout_piped()
        .budget(Budget::Short)
}

fn parse_probe_json(stdout: &str) -> Result<MediaInfo, JobError> {
    let value: Value = serde_json::from_str(stdout).map_err(|e| {
        JobError::new(
            ErrorCode::EngineFailed,
            format!("ffprobe returned unreadable JSON: {e}"),
        )
    })?;

    let number = |v: &Value| match v {
        Value::String(s) => s.parse::<f64>().ok(),
        Value::Number(n) => n.as_f64(),
        _ => None,
    };
    let mut info = MediaInfo {
        duration: number(&value["format"]["duration"]).unwrap_or(0.0),
        size: number(&value["format"]["size"])
            .map(|v| v.max(0.0) as u64)
            .unwrap_or(0),
        ..MediaInfo::default()
    };

    if let Some(streams) = value["streams"].as_array() {
        for stream in streams {
            match stream["codec_type"].as_str() {
                Some("audio") if !info.has_audio => {
                    info.has_audio = true;
                    info.audio_channels = stream["channels"].as_u64().unwrap_or(0) as u32;
                    info.audio_rate = stream["sample_rate"]
                        .as_str()
                        .and_then(|s| s.parse().ok())
                        .or_else(|| stream["sample_rate"].as_u64())
                        .unwrap_or(0) as u32;
                }
                Some("audio") => {}
                Some("video") if !info.has_video => {
                    info.has_video = true;
                    info.width = stream["width"].as_u64().unwrap_or(0) as u32;
                    info.height = stream["height"].as_u64().unwrap_or(0) as u32;
                }
                _ => {}
            }
        }
    }
    Ok(info)
}

/// Re-encode an artifact as MP3 for playback in the dialog.
///
/// WebKit's `<audio>` does not play every container the engine writes (FLAC in a
/// WAV wrapper, Ogg Opus), so a preview is normalised to one codec. `max_bytes` is
/// checked on the encoded file: a cap that is never enforced is a promise.
pub fn transcode_preview(
    tools: &crate::Toolset,
    from: &Path,
    to: &Path,
    max_bytes: u64,
) -> Result<(), JobError> {
    let ffmpeg = tools.path(ToolKey::Ffmpeg)?;
    let ctx = JobCtx::new();
    let stamp = std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .map(|d| d.as_nanos())
        .unwrap_or(0);
    let staged = to.with_extension(format!("preview-{stamp}.mp3"));
    let cmd = ffmpeg_cmd(&ffmpeg)
        .arg("-i")
        .arg(from)
        .arg("-vn")
        .args(["-c:a", "libmp3lame", "-b:a", "96k"])
        .arg(&staged)
        .budget(Budget::Long);
    let result = proc::run(cmd, &ctx)?;
    if let Err(e) = result.ok() {
        let _ = std::fs::remove_file(&staged);
        return Err(e);
    }
    let size = std::fs::metadata(&staged).map(|m| m.len()).unwrap_or(0);
    if size == 0 {
        let _ = std::fs::remove_file(&staged);
        return Err(JobError::new(
            ErrorCode::EngineFailed,
            "the audio preview produced an empty file",
        ));
    }
    if size > max_bytes {
        let _ = std::fs::remove_file(&staged);
        return Err(JobError::new(
            ErrorCode::Unsupported,
            format!(
                "the preview would be {} MB; shorten the selection",
                size.div_ceil(1024 * 1024)
            ),
        ));
    }
    std::fs::rename(&staged, to).map_err(|e| JobError::io("storing the audio preview", &e))?;
    Ok(())
}

// ------------------------------------------------------------------ ffmpeg

fn ffmpeg_cmd(ffmpeg: &Path) -> Cmd {
    Cmd::new(ffmpeg).args([
        "-hide_banner",
        "-nostdin",
        "-y",
        "-nostats",
        "-progress",
        "pipe:1",
    ])
}

fn run_ffmpeg(
    cmd: Cmd,
    cx: &Context<'_>,
    budget: Budget,
    duration: f64,
    base: f64,
    span: f64,
) -> Result<(), JobError> {
    let progress = Progress {
        ctx: cx.ctx,
        duration,
        base,
        span,
    };
    let output = proc::run_with_lines(cmd.budget(budget).stdout_piped(), cx.ctx, |line| {
        progress.feed(line)
    })?;
    output.ok()?;
    Ok(())
}

struct Progress<'a> {
    ctx: &'a JobCtx,
    duration: f64,
    base: f64,
    span: f64,
}

impl Progress<'_> {
    fn feed(&self, line: &str) {
        let Some(seconds) = progress_seconds(line) else {
            return;
        };
        if self.duration <= 0.0 {
            return;
        }
        let fraction = (seconds / self.duration).clamp(0.0, 1.0);
        self.ctx.set_progress(self.base + self.span * fraction);
    }
}

fn progress_seconds(line: &str) -> Option<f64> {
    let (key, value) = line.split_once('=')?;
    let value = value.trim();
    match key.trim() {
        // `out_time_ms` has always held microseconds in ffmpeg's progress protocol.
        "out_time_us" | "out_time_ms" => value.parse::<f64>().ok().map(|us| us / 1_000_000.0),
        "out_time" => parse_hms(value),
        _ => None,
    }
}

fn parse_hms(value: &str) -> Option<f64> {
    let mut parts = value.split(':');
    let hours: f64 = parts.next()?.parse().ok()?;
    let minutes: f64 = parts.next()?.parse().ok()?;
    let seconds: f64 = parts.next()?.parse().ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some(hours * 3600.0 + minutes * 60.0 + seconds)
}

fn budget_for(category: Category) -> Budget {
    match category {
        Category::Video => Budget::Long,
        _ => Budget::Short,
    }
}

fn null_sink() -> &'static str {
    if cfg!(windows) { "NUL" } else { "/dev/null" }
}

// ------------------------------------------------------------------ helpers

fn category_of(job: &Job) -> Result<Category, JobError> {
    let id = job
        .input_format
        .or_else(|| Registry::detect(&job.source).map(|f| f.id))
        .ok_or_else(|| invalid("unknown input format"))?;
    Registry::get(id)
        .map(|format| format.category)
        .ok_or_else(|| invalid(format!("{id} is not a known format")))
}

fn source_ext(job: &Job) -> String {
    job.source
        .extension()
        .and_then(|ext| ext.to_str())
        .map(|ext| ext.to_ascii_lowercase())
        .or_else(|| {
            job.input_format
                .and_then(Registry::get)
                .map(|format| format.canonical_ext().to_string())
        })
        .unwrap_or_else(|| "mkv".to_string())
}

fn stem_of(job: &Job) -> String {
    job.source
        .file_stem()
        .map(|stem| stem.to_string_lossy().into_owned())
        .unwrap_or_else(|| "output".to_string())
}

fn out_name(job: &Job, suffix: Option<&str>, ext: &str) -> String {
    let stem = stem_of(job);
    match suffix {
        Some(suffix) => format!("{stem}-{suffix}.{ext}"),
        None => format!("{stem}.{ext}"),
    }
}

fn muxer_for_ext(ext: &str) -> Option<&'static str> {
    Some(match ext {
        "mp4" | "m4v" => "mp4",
        "m4a" | "m4b" => "ipod",
        "mov" => "mov",
        "mkv" => "matroska",
        "webm" => "webm",
        "avi" => "avi",
        "wmv" | "wma" => "asf",
        "gif" => "gif",
        "mp3" => "mp3",
        "wav" => "wav",
        "flac" => "flac",
        "ogg" | "oga" => "ogg",
        "opus" => "opus",
        "aiff" | "aif" => "aiff",
        "aac" => "adts",
        _ => return None,
    })
}

fn is_cut_friendly(ext: &str) -> bool {
    matches!(
        ext,
        "mp4"
            | "mov"
            | "m4a"
            | "m4b"
            | "mkv"
            | "webm"
            | "avi"
            | "wmv"
            | "wma"
            | "mp3"
            | "flac"
            | "wav"
            | "aiff"
            | "aif"
            | "ogg"
            | "oga"
            | "opus"
    )
}

fn is_lossless(ext: &str) -> bool {
    matches!(ext, "wav" | "flac" | "aiff" | "aif")
}

fn lossy_audio_codec(ext: &str) -> &'static str {
    match ext {
        "mp3" => "libmp3lame",
        "ogg" | "oga" => "libvorbis",
        "opus" | "webm" => "libopus",
        "wma" | "asf" => "wmav2",
        "avi" => "libmp3lame",
        _ => "aac",
    }
}

/// The lowest bitrate each encoder accepts; below it ffmpeg refuses to open the
/// encoder ("encoder setup failed"), which a relative target on a small file hits.
fn min_audio_kbps(codec: &str) -> f64 {
    match codec {
        "libvorbis" | "wmav2" => 32.0,
        "aac" => 16.0,
        "libopus" => 6.0,
        _ => 8.0,
    }
}

fn encoder_feature(encoder: &str) -> Option<&'static str> {
    Some(match encoder {
        "libx264" => "libx264",
        "libvpx-vp9" => "libvpx",
        "libmp3lame" => "lame",
        "libopus" => "libopus",
        "libvorbis" => "libvorbis",
        _ => return None,
    })
}

fn require_encoder(cx: &Context<'_>, feature: &str, encoder: &str) -> Result<(), JobError> {
    if cx.tools.feature(ToolKey::Ffmpeg, feature) {
        Ok(())
    } else {
        Err(invalid(format!(
            "this build of ffmpeg was made without the {encoder} encoder"
        )))
    }
}

/// Video re-encode args for a container that is kept as-is.
fn video_encode_args(cx: &Context<'_>, ext: &str) -> Result<Vec<&'static str>, JobError> {
    match ext {
        "webm" => {
            require_encoder(cx, "libvpx", "libvpx-vp9")?;
            Ok(vec![
                "-c:v",
                "libvpx-vp9",
                "-b:v",
                "0",
                "-crf",
                "32",
                "-row-mt",
                "1",
            ])
        }
        "wmv" => Ok(vec!["-c:v", "wmv2", "-b:v", "2M"]),
        "gif" => Ok(Vec::new()),
        _ => {
            if cx.tools.feature(ToolKey::Ffmpeg, "libx264") {
                Ok(vec![
                    "-c:v", "libx264", "-preset", "veryfast", "-crf", "20", "-pix_fmt", "yuv420p",
                ])
            } else {
                Ok(vec!["-c:v", "mpeg4", "-q:v", "3"])
            }
        }
    }
}

fn audio_encode_args(cx: &Context<'_>, ext: &str) -> Result<Option<Vec<&'static str>>, JobError> {
    let (codec, feature, extra): (&'static str, Option<&'static str>, Vec<&'static str>) = match ext
    {
        "mp3" => ("libmp3lame", Some("lame"), vec!["-q:a", "2"]),
        "ogg" | "oga" => ("libvorbis", Some("libvorbis"), vec!["-q:a", "5"]),
        "opus" | "webm" => ("libopus", Some("libopus"), vec!["-b:a", "128k"]),
        "flac" => ("flac", None, vec!["-compression_level", "5"]),
        "wav" | "aiff" | "aif" => ("pcm_s16le", None, Vec::new()),
        "wma" | "asf" => ("wmav2", None, vec!["-b:a", "192k"]),
        "avi" => ("libmp3lame", Some("lame"), vec!["-q:a", "2"]),
        "mp4" | "mov" | "mkv" | "m4a" | "m4b" | "aac" => ("aac", None, vec!["-b:a", "192k"]),
        "gif" => return Ok(None),
        _ => return Ok(None),
    };
    if let Some(feature) = feature {
        require_encoder(cx, feature, codec)?;
    }
    let mut args = vec!["-c:a", codec];
    args.extend(extra);
    Ok(Some(args))
}

fn secs(value: f64) -> String {
    format!("{value:.3}")
}

fn even(value: u32) -> u32 {
    value & !1
}

fn parse_aspect(value: &str) -> Option<(f64, f64)> {
    let (w, h) = value.split_once([':', 'x', 'X'])?;
    let w: f64 = w.trim().parse().ok()?;
    let h: f64 = h.trim().parse().ok()?;
    if w <= 0.0 || h <= 0.0 || !w.is_finite() || !h.is_finite() {
        return None;
    }
    Some((w, h))
}

fn parse_size(value: &str) -> Option<(u32, u32)> {
    let (w, h) = value.split_once(['x', 'X'])?;
    let w: u32 = w.trim().parse().ok()?;
    let h: u32 = h.trim().parse().ok()?;
    if !(16..=7680).contains(&w) || !(16..=7680).contains(&h) {
        return None;
    }
    Some((w, h))
}

fn sanitize_color(value: &str) -> Result<String, JobError> {
    let ok = !value.is_empty()
        && value.len() <= 32
        && value
            .chars()
            .all(|c| c.is_ascii_alphanumeric() || c == '#' || c == '.');
    if ok {
        Ok(value.to_string())
    } else {
        Err(invalid("color must be a name or #RRGGBB reference"))
    }
}

fn resolve_background(cx: &Context<'_>, value: &str) -> Result<PathBuf, JobError> {
    let root = std::fs::canonicalize(cx.work.root())
        .map_err(|e| JobError::io("reading the work directory", &e))?;
    let raw = Path::new(value);
    let candidates = if raw.is_absolute() {
        vec![raw.to_path_buf()]
    } else {
        vec![
            cx.work.root().join(raw),
            cx.work.src().join(raw),
            cx.work.out().join(raw),
        ]
    };
    for candidate in candidates {
        let Ok(canonical) = std::fs::canonicalize(&candidate) else {
            continue;
        };
        if !canonical.starts_with(&root) {
            return Err(invalid(
                "background must live inside the job's work directory",
            ));
        }
        if canonical.is_file() {
            return Ok(canonical);
        }
    }
    Err(JobError::new(
        ErrorCode::Io,
        format!("background image not found: {value}"),
    ))
}

/// The concat demuxer accepts single-quoted paths; an embedded quote is written
/// as `'\''` so hostile file names survive.
fn concat_escape(path: &Path) -> String {
    path.to_string_lossy()
        .replace('\\', "/")
        .replace('\'', "'\\''")
}

fn num_param(params: Option<&ToolParams>, key: &str) -> Option<f64> {
    match params?.get(key)? {
        Value::Number(number) => number.as_f64(),
        Value::String(text) => text.trim().parse().ok(),
        _ => None,
    }
}

fn bool_param(params: Option<&ToolParams>, key: &str) -> Option<bool> {
    match params?.get(key)? {
        Value::Bool(value) => Some(*value),
        Value::String(text) => match text.trim().to_ascii_lowercase().as_str() {
            "true" | "1" | "yes" | "on" => Some(true),
            "false" | "0" | "no" | "off" => Some(false),
            _ => None,
        },
        Value::Number(number) => Some(number.as_f64().unwrap_or(0.0) != 0.0),
        _ => None,
    }
}

fn str_param<'a>(params: Option<&'a ToolParams>, key: &str) -> Option<&'a str> {
    params?.get(key)?.as_str()
}

fn invalid(message: impl Into<String>) -> JobError {
    JobError::new(ErrorCode::Unsupported, message)
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::Toolset;
    use convertme_core::job::{JobCtx, WorkDir};
    use serde_json::json;
    use std::process::Command;

    struct Env {
        dir: tempfile::TempDir,
        tools: Toolset,
        work: WorkDir,
        ctx: JobCtx,
    }

    impl Env {
        fn new() -> Option<Self> {
            let tools = Toolset::probe_now();
            if !tools.has(ToolKey::Ffmpeg) || !tools.has(ToolKey::Ffprobe) {
                return None;
            }
            let dir = tempfile::tempdir().ok()?;
            let work = WorkDir::create(dir.path().join("work")).ok()?;
            Some(Self {
                dir,
                tools,
                work,
                ctx: JobCtx::new(),
            })
        }

        fn context(&self) -> Context<'_> {
            Context {
                tools: &self.tools,
                work: &self.work,
                ctx: &self.ctx,
                preview: None,
            }
        }

        fn ffmpeg(&self) -> PathBuf {
            self.tools
                .path(ToolKey::Ffmpeg)
                .expect("ffmpeg probed above")
        }

        fn ffprobe(&self) -> PathBuf {
            self.tools
                .path(ToolKey::Ffprobe)
                .expect("ffprobe probed above")
        }

        fn video(&self) -> PathBuf {
            let path = self.dir.path().join("source.mp4");
            if !path.exists() {
                let output = Command::new(self.ffmpeg())
                    .args([
                        "-hide_banner",
                        "-y",
                        "-f",
                        "lavfi",
                        "-i",
                        "testsrc=duration=2:size=320x240:rate=30",
                        "-f",
                        "lavfi",
                        "-i",
                        "sine=frequency=440:duration=2",
                        "-c:v",
                        "mpeg4",
                        "-q:v",
                        "5",
                        "-c:a",
                        "aac",
                        "-b:a",
                        "128k",
                        "-shortest",
                    ])
                    .arg(&path)
                    .output()
                    .expect("running ffmpeg to build the fixture");
                assert!(
                    output.status.success(),
                    "fixture generation failed: {}",
                    String::from_utf8_lossy(&output.stderr)
                );
            }
            path
        }

        fn audio(&self) -> PathBuf {
            let path = self.dir.path().join("voice.m4a");
            if !path.exists() {
                let output = Command::new(self.ffmpeg())
                    .args([
                        "-hide_banner",
                        "-y",
                        "-f",
                        "lavfi",
                        "-i",
                        "sine=frequency=330:duration=1.5",
                        "-c:a",
                        "aac",
                        "-b:a",
                        "128k",
                    ])
                    .arg(&path)
                    .output()
                    .expect("running ffmpeg to build the audio fixture");
                assert!(
                    output.status.success(),
                    "audio fixture generation failed: {}",
                    String::from_utf8_lossy(&output.stderr)
                );
            }
            path
        }

        fn wav(&self) -> PathBuf {
            let path = self.dir.path().join("voice.wav");
            if !path.exists() {
                let output = Command::new(self.ffmpeg())
                    .args([
                        "-hide_banner",
                        "-y",
                        "-f",
                        "lavfi",
                        "-i",
                        "sine=frequency=330:duration=1.0",
                        "-c:a",
                        "pcm_s16le",
                    ])
                    .arg(&path)
                    .output()
                    .expect("running ffmpeg to build the wav fixture");
                assert!(
                    output.status.success(),
                    "wav fixture generation failed: {}",
                    String::from_utf8_lossy(&output.stderr)
                );
            }
            path
        }

        /// A tone with one silent second at each end: what "trim silent ends" is for.
        fn silent_wav(&self) -> PathBuf {
            let path = self.dir.path().join("silent-ends.wav");
            if !path.exists() {
                let output = Command::new(self.ffmpeg())
                    .args([
                        "-hide_banner",
                        "-y",
                        "-f",
                        "lavfi",
                        "-i",
                        "sine=frequency=440:duration=2",
                        "-af",
                        "adelay=1000|1000,apad=pad_dur=1",
                        "-c:a",
                        "pcm_s16le",
                    ])
                    .arg(&path)
                    .output()
                    .expect("building the silent-ends fixture");
                assert!(
                    output.status.success(),
                    "silent fixture failed: {}",
                    String::from_utf8_lossy(&output.stderr)
                );
            }
            path
        }

        /// The same tone on both channels, so a pan gain is measurable per side.
        fn stereo_wav(&self) -> PathBuf {
            let path = self.dir.path().join("stereo.wav");
            if !path.exists() {
                let output = Command::new(self.ffmpeg())
                    .args([
                        "-hide_banner",
                        "-y",
                        "-f",
                        "lavfi",
                        "-i",
                        "sine=frequency=440:duration=1",
                        "-ac",
                        "2",
                        "-c:a",
                        "pcm_s16le",
                    ])
                    .arg(&path)
                    .output()
                    .expect("building the stereo fixture");
                assert!(
                    output.status.success(),
                    "stereo fixture failed: {}",
                    String::from_utf8_lossy(&output.stderr)
                );
            }
            path
        }

        /// A solid-colour clip with a tone, for join order checks.
        fn colored_clip(&self, color: &str, width: u32, height: u32) -> PathBuf {
            let path = self.dir.path().join(format!("{color}-{width}x{height}.mp4"));
            if !path.exists() {
                let output = Command::new(self.ffmpeg())
                    .args(["-hide_banner", "-y", "-f", "lavfi", "-i"])
                    .arg(format!("color=c={color}:s={width}x{height}:r=25:d=1"))
                    .args(["-f", "lavfi", "-i", "sine=frequency=440:duration=1"])
                    .args([
                        "-c:v",
                        "mpeg4",
                        "-q:v",
                        "4",
                        "-c:a",
                        "aac",
                        "-shortest",
                    ])
                    .arg(&path)
                    .output()
                    .expect("building a coloured clip");
                assert!(
                    output.status.success(),
                    "coloured clip failed: {}",
                    String::from_utf8_lossy(&output.stderr)
                );
            }
            path
        }

        /// RMS level in dBFS of one channel (0-based) or the mono mix.
        fn rms_db(&self, path: &Path, channel: Option<u32>, length: f64) -> f64 {
            let pan = match channel {
                Some(channel) => format!("pan=mono|c0=c{channel},astats=metadata=1:reset=0"),
                None => "astats=metadata=1:reset=0".to_string(),
            };
            let output = Command::new(self.ffmpeg())
                .args(["-hide_banner", "-nostats", "-i"])
                .arg(path)
                .args(["-t", &length.to_string(), "-af", &pan, "-f", "null", "-"])
                .output()
                .expect("measuring RMS");
            let stderr = String::from_utf8_lossy(&output.stderr);
            stderr
                .lines()
                .filter_map(|line| line.split("RMS level dB:").nth(1))
                .filter_map(|value| value.trim().parse::<f64>().ok())
                .next_back()
                .expect("an RMS level")
        }

        /// The centre pixel of the first frame, as RGB.
        fn first_frame_rgb(&self, path: &Path, width: u32, height: u32) -> [u8; 3] {
            let output = Command::new(self.ffmpeg())
                .args(["-v", "error", "-i"])
                .arg(path)
                .args(["-frames:v", "1", "-f", "rawvideo", "-pix_fmt", "rgb24", "-"])
                .output()
                .expect("decoding the first frame");
            assert!(output.status.success());
            let offset = (((height / 2) * width + width / 2) * 3) as usize;
            [
                output.stdout[offset],
                output.stdout[offset + 1],
                output.stdout[offset + 2],
            ]
        }

        /// Integrated loudness the way the dialog measures an output.
        fn measured_loudness(&self, path: &Path) -> f64 {
            let output = Command::new(self.ffmpeg())
                .args(["-hide_banner", "-nostats", "-i"])
                .arg(path)
                .args([
                    "-af",
                    "loudnorm=I=-16:TP=-1.5:LRA=11:print_format=json",
                    "-f",
                    "null",
                    "-",
                ])
                .output()
                .expect("measuring loudness");
            let stderr = String::from_utf8_lossy(&output.stderr);
            let start = stderr.rfind('{').expect("a JSON block");
            let end = stderr.rfind('}').expect("a JSON block");
            let value: Value = serde_json::from_str(&stderr[start..=end]).expect("loudnorm JSON");
            value["input_i"]
                .as_str()
                .and_then(|text| text.parse().ok())
                .expect("input_i")
        }

        fn inspect(&self, path: &Path) -> serde_json::Value {
            let output = Command::new(self.ffprobe())
                .args([
                    "-v",
                    "error",
                    "-show_entries",
                    "format=format_name,duration:stream=codec_type,channels,width,height",
                    "-of",
                    "json",
                ])
                .arg(path)
                .output()
                .expect("running ffprobe");
            assert!(
                output.status.success(),
                "ffprobe failed: {}",
                String::from_utf8_lossy(&output.stderr)
            );
            serde_json::from_slice(&output.stdout).expect("ffprobe JSON")
        }
    }

    fn run_job(env: &Env, job: &Job) -> Vec<PathBuf> {
        let cx = env.context();
        run(job, &cx).expect("engine run")
    }

    fn tool_params(value: Value) -> ToolParams {
        value.as_object().cloned().unwrap_or_default()
    }

    fn tool_job(source: &Path, tool: ToolId, params: Value) -> Job {
        Job::new(
            source.to_path_buf(),
            Intent::Tool {
                tool,
                params: tool_params(params),
            },
        )
    }

    fn duration_of(value: &serde_json::Value) -> f64 {
        value["format"]["duration"]
            .as_str()
            .and_then(|s| s.parse().ok())
            .or_else(|| value["format"]["duration"].as_f64())
            .expect("duration")
    }

    #[test]
    fn validate_rejects_bad_parameters() {
        let source = PathBuf::from("clip.mp4");
        let speed = |factor: f64| tool_job(&source, ToolId::Speed, json!({ "factor": factor }));
        assert_eq!(
            validate(&speed(0.0), Category::Video).unwrap_err().code,
            ErrorCode::Unsupported
        );
        assert_eq!(
            validate(&speed(9.0), Category::Video).unwrap_err().code,
            ErrorCode::Unsupported
        );
        assert!(validate(&speed(2.0), Category::Video).is_ok());

        let trim = tool_job(&source, ToolId::Trim, json!({ "start": 2.0, "end": 1.0 }));
        assert_eq!(
            validate(&trim, Category::Video).unwrap_err().code,
            ErrorCode::Unsupported
        );

        let split = tool_job(&source, ToolId::Split, json!({ "sections": [] }));
        assert_eq!(
            validate(&split, Category::Video).unwrap_err().code,
            ErrorCode::Unsupported
        );
    }

    #[test]
    fn mp4_to_mp3_produces_an_mp3() {
        let Some(env) = Env::new() else { return };
        if !env.tools.feature(ToolKey::Ffmpeg, "lame") {
            return;
        }
        let job = Job::new(env.video(), Intent::convert("MP3").expect("MP3 registered"));
        let artifacts = run_job(&env, &job);
        assert_eq!(artifacts.len(), 1);
        assert_eq!(
            artifacts[0].extension().and_then(|e| e.to_str()),
            Some("mp3")
        );
        let info = env.inspect(&artifacts[0]);
        let format = info["format"]["format_name"].as_str().unwrap_or_default();
        assert!(format.contains("mp3"), "unexpected format {format}");
        let audio = info["streams"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["codec_type"] == "audio");
        assert!(audio, "the mp3 has no audio stream");
    }

    #[test]
    fn video_compress_percent_shrinks_a_short_clip() {
        let Some(env) = Env::new() else { return };
        let source = env.video();
        let before = std::fs::metadata(&source).expect("metadata").len();
        let job = tool_job(&source, ToolId::Compress, json!({ "percent": 50 }));
        validate(&job, Category::Video).expect("percent is a valid target");
        let artifacts = run_job(&env, &job);
        let after = std::fs::metadata(&artifacts[0]).expect("metadata").len();
        assert!(
            after < before,
            "50 % of {before} bytes produced {after} bytes"
        );
    }

    #[test]
    fn audio_compress_percent_shrinks() {
        let Some(env) = Env::new() else { return };
        let source = env.audio();
        let before = std::fs::metadata(&source).expect("metadata").len();
        let job = tool_job(&source, ToolId::Compress, json!({ "percent": 50 }));
        validate(&job, Category::Audio).expect("percent is a valid target");
        let artifacts = run_job(&env, &job);
        let after = std::fs::metadata(&artifacts[0]).expect("metadata").len();
        assert!(
            after < before,
            "50 % of {before} bytes produced {after} bytes"
        );
    }

    /// A target larger than the source once made the encoder write a bigger file.
    #[test]
    fn compress_never_grows_the_file() {
        let Some(env) = Env::new() else { return };
        let source = env.video();
        let before = std::fs::metadata(&source).expect("metadata").len();
        let job = tool_job(
            &source,
            ToolId::Compress,
            json!({ "target_bytes": before * 2 }),
        );
        let artifacts = run_job(&env, &job);
        let after = std::fs::metadata(&artifacts[0]).expect("metadata").len();
        assert!(after <= before, "{before} bytes grew to {after}");
    }

    /// WAV keeps every sample, so a size target cannot apply. Writing the same file
    /// back under a "compressed" name claimed a compression that never happened.
    #[test]
    fn lossless_audio_is_refused() {
        let Some(env) = Env::new() else { return };
        let source = env.wav();
        let job = tool_job(&source, ToolId::Compress, json!({ "percent": 50 }));
        let err = validate(&job, Category::Audio).expect_err("must refuse");
        assert_eq!(err.code, ErrorCode::Unsupported);
        assert!(err.message.contains("lossless"), "{}", err.message);
        // The engine refuses too, for callers that skip plan validation.
        let cx = env.context();
        let err = run(&job, &cx).expect_err("must refuse");
        assert_eq!(err.code, ErrorCode::Unsupported);
    }

    #[test]
    fn trim_shortens_the_clip() {
        let Some(env) = Env::new() else { return };
        let job = tool_job(
            &env.video(),
            ToolId::Trim,
            json!({ "start": 0.0, "end": 1.0 }),
        );
        let artifacts = run_job(&env, &job);
        assert_eq!(artifacts.len(), 1);
        let duration = duration_of(&env.inspect(&artifacts[0]));
        assert!(duration < 1.6, "trimmed clip is still {duration:.2}s long");
        assert!(duration > 0.2, "trimmed clip is {duration:.2}s long");
    }

    #[test]
    fn mute_removes_the_audio_stream() {
        let Some(env) = Env::new() else { return };
        let job = tool_job(&env.video(), ToolId::Mute, json!({}));
        let artifacts = run_job(&env, &job);
        assert_eq!(artifacts.len(), 1);
        let info = env.inspect(&artifacts[0]);
        let audio = info["streams"]
            .as_array()
            .unwrap()
            .iter()
            .any(|s| s["codec_type"] == "audio");
        assert!(!audio, "the muted clip still has an audio stream");
    }

    #[test]
    fn snapshot_writes_png_frames() {
        let Some(env) = Env::new() else { return };
        let job = tool_job(
            &env.video(),
            ToolId::Snapshot,
            json!({ "times": [0.5, 1.5] }),
        );
        let artifacts = run_job(&env, &job);
        assert_eq!(artifacts.len(), 2);
        for artifact in &artifacts {
            let bytes = std::fs::read(artifact).expect("frame bytes");
            assert!(
                bytes.starts_with(b"\x89PNG"),
                "{} is not a PNG",
                artifact.display()
            );
        }
    }

    #[test]
    fn speed_two_halves_the_duration() {
        let Some(env) = Env::new() else { return };
        let job = tool_job(&env.video(), ToolId::Speed, json!({ "factor": 2.0 }));
        let artifacts = run_job(&env, &job);
        assert_eq!(artifacts.len(), 1);
        let duration = duration_of(&env.inspect(&artifacts[0]));
        assert!(
            (0.85..=1.15).contains(&duration),
            "2x speed produced {duration:.2}s instead of about 1s"
        );
    }

    #[test]
    fn channels_one_is_mono() {
        let Some(env) = Env::new() else { return };
        let job = tool_job(&env.audio(), ToolId::Channels, json!({ "channels": 1 }));
        let artifacts = run_job(&env, &job);
        assert_eq!(artifacts.len(), 1);
        let info = env.inspect(&artifacts[0]);
        let channel_count = info["streams"]
            .as_array()
            .unwrap()
            .iter()
            .find(|s| s["codec_type"] == "audio")
            .and_then(|s| s["channels"].as_u64())
            .expect("an audio stream");
        assert_eq!(channel_count, 1);
    }

    #[test]
    fn loudnorm_filter_carries_every_knob() {
        assert_eq!(
            loudnorm_filter(-16.0, 11.0, -1.5),
            "loudnorm=I=-16:TP=-1.5:LRA=11"
        );
        assert_eq!(
            loudnorm_filter(-20.0, 7.0, -3.0),
            "loudnorm=I=-20:TP=-3:LRA=7"
        );
        let source = PathBuf::from("voice.mp3");
        for bad in [
            json!({ "lufs": 0.0 }),
            json!({ "range": 80.0 }),
            json!({ "true_peak": -12.0 }),
        ] {
            let job = tool_job(&source, ToolId::Normalize, bad.clone());
            let err = validate(&job, Category::Audio).expect_err("must refuse");
            assert_eq!(err.code, ErrorCode::Unsupported, "{bad}");
        }
    }

    #[test]
    fn normalize_reaches_the_requested_loudness() {
        let Some(env) = Env::new() else { return };
        let job = tool_job(
            &env.wav(),
            ToolId::Normalize,
            json!({ "lufs": -20.0, "range": 7.0, "true_peak": -3.0 }),
        );
        let artifacts = run_job(&env, &job);
        let measured = env.measured_loudness(&artifacts[0]);
        assert!(
            (measured + 20.0).abs() <= 3.0,
            "normalize to -20 LUFS measured {measured:.1}"
        );
    }

    #[test]
    fn trim_silence_removes_both_ends() {
        let Some(env) = Env::new() else { return };
        let source = env.silent_wav();
        let before = duration_of(&env.inspect(&source));
        let job = tool_job(
            &source,
            ToolId::Trim,
            json!({ "start": 0.0, "end": before, "trim_silence": true }),
        );
        let artifacts = run_job(&env, &job);
        let after = duration_of(&env.inspect(&artifacts[0]));
        assert!(
            (after - 2.0).abs() <= 0.35,
            "silence trim left {after:.2}s of {before:.2}s"
        );
        let leading = env.rms_db(&artifacts[0], None, 0.1);
        assert!(leading > -40.0, "the result starts with {leading} dBFS");
    }

    #[test]
    fn channels_gain_halves_one_side() {
        let Some(env) = Env::new() else { return };
        let job = tool_job(
            &env.stereo_wav(),
            ToolId::Channels,
            json!({ "channels": 2, "left_volume": 50, "right_volume": 100 }),
        );
        let artifacts = run_job(&env, &job);
        let left = env.rms_db(&artifacts[0], Some(0), 1.0);
        let right = env.rms_db(&artifacts[0], Some(1), 1.0);
        let difference = right - left;
        assert!(
            (4.5..=7.5).contains(&difference),
            "50 % left measured {difference:.2} dB below right"
        );
    }

    #[test]
    fn visualize_shapes_set_the_canvas() {
        let Some(env) = Env::new() else { return };
        for (shape, width, height) in [("16:9", 1280u64, 720u64), ("9:16", 720, 1280), ("1:1", 1080, 1080)] {
            let job = tool_job(
                &env.wav(),
                ToolId::Visualize,
                json!({ "shape": shape, "style": "bars" }),
            );
            let artifacts = run_job(&env, &job);
            let info = env.inspect(&artifacts[0]);
            let video = info["streams"]
                .as_array()
                .unwrap()
                .iter()
                .find(|stream| stream["codec_type"] == "video")
                .expect("a video stream");
            assert_eq!(video["width"].as_u64(), Some(width), "{shape} width");
            assert_eq!(video["height"].as_u64(), Some(height), "{shape} height");
        }
        let job = tool_job(
            &env.wav(),
            ToolId::Visualize,
            json!({ "style": "noise" }),
        );
        assert!(validate(&job, Category::Audio).is_err());
    }

    #[test]
    fn join_follows_the_order_and_the_canvas() {
        let Some(env) = Env::new() else { return };
        let red = env.colored_clip("red", 320, 240);
        let blue = env.colored_clip("blue", 240, 320);
        let job = Job::with_inputs(
            vec![red.clone(), blue.clone()],
            Intent::Tool {
                tool: ToolId::Join,
                params: tool_params(json!({})),
            },
        );
        let artifacts = run_job(&env, &job);
        let forward = env.first_frame_rgb(&artifacts[0], 320, 240);
        assert!(forward[0] > forward[2] + 60, "first frame is {forward:?}, not red");

        let reversed = Job::with_inputs(
            vec![red, blue],
            Intent::Tool {
                tool: ToolId::Join,
                params: tool_params(json!({ "order": [1, 0] })),
            },
        );
        let artifacts = run_job(&env, &reversed);
        let info = env.inspect(&artifacts[0]);
        let video = info["streams"]
            .as_array()
            .unwrap()
            .iter()
            .find(|stream| stream["codec_type"] == "video")
            .expect("a video stream");
        assert_eq!(video["width"].as_u64(), Some(240));
        assert_eq!(video["height"].as_u64(), Some(320));
        let first = env.first_frame_rgb(&artifacts[0], 240, 320);
        assert!(first[2] > first[0] + 60, "reversed first frame is {first:?}, not blue");
    }

    #[test]
    fn join_rejects_a_non_permutation() {
        let source = PathBuf::from("a.mp4");
        let job = Job::with_inputs(
            vec![source.clone(), PathBuf::from("b.mp4")],
            Intent::Tool {
                tool: ToolId::Join,
                params: tool_params(json!({ "order": [0, 0] })),
            },
        );
        let err = validate(&job, Category::Video).expect_err("must refuse");
        assert_eq!(err.code, ErrorCode::Unsupported);
        let short = Job::with_inputs(
            vec![source],
            Intent::Tool {
                tool: ToolId::Join,
                params: tool_params(json!({})),
            },
        );
        assert!(validate(&short, Category::Video).is_err());
    }
}
