//! The format registry: the single data source for every wheel, every planner
//! decision and the derived conversion-option count.
//!
//! Nothing here is hardcoded elsewhere. Adding a format to this table adds it to
//! the wheels, to the planner, to the debrand-exempt fixture list and to the count.
//!
//! ## How the wheel contents were derived
//!
//! The wheels are the registry's writable targets for the input's category, with
//! the *source* format removed (a wheel never offers "convert PNG to PNG"). Every
//! wheel in the reference screenshots is exactly `targets(category) - {source}`:
//! the image wheel shows seven entries for eight image targets with JPG as the
//! source, the audio wheel omits MP3 for an MP3 source, and so on. That rule is
//! implemented once, in [`Registry::wheel_targets`].

use crate::capability::ToolKey;
use serde::{Deserialize, Serialize};
use std::fmt;
use std::path::Path;

/// The ten product categories, in build order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
#[derive(Default)]
pub enum Category {
    #[default]
    Images,
    Video,
    Audio,
    Pdf,
    Documents,
    Slides,
    Spreadsheets,
    Epub,
    Subtitles,
    Archives,
}

impl Category {
    /// Build order — the order of the bar in the UI and of the registry table.
    pub const ALL: [Category; 10] = [
        Category::Images,
        Category::Video,
        Category::Audio,
        Category::Pdf,
        Category::Documents,
        Category::Slides,
        Category::Spreadsheets,
        Category::Epub,
        Category::Subtitles,
        Category::Archives,
    ];

    pub fn key(self) -> &'static str {
        match self {
            Category::Images => "images",
            Category::Video => "video",
            Category::Audio => "audio",
            Category::Pdf => "pdf",
            Category::Documents => "documents",
            Category::Slides => "slides",
            Category::Spreadsheets => "spreadsheets",
            Category::Epub => "epub",
            Category::Subtitles => "subtitles",
            Category::Archives => "archives",
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            Category::Images => "Images",
            Category::Video => "Video",
            Category::Audio => "Audio",
            Category::Pdf => "PDF",
            Category::Documents => "Documents",
            Category::Slides => "Slides",
            Category::Spreadsheets => "Spreadsheets",
            Category::Epub => "EPUB",
            Category::Subtitles => "Subtitles",
            Category::Archives => "Archives",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        Category::ALL.into_iter().find(|c| c.key() == s)
    }
}

impl fmt::Display for Category {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.key())
    }
}

/// A format identifier. Always the canonical spelling from [`FORMATS`].
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub struct FormatId(pub &'static str);

impl FormatId {
    pub fn as_str(self) -> &'static str {
        self.0
    }
}

impl fmt::Display for FormatId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.0)
    }
}

impl<'de> Deserialize<'de> for FormatId {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        Registry::by_id(&s)
            .map(|f| f.id)
            .ok_or_else(|| serde::de::Error::custom(format!("unknown format {s:?}")))
    }
}

impl Serialize for FormatId {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.0)
    }
}

/// The process that performs a conversion.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum EngineRef {
    ImageMagick,
    Ffmpeg,
    Ghostscript,
    Qpdf,
    PopplerRender,
    PopplerText,
    Pandoc,
    SevenZip,
    Tesseract,
    Sidecar,
    LibreOffice,
    PrintPdf,
    Calamine,
    RustXlsx,
    NativeText,
    SubtitleNative,
}

impl EngineRef {
    /// Tools that must be present for this engine to run. Empty means "in-process".
    pub fn required(self) -> &'static [ToolKey] {
        use ToolKey::*;
        match self {
            EngineRef::ImageMagick => &[Magick],
            EngineRef::Ffmpeg => &[Ffmpeg, Ffprobe],
            EngineRef::Ghostscript => &[Gs],
            EngineRef::Qpdf => &[Qpdf],
            EngineRef::PopplerRender => &[Pdftoppm],
            EngineRef::PopplerText => &[Pdftotext],
            EngineRef::Pandoc => &[Pandoc],
            EngineRef::SevenZip => &[SevenZip],
            EngineRef::Tesseract => &[Tesseract],
            EngineRef::Sidecar => &[Sidecar],
            EngineRef::LibreOffice => &[Soffice],
            EngineRef::PrintPdf | EngineRef::Calamine | EngineRef::RustXlsx => &[],
            EngineRef::NativeText | EngineRef::SubtitleNative => &[],
        }
    }

    pub fn label(self) -> &'static str {
        match self {
            EngineRef::ImageMagick => "ImageMagick",
            EngineRef::Ffmpeg => "FFmpeg",
            EngineRef::Ghostscript => "Ghostscript",
            EngineRef::Qpdf => "qpdf",
            EngineRef::PopplerRender => "Poppler (pdftoppm)",
            EngineRef::PopplerText => "Poppler (pdftotext)",
            EngineRef::Pandoc => "Pandoc",
            EngineRef::SevenZip => "7-Zip",
            EngineRef::Tesseract => "Tesseract OCR",
            EngineRef::Sidecar => "PDF sidecar",
            EngineRef::LibreOffice => "LibreOffice",
            EngineRef::PrintPdf => "built-in PDF writer",
            EngineRef::Calamine => "built-in spreadsheet reader",
            EngineRef::RustXlsx => "built-in XLSX writer",
            EngineRef::NativeText => "built-in text writer",
            EngineRef::SubtitleNative => "built-in subtitle engine",
        }
    }
}

/// One row of the registry.
pub struct Format {
    pub id: FormatId,
    /// Human label used on a wheel segment.
    pub label: &'static str,
    /// The format's home category (the tab that owns its tool set).
    pub category: Category,
    pub mime: &'static str,
    /// Extensions, lowercase, without the dot. The first one is the canonical
    /// output extension.
    pub exts: &'static [&'static str],
    pub read: bool,
    pub write: bool,
    /// Convert wheels that list this format as a target.
    pub wheels: &'static [Category],
    /// Engine used when this format is the *input* of a conversion.
    pub read_engine: EngineRef,
    /// Engine used when this format is the *target*, unless a pair override says
    /// otherwise.
    pub write_engine: EngineRef,
}

impl Format {
    pub fn canonical_ext(&self) -> &'static str {
        self.exts[0]
    }

    pub fn is_listed_in(&self, category: Category) -> bool {
        self.wheels.contains(&category)
    }
}

macro_rules! fmt {
    (
        $id:literal, $label:literal, $cat:ident, $mime:literal, [$($ext:literal),+],
        read: $read:ident, write: $write:ident, wheels: [$($w:ident),*],
        read_engine: $re:ident, write_engine: $we:ident
    ) => {
        Format {
            id: FormatId($id),
            label: $label,
            category: Category::$cat,
            mime: $mime,
            exts: &[$($ext),+],
            read: fmt!(@bool $read),
            write: fmt!(@bool $write),
            wheels: &[$(Category::$w),*],
            read_engine: EngineRef::$re,
            write_engine: EngineRef::$we,
        }
    };
    (@bool yes) => { true };
    (@bool no) => { false };
}

/// Every format ConvertMe knows about. Order is build order, then wheel order.
pub static FORMATS: &[Format] = &[
    // ---------------------------------------------------------------- Images
    fmt!("JPG", "JPG", Images, "image/jpeg", ["jpg", "jpeg"], read: yes, write: yes,
         wheels: [Images, Pdf, Documents, Slides, Spreadsheets],
         read_engine: ImageMagick, write_engine: ImageMagick),
    fmt!("PNG", "PNG", Images, "image/png", ["png"], read: yes, write: yes,
         wheels: [Images, Pdf, Documents, Slides, Spreadsheets],
         read_engine: ImageMagick, write_engine: ImageMagick),
    fmt!("WEBP", "WEBP", Images, "image/webp", ["webp"], read: yes, write: yes,
         wheels: [Images], read_engine: ImageMagick, write_engine: ImageMagick),
    fmt!("HEIC", "HEIC", Images, "image/heic", ["heic", "heif"], read: yes, write: yes,
         wheels: [Images], read_engine: ImageMagick, write_engine: ImageMagick),
    fmt!("TIFF", "TIFF", Images, "image/tiff", ["tif", "tiff"], read: yes, write: yes,
         wheels: [Images], read_engine: ImageMagick, write_engine: ImageMagick),
    fmt!("AVIF", "AVIF", Images, "image/avif", ["avif"], read: yes, write: yes,
         wheels: [Images], read_engine: ImageMagick, write_engine: ImageMagick),
    fmt!("BMP", "BMP", Images, "image/bmp", ["bmp"], read: yes, write: yes,
         wheels: [Images], read_engine: ImageMagick, write_engine: ImageMagick),
    // Input only: rasterising a drawing is fine, authoring one is not.
    fmt!("SVG", "SVG", Images, "image/svg+xml", ["svg"], read: yes, write: no,
         wheels: [], read_engine: ImageMagick, write_engine: ImageMagick),
    // ----------------------------------------------------------------- Video
    fmt!("MP4", "MP4", Video, "video/mp4", ["mp4"], read: yes, write: yes,
         wheels: [Video], read_engine: Ffmpeg, write_engine: Ffmpeg),
    fmt!("MOV", "MOV", Video, "video/quicktime", ["mov"], read: yes, write: yes,
         wheels: [Video], read_engine: Ffmpeg, write_engine: Ffmpeg),
    fmt!("MKV", "MKV", Video, "video/x-matroska", ["mkv"], read: yes, write: yes,
         wheels: [Video], read_engine: Ffmpeg, write_engine: Ffmpeg),
    fmt!("WEBM", "WEBM", Video, "video/webm", ["webm"], read: yes, write: yes,
         wheels: [Video], read_engine: Ffmpeg, write_engine: Ffmpeg),
    fmt!("AVI", "AVI", Video, "video/x-msvideo", ["avi"], read: yes, write: yes,
         wheels: [Video], read_engine: Ffmpeg, write_engine: Ffmpeg),
    fmt!("WMV", "WMV", Video, "video/x-ms-wmv", ["wmv"], read: yes, write: yes,
         wheels: [Video], read_engine: Ffmpeg, write_engine: Ffmpeg),
    fmt!("GIF", "GIF", Video, "image/gif", ["gif"], read: yes, write: yes,
         wheels: [Video], read_engine: Ffmpeg, write_engine: Ffmpeg),
    // ----------------------------------------------------------------- Audio
    fmt!("MP3", "MP3", Audio, "audio/mpeg", ["mp3"], read: yes, write: yes,
         wheels: [Audio, Video], read_engine: Ffmpeg, write_engine: Ffmpeg),
    fmt!("M4A", "M4A", Audio, "audio/mp4", ["m4a"], read: yes, write: yes,
         wheels: [Audio], read_engine: Ffmpeg, write_engine: Ffmpeg),
    fmt!("WAV", "WAV", Audio, "audio/wav", ["wav"], read: yes, write: yes,
         wheels: [Audio], read_engine: Ffmpeg, write_engine: Ffmpeg),
    fmt!("FLAC", "FLAC", Audio, "audio/flac", ["flac"], read: yes, write: yes,
         wheels: [Audio], read_engine: Ffmpeg, write_engine: Ffmpeg),
    fmt!("OGG", "OGG", Audio, "audio/ogg", ["ogg", "oga"], read: yes, write: yes,
         wheels: [Audio], read_engine: Ffmpeg, write_engine: Ffmpeg),
    fmt!("OPUS", "OPUS", Audio, "audio/opus", ["opus"], read: yes, write: yes,
         wheels: [Audio], read_engine: Ffmpeg, write_engine: Ffmpeg),
    fmt!("AIFF", "AIFF", Audio, "audio/aiff", ["aiff", "aif"], read: yes, write: yes,
         wheels: [Audio], read_engine: Ffmpeg, write_engine: Ffmpeg),
    fmt!("WMA", "WMA", Audio, "audio/x-ms-wma", ["wma"], read: yes, write: yes,
         wheels: [Audio], read_engine: Ffmpeg, write_engine: Ffmpeg),
    // ------------------------------------------------------------------- PDF
    fmt!("PDF", "PDF", Pdf, "application/pdf", ["pdf"], read: yes, write: yes,
         wheels: [Pdf, Images, Documents, Slides, Spreadsheets, Epub],
         read_engine: PopplerRender, write_engine: PrintPdf),
    // ------------------------------------------------------------- Documents
    fmt!("DOCX", "DOCX", Documents,
         "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
         ["docx"], read: yes, write: yes,
         wheels: [Documents, Epub, Pdf], read_engine: Pandoc, write_engine: Pandoc),
    fmt!("ODT", "ODT", Documents, "application/vnd.oasis.opendocument.text",
         ["odt"], read: yes, write: yes,
         wheels: [Documents, Epub, Pdf], read_engine: LibreOffice, write_engine: LibreOffice),
    fmt!("MD", "MD", Documents, "text/markdown", ["md", "markdown"], read: yes, write: yes,
         wheels: [Documents, Pdf, Epub], read_engine: Pandoc, write_engine: Pandoc),
    fmt!("TXT", "TXT", Documents, "text/plain", ["txt"], read: yes, write: yes,
         wheels: [Documents, Pdf, Epub, Subtitles],
         read_engine: Pandoc, write_engine: NativeText),
    fmt!("HTML", "HTML", Documents, "text/html", ["html", "htm"], read: yes, write: yes,
         wheels: [Documents, Epub], read_engine: Pandoc, write_engine: Pandoc),
    // ---------------------------------------------------------------- Slides
    fmt!("PPTX", "PPTX", Slides,
         "application/vnd.openxmlformats-officedocument.presentationml.presentation",
         ["pptx"], read: yes, write: yes,
         wheels: [Slides, Pdf], read_engine: LibreOffice, write_engine: Sidecar),
    // Legacy PowerPoint is import-only.
    fmt!("PPT", "PPT", Slides, "application/vnd.ms-powerpoint", ["ppt"], read: yes, write: no,
         wheels: [], read_engine: LibreOffice, write_engine: LibreOffice),
    // ---------------------------------------------------------- Spreadsheets
    fmt!("XLSX", "XLSX", Spreadsheets,
         "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
         ["xlsx"], read: yes, write: yes,
         wheels: [Spreadsheets, Pdf], read_engine: Calamine, write_engine: RustXlsx),
    fmt!("ODS", "ODS", Spreadsheets, "application/vnd.oasis.opendocument.spreadsheet",
         ["ods"], read: yes, write: yes,
         wheels: [Spreadsheets], read_engine: Calamine, write_engine: LibreOffice),
    fmt!("CSV", "CSV", Spreadsheets, "text/csv", ["csv"], read: yes, write: yes,
         wheels: [Spreadsheets], read_engine: Calamine, write_engine: NativeText),
    fmt!("TSV", "TSV", Spreadsheets, "text/tab-separated-values", ["tsv"],
         read: yes, write: yes, wheels: [Spreadsheets],
         read_engine: Calamine, write_engine: NativeText),
    // Legacy Excel is import-only.
    fmt!("XLS", "XLS", Spreadsheets, "application/vnd.ms-excel", ["xls"], read: yes, write: no,
         wheels: [], read_engine: Calamine, write_engine: LibreOffice),
    // ------------------------------------------------------------------ EPUB
    fmt!("EPUB", "EPUB", Epub, "application/epub+zip", ["epub"], read: yes, write: yes,
         wheels: [Epub, Pdf, Documents], read_engine: Pandoc, write_engine: Pandoc),
    // ------------------------------------------------------------- Subtitles
    fmt!("SRT", "SRT", Subtitles, "application/x-subrip", ["srt"], read: yes, write: yes,
         wheels: [Subtitles], read_engine: SubtitleNative, write_engine: SubtitleNative),
    fmt!("VTT", "VTT", Subtitles, "text/vtt", ["vtt"], read: yes, write: yes,
         wheels: [Subtitles], read_engine: SubtitleNative, write_engine: SubtitleNative),
    // -------------------------------------------------------------- Archives
    fmt!("ZIP", "ZIP", Archives, "application/zip", ["zip"], read: yes, write: yes,
         wheels: [Archives], read_engine: SevenZip, write_engine: SevenZip),
    fmt!("TAR", "TAR", Archives, "application/x-tar", ["tar"], read: yes, write: yes,
         wheels: [Archives], read_engine: SevenZip, write_engine: SevenZip),
    fmt!("GZIP", "GZIP", Archives, "application/gzip", ["gz", "gzip"], read: yes, write: yes,
         wheels: [Archives], read_engine: SevenZip, write_engine: SevenZip),
    // Decoding only: no encoder is bundled or licensed.
    fmt!("RAR", "RAR", Archives, "application/vnd.rar", ["rar"], read: yes, write: no,
         wheels: [], read_engine: SevenZip, write_engine: SevenZip),
];

/// Per-pair engine overrides. A pair not listed here uses the target's
/// `write_engine` (or the input's `read_engine` for extract-style jobs).
pub static PAIR_OVERRIDES: &[(FormatId, FormatId, EngineRef)] = &[
    // Office → PDF needs a real layout engine; pandoc alone cannot honour it.
    (FormatId("DOCX"), FormatId("PDF"), EngineRef::LibreOffice),
    (FormatId("ODT"), FormatId("PDF"), EngineRef::LibreOffice),
    (FormatId("EPUB"), FormatId("PDF"), EngineRef::LibreOffice),
    (FormatId("PPTX"), FormatId("PDF"), EngineRef::LibreOffice),
    (FormatId("PPT"), FormatId("PDF"), EngineRef::LibreOffice),
    (FormatId("XLSX"), FormatId("PDF"), EngineRef::LibreOffice),
    (FormatId("XLS"), FormatId("PDF"), EngineRef::LibreOffice),
    (FormatId("ODS"), FormatId("PDF"), EngineRef::LibreOffice),
    (FormatId("CSV"), FormatId("PDF"), EngineRef::LibreOffice),
    (FormatId("TSV"), FormatId("PDF"), EngineRef::LibreOffice),
    // DOCX ↔ ODT keeps tables and paragraph structure only through LibreOffice.
    (FormatId("DOCX"), FormatId("ODT"), EngineRef::LibreOffice),
    (FormatId("ODT"), FormatId("DOCX"), EngineRef::LibreOffice),
    // EPUB → ODT: pandoc to HTML, then LibreOffice for the page model.
    (FormatId("EPUB"), FormatId("ODT"), EngineRef::LibreOffice),
    // PDF → office/EPUB lives in the Python sidecar.
    (FormatId("PDF"), FormatId("DOCX"), EngineRef::Sidecar),
    (FormatId("PDF"), FormatId("PPTX"), EngineRef::Sidecar),
    (FormatId("PDF"), FormatId("XLSX"), EngineRef::Sidecar),
    (FormatId("PDF"), FormatId("EPUB"), EngineRef::Sidecar),
    // PDF → text and images are Poppler's job.
    (FormatId("PDF"), FormatId("TXT"), EngineRef::PopplerText),
    (FormatId("PDF"), FormatId("MD"), EngineRef::PopplerText),
    (FormatId("PDF"), FormatId("PNG"), EngineRef::PopplerRender),
    (FormatId("PDF"), FormatId("JPG"), EngineRef::PopplerRender),
    // Images → PDF is written in-process, not through ImageMagick.
    (FormatId("JPG"), FormatId("PDF"), EngineRef::PrintPdf),
    (FormatId("PNG"), FormatId("PDF"), EngineRef::PrintPdf),
    (FormatId("WEBP"), FormatId("PDF"), EngineRef::PrintPdf),
    (FormatId("HEIC"), FormatId("PDF"), EngineRef::PrintPdf),
    (FormatId("TIFF"), FormatId("PDF"), EngineRef::PrintPdf),
    (FormatId("AVIF"), FormatId("PDF"), EngineRef::PrintPdf),
    (FormatId("BMP"), FormatId("PDF"), EngineRef::PrintPdf),
    (FormatId("SVG"), FormatId("PDF"), EngineRef::ImageMagick),
    // ODS output goes through LibreOffice's spreadsheet writer.
    (FormatId("CSV"), FormatId("ODS"), EngineRef::LibreOffice),
    (FormatId("TSV"), FormatId("ODS"), EngineRef::LibreOffice),
    (FormatId("XLSX"), FormatId("ODS"), EngineRef::LibreOffice),
    // XLSX output from flat text is written in-process.
    (FormatId("CSV"), FormatId("XLSX"), EngineRef::RustXlsx),
    (FormatId("TSV"), FormatId("XLSX"), EngineRef::RustXlsx),
    // Subtitles stay in-process.
    (FormatId("SRT"), FormatId("TXT"), EngineRef::SubtitleNative),
    (FormatId("VTT"), FormatId("TXT"), EngineRef::SubtitleNative),
];

/// A file tool. Tools are per-category: `Compress` exists for images, video,
/// audio and PDF and means something different in each.
pub struct Tool {
    pub id: ToolId,
    pub category: Category,
    pub label: &'static str,
    pub blurb: &'static str,
    pub engine: EngineRef,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ToolId {
    Compress,
    Crop,
    Annotate,
    AddBackground,
    EditPhoto,
    Metadata,
    Redact,
    Trim,
    Speed,
    Mute,
    Snapshot,
    Join,
    Split,
    Normalize,
    Channels,
    Visualize,
    Bleep,
    Organize,
    Merge,
    ExtractText,
    SavePages,
    Extract,
}

impl ToolId {
    pub fn as_str(self) -> &'static str {
        match self {
            ToolId::Compress => "compress",
            ToolId::Crop => "crop",
            ToolId::Annotate => "annotate",
            ToolId::AddBackground => "add-background",
            ToolId::EditPhoto => "edit-photo",
            ToolId::Metadata => "metadata",
            ToolId::Redact => "redact",
            ToolId::Trim => "trim",
            ToolId::Speed => "speed",
            ToolId::Mute => "mute",
            ToolId::Snapshot => "snapshot",
            ToolId::Join => "join",
            ToolId::Split => "split",
            ToolId::Normalize => "normalize",
            ToolId::Channels => "channels",
            ToolId::Visualize => "visualize",
            ToolId::Bleep => "bleep",
            ToolId::Organize => "organize",
            ToolId::Merge => "merge",
            ToolId::ExtractText => "extract-text",
            ToolId::SavePages => "save-pages",
            ToolId::Extract => "extract",
        }
    }

    pub fn parse(s: &str) -> Option<Self> {
        TOOLS.iter().map(|t| t.id).find(|id| id.as_str() == s)
    }
}

impl Serialize for ToolId {
    fn serialize<S: serde::Serializer>(&self, s: S) -> Result<S::Ok, S::Error> {
        s.serialize_str(self.as_str())
    }
}

impl<'de> Deserialize<'de> for ToolId {
    fn deserialize<D: serde::Deserializer<'de>>(d: D) -> Result<Self, D::Error> {
        let s = String::deserialize(d)?;
        ToolId::parse(&s).ok_or_else(|| serde::de::Error::custom(format!("unknown tool {s:?}")))
    }
}

impl fmt::Display for ToolId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.write_str(self.as_str())
    }
}

macro_rules! tool {
    ($id:ident, $cat:ident, $label:literal, $blurb:literal, $engine:ident) => {
        Tool {
            id: ToolId::$id,
            category: Category::$cat,
            label: $label,
            blurb: $blurb,
            engine: EngineRef::$engine,
        }
    };
}

/// The tool wheel, in wheel order. Copy is written for a multi-platform audience
/// and never names a key.
pub static TOOLS: &[Tool] = &[
    // Images
    tool!(
        Compress,
        Images,
        "Compress",
        "Hit a target size or a percentage; the preview shows the result before you commit.",
        ImageMagick
    ),
    tool!(
        Crop,
        Images,
        "Crop",
        "Drag the crop box or pick an aspect ratio.",
        ImageMagick
    ),
    tool!(
        Annotate,
        Images,
        "Annotate",
        "Arrows, shapes, text and numbered callouts burned into a copy.",
        ImageMagick
    ),
    tool!(
        AddBackground,
        Images,
        "Add background",
        "Place the photo on a colour, a gradient or another image, with padding and rounded corners.",
        ImageMagick
    ),
    tool!(
        EditPhoto,
        Images,
        "Edit photo",
        "Adjust exposure, colour and detail without leaving the app.",
        ImageMagick
    ),
    tool!(
        Metadata,
        Images,
        "Metadata",
        "Read, edit or strip camera and location tags across a selection.",
        ImageMagick
    ),
    tool!(
        Redact,
        Images,
        "Redact",
        "Detect faces and text, then fill the boxes you keep.",
        ImageMagick
    ),
    // Video
    tool!(
        Compress,
        Video,
        "Compress",
        "Hit a target size with a two-pass encode that keeps the audio intact.",
        Ffmpeg
    ),
    tool!(
        Trim,
        Video,
        "Trim",
        "Drag the start and end handles on the filmstrip and step through frames.",
        Ffmpeg
    ),
    tool!(
        Crop,
        Video,
        "Crop",
        "Drag the frame or pick an aspect ratio; the audio is untouched.",
        Ffmpeg
    ),
    tool!(
        Speed,
        Video,
        "Speed",
        "Speed up or slow down the picture and keep the audio at its original pitch.",
        Ffmpeg
    ),
    tool!(
        Mute,
        Video,
        "Remove audio",
        "Drop the audio track and save a silent copy.",
        Ffmpeg
    ),
    tool!(
        Snapshot,
        Video,
        "Save frames",
        "Export still frames as PNG at chosen timestamps or a fixed interval.",
        Ffmpeg
    ),
    tool!(
        Redact,
        Video,
        "Redact",
        "Blur a region for a time range and re-encode a clean copy.",
        Ffmpeg
    ),
    tool!(
        Join,
        Video,
        "Join",
        "Arrange clips in playback order and save one file.",
        Ffmpeg
    ),
    tool!(
        Split,
        Video,
        "Split",
        "Mark sections on the timeline and save each one as its own clip in a folder.",
        Ffmpeg
    ),
    // Audio
    tool!(
        Compress,
        Audio,
        "Compress",
        "Shrink a recording to a target size or bitrate.",
        Ffmpeg
    ),
    tool!(
        Normalize,
        Audio,
        "Normalize",
        "Even out loudness to a broadcast target.",
        Ffmpeg
    ),
    tool!(
        Trim,
        Audio,
        "Trim",
        "Cut with waveform handles and preview the edges.",
        Ffmpeg
    ),
    tool!(
        Channels,
        Audio,
        "Channels",
        "Convert to mono or stereo.",
        Ffmpeg
    ),
    tool!(
        Visualize,
        Audio,
        "Visualize",
        "Turn a recording into a waveform video on a background you pick.",
        Ffmpeg
    ),
    tool!(
        Bleep,
        Audio,
        "Bleep",
        "Mark ranges on the waveform and replace them with a tone.",
        Ffmpeg
    ),
    tool!(
        Metadata,
        Audio,
        "Metadata",
        "Edit title, artist, album and cover art.",
        Ffmpeg
    ),
    // PDF
    tool!(
        Compress,
        Pdf,
        "Compress",
        "Downsample to a preset quality tier and keep the pages readable.",
        Ghostscript
    ),
    tool!(
        Organize,
        Pdf,
        "Organize pages",
        "Reorder, rotate, duplicate or delete pages, then save a new file.",
        Qpdf
    ),
    tool!(
        Metadata,
        Pdf,
        "Metadata",
        "Edit the document information dictionary.",
        Qpdf
    ),
    tool!(
        Merge,
        Pdf,
        "Merge",
        "Combine the selected files into one document in selection order.",
        Qpdf
    ),
    tool!(
        Split,
        Pdf,
        "Split pages",
        "Save every page as its own PDF in a new folder.",
        Qpdf
    ),
    tool!(
        SavePages,
        Pdf,
        "Save pages as images",
        "Render every page at 300 DPI as JPG or PNG into a folder.",
        PopplerRender
    ),
    tool!(
        ExtractText,
        Pdf,
        "Extract text",
        "Pull selectable text out, or Markdown when headings and tables matter.",
        PopplerText
    ),
    // Archives
    tool!(
        Extract,
        Archives,
        "Extract",
        "Unpack into a folder beside the original.",
        SevenZip
    ),
];

/// Lookup helpers over [`FORMATS`] and [`TOOLS`].
pub struct Registry;

impl Registry {
    pub fn get(id: FormatId) -> Option<&'static Format> {
        FORMATS.iter().find(|f| f.id == id)
    }

    pub fn by_id(s: &str) -> Option<&'static Format> {
        FORMATS.iter().find(|f| f.id.0.eq_ignore_ascii_case(s))
    }

    /// Detect a format from a path's extension.
    pub fn detect(path: &Path) -> Option<&'static Format> {
        let ext = path.extension()?.to_str()?.to_ascii_lowercase();
        FORMATS.iter().find(|f| f.exts.iter().any(|e| *e == ext))
    }

    pub fn category_of(path: &Path) -> Option<Category> {
        Self::detect(path).map(|f| f.category)
    }

    /// Convert-wheel targets for a category and source: writable formats listing
    /// that category, minus the source itself.
    pub fn wheel_targets(category: Category, source: Option<FormatId>) -> Vec<&'static Format> {
        FORMATS
            .iter()
            .filter(|f| f.write && f.is_listed_in(category))
            .filter(|f| Some(f.id) != source)
            .collect()
    }

    pub fn tools_for(category: Category) -> Vec<&'static Tool> {
        TOOLS.iter().filter(|t| t.category == category).collect()
    }

    pub fn tool(category: Category, id: ToolId) -> Option<&'static Tool> {
        TOOLS.iter().find(|t| t.category == category && t.id == id)
    }

    /// Whether `from` can be converted to `to` at all.
    pub fn can_convert(from: FormatId, to: FormatId) -> bool {
        let Some(src) = Self::get(from) else {
            return false;
        };
        let Some(dst) = Self::get(to) else {
            return false;
        };
        src.read && dst.write && dst.is_listed_in(src.category)
    }

    pub fn engine_for(from: FormatId, to: FormatId) -> Option<EngineRef> {
        if let Some((_, _, e)) = PAIR_OVERRIDES
            .iter()
            .find(|(f, t, _)| *f == from && *t == to)
        {
            return Some(*e);
        }
        if !Self::can_convert(from, to) {
            return None;
        }
        Self::get(to).map(|f| f.write_engine)
    }

    /// Every (input, target) pair the app offers. This is the number the product
    /// copy calls "conversion options"; it is derived, never stored.
    pub fn pairs() -> impl Iterator<Item = (FormatId, FormatId)> {
        FORMATS.iter().flat_map(|src| {
            let category = src.category;
            FORMATS
                .iter()
                .filter(move |dst| dst.write && dst.is_listed_in(category))
                .map(move |dst| (src.id, dst.id))
        })
    }

    /// Tools whose engine is present. Used by the GUI to omit segments and by
    /// Settings to list gaps.
    pub fn tools_for_available(
        category: Category,
        caps: &crate::capability::Capabilities,
    ) -> Vec<&'static Tool> {
        Self::tools_for(category)
            .into_iter()
            .filter(|t| caps.has_engine(t.engine))
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn every_wheel_is_targets_minus_source() {
        // Image wheel: 7 segments for an 8-target category, the 8th being JPG.
        let w = Registry::wheel_targets(Category::Images, Some(FormatId("JPG")));
        assert_eq!(
            w.len(),
            7,
            "{:?}",
            w.iter().map(|f| f.id).collect::<Vec<_>>()
        );
        assert!(!w.iter().any(|f| f.id == FormatId("JPG")));
        let ids: Vec<&str> = w.iter().map(|f| f.id.0).collect();
        assert_eq!(
            ids,
            ["PNG", "WEBP", "HEIC", "TIFF", "AVIF", "BMP", "PDF"],
            "wheel order is registry order"
        );

        let audio = Registry::wheel_targets(Category::Audio, Some(FormatId("MP3")));
        assert_eq!(audio.len(), 7);
        assert!(!audio.iter().any(|f| f.id == FormatId("MP3")));

        let pdf = Registry::wheel_targets(Category::Pdf, Some(FormatId("PDF")));
        assert_eq!(pdf.len(), 9);
    }

    #[test]
    fn read_only_formats_are_not_targets() {
        for id in ["SVG", "PPT", "XLS", "RAR"] {
            let f = Registry::by_id(id).unwrap();
            assert!(!f.write, "{id} must be input only");
            assert!(f.wheels.is_empty(), "{id} must not appear on a wheel");
            assert!(f.read);
        }
    }

    #[test]
    fn counts_are_derived() {
        let n = Registry::pairs().count();
        // The registry must be big enough to matter and must not silently shrink.
        assert!(n > 200, "only {n} pairs");
        assert_eq!(n, Registry::pairs().count());
    }

    #[test]
    fn engine_overrides_cover_office_to_pdf() {
        assert_eq!(
            Registry::engine_for(FormatId("DOCX"), FormatId("PDF")),
            Some(EngineRef::LibreOffice)
        );
        assert_eq!(
            Registry::engine_for(FormatId("PDF"), FormatId("DOCX")),
            Some(EngineRef::Sidecar)
        );
        assert_eq!(
            Registry::engine_for(FormatId("PNG"), FormatId("PDF")),
            Some(EngineRef::PrintPdf)
        );
        assert_eq!(
            Registry::engine_for(FormatId("SVG"), FormatId("PNG")),
            Some(EngineRef::ImageMagick)
        );
    }

    #[test]
    fn unsupported_pairs_report_none() {
        assert_eq!(Registry::engine_for(FormatId("SVG"), FormatId("SVG")), None);
        assert_eq!(Registry::engine_for(FormatId("PNG"), FormatId("MKV")), None);
        assert!(!Registry::can_convert(FormatId("PNG"), FormatId("RAR")));
    }

    #[test]
    fn extension_detection_is_case_insensitive() {
        assert_eq!(
            Registry::detect(Path::new("A.JPEG")).unwrap().id,
            FormatId("JPG")
        );
        assert_eq!(
            Registry::detect(Path::new("x.tar.gz")).unwrap().id,
            FormatId("GZIP")
        );
        assert!(Registry::detect(Path::new("noext")).is_none());
    }
}
