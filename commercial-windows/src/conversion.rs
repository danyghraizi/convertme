use image::{DynamicImage, GenericImageView, ImageFormat, ImageReader, imageops::FilterType};
use serde::Serialize;
use std::fs::{self, File, OpenOptions};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};

#[derive(Debug, Clone)]
pub struct ConvertOptions {
    pub format: String,
    pub max_width: Option<u32>,
    pub max_height: Option<u32>,
    pub rotation: u16,
    pub jpeg_quality: u8,
}

#[derive(Debug, Clone, Serialize)]
pub struct FileResult {
    pub source: String,
    pub output: Option<String>,
    pub error: Option<String>,
}

#[derive(Debug, Clone, Serialize)]
pub struct BatchResult {
    pub files: Vec<FileResult>,
    pub succeeded: usize,
    pub failed: usize,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum OutputFormat {
    Png,
    Jpeg,
    Webp,
    Bmp,
    Gif,
    Tiff,
}

impl OutputFormat {
    fn parse(value: &str) -> Result<Self, String> {
        match value.to_ascii_lowercase().as_str() {
            "png" => Ok(Self::Png),
            "jpg" | "jpeg" => Ok(Self::Jpeg),
            "webp" => Ok(Self::Webp),
            "bmp" => Ok(Self::Bmp),
            "gif" => Ok(Self::Gif),
            "tif" | "tiff" => Ok(Self::Tiff),
            _ => Err(format!("Unsupported output format: {value}")),
        }
    }

    fn extension(self) -> &'static str {
        match self {
            Self::Png => "png",
            Self::Jpeg => "jpg",
            Self::Webp => "webp",
            Self::Bmp => "bmp",
            Self::Gif => "gif",
            Self::Tiff => "tiff",
        }
    }

    fn image_format(self) -> ImageFormat {
        match self {
            Self::Png => ImageFormat::Png,
            Self::Jpeg => ImageFormat::Jpeg,
            Self::Webp => ImageFormat::WebP,
            Self::Bmp => ImageFormat::Bmp,
            Self::Gif => ImageFormat::Gif,
            Self::Tiff => ImageFormat::Tiff,
        }
    }
}

pub fn convert_batch(paths: &[PathBuf], options: &ConvertOptions) -> BatchResult {
    let format = OutputFormat::parse(&options.format);
    let files = paths
        .iter()
        .map(|source| {
            let result = format
                .as_ref()
                .map_err(Clone::clone)
                .and_then(|format| convert_one(source, *format, options));
            match result {
                Ok(output) => FileResult {
                    source: source.display().to_string(),
                    output: Some(output.display().to_string()),
                    error: None,
                },
                Err(error) => FileResult {
                    source: source.display().to_string(),
                    output: None,
                    error: Some(error),
                },
            }
        })
        .collect::<Vec<_>>();
    let succeeded = files.iter().filter(|file| file.output.is_some()).count();
    let failed = files.len() - succeeded;
    BatchResult {
        files,
        succeeded,
        failed,
    }
}

fn convert_one(
    source: &Path,
    format: OutputFormat,
    options: &ConvertOptions,
) -> Result<PathBuf, String> {
    if options.rotation % 90 != 0 || options.rotation > 270 {
        return Err("Rotation must be 0, 90, 180, or 270 degrees".into());
    }
    let mut reader = ImageReader::open(source)
        .map_err(|error| format!("Cannot read {}: {error}", source.display()))?
        .with_guessed_format()
        .map_err(|error| format!("Cannot identify {}: {error}", source.display()))?;
    reader.limits(image::Limits::default());
    let decoded = reader
        .decode()
        .map_err(|error| format!("Cannot decode {}: {error}", source.display()))?;
    let image = apply_transform(decoded, options);
    let (output, file) = create_output_path(source, format.extension())?;
    let write_result = write_image(file, image, format, options.jpeg_quality);
    if let Err(error) = write_result {
        let _ = fs::remove_file(&output);
        return Err(format!("Cannot write {}: {error}", output.display()));
    }
    Ok(output)
}

fn apply_transform(mut image: DynamicImage, options: &ConvertOptions) -> DynamicImage {
    image = match options.rotation {
        90 => image.rotate90(),
        180 => image.rotate180(),
        270 => image.rotate270(),
        _ => image,
    };

    let (width, height) = image.dimensions();
    let scale_x = options
        .max_width
        .filter(|limit| *limit > 0)
        .map(|limit| limit as f64 / width as f64)
        .unwrap_or(f64::INFINITY);
    let scale_y = options
        .max_height
        .filter(|limit| *limit > 0)
        .map(|limit| limit as f64 / height as f64)
        .unwrap_or(f64::INFINITY);
    let scale = scale_x.min(scale_y).min(1.0);
    if scale < 1.0 {
        let target_width = ((width as f64 * scale).round() as u32).max(1);
        let target_height = ((height as f64 * scale).round() as u32).max(1);
        image = image.resize_exact(target_width, target_height, FilterType::Lanczos3);
    }
    image
}

fn write_image(
    file: File,
    image: DynamicImage,
    format: OutputFormat,
    jpeg_quality: u8,
) -> Result<(), image::ImageError> {
    if format == OutputFormat::Jpeg {
        let rgb = flatten_alpha_on_white(image);
        image::codecs::jpeg::JpegEncoder::new_with_quality(
            BufWriter::new(file),
            jpeg_quality.clamp(1, 100),
        )
        .encode_image(&rgb)
    } else {
        let mut writer = BufWriter::new(file);
        image.write_to(&mut writer, format.image_format())?;
        writer.flush().map_err(image::ImageError::IoError)
    }
}

fn flatten_alpha_on_white(image: DynamicImage) -> DynamicImage {
    let mut rgba = image.to_rgba8();
    for pixel in rgba.pixels_mut() {
        let alpha = u16::from(pixel[3]);
        for channel in 0..3 {
            let color = u16::from(pixel[channel]);
            pixel[channel] = ((color * alpha + 255 * (255 - alpha) + 127) / 255) as u8;
        }
        pixel[3] = 255;
    }
    DynamicImage::ImageRgb8(image::DynamicImage::ImageRgba8(rgba).to_rgb8())
}

fn create_output_path(source: &Path, extension: &str) -> Result<(PathBuf, File), String> {
    let parent = source.parent().unwrap_or_else(|| Path::new("."));
    let stem = source
        .file_stem()
        .and_then(|stem| stem.to_str())
        .filter(|stem| !stem.is_empty())
        .unwrap_or("image");
    for index in 1..=10_000_u32 {
        let suffix = if index == 1 {
            "-converted".to_string()
        } else {
            format!("-converted-{index}")
        };
        let output = parent.join(format!("{stem}{suffix}.{extension}"));
        match OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&output)
        {
            Ok(file) => return Ok((output, file)),
            Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(format!("Cannot create output beside source: {error}")),
        }
    }
    Err("Could not choose a unique output filename".into())
}

#[cfg(test)]
mod tests {
    use super::*;
    use image::{ImageBuffer, Rgba};

    #[test]
    fn batch_converts_pixels_resizes_and_never_overwrites_the_source() {
        let dir = tempfile::tempdir().unwrap();
        let source = dir.path().join("sample.png");
        let image = ImageBuffer::from_fn(40, 20, |x, y| {
            if x < 20 && y < 10 {
                Rgba([240_u8, 30, 20, 255])
            } else {
                Rgba([10_u8, 70, 240, 128])
            }
        });
        image.save(&source).unwrap();
        let original = fs::read(&source).unwrap();
        let options = ConvertOptions {
            format: "jpeg".into(),
            max_width: Some(10),
            max_height: None,
            rotation: 90,
            jpeg_quality: 95,
        };

        let result = convert_batch(std::slice::from_ref(&source), &options);
        assert_eq!((result.succeeded, result.failed), (1, 0));
        assert_eq!(fs::read(&source).unwrap(), original);
        let output = PathBuf::from(result.files[0].output.as_ref().unwrap());
        assert_eq!(output.file_name().unwrap(), "sample-converted.jpg");
        let decoded = ImageReader::open(&output).unwrap().decode().unwrap();
        assert_eq!(decoded.dimensions(), (10, 20));
        let pixel = decoded.get_pixel(4, 9).0;
        assert_eq!(pixel[3], 255);
        assert!(
            pixel[0] > 100 && pixel[1] > 130 && pixel[2] > 210,
            "{pixel:?}"
        );

        let second = convert_batch(std::slice::from_ref(&source), &options);
        assert_eq!(
            Path::new(second.files[0].output.as_ref().unwrap())
                .file_name()
                .unwrap(),
            "sample-converted-2.jpg"
        );
    }

    #[test]
    fn malformed_inputs_fail_per_file_and_do_not_leave_outputs() {
        let dir = tempfile::tempdir().unwrap();
        let malformed = dir.path().join("broken.png");
        fs::write(&malformed, b"not an image").unwrap();
        let options = ConvertOptions {
            format: "png".into(),
            max_width: None,
            max_height: None,
            rotation: 0,
            jpeg_quality: 90,
        };

        let result = convert_batch(std::slice::from_ref(&malformed), &options);
        assert_eq!((result.succeeded, result.failed), (0, 1));
        assert!(result.files[0].error.as_deref().unwrap().contains("decode"));
        assert!(!dir.path().join("broken-converted.png").exists());
    }

    #[test]
    fn enabled_raster_formats_round_trip_through_png() {
        let dir = tempfile::tempdir().unwrap();
        let source_pixels = ImageBuffer::from_fn(7, 5, |x, y| {
            Rgba([((x * 31) as u8), ((y * 43) as u8), 90, 255])
        });
        let formats = [
            ("png", ImageFormat::Png),
            ("jpeg", ImageFormat::Jpeg),
            ("webp", ImageFormat::WebP),
            ("bmp", ImageFormat::Bmp),
            ("gif", ImageFormat::Gif),
            ("tiff", ImageFormat::Tiff),
        ];
        let options = ConvertOptions {
            format: "png".into(),
            max_width: None,
            max_height: None,
            rotation: 0,
            jpeg_quality: 90,
        };

        for (name, format) in formats {
            let input = dir.path().join(format!("input-{name}"));
            DynamicImage::ImageRgba8(source_pixels.clone())
                .save_with_format(&input, format)
                .unwrap();
            let result = convert_batch(std::slice::from_ref(&input), &options);
            assert_eq!((result.succeeded, result.failed), (1, 0), "{name}");
            let output = PathBuf::from(result.files[0].output.as_ref().unwrap());
            let decoded = ImageReader::open(output).unwrap().decode().unwrap();
            assert_eq!(decoded.dimensions(), (7, 5), "{name}");
        }

        let png_input = dir.path().join("output-source.png");
        DynamicImage::ImageRgba8(source_pixels)
            .save(&png_input)
            .unwrap();
        for format in ["png", "jpeg", "webp", "bmp", "gif", "tiff"] {
            let mut output_options = options.clone();
            output_options.format = format.into();
            let result = convert_batch(std::slice::from_ref(&png_input), &output_options);
            assert_eq!((result.succeeded, result.failed), (1, 0), "to {format}");
            let output = PathBuf::from(result.files[0].output.as_ref().unwrap());
            let decoded = ImageReader::open(output).unwrap().decode().unwrap();
            assert_eq!(decoded.dimensions(), (7, 5), "to {format}");
        }
    }

    #[test]
    fn scaling_only_downscales_and_preserves_aspect_ratio() {
        let image =
            DynamicImage::ImageRgba8(ImageBuffer::from_pixel(120, 60, Rgba([1, 2, 3, 255])));
        let options = ConvertOptions {
            format: "png".into(),
            max_width: Some(30),
            max_height: Some(100),
            rotation: 0,
            jpeg_quality: 90,
        };
        assert_eq!(apply_transform(image, &options).dimensions(), (30, 15));
    }
}
