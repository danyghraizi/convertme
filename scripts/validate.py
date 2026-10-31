#!/usr/bin/env python3
"""Facts about produced files, shared by the harnesses.

Every check in `app-e2e.py` and `quality-audit.py` reads a produced artifact through
this module, so "the output is a valid PNG of 2000×1500" means the same thing in both
layers. Facts come from the real decoders — ImageMagick identify, `ffprobe`,
`pdfinfo`, the DOCX zip — never from the extension of the file name.

Nothing here raises for a missing tool: a probe returns `None` and the caller decides
whether that is a skip or a failure. A validator that crashes reports less than one
that says "unavailable".
"""

from __future__ import annotations

import json
import re
import shutil
import subprocess
import zipfile
from pathlib import Path


def _run(argv: list[str], timeout: float = 60.0) -> subprocess.CompletedProcess | None:
    try:
        return subprocess.run(
            argv, capture_output=True, text=True, timeout=timeout, check=False
        )
    except (OSError, subprocess.SubprocessError):
        return None


def _have(tool: str) -> bool:
    return shutil.which(tool) is not None


# ------------------------------------------------------------------- images

# ImageMagick's own format specifiers: %m is the magick, %w/%h the pixel size.
IDENTIFY = "%m|%w|%h|%[colorspace]|%[type]|%z"


def image(path: str | Path) -> dict | None:
    """Format, size, colour space, type, depth and alpha, from ImageMagick."""
    if not _have("magick"):
        return None
    out = _run(["magick", "identify", "-format", IDENTIFY, str(path)])
    if out is None or out.returncode != 0 or "|" not in out.stdout:
        return None
    fields = out.stdout.split("|")
    if len(fields) < 6:
        return None
    magick, width, height, colorspace, kind, depth = fields[:6]
    return {
        "magick": magick,  # e.g. "PNG"
        "width": int(width),
        "height": int(height),
        "colorspace": colorspace,
        "type": kind,
        "depth": depth,
        "format": magick,
        "alpha": "a" in kind.lower() or "alpha" in colorspace.lower(),
        "bytes": Path(path).stat().st_size,
    }


def pixel(path: str | Path, x: int, y: int) -> tuple[int, int, int] | None:
    """One pixel, as 8-bit RGB."""
    if not _have("magick"):
        return None
    out = _run(
        ["magick", str(path), "-depth", "8", "-format", f"%[pixel:p{{{x},{y}}}]", "info:"]
    )
    if out is None or out.returncode != 0:
        return None
    numbers = re.findall(r"\d+", out.stdout)
    if len(numbers) < 3:
        return None
    return tuple(int(n) for n in numbers[:3])  # type: ignore[return-value]


def mean_luma(path: str | Path) -> float | None:
    """Average brightness, 0..1."""
    if not _have("magick"):
        return None
    out = _run(["magick", str(path), "-colorspace", "gray", "-format", "%[fx:mean]", "info:"])
    if out is None or out.returncode != 0:
        return None
    try:
        return float(out.stdout.strip())
    except ValueError:
        return None


def variance(path: str | Path) -> float | None:
    """Pixel variance: how much local detail the picture carries."""
    if not _have("magick"):
        return None
    out = _run(
        [
            "magick",
            str(path),
            "-colorspace",
            "gray",
            "-format",
            "%[fx:standard_deviation]",
            "info:",
        ]
    )
    if out is None or out.returncode != 0:
        return None
    try:
        return float(out.stdout.strip())
    except ValueError:
        return None


def corner(path: str | Path, which: str = "top-left") -> tuple[int, int, int] | None:
    info = image(path)
    if not info:
        return None
    x = 2 if "left" in which else info["width"] - 3
    y = 2 if "top" in which else info["height"] - 3
    return pixel(path, x, y)


# --------------------------------------------------------------------- media

def media(path: str | Path) -> dict | None:
    """Duration, container and per-stream facts, from ffprobe."""
    if not _have("ffprobe"):
        return None
    out = _run(
        [
            "ffprobe",
            "-v",
            "error",
            "-show_entries",
            "format=duration,format_name,size:stream=codec_type,codec_name,channels,"
            "sample_rate,width,height,nb_frames:stream_tags=rotate",
            "-of",
            "json",
            str(path),
        ]
    )
    if out is None or out.returncode != 0:
        return None
    try:
        value = json.loads(out.stdout)
    except json.JSONDecodeError:
        return None
    streams = [
        {
            "type": stream.get("codec_type"),
            "codec": stream.get("codec_name"),
            "channels": stream.get("channels"),
            "sample_rate": stream.get("sample_rate"),
            "width": stream.get("width"),
            "height": stream.get("height"),
            "frames": stream.get("nb_frames"),
        }
        for stream in value.get("streams", [])
    ]
    duration = value.get("format", {}).get("duration")
    return {
        "format": value.get("format", {}).get("format_name"),
        "duration": float(duration) if duration not in (None, "N/A") else 0.0,
        "streams": streams,
        "audio": [s for s in streams if s["type"] == "audio"],
        "video": [s for s in streams if s["type"] == "video"],
        "bytes": Path(path).stat().st_size,
    }


def duration(path: str | Path) -> float | None:
    facts = media(path)
    return facts["duration"] if facts else None


def audio_loudness(path: str | Path) -> dict | None:
    """Integrated loudness and true peak, measured by ffmpeg's own loudnorm filter."""
    if not _have("ffmpeg"):
        return None
    out = _run(
        [
            "ffmpeg",
            "-hide_banner",
            "-nostats",
            "-i",
            str(path),
            "-af",
            "loudnorm=I=-16:TP=-1.5:LRA=11:print_format=json",
            "-f",
            "null",
            "-",
        ]
    )
    if out is None or out.returncode != 0:
        return None
    text = out.stderr
    start = text.rfind("{")
    end = text.rfind("}")
    if start < 0 or end <= start:
        return None
    try:
        block = json.loads(text[start : end + 1])
    except json.JSONDecodeError:
        return None

    def number(key: str) -> float | None:
        try:
            return float(block[key])
        except (KeyError, TypeError, ValueError):
            return None

    return {
        "input_i": number("input_i"),
        "input_tp": number("input_tp"),
        "input_lra": number("input_lra"),
    }


def rms(path: str | Path, start: float = 0.0, length: float | None = None) -> float | None:
    """RMS level in dBFS over a window: enough to prove silence or a tone."""
    if not _have("ffmpeg"):
        return None
    argv = ["ffmpeg", "-hide_banner", "-nostats"]
    if start > 0:
        argv += ["-ss", f"{start}"]
    argv += ["-i", str(path)]
    if length is not None:
        argv += ["-t", f"{length}"]
    argv += ["-af", "astats=metadata=1:reset=0", "-f", "null", "-"]
    out = _run(argv)
    if out is None or out.returncode != 0:
        return None
    matches = re.findall(r"RMS level dB:\s*(-?\d+\.?\d*)", out.stderr)
    if not matches:
        return None
    try:
        return float(matches[-1])
    except ValueError:
        return None


def _pcm(path: str | Path, start: float, length: float, rate: int = 16000):
    """Mono 16-bit samples of a window, decoded through ffmpeg into a temp WAV.

    A WAV file rather than raw bytes on stdout: `_run` captures text, and a binary
    stream is not text.
    """
    if not _have("ffmpeg"):
        return None, rate
    import array
    import os
    import tempfile
    import wave

    handle = tempfile.NamedTemporaryFile(suffix=".wav", delete=False)
    handle.close()
    try:
        out = _run(
            [
                "ffmpeg",
                "-hide_banner",
                "-loglevel",
                "error",
                "-y",
                "-ss",
                f"{start}",
                "-t",
                f"{length}",
                "-i",
                str(path),
                "-ac",
                "1",
                "-ar",
                str(rate),
                "-f",
                "wav",
                handle.name,
            ]
        )
        if out is None or out.returncode != 0:
            return None, rate
        with wave.open(handle.name, "rb") as decoded:
            if decoded.getsampwidth() != 2:
                return None, rate
            frames = decoded.readframes(decoded.getnframes())
        samples = array.array("h")
        samples.frombytes(frames[: len(frames) // 2 * 2])
        return samples, rate
    finally:
        os.unlink(handle.name)


def tone_strength(
    path: str | Path, start: float = 0.0, length: float = 0.4, freq: float = 1000.0
) -> float | None:
    """How much stronger `freq` is than its neighbouring bands in a window.

    A bleep is a pure tone: its band towers over 0.7x and 1.4x the frequency, which
    speech in the same window does not do. The earlier implementation asked ffmpeg's
    `aspectralstats` for a "peak", a filter some builds do not carry, so it returned
    nothing at all; this needs only a PCM decode and a Goertzel pass.
    """
    samples, rate = _pcm(path, start, length)
    if not samples:
        return None
    import math

    def power(target: float) -> float:
        if target >= rate / 2:
            return 0.0
        coeff = 2.0 * math.cos(2.0 * math.pi * target / rate)
        s1 = 0.0
        s2 = 0.0
        for value in samples:
            s0 = value + coeff * s1 - s2
            s2 = s1
            s1 = s0
        return s1 * s1 + s2 * s2 - coeff * s1 * s2

    peak = power(freq)
    flank = max(power(freq * 0.7), power(freq * 1.4), 1e-9)
    return peak / flank


def pdf(path: str | Path) -> dict | None:
    """Page count and per-page size/rotation, from pdfinfo."""
    if not _have("pdfinfo"):
        return None
    out = _run(["pdfinfo", str(path)])
    if out is None or out.returncode != 0:
        return None
    pages = 0
    sizes: list[tuple[float, float]] = []
    for line in out.stdout.splitlines():
        key, _, value = line.partition(":")
        if key.strip() == "Pages":
            pages = int(value.strip() or 0)
        elif key.strip().startswith("Page") and "size" in key:
            numbers = re.findall(r"[\d.]+", value)
            if len(numbers) >= 2:
                sizes.append((float(numbers[0]), float(numbers[1])))
    return {"pages": pages, "sizes": sizes, "bytes": Path(path).stat().st_size}


def pdf_text(path: str | Path, page: int | None = None) -> str | None:
    if not _have("pdftotext"):
        return None
    argv = ["pdftotext"]
    if page is not None:
        argv += ["-f", str(page), "-l", str(page)]
    argv += [str(path), "-"]
    out = _run(argv)
    if out is None or out.returncode != 0:
        return None
    return out.stdout


def pdf_page_rotation(path: str | Path, page: int = 1) -> int | None:
    if not _have("pdfinfo") or not _have("qpdf"):
        return None
    out = _run(["qpdf", "--show-pages", "--json", str(path)])
    if out is None or out.returncode != 0:
        return None
    try:
        value = json.loads(out.stdout)
    except json.JSONDecodeError:
        return None
    try:
        entry = value["pages"][page - 1]
    except (KeyError, IndexError):
        return None
    return entry.get("rotate") or 0


# ---------------------------------------------------------------------- docx

DOCX_STYLES = re.compile(r'w:pStyle w:val="([^"]+)"')
DOCX_NUMPR = re.compile(r"<w:numPr>")
DOCX_BREAK = re.compile(r'w:type="page"')
DOCX_DRAWING = re.compile(r"<w:drawing>")


def docx(path: str | Path) -> dict | None:
    """Paragraphs, styles, list items, images and page breaks, straight from the XML.

    Deliberately dependency-free: python-docx is not installed everywhere the
    harness runs, and the facts that matter (a heading is a `w:pStyle`, a list item
    is a `w:numPr`) are one XPath away in the package itself.
    """
    path = Path(path)
    try:
        with zipfile.ZipFile(path) as package:
            names = package.namelist()
            document = package.read("word/document.xml").decode("utf-8", "replace")
            styles = (
                package.read("word/styles.xml").decode("utf-8", "replace")
                if "word/styles.xml" in names
                else ""
            )
    except (OSError, KeyError, zipfile.BadZipFile):
        return None
    paragraphs = re.findall(r"<w:p[ >].*?</w:p>", document, re.S)
    texts = [
        "".join(re.findall(r"<w:t[^>]*>(.*?)</w:t>", block, re.S)) for block in paragraphs
    ]
    styles_used = DOCX_STYLES.findall(document)
    return {
        "paragraphs": len(paragraphs),
        "text": "\n".join(texts),
        "styles": styles_used,
        "heading1": sum(1 for s in styles_used if s in ("Heading1", "Heading 1")),
        "heading2": sum(1 for s in styles_used if s in ("Heading2", "Heading 2")),
        "heading3": sum(1 for s in styles_used if s in ("Heading3", "Heading 3")),
        "lists": sum(1 for s in styles_used if "ListParagraph" in s or "List" in s),
        "numpr": len(DOCX_NUMPR.findall(document)),
        "images": len(DOCX_DRAWING.findall(document)),
        "page_breaks": len(DOCX_BREAK.findall(document)),
        "has_styles": bool(styles),
        "media": [n for n in names if n.startswith("word/media/")],
        "bytes": path.stat().st_size,
    }


def text_coverage(source_text: str, produced_text: str) -> float | None:
    """Share of the source's alphabetic words that appear in the output."""
    words = re.findall(r"[A-Za-z][A-Za-z'-]{2,}", source_text or "")
    if not words:
        return None
    haystack = (produced_text or "").lower()
    missing = [word for word in words if word.lower() not in haystack]
    return 1.0 - len(missing) / len(words)


# --------------------------------------------------------------------- epub

def epub(path: str | Path) -> dict | None:
    """Container sanity: the mimetype must be first and stored, per EPUB 3."""
    path = Path(path)
    try:
        with zipfile.ZipFile(path) as package:
            infos = package.infolist()
            if not infos:
                return None
            first = infos[0]
            names = package.namelist()
            text = ""
            for name in names:
                if name.endswith((".xhtml", ".html", ".htm")):
                    text += package.read(name).decode("utf-8", "replace")
    except (OSError, zipfile.BadZipFile):
        return None
    return {
        "zip": True,
        "mimetype_first": first.filename == "mimetype",
        "mimetype_stored": first.compress_type == zipfile.ZIP_STORED,
        "mimetype": _safe_read(path, "mimetype"),
        "entries": names,
        "chapters": [n for n in names if n.endswith((".xhtml", ".html"))],
        "text": text,
        "bytes": path.stat().st_size,
    }


def _safe_read(path: Path, member: str) -> str:
    try:
        with zipfile.ZipFile(path) as package:
            return package.read(member).decode("utf-8", "replace").strip()
    except (OSError, KeyError, zipfile.BadZipFile):
        return ""


# ------------------------------------------------------------------ archives

def archive(path: str | Path) -> dict | None:
    """Entries in an archive, whatever the container."""
    path = Path(path)
    if zipfile.is_zipfile(path):
        try:
            with zipfile.ZipFile(path) as package:
                return {"kind": "zip", "names": package.namelist()}
        except zipfile.BadZipFile:
            return None
    if _have("7z"):
        out = _run(["7z", "l", "-slt", str(path)])
        if out is None or out.returncode != 0:
            return None
        names = [
            line.split("=", 1)[1]
            for line in out.stdout.splitlines()
            if line.startswith("Path = ")
        ]
        # The first Path is the archive itself.
        return {"kind": "7z", "names": names[1:]}
    return None


# ------------------------------------------------------------------ dispatch

PROBES = {
    "image": image,
    "media": media,
    "pdf": pdf,
    "docx": docx,
    "epub": epub,
    "archive": archive,
}


def probe(kind: str, path: str | Path) -> dict | None:
    fn = PROBES.get(kind)
    return fn(path) if fn else None
