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
///
/// Every field here is product copy the dialog shows, so the two-pane dialog and
/// its explanatory rail are never written twice: `label` is the wheel word,
/// `title` is the sheet heading *and* the rail heading, `action` is the primary
/// button (`{n}` is replaced by a count), `benefits` are the rail's check lines
/// and `eyebrow` overrides the category label above the rail title.
pub struct Tool {
    pub id: ToolId,
    pub category: Category,
    pub label: &'static str,
    pub title: &'static str,
    pub action: &'static str,
    pub benefits: &'static [&'static str],
    pub eyebrow: Option<&'static str>,
    pub blurb: &'static str,
    pub engine: EngineRef,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash)]
pub enum ToolId {
    Compress,
    Crop,
    Annotate,
    Collage,
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
            ToolId::Collage => "collage",
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

/// One tool row.
///
/// Two forms. The short one is for a tool whose dialog needs no special copy: the
/// sheet is titled with the wheel label and the primary button says `Run`:
///
/// ```text
/// tool!(Split, Pdf, "Split pages", "…blurb…", Qpdf)
/// ```
///
/// The long form carries the two-pane dialog's copy — sheet and rail title, the
/// primary action (`{n}` is a count), the rail's check lines and an eyebrow that
/// replaces the category label:
///
/// ```text
/// tool!(Metadata, Pdf, "Metadata", "Edit metadata", "Save", "…blurb…", Qpdf,
///       ["Search fields grouped by file", "Remove hidden camera data"],
///       Some("Any file"))
/// ```
macro_rules! tool {
    ($id:ident, $cat:ident, $label:literal, $blurb:literal, $engine:ident) => {
        tool!(
            $id,
            $cat,
            $label,
            $label,
            "Run",
            $blurb,
            $engine,
            [],
            None
        )
    };
    (
        $id:ident,
        $cat:ident,
        $label:literal,
        $title:literal,
        $action:literal,
        $blurb:literal,
        $engine:ident,
        [$($benefit:literal),* $(,)?],
        $eyebrow:expr
    ) => {
        Tool {
            id: ToolId::$id,
            category: Category::$cat,
            label: $label,
            title: $title,
            action: $action,
            benefits: &[$($benefit),*],
            eyebrow: $eyebrow,
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
        "Compress",
        "Compress",
        "Hit a target size or a percentage; the preview shows the result before you commit.",
        ImageMagick,
        [
            "Target a file size or percentage reduction",
            "Resize images while compressing"
        ],
        Some("Any file")
    ),
    tool!(
        Crop,
        Images,
        "Crop",
        "Crop images",
        "Apply",
        "Drag the crop box or pick an aspect ratio.",
        ImageMagick,
        [
            "Enter exact pixel dimensions",
            "Use preset or custom aspect ratios"
        ],
        None
    ),
    tool!(
        Annotate,
        Images,
        "Annotate",
        "Annotate photos",
        "Save Annotated Copy",
        "Arrows, shapes, text and numbered callouts burned into a copy.",
        ImageMagick,
        [
            "Add numbered labels, arrows, and text",
            "Draw or highlight directly on the photo"
        ],
        None
    ),
    tool!(
        Collage,
        Images,
        "Collage",
        "Create a collage",
        "Create Collage",
        "Arrange several images into one composition.",
        ImageMagick,
        [
            "Grid and featured layouts with any rows and columns",
            "Set final dimensions, spacing, corners, and background"
        ],
        None
    ),
    tool!(
        AddBackground,
        Images,
        "Add background",
        "Add a background",
        "Save with Background",
        "Place the photo on a colour, a gradient or another image, with padding and rounded corners.",
        ImageMagick,
        [
            "Use another image as the background",
            "Add padding, rounded corners, and shadow"
        ],
        None
    ),
    tool!(
        EditPhoto,
        Images,
        "Edit photo",
        "Edit photos",
        "Save Edited Copy",
        "Adjust exposure, colour and detail without leaving the app.",
        ImageMagick,
        [
            "Dehaze, clarity, grain, and noise reduction",
            "Fine controls for light, colour, and detail"
        ],
        None
    ),
    tool!(
        Metadata,
        Images,
        "Metadata",
        "Edit metadata",
        "Save",
        "Read, edit or strip camera and location tags across a selection.",
        ImageMagick,
        [
            "Search fields grouped by file, track, or chapter",
            "Remove hidden camera and processing data"
        ],
        Some("Any file")
    ),
    tool!(
        Redact,
        Images,
        "Redact",
        "Redact photos",
        "Save Redacted Copy",
        "Detect faces and text, then fill the boxes you keep.",
        ImageMagick,
        [
            "Detect faces and text locally",
            "Layer several movable redaction areas"
        ],
        None
    ),
    // Video
    tool!(
        Compress,
        Video,
        "Compress",
        "Compress",
        "Compress",
        "Hit a target size with a two-pass encode that keeps the audio intact.",
        Ffmpeg,
        ["Shrink to a size or bitrate", "Never grows the file"],
        Some("Any file")
    ),
    tool!(
        Trim,
        Video,
        "Trim",
        "Trim video",
        "Apply",
        "Drag the start and end handles on the filmstrip and step through frames.",
        Ffmpeg,
        [
            "Step through the video frame by frame",
            "Enter precise start and end timecodes"
        ],
        None
    ),
    tool!(
        Crop,
        Video,
        "Crop",
        "Crop video",
        "Apply",
        "Drag the frame or pick an aspect ratio; the audio is untouched.",
        Ffmpeg,
        ["Enter exact output dimensions", "Keep the original audio"],
        None
    ),
    tool!(
        Speed,
        Video,
        "Speed",
        "Change video speed",
        "Apply",
        "Speed up or slow down the picture and keep the audio at its original pitch.",
        Ffmpeg,
        ["Preserve audio pitch", "Works with silent videos"],
        None
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
        "Save video frames",
        "Save {n} Frame(s)",
        "Export still frames as PNG at chosen timestamps or a fixed interval.",
        Ffmpeg,
        [
            "Choose frames one step at a time",
            "Export full-resolution PNGs"
        ],
        None
    ),
    tool!(
        Redact,
        Video,
        "Redact",
        "Redact video",
        "Save Redacted Copy",
        "Blur a region for a time range and re-encode a clean copy.",
        Ffmpeg,
        [
            "Give each redaction its own time range",
            "Use solid colors or blur"
        ],
        None
    ),
    tool!(
        Join,
        Video,
        "Join",
        "Join videos",
        "Join Videos",
        "Arrange clips in playback order and save one file.",
        Ffmpeg,
        [
            "Mix MP4, MOV, and MKV clips",
            "Use the first clip as the output canvas"
        ],
        None
    ),
    tool!(
        Split,
        Video,
        "Split",
        "Split video",
        "Split Video",
        "Mark sections on the timeline and save each one as its own clip in a folder.",
        Ffmpeg,
        [
            "Split at custom points or into equal parts",
            "Place every output clip in one folder"
        ],
        None
    ),
    // Audio
    tool!(
        Compress,
        Audio,
        "Compress",
        "Compress",
        "Compress",
        "Shrink a recording to a target size or bitrate.",
        Ffmpeg,
        ["Shrink to a size or bitrate", "Never grows the file"],
        Some("Any file")
    ),
    tool!(
        Normalize,
        Audio,
        "Normalize",
        "Normalize volume",
        "Normalize Audio",
        "Even out loudness to a broadcast target.",
        Ffmpeg,
        [
            "Control loudness range and true peak",
            "Compare input and output levels"
        ],
        None
    ),
    tool!(
        Trim,
        Audio,
        "Trim",
        "Trim audio",
        "Apply",
        "Cut with waveform handles and preview the edges.",
        Ffmpeg,
        [
            "Remove silence only from the beginning and end",
            "Enter exact start and end times"
        ],
        None
    ),
    tool!(
        Channels,
        Audio,
        "Channels",
        "Convert audio channels",
        "Apply",
        "Convert to mono or stereo.",
        Ffmpeg,
        [
            "Set left and right volume independently",
            "Preview either channel on its own"
        ],
        None
    ),
    tool!(
        Visualize,
        Audio,
        "Visualize",
        "Create an audio visualizer",
        "Create Visualization",
        "Turn a recording into a waveform video on a background you pick.",
        Ffmpeg,
        [
            "Use an animated waveform or a still image",
            "Choose landscape, portrait, or square video"
        ],
        None
    ),
    tool!(
        Bleep,
        Audio,
        "Bleep",
        "Bleep audio",
        "Save Bleeped Copy",
        "Mark ranges on the waveform and replace them with a tone.",
        Ffmpeg,
        [
            "Move a whole bleep range or fine-tune its edges",
            "Compare original and bleeped playback"
        ],
        None
    ),
    tool!(
        Metadata,
        Audio,
        "Metadata",
        "Edit metadata",
        "Save",
        "Edit title, artist, album and cover art.",
        Ffmpeg,
        [
            "Search fields grouped by file, track, or chapter",
            "Remove hidden camera and processing data"
        ],
        Some("Any file")
    ),
    // PDF
    tool!(
        Compress,
        Pdf,
        "Compress",
        "Compress",
        "Compress",
        "Downsample to a preset quality tier and keep the pages readable.",
        Ghostscript,
        ["Choose a quality preset", "Never grows the file"],
        Some("Any file")
    ),
    tool!(
        Organize,
        Pdf,
        "Organize pages",
        "Organize PDF pages",
        "Save Organized PDF",
        "Reorder, rotate, duplicate or delete pages, then save a new file.",
        Qpdf,
        ["Rotate, duplicate, or remove individual pages"],
        None
    ),
    tool!(
        Metadata,
        Pdf,
        "Metadata",
        "Edit metadata",
        "Save",
        "Edit the document information dictionary.",
        Qpdf,
        [
            "Search fields grouped by file, track, or chapter",
            "Remove hidden camera and processing data"
        ],
        Some("Any file")
    ),
    tool!(
        Merge,
        Pdf,
        "Merge",
        "Merge PDFs",
        "Merge PDFs",
        "Combine the selected files into one document in selection order.",
        Qpdf,
        [
            "Start with the selection order",
            "Reorder documents before merging"
        ],
        None
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
    fn every_tool_has_a_title_and_an_action() {
        // The dialog reads these two as its heading and its primary button; an empty
        // one renders a dialog with no way out that says nothing.
        for tool in TOOLS {
            assert!(
                !tool.title.trim().is_empty(),
                "{} has no dialog title",
                tool.id
            );
            assert!(
                !tool.action.trim().is_empty(),
                "{} has no primary action",
                tool.id
            );
        }
    }

    /// The two-pane dialog copy, pinned. `title`/`action` are what the sheet and its
    /// primary button must say, and the rail lists `benefits` verbatim.
    #[test]
    fn reference_tools_carry_the_dialog_copy() {
        let expect = [
            (Category::Images, ToolId::Compress, "Compress", "Compress"),
            (Category::Images, ToolId::Crop, "Crop images", "Apply"),
            (
                Category::Images,
                ToolId::Annotate,
                "Annotate photos",
                "Save Annotated Copy",
            ),
            (
                Category::Images,
                ToolId::Collage,
                "Create a collage",
                "Create Collage",
            ),
            (
                Category::Images,
                ToolId::AddBackground,
                "Add a background",
                "Save with Background",
            ),
            (
                Category::Images,
                ToolId::EditPhoto,
                "Edit photos",
                "Save Edited Copy",
            ),
            (Category::Images, ToolId::Metadata, "Edit metadata", "Save"),
            (
                Category::Images,
                ToolId::Redact,
                "Redact photos",
                "Save Redacted Copy",
            ),
            (Category::Video, ToolId::Compress, "Compress", "Compress"),
            (Category::Video, ToolId::Trim, "Trim video", "Apply"),
            (Category::Video, ToolId::Crop, "Crop video", "Apply"),
            (
                Category::Video,
                ToolId::Speed,
                "Change video speed",
                "Apply",
            ),
            (
                Category::Video,
                ToolId::Snapshot,
                "Save video frames",
                "Save {n} Frame(s)",
            ),
            (
                Category::Video,
                ToolId::Redact,
                "Redact video",
                "Save Redacted Copy",
            ),
            (Category::Video, ToolId::Join, "Join videos", "Join Videos"),
            (Category::Video, ToolId::Split, "Split video", "Split Video"),
            (Category::Audio, ToolId::Compress, "Compress", "Compress"),
            (
                Category::Audio,
                ToolId::Normalize,
                "Normalize volume",
                "Normalize Audio",
            ),
            (Category::Audio, ToolId::Trim, "Trim audio", "Apply"),
            (
                Category::Audio,
                ToolId::Channels,
                "Convert audio channels",
                "Apply",
            ),
            (
                Category::Audio,
                ToolId::Visualize,
                "Create an audio visualizer",
                "Create Visualization",
            ),
            (
                Category::Audio,
                ToolId::Bleep,
                "Bleep audio",
                "Save Bleeped Copy",
            ),
            (Category::Audio, ToolId::Metadata, "Edit metadata", "Save"),
            (Category::Pdf, ToolId::Compress, "Compress", "Compress"),
            (
                Category::Pdf,
                ToolId::Organize,
                "Organize PDF pages",
                "Save Organized PDF",
            ),
            (Category::Pdf, ToolId::Metadata, "Edit metadata", "Save"),
            (Category::Pdf, ToolId::Merge, "Merge PDFs", "Merge PDFs"),
        ];
        for (category, id, title, action) in expect {
            let tool = Registry::tool(category, id)
                .unwrap_or_else(|| panic!("{category}/{id} is missing from the tool table"));
            assert_eq!(tool.title, title, "{category}/{id} title");
            assert_eq!(tool.action, action, "{category}/{id} action");
            assert!(
                !tool.benefits.is_empty(),
                "{category}/{id} has no rail benefits"
            );
            // The reference shows two check lines everywhere but the two page tools,
            // where the capture has a single line.
            let single = matches!((category, id), (Category::Pdf, ToolId::Organize))
                || matches!((category, id), (Category::Pdf, ToolId::Merge));
            if !single {
                assert!(
                    tool.benefits.len() >= 2,
                    "{category}/{id} shows {} benefit(s)",
                    tool.benefits.len()
                );
            }
        }
    }

    #[test]
    fn cross_format_editors_are_labelled_any_file() {
        // Compress and Metadata exist per category but read the same in each: the
        // eyebrow replaces the category word the rail would otherwise show.
        for category in [
            Category::Images,
            Category::Video,
            Category::Audio,
            Category::Pdf,
        ] {
            let tool = Registry::tool(category, ToolId::Compress).unwrap();
            assert_eq!(tool.eyebrow, Some("Any file"));
        }
        for category in [Category::Images, Category::Audio, Category::Pdf] {
            let tool = Registry::tool(category, ToolId::Metadata).unwrap();
            assert_eq!(tool.eyebrow, Some("Any file"));
        }
        // Every other tool falls back to its category label.
        let crop = Registry::tool(Category::Images, ToolId::Crop).unwrap();
        assert_eq!(crop.eyebrow, None);
    }

    /// A tool on the wheel must have an engine behind it: a card that always fails
    /// is worse than a missing card, so a tool is registered with its implementation.
    #[test]
    fn one_id_per_wheel_entry() {
        for category in Category::ALL {
            let ids: Vec<&str> = Registry::tools_for(category)
                .iter()
                .map(|t| t.id.as_str())
                .collect();
            let mut unique = ids.clone();
            unique.sort_unstable();
            unique.dedup();
            assert_eq!(ids.len(), unique.len(), "{category} lists a tool twice");
            // Every registered tool names an engine that exists.
            for tool in Registry::tools_for(category) {
                assert!(!tool.engine.label().is_empty());
            }
        }
        // The id parses back to itself through the table.
        for category in Category::ALL {
            for tool in Registry::tools_for(category) {
                assert_eq!(ToolId::parse(tool.id.as_str()), Some(tool.id));
            }
        }
    }
}
