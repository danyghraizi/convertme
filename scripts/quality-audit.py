#!/usr/bin/env python3
"""Convert and run tools through the CLI, then check what came out.

The third layer, below `app-e2e.py` (which drives the window): this one exercises the
same planner, queue and engines headlessly, so a whole conversion matrix can be
audited in one run and every produced file is checked by `scripts/validate.py`
rather than by its file name.

    ./scripts/quality-audit.py                     # every case
    ./scripts/quality-audit.py --only pdf          # substring filter
    ./scripts/quality-audit.py --list

Each case copies its fixture into its own directory first: the CLI publishes beside
its input, so a run never writes next to the user's files and two cases can share a
fixture. A case whose engine is missing on this machine is reported as `skipped`
with the reason, never silently dropped.

Exit codes: 0 every case passed or skipped; 1 a check failed.
"""

from __future__ import annotations

import argparse
import json
import hashlib
import os
import re
import shutil
import subprocess
import sys
import tempfile
import zipfile
from datetime import datetime, timezone
from pathlib import Path

import validate

REPO = Path(__file__).resolve().parent.parent
BIN = REPO / ("target/release/convertme.exe" if os.name == "nt" else "target/release/convertme")
DEFAULT_FIXTURES = Path(os.environ.get("CONVERTME_FIXTURES", REPO / "fixtures"))

FULL_UIA_FIXTURES = (
    "images/poster-portrait.png",
    "images/photo-landscape-4000x3000.jpg",
    "images/screenshot-text.png",
    "images/tiny-8x8.png",
    "video/clip-landscape-720p-10s.mp4",
    "audio/speech.mp3",
    "audio/speech.flac",
    "pdf/report-5-pages.pdf",
)


class Case:
    """One CLI run and the facts its output must show."""

    def __init__(self, name, source, argv, check):  # source: str | list[str]
        self.name = name
        self.source = source
        self.argv = argv
        self.check = check


def convert(name: str, source: str, target: str, check) -> Case:
    return Case(
        name,
        source,
        ["convert", "{input}", "--to", target, "--overwrite", "replace"],
        check,
    )


def tool_case(name: str, source: str, tool: str, category: str, params: dict, check) -> Case:
    return Case(
        name,
        source,
        [
            "tool",
            "{input}",
            "--tool",
            tool,
            "--category",
            category,
            "--params",
            json.dumps(params),
            "--overwrite",
            "replace",
        ],
        check,
    )


# --------------------------------------------------------------------- checks

def image_check(format_name=None, size=None, alpha=None, unchanged_size=False):
    def check(produced: list[Path]) -> list[dict]:
        checks = []
        page_set = bool(
            len(produced) > 1
            and CURRENT_INPUTS
            and CURRENT_INPUTS[0].suffix.lower() == ".pdf"
        )
        if len(produced) != 1 and not page_set:
            return [{"name": "one artifact", "ok": False, "actual": len(produced)}]
        if page_set:
            checks.append(
                {"name": "page images", "ok": True, "actual": len(produced)}
            )
        artifact = produced[0]
        facts = validate.image(artifact)
        if facts is None:
            return [{"name": "readable image", "ok": False, "actual": "unreadable"}]
        checks.append(
            {"name": "magic", "ok": True, "actual": facts["magick"]}
        )
        if format_name:
            # ImageMagick reports the coder (`JPEG`, `TIFF`); the registry says `JPG`,
            # `TIF`. Compare the canonical names, not the spellings.
            aliases = {"JPEG": "JPG", "TIFF": "TIF"}
            actual = facts["magick"].upper()
            canonical = aliases.get(actual, actual)
            checks.append(
                {
                    "name": f"container is {format_name}",
                    "ok": canonical.startswith(format_name.upper()),
                    "actual": facts["magick"],
                }
            )
        if size:
            checks.append(
                {
                    "name": f"{size[0]}×{size[1]}",
                    "ok": (facts["width"], facts["height"]) == tuple(size),
                    "actual": [facts["width"], facts["height"]],
                }
            )
        if alpha is not None:
            checks.append({"name": "alpha", "ok": facts["alpha"] == alpha, "actual": facts["alpha"]})
        if unchanged_size:
            checks.append(
                {
                    "name": "pixels survive the format change",
                    "ok": facts["width"] > 0 and facts["height"] > 0,
                    "actual": [facts["width"], facts["height"]],
                }
            )
        return checks

    return check


def media_check(container=None, seconds=None, audio=True, video=True, size=None):
    def check(produced: list[Path]) -> list[dict]:
        if len(produced) != 1:
            return [{"name": "one artifact", "ok": False, "actual": len(produced)}]
        facts = validate.media(produced[0])
        if facts is None:
            return [{"name": "readable media", "ok": False, "actual": "unreadable"}]
        checks = []
        if container:
            checks.append(
                {
                    "name": f"container is {container}",
                    "ok": container in (facts["format"] or ""),
                    "actual": facts["format"],
                }
            )
        if audio is not None:
            checks.append(
                {"name": "audio stream", "ok": bool(facts["audio"]) == audio, "actual": bool(facts["audio"])}
            )
        if video is not None:
            checks.append(
                {"name": "video stream", "ok": bool(facts["video"]) == video, "actual": bool(facts["video"])}
            )
        if seconds is not None and facts["video"]:
            low, high = seconds
            checks.append(
                {
                    "name": f"duration {low}–{high} s",
                    "ok": low <= facts["duration"] <= high,
                    "actual": round(facts["duration"], 2),
                }
            )
        if size and facts["video"]:
            checks.append(
                {
                    "name": f"{size[0]}×{size[1]}",
                    "ok": (facts["video"][0]["width"], facts["video"][0]["height"]) == tuple(size),
                    "actual": [facts["video"][0]["width"], facts["video"][0]["height"]],
                }
            )
        return checks

    return check


def audio_check(codec=None, channels=None, audio_only=True):
    def check(produced: list[Path]) -> list[dict]:
        if len(produced) != 1:
            return [{"name": "one artifact", "ok": False, "actual": len(produced)}]
        facts = validate.media(produced[0])
        if facts is None:
            return [{"name": "readable audio", "ok": False, "actual": "unreadable"}]
        checks = [
            {"name": "audio stream", "ok": bool(facts["audio"]), "actual": bool(facts["audio"])},
        ]
        if codec:
            checks.append(
                {
                    "name": f"codec is {codec}",
                    "ok": (facts["audio"][0]["codec"] or "") == codec if facts["audio"] else False,
                    "actual": facts["audio"][0]["codec"] if facts["audio"] else None,
                }
            )
        if channels is not None and facts["audio"]:
            checks.append(
                {
                    "name": f"{channels} channel(s)",
                    "ok": facts["audio"][0]["channels"] == channels,
                    "actual": facts["audio"][0]["channels"],
                }
            )
        if audio_only:
            checks.append(
                {
                    "name": "no video stream",
                    "ok": not facts["video"],
                    "actual": bool(facts["video"]),
                }
            )
        return checks

    return check


def pdf_check(pages=None, at_least_one_page=True):
    def check(produced: list[Path]) -> list[dict]:
        if len(produced) != 1:
            return [{"name": "one artifact", "ok": False, "actual": len(produced)}]
        artifact = produced[0]
        checks = [
            {
                "name": "magic is PDF",
                "ok": artifact.read_bytes()[:5] == b"%PDF-",
                "actual": artifact.read_bytes()[:5].decode("latin-1"),
            }
        ]
        facts = validate.pdf(artifact)
        if facts is None:
            checks.append({"name": "pdfinfo reads it", "ok": False, "actual": "unreadable"})
            return checks
        if pages is not None:
            checks.append({"name": f"{pages} pages", "ok": facts["pages"] == pages, "actual": facts["pages"]})
        if at_least_one_page:
            checks.append({"name": "has pages", "ok": facts["pages"] >= 1, "actual": facts["pages"]})
        return checks

    return check


def pdf_rotation_check(page: int, degrees: int):
    """A page's /Rotate value, read back with pdfinfo."""

    def check(produced: list[Path]) -> list[dict]:
        if len(produced) != 1:
            return [{"name": "one artifact", "ok": False, "actual": len(produced)}]
        result = validate.run_tool(
            ["pdfinfo", "-f", str(page), "-l", str(page), str(produced[0])]
        )
        found = re.search(rf"Page\s+{page} rot:\s+(-?\d+)", result.stdout) if result else None
        actual = int(found.group(1)) if found else None
        return [
            {
                "name": f"page {page} is rotated {degrees}°",
                "ok": actual == degrees,
                "actual": actual,
            }
        ]

    return check


def text_check(min_words=1, expect=None):
    def check(produced: list[Path]) -> list[dict]:
        if len(produced) != 1:
            return [{"name": "one artifact", "ok": False, "actual": len(produced)}]
        text = produced[0].read_text(errors="replace")
        words = [w for w in text.split() if any(c.isalpha() for c in w)]
        checks = [
            {"name": "non-empty text", "ok": len(words) >= min_words, "actual": len(words)}
        ]
        if expect:
            checks.append(
                {"name": f"contains {expect!r}", "ok": expect in text, "actual": text[:120]}
            )
        return checks

    return check


def package_check(suffix_hint: str):
    """A zip container written by the in-process writers (PPTX, XLSX, DOCX, EPUB)."""

    def check(produced: list[Path]) -> list[dict]:
        if len(produced) != 1:
            return [{"name": "one artifact", "ok": False, "actual": len(produced)}]
        artifact = produced[0]
        head = artifact.read_bytes()[:2]
        return [
            {"name": "one artifact", "ok": True, "actual": artifact.name},
            {"name": f"{suffix_hint} package", "ok": head == b"PK", "actual": head.decode("latin-1")},
        ]

    return check


def docx_check(headings=0, lists=0, images=0):
    def check(produced: list[Path]) -> list[dict]:
        if len(produced) != 1:
            return [{"name": "one artifact", "ok": False, "actual": len(produced)}]
        facts = validate.docx(produced[0])
        if facts is None:
            return [{"name": "readable docx", "ok": False, "actual": "unreadable"}]
        return [
            {"name": "opens as a package", "ok": True, "actual": True},
            {
                "name": "text inside",
                "ok": len(facts["text"].split()) > 0,
                "actual": len(facts["text"].split()),
            },
            {
                "name": f"at least {headings} heading(s)",
                "ok": facts["heading1"] + facts["heading2"] >= headings,
                "actual": facts["heading1"] + facts["heading2"],
            },
            {
                "name": f"at least {lists} list paragraph(s)",
                "ok": facts["lists"] >= lists,
                "actual": facts["lists"],
            },
            {
                "name": f"at least {images} image(s)",
                "ok": facts["images"] >= images,
                "actual": facts["images"],
            },
        ]

    return check


def markdown_check(min_headings=5, min_words=40):
    """A Markdown file with a real document shape, not one flattened line."""
    def check(produced: list[Path]) -> list[dict]:
        if len(produced) != 1:
            return [{"name": "one artifact", "ok": False, "actual": len(produced)}]
        text = produced[0].read_text(errors="replace")
        lines = text.splitlines()
        headings = [line for line in lines if line.startswith("#")]
        words = [word for word in text.split() if any(ch.isalpha() for ch in word)]
        source = (
            validate.pdf_text(str(CURRENT_INPUTS[0])) if CURRENT_INPUTS else ""
        )
        coverage = validate.text_coverage(source, text) if source else None
        return [
            {"name": "non-empty markdown", "ok": len(words) >= min_words, "actual": len(words)},
            {
                "name": f"at least {min_headings} headings",
                "ok": len(headings) >= min_headings,
                "actual": [heading[:60] for heading in headings[:8]],
            },
            {
                "name": "no wall of text",
                "ok": max((len(line) for line in lines), default=0) < 1200,
                "actual": max((len(line) for line in lines), default=0),
            },
            {
                "name": "source words survive",
                "ok": coverage is None or coverage >= 0.98,
                "actual": round(coverage, 4) if coverage is not None else None,
            },
        ]

    return check


def odt_check(min_headings=1, min_tables=0, min_images=0):
    def check(produced: list[Path]) -> list[dict]:
        if len(produced) != 1:
            return [{"name": "one artifact", "ok": False, "actual": len(produced)}]
        facts = validate.odt(produced[0])
        if facts is None:
            return [{"name": "readable odt", "ok": False, "actual": "unreadable"}]
        source = validate.pdf_text(str(CURRENT_INPUTS[0])) if CURRENT_INPUTS else ""
        coverage = validate.text_coverage(source, facts["text"]) if source else None
        return [
            {
                "name": "odf text mimetype",
                "ok": facts["mimetype"] == "application/vnd.oasis.opendocument.text",
                "actual": facts["mimetype"],
            },
            {
                "name": f"at least {min_headings} heading(s)",
                "ok": facts["headings"] >= min_headings,
                "actual": facts["headings"],
            },
            {
                "name": f"at least {min_tables} table(s)",
                "ok": facts["tables"] >= min_tables,
                "actual": facts["tables"],
            },
            {
                "name": f"at least {min_images} image(s)",
                "ok": facts["images"] >= min_images,
                "actual": facts["images"],
            },
            {
                "name": "source words survive",
                "ok": coverage is None or coverage >= 0.98,
                "actual": round(coverage, 4) if coverage is not None else None,
            },
        ]

    return check


def pptx_check(min_slides=1, min_pictures=0, expect=None):
    def check(produced: list[Path]) -> list[dict]:
        if len(produced) != 1:
            return [{"name": "one artifact", "ok": False, "actual": len(produced)}]
        facts = validate.pptx(produced[0])
        if facts is None:
            return [{"name": "readable pptx", "ok": False, "actual": "unreadable"}]
        return [
            {
                "name": f"at least {min_slides} slide(s)",
                "ok": facts["slides"] >= min_slides,
                "actual": facts["slides"],
            },
            {
                "name": "text inside",
                "ok": facts["text_chars"] > 0,
                "actual": facts["text_chars"],
            },
            {
                "name": f"at least {min_pictures} picture(s)",
                "ok": facts["pictures"] >= min_pictures,
                "actual": facts["pictures"],
            },
            {
                "name": f"contains {expect!r}" if expect else "text expected",
                "ok": expect in facts["text"] if expect else True,
                "actual": facts["text"][:120],
            },
        ]

    return check


def xlsx_check(min_cells=40, max_longest=600, expect=()):
    def check(produced: list[Path]) -> list[dict]:
        if len(produced) != 1:
            return [{"name": "one artifact", "ok": False, "actual": len(produced)}]
        facts = validate.xlsx(produced[0])
        if facts is None:
            return [{"name": "readable xlsx", "ok": False, "actual": "unreadable"}]
        values = " ".join(facts["cells"].values())
        return [
            {
                "name": f"at least {min_cells} populated cells",
                "ok": facts["populated"] >= min_cells,
                "actual": facts["populated"],
            },
            {
                "name": "no blob cell",
                "ok": facts["longest"] <= max_longest,
                "actual": facts["longest"],
            },
            *[
                {
                    "name": f"contains {phrase!r}",
                    "ok": phrase in values,
                    "actual": values[:120],
                }
                for phrase in expect
            ],
        ]

    return check


def epub_check(min_chapters=1, expect=None):
    def check(produced: list[Path]) -> list[dict]:
        if len(produced) != 1:
            return [{"name": "one artifact", "ok": False, "actual": len(produced)}]
        facts = validate.epub(produced[0])
        if facts is None:
            return [{"name": "valid zip", "ok": False, "actual": "unreadable"}]
        plain = re.sub(r"<[^>]+>", " ", facts["text"])
        plain = " ".join(plain.split())
        source = validate.pdf_text(str(CURRENT_INPUTS[0])) if CURRENT_INPUTS else ""
        coverage = validate.text_coverage(source, plain) if source else None
        checks = [
            {
                "name": "mimetype first and stored",
                "ok": facts["mimetype_first"] and facts["mimetype_stored"],
                "actual": [facts["mimetype_first"], facts["mimetype_stored"]],
            },
            {
                "name": "mimetype says epub",
                "ok": "epub" in (facts["mimetype"] or ""),
                "actual": facts["mimetype"],
            },
            {
                "name": f"{min_chapters} chapter(s)",
                "ok": len(facts["chapters"]) >= min_chapters,
                "actual": len(facts["chapters"]),
            },
            {
                "name": "source words survive",
                "ok": coverage is None or coverage >= 0.98,
                "actual": round(coverage, 4) if coverage is not None else None,
            },
            {
                "name": "stylesheet is a linked manifest item",
                "ok": any(style.endswith("style.css") for style in facts["styles"])
                and 'href="style.css"' in facts["text"],
                "actual": facts["styles"],
            },
            {
                "name": "every image has explicit dimensions",
                "ok": all(
                    re.search(r'<img[^>]*\bwidth="\d+"[^>]*\bheight="\d+"', tag)
                    for tag in re.findall(r"<img\b[^>]*>", facts["text"])
                ) if "<img" in facts["text"] else True,
                "actual": len(re.findall(r"<img\b[^>]*>", facts["text"])),
            },
        ]
        if expect:
            checks.append(
                {
                    "name": f"contains {expect!r}",
                    "ok": expect in plain,
                    "actual": plain[:120],
                }
            )
        return checks

    return check


#: The inputs of the case being run. A pixel check compares the output with the very
#: file it was made from, and threading that through every check signature would make
#: the simple ones (`image`, `media`) carry an argument they never use.
CURRENT_INPUTS: list[Path] = []


def frame_pixel(path: Path, seconds: float, x: int, y: int, work: Path) -> tuple | None:
    """One pixel of one frame of a video, as RGB."""
    frame = work / f"frame-{seconds}-{x}-{y}.png"
    result = validate.run_tool(
        ["ffmpeg", "-hide_banner", "-nostdin", "-y", "-ss", str(seconds), "-i", str(path),
         "-frames:v", "1", str(frame)]
    )
    if result is None or result.returncode != 0 or not frame.exists():
        return None
    return validate.pixel(frame, x, y)


def redaction_check(inside=(420, 300), plain=(544, 360), start=0.5, end=2.0, fill=(228, 87, 46)):
    """A per-box time range: covered inside the range, untouched outside it.

    This is the acceptance the dialog's `Region n start/end` rows feed, measured on the
    engine's own output: a redaction that leaked outside its range, or one that missed
    the range entirely, fails here.
    """

    def check(produced: list[Path]) -> list[dict]:
        if len(produced) != 1:
            return [{"name": "one artifact", "ok": False, "actual": len(produced)}]
        source = next((p for p in CURRENT_INPUTS if p.suffix == produced[0].suffix), None)
        work = Path(tempfile.mkdtemp(prefix="convertme-frame-"))
        try:
            covered = frame_pixel(produced[0], (start + end) / 2, *inside, work)
            early = frame_pixel(produced[0], max(0.0, start - 0.3), *plain, work)
            late = frame_pixel(produced[0], end + 0.5, *plain, work)
            source_early = frame_pixel(source, max(0.0, start - 0.3), *plain, work) if source else None
            source_late = frame_pixel(source, end + 0.5, *plain, work) if source else None
        finally:
            shutil.rmtree(work, ignore_errors=True)
        if covered is None or early is None or late is None:
            return [{"name": "frames readable", "ok": False, "actual": [covered, early, late]}]
        matches_fill = all(abs(a - b) <= 24 for a, b in zip(covered, fill))
        distance = lambda a, b: max(abs(x - y) for x, y in zip(a, b)) if a and b else 999
        return [
            {"name": "the box is filled inside its range", "ok": matches_fill, "actual": covered},
            {
                "name": "the frame before the range matches the source",
                "ok": source_early is not None and distance(early, source_early) <= 24,
                "actual": early,
            },
            {
                "name": "the frame after the range matches the source",
                "ok": source_late is not None and distance(late, source_late) <= 24,
                "actual": late,
            },
            {
                "name": "the redaction did not bleed outside the box",
                "ok": distance(early, fill) > 60 and distance(late, fill) > 60,
                "actual": [early, late],
            },
        ]

    return check


def members_check(min_files=1):
    """A tool that publishes a folder: its members are the artifact."""

    def check(produced: list[Path]) -> list[dict]:
        return [
            {
                "name": f"at least {min_files} file(s) extracted",
                "ok": len(produced) >= min_files,
                "actual": len(produced),
            },
            {"name": "the names are intact", "ok": all(p.name for p in produced), "actual": True},
        ]

    return check


def archive_check(min_entries=1, expect_name=None):
    def check(produced: list[Path]) -> list[dict]:
        if len(produced) != 1:
            return [{"name": "one artifact", "ok": False, "actual": len(produced)}]
        facts = validate.archive(produced[0])
        if facts is None:
            return [{"name": "readable archive", "ok": False, "actual": "unreadable"}]
        checks = [
            {
                "name": f"{min_entries} entr(y/ies)",
                "ok": len(facts["names"]) >= min_entries,
                "actual": len(facts["names"]),
            }
        ]
        if expect_name:
            checks.append(
                {
                    "name": f"contains {expect_name}",
                    "ok": expect_name in facts["names"],
                    "actual": facts["names"][:6],
                }
            )
        return checks

    return check


# ---------------------------------------------------------------------- cases

CASES: list[Case] = [
    # Images: one source, every writable target in the category.
    convert("image-jpg-to-png", "images/photo-landscape-4000x3000.jpg", "PNG",
            image_check("PNG", (4000, 3000))),
    convert("image-jpg-to-webp", "images/photo-landscape-4000x3000.jpg", "WEBP",
            image_check("WEBP", (4000, 3000))),
    convert("image-jpg-to-tiff", "images/photo-landscape-4000x3000.jpg", "TIFF",
            image_check(None, (4000, 3000))),
    convert("image-png-to-png", "images/transparent-logo.png", "WEBP",
            image_check("WEBP", alpha=None)),
    convert("image-jpg-to-pdf", "images/photo-landscape-4000x3000.jpg", "PDF",
            pdf_check(pages=1)),
    tool_case("image-compress-default", "images/poster-portrait.png", "compress", "images",
              {"quality": 80, "scale": 100},
              image_check("PNG")),
    tool_case("image-crop-free", "images/photo-landscape-4000x3000.jpg", "crop", "images",
              {"x": 0.1, "y": 0.1, "w": 0.5, "h": 0.5},
              image_check("JPG", (2000, 1500))),
    tool_case("image-annotate-rect", "images/photo-landscape-4000x3000.jpg", "annotate", "images",
              {"shapes": [{"kind": "rect", "x": 0.1, "y": 0.1, "w": 0.3, "h": 0.2,
                           "color": "#ff0000"}]},
              image_check(None, (4000, 3000))),
    tool_case("image-background-default", "images/photo-portrait-3000x4000.jpg",
              "add-background", "images", {}, image_check(None)),
    # Video
    convert("video-mkv-to-mp4", "video/clip-landscape-720p-10s.mkv", "MP4",
            media_check("mp4", (9.0, 11.0), size=(1280, 720))),
    convert("video-mp4-to-gif", "video/clip-landscape-720p-10s.mp4", "GIF",
            media_check("gif", None, audio=False)),
    convert("video-mp4-to-mp3", "video/clip-landscape-720p-10s.mp4", "MP3",
            audio_check("mp3", audio_only=True)),
    tool_case("video-trim", "video/clip-landscape-720p-10s.mp4", "trim", "video",
              {"start": 2, "end": 5}, media_check(None, (2.5, 5.5), size=(1280, 720))),
    tool_case("video-speed-double", "video/clip-landscape-720p-10s.mp4", "speed", "video",
              {"factor": 2}, media_check(None, (4.5, 5.5))),
    tool_case("video-crop-free", "video/clip-landscape-720p-10s.mp4", "crop", "video",
              {"x": 0.25, "y": 0.25, "w": 0.5, "h": 0.5},
              media_check(None, None, audio=True, video=True, size=(640, 360))),
    tool_case(
        "video-redact-range",
        "video/clip-landscape-720p-10s.mp4",
        "redact",
        "video",
        {
            "boxes": [{"x": 0.25, "y": 0.3, "w": 0.35, "h": 0.4, "start": 0.5, "end": 2.0}],
            "style": "solid",
            "color": "#e4572e",
        },
        redaction_check(),
    ),
    tool_case("video-frames", "video/clip-landscape-720p-10s.mp4", "snapshot", "video",
              {"times": [1.0]}, image_check("PNG", (1280, 720))),
    # Audio
    convert("audio-mp3-to-wav", "audio/speech.mp3", "WAV", audio_check(None)),
    convert("audio-mp3-to-flac", "audio/speech.mp3", "FLAC", audio_check("flac")),
    convert("audio-mp3-to-ogg", "audio/speech.mp3", "OGG", audio_check("vorbis")),
    tool_case("audio-normalize", "audio/speech.mp3", "normalize", "audio",
              {"lufs": -16}, audio_check(None)),
    tool_case("audio-bleep", "audio/speech.mp3", "bleep", "audio",
              {"ranges": [{"start": 2.0, "end": 3.0}]}, audio_check(None)),
    tool_case("audio-channels-gain", "audio/tone-stereo-30s.wav", "channels", "audio",
              {"channels": 2, "left_volume": 50, "right_volume": 100},
              audio_check(None, channels=2)),
    tool_case("audio-visualize-portrait", "audio/speech.mp3", "visualize", "audio",
              {"shape": "9:16", "style": "bars"},
              media_check("mp4", (13.9, 14.3), size=(720, 1280))),
    # PDF
    convert("pdf-to-png", "pdf/report-5-pages.pdf", "PNG", image_check("PNG")),
    convert("pdf-to-txt", "pdf/report-5-pages.pdf", "TXT", text_check(20)),
    convert("pdf-to-md", "pdf/report-5-pages.pdf", "MD", text_check(20)),
    convert("pdf-to-docx", "pdf/Jake_s_Resume__Anonymous_.pdf", "DOCX",
           docx_check(headings=1)),
    convert("pdf-to-md-resume", "pdf/Jake_s_Resume__Anonymous_.pdf", "MD",
           markdown_check(7)),
    convert("pdf-to-epub", "pdf/Jake_s_Resume__Anonymous_.pdf", "EPUB",
           epub_check(1, "Relevant Coursework")),
    convert("pdf-to-odt", "pdf/Jake_s_Resume__Anonymous_.pdf", "ODT",
           odt_check(5, 1, 4)),
    convert("pdf-to-xlsx", "pdf/Jake_s_Resume__Anonymous_.pdf", "XLSX",
           xlsx_check(40, 600, ("State University", "Sep. 2017", "Data Structures"))),
    convert("pdf-to-pptx-resume", "pdf/Jake_s_Resume__Anonymous_.pdf", "PPTX",
           pptx_check(1, 4, "Relevant Coursework")),
    convert("pdf-to-pptx", "pdf/report-5-pages.pdf", "PPTX", package_check("PPTX")),
    tool_case(
        "pdf-organize-rotate",
        "pdf/report-5-pages.pdf",
        "organize",
        "pdf",
        {"order": [1, 2, 3, 4, 5], "rotate": {"1": 90}},
        pdf_rotation_check(1, 90),
    ),
    tool_case("pdf-organize", "pdf/report-5-pages.pdf", "organize", "pdf",
              {"order": [1, 2, 3, 4, 5]}, pdf_check(pages=5)),
    # Merge needs the whole selection: the CLI takes every input as one job.
    Case(
        "pdf-merge-order",
        ["pdf/report-5-pages.pdf", "pdf/scanned-3-pages.pdf"],
        [
            "tool",
            "{input}",
            "--tool",
            "merge",
            "--category",
            "pdf",
            "--params",
            "{}",
            "--overwrite",
            "replace",
        ],
        pdf_check(pages=8),
    ),
    # Documents, slides, spreadsheets (LibreOffice-dependent ones skip without it)
    convert("doc-md-to-pdf", "documents/report.md", "PDF", pdf_check(at_least_one_page=True)),
    convert("doc-md-to-html", "documents/report.md", "HTML", text_check(20)),
    convert("doc-md-to-epub", "documents/report.md", "EPUB", epub_check(1)),
    convert("slides-pptx-to-pdf", "slides/deck-8-slides.pptx", "PDF", pdf_check(pages=8)),
    convert("sheet-xlsx-to-csv", "spreadsheets/sheet.xlsx", "CSV", text_check(2)),
    convert("epub-to-md", "epub/report.epub", "MD", text_check(20)),
    # Subtitles and archives
    convert("srt-to-vtt", "subtitles/cues.srt", "VTT", text_check(4, "WEBVTT")),
    convert("vtt-to-srt", "subtitles/cues.vtt", "SRT", text_check(4)),
    convert("archive-zip-to-tar", "archives/project.zip", "TAR",
            archive_check(3)),
    tool_case("archive-extract", "archives/project.zip", "extract", "archives", {},
              members_check(3)),
]


# ------------------------------------------------------------------- matrix

# One fixture per source format. A pair whose source has no fixture is skipped with
# that reason rather than quietly missing from the audit.
MATRIX_FIXTURES = {
    "JPG": "images/photo-landscape-4000x3000.jpg",
    "PNG": "images/poster-portrait.png",
    "WEBP": "images/photo.webp",
    "TIFF": "images/photo.tiff",
    "AVIF": "images/photo.avif",
    "BMP": "images/photo.bmp",
    "GIF": "video/animated.gif",
    "MP4": "video/clip-landscape-720p-10s.mp4",
    "MOV": "video/clip-landscape-720p-10s.mov",
    "MKV": "video/clip-landscape-720p-10s.mkv",
    "WEBM": "video/clip-landscape-720p-10s.webm",
    "AVI": "video/clip-landscape-720p-10s.avi",
    "WMV": "video/clip-landscape-720p-10s.wmv",
    "MP3": "audio/speech.mp3",
    "M4A": "audio/speech.m4a",
    "WAV": "audio/speech.wav",
    "FLAC": "audio/speech.flac",
    "OGG": "audio/speech.ogg",
    "OPUS": "audio/speech.opus",
    "AIFF": "audio/speech.aiff",
    "WMA": "audio/speech.wma",
    "PDF": "pdf/report-5-pages.pdf",
    "DOCX": "documents/report.docx",
    "ODT": "documents/report.odt",
    "MD": "documents/report.md",
    "TXT": "documents/notes.txt",
    "HTML": "documents/report.html",
    "PPTX": "slides/deck-8-slides.pptx",
    "XLSX": "spreadsheets/sheet.xlsx",
    "ODS": "spreadsheets/sheet.ods",
    "CSV": "spreadsheets/sales-1000-rows.csv",
    "TSV": "spreadsheets/sheet.tsv",
    "EPUB": "epub/report.epub",
    "SRT": "subtitles/cues.srt",
    "VTT": "subtitles/cues.vtt",
    "ZIP": "archives/project.zip",
    "TAR": "archives/bundle.tar",
    "GZIP": "archives/bundle.gz",
}

IMAGE_TARGETS = {"PNG", "JPG", "WEBP", "HEIC", "TIFF", "AVIF", "BMP", "ICO", "JXL", "TGA"}
VIDEO_TARGETS = {"MP4", "MOV", "MKV", "WEBM", "AVI", "WMV", "GIF"}
AUDIO_TARGETS = {"MP3", "M4A", "WAV", "FLAC", "OGG", "OPUS", "AIFF", "WMA"}
TEXT_TARGETS = {"TXT", "MD", "HTML", "CSV", "TSV", "SRT", "VTT"}
PACKAGE_TARGETS = {"DOCX", "ODT", "XLSX", "ODS", "PPTX"}
ARCHIVE_TARGETS = {"ZIP", "TAR", "GZIP", "7Z"}


def check_for_target(target: str):
    """The facts a target family must show, whatever the source was."""
    if target in IMAGE_TARGETS:
        return image_check(None)
    if target in VIDEO_TARGETS:
        return media_check(None, None, audio=None, video=True)
    if target in AUDIO_TARGETS:
        return audio_check(None, audio_only=False)
    if target == "PDF":
        return pdf_check(at_least_one_page=True)
    if target == "EPUB":
        return epub_check(1)
    if target in TEXT_TARGETS:
        return text_check(1)
    if target in PACKAGE_TARGETS:
        return package_check(target)
    if target in ARCHIVE_TARGETS:
        return archive_check(1)
    return package_check(target)


def matrix_cases() -> list[Case]:
    """One case per conversion the product offers.

    The pair list is derived from the registry snapshot (a target is offered when it is
    writable and listed in the source's category — the same rule `Registry::wheel_targets`
    applies), so a reached pair that the audit does not cover would mean the snapshot
    and the product disagree.
    """
    snapshot = json.loads(
        (REPO / "crates/convertme-core/tests/formats.snapshot.json").read_text()
    )
    formats = {item["id"]: item for item in snapshot["formats"]}
    cases: list[Case] = []
    for source, fixture in sorted(MATRIX_FIXTURES.items()):
        entry = formats.get(source)
        if entry is None:
            continue
        category = entry["category"]
        for target, target_entry in sorted(formats.items()):
            if not target_entry["write"] or category not in target_entry["wheels"]:
                continue
            if target == source:
                continue
            cases.append(
                convert(
                    f"matrix-{source.lower()}-to-{target.lower()}",
                    fixture,
                    target,
                    check_for_target(target),
                )
            )
    return cases


def hint_for(stderr: str) -> str | None:
    """A skip reason when the failure is this machine's, not the product's.

    Four things are the environment talking rather than the conversion being wrong: a
    tool that is not installed, a bundled tool that went missing, an ImageMagick build
    without a codec for the source (`no decode delegate`, `unable to load module`), and
    the planner refusing a pair whose engine is absent ("CSV → JPG is not supported").
    Every other non-zero exit is a failure and must be fixed.
    """
    lowered = stderr.lower()
    for marker in (
        "not installed",
        "reinstall convertme",
        "no decode delegate",
        "unable to load module",
        "is not supported",
        # A property of the input, not a conversion bug: a silent clip has no audio
        # to put in an MP3, and GZIP cannot hold a folder.
        "the input has no audio track",
        "holds one file",
    ):
        if marker in lowered:
            return stderr.strip().splitlines()[0][:160]
    return None


def run_case(case: Case, fixtures: Path, out_dir: Path, binary: Path, release: bool,
             libreoffice_missing: bool) -> dict:
    work = Path(tempfile.mkdtemp(prefix="convertme-audit-"))
    record = {"name": case.name, "status": "passed", "checks": [], "outputs": []}
    try:
        sources = [case.source] if isinstance(case.source, str) else list(case.source)
        fixture_manifest = json.loads((fixtures / "input-hashes.json").read_text(encoding="utf-8-sig")) if release else None
        copies: list[Path] = []
        for relative in sources:
            source = fixtures / relative
            if not source.is_file():
                record["status"] = "failed" if release else "skipped"
                record["reason"] = f"missing fixture {relative}"
                return record
            if fixture_manifest is not None:
                digest = hashlib.sha256(source.read_bytes()).hexdigest()
                if fixture_manifest.get(relative) != digest:
                    record["status"] = "failed"
                    record["reason"] = f"fixture digest mismatch: {relative}"
                    return record
            copy = work / source.name
            shutil.copy2(source, copy)
            copies.append(copy)
        input_hashes = {str(copy): hashlib.sha256(copy.read_bytes()).hexdigest() for copy in copies}
        record["input_hashes"] = {
            relative: hashlib.sha256(copy.read_bytes()).hexdigest()
            for relative, copy in zip(sources, copies)
        }
        inputs = [copy.as_posix() for copy in copies]
        argv = [str(binary)]
        for part in case.argv:
            if part == "{input}":
                argv.extend(inputs)
            else:
                argv.append(part)
        env = os.environ.copy()
        if release and os.name == "nt":
            system = Path(env["SystemRoot"])
            env["PATH"] = os.pathsep.join((str(system / "System32"), str(system)))
            env.pop("CONVERTME_TOOLS_DIR", None)
        completed = subprocess.run(argv, capture_output=True, text=True, encoding="utf-8",
                                   errors="replace", check=False, env=env)
        if completed.returncode != 0:
            reason = None if release else hint_for(completed.stderr)
            error_line = completed.stderr.strip()
            optional_error = error_line in (
                "missing_tool: LibreOffice is not installed",
                "failed: missing_tool: LibreOffice is not installed",
            )
            if release and libreoffice_missing and optional_error:
                reason = error_line
                record["skip_approval"] = {
                    "approved": True,
                    "reason": "optional_libreoffice_unavailable",
                    "evidence": "application probe reported soffice missing",
                }
            record["status"] = "skipped" if reason else "failed"
            record["reason"] = reason or completed.stderr.strip()[:400]
            record["checks"] = [{"name": "cli exit", "ok": False, "actual": completed.returncode}]
            return record

        input_names = {copy.name for copy in copies}
        produced = sorted(
            path
            for path in work.iterdir()
            if path.name not in input_names and not path.name.endswith(".log")
        )
        record["outputs"] = [p.name for p in produced]
        if case.check is None:
            record["checks"] = [{"name": "artifact written", "ok": bool(produced), "actual": produced}]
        else:
            # Directories (save-pages, extract) are inspected through their members.
            flattened: list[Path] = []
            for path in produced:
                if path.is_dir():
                    flattened.extend(sorted(p for p in path.rglob("*") if p.is_file()))
                else:
                    flattened.append(path)
            CURRENT_INPUTS[:] = copies
            record["checks"] = case.check(flattened)
            CURRENT_INPUTS.clear()
        for copy in copies:
            record["checks"].append({
                "name": f"input unchanged: {copy.name}",
                "ok": copy.is_file() and hashlib.sha256(copy.read_bytes()).hexdigest() == input_hashes[str(copy)],
            })
        failures = [check for check in record["checks"] if not check.get("ok")]
        record["status"] = "failed" if failures else "passed"
        # Keep the artifact of a *failed* case (that is when someone needs to look at
        # it). A passing matrix run produces gigabytes of video otherwise, and the
        # first version of this filled the disk mid-audit.
        if failures:
            kept = out_dir / case.name
            kept.mkdir(parents=True, exist_ok=True)
            for path in produced[:2]:
                target = kept / path.name
                if path.is_dir():
                    shutil.copytree(path, target, dirs_exist_ok=True)
                else:
                    shutil.copy2(path, target)
        return record
    finally:
        shutil.rmtree(work, ignore_errors=True)


def all_cases(with_matrix: bool) -> list[Case]:
    cases = list(CASES)
    if with_matrix:
        known = {case.name for case in cases}
        cases.extend(case for case in matrix_cases() if case.name not in known)
    return cases

def export_fixtures(fixtures: Path, destination: Path) -> None:
    paths = set(MATRIX_FIXTURES.values())
    paths.update(FULL_UIA_FIXTURES)
    for case in CASES:
        paths.update([case.source] if isinstance(case.source, str) else case.source)
    root = fixtures.resolve()
    entries = {}
    folded = set()
    for relative in sorted(paths):
        parts = Path(relative).parts
        if not parts or Path(relative).is_absolute() or any(part in (".", "..") for part in parts):
            raise ValueError(f"unsafe fixture path: {relative}")
        if relative.casefold() in folded:
            raise ValueError(f"case-colliding fixture path: {relative}")
        folded.add(relative.casefold())
        if any(re.search(r'[<>:"\\|?*\x00-\x1f]', part) or
               part.endswith((" ", ".")) for part in parts):
            raise ValueError(f"fixture path cannot be exported to Windows: {relative}")
        source = (root / relative).resolve()
        if not source.is_relative_to(root) or not source.is_file():
            raise ValueError(f"missing or invalid fixture: {relative}")
        entries[relative] = hashlib.sha256(source.read_bytes()).hexdigest()
    destination.parent.mkdir(parents=True, exist_ok=True)
    with zipfile.ZipFile(destination, "w", zipfile.ZIP_DEFLATED) as archive:
        for relative in sorted(entries):
            archive.write(root / relative, relative)
        archive.writestr("input-hashes.json", json.dumps(entries, indent=2) + "\n")



def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--fixtures", default=str(DEFAULT_FIXTURES))
    parser.add_argument("--binary", default=str(BIN), help="installed or extracted application executable")
    parser.add_argument("--release", action="store_true", help="fail missing fixtures, tools and unapproved skips")
    parser.add_argument("--export-fixtures", metavar="ZIP", help="export referenced inputs and their hashes without running the app")
    parser.add_argument("--out", help="report directory (default artifacts/quality-audit/<stamp>)")
    parser.add_argument("--only", help="only cases whose name contains this")
    parser.add_argument("--list", action="store_true", help="list the cases and exit")
    parser.add_argument(
        "--no-matrix",
        action="store_true",
        help="only the curated cases, not the full conversion matrix",
    )
    args = parser.parse_args(argv)
    stamp = datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%SZ")
    out_dir = Path(args.out) if args.out else REPO / "artifacts/quality-audit" / stamp
    if not args.export_fixtures and not args.list:
        out_dir.mkdir(parents=True, exist_ok=True)

    def startup_failure(message: str, **details) -> int:
        report = {
            "stamp": stamp, "status": "failed", "stage": "startup",
            "reason": message, "binary": args.binary, "fixtures": args.fixtures,
            "release": args.release, "selectors": {"only": args.only, "no_matrix": args.no_matrix},
            **details,
        }
        (out_dir / "report.json").write_text(json.dumps(report, indent=2) + "\n", encoding="utf-8")
        print(f"quality-audit: {message}", file=sys.stderr)
        return 2
    fixtures = Path(args.fixtures)
    if args.export_fixtures:
        try:
            export_fixtures(fixtures, Path(args.export_fixtures))
        except (OSError, ValueError) as error:
            print(f"quality-audit: {error}", file=sys.stderr)
            return 2
        return 0

    selected_cases = all_cases(not args.no_matrix)
    if args.list:
        for case in selected_cases:
            print(case.name)
        return 0
    binary = Path(args.binary).resolve()
    if not binary.is_file():
        return startup_failure(f"binary not found ({binary})")
    try:
        validate.configure_tools(binary, args.release)
    except (OSError, ValueError, json.JSONDecodeError) as error:
        return startup_failure(str(error))
    if not fixtures.is_dir():
        return startup_failure(f"fixture root not found: {fixtures}")
    if args.release:
        try:
            fixture_manifest = json.loads((fixtures / "input-hashes.json").read_text(encoding="utf-8-sig"))
            if not isinstance(fixture_manifest, dict):
                raise ValueError("fixture manifest must be an object")
        except (OSError, ValueError) as error:
            return startup_failure(f"invalid fixture manifest: {error}")

    selected = [case for case in selected_cases if not args.only or args.only in case.name]
    app_env = os.environ.copy()
    if args.release and os.name == "nt":
        system = Path(app_env["SystemRoot"])
        app_env["PATH"] = os.pathsep.join((str(system / "System32"), str(system)))
        app_env.pop("CONVERTME_TOOLS_DIR", None)
    try:
        probe = subprocess.run([str(binary), "probe"], capture_output=True, text=True,
                               encoding="utf-8", errors="replace", env=app_env, check=False)
    except (OSError, subprocess.SubprocessError) as error:
        return startup_failure(f"application probe could not start: {error}")
    if args.release:
        required = ("ffmpeg", "ffprobe", "magick", "gs", "qpdf", "pdftoppm",
                    "pdftotext", "pandoc", "7zz", "tesseract", "convertme-pdf")
        if os.name == "nt":
            required += ("ffplay",)
        lines = {line.split()[0]: line for line in probe.stdout.splitlines() if line.split()}
        missing = [key for key in required if key not in lines or
                   len(lines[key].split()) < 2 or lines[key].split()[1] != "ok"]
        if probe.returncode or missing:
            return startup_failure(f"required app tools unavailable: {', '.join(missing)}",
                                   app_probe={"exit_code": probe.returncode, "stdout": probe.stdout, "stderr": probe.stderr})
        missing_validators = [key for key, value in validate.validator_dependencies().items() if not value]
        if missing_validators:
            return startup_failure(f"missing validators: {', '.join(missing_validators)}",
                                   app_probe={"exit_code": probe.returncode, "stdout": probe.stdout, "stderr": probe.stderr})
    office_lines = [line.split() for line in probe.stdout.splitlines()
                    if line.split() and line.split()[0] == "soffice"]
    office_state = (office_lines[0][1] if probe.returncode == 0 and len(office_lines) == 1
                    and len(office_lines[0]) > 1 else None)
    libreoffice_missing = office_state == "missing"
    results = []
    for case in selected:
        record = run_case(case, fixtures, out_dir, binary, args.release, libreoffice_missing)
        results.append(record)
        mark = {"passed": "ok  ", "skipped": "skip", "failed": "FAIL"}[record["status"]]
        detail = ""
        if record["status"] == "failed":
            failed = [c["name"] for c in record["checks"] if not c.get("ok")]
            detail = " — " + "; ".join(failed[:3])
        elif record["status"] == "skipped":
            detail = f" — {record.get('reason', '')[:100]}"
        print(f"{mark} {case.name}{detail}")

    counts = {status: sum(1 for r in results if r["status"] == status) for status in
              ("passed", "failed", "skipped")}
    approved_skips = sum(1 for r in results if r["status"] == "skipped" and
                         r.get("skip_approval", {}).get("approved") is True and
                         r["skip_approval"].get("reason") == "optional_libreoffice_unavailable")
    unapproved_skips = counts["skipped"] - approved_skips
    counts["selected"] = len(selected)
    counts["recorded"] = len(results)
    counts["approved_skips"] = approved_skips
    counts["unapproved_skips"] = unapproved_skips
    accounting_ok = (counts["recorded"] == counts["selected"] ==
                     counts["passed"] + counts["failed"] + counts["skipped"] and
                     [r["name"] for r in results] == [case.name for case in selected])
    report = {
        "stamp": stamp,
        "binary": str(binary),
        "binary_sha256": hashlib.sha256(binary.read_bytes()).hexdigest(),
        "release": args.release,
        "selectors": {"only": args.only, "no_matrix": args.no_matrix},
        "selected_cases": [case.name for case in selected],
        "fixtures_manifest_sha256": (hashlib.sha256((fixtures / "input-hashes.json").read_bytes()).hexdigest()
                                     if (fixtures / "input-hashes.json").is_file() else None),
        "validator_dependencies": validate.validator_dependencies(),
        "app_probe": {"exit_code": probe.returncode, "stdout": probe.stdout, "stderr": probe.stderr},
        "dependency_roles": {
            "application": "Capabilities reported by the selected package executable in app_probe.",
            "validators": "Independent output-inspection executables in validator_dependencies; these do not determine the application's PATH.",
            "optional": {"soffice": {"probe_status": office_state,
                                      "probe": "app_probe"}},
        },
        "fixtures": str(fixtures),
        "counts": counts,
        "accounting_ok": accounting_ok,
        "cases": results,
    }
    (out_dir / "report.json").write_text(json.dumps(report, indent=2, default=str) + "\n", encoding="utf-8")
    print(f"\n{counts['passed']} passed, {counts['failed']} failed, {counts['skipped']} skipped")
    print(f"report: {out_dir / 'report.json'}")
    return 1 if counts["failed"] or not accounting_ok or (args.release and unapproved_skips) else 0


if __name__ == "__main__":
    raise SystemExit(main())
