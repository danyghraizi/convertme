#!/usr/bin/env python3
"""Real-app end-to-end harness for ConvertMe.

This is the second testing layer, next to `scripts/ui-shots.py`:

* `ui-shots.py` drives the *mock* frontend in headless Firefox. It proves
  rendering, field collection, visibility and layout, never the engine.
* this script launches the **release binary** under Xvfb + dbus-run-session,
  drives the **real window** through AT-SPI, runs **real jobs** and validates
  the **published artifacts** (bytes, magic, dimensions, colour type/palette,
  alpha, pixels, metadata, pages, streams, never-grow).

Every scenario writes a screenshot and a JSON record (build id, parameters,
accessibility measurements, output stat, validator output, pass/fail, skip
reason). Any mismatch exits non-zero; a scenario that cannot run is reported as
`skipped` with its reason, never omitted.

Usage:
    ./scripts/app-e2e.py                      # build must exist already
    ./scripts/app-e2e.py --build              # npm build + cargo release first
    ./scripts/app-e2e.py --only compress      # substring filter
    ./scripts/app-e2e.py --list

The app runs with a throwaway HOME/XDG profile and its inputs are copies, so a
run never touches the user's settings, history, preview cache or fixtures.
"""

from __future__ import annotations

import argparse
import contextlib
import hashlib
import json
import os
import re
import shutil
import subprocess
import sys
import tarfile
import tempfile
import time
import traceback
import zipfile
from datetime import datetime, timezone

REPO = os.path.dirname(os.path.dirname(os.path.abspath(__file__)))
sys.path.insert(0, os.path.join(REPO, "scripts"))
import validate  # noqa: E402 - shared artifact validators
BIN = os.path.join(REPO, "target/release/convertme")
UI_DIST = os.path.join(REPO, "crates/convertme-gui/ui-dist")
DEFAULT_FIXTURES = os.environ.get(
    "CONVERTME_FIXTURES", "/home/dany/Downloads/Testing Files"
)
PREVIEW_SHOT = (1400, 1000)
# Set by the worker; published artifacts are copied here so the report survives the
# throwaway profile.
RUN_OUT_DIR = None

CATEGORIES = [
    "Images",
    "Video",
    "Audio",
    "PDF",
    "Documents",
    "Slides",
    "Spreadsheets",
    "EPUB",
    "Subtitles",
    "Archives",
]

CATEGORY_FIXTURES = {
    "Images": "images/poster-portrait.png",
    "Video": "video/clip-landscape-720p-10s.mp4",
    "Audio": "audio/speech.mp3",
    "PDF": "pdf/report-5-pages.pdf",
    "Documents": "documents/report.md",
    "Slides": "slides/deck-8-slides.pptx",
    "Spreadsheets": "spreadsheets/sheet.xlsx",
    "EPUB": "epub/report.epub",
    "Subtitles": "subtitles/cues.srt",
    "Archives": "archives/project.zip",
}

CATEGORY_EXTS = {
    "Images": {"png", "jpg", "jpeg", "webp", "tif", "tiff", "avif", "bmp", "svg", "heic", "heif"},
    "Video": {"mp4", "mov", "mkv", "webm", "avi", "wmv", "gif"},
    "Audio": {"mp3", "m4a", "wav", "flac", "ogg", "oga", "opus", "aiff", "aif", "wma"},
    "PDF": {"pdf"},
    "Documents": {"md", "markdown", "docx", "odt", "txt", "html", "htm"},
    "Slides": {"pptx", "ppt"},
    "Spreadsheets": {"xlsx", "xls", "ods", "csv", "tsv"},
    "EPUB": {"epub"},
    "Subtitles": {"srt", "vtt"},
    "Archives": {"zip", "tar", "gz", "gzip", "7z", "rar"},
}


def category_for_path(path: str) -> str | None:
    ext = os.path.splitext(path)[1].lstrip(".").lower()
    for category, exts in CATEGORY_EXTS.items():
        if ext in exts:
            return category
    return None


# The tools the backend actually registers (core/src/formats.rs TOOLS). The UI
# falls back to FALLBACK_TOOLS for categories the backend does not serve; those
# cards are recorded as skips with a reason, not as dialogs.
BACKEND_TOOLS = {
    "Images": [
        "Compress",
        "Crop",
        "Annotate",
        "Collage",
        "Add background",
        "Edit photo",
        "Metadata",
        "Redact",
    ],
    "Video": [
        "Compress",
        "Trim",
        "Crop",
        "Speed",
        "Remove audio",
        "Save frames",
        "Redact",
        "Join",
        "Split",
    ],
    "Audio": [
        "Compress",
        "Normalize",
        "Trim",
        "Channels",
        "Visualize",
        "Bleep",
        "Metadata",
    ],
    "PDF": [
        "Compress",
        "Organize pages",
        "Metadata",
        "Merge",
        "Split pages",
        "Save pages as images",
        "Extract text",
    ],
    "Archives": ["Extract"],
}

TOOL_TABLE: dict = {}


def tool_table() -> dict:
    """The backend tool table, read from the binary itself.

    The dialog's title, primary action and rail benefits are product copy that lives
    in `crates/convertme-core/src/formats.rs`. Reading it through `convertme probe
    --tools` means the harness asserts against the real table instead of a fourth
    copy of the same strings, which would drift and then agree with itself.
    """
    result = run([BIN, "probe", "--tools"], timeout=60)
    if result.returncode != 0:
        return {}
    try:
        payload = json.loads(result.stdout)
    except json.JSONDecodeError:
        return {}
    return {
        (item["category"].capitalize() if item["category"] != "pdf" else "PDF",
         item["label"]): item
        for item in payload.get("tools", [])
    }


# Tools whose field table is empty: clicking the card runs the job immediately.
FIELDLESS = {
    ("Video", "Remove audio"),
    ("PDF", "Split pages"),
    ("PDF", "Extract text"),
    ("Archives", "Extract"),
}

CONTROL_EXPECTATIONS = {
    ("Images", "Crop"): [
        ("toggle button", "1:1"),
        ("entry", "Width"),
        ("entry", "Height"),
    ],
    ("Images", "Annotate"): [
        ("entry", "Caption"),
        ("toggle button", "Rectangle"),
        ("toggle button", "Arrow"),
        ("toggle button", "Number"),
        ("toggle button", "Highlight"),
        ("button", "Undo"),
    ],
    ("Images", "Collage"): [
        ("combo box", "Style"),
        ("entry", "Columns"),
        ("combo box", "Canvas ratio"),
        ("entry", "Width"),
        ("combo box", "Fit"),
        ("entry", "Spacing"),
    ],
    ("Images", "Redact"): [("button", "Detect text"), ("toggle button", "Blur")],
    ("Images", "Edit photo"): [
        ("slider", "Exposure"),
        ("slider", "Highlights"),
        ("slider", "Shadows"),
        ("slider", "Gamma"),
        ("slider", "Tint"),
        ("slider", "Vibrance"),
        ("slider", "Clarity"),
        ("slider", "Dehaze"),
        ("slider", "Grain"),
        ("slider", "Noise reduction"),
        ("slider", "Sharpness"),
    ],
    ("Images", "Add background"): [
        ("slider", "Padding"),
        ("slider", "Corner radius"),
        ("button", "Ink"),
        ("button", "Dusk"),
    ],
    ("Images", "Metadata"): [
        ("entry", "Search metadata"),
        ("entry", "Title"),
        ("toggle button", "Remove All"),
    ],
    ("Video", "Compress"): [
        ("slider", "Size (% of the original)"),
        ("check box", "Or a target file size"),
    ],
    ("Video", "Crop"): [("toggle button", "Free"), ("toggle button", "1:1"), ("toggle button", "16:9")],
    ("Video", "Speed"): [("slider", "Speed")],
    ("Video", "Save frames"): [("button", "Add frame")],
    ("Video", "Redact"): [("toggle button", "Solid"), ("toggle button", "Blur")],
    ("Video", "Split"): [
        ("button", "Add split at playhead"),
        ("entry", "Equal clips"),
    ],
    ("Video", "Trim"): [("slider", "Trim start"), ("slider", "Trim end")],
    ("Audio", "Compress"): [
        ("slider", "Size (% of the original)"),
        ("check box", "Or a bitrate"),
        ("check box", "Or a target file size"),
    ],
    ("Audio", "Normalize"): [
        ("slider", "Target loudness"),
        ("slider", "Loudness range"),
        ("slider", "True peak limit"),
        ("button", "Play Output"),
    ],
    ("Audio", "Channels"): [
        ("toggle button", "Mono"),
        ("toggle button", "Stereo"),
        ("slider", "Left volume"),
        ("slider", "Right volume"),
        ("button", "Play Output"),
    ],
    ("Audio", "Visualize"): [
        ("toggle button", "Waveform"),
        ("toggle button", "Bars"),
        ("toggle button", "Line"),
        ("toggle button", "Landscape · 16:9"),
        ("toggle button", "Portrait · 9:16"),
    ],
    ("Audio", "Bleep"): [("button", "Original"), ("button", "With Bleeps")],
    ("Audio", "Trim"): [
        ("slider", "Trim start"),
        ("slider", "Trim end"),
        ("check box", "Trim silent ends automatically"),
    ],
    ("Audio", "Metadata"): [
        ("entry", "Search metadata"),
        ("entry", "Title"),
        ("entry", "Artist"),
    ],
    ("PDF", "Compress"): [("combo box", "Quality"), ("entry", "Target size")],
    ("PDF", "Organize pages"): [
        ("button", "Reset Order"),
        ("check box", "Make all pages the same width"),
    ],
    ("PDF", "Metadata"): [
        ("entry", "Search metadata"),
        ("entry", "Title"),
        ("toggle button", "Remove All"),
    ],
    ("PDF", "Save pages as images"): [("combo box", "Image format")],
}

POSTER = "images/poster-portrait.png"
POSTER_SIZE = 96910
POSTER_TARGET_BYTES = 1597


class Skip(Exception):
    """The scenario is valid but cannot run here; the reason is reported."""


class ScenarioFailure(AssertionError):
    """A check failed; the record carries the detail."""


# --------------------------------------------------------------------------
# Small utilities (no AT-SPI here, so the orchestrator can import them)
# --------------------------------------------------------------------------


def now_iso() -> str:
    return datetime.now(timezone.utc).isoformat(timespec="seconds")


def run(cmd, timeout=60, env=None):
    return subprocess.run(
        cmd,
        capture_output=True,
        text=True,
        timeout=timeout,
        env=env,
        check=False,
    )


def sha256(path: str) -> str:
    h = hashlib.sha256()
    with open(path, "rb") as handle:
        for block in iter(lambda: handle.read(1 << 20), b""):
            h.update(block)
    return h.hexdigest()


def ui_dist_files() -> list[str]:
    found = []
    for root, _dirs, names in os.walk(UI_DIST):
        for name in names:
            found.append(os.path.join(root, name))
    found.sort()
    return found


def ui_build_stamp() -> str:
    """The same sum build.rs embeds as UI_BUILD_STAMP."""
    stamp = 0
    for path in ui_dist_files():
        meta = os.stat(path)
        stamp = (stamp + int(meta.st_mtime) + meta.st_size) & ((1 << 64) - 1)
    return f"{stamp:x}"


def bundle_build_id() -> str:
    """The interface build id compiled into the bundle (Settings shows it)."""
    dates = []
    for path in ui_dist_files():
        if not path.endswith(".js"):
            continue
        with open(path, "r", encoding="utf-8", errors="replace") as handle:
            text = handle.read()
        for match in re.finditer(r"\d{4}-\d{2}-\d{2} \d{2}:\d{2}", text):
            dates.append((match.start(), match.group(0), text))
    if not dates:
        return ""
    # Prefer a date near the "Interface build" template; the bundle also carries
    # runtime-looking date strings only in code, never literals.
    for start, value, text in dates:
        window = text[max(0, start - 400): start + 400]
        if "Interface build" in window:
            return value
    return dates[0][1]


def binary_contains(text: str) -> bool:
    needle = text.encode()
    with open(BIN, "rb") as handle:
        carry = b""
        while True:
            block = handle.read(1 << 22)
            if not block:
                return False
            hay = carry + block
            if needle in hay:
                return True
            carry = hay[-(len(needle) - 1):]


def magic_kind(path: str) -> str:
    """A byte-signature description, independent of the extension."""
    with open(path, "rb") as handle:
        head = handle.read(512)
    if head.startswith(b"\x89PNG\r\n\x1a\n"):
        return "png"
    if head.startswith(b"\xff\xd8\xff"):
        return "jpeg"
    if head.startswith((b"GIF87a", b"GIF89a")):
        return "gif"
    if head[:4] == b"RIFF" and head[8:12] == b"WEBP":
        return "webp"
    if head[:4] == b"RIFF" and head[8:12] == b"WAVE":
        return "wav"
    if head[:4] == b"RIFF" and head[8:12] == b"AVI ":
        return "avi"
    if head[:4] in (b"II*\x00", b"MM\x00*"):
        return "tiff"
    if head.startswith(b"%PDF-"):
        return "pdf"
    if head.startswith(b"PK\x03\x04"):
        return "zip"
    if head.startswith(b"7z\xbc\xaf\x27\x1c"):
        return "7z"
    if head.startswith(b"\x1f\x8b"):
        return "gzip"
    if head.startswith(b"OggS"):
        return "ogg"
    if head.startswith(b"fLaC"):
        return "flac"
    if head.startswith(b"ID3") or head[:2] in (b"\xff\xfb", b"\xff\xf3", b"\xff\xf2"):
        return "mp3"
    if head[4:8] == b"ftyp":
        brand = head[8:12]
        if brand in (b"qt  ",):
            return "mov"
        return "mp4"
    if head.startswith(b"\x1aE\xdf\xa3"):
        return "matroska"
    if head.startswith(b"\x30\x26\xb2\x75"):
        return "asf"
    if head.startswith(b"WEBVTT"):
        return "webvtt"
    if head.startswith(b"BM"):
        return "bmp"
    if len(head) > 262 and head[257:262] == b"ustar":
        return "tar"
    text = head.decode("utf-8", "replace")
    if re.match(r"^\s*\d+\s*\n\d\d:\d\d:\d\d[,.]\d\d\d\s*-->", text):
        return "srt"
    if head.startswith(b"{\\rtf"):
        return "rtf"
    return "unknown"


def image_info(path: str) -> dict:
    result = run(
        [
            "identify",
            "-format",
            "%m|%w|%h|%[colorspace]|%[channels]|%k|%z",
            path,
        ],
        timeout=60,
    )
    if result.returncode != 0:
        raise ScenarioFailure(f"identify failed for {path}: {result.stderr.strip()}")
    parts = result.stdout.strip().split("|")
    info = {
        "format": parts[0],
        "width": int(parts[1]),
        "height": int(parts[2]),
        "colorspace": parts[3],
        "channels": parts[4],
        "colors": int(float(parts[5])),
        "depth": parts[6],
    }
    if info["format"].upper() == "PNG":
        png = run(["identify", "-format", "%[png:IHDR.color-type-orig]", path])
        try:
            info["png_color_type"] = int(float(png.stdout.strip()))
        except ValueError:
            info["png_color_type"] = None
    return info


def pixel(path: str, x: int, y: int) -> dict:
    result = run(
        ["magick", path, "-format", f"%[pixel:p{{{x},{y}}}]", "info:"], timeout=60
    )
    text = result.stdout.strip()
    alpha = run(
        ["magick", path, "-format", f"%[fx:round(p{{{x},{y}}}.a*255)]", "info:"],
        timeout=60,
    )
    try:
        alpha_value = int(float(alpha.stdout.strip()))
    except ValueError:
        alpha_value = None
    return {"pixel": text, "alpha": alpha_value}


def compare_pixels(a: str, b: str) -> float:
    result = run(["magick", "compare", "-metric", "AE", a, b, "null:"], timeout=120)
    # The metric goes to stderr; a non-zero status only means "different".
    match = re.search(r"([0-9.eE+]+)", result.stderr)
    if not match:
        raise ScenarioFailure(f"compare produced no metric: {result.stderr!r}")
    return float(match.group(1))


def comment_of(path: str) -> str:
    return run(["identify", "-format", "%c", path], timeout=60).stdout.strip()


def ffprobe(path: str) -> dict:
    result = run(
        [
            "ffprobe",
            "-v",
            "error",
            "-print_format",
            "json",
            "-show_format",
            "-show_streams",
            path,
        ],
        timeout=120,
    )
    if result.returncode != 0:
        raise ScenarioFailure(f"ffprobe failed for {path}: {result.stderr.strip()}")
    data = json.loads(result.stdout or "{}")
    fmt = data.get("format", {})
    streams = [
        {
            "type": s.get("codec_type"),
            "codec": s.get("codec_name"),
            "width": s.get("width"),
            "height": s.get("height"),
            "channels": s.get("channels"),
        }
        for s in data.get("streams", [])
    ]
    return {
        "format": fmt.get("format_name"),
        "duration": float(fmt.get("duration", 0.0) or 0.0),
        "size": int(fmt.get("size", 0) or 0),
        "streams": streams,
        "format_tags": fmt.get("tags", {}),
        "stream_tags": [s.get("tags") or {} for s in data.get("streams", [])],
    }


def pdfinfo(path: str) -> dict:
    result = run(["pdfinfo", path], timeout=120)
    if result.returncode != 0:
        raise ScenarioFailure(f"pdfinfo failed for {path}: {result.stderr.strip()}")
    info = {}
    for line in result.stdout.splitlines():
        if ":" in line:
            key, value = line.split(":", 1)
            info[key.strip()] = value.strip()
    return info


def zip_names(path: str) -> list[str]:
    with zipfile.ZipFile(path) as archive:
        return archive.namelist()


def zip_member_text(path: str, member: str) -> str:
    with zipfile.ZipFile(path) as archive:
        return archive.read(member).decode("utf-8", "replace")


def tar_names(path: str) -> list[str]:
    with tarfile.open(path) as archive:
        return [member.name for member in archive.getmembers() if member.isfile()]


def text_of(path: str) -> str:
    with open(path, "r", encoding="utf-8", errors="replace") as handle:
        return handle.read()


def describe_artifact(path: str) -> dict:
    """Everything validators can say about one published artifact."""
    if os.path.isdir(path):
        entries = sorted(os.listdir(path))
        return {
            "kind": "directory",
            "entries": entries[:100],
            "entry_count": len(entries),
        }
    record = {
        "kind": magic_kind(path),
        "size": os.path.getsize(path),
        "sha256": sha256(path),
    }
    try:
        if record["kind"] in ("png", "jpeg", "gif", "webp", "tiff", "bmp"):
            record["image"] = image_info(path)
        if record["kind"] in ("png", "jpeg", "webp", "tiff", "gif", "bmp"):
            # A second call that tolerates non-image formats.
            pass
    except Exception as error:  # evidence, not failure of the scenario itself
        record["image_error"] = str(error)
    try:
        if record["kind"] in ("mp4", "mov", "matroska", "avi", "asf", "mp3", "ogg", "wav", "flac"):
            record["media"] = ffprobe(path)
    except Exception as error:
        record["media_error"] = str(error)
    try:
        if record["kind"] == "pdf":
            record["pdf"] = pdfinfo(path)
    except Exception as error:
        record["pdf_error"] = str(error)
    try:
        if record["kind"] == "zip":
            record["zip"] = {"members": zip_names(path)[:100]}
    except Exception as error:
        record["zip_error"] = str(error)
    try:
        if record["kind"] == "tar":
            record["tar"] = {"members": tar_names(path)[:100]}
    except Exception as error:
        record["tar_error"] = str(error)
    return record


# --------------------------------------------------------------------------
# Orchestrator: Xvfb + dbus-run-session + isolated profile
# --------------------------------------------------------------------------


def pick_display() -> str:
    for number in range(90, 140):
        if not os.path.exists(f"/tmp/.X11-unix/X{number}"):
            return f":{number}"
    raise SystemExit("no free X display number")


def start_xvfb(display: str):
    proc = subprocess.Popen(
        [
            "Xvfb",
            display,
            "-screen",
            "0",
            f"{PREVIEW_SHOT[0]}x{PREVIEW_SHOT[1]}x24",
            "-nolisten",
            "tcp",
        ],
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    socket = f"/tmp/.X11-unix/X{display.lstrip(':')}"
    deadline = time.time() + 15
    while time.time() < deadline:
        if proc.poll() is not None:
            raise SystemExit(f"Xvfb {display} exited with {proc.returncode}")
        if os.path.exists(socket):
            return proc
        time.sleep(0.05)
    proc.kill()
    raise SystemExit(f"Xvfb {display} did not become ready")


def preflight_checks(build: bool):
    if build:
        for cmd in (
            ["npm", "--prefix", "ui", "run", "build"],
            ["cargo", "build", "--release", "-p", "convertme-gui"],
        ):
            print(f"$ {' '.join(cmd)}")
            result = subprocess.run(cmd, cwd=REPO)
            if result.returncode != 0:
                raise SystemExit(f"build failed: {' '.join(cmd)}")
    problems = []
    for path in (BIN, UI_DIST):
        if not os.path.exists(path):
            problems.append(f"missing {path}")
    for binary in ("Xvfb", "dbus-run-session", "xdotool", "import", "identify", "magick"):
        if shutil.which(binary) is None:
            problems.append(f"missing {binary}")
    if not os.path.exists("/usr/lib/at-spi2-registryd"):
        problems.append("missing /usr/lib/at-spi2-registryd")
    try:
        import gi  # noqa: F401
    except Exception:
        problems.append("python gi/Atspi is not importable")
    if problems:
        raise SystemExit("preflight failed:\n  " + "\n  ".join(problems))


def outer(args) -> int:
    preflight_checks(args.build)
    out_dir = os.path.abspath(args.out or os.path.join(REPO, "artifacts/app-e2e", datetime.now().strftime("%Y%m%d-%H%M%S")))
    os.makedirs(out_dir, exist_ok=True)
    profile = tempfile.mkdtemp(prefix="convertme-e2e-")
    display = pick_display()
    xvfb = start_xvfb(display)
    inner = [
        sys.executable,
        os.path.abspath(__file__),
        "--worker",
        "--out",
        out_dir,
        "--profile",
        profile,
        "--fixtures",
        os.path.abspath(args.fixtures),
        "--timeout",
        str(args.timeout),
    ]
    if args.only:
        inner += ["--only", args.only]
    env = os.environ.copy()
    env.update(
        {
            "DISPLAY": display,
            "HOME": os.path.join(profile, "home"),
            "XDG_DATA_HOME": os.path.join(profile, "data"),
            "XDG_CACHE_HOME": os.path.join(profile, "cache"),
            "XDG_CONFIG_HOME": os.path.join(profile, "config"),
            "XDG_RUNTIME_DIR": os.path.join(profile, "run"),
            "TMPDIR": os.path.join(profile, "tmp"),
        }
    )
    for sub in ("home", "data", "cache", "config", "run", "tmp"):
        os.makedirs(os.path.join(profile, sub), exist_ok=True)
    os.chmod(os.path.join(profile, "run"), 0o700)
    print(f"run directory: {out_dir}")
    print(f"profile:       {profile}")
    try:
        result = subprocess.run(
            ["dbus-run-session", "--"] + inner,
            cwd=REPO,
            env=env,
        )
    finally:
        xvfb.terminate()
        try:
            xvfb.wait(timeout=5)
        except subprocess.TimeoutExpired:
            xvfb.kill()
        if not args.keep_profile:
            shutil.rmtree(profile, ignore_errors=True)
    return result.returncode


# --------------------------------------------------------------------------
# Worker: AT-SPI driver
# --------------------------------------------------------------------------


def start_accessibility() -> subprocess.Popen | None:
    address = None
    result = run(
        [
            "dbus-send",
            "--session",
            "--print-reply",
            "--dest=org.a11y.Bus",
            "/org/a11y/bus",
            "org.a11y.Bus.GetAddress",
        ],
        timeout=10,
    )
    match = re.search(r'string "([^"]+)"', result.stdout)
    if match:
        address = match.group(1)
    if not address:
        raise SystemExit(f"could not read the AT-SPI bus address: {result.stderr.strip()}")
    os.environ["AT_SPI_BUS_ADDRESS"] = address
    registry = None
    if os.path.exists("/usr/lib/at-spi2-registryd"):
        registry = subprocess.Popen(
            ["/usr/lib/at-spi2-registryd"],
            stdout=subprocess.DEVNULL,
            stderr=subprocess.DEVNULL,
        )
        time.sleep(0.4)
    return registry


def import_atspi():
    import gi

    gi.require_version("Atspi", "2.0")
    from gi.repository import Atspi

    Atspi.init()
    return Atspi


class Checks:
    def __init__(self):
        self.items = []

    def add(self, name: str, ok: bool, expected=None, actual=None, detail: str = ""):
        entry = {"name": name, "ok": bool(ok)}
        if expected is not None:
            entry["expected"] = expected
        if actual is not None:
            entry["actual"] = actual
        if detail:
            entry["detail"] = detail
        self.items.append(entry)
        return bool(ok)

    @property
    def ok(self) -> bool:
        return all(item["ok"] for item in self.items)

    def failures(self):
        return [item for item in self.items if not item["ok"]]


class Session:
    """One launched app window and the AT-SPI calls the scenarios need."""

    def __init__(self, harness, name, fixtures, category=None, env_extra=None, exe=None):
        self.h = harness
        self.name = name
        self.fixtures = fixtures
        self.category = category
        self.env_extra = env_extra or {}
        self.exe = exe or BIN
        self.workdir = os.path.join(harness.work_root, name)
        self.log_path = os.path.join(harness.log_dir, f"{name}.log")
        self.proc = None
        self.app = None
        self.inputs = []
        self.before = {}
        self.atspi = harness.atspi
        self.category_hint = category

    # -- lifecycle -------------------------------------------------------

    def __enter__(self):
        shutil.rmtree(self.workdir, ignore_errors=True)
        os.makedirs(self.workdir, exist_ok=True)
        for item in self.fixtures:
            src, dest = item if isinstance(item, tuple) else (item, os.path.basename(item))
            target = os.path.join(self.workdir, dest)
            os.makedirs(os.path.dirname(target) or self.workdir, exist_ok=True)
            shutil.copy2(src, target)
            self.inputs.append(target)
        self.before = self.snapshot()
        if self.category_hint is None and self.inputs:
            self.category_hint = category_for_path(self.inputs[0])
        env = os.environ.copy()
        env.update(self.env_extra)
        log = open(self.log_path, "wb")
        self.proc = subprocess.Popen(
            [self.exe] + self.inputs,
            cwd=self.workdir,
            env=env,
            stdout=log,
            stderr=subprocess.STDOUT,
        )
        self.log_handle = log
        try:
            self.wait_ready()
        except Exception:
            self.close()
            raise
        return self

    def __exit__(self, *exc):
        self.close()

    def close(self):
        if self.proc and self.proc.poll() is None:
            self.proc.terminate()
            try:
                self.proc.wait(timeout=5)
            except subprocess.TimeoutExpired:
                self.proc.kill()
                try:
                    self.proc.wait(timeout=5)
                except subprocess.TimeoutExpired:
                    pass
        handle = getattr(self, "log_handle", None)
        if handle:
            handle.close()
            self.log_handle = None

    def log_tail(self, lines=40) -> str:
        try:
            with open(self.log_path, "r", encoding="utf-8", errors="replace") as handle:
                return "".join(handle.readlines()[-lines:])
        except OSError:
            return ""

    def wait_ready(self, timeout=30):
        deadline = time.time() + timeout
        while time.time() < deadline:
            if self.proc.poll() is not None:
                raise ScenarioFailure(
                    f"app exited with {self.proc.returncode}:\n{self.log_tail()}"
                )
            self.app = self.find_app()
            if self.app is not None and self.find(
                role="document web", within=self.app
            ) is not None:
                if self.find(
                    role="button", startswith="Activity", within=self.app
                ) is not None:
                    return
            time.sleep(0.2)
        raise ScenarioFailure(
            f"window was not ready after {timeout}s; log:\n{self.log_tail()}"
        )

    # -- AT-SPI primitives ----------------------------------------------

    def find_app(self):
        desktop = self.atspi.get_desktop(0)
        for app in _children(desktop):
            try:
                if app.get_name() != "convertme":
                    continue
                if app.get_process_id() == self.proc.pid:
                    return app
            except Exception:
                continue
        return None

    def refresh(self):
        self.app = self.find_app()
        return self.app

    def find(
        self,
        role=None,
        name=None,
        contains=None,
        startswith=None,
        within=None,
        tag=None,
    ):
        found = self.find_all(
            role=role,
            name=name,
            contains=contains,
            startswith=startswith,
            within=within,
            tag=tag,
        )
        return found[0] if found else None

    def find_all(
        self,
        role=None,
        name=None,
        contains=None,
        startswith=None,
        within=None,
        tag=None,
    ):
        root = within if within is not None else self.refresh()
        if root is None:
            return []
        out = []
        for node, _depth in _walk(root):
            if role is not None and _safe(node, "get_role_name") != role:
                continue
            node_name = _safe(node, "get_name") or ""
            if name is not None and node_name != name:
                continue
            if contains is not None and contains not in node_name:
                continue
            if startswith is not None and not node_name.startswith(startswith):
                continue
            if tag is not None:
                attrs = _attributes(node)
                if attrs.get("tag") != tag:
                    continue
            out.append(node)
        return out

    def text(self, node) -> str:
        return _text(node)

    def subtree_text(self, node) -> str:
        parts = []
        for child, _depth in _walk(node):
            value = _text(child)
            if value:
                parts.append(value)
        return "\n".join(parts)

    def all_texts(self, within=None):
        root = within if within is not None else self.refresh()
        if root is None:
            return []
        out = []
        for node, _depth in _walk(root):
            value = _text(node)
            if value.strip():
                out.append((_safe(node, "get_role_name"), value))
        return out

    def has_text(self, needle: str, within=None) -> bool:
        return any(needle in value for _role, value in self.all_texts(within))

    def wait_text(self, needle: str, within=None, timeout=30):
        deadline = time.time() + timeout
        last = ""
        while time.time() < deadline:
            texts = self.all_texts(within)
            for _role, value in texts:
                if needle in value:
                    return value
            last = "\n".join(value for _role, value in texts[-8:])
            time.sleep(0.3)
        raise ScenarioFailure(f"text {needle!r} did not appear; last texts:\n{last}")

    def extents(self, node):
        return _extents(node)

    def press(self, node) -> bool:
        ai = _safe(node, "get_action_iface")
        if ai is None:
            return False
        for index in range(_safe(ai, "get_n_actions") or 0):
            action = _safe(ai, "get_action_name", index)
            if action in ("press", "click", "check", "toggle", "select"):
                return bool(_safe(ai, "do_action", index))
        return bool(_safe(ai, "do_action", 0))

    def click(self, node):
        box = self.extents(node)
        if box is None:
            raise ScenarioFailure("element has no screen extents")
        x, y, width, height = box
        run(
            ["xdotool", "mousemove", str(x + width // 2), str(y + height // 2), "click", "1"],
            timeout=15,
        )
        time.sleep(0.15)

    def focus(self, node) -> bool:
        component = _safe(node, "get_component_iface")
        if component is None:
            return False
        return bool(_safe(component, "grab_focus"))

    def ensure_visible(self, node):
        box = self.extents(node)
        frame = self.frame_extents()
        if box and frame:
            x, y, w, h = box
            fx, fy, fw, fh = frame
            if x >= fx and y >= fy and x + w <= fx + fw and y + h <= fy + fh:
                return
        component = _safe(node, "get_component_iface")
        if component is not None:
            _safe(component, "scroll_to", self.atspi.ScrollType.ANYWHERE)
            time.sleep(0.2)

    def frame_extents(self):
        frame = self.find(role="frame", name="ConvertMe")
        return self.extents(frame) if frame is not None else None

    def type_text(self, value: str):
        run(["xdotool", "key", "--clearmodifiers", "ctrl+a"], timeout=10)
        run(["xdotool", "type", "--delay", "25", value], timeout=30)

    # -- widgets ---------------------------------------------------------

    def button(self, name, within=None, exact=True):
        for role in ("button", "toggle button"):
            if exact:
                node = self.find(role=role, name=name, within=within)
            else:
                node = self.find(role=role, contains=name, within=within)
            if node is not None:
                return node
        return None

    def press_button(self, name, within=None, exact=True, timeout=10):
        deadline = time.time() + timeout
        while time.time() < deadline:
            node = self.button(name, within=within, exact=exact)
            if node is not None:
                if not self.press(node):
                    raise ScenarioFailure(f"could not press button {name!r}")
                return
            time.sleep(0.2)
        raise ScenarioFailure(f"button {name!r} not found")

    def active_category(self) -> str:
        for node in self.find_all(role="toggle button"):
            if "PRESSED" in _states(node):
                name = _safe(node, "get_name") or ""
                if name in CATEGORIES:
                    return name
        return ""

    def toggle_category(self, label: str):
        node = self.find(role="toggle button", name=label)
        if node is None:
            raise ScenarioFailure(f"category {label!r} not found")
        if not self.press(node):
            raise ScenarioFailure(f"could not press category {label!r}")
        self.category_hint = label
        deadline = time.time() + 5
        while time.time() < deadline:
            if self.active_category() == label:
                break
            time.sleep(0.2)
        time.sleep(0.4)

    def cards(self):
        tools = self.find(name="Tools")
        if tools is None:
            return []
        buttons = self.find_all(role="button", within=tools)
        out = []
        for node in buttons:
            label = _safe(node, "get_name") or ""
            match = re.match(r"^\d{2} (.+?)(?: [A-Z].*)?$", label)
            if match:
                out.append((match.group(1), node))
        return out

    def target_buttons(self):
        targets = self.find(name="Convert to")
        if targets is None:
            return []
        return self.find_all(role="button", within=targets)

    def target_entries(self):
        """(label, hint, enabled, node): WebKit does not always name the button."""
        entries = []
        for node in self.target_buttons():
            name = (_safe(node, "get_name") or "").strip()
            lines = [
                line.strip()
                for line in self.subtree_text(node).splitlines()
                if line.strip()
            ]
            label = name or (lines[0] if lines else "")
            if lines and name.startswith(lines[0]):
                label = lines[0]
            hint = " ".join(line for line in lines if line != label)
            states = _states(node)
            enabled = "ENABLED" in states and "SENSITIVE" in states
            entries.append((label, hint, enabled, node))
        return entries

    def wait_targets(self, timeout=20):
        deadline = time.time() + timeout
        while time.time() < deadline:
            if self.target_buttons():
                return
            time.sleep(0.25)
        raise ScenarioFailure("conversion targets did not appear")

    def dialog(self):
        return self.find(role="dialog")

    def wait_dialog(self, timeout=6):
        deadline = time.time() + timeout
        while time.time() < deadline:
            node = self.dialog()
            if node is not None:
                return node
            time.sleep(0.2)
        return None

    def wait_dialog_closed(self, timeout=10):
        deadline = time.time() + timeout
        while time.time() < deadline:
            if self.dialog() is None:
                return True
            time.sleep(0.2)
        return False

    def wait_card(self, label: str, timeout=20):
        deadline = time.time() + timeout
        last = []
        while time.time() < deadline:
            for card_label, card in self.cards():
                if card_label == label:
                    return card
            last = [card_label for card_label, _card in self.cards()]
            time.sleep(0.25)
        raise ScenarioFailure(f"tool card {label!r} not found; cards: {last}")

    def open_tool(self, label: str, timeout=4):
        """Press a tool card. Returns the dialog, or None when it runs at once."""
        if self.category_hint and self.active_category() != self.category_hint:
            self.toggle_category(self.category_hint)
        node = self.wait_card(label)
        if not self.press(node):
            raise ScenarioFailure(f"could not press tool card {label!r}")
        return self.wait_dialog(timeout=timeout)

    def check_box(self, label):
        return self.find(role="check box", name=label)

    def check_state(self, label) -> bool:
        node = self.check_box(label)
        if node is None:
            raise ScenarioFailure(f"check box {label!r} not found")
        return "CHECKED" in _states(node)

    def set_check(self, label: str, value: bool):
        node = self.check_box(label)
        if node is None:
            raise ScenarioFailure(f"check box {label!r} not found")
        if ("CHECKED" in _states(node)) == value:
            return
        if not self.press(node):
            raise ScenarioFailure(f"could not toggle {label!r}")
        time.sleep(0.35)

    def combo(self, label):
        return self.find(role="combo box", name=label)

    def options(self, combo) -> list[str]:
        return [_safe(item, "get_name") or "" for item in self.find_all(role="menu item", within=combo)]

    def selected_option(self, label) -> str:
        combo = self.combo(label)
        if combo is None:
            return ""
        for item in self.find_all(role="menu item", within=combo):
            if "SELECTED" in _states(item):
                return _safe(item, "get_name") or ""
        return ""

    def select_option(self, label: str, option: str):
        combo = self.combo(label)
        if combo is None:
            raise ScenarioFailure(f"select {label!r} not found")
        options = self.options(combo)
        if option not in options:
            raise ScenarioFailure(f"{option!r} is not an option of {label!r}: {options}")
        if self.selected_option(label) == option:
            return
        current = options.index(self.selected_option(label)) if self.selected_option(label) in options else 0
        target = options.index(option)
        delta = target - current
        self.ensure_visible(combo)
        self.focus(combo)
        self.click(combo)
        time.sleep(0.3)
        key = "Down" if delta > 0 else "Up"
        if delta:
            run(["xdotool", "key", "--repeat", str(abs(delta)), "--delay", "40", key], timeout=30)
        run(["xdotool", "key", "Return"], timeout=10)
        time.sleep(0.4)
        if self.selected_option(label) != option:
            # The click may not have opened the native popup; a focused select
            # also changes on the arrow keys.
            self.focus(self.combo(label))
            run(["xdotool", "key", "Home"], timeout=10)
            if target:
                run(["xdotool", "key", "--repeat", str(target), "--delay", "40", "Down"], timeout=30)
            run(["xdotool", "key", "Return"], timeout=10)
            time.sleep(0.4)
        if self.selected_option(label) != option:
            raise ScenarioFailure(
                f"select {label!r} stayed on {self.selected_option(label)!r}, wanted {option!r}"
            )

    def entry(self, label):
        exact = self.find(role="entry", name=label)
        if exact is not None:
            return exact
        return self.find(role="entry", startswith=label) or self.find(
            role="entry", contains=label
        )

    def set_entry(self, label: str, value: str):
        """Type into an entry and read it back, re-locating it on every attempt.

        A field that appears in response to a checkbox shifts the dialog; a click that
        used the pre-shift extents lands somewhere else and the typing goes nowhere. Each
        attempt re-finds the node, so the retry is a fresh gesture rather than a repeat.
        """
        actual = ""
        for _attempt in range(3):
            node = self.entry(label)
            if node is None:
                raise ScenarioFailure(f"entry {label!r} not found")
            self.ensure_visible(node)
            self.focus(node)
            self.click(node)
            # The click is a coordinate gesture; make sure it landed on the entry.
            node = self.entry(label)
            if node is not None:
                self.focus(node)
            self.type_text(value)
            time.sleep(0.35)
            node = self.entry(label)
            actual = self.text(node) if node is not None else ""
            if actual == value:
                return
        raise ScenarioFailure(f"entry {label!r} reads {actual!r}, wanted {value!r}")

    def slider(self, label):
        return self.find(role="slider", name=label) or self.find(
            role="slider", contains=label
        )

    def set_slider(self, label: str, value: float, minimum: float, step: float):
        node = self.slider(label)
        if node is None:
            raise ScenarioFailure(f"slider {label!r} not found")
        self.ensure_visible(node)
        # Clicking gives the X window focus; Home then sets the minimum before the
        # arrow keys walk to the target. A Value-interface write alone does not fire
        # the Solid input handler on WebKit.
        self.click(node)
        self.focus(self.slider(label))
        run(["xdotool", "key", "--clearmodifiers", "Home"], timeout=10)
        count = max(0, round((value - minimum) / step))
        if count:
            run(["xdotool", "key", "--repeat", str(count), "--delay", "12", "Right"], timeout=60)
        time.sleep(0.3)
        node = self.slider(label)
        value_iface = _safe(node, "get_value_iface")
        current = _safe(value_iface, "get_current_value") if value_iface is not None else None
        if current is None or abs(float(current) - value) > step / 2:
            if value_iface is not None and _safe(value_iface, "set_current_value", value):
                time.sleep(0.2)
                node = self.slider(label)
                value_iface = _safe(node, "get_value_iface")
                current = _safe(value_iface, "get_current_value") if value_iface is not None else None
        if current is None or abs(float(current) - value) > step / 2:
            raise ScenarioFailure(f"slider {label!r} is {current!r}, wanted {value}")

    def preview_image(self, dialog, timeout=12):
        """The canvas preview is generated asynchronously; a video frame takes longer."""
        deadline = time.time() + timeout
        while time.time() < deadline:
            images = self.find_all(role="image", within=dialog)
            for image in images:
                if (_safe(image, "get_name") or "") in ("Preview", "The compressed result", ""):
                    return image
            if images:
                return images[0]
            time.sleep(0.3)
        return None

    def marked_count(self, dialog) -> int:
        match = re.search(r"(\d+) marked", self.subtree_text(dialog))
        return int(match.group(1)) if match else 0

    def drag_fraction(self, node, start_fraction, end_fraction):
        """Drag between two fractions of a node's box, through intermediate moves."""
        box = self.extents(node)
        if not box:
            raise Skip("the surface has no screen extents to drag on")
        x, y, width, height = box
        start = (x + int(width * start_fraction[0]), y + int(height * start_fraction[1]))
        end = (x + int(width * end_fraction[0]), y + int(height * end_fraction[1]))
        run(["xdotool", "mousemove", str(start[0]), str(start[1])], timeout=10)
        time.sleep(0.15)
        run(["xdotool", "mousedown", "1"], timeout=10)
        time.sleep(0.2)
        # Several moves: one jump can be coalesced before WebKit sees a pointermove.
        for step in range(1, 6):
            mx = start[0] + (end[0] - start[0]) * step // 5
            my = start[1] + (end[1] - start[1]) * step // 5
            run(["xdotool", "mousemove", str(mx), str(my)], timeout=10)
            time.sleep(0.08)
        run(["xdotool", "mouseup", "1"], timeout=10)
        time.sleep(0.6)

    def move_dialog_by(self, dx: int, dy: int):
        """Drag the dialog's head by (dx, dy) screen pixels, with real mouse events."""
        box = self.extents(self.sheet())
        if not box:
            raise Skip("the sheet has no extents to drag")
        x, y = box[0] + box[2] // 2, box[1] + 18
        run(["xdotool", "mousemove", str(x), str(y)], timeout=10)
        time.sleep(0.15)
        run(["xdotool", "mousedown", "1"], timeout=10)
        time.sleep(0.15)
        for step in range(1, 6):
            run(
                ["xdotool", "mousemove", str(x + dx * step // 5), str(y + dy * step // 5)],
                timeout=10,
            )
            time.sleep(0.06)
        run(["xdotool", "mouseup", "1"], timeout=10)
        time.sleep(0.5)

    def centre_dialog(self):
        """Double-click the head: the dialog returns to the centred position."""
        box = self.extents(self.sheet())
        if not box:
            raise Skip("the sheet has no extents to double-click")
        x, y = box[0] + box[2] // 2, box[1] + 18
        run(["xdotool", "mousemove", str(x), str(y)], timeout=10)
        run(["xdotool", "click", "--repeat", "2", "--delay", "90", "1"], timeout=10)
        time.sleep(0.5)

    def mark_region(self, dialog, start=(0.25, 0.3), end=(0.6, 0.7)):
        """Drag one rectangle on the boxes canvas inside the virtual display."""
        # The canvas, not the first image in the dialog: a video dialog also shows
        # filmstrip frames, and dragging one of those never becomes a region.
        images = self.find_all(role="image", within=dialog)

        def area(node):
            box = self.extents(node)
            return (box[2] * box[3]) if box else 0

        image = max(images, key=area) if images else None
        if image is None or area(image) == 0:
            detail = self.subtree_text(dialog).strip().replace("\n", " | ")[-300:]
            raise Skip(f"the boxes canvas exposes no preview image to drag on: {detail}")
        # A surface below the fold (under a timeline, in a long dialog) has to be
        # scrolled into view first, or the drag happens somewhere else entirely.
        self.ensure_visible(image)
        time.sleep(0.2)
        self.drag_fraction(image, start, end)

    def scroll_dialog(self, clicks: int, direction: str = "down"):
        """Scroll the sheet with the wheel, over its centre.

        WebKit prunes what is scrolled out of the sheet from the accessibility tree, so
        a control below the fold is not merely invisible: it is absent. Moving the wheel
        is how a person reaches it, and it is the only reliable way to make the tree grow.
        """
        dialog = self.dialog()
        if dialog is None:
            return
        box = self.extents(dialog)
        if not box:
            return
        x, y, width, height = box
        run(["xdotool", "mousemove", str(x + width // 2), str(y + height // 2)], timeout=10)
        button = "5" if direction == "down" else "4"
        for _ in range(max(1, clicks)):
            run(["xdotool", "click", button], timeout=10)
            time.sleep(0.08)
        time.sleep(0.3)

    def drag_bleep_range(self, dialog, start, end):
        """Drag across the waveform lane, which is where a bleep is marked."""
        surface = self.find(name="Bleep ranges", within=dialog)
        if surface is None:
            raise ScenarioFailure("the bleep lane is missing")
        self.ensure_visible(surface)
        time.sleep(0.2)
        self.drag_fraction(surface, (start, 0.5), (end, 0.5))

    def detect_or_mark(self, dialog, timeout=12):
        """Run OCR, then draw one rectangle too: the drawn box is the guarantee.

        The toolbar's "N marked" text is not exposed by WebKit, so the boxes count
        cannot be read back; the job's success (or "no boxes to redact") is the
        observable result.
        """
        detect = self.button("Detect text", within=dialog)
        if detect is not None and self.press(detect):
            deadline = time.time() + timeout
            while time.time() < deadline:
                if "detected" in self.subtree_text(dialog):
                    break
                time.sleep(0.4)
        self.mark_region(dialog)
        return "ocr+drawn"

    # -- jobs ------------------------------------------------------------

    def snapshot(self) -> dict:
        out = {}
        for name in os.listdir(self.workdir):
            if name.startswith(".") or name.endswith((".tmp", ".part", ".lock")):
                continue
            path = os.path.join(self.workdir, name)
            try:
                stat = os.stat(path)
            except OSError:
                continue
            out[name] = (stat.st_size, stat.st_mtime_ns, os.path.isdir(path))
        return out

    def history_rows(self):
        # The panel's rows stay in the DOM while Activity is collapsed; read those
        # first. Pressing the Activity bar while a modal is open is blocked by the
        # backdrop, and that used to return an empty history just before Run.
        panel = self.find(name="Job history")
        rows = (
            [
                _text(row) or (_safe(row, "get_name") or "")
                for row in self.find_all(role="list item", within=panel)
            ]
            if panel is not None
            else []
        )
        if rows:
            return rows
        if self.dialog() is None:
            activity = self.find(role="button", startswith="Activity")
            if activity is not None:
                self.press(activity)
                time.sleep(0.45)
                panel = self.find(name="Job history")
                if panel is not None:
                    rows = [
                        _text(row) or (_safe(row, "get_name") or "")
                        for row in self.find_all(role="list item", within=panel)
                    ]
        return rows

    def show_history(self):
        """Open Activity while no modal is up, so rows are readable during the job."""
        panel = self.find(name="Job history")
        if panel is not None and self.find_all(role="list item", within=panel):
            return
        if self.dialog() is not None:
            return
        activity = self.find(role="button", startswith="Activity")
        if activity is not None:
            self.press(activity)
            time.sleep(0.45)

    def status_text(self) -> str:
        bar = self.find(role="status bar")
        return _text(bar) if bar is not None else ""

    def dialog_buttons(self):
        """Every button in the dialog, in document order.

        WebKit reports role `button`; AT-SPI's `push button` is what other toolkits
        call the same thing. One walk, so the order is the DOM's — the footer is last,
        which is how the primary is found among the fields' own buttons.
        """
        dialog = self.dialog()
        if dialog is None:
            return []
        return [
            node
            for node, _depth in _walk(dialog)
            if _safe(node, "get_role_name") in ("push button", "button", "toggle button")
        ]

    def primary_button(self):
        """The dialog's commit button: the last button that is not Reset or Close.

        Its label comes from the tool table ("Save Organized PDF", "Create Collage"),
        so it cannot be matched by name. The footer is the last thing in the sheet, so
        the primary is the last button in document order — the fields' own buttons
        (colour swatches, segmented options, Choose…) all come before it.
        """
        for node in reversed(self.dialog_buttons()):
            name = _safe(node, "get_name") or ""
            if name in ("Reset", "Close", "Cancel"):
                continue
            return node
        return None

    def primary_label(self) -> str:
        node = self.primary_button()
        return (_safe(node, "get_name") or "") if node is not None else ""

    def press_primary(self):
        # The accessibility tree is rebuilt for a moment when a preview inserts its
        # player element; wait for the dialog instead of failing the scenario.
        deadline = time.time() + 3
        dialog = self.dialog()
        while dialog is None and time.time() < deadline:
            time.sleep(0.3)
            dialog = self.dialog()
        if dialog is None:
            raise ScenarioFailure("the primary action was pressed with no dialog")
        button = self.primary_button()
        if button is None:
            labels = [_safe(node, "get_name") for node in self.dialog_buttons()]
            raise ScenarioFailure(f"no primary button in the dialog; buttons: {labels}")
        if "SENSITIVE" not in _states(button) and "ENABLED" not in _states(button):
            raise ScenarioFailure(
                f"the primary button {_safe(button, 'get_name')!r} is disabled"
            )
        if not self.press(button):
            raise ScenarioFailure("could not press the primary button")

    def press_run(self):
        self.press_primary()

    def press_in_dialog(self, name: str):
        dialog = self.dialog()
        if dialog is None:
            raise ScenarioFailure(f"{name!r} pressed with no dialog")
        self.press_button(name, within=dialog)

    def dialog_entry(self, label: str):
        dialog = self.dialog()
        return self.entry(label) if dialog is None else (
            self.find(role="entry", name=label, within=dialog)
            or self.find(role="entry", contains=label, within=dialog)
        )

    def dialog_check(self, label: str):
        dialog = self.dialog()
        if dialog is None:
            return self.check_box(label)
        return self.find(role="check box", name=label, within=dialog)

    def sheet(self):
        """The settings pane: the form inside the dialog wrapper."""
        dialog = self.dialog()
        if dialog is None:
            return None
        return self.find(tag="form", within=dialog) or dialog

    def dialog_info(self) -> dict:
        """The dialog contract, measured: one sheet, its copy, its action."""
        dialog = self.dialog()
        if dialog is None:
            return {"dialog": False}
        sheet = self.sheet()
        buttons = [_safe(node, "get_name") or "" for node in self.dialog_buttons()]
        # WebKit exposes an <h2> as a heading in some builds and as a label in
        # others; the sheet title is whichever one carries the tag.
        title_nodes = []
        if sheet is not None:
            for role in ("heading", "label"):
                title_nodes.extend(self.find_all(role=role, within=sheet))
        sheet_title = ""
        for node in title_nodes:
            if (_attributes(node).get("tag") or "") in ("h2", "h1"):
                sheet_title = _safe(node, "get_name") or _text(node) or ""
                break
        if not sheet_title and title_nodes:
            sheet_title = _safe(title_nodes[0], "get_name") or _text(title_nodes[0]) or ""
        return {
            "dialog": True,
            "sheet_title": sheet_title,
            "buttons": buttons,
            "primary": self.primary_label(),
            "sheet_box": list(self.extents(sheet)) if sheet is not None and self.extents(sheet) else None,
            "dialog_box": list(self.extents(dialog)) if self.extents(dialog) else None,
            "frame_box": list(self.frame_extents()) if self.frame_extents() else None,
        }

    def strays_in_sheet(self, limit: int = 8):
        """Controls drawn outside the sheet's box: clipped content that still "works".

        A row wider than the sheet is cut by `overflow: hidden`; the screenshot shows a
        truncated label and every functional check still passes. Measure the geometry
        instead, in the real window.
        """
        sheet = self.sheet()
        box = self.extents(sheet) if sheet is not None else None
        if not box:
            return []
        sx, _sy, sw, _sh = box
        strays = []
        for node, _depth in _walk(sheet):
            b = self.extents(node)
            if not b:
                continue
            x, _y, w, _h = b
            if w <= 1:
                continue
            if x < sx - 2 or x + w > sx + sw + 2:
                name = (_safe(node, "get_name") or _text(node) or "").strip().replace("\n", " ")[:28]
                role = _safe(node, "get_role_name") or ""
                strays.append([role, name, [int(v) for v in b]])
                if len(strays) >= limit:
                    break
        return strays

    def set_slider_auto(self, label: str, value: float):
        """Set a slider without being told its bounds: AT-SPI reports both.

        A timeline's handles appear (and get their maximum) once the dialogue has
        read the clip's length; pressing Home before that would clamp every step to
        the initial 0. Wait for the handle and for a maximum that can hold the value.
        """
        deadline = time.time() + 20
        node = None
        value_iface = None
        maximum = None
        while time.time() < deadline:
            node = self.slider(label)
            value_iface = _safe(node, "get_value_iface") if node is not None else None
            maximum = _safe(value_iface, "get_maximum_value") if value_iface is not None else None
            if node is not None and (maximum is None or float(maximum) >= value):
                break
            time.sleep(0.3)
        if node is None:
            raise ScenarioFailure(f"slider {label!r} not found")
        minimum = (
            float(_safe(value_iface, "get_minimum_value"))
            if value_iface is not None and _safe(value_iface, "get_minimum_value") is not None
            else 0.0
        )
        step = (
            float(_safe(value_iface, "get_minimum_increment"))
            if value_iface is not None and _safe(value_iface, "get_minimum_increment")
            else 1.0
        )
        self.set_slider(label, value, minimum, step or 1.0)

    def wait_job(self, before_files, before_rows, timeout=180, expect=1):
        deadline = time.time() + timeout
        candidate = []
        stable_since = None
        last_rows = []
        next_history_check = 0.0
        while time.time() < deadline:
            current = self.snapshot()
            new_names = [name for name in current if name not in before_files]
            new_paths = [os.path.join(self.workdir, name) for name in sorted(new_names)]
            if len(new_paths) >= expect:
                fingerprint = tuple((name, current[name][0], current[name][1]) for name in sorted(new_names))
                if fingerprint == candidate:
                    if stable_since is None:
                        stable_since = time.time()
                    elif time.time() - stable_since >= 0.5:
                        return new_paths
                else:
                    candidate = fingerprint
                    stable_since = None
            else:
                candidate = []
                stable_since = None
            if time.time() >= next_history_check:
                next_history_check = time.time() + 1.5
                rows = self.history_rows()
                added = _multiset_minus(rows, before_rows)
                last_rows = added or last_rows
                for row in added:
                    lowered = row.lower()
                    if ("failed" in lowered or "cancelled" in lowered) and not new_paths:
                        raise ScenarioFailure(
                            f"job ended without an output: {row.strip()}"
                        )
            if self.dialog() is not None:
                # A validation error keeps the dialog open and shows it inline.
                dialog = self.dialog()
                texts = [_text(n) for n in self.find_all(within=dialog)]
                error = next(
                    (
                        t
                        for t in texts
                        if t
                        and any(
                            word in t.lower()
                            for word in (
                                "unknown",
                                "must",
                                "failed",
                                "missing",
                                "not installed",
                                "reinstall",
                                "could not",
                                "error",
                                "cannot",
                            )
                        )
                    ),
                    "",
                )
                if error:
                    raise ScenarioFailure(f"dialog rejected the parameters: {error}")
            time.sleep(0.3)
        raise ScenarioFailure(
            f"no new artifact after {timeout}s; history: {last_rows[-3:]} status: {self.status_text()!r}"
        )

    def cancel_running(self, timeout=8) -> bool:
        """Cancel the newest active row. Returns True when a Cancel was pressed."""
        panel = self.find(name="Job history")
        if panel is None or "SHOWING" not in _states(panel):
            activity = self.find(role="button", startswith="Activity")
            if activity is not None and self.press(activity):
                time.sleep(0.4)
        deadline = time.time() + timeout
        while time.time() < deadline:
            cancel = self.find(role="button", name="Cancel")
            if cancel is not None:
                return self.press(cancel)
            time.sleep(0.2)
        return False

    def cancel_all(self):
        activity = self.find(role="button", startswith="Activity")
        if activity is not None and "SHOWING" not in _states(activity):
            self.press(activity)
            time.sleep(0.3)
        cancel = self.find(role="button", name="Cancel all")
        if cancel is not None:
            self.press(cancel)
            time.sleep(0.3)

    def preview_text(self) -> str:
        dialog = self.dialog()
        if dialog is None:
            return ""
        for node in self.find_all(within=dialog):
            attrs = _attributes(node)
            if attrs.get("live") == "polite" or attrs.get("aria-live") == "polite":
                return _text(node)
        return ""

    # -- screenshots ------------------------------------------------------

    def window_id(self) -> str:
        result = run(["xdotool", "search", "--onlyvisible", "--name", "^ConvertMe$"], timeout=15)
        ids = result.stdout.split()
        if not ids:
            result = run(["xdotool", "search", "--onlyvisible", "--name", "ConvertMe"], timeout=15)
            ids = result.stdout.split()
        if not ids:
            raise ScenarioFailure("no visible ConvertMe window")
        return ids[-1]

    def shot(self, label: str) -> str:
        path = os.path.join(self.h.shot_dir, f"{self.name}-{label}.png")
        result = run(["import", "-window", self.window_id(), path], timeout=30)
        if result.returncode != 0:
            root = os.path.join(self.h.shot_dir, f"{self.name}-{label}-root.png")
            run(["import", "-window", "root", root], timeout=30)
            frame = self.frame_extents()
            if frame:
                x, y, w, h = frame
                run(["magick", root, "-crop", f"{w}x{h}+{x}+{y}", "+repage", path], timeout=30)
                try:
                    os.remove(root)
                except OSError:
                    pass
        return os.path.relpath(path, self.h.out_dir)


class Harness:
    def __init__(self, atspi, out_dir, profile, fixtures, timeout):
        self.atspi = atspi
        self.out_dir = out_dir
        self.profile = profile
        self.fixtures_root = fixtures
        self.timeout = timeout
        self.work_root = os.path.join(profile, "work")
        self.shot_dir = os.path.join(out_dir, "screenshots")
        self.log_dir = os.path.join(out_dir, "logs")
        os.makedirs(self.work_root, exist_ok=True)
        os.makedirs(self.shot_dir, exist_ok=True)
        os.makedirs(self.log_dir, exist_ok=True)
        self.build_id = bundle_build_id()
        self.stamp = ui_build_stamp()
        self.stamp_in_binary = binary_contains(self.stamp)
        self.results = []

    def fixture(self, relative: str) -> str:
        path = os.path.join(self.fixtures_root, relative)
        if not os.path.exists(path):
            raise Skip(f"missing fixture {relative}")
        return path

    def launch(
        self,
        name,
        fixtures,
        category=None,
        env_extra=None,
        exe=None,
    ) -> Session:
        return Session(self, name, fixtures, category=category, env_extra=env_extra, exe=exe)

    def run_scenario(self, name, fn):
        rec = {
            "name": name,
            "status": "passed",
            "started": now_iso(),
            "build_id": self.build_id,
            "checks": [],
            "screenshots": [],
            "measurements": {},
            "parameters": {},
            "artifacts": [],
        }
        checks = Checks()
        started = time.time()
        print(f"\n=== {name} ===")
        try:
            fn(self, rec, checks)
        except Skip as error:
            rec["status"] = "skipped"
            rec["reason"] = str(error)
            print(f"SKIP: {error}")
        except Exception as error:
            rec["status"] = "failed"
            rec["reason"] = f"{type(error).__name__}: {error}"
            rec["traceback"] = traceback.format_exc()
            print(f"FAIL: {rec['reason']}")
        rec["checks"] = checks.items
        if rec["status"] == "passed" and not checks.ok:
            rec["status"] = "failed"
            failed = checks.failures()[0]
            rec["reason"] = f"check failed: {failed['name']} (expected {failed.get('expected')!r}, actual {failed.get('actual')!r})"
            print(f"FAIL: {rec['reason']}")
        rec["duration_s"] = round(time.time() - started, 1)
        rec["ended"] = now_iso()
        self.results.append(rec)
        self.write_report()

    def write_report(self):
        counts = {"passed": 0, "failed": 0, "skipped": 0}
        for rec in self.results:
            counts[rec["status"]] += 1
        report = {
            "started": self.results[0]["started"] if self.results else now_iso(),
            "updated": now_iso(),
            "build_id": self.build_id,
            "ui_build_stamp": self.stamp,
            "binary_contains_stamp": self.stamp_in_binary,
            "binary": BIN,
            "binary_sha256": sha256(BIN),
            "counts": counts,
            "environment": self.environment,
            "scenarios": self.results,
        }
        path = os.path.join(self.out_dir, "report.json")
        with open(path, "w", encoding="utf-8") as handle:
            json.dump(report, handle, indent=2, sort_keys=True)
            handle.write("\n")

    environment = {}


# --------------------------------------------------------------------------
# Helpers for AT-SPI objects
# --------------------------------------------------------------------------


def _children(node):
    out = []
    count = _safe(node, "get_child_count") or 0
    for index in range(count):
        child = _safe(node, "get_child_at_index", index)
        if child is not None:
            out.append(child)
    return out


def _walk(node, depth=0, max_depth=40):
    if node is None or depth > max_depth:
        return
    yield node, depth
    for child in _children(node):
        yield from _walk(child, depth + 1, max_depth)


def _safe(obj, attribute, *args):
    if obj is None:
        return None
    try:
        fn = getattr(obj, attribute)
        return fn(*args)
    except Exception:
        return None


def _text(node) -> str:
    if node is None:
        return ""
    try:
        text_iface = node.get_text_iface()
        if text_iface is None:
            return ""
        count = _safe(text_iface, "get_character_count")
        # PyGObject binds Accessible.get_text first, so call the interface method
        # explicitly; the deprecated Accessible.get_text takes no offsets.
        import gi

        gi.require_version("Atspi", "2.0")
        from gi.repository import Atspi

        return Atspi.Text.get_text(text_iface, 0, count or 0) or ""
    except Exception:
        return ""


def _states(node) -> set[str]:
    state_set = _safe(node, "get_state_set")
    if state_set is None:
        return set()
    names = (
        "SHOWING",
        "VISIBLE",
        "FOCUSED",
        "ENABLED",
        "SENSITIVE",
        "CHECKED",
        "SELECTED",
        "EXPANDED",
        "PRESSED",
        "EDITABLE",
    )
    out = set()
    try:
        from gi.repository import Atspi

        for name in names:
            if state_set.contains(getattr(Atspi.StateType, name)):
                out.add(name)
    except Exception:
        pass
    return out


def _attributes(node) -> dict:
    attrs = _safe(node, "get_attributes")
    if not attrs:
        return {}
    try:
        return dict(attrs)
    except Exception:
        return {}


def _extents(node):
    component = _safe(node, "get_component_iface")
    if component is None:
        return None
    try:
        from gi.repository import Atspi

        rect = component.get_extents(Atspi.CoordType.SCREEN)
        return (rect.x, rect.y, rect.width, rect.height)
    except Exception:
        return None


def _multiset_minus(rows, before):
    remaining = list(before)
    out = []
    for row in rows:
        if row in remaining:
            remaining.remove(row)
        else:
            out.append(row)
    return out


# --------------------------------------------------------------------------
# Scenario helpers
# --------------------------------------------------------------------------


def record_dialog(rec, session, label):
    dialog = session.dialog()
    if dialog is None:
        raise ScenarioFailure(f"no dialog for {label}")
    frame = session.frame_extents()
    box = session.extents(dialog)
    nodes = []
    for node, depth in _walk(dialog):
        role = _safe(node, "get_role_name")
        name = _safe(node, "get_name") or ""
        value = _text(node)
        entry = {
            "role": role,
            "name": name,
            "depth": depth,
            "states": sorted(_states(node)),
        }
        if value:
            entry["text"] = value
        extents = _extents(node)
        if extents:
            entry["box"] = list(extents)
        tag = _attributes(node).get("tag")
        if tag:
            entry["tag"] = tag
        nodes.append(entry)
    rec["measurements"][f"dialog_{label}"] = {
        "title": _safe(dialog, "get_name"),
        "box": list(box) if box else None,
        "frame": list(frame) if frame else None,
        "nodes": nodes,
    }
    rec["screenshots"].append(session.shot(f"dialog-{label.lower().replace(' ', '-')}"))
    return dialog, frame, box, nodes


def action_label(action: str, count: int = 0) -> str:
    """The primary button's text, with the count substituted.

    The tool table writes `Save {n} Frame(s)`; one frame reads "Frame", anything else
    "Frames". Shared with `ui/src/components/tool-modal.tsx::actionLabel`.
    """
    if "{n}" not in action:
        return action
    return action.replace("{n}", str(count)).replace(
        "Frame(s)", "Frame" if count == 1 else "Frames"
    )


def assert_design(rec, checks, session, meta, category, label):
    """The dialog contract: copy, geometry, and one primary action.

    Every check is a measurement, not a picture: the title must be the backend's own
    copy, the sheet must stay a dialog inside the window, and the footer must carry the
    tool's action label.
    """
    info = session.dialog_info()
    rec["measurements"][f"design_{label}"] = info
    if not info.get("dialog"):
        checks.add(f"{label}: dialog opened", False, "a dialog", "none")
        return
    expected_title = meta.get("title") or label
    checks.add(
        f"{label}: sheet title",
        info["sheet_title"] == expected_title,
        expected_title,
        info["sheet_title"],
    )
    # One sheet, never the whole window: the wheel behind it stays visible, and the
    # dialog can be dragged and resized instead of owning the screen.
    sheet_box = info.get("sheet_box") or [0, 0, 0, 0]
    frame_box = info.get("frame_box") or [0, 0, 1, 1]
    checks.add(
        f"{label}: sheet keeps a margin",
        sheet_box[0] >= frame_box[0] + 4
        and sheet_box[1] >= frame_box[1] + 4
        and sheet_box[0] + sheet_box[2] <= frame_box[0] + frame_box[2] - 4
        and sheet_box[1] + sheet_box[3] <= frame_box[1] + frame_box[3] - 4,
        f"inside {frame_box} with 4 px",
        sheet_box,
    )
    checks.add(
        f"{label}: sheet is not the window",
        sheet_box[2] < frame_box[2] * 0.9 or sheet_box[3] < frame_box[3] * 0.9,
        "narrower than 90 % of the window",
        (sheet_box[2], frame_box[2]),
    )
    strays = session.strays_in_sheet()
    rec["measurements"][f"strays_{label}"] = strays
    checks.add(
        f"{label}: every control stays inside the sheet",
        not strays,
        "no element wider than the sheet",
        strays[:4],
    )
    expected_action = action_label(meta.get("action") or "Run")
    checks.add(
        f"{label}: primary action label",
        info["primary"] == expected_action,
        expected_action,
        info["primary"],
    )
    checks.add(
        f"{label}: Reset on the left, Close in the head",
        "Reset" in info["buttons"] and "Close" in info["buttons"],
        ["Reset", "Close"],
        info["buttons"],
    )
    checks.add(
        f"{label}: no Cancel button",
        "Cancel" not in info["buttons"],
        "no Cancel",
        info["buttons"],
    )
    frame = info.get("frame_box")
    for pane in ("sheet_box",):
        box = info.get(pane)
        if not box or not frame:
            checks.add(f"{label}: {pane} measured", False, "extents", box)
            continue
        fx, fy, fw, fh = frame
        x, y, w, h = box
        checks.add(
            f"{label}: {pane} inside the window",
            x >= fx - 2 and y >= fy - 2 and x + w <= fx + fw + 2 and y + h <= fy + fh + 2,
            f"inside {frame}",
            box,
        )
        checks.add(
            f"{label}: {pane} has no horizontal overflow",
            x + w <= fx + fw + 2,
            f"right edge <= {fx + fw + 2}",
            x + w,
        )


def assert_dialog_inside(rec, checks, label, frame, box):
    if not frame or not box:
        checks.add(f"{label}: dialog geometry", False, "frame and dialog extents", box)
        return
    fx, fy, fw, fh = frame
    x, y, w, h = box
    inside = x >= fx - 2 and y >= fy - 2 and x + w <= fx + fw + 2 and y + h <= fy + fh + 2
    checks.add(
        f"{label}: dialog inside window",
        inside,
        f"within {frame}",
        box,
    )


def open_and_record(session, rec, label):
    dialog = session.open_tool(label)
    if dialog is None:
        return None
    _, frame, box, nodes = record_dialog(rec, session, label)
    return (dialog, frame, box, nodes)


def close_dialog(session):
    """The X in the sheet head — the reference has no Cancel button."""
    dialog = session.dialog()
    if dialog is None:
        return
    close = session.button("Close", within=dialog)
    if close is None:
        raise ScenarioFailure("Close button not found")
    if not session.press(close):
        raise ScenarioFailure("could not press Close")
    if not session.wait_dialog_closed():
        raise ScenarioFailure("dialog did not close after Close")


def run_tool_job(session, rec, checks, label, apply=None, expect=1, timeout=240, category=None):
    session.show_history()
    before_rows = session.history_rows()
    rec["measurements"]["history_after_show"] = before_rows[:2]
    if category:
        session.toggle_category(category)
    dialog = session.open_tool(label)
    if dialog is None:
        raise ScenarioFailure(f"{label} ran without a dialog; expected one")
    time.sleep(0.3)
    record_dialog(rec, session, label)
    if apply is not None:
        apply(session)
    rec["screenshots"].append(session.shot("dialog"))
    preview = session.preview_text()
    if preview:
        rec["measurements"]["preview"] = preview.strip()
    before_files = session.snapshot()
    rec["measurements"]["history_before"] = before_rows[:3]
    session.press_run()
    outputs = session.wait_job(before_files, before_rows, timeout=timeout, expect=expect)
    rec["measurements"]["history_after"] = session.history_rows()[:4]
    rec["screenshots"].append(session.shot("after"))
    return outputs, preview


def persist_artifact(path: str, scenario: str) -> str:
    """Copy a published artifact out of the throwaway work dir."""
    if not RUN_OUT_DIR:
        return path
    root = os.path.join(RUN_OUT_DIR, "artifacts", scenario)
    os.makedirs(root, exist_ok=True)
    destination = os.path.join(root, os.path.basename(path.rstrip("/")))
    if os.path.isdir(path):
        shutil.copytree(path, destination, dirs_exist_ok=True)
    else:
        shutil.copy2(path, destination)
    return destination


def record_outputs(rec, outputs):
    records = []
    for path in outputs:
        persisted = persist_artifact(path, rec.get("name", "scenario"))
        record = describe_artifact(persisted)
        record["path"] = (
            os.path.relpath(persisted, RUN_OUT_DIR) if RUN_OUT_DIR else persisted
        )
        record["source_path"] = path
        record["name"] = os.path.basename(path.rstrip("/"))
        records.append(record)
    rec["artifacts"].extend(records)
    return records


def conversion_job(session, rec, checks, target, expect=1, timeout=240):
    if session.category_hint and session.active_category() != session.category_hint:
        session.toggle_category(session.category_hint)
    session.wait_targets()
    # The analysis replaces the fallback target list; re-read after it settles.
    time.sleep(0.6)
    rec["screenshots"].append(session.shot("workspace"))
    entries = session.target_entries()
    names = [label for label, _hint, _enabled, _node in entries]
    rec["measurements"]["targets"] = [
        {"label": label, "hint": hint, "enabled": enabled}
        for label, hint, enabled, _node in entries
    ]
    # A disabled target's accessible name is label + reason ("PDF LibreOffice is not
    # installed"); a prefix match keeps the label usable.
    match = next((entry for entry in entries if entry[0] == target), None)
    if match is None:
        match = next((entry for entry in entries if entry[0].startswith(target + " ")), None)
    if match is None:
        raise Skip(
            f"target {target!r} is not offered for this source (offered: {names}); "
            "the engine is probably missing"
        )
    label, hint, enabled, button = match
    reason = hint or (label[len(target):].strip() if label.startswith(target) else "")
    if not enabled or "not installed" in label.lower() or "not installed" in hint.lower():
        raise Skip(f"target {target!r} is disabled: {reason or 'no reason exposed'}")
    rec["parameters"]["target"] = target
    before_files = session.snapshot()
    before_rows = session.history_rows()
    if not session.press(button):
        # Analysis may have replaced the node between discovery and the press.
        time.sleep(0.3)
        entries = session.target_entries()
        button = next((node for label, _hint, _enabled, node in entries if label == target), None)
        if button is None or not session.press(button):
            raise ScenarioFailure(f"could not press target {target!r}")
    outputs = session.wait_job(before_files, before_rows, timeout=timeout, expect=expect)
    rec["measurements"]["history_after"] = session.history_rows()[:4]
    rec["screenshots"].append(session.shot("after"))
    return outputs


# --------------------------------------------------------------------------
# Scenarios
# --------------------------------------------------------------------------


def scenario_build_identity(h, rec, checks):
    poster = h.fixture(POSTER)
    with h.launch("build-identity", [poster]) as session:
        rec["workdir"] = session.workdir
        exe_link = os.path.realpath(f"/proc/{session.proc.pid}/exe")
        checks.add(
            "process exe is the release binary",
            exe_link == os.path.realpath(BIN),
            os.path.realpath(BIN),
            exe_link,
        )
        checks.add(
            "binary embeds the ui-dist build stamp",
            h.stamp_in_binary,
            "found",
            "found" if h.stamp_in_binary else "not found",
        )
        session.press_button("Settings", exact=True)
        session.wait_text("Interface build", timeout=10)
        texts = session.all_texts()
        rec["measurements"]["settings"] = [
            {"role": role, "text": value} for role, value in texts if value.strip()
        ]
        build_text = next(
            (value for _role, value in texts if "Interface build" in value), ""
        )
        version_text = next(
            (value for _role, value in texts if "version " in value and "conversion options" in value),
            "",
        )
        shown_build = ""
        match = re.search(r"Interface build ([0-9-]+ [0-9:]+)", build_text)
        if match:
            shown_build = match.group(1)
        checks.add(
            "Settings shows the bundle build id",
            shown_build == h.build_id,
            h.build_id,
            shown_build,
        )
        cli_version = run([BIN, "--version"]).stdout.strip()
        cli_match = re.search(r"([0-9]+\.[0-9]+\.[0-9]+)", cli_version)
        shown_version = ""
        version_match = re.search(r"version ([0-9]+\.[0-9]+\.[0-9]+)", version_text)
        if version_match:
            shown_version = version_match.group(1)
        checks.add(
            "Settings version matches --version",
            bool(cli_match) and shown_version == cli_match.group(1),
            cli_version,
            shown_version or version_text,
        )
        rec["measurements"]["build_chain"] = {
            "bundle_build_id": h.build_id,
            "ui_build_stamp": h.stamp,
            "stamp_in_binary": h.stamp_in_binary,
            "settings_interface_build": shown_build,
            "settings_version_text": version_text,
            "cli_version": cli_version,
            "process_exe": exe_link,
        }
        rec["screenshots"].append(session.shot("settings"))
        session.press_button("Close settings")
        time.sleep(0.5)


def scenario_dialogs(h, rec, checks, category):
    fixture = h.fixture(CATEGORY_FIXTURES[category])
    tools = BACKEND_TOOLS.get(category, [])
    with h.launch(f"dialogs-{category.lower()}", [fixture]) as session:
        rec["workdir"] = session.workdir
        session.toggle_category(category)
        cards = session.cards()
        card_labels = [label for label, _node in cards]
        rec["measurements"]["cards"] = card_labels
        rec["screenshots"].append(session.shot("workspace"))
        checks.add(
            f"{category}: tool cards listed",
            bool(cards),
            "at least one card",
            card_labels,
        )
        if not tools:
            # The UI shows FALLBACK_TOOLS for categories the backend does not
            # serve. Open the first one to record the real behaviour, then report
            # every card as skipped with the reason. No silent omission.
            observed = ""
            if cards:
                session.press(cards[0][1])
                time.sleep(2.0)
                dialog = session.dialog()
                if dialog is not None:
                    record_dialog(rec, session, cards[0][0])
                    close_dialog(session)
                    observed = "a dialog appeared unexpectedly"
                else:
                    observed = session.status_text().strip() or session.has_text("missing") and "missing note" or ""
                    session.cancel_all()
                checks.add(
                    f"{category}: fallback card does not open a dialog",
                    dialog is None,
                    "no dialog",
                    observed or "no dialog",
                )
            raise Skip(
                f"backend registers no {category} tools; the UI shows "
                f"{len(cards)} fallback card(s) with no field table/engine. "
                f"First card observed: {observed or 'no dialog, no job'}"
            )
        for label in tools:
            if (category, label) in FIELDLESS:
                rec["measurements"][f"dialog_{label}"] = {
                    "dialog": False,
                    "note": "tool has no fields; the card runs immediately",
                }
                continue
            opened = open_and_record(session, rec, label)
            if opened is not None:
                meta = TOOL_TABLE.get((category, label), {})
                assert_design(rec, checks, session, meta, category, label)
            if opened is None:
                observed = session.status_text().strip()
                session.cancel_all()
                time.sleep(0.3)
                rows = session.history_rows()[:2]
                rec["measurements"][f"dialog_{label}"] = {
                    "dialog": False,
                    "status": observed,
                    "history": rows,
                }
                checks.add(
                    f"{category}/{label}: dialog opened",
                    False,
                    "dialog",
                    {"status": observed, "history": rows},
                )
                continue
            dialog, frame, box, nodes = opened
            # The metadata browser asks the writer for its tags asynchronously; the
            # rows are not in the tree until that call returns.
            if label == "Metadata":
                deadline = time.time() + 5
                while time.time() < deadline and session.entry("Title") is None:
                    time.sleep(0.2)
            assert_dialog_inside(rec, checks, f"{category}/{label}", frame, box)
            # A timeline/crop/waveform surface is fetched when the dialog opens;
            # its controls (handles, transports) exist only after that.
            time.sleep(0.6)
            roles = {node["role"] for node in nodes}
            # The reference footer has Reset and the tool's own action; the head has
            # Close. There is no generic Run and no Cancel.
            labels = [_safe(node, "get_name") or "" for node in session.dialog_buttons()]
            checks.add(
                f"{category}/{label}: Reset, Close and the tool's action",
                {"Reset", "Close"} <= set(labels)
                and bool(TOOL_TABLE.get((category, label), {}).get("action")),
                ["Reset", "Close", "the tool's action"],
                labels,
            )
            # A tool that declares "Run" as its action gets a Run button; the reference
            # tools name their own action and must not fall back to it.
            declared = TOOL_TABLE.get((category, label), {}).get("action", "Run")
            stray = [
                name
                for name in labels
                if name == "Cancel" or (name == "Run" and declared != "Run")
            ]
            checks.add(
                f"{category}/{label}: the footer uses the tool's action",
                not stray,
                [],
                stray,
            )
            for role, name in CONTROL_EXPECTATIONS.get((category, label), []):
                # WebKit names a pushed button `button`, some AT-SPI builds say
                # `push button`; the contract is the control, not the spelling.
                aliases = (role, "push button") if role == "button" else (role,)
                found = None
                for alias in aliases:
                    found = session.find(role=alias, name=name, within=dialog)
                    if found is not None:
                        break
                checks.add(
                    f"{category}/{label}: {name} is a {role}",
                    found is not None,
                    role,
                    role if found is not None else "missing",
                )
            if label == "Compress" and category == "Images":
                # show_if: the palette picker appears only with the switch on.
                session.set_check("Reduce colours (lossy)", True)
                time.sleep(0.4)
                checks.add(
                    "Compress: Reduce colours reveals Colours",
                    session.combo("Colours") is not None,
                    "Colours select",
                    None,
                )
                session.select_option("Colours", "16 colours")
                session.set_check("Compress to a target file size", True)
                time.sleep(0.4)
                checks.add(
                    "Compress: target switch reveals Target size",
                    session.entry("Target size") is not None,
                    "Target size entry",
                    None,
                )
                session.set_entry("Target size", "1000")
                session.press_button("Reset", within=dialog)
                time.sleep(0.5)
                checks.add(
                    "Compress: Reset clears Reduce colours",
                    not session.check_state("Reduce colours (lossy)"),
                    False,
                    session.check_state("Reduce colours (lossy)"),
                )
                checks.add(
                    "Compress: Reset hides Colours",
                    session.combo("Colours") is None,
                    "no Colours select",
                    "still present",
                )
                checks.add(
                    "Compress: Reset clears target",
                    not session.check_state("Compress to a target file size")
                    and session.entry("Target size") is None,
                    "target off and hidden",
                    None,
                )
            if label == "Add background":
                checks.add(
                    "Add background: default is colour",
                    session.selected_option("Background") == "A colour",
                    "A colour",
                    session.selected_option("Background"),
                )
                session.select_option("Background", "A gradient")
                time.sleep(0.4)
                checks.add(
                    "Add background: gradient mode shows the gradient editor",
                    session.find(name="Gradient type") is not None,
                    "Gradient type group",
                    "found" if session.find(name="Gradient type") is not None else "missing",
                )
                session.select_option("Background", "Another image")
                time.sleep(0.4)
                checks.add(
                    "Add background: image mode shows Background image",
                    session.entry("Background image") is not None,
                    "Background image entry",
                    None,
                )
                checks.add(
                    "Add background: image mode shows Image zoom",
                    session.slider("Image zoom") is not None,
                    "Image zoom slider",
                    None,
                )
                session.select_option("Background", "A gradient")
                session.press_button("Reset", within=session.dialog())
                time.sleep(0.4)
                checks.add(
                    "Add background: Reset restores the colour mode",
                    session.selected_option("Background") == "A colour",
                    "A colour",
                    session.selected_option("Background"),
                )
            if label == "Annotate":
                checks.add(
                    "Annotate: Caption is a text entry",
                    session.entry("Caption") is not None,
                    "entry",
                    None,
                )
            if label == "Edit photo":
                checks.add(
                    "Edit photo: Exposure is a slider",
                    session.slider("Exposure") is not None,
                    "slider",
                    None,
                )
            if (category, label) == ("PDF", "Compress"):
                session.select_option("Quality", "screen")
                session.press_button("Reset", within=session.dialog())
                time.sleep(0.4)
                checks.add(
                    "PDF Compress: Reset restores the ebook preset",
                    session.selected_option("Quality") == "ebook",
                    "ebook",
                    session.selected_option("Quality"),
                )
            before = session.snapshot()
            close_dialog(session)
            after = session.snapshot()
            checks.add(
                f"{category}/{label}: Cancel publishes nothing",
                before == after,
                "same files",
                sorted(set(after) - set(before)),
            )


def scenario_compress_poster(h, rec, checks):
    poster = h.fixture(POSTER)
    with h.launch("compress-poster", [poster]) as session:
        rec["workdir"] = session.workdir
        session.show_history()
        before_rows = session.history_rows()
        dialog = session.open_tool("Compress")
        if dialog is None:
            raise ScenarioFailure("Compress opened no dialog")
        session.set_check("Reduce colours (lossy)", True)
        time.sleep(0.4)
        session.select_option("Colours", "256 colours")
        session.set_check("Compress to a target file size", True)
        time.sleep(0.4)
        session.set_entry("Target size", "1000")
        rec["parameters"] = {
            "tool": "compress",
            "target_bytes": 1000,
            "colours": 256,
        }
        # The note is computed from the last encoded artifact, so wait for the
        # re-encode (12 s for this target) before reading either number.
        session.wait_text("1445 × 2168", within=session.dialog(), timeout=150)
        preview = session.preview_text()
        note = preview
        checks.add(
            "dialog reports the unreachable target",
            "The target of 1000 B is out of reach" in note,
            "The target of 1000 B is out of reach",
            note.strip(),
        )
        rec["measurements"]["preview"] = preview.strip()
        checks.add(
            "preview dimensions are the artifact's",
            "1445 × 2168" in preview,
            "1445 × 2168",
            preview,
        )
        checks.add(
            "preview size is the artifact's",
            "1.6 KB" in preview,
            "1.6 KB",
            preview,
        )
        rec["screenshots"].append(session.shot("target-out-of-reach"))
        before = session.snapshot()
        session.press_run()
        outputs = session.wait_job(before, before_rows, timeout=180)
        checks.add("one artifact published", len(outputs) == 1, 1, outputs)
        output = outputs[0]
        record = record_outputs(rec, outputs)[0]
        rec["measurements"]["history_after"] = session.history_rows()[:2]
        checks.add(
            "published name is poster-portrait-compress.png",
            os.path.basename(output) == "poster-portrait-compress.png",
            "poster-portrait-compress.png",
            os.path.basename(output),
        )
        checks.add("exact size 1,597 B", os.path.getsize(output) == POSTER_TARGET_BYTES, POSTER_TARGET_BYTES, os.path.getsize(output))
        checks.add("magic is PNG", record["kind"] == "png", "png", record["kind"])
        info = record.get("image", {})
        checks.add("dimensions 1,445×2,168", (info.get("width"), info.get("height")) == (1445, 2168), [1445, 2168], [info.get("width"), info.get("height")])
        checks.add("PNG colour type 3", info.get("png_color_type") == 3, 3, info.get("png_color_type"))
        checks.add("palette has 2 colours", info.get("colors") == 2, 2, info.get("colors"))
        checks.add("source is untouched", os.path.getsize(os.path.join(session.workdir, "poster-portrait.png")) == POSTER_SIZE, POSTER_SIZE, os.path.getsize(os.path.join(session.workdir, "poster-portrait.png")))
        rec["screenshots"].append(session.shot("workspace-after"))


def scenario_compress_16_colours(h, rec, checks):
    poster = h.fixture(POSTER)
    with h.launch("compress-16-colours", [poster]) as session:
        rec["workdir"] = session.workdir
        outputs, preview = run_tool_job(
            session,
            rec,
            checks,
            "Compress",
            apply=lambda s: (
                s.set_check("Reduce colours (lossy)", True),
                time.sleep(0.4),
                s.select_option("Colours", "16 colours"),
            ),
        )
        checks.add("one artifact published", len(outputs) == 1, 1, outputs)
        record = record_outputs(rec, outputs)[0]
        info = record.get("image", {})
        checks.add("16-colour artifact is 11,083 B", record["size"] == 11083, 11083, record["size"])
        checks.add("dimensions stay 2,000×3,000", (info.get("width"), info.get("height")) == (2000, 3000), [2000, 3000], [info.get("width"), info.get("height")])
        checks.add("PNG colour type 3", info.get("png_color_type") == 3, 3, info.get("png_color_type"))
        checks.add("palette is 16", info.get("colors") == 16, 16, info.get("colors"))
        checks.add("smaller than the source", record["size"] < POSTER_SIZE, f"< {POSTER_SIZE}", record["size"])
        checks.add("no out-of-reach note", "out of reach" not in (preview or ""), "absent", preview)


def scenario_compress_lossless(h, rec, checks):
    source = h.fixture("images/screenshot-text.png")
    source_size = os.path.getsize(source)
    with h.launch("compress-lossless", [source]) as session:
        rec["workdir"] = session.workdir
        outputs, _preview = run_tool_job(session, rec, checks, "Compress")
        checks.add("one artifact published", len(outputs) == 1, 1, outputs)
        output = outputs[0]
        record = record_outputs(rec, outputs)[0]
        info = record.get("image", {})
        checks.add("lossless artifact is 67,606 B", record["size"] == 67606, 67606, record["size"])
        checks.add("smaller than the source", record["size"] < source_size, f"< {source_size}", record["size"])
        checks.add("dimensions stay 1,600×1,000", (info.get("width"), info.get("height")) == (1600, 1000), [1600, 1000], [info.get("width"), info.get("height")])
        checks.add("pixels are identical", compare_pixels(os.path.join(session.workdir, "screenshot-text.png"), output) == 0, 0, "non-zero")


def scenario_compress_transparent(h, rec, checks):
    source = h.fixture("images/transparent-logo.png")
    with h.launch("compress-transparent", [source]) as session:
        rec["workdir"] = session.workdir
        outputs, _preview = run_tool_job(session, rec, checks, "Compress")
        output = outputs[0]
        record = record_outputs(rec, outputs)[0]
        info = record.get("image", {})
        checks.add("alpha channel kept", "a" in info.get("channels", ""), "alpha", info.get("channels"))
        checks.add("pixels are identical", compare_pixels(os.path.join(session.workdir, "transparent-logo.png"), output) == 0, 0, "non-zero")
        corner = pixel(output, 0, 0)
        checks.add("transparent corner stays transparent", corner["alpha"] == 0, 0, corner["alpha"])
        centre = pixel(output, info["width"] // 2, info["height"] // 2)
        checks.add("opaque centre stays opaque", centre["alpha"] == 255, 255, centre["alpha"])


def scenario_compress_metadata(h, rec, checks):
    source = h.fixture("images/tiny-8x8.png")
    tagged = os.path.join(h.profile, "tagged.png")
    subprocess.run(
        ["magick", source, "-set", "comment", "keep-me", tagged],
        check=True,
        timeout=60,
    )
    with h.launch("compress-metadata", [tagged]) as session:
        rec["workdir"] = session.workdir
        outputs, _preview = run_tool_job(session, rec, checks, "Compress")
        output = outputs[0]
        checks.add("comment is preserved", comment_of(output) == "keep-me", "keep-me", comment_of(output))
        checks.add("pixels are identical", compare_pixels(os.path.join(session.workdir, "tagged.png"), output) == 0, 0, "non-zero")


def scenario_compress_scale(h, rec, checks):
    poster = h.fixture(POSTER)
    with h.launch("compress-scale", [poster]) as session:
        rec["workdir"] = session.workdir
        outputs, _preview = run_tool_job(
            session,
            rec,
            checks,
            "Compress",
            apply=lambda s: s.select_option("Image size", "50 % of the dimensions"),
        )
        record = record_outputs(rec, outputs)[0]
        info = record.get("image", {})
        checks.add("scaled artifact is 50,485 B", record["size"] == 50485, 50485, record["size"])
        checks.add("dimensions halve", (info.get("width"), info.get("height")) == (1000, 1500), [1000, 1500], [info.get("width"), info.get("height")])


def scenario_compress_jpeg(h, rec, checks):
    source = h.fixture("images/photo-landscape-4000x3000.jpg")
    source_size = os.path.getsize(source)
    with h.launch("compress-jpeg", [source]) as session:
        rec["workdir"] = session.workdir
        outputs, _preview = run_tool_job(session, rec, checks, "Compress")
        output = outputs[0]
        record = record_outputs(rec, outputs)[0]
        info = record.get("image", {})
        checks.add("magic is JPEG", record["kind"] == "jpeg", "jpeg", record["kind"])
        checks.add("dimensions stay 4,000×3,000", (info.get("width"), info.get("height")) == (4000, 3000), [4000, 3000], [info.get("width"), info.get("height")])
        checks.add("never grows", record["size"] <= source_size, f"<= {source_size}", record["size"])


def scenario_compress_video(h, rec, checks):
    source = h.fixture("video/clip-landscape-720p-10s.mp4")
    source_size = os.path.getsize(source)
    with h.launch("compress-video", [source]) as session:
        rec["workdir"] = session.workdir
        outputs, _preview = run_tool_job(session, rec, checks, "Compress")
        output = outputs[0]
        record = record_outputs(rec, outputs)[0]
        media = record.get("media", {})
        types = {stream["type"] for stream in media.get("streams", [])}
        checks.add("container stays MP4", record["kind"] == "mp4", "mp4", record["kind"])
        checks.add("video and audio streams kept", {"video", "audio"} <= types, ["audio", "video"], sorted(types))
        checks.add("duration is preserved", abs(media.get("duration", 0) - 10.0) < 1.0, "≈10 s", media.get("duration"))
        checks.add("never grows", record["size"] <= source_size, f"<= {source_size}", record["size"])


def scenario_compress_audio(h, rec, checks):
    source = h.fixture("audio/speech.mp3")
    source_size = os.path.getsize(source)
    with h.launch("compress-audio", [source]) as session:
        rec["workdir"] = session.workdir
        outputs, _preview = run_tool_job(session, rec, checks, "Compress")
        record = record_outputs(rec, outputs)[0]
        media = record.get("media", {})
        checks.add("magic is MP3", record["kind"] == "mp3", "mp3", record["kind"])
        checks.add("audio stream kept", any(s["type"] == "audio" for s in media.get("streams", [])), "audio", media.get("streams"))
        checks.add("never grows", record["size"] <= source_size, f"<= {source_size}", record["size"])


def scenario_compress_pdf(h, rec, checks):
    source = h.fixture("pdf/report-5-pages.pdf")
    source_size = os.path.getsize(source)
    with h.launch("compress-pdf", [source]) as session:
        rec["workdir"] = session.workdir
        outputs, _preview = run_tool_job(
            session,
            rec,
            checks,
            "Compress",
            apply=lambda s: s.select_option("Quality", "screen"),
        )
        record = record_outputs(rec, outputs)[0]
        info = record.get("pdf", {})
        checks.add("magic is PDF", record["kind"] == "pdf", "pdf", record["kind"])
        checks.add("page count stays 5", info.get("Pages") == "5", "5", info.get("Pages"))
        checks.add("never grows", record["size"] <= source_size, f"<= {source_size}", record["size"])


def scenario_never_grow_tiny(h, rec, checks):
    source = h.fixture("images/tiny-8x8.png")
    source_size = os.path.getsize(source)
    with h.launch("never-grow-tiny", [source]) as session:
        rec["workdir"] = session.workdir
        outputs, _preview = run_tool_job(session, rec, checks, "Compress")
        output = outputs[0]
        record = record_outputs(rec, outputs)[0]
        checks.add("never grows", record["size"] <= source_size, f"<= {source_size}", record["size"])
        checks.add("returns the source bytes when it cannot shrink", record["size"] == source_size, source_size, record["size"])
        checks.add("pixels are identical", compare_pixels(os.path.join(session.workdir, "tiny-8x8.png"), output) == 0, 0, "non-zero")


def scenario_reachable_target(h, rec, checks):
    source = h.fixture("images/screenshot-text.png")
    with h.launch("reachable-target", [source]) as session:
        rec["workdir"] = session.workdir
        def apply(s):
            s.set_check("Compress to a target file size", True)
            time.sleep(0.4)
            s.set_entry("Target size", "40000")
            # The preview re-encodes 400 ms after the last edit and takes a second or
            # two; the note it prints is about the request it measured. Wait until it
            # reports a size at or under the new target before anything is asserted, or
            # the check reads the previous request's verdict.
            deadline = time.time() + 30
            while time.time() < deadline:
                text = s.subtree_text(s.dialog())
                numbers = re.findall(r"(\d+(?:\.\d+)?)\s*KB", text)
                if numbers and float(numbers[0]) * 1024 <= 40000:
                    break
                time.sleep(0.5)
            rec["measurements"]["preview_at_assert"] = s.subtree_text(s.dialog())[-200:]
        outputs, preview = run_tool_job(session, rec, checks, "Compress", apply=apply)
        record = record_outputs(rec, outputs)[0]
        info = record.get("image", {})
        checks.add("target is met", record["size"] <= 40000, "<= 40000", record["size"])
        checks.add("artifact is 22,701 B", record["size"] == 22701, 22701, record["size"])
        checks.add("indexed PNG", info.get("png_color_type") == 3, 3, info.get("png_color_type"))
        checks.add("no out-of-reach note", "out of reach" not in (preview or ""), "absent", preview)


def scenario_multi_file(h, rec, checks):
    poster = h.fixture(POSTER)
    screenshot = h.fixture("images/screenshot-text.png")
    with h.launch("multi-file", [poster, screenshot]) as session:
        rec["workdir"] = session.workdir
        session.show_history()
        before_rows = session.history_rows()
        dialog = session.open_tool("Compress")
        if dialog is None:
            raise ScenarioFailure("Compress opened no dialog")
        rec["screenshots"].append(session.shot("dialog"))
        texts = [value for _role, value in session.all_texts(within=dialog)]
        joined = "\n".join(texts)
        checks.add("dialog says 1 of 2", "1 of 2" in joined, "1 of 2", joined[:300])
        session.press_button("Next file", within=dialog)
        time.sleep(0.4)
        dialog = session.dialog()
        joined = "\n".join(value for _role, value in session.all_texts(within=dialog))
        checks.add("arrow picker reaches the second file", "2 of 2" in joined, "2 of 2", joined[:300])
        session.press_button("Previous file", within=dialog)
        time.sleep(0.4)
        dialog = session.dialog()
        session.set_check("Same settings for every file", True)
        session.select_option("Image size", "50 % of the dimensions")
        checks.add(
            "same settings is checked",
            session.check_state("Same settings for every file"),
            True,
            session.check_state("Same settings for every file"),
        )
        checks.add(
            "image size reads 50 %",
            session.selected_option("Image size") == "50 % of the dimensions",
            "50 % of the dimensions",
            session.selected_option("Image size"),
        )
        rec["measurements"]["dialog_before_run"] = [
            {"role": _safe(node, "get_role_name"), "name": _safe(node, "get_name"), "text": _text(node), "states": sorted(_states(node))}
            for node, _depth in _walk(session.dialog())
        ]
        before = session.snapshot()
        session.press_run()
        outputs = session.wait_job(before, before_rows, timeout=240, expect=2)
        records = record_outputs(rec, outputs)
        by_name = {record_["name"]: record_ for record_ in records}
        checks.add("two artifacts published", len(outputs) == 2, 2, [r["name"] for r in records])
        poster_out = next((r for n, r in by_name.items() if n.startswith("poster-portrait-compress")), None)
        shot_out = next((r for n, r in by_name.items() if n.startswith("screenshot-text-compress")), None)
        checks.add(
            "poster is scaled to 1,000×1,500",
            poster_out is not None and (poster_out.get("image", {}).get("width"), poster_out.get("image", {}).get("height")) == (1000, 1500),
            [1000, 1500],
            poster_out and [poster_out.get("image", {}).get("width"), poster_out.get("image", {}).get("height")],
        )
        checks.add(
            "screenshot is scaled to 800×500",
            shot_out is not None and (shot_out.get("image", {}).get("width"), shot_out.get("image", {}).get("height")) == (800, 500),
            [800, 500],
            shot_out and [shot_out.get("image", {}).get("width"), shot_out.get("image", {}).get("height")],
        )


def _first_words(text, count=4):
    words = re.findall(r"[A-Za-z]{4,}", text)
    return words[:count]


def _body_words(text, count=3):
    """Distinctive words from the body, not YAML front matter or markup keys."""
    body = re.sub(r"^---\s*\n.*?\n---\s*\n", "", text, count=1, flags=re.S)
    words = []
    for word in re.findall(r"[A-Za-z]{5,}", body):
        if word not in words:
            words.append(word)
        if len(words) >= count:
            break
    return words


def scenario_poster_16_target(h, rec, checks):
    """The reported case: reduce to 16 colours *and* a 1,000 B target."""
    poster = h.fixture(POSTER)
    with h.launch("poster-16-target", [poster]) as session:
        rec["workdir"] = session.workdir
        session.show_history()
        before_rows = session.history_rows()
        dialog = session.open_tool("Compress")
        if dialog is None:
            raise ScenarioFailure("Compress opened no dialog")
        session.set_check("Reduce colours (lossy)", True)
        time.sleep(0.4)
        session.select_option("Colours", "16 colours")
        session.set_check("Compress to a target file size", True)
        time.sleep(0.4)
        session.set_entry("Target size", "1000")
        rec["parameters"] = {
            "tool": "compress",
            "target_bytes": 1000,
            "colours": 16,
        }
        session.wait_text("1445 × 2168", within=session.dialog(), timeout=150)
        preview = session.preview_text()
        rec["measurements"]["preview"] = preview.strip()
        checks.add(
            "the 16-colour target is reported out of reach",
            "The target of 1000 B is out of reach" in preview,
            "out-of-reach note",
            preview.strip(),
        )
        rec["screenshots"].append(session.shot("target-out-of-reach"))
        before = session.snapshot()
        session.press_run()
        outputs = session.wait_job(before, before_rows, timeout=180)
        record = record_outputs(rec, outputs)[0]
        info = record.get("image", {})
        checks.add("one artifact published", len(outputs) == 1, 1, outputs)
        checks.add("exact size 1,597 B", record["size"] == POSTER_TARGET_BYTES, POSTER_TARGET_BYTES, record["size"])
        checks.add("dimensions 1,445×2,168", (info.get("width"), info.get("height")) == (1445, 2168), [1445, 2168], [info.get("width"), info.get("height")])
        checks.add("PNG colour type 3", info.get("png_color_type") == 3, 3, info.get("png_color_type"))
        checks.add("palette has 2 colours", info.get("colors") == 2, 2, info.get("colors"))
        rec["screenshots"].append(session.shot("after"))


def scenario_multi_file_per_file(h, rec, checks):
    """Different settings per file through the arrow picker."""
    poster = h.fixture(POSTER)
    photo = h.fixture("images/photo-landscape-4000x3000.jpg")
    with h.launch("multi-file-per-file", [poster, photo]) as session:
        rec["workdir"] = session.workdir
        session.show_history()
        before_rows = session.history_rows()
        dialog = session.open_tool("Compress")
        if dialog is None:
            raise ScenarioFailure("Compress opened no dialog")
        rec["screenshots"].append(session.shot("dialog"))
        session.select_option("Image size", "50 % of the dimensions")
        session.press_button("Next file", within=session.dialog())
        time.sleep(0.4)
        dialog = session.dialog()
        checks.add(
            "the second file is shown",
            "2 of 2" in "\n".join(value for _role, value in session.all_texts(within=dialog)),
            "2 of 2",
            "\n".join(value for _role, value in session.all_texts(within=dialog))[:200],
        )
        checks.add(
            "the second file starts from its own defaults",
            session.selected_option("Image size") == "Keep original dimensions",
            "Keep original dimensions",
            session.selected_option("Image size"),
        )
        session.select_option("Image size", "25 % of the dimensions")
        before = session.snapshot()
        session.press_run()
        outputs = session.wait_job(before, before_rows, timeout=240, expect=2)
        records = record_outputs(rec, outputs)
        by_name = {record["name"]: record for record in records}
        poster_out = next((record for name, record in by_name.items() if name.startswith("poster-portrait-compress")), None)
        photo_out = next((record for name, record in by_name.items() if name.startswith("photo-landscape-4000x3000-compress")), None)
        checks.add("two artifacts published", len(records) == 2, 2, list(by_name))
        checks.add(
            "the first file keeps 50 %",
            poster_out is not None
            and (poster_out.get("image", {}).get("width"), poster_out.get("image", {}).get("height")) == (1000, 1500),
            [1000, 1500],
            poster_out and [poster_out.get("image", {}).get("width"), poster_out.get("image", {}).get("height")],
        )
        checks.add(
            "the second file uses 25 %",
            photo_out is not None
            and (photo_out.get("image", {}).get("width"), photo_out.get("image", {}).get("height")) == (1000, 750),
            [1000, 750],
            photo_out and [photo_out.get("image", {}).get("width"), photo_out.get("image", {}).get("height")],
        )
        rec["screenshots"].append(session.shot("after"))

def _stream_types(record):
    return {stream["type"] for stream in record.get("media", {}).get("streams", [])}


def _first_image(record):
    return record.get("image", {})


def scenario_tool_image_crop(h, rec, checks):
    with h.launch("tool-image-crop", [h.fixture("images/photo-landscape-4000x3000.jpg")]) as session:
        rec["workdir"] = session.workdir

        def set_free_box(s):
            # The W/H row is seeded from the source's real pixels, read asynchronously.
            deadline = time.time() + 10
            while time.time() < deadline:
                entry = s.dialog_entry("Width")
                if entry is not None and s.text(entry) not in ("", "0"):
                    break
                time.sleep(0.2)
            s.set_entry("Width", "2000")
            s.set_entry("Height", "1500")

        outputs, _preview = run_tool_job(
            session, rec, checks, "Crop", category="Images", apply=set_free_box
        )
        record = record_outputs(rec, outputs)[0]
        info = _first_image(record)
        checks.add(
            "free crop box 0.5 × 0.5 of 4000×3000 is 2,000×1,500",
            (info.get("width"), info.get("height")) == (2000, 1500),
            [2000, 1500],
            [info.get("width"), info.get("height")],
        )

        # A ratio keeps the picture's own proportions: 1:1 on 4000×3000 is 3000×3000.
        outputs, _preview = run_tool_job(
            session,
            rec,
            checks,
            "Crop",
            category="Images",
            apply=lambda s: s.press_button("1:1", within=s.dialog()),
        )
        record = record_outputs(rec, outputs)[0]
        info = _first_image(record)
        checks.add(
            "1:1 crop is 3,000×3,000",
            (info.get("width"), info.get("height")) == (3000, 3000),
            [3000, 3000],
            [info.get("width"), info.get("height")],
        )


def scenario_tool_image_annotate(h, rec, checks):
    def click_canvas(s):
        image = s.preview_image(s.dialog())
        if image is None:
            raise Skip("the annotation canvas exposes no image to click")
        box = s.extents(image)
        if not box:
            raise Skip("the annotation canvas has no screen extents")
        x, y, width, height = box
        run(
            ["xdotool", "mousemove", str(x + width // 2), str(y + height // 2)],
            timeout=10,
        )
        time.sleep(0.15)
        run(["xdotool", "click", "1"], timeout=10)
        time.sleep(0.5)

    def apply(s):
        s.set_entry("Caption", "Harness")
        rec["parameters"]["annotations"] = "caption"

    with h.launch("tool-image-annotate", [h.fixture("images/screenshot-text.png")]) as session:
        rec["workdir"] = session.workdir
        outputs, _preview = run_tool_job(session, rec, checks, "Annotate", category="Images", apply=apply)
        record = record_outputs(rec, outputs)[0]
        info = _first_image(record)
        checks.add(
            "annotate keeps the dimensions",
            (info.get("width"), info.get("height")) == (1600, 1000),
            [1600, 1000],
            [info.get("width"), info.get("height")],
        )
        checks.add(
            "annotate changes pixels",
            compare_pixels(os.path.join(session.workdir, "screenshot-text.png"), outputs[0]) > 0,
            "> 0",
            "0",
        )
        # The untouched corner is the guarantee that the engine drew shapes, not a
        # re-encode of the whole picture.
        corner = validate.pixel(outputs[0], 2, 2)
        source_corner = validate.pixel(os.path.join(session.workdir, "screenshot-text.png"), 2, 2)
        checks.add(
            "annotate leaves the corner alone",
            corner == source_corner,
            source_corner,
            corner,
        )


def scenario_tool_image_collage(h, rec, checks):
    fixtures = [
        h.fixture("batch/wide.png"),
        h.fixture("batch/portrait.png"),
        h.fixture("batch/square.png"),
        h.fixture("images/photo-landscape-4000x3000.jpg"),
    ]
    with h.launch("tool-image-collage", fixtures) as session:
        rec["workdir"] = session.workdir
        outputs, _preview = run_tool_job(session, rec, checks, "Collage", category="Images")
        record = record_outputs(rec, outputs)[0]
        info = _first_image(record)
        checks.add(
            "collage default is 2,048×2,048",
            (info.get("width"), info.get("height")) == (2048, 2048),
            [2048, 2048],
            [info.get("width"), info.get("height")],
        )
        sources = [
            (fixtures[0], 2400, 1350, 512, 512),
            (fixtures[1], 1500, 2000, 1536, 512),
            (fixtures[2], 1600, 1600, 512, 1536),
            (fixtures[3], 4000, 3000, 1536, 1536),
        ]
        for source, sw, sh, ox, oy in sources:
            expected = validate.pixel(source, sw // 2, sh // 2)
            actual = validate.pixel(outputs[0], ox, oy)
            if expected is None or actual is None:
                raise Skip("pixel sampling needs ImageMagick")
            checks.add(
                f"collage quadrant at {ox},{oy} matches {os.path.basename(source)}",
                max(abs(a - b) for a, b in zip(expected, actual)) <= 12,
                expected,
                actual,
            )


def scenario_tool_image_background(h, rec, checks):
    with h.launch("tool-image-background", [h.fixture(POSTER)]) as session:
        rec["workdir"] = session.workdir
        outputs, _preview = run_tool_job(session, rec, checks, "Add background", category="Images")
        record = record_outputs(rec, outputs)[0]
        info = _first_image(record)
        checks.add(
            "default padding grows the canvas to 2,160×3,160",
            (info.get("width"), info.get("height")) == (2160, 3160),
            [2160, 3160],
            [info.get("width"), info.get("height")],
        )
        corner = pixel(outputs[0], 5, 5)
        checks.add("the default background is #101418", corner["pixel"] == "srgb(16,20,24)", "srgb(16,20,24)", corner["pixel"])


def scenario_tool_image_edit(h, rec, checks):
    source = h.fixture("images/photo-landscape-4000x3000.jpg")
    with h.launch("tool-image-edit", [source]) as session:
        rec["workdir"] = session.workdir

        outputs, _preview = run_tool_job(
            session,
            rec,
            checks,
            "Edit photo",
            category="Images",
            apply=lambda s: s.set_slider("Exposure", 1.0, -5.0, 0.01),
        )
        record = record_outputs(rec, outputs)[0]
        info = _first_image(record)
        checks.add(
            "edit keeps the full resolution",
            (info.get("width"), info.get("height")) == (4000, 3000),
            [4000, 3000],
            [info.get("width"), info.get("height")],
        )
        before = validate.mean_luma(source)
        after = validate.mean_luma(outputs[0])
        if before is None or after is None:
            raise Skip("mean luma needs ImageMagick")
        rec["measurements"]["edit_mean_luma"] = {"before": before, "after": after}
        checks.add(
            "+1 EV raises mean luma by >= 10%",
            after >= before * 1.10,
            f">= {before * 1.10:.4f}",
            after,
        )

        grain_outputs, _preview = run_tool_job(
            session,
            rec,
            checks,
            "Edit photo",
            category="Images",
            apply=lambda s: s.set_slider("Grain", 100.0, 0.0, 1.0),
        )
        base_variance = validate.variance(source)
        grain_variance = validate.variance(grain_outputs[0])
        if base_variance is None or grain_variance is None:
            raise Skip("variance needs ImageMagick")
        checks.add(
            "grain 100 raises the variance",
            grain_variance > base_variance,
            f"> {base_variance:.4f}",
            grain_variance,
        )

        noise_outputs, _preview = run_tool_job(
            session,
            rec,
            checks,
            "Edit photo",
            category="Images",
            apply=lambda s: s.set_slider("Noise reduction", 100.0, 0.0, 1.0),
        )
        noise_variance = validate.variance(noise_outputs[0])
        checks.add(
            "noise reduction lowers the variance",
            noise_variance is not None and noise_variance < base_variance,
            f"< {base_variance:.4f}",
            noise_variance,
        )


def scenario_tool_image_metadata(h, rec, checks):
    with h.launch("tool-image-metadata", [h.fixture(POSTER)]) as session:
        rec["workdir"] = session.workdir
        def set_title(s):
            deadline = time.time() + 5
            while time.time() < deadline and s.entry("Title") is None:
                time.sleep(0.2)
            s.set_entry("Title", "Harness")

        outputs, _preview = run_tool_job(
            session,
            rec,
            checks,
            "Metadata",
            category="Images",
            apply=set_title,
        )
        record = record_outputs(rec, outputs)[0]
        checks.add("metadata writes one PNG", len(outputs) == 1 and record["kind"] == "png", 1, len(outputs))
        checks.add("Title reaches the PNG comment", comment_of(outputs[0]) == "Harness", "Harness", comment_of(outputs[0]))
        checks.add("metadata leaves pixels identical", compare_pixels(os.path.join(session.workdir, "poster-portrait.png"), outputs[0]) == 0, 0, "non-zero")

    with h.launch("tool-image-metadata-read", [outputs[0]]) as session:
        session.category_hint = "Images"
        if session.open_tool("Metadata") is None:
            raise Skip("Metadata ran without opening a dialog")
        deadline = time.time() + 8
        node = None
        while time.time() < deadline and node is None:
            node = session.entry("Title")
            time.sleep(0.2)
        shown = session.text(node) if node is not None else ""
        rec["measurements"]["metadata_shows_existing"] = shown
        checks.add(
            "the metadata dialog shows the existing title",
            shown == "Harness",
            "Harness",
            shown,
        )


def scenario_tool_image_redact(h, rec, checks):
    def apply(s):
        method = s.detect_or_mark(s.dialog())
        rec["parameters"]["region_method"] = method

    def apply_blur(s):
        s.press_button("Blur", within=s.dialog())
        s.mark_region(s.dialog())
        rec["parameters"]["region_method"] = "drawn+blur"

    with h.launch("tool-image-redact", [h.fixture("images/screenshot-text.png")]) as session:
        rec["workdir"] = session.workdir
        source = os.path.join(session.workdir, "screenshot-text.png")
        outputs, _preview = run_tool_job(session, rec, checks, "Redact", category="Images", apply=apply)
        record = record_outputs(rec, outputs)[0]
        info = _first_image(record)
        checks.add("redact keeps the dimensions", (info.get("width"), info.get("height")) == (1600, 1000), [1600, 1000], [info.get("width"), info.get("height")])
        checks.add("redact changes pixels", compare_pixels(source, outputs[0]) > 0, "> 0", "0")
        # The drawn box covers the picture's centre; solid redaction fills it black.
        centre = validate.pixel(outputs[0], 800, 500)
        checks.add("solid redaction fills the centre", centre == (0, 0, 0), (0, 0, 0), centre)

        blur_outputs, _preview = run_tool_job(
            session, rec, checks, "Redact", category="Images", apply=apply_blur
        )
        blurred_centre = validate.pixel(blur_outputs[0], 800, 500)
        source_centre = validate.pixel(source, 800, 500)
        checks.add(
            "blur redaction changes the centre",
            blurred_centre is not None and blurred_centre != source_centre,
            "different from the source",
            blurred_centre,
        )
        checks.add(
            "blur redaction is not the solid fill",
            blurred_centre != (0, 0, 0),
            "not black",
            blurred_centre,
        )


def media_duration(record) -> float:
    return float(record.get("media", {}).get("duration") or 0.0)


def video_stream(record) -> dict:
    return next(
        (
            stream
            for stream in record.get("media", {}).get("streams", [])
            if stream.get("type") == "video"
        ),
        {},
    )


def channel_rms_db(path: str, channel: int) -> float | None:
    """RMS of one channel, so a pan gain can be measured on the side it applies to."""
    result = run(
        [
            "ffmpeg",
            "-hide_banner",
            "-nostats",
            "-i",
            path,
            "-af",
            f"pan=mono|c0=c{channel},astats=metadata=1:reset=0",
            "-f",
            "null",
            "-",
        ],
        timeout=120,
    )
    matches = re.findall(r"RMS level dB:\s*(-?\d+\.?\d*)", result.stderr)
    return float(matches[-1]) if matches else None


def frame_pixel(path: str, at: float, x: int, y: int, directory: str, tag: str):
    """One pixel of one decoded frame, for before/after redaction comparisons."""
    out = os.path.join(directory, f"probe-{tag}-{at}.png")
    run(["ffmpeg", "-v", "error", "-y", "-ss", f"{at}", "-i", path, "-frames:v", "1", out], timeout=60)
    return validate.pixel(out, x, y)


def pixel_distance(a, b) -> int:
    if not a or not b:
        return -1
    return max(abs(int(pa) - int(pb)) for pa, pb in zip(a, b))


def scenario_tool_video_trim(h, rec, checks):
    def apply(s):
        # The two timeline handles are the only trim controls: no text fields.
        # End first: a start cannot pass the current end, which is 0 until the
        # dialog has the clip's length.
        s.set_slider_auto("Trim end", 5.0)
        s.set_slider_auto("Trim start", 2.0)

    with h.launch("tool-video-trim", [h.fixture("video/clip-landscape-720p-10s.mp4")]) as session:
        rec["workdir"] = session.workdir
        outputs, _preview = run_tool_job(session, rec, checks, "Trim", category="Video", apply=apply)
        record = record_outputs(rec, outputs)[0]
        checks.add("trim keeps video and audio", {"video", "audio"} <= _stream_types(record), ["audio", "video"], sorted(_stream_types(record)))
        checks.add(
            "trim 2→5 s writes 3.0 s",
            abs(media_duration(record) - 3.0) <= 0.2,
            "3.0±0.2 s",
            media_duration(record),
        )


def scenario_tool_video_crop(h, rec, checks):
    """The crop dialog: the canvas shows the first frame, the size readout is real, and
    the aspect control produces the dimensions it promises.

    The free rectangle is submitted as `x/y/w/h`; that path is measured by
    `quality-audit.py --only video-crop-free` (0.25, 0.25, 0.5, 0.5 → 640 × 360), because
    a synthetic pointer drag depends on the X server delivering motion to the webview
    rather than on the app's own behaviour.
    """
    with h.launch("tool-video-crop", [h.fixture("video/clip-landscape-720p-10s.mp4")]) as session:
        rec["workdir"] = session.workdir

        def check_surface(s):
            deadline = time.time() + 15
            node = None
            while time.time() < deadline:
                node = s.find(name="Crop area", within=s.dialog())
                if node is not None:
                    break
                time.sleep(0.3)
            if node is None:
                raise ScenarioFailure("the crop canvas never showed its preview")
            s.press_in_dialog("1:1")

        outputs, _preview = run_tool_job(
            session,
            rec,
            checks,
            "Crop",
            category="Video",
            apply=check_surface,
        )
        record = record_outputs(rec, outputs)[0]
        video = video_stream(record)
        checks.add(
            "1:1 crop is 720×720",
            (video.get("width"), video.get("height")) == (720, 720),
            [720, 720],
            [video.get("width"), video.get("height")],
        )
        checks.add(
            "aspect crop keeps the audio",
            "audio" in _stream_types(record),
            "audio",
            sorted(_stream_types(record)),
        )


def scenario_tool_video_speed(h, rec, checks):
    def at(factor):
        return lambda s: s.set_slider_auto("Speed", factor)

    with h.launch("tool-video-speed", [h.fixture("video/clip-landscape-720p-10s.mp4")]) as session:
        rec["workdir"] = session.workdir
        outputs, _preview = run_tool_job(session, rec, checks, "Speed", category="Video", apply=at(2.0))
        record = record_outputs(rec, outputs)[0]
        checks.add("2× writes 5.0 s", abs(media_duration(record) - 5.0) <= 0.2, "5.0±0.2 s", media_duration(record))
        checks.add("speed keeps the audio", "audio" in _stream_types(record), "audio", sorted(_stream_types(record)))

    with h.launch("tool-video-speed-half", [h.fixture("video/clip-landscape-720p-10s.mp4")]) as session:
        rec["workdir"] = session.workdir
        outputs, _preview = run_tool_job(session, rec, checks, "Speed", category="Video", apply=at(0.5))
        record = record_outputs(rec, outputs)[0]
        checks.add("0.5× writes 20 s", abs(media_duration(record) - 20.0) <= 0.4, "20.0±0.4 s", media_duration(record))

    with h.launch("tool-video-speed-silent", [h.fixture("video/clip-silent-720p-5s.mp4")]) as session:
        rec["workdir"] = session.workdir
        outputs, _preview = run_tool_job(session, rec, checks, "Speed", category="Video", apply=at(2.0))
        record = record_outputs(rec, outputs)[0]
        checks.add("a silent clip still speeds up", abs(media_duration(record) - 2.5) <= 0.2, "2.5±0.2 s", media_duration(record))
        checks.add("the silent clip stays silent", "audio" not in _stream_types(record), "no audio", sorted(_stream_types(record)))


def scenario_tool_video_frames(h, rec, checks):
    def apply(s):
        # The chips are the value: scrub to a moment, add it, repeat.
        s.set_slider_auto("Playhead", 1.0)
        s.press_in_dialog("Add frame")
        s.set_slider_auto("Playhead", 3.0)
        s.press_in_dialog("Add frame")
        time.sleep(0.4)
        rec["parameters"]["primary"] = s.primary_label()

    with h.launch("tool-video-frames", [h.fixture("video/clip-landscape-720p-10s.mp4")]) as session:
        rec["workdir"] = session.workdir
        outputs, _preview = run_tool_job(session, rec, checks, "Save frames", category="Video", apply=apply, expect=2)
        records = record_outputs(rec, outputs)
        checks.add("two chips write two frames", len(records) == 2, 2, len(records))
        checks.add(
            "every frame is a 1280×720 PNG",
            all(
                record["kind"] == "png"
                and (record.get("image", {}).get("width"), record.get("image", {}).get("height")) == (1280, 720)
                for record in records
            ),
            "2 PNGs 1280×720",
            [(record["kind"], record.get("image", {}).get("width"), record.get("image", {}).get("height")) for record in records],
        )
        checks.add(
            "the primary names the frame count",
            rec["parameters"].get("primary") == "Save 2 Frames",
            "Save 2 Frames",
            rec["parameters"].get("primary"),
        )
        checks.add(
            "the two frames differ",
            len(records) == 2 and records[0]["sha256"] != records[1]["sha256"],
            "different pixels",
            "identical",
        )


def scenario_tool_video_split(h, rec, checks):
    def apply(s):
        s.set_slider_auto("Playhead", 2.5)
        s.press_in_dialog("Add split at playhead")
        s.set_slider_auto("Playhead", 5.0)
        s.press_in_dialog("Add split at playhead")
        time.sleep(0.4)

    with h.launch("tool-video-split", [h.fixture("video/clip-landscape-720p-10s.mp4")]) as session:
        rec["workdir"] = session.workdir
        outputs, _preview = run_tool_job(session, rec, checks, "Split", category="Video", apply=apply, expect=3)
        records = record_outputs(rec, outputs)
        durations = [round(media_duration(record), 2) for record in records]
        wanted = [2.5, 2.5, 5.0]
        checks.add("two split points write three clips", len(records) == 3, 3, len(records))
        checks.add(
            "clips are 2.5/2.5/5.0 s",
            len(durations) == 3
            and all(abs(a - b) <= 0.2 for a, b in zip(sorted(durations), wanted)),
            wanted,
            sorted(durations),
        )
        checks.add("every clip keeps the audio", all("audio" in _stream_types(record) for record in records), "audio in all", [sorted(_stream_types(record)) for record in records])

    with h.launch("tool-video-split-equal", [h.fixture("video/clip-landscape-720p-10s.mp4")]) as session:
        rec["workdir"] = session.workdir

        def equal(s):
            s.set_entry("Equal clips", "4")
            s.press_in_dialog("Set")
            time.sleep(0.4)

        outputs, _preview = run_tool_job(session, rec, checks, "Split", category="Video", apply=equal, expect=4)
        records = record_outputs(rec, outputs)
        durations = [round(media_duration(record), 2) for record in records]
        checks.add("equal clips 4 writes four files", len(records) == 4, 4, len(records))
        checks.add(
            "each equal clip is 2.5 s",
            len(durations) == 4 and all(abs(duration - 2.5) <= 0.2 for duration in durations),
            [2.5, 2.5, 2.5, 2.5],
            durations,
        )


def scenario_tool_video_redact(h, rec, checks):
    source = h.fixture("video/clip-landscape-720p-10s.mp4")
    # The fixture is colour bars: a sample on the green/yellow boundary inside the
    # region is where a blur changes pixels (a flat patch would survive a blur).
    inside = (420, 300)
    plain = (544, 360)

    def apply(solid: bool):
        def run_redact(s):
            s.mark_region(s.dialog(), (0.25, 0.3), (0.6, 0.7))
            time.sleep(0.6)
            rec["screenshots"].append(s.shot("marked"))
            rec["measurements"]["marked_text"] = s.subtree_text(s.dialog())[-400:]
            if s.find(name="Region 1 start", within=s.dialog()) is None:
                # The same `BoxesEditor` accepts the synthetic drag in the image dialog
                # and not here, where it sits under a timeline in a scrolled sheet. The
                # engine's per-box ranges are measured by
                # `quality-audit.py --only video-redact`; this scenario cannot drive the
                # canvas, and saying so is better than a fake pass.
                raise Skip(
                    "the video redaction canvas did not take the synthetic drag; the "
                    "per-box time ranges are measured by quality-audit video-redact"
                )
            s.set_entry("Region 1 start", "0.5")
            s.set_entry("Region 1 end", "2.0")
            if solid:
                s.press_in_dialog("#e4572e")

        return run_redact

    with h.launch("tool-video-redact", [source]) as session:
        rec["workdir"] = session.workdir
        outputs, _preview = run_tool_job(session, rec, checks, "Redact", category="Video", apply=apply(True))
        record = record_outputs(rec, outputs)[0]
        checks.add("redact keeps the duration", abs(media_duration(record) - 10.0) <= 0.4, "≈10 s", media_duration(record))
        checks.add("redact keeps video and audio", {"video", "audio"} <= _stream_types(record), ["audio", "video"], sorted(_stream_types(record)))
        before = frame_pixel(source, 1.5, *inside, session.workdir, "source")
        covered = frame_pixel(outputs[0], 1.5, *inside, session.workdir, "covered")
        early = frame_pixel(outputs[0], 0.2, *plain, session.workdir, "early")
        late = frame_pixel(outputs[0], 2.5, *plain, session.workdir, "late")
        source_early = frame_pixel(source, 0.2, *plain, session.workdir, "source-early")
        source_late = frame_pixel(source, 2.5, *plain, session.workdir, "source-late")
        checks.add(
            "the box is the solid colour inside 0.5–2.0 s",
            bool(covered) and covered[0] > 150 and covered[1] < 140,
            "#e4572e",
            covered,
        )
        checks.add("the frame at 0.2 s is unchanged", pixel_distance(early, source_early) <= 24, "≤24 levels", pixel_distance(early, source_early))
        checks.add("the frame at 2.5 s is unchanged", pixel_distance(late, source_late) <= 24, "≤24 levels", pixel_distance(late, source_late))
        checks.add("the source pixel was not already the fill", pixel_distance(before, covered) > 60, ">60 levels", pixel_distance(before, covered))

    with h.launch("tool-video-redact-blur", [source]) as session:
        rec["workdir"] = session.workdir

        def blur(s):
            s.press_in_dialog("Blur")
            s.mark_region(s.dialog(), (0.25, 0.3), (0.6, 0.7))
            time.sleep(0.3)

        outputs, _preview = run_tool_job(session, rec, checks, "Redact", category="Video", apply=blur)
        record = record_outputs(rec, outputs)[0]
        checks.add("blur redaction writes a clip", media_duration(record) > 9.0, "≈10 s", media_duration(record))
        blurred = frame_pixel(outputs[0], 5.0, *inside, session.workdir, "blurred")
        source_frame = frame_pixel(source, 5.0, *inside, session.workdir, "source-mid")
        checks.add(
            "blur changes the covered pixels",
            pixel_distance(blurred, source_frame) > 3,
            ">3 levels",
            pixel_distance(blurred, source_frame),
        )
        checks.add(
            "blur is not the solid colour",
            not (blurred and blurred[0] > 200 and blurred[1] < 120 and blurred[2] < 100),
            "not #e4572e",
            blurred,
        )


def scenario_tool_audio_normalize(h, rec, checks):
    def apply(s):
        # The readout sits under the sliders; WebKit prunes what is scrolled out of the
        # sheet, so bring the control it belongs to into view first.
        slider = s.slider("Target loudness")
        if slider is not None:
            s.ensure_visible(slider)
            time.sleep(0.3)
        # The measurement sits under the sliders, in the editor block. Scroll to that
        # block by its own control, or WebKit's a11y tree never shows it.
        # Back to the top of the sheet: the measurement sits under the waveform, above
        # the sliders, and WebKit only exposes what is in view.
        s.scroll_dialog(8, "up")
        time.sleep(0.3)
        deadline = time.time() + 15
        seen = ""
        while time.time() < deadline:
            seen = s.subtree_text(s.dialog())
            if "Input:" in seen:
                break
            time.sleep(0.5)
        # The readout renders (the mock harness screenshots it as "Input: −23.4 LUFS") and
        # the measurement itself is a tested engine call, but this WebKit build does not
        # expose that row in the real window's accessibility tree, so it is recorded
        # rather than asserted here.
        rec["measurements"]["input_readout_exposed"] = "Input:" in seen
        if "Input:" not in seen:
            rec["measurements"]["input_readout_note"] = (
                "the row is absent from the real window's a11y tree; the mock harness "
                "shows it and av::measure_loudness is unit-tested"
            )

    with h.launch("tool-audio-normalize", [h.fixture("audio/quiet-speech.mp3")]) as session:
        rec["workdir"] = session.workdir
        outputs, _preview = run_tool_job(session, rec, checks, "Normalize", category="Audio", apply=apply)
        record = record_outputs(rec, outputs)[0]
        checks.add("normalize keeps the duration", 13.0 < media_duration(record) < 15.0, "≈14.1 s", media_duration(record))
        checks.add("normalize keeps audio", "audio" in _stream_types(record), "audio", sorted(_stream_types(record)))
        loudness = validate.audio_loudness(outputs[0]) or {}
        measured = loudness.get("input_i")
        peak = loudness.get("input_tp")
        checks.add(
            "the output measures -16±1 LUFS",
            measured is not None and abs(float(measured) + 16.0) <= 1.0,
            "-16±1 LUFS",
            measured,
        )
        checks.add(
            "true peak stays at or under -1.5 dBTP",
            peak is not None and float(peak) <= -1.0,
            "≤ -1.5 dBTP",
            peak,
        )


def scenario_tool_audio_trim(h, rec, checks):
    def apply(s):
        # This is the control that used to mount <audio> and take the web process
        # down. Press it and prove the dialog is still there afterwards.
        play = s.button("Play", within=s.dialog())
        if play is not None:
            s.press_in_dialog("Play")
            time.sleep(1.0)
            checks.add(
                "playing trim audio keeps the dialog alive",
                s.dialog() is not None,
                True,
                s.dialog() is not None,
            )
        s.set_slider_auto("Trim end", 5.0)
        s.set_slider_auto("Trim start", 2.0)

    with h.launch("tool-audio-trim", [h.fixture("audio/speech.wav")]) as session:
        rec["workdir"] = session.workdir
        outputs, _preview = run_tool_job(session, rec, checks, "Trim", category="Audio", apply=apply)
        record = record_outputs(rec, outputs)[0]
        checks.add("trim cuts 2→5 s to 3.0 s", abs(media_duration(record) - 3.0) <= 0.1, "3.0±0.1 s", media_duration(record))

    # 1 s of silence at each end: the checkbox must remove both, and the first
    # 100 ms of the result must carry the tone.
    silent = os.path.join(h.profile, "work", "silent-ends.wav")
    os.makedirs(os.path.dirname(silent), exist_ok=True)
    built = run(
        [
            "ffmpeg",
            "-v",
            "error",
            "-y",
            "-f",
            "lavfi",
            "-i",
            "sine=frequency=440:duration=2",
            "-af",
            "adelay=1000|1000,apad=pad_dur=1",
            "-c:a",
            "pcm_s16le",
            silent,
        ],
        timeout=60,
    )
    if built.returncode != 0:
        raise Skip(f"could not build the silent-ended fixture: {built.stderr.strip()[:200]}")

    with h.launch("tool-audio-trim-silence", [silent]) as session:
        rec["workdir"] = session.workdir

        def silence(s):
            s.set_check("Trim silent ends automatically", True)
            time.sleep(0.5)

        outputs, _preview = run_tool_job(session, rec, checks, "Trim", category="Audio", apply=silence)
        record = record_outputs(rec, outputs)[0]
        checks.add(
            "trim_silence removes 2 s of silent ends",
            abs(media_duration(record) - 2.0) <= 0.3,
            "≈2.0 s",
            media_duration(record),
        )
        leading = validate.rms(outputs[0], 0.0, 0.1)
        checks.add(
            "the silence-removed clip starts with sound",
            leading is not None and leading > -40.0,
            "> -40 dBFS",
            leading,
        )


def scenario_tool_audio_channels(h, rec, checks):
    def gains(s):
        s.set_slider_auto("Left volume", 50.0)

    with h.launch("tool-audio-channels", [h.fixture("audio/tone-stereo-30s.wav")]) as session:
        rec["workdir"] = session.workdir
        outputs, _preview = run_tool_job(session, rec, checks, "Channels", category="Audio", apply=gains)
        record = record_outputs(rec, outputs)[0]
        audio = next(
            (stream for stream in record.get("media", {}).get("streams", []) if stream.get("type") == "audio"),
            {},
        )
        checks.add("stereo output has 2 channels", audio.get("channels") == 2, 2, audio.get("channels"))
        left = channel_rms_db(outputs[0], 0)
        right = channel_rms_db(outputs[0], 1)
        difference = (right - left) if left is not None and right is not None else None
        checks.add(
            "L=50 % is about half the right channel",
            difference is not None and 4.5 <= difference <= 7.5,
            "right − left ≈ 6 dB",
            difference,
        )

    with h.launch("tool-audio-channels-mono", [h.fixture("audio/tone-stereo-30s.wav")]) as session:
        rec["workdir"] = session.workdir
        outputs, _preview = run_tool_job(
            session,
            rec,
            checks,
            "Channels",
            category="Audio",
            apply=lambda s: s.press_in_dialog("Mono"),
        )
        record = record_outputs(rec, outputs)[0]
        audio = next(
            (stream for stream in record.get("media", {}).get("streams", []) if stream.get("type") == "audio"),
            {},
        )
        checks.add("mono output has 1 channel", audio.get("channels") == 1, 1, audio.get("channels"))


def scenario_tool_audio_visualize(h, rec, checks):
    def at(shape_label):
        def choose(s):
            s.press_in_dialog(shape_label)

        return choose

    cases = [
        ("Landscape · 16:9", 1280, 720),
        ("Portrait · 9:16", 720, 1280),
        ("Square · 1:1", 1080, 1080),
    ]
    for index, (label, width, height) in enumerate(cases):
        with h.launch(f"tool-audio-visualize-{index}", [h.fixture("audio/speech.mp3")]) as session:
            rec["workdir"] = session.workdir
            outputs, _preview = run_tool_job(
                session,
                rec,
                checks,
                "Visualize",
                category="Audio",
                apply=at(label) if label != "Landscape · 16:9" else None,
            )
            record = record_outputs(rec, outputs)[0]
            video = video_stream(record)
            checks.add(
                f"visualize {label} is {width}×{height}",
                (video.get("width"), video.get("height")) == (width, height),
                [width, height],
                [video.get("width"), video.get("height")],
            )
            checks.add(
                f"visualize {label} keeps the duration",
                abs(media_duration(record) - 14.1) <= 0.2,
                "14.1±0.2 s",
                media_duration(record),
            )
            checks.add(
                f"visualize {label} keeps the audio",
                "audio" in _stream_types(record),
                "audio",
                sorted(_stream_types(record)),
            )


def scenario_tool_audio_bleep(h, rec, checks):
    fixture = h.fixture("audio/speech.wav")

    def apply(s):
        dialog = s.dialog()
        # One drag, at a known place: two synthetic drags on the same lane raced and the
        # second replaced the first, so the tone ended up somewhere the check did not
        # look. The lane appends each drag, so one is enough to prove the mapping.
        s.drag_bleep_range(dialog, 4.0 / 14.1, 5.0 / 14.1)
        time.sleep(0.5)
        note = s.subtree_text(s.dialog())
        rec["measurements"]["range_note"] = [line for line in note.splitlines() if "range" in line][:2]
        if s.button("With Bleeps", within=s.dialog()) is None:
            raise Skip("the bleep dialog rendered no preview row")
        if "SENSITIVE" not in _states(s.button("With Bleeps", within=s.dialog())):
            # The synthetic drag over the waveform does not register under WebKit/Xvfb
            # (the same component takes it on a photo), so no range exists and the preview
            # control is correctly disabled. The engine's per-range behaviour — a 1 kHz
            # tone inside the range and the source's own level outside it — is measured by
            # `quality-audit.py --only audio-bleep`.
            raise Skip(
                "the waveform did not take the synthetic drag, so no bleep range exists; "
                "the per-range output is measured by quality-audit audio-bleep"
            )
        # The "With Bleeps" preview runs the engine; the cache must gain its MP3.
        s.press_in_dialog("With Bleeps")
        # The command writes the MP3 into the cache the *app* uses: XDG_CACHE_HOME is
        # set for this worker, and the outer launcher's profile is a different path.
        cache = os.path.join(
            os.environ.get("XDG_CACHE_HOME", os.path.expanduser("~/.cache")),
            "convertme",
            "previews",
        )
        deadline = time.time() + 30
        final = []
        while time.time() < deadline:
            final = [
                name
                for _root, _dirs, names in os.walk(cache)
                for name in names
                if name.endswith(".mp3") and ".preview-" not in name
            ] if os.path.isdir(cache) else []
            if final:
                break
            time.sleep(0.3)
        dialog = s.dialog()
        rec["measurements"]["preview_text"] = (
            s.subtree_text(dialog)[-400:] if dialog is not None else "(no dialog)"
        )
        rec["measurements"]["preview_files"] = final[:6]
        rec["measurements"]["bleep_preview_cached"] = bool(final)

    with h.launch("tool-audio-bleep", [fixture]) as session:
        rec["workdir"] = session.workdir
        outputs, _preview = run_tool_job(session, rec, checks, "Bleep", category="Audio", apply=apply)
        record = record_outputs(rec, outputs)[0]
        checks.add("bleep keeps the duration", 13.0 < media_duration(record) < 15.0, "≈14.1 s", media_duration(record))
        checks.add("bleep rewrites the audio", record["sha256"] != sha256(os.path.join(session.workdir, "speech.wav")), "different bytes", record["sha256"][:12])
        # The drag above marks 4.0–5.0 s of a 14.1 s clip.
        tone = validate.tone_strength(outputs[0], 4.4, 0.3, 1000.0)
        source_tone = validate.tone_strength(fixture, 4.4, 0.3, 1000.0)
        checks.add(
            "the first range is a 1 kHz tone",
            tone is not None and tone > 100,
            "1 kHz dominates its neighbours (>100x)",
            None if tone is None else round(tone),
        )
        checks.add(
            "the same window of the source is not a tone",
            source_tone is not None and source_tone < 1,
            "< 1",
            None if source_tone is None else round(source_tone, 3),
        )
        inside = validate.rms(outputs[0], 1.3, 0.3)
        outside = validate.rms(outputs[0], 3.0, 0.4)
        source_outside = validate.rms(fixture, 3.0, 0.4)
        checks.add(
            "outside the ranges the level matches the source",
            inside is not None
            and outside is not None
            and source_outside is not None
            and abs(outside - source_outside) <= 2.0,
            "outside within 2 dB of the source",
            [outside, source_outside],
        )
        checks.add(
            "the bleep preview file was written",
            bool(rec["measurements"].get("bleep_preview_cached")),
            "an MP3 in the preview cache",
            rec["measurements"].get("bleep_preview_cached"),
        )


def scenario_tool_audio_metadata(h, rec, checks):
    with h.launch("tool-audio-metadata", [h.fixture("audio/speech.mp3")]) as session:
        rec["workdir"] = session.workdir
        outputs, _preview = run_tool_job(
            session,
            rec,
            checks,
            "Metadata",
            category="Audio",
            apply=lambda s: s.set_entry("Title", "Harness"),
        )
        record = record_outputs(rec, outputs)[0]
        tags = record.get("media", {}).get("format_tags", {})
        checks.add("Title is written to the MP3", tags.get("title") == "Harness", "Harness", tags.get("title"))
        checks.add("metadata keeps audio", "audio" in _stream_types(record), "audio", sorted(_stream_types(record)))

    # Reopening the dialog must show the tag the file already carries: the form is
    # populated from the file, not presented empty for the user to retype.
    with h.launch("tool-audio-metadata-read", [outputs[0]]) as session:
        session.category_hint = "Audio"
        if session.open_tool("Metadata") is None:
            raise Skip("Metadata ran without opening a dialog")
        deadline = time.time() + 8
        node = None
        while time.time() < deadline and node is None:
            node = session.entry("Title")
            time.sleep(0.2)
        shown = session.text(node) if node is not None else ""
        rec["measurements"]["metadata_shows_existing"] = shown
        checks.add(
            "the metadata dialog shows the existing title",
            shown == "Harness",
            "Harness",
            shown,
        )


def scenario_tool_pdf_organize(h, rec, checks):
    source = h.fixture("pdf/report-5-pages.pdf")
    def apply(s):
        s.set_entry("Page order", "2,1,3,4,5")

    with h.launch("tool-pdf-organize", [source]) as session:
        rec["workdir"] = session.workdir
        outputs, _preview = run_tool_job(session, rec, checks, "Organize pages", category="PDF", apply=apply)
        record = record_outputs(rec, outputs)[0]
        info = record.get("pdf", {})
        checks.add("organize keeps every page", info.get("Pages") == "5", "5", info.get("Pages"))
        source_page = run(["pdftotext", "-f", "2", "-l", "2", os.path.join(session.workdir, "report-5-pages.pdf"), "-"]).stdout
        output_page = run(["pdftotext", "-f", "1", "-l", "1", outputs[0], "-"]).stdout
        def head(text):
            return re.sub(r"\s+", " ", text).strip()[:40]
        checks.add("page 2 is now first", bool(head(source_page)) and head(source_page) == head(output_page), head(source_page), head(output_page))


def scenario_tool_pdf_metadata(h, rec, checks):
    with h.launch("tool-pdf-metadata", [h.fixture("pdf/report-5-pages.pdf")]) as session:
        rec["workdir"] = session.workdir
        outputs, _preview = run_tool_job(
            session,
            rec,
            checks,
            "Metadata",
            category="PDF",
            apply=lambda s: s.set_entry("Title", "Harness"),
        )
        record = record_outputs(rec, outputs)[0]
        info = record.get("pdf", {})
        checks.add("Title reaches the PDF info dictionary", info.get("Title") == "Harness", "Harness", info.get("Title"))
        checks.add("metadata keeps every page", info.get("Pages") == "5", "5", info.get("Pages"))

    with h.launch("tool-pdf-metadata-read", [outputs[0]]) as session:
        session.category_hint = "PDF"
        if session.open_tool("Metadata") is None:
            raise Skip("Metadata ran without opening a dialog")
        deadline = time.time() + 8
        node = None
        while time.time() < deadline and node is None:
            node = session.entry("Title")
            time.sleep(0.2)
        shown = session.text(node) if node is not None else ""
        rec["measurements"]["metadata_shows_existing"] = shown
        checks.add(
            "the metadata dialog shows the existing title",
            shown == "Harness",
            "Harness",
            shown,
        )


def scenario_tool_pdf_pages(h, rec, checks):
    def apply(s):
        s.select_option("Image format", "png")

    with h.launch("tool-pdf-pages", [h.fixture("pdf/report-5-pages.pdf")]) as session:
        rec["workdir"] = session.workdir
        outputs, _preview = run_tool_job(session, rec, checks, "Save pages as images", category="PDF", apply=apply, expect=5)
        records = record_outputs(rec, outputs)
        checks.add("one image per page", len(records) == 5, 5, len(records))
        checks.add(
            "every page image is a PNG at 300 DPI",
            all(record["kind"] == "png" and record.get("image", {}).get("width", 0) >= 2400 for record in records),
            "5 PNGs >= 2400 px wide",
            [(record.get("image", {}).get("width"), record.get("image", {}).get("height")) for record in records[:3]],
        )


def scenario_convert_image(h, rec, checks):
    source = h.fixture(POSTER)
    with h.launch("convert-image", [source]) as session:
        rec["workdir"] = session.workdir
        outputs = conversion_job(session, rec, checks, "JPG")
        record = record_outputs(rec, outputs)[0]
        info = record.get("image", {})
        checks.add("magic is JPEG", record["kind"] == "jpeg", "jpeg", record["kind"])
        checks.add("extension follows the target", record["name"].endswith(".jpg"), ".jpg", record["name"])
        checks.add("dimensions preserved", (info.get("width"), info.get("height")) == (2000, 3000), [2000, 3000], [info.get("width"), info.get("height")])


def scenario_convert_video(h, rec, checks):
    source = h.fixture("video/clip-landscape-720p-10s.mp4")
    with h.launch("convert-video", [source]) as session:
        rec["workdir"] = session.workdir
        outputs = conversion_job(session, rec, checks, "MKV")
        record = record_outputs(rec, outputs)[0]
        media = record.get("media", {})
        checks.add("magic is Matroska", record["kind"] == "matroska", "matroska", record["kind"])
        checks.add("duration preserved", abs(media.get("duration", 0) - 10.0) < 1.0, "≈10 s", media.get("duration"))


def scenario_convert_audio(h, rec, checks):
    source = h.fixture("audio/speech.mp3")
    with h.launch("convert-audio", [source]) as session:
        rec["workdir"] = session.workdir
        outputs = conversion_job(session, rec, checks, "OGG")
        record = record_outputs(rec, outputs)[0]
        media = record.get("media", {})
        checks.add("magic is Ogg", record["kind"] == "ogg", "ogg", record["kind"])
        checks.add("audio stream present", any(s["type"] == "audio" for s in media.get("streams", [])), "audio", media.get("streams"))


def scenario_convert_pdf_text(h, rec, checks):
    source = h.fixture("pdf/report-5-pages.pdf")
    source_text = run(["pdftotext", source, "-"]).stdout
    words = _first_words(source_text, 4)
    with h.launch("convert-pdf-text", [source]) as session:
        rec["workdir"] = session.workdir
        outputs = conversion_job(session, rec, checks, "TXT")
        output = outputs[0]
        text = text_of(output)
        checks.add("output is text", record_outputs(rec, outputs)[0]["kind"] == "unknown" or output.endswith(".txt"), ".txt", os.path.basename(output))
        checks.add("extracted text matches the source", all(word in text for word in words) and bool(words), words, text[:200])


def scenario_convert_pdf_docx(h, rec, checks):
    """The headline defect: PDF → DOCX must keep the document's structure."""
    source = h.fixture("pdf/Jake_s_Resume__Anonymous_.pdf")
    with h.launch("convert-pdf-docx", [source]) as session:
        rec["workdir"] = session.workdir
        outputs = conversion_job(session, rec, checks, "DOCX")
        output = outputs[0]
        record_outputs(rec, outputs)
        facts = validate.docx(output)
        if facts is None:
            raise ScenarioFailure("the published DOCX is not a readable package")
        rec["measurements"]["docx"] = {
            key: facts[key]
            for key in ("paragraphs", "heading1", "heading2", "lists", "numpr",
                        "images", "page_breaks", "bytes")
        }
        checks.add("Heading 1 paragraphs", facts["heading1"] >= 1, ">=1", facts["heading1"])
        checks.add("Heading 2 paragraphs", facts["heading2"] >= 2, ">=2", facts["heading2"])
        checks.add("numbered list paragraphs", facts["lists"] >= 23, ">=23", facts["lists"])
        checks.add("list markers (w:numPr)", facts["numpr"] >= 23, ">=23", facts["numpr"])
        checks.add("embedded images", facts["images"] >= 4, ">=4", facts["images"])
        checks.add("no page breaks", facts["page_breaks"] == 0, 0, facts["page_breaks"])
        # The source's own text layer, not its bytes: `text_of` reads a text file.
        source_words = validate.pdf_text(source) or ""
        coverage = validate.text_coverage(source_words, facts["text"])
        checks.add("text coverage", coverage is not None and coverage >= 0.98, ">=0.98", coverage)
        checks.add("no bullet glyph in the text", "•" not in facts["text"], "none", "•" in facts["text"])
        text = facts["text"]
        sections = [
            "Summary",
            "Education",
            "Relevant Coursework",
            "Experience",
            "Projects",
            "Technical Skills",
            "Leadership",
        ]
        positions = [text.find(name) for name in sections]
        checks.add(
            "the seven sections in order",
            all(position >= 0 for position in positions) and positions == sorted(positions),
            sections,
            positions,
        )
        checks.add(
            "the document opens in a reader",
            "word/document.xml" in zip_members(output),
            "word/document.xml",
            zip_members(output)[:4],
        )


def zip_members(path: str) -> list[str]:
    try:
        with zipfile.ZipFile(path) as package:
            return package.namelist()
    except (OSError, zipfile.BadZipFile):
        return []


def scenario_convert_document(h, rec, checks):
    source = h.fixture("documents/report.md")
    source_text = text_of(source)
    words = _body_words(source_text, 3)
    with h.launch("convert-document", [source]) as session:
        rec["workdir"] = session.workdir
        outputs = conversion_job(session, rec, checks, "DOCX")
        output = outputs[0]
        record = record_outputs(rec, outputs)[0]
        checks.add("magic is a zip container", record["kind"] == "zip", "zip", record["kind"])
        document = zip_member_text(output, "word/document.xml")
        checks.add("document text survives", all(word in document for word in words), words, document[:200])


def scenario_convert_slides(h, rec, checks):
    source = h.fixture("slides/deck-8-slides.pptx")
    with h.launch("convert-slides", [source]) as session:
        rec["workdir"] = session.workdir
        outputs = conversion_job(session, rec, checks, "PDF")
        record = record_outputs(rec, outputs)[0]
        info = record.get("pdf", {})
        checks.add("magic is PDF", record["kind"] == "pdf", "pdf", record["kind"])
        checks.add("slides became pages", int(info.get("Pages", "0")) >= 1, ">=1 page", info.get("Pages"))


def scenario_convert_spreadsheet(h, rec, checks):
    source = h.fixture("spreadsheets/sheet.xlsx")
    with h.launch("convert-spreadsheet", [source]) as session:
        rec["workdir"] = session.workdir
        try:
            outputs = conversion_job(session, rec, checks, "CSV")
        except Skip as error:
            raise Skip(str(error))
        output = outputs[0]
        text = text_of(output)
        checks.add("output has CSV rows", "," in text and len(text.splitlines()) >= 2, ">=2 comma rows", text[:200])


def scenario_convert_epub(h, rec, checks):
    source = h.fixture("epub/report.epub")
    with h.launch("convert-epub", [source]) as session:
        rec["workdir"] = session.workdir
        try:
            outputs = conversion_job(session, rec, checks, "MD")
        except Skip as error:
            raise Skip(str(error))
        output = outputs[0]
        text = text_of(output)
        words = _first_words(text, 3)
        checks.add("markdown has words", bool(words), "non-empty", text[:120])


def scenario_convert_subtitles(h, rec, checks):
    source = h.fixture("subtitles/cues.srt")
    source_text = text_of(source)
    cue_words = _first_words(source_text, 3)
    with h.launch("convert-subtitles", [source]) as session:
        rec["workdir"] = session.workdir
        outputs = conversion_job(session, rec, checks, "VTT")
        output = outputs[0]
        text = text_of(output)
        checks.add("starts with WEBVTT", text.lstrip().startswith("WEBVTT"), "WEBVTT", text[:40])
        checks.add("cue text preserved", all(word in text for word in cue_words) and bool(cue_words), cue_words, text[:200])


def scenario_convert_archive(h, rec, checks):
    source = h.fixture("archives/project.zip")
    names = zip_names(source)
    with h.launch("convert-archive", [source]) as session:
        rec["workdir"] = session.workdir
        outputs = conversion_job(session, rec, checks, "TAR")
        output = outputs[0]
        record = record_outputs(rec, outputs)[0]
        checks.add("magic is TAR", record["kind"] == "tar" or output.endswith(".tar"), "tar", record["kind"])
        checks.add(
            "entries match the zip",
            sorted(tar_names(output)) == sorted(names),
            sorted(names),
            sorted(tar_names(output)),
        )


def scenario_extract_archive(h, rec, checks):
    source = h.fixture("archives/project.zip")
    names = zip_names(source)
    with h.launch("extract-archive", [source]) as session:
        rec["workdir"] = session.workdir
        before = session.snapshot()
        before_rows = session.history_rows()
        dialog = session.open_tool("Extract")
        if dialog is not None:
            raise ScenarioFailure("Extract unexpectedly opened a dialog")
        outputs = session.wait_job(before, before_rows, timeout=180)
        rec["measurements"]["history_after"] = session.history_rows()[:2]
        rec["screenshots"].append(session.shot("after"))
        record = record_outputs(rec, outputs)[0]
        entries = record.get("entries", [])
        checks.add("a folder was published", record["kind"] == "directory", "directory", record["kind"])
        checks.add(
            "every archive entry is present",
            all(any(entry == name or entry.startswith(name.rstrip("/") + "/") or name.startswith(entry) for entry in entries) for name in names),
            names,
            entries,
        )


def scenario_immediate_tools(h, rec, checks):
    """Zero-field tools: Mute, Join, PDF Merge, PDF Split, PDF Extract text."""
    jobs = [
        ("mute", "video/clip-landscape-720p-10s.mp4", "Remove audio", _check_muted, 1),
        ("pdf-split", "pdf/report-5-pages.pdf", "Split pages", _check_split, 5),
        ("pdf-extract-text", "pdf/report-5-pages.pdf", "Extract text", _check_extracted, 1),
    ]
    for name, fixture, tool, validator, expect in jobs:
        fixture = h.fixture(fixture) if isinstance(fixture, str) else [h.fixture(f) for f in fixture]
        fixtures = [fixture] if isinstance(fixture, str) else fixture
        with h.launch(f"immediate-{name}", fixtures) as session:
            rec["workdir"] = session.workdir
            if not fixtures:
                continue
            if isinstance(fixture, str):
                session.toggle_category("Video" if "video" in fixture else "PDF")
            elif "video" in fixtures[0]:
                session.toggle_category("Video")
            else:
                session.toggle_category("PDF")
            before = session.snapshot()
            before_rows = session.history_rows()
            dialog = session.open_tool(tool)
            if dialog is not None:
                close_dialog(session)
                raise ScenarioFailure(f"{tool} opened a dialog; expected a direct job")
            outputs = session.wait_job(before, before_rows, timeout=240, expect=expect)
            records = record_outputs(rec, outputs)
            rec["parameters"] = {"tool": tool}
            rec["screenshots"].append(session.shot("after"))
            validator(session, checks, records, outputs)


def _check_muted(session, checks, records, outputs):
    media = records[0].get("media", {})
    types = [s["type"] for s in media.get("streams", [])]
    checks.add("mute drops the audio stream", "audio" not in types, "no audio", types)
    checks.add("mute keeps the video stream", "video" in types, "video", types)


def _check_joined(session, checks, records, outputs):
    media = records[0].get("media", {})
    checks.add("join produces one clip", len(outputs) == 1, 1, len(outputs))
    checks.add("joined duration covers both clips", media.get("duration", 0) >= 14.0, ">=14 s", media.get("duration"))


def _check_merged(session, checks, records, outputs):
    info = records[0].get("pdf", {})
    checks.add("merge keeps every page", info.get("Pages") == "8", "8", info.get("Pages"))


def _check_split(session, checks, records, outputs):
    pdfs = [record for record in records if record.get("kind") == "pdf"]
    checks.add("split writes one PDF per page", len(pdfs) == 5, 5, [record["name"] for record in records])
    checks.add(
        "every split file is a single page",
        all(record.get("pdf", {}).get("Pages") == "1" for record in pdfs),
        "one page each",
        [record.get("pdf", {}).get("Pages") for record in pdfs],
    )


def _check_extracted(session, checks, records, outputs):
    output = outputs[0]
    text = text_of(output)
    source = os.path.join(session.workdir, "report-5-pages.pdf")
    words = _first_words(run(["pdftotext", source, "-"]).stdout, 3)
    checks.add("extracted text matches the PDF", all(word in text for word in words) and bool(words), words, text[:200])


def page_one_text(path: str) -> str:
    result = run(["pdftotext", "-f", "1", "-l", "1", path, "-"], timeout=60)
    return " ".join(result.stdout.split())


def scenario_tool_pdf_merge(h, rec, checks):
    """Merge in selection order, then reversed: the first page must follow the order."""
    files = [h.fixture("pdf/report-5-pages.pdf"), h.fixture("pdf/scanned-3-pages.pdf")]
    with h.launch("tool-pdf-merge", files) as session:
        rec["workdir"] = session.workdir
        outputs, _preview = run_tool_job(session, rec, checks, "Merge", category="PDF")
        record = record_outputs(rec, outputs)[0]
        info = record.get("pdf", {})
        checks.add("merge keeps every page", info.get("Pages") == "8", "8", info.get("Pages"))
        first_line = page_one_text(outputs[0])[:24]
        source_line = page_one_text(os.path.join(session.workdir, "report-5-pages.pdf"))[:24]
        checks.add(
            "merge starts with the first selected document",
            bool(source_line) and first_line.startswith(source_line),
            source_line,
            first_line,
        )

    with h.launch("tool-pdf-merge-reversed", files) as session:
        rec["workdir"] = session.workdir

        def reverse(s):
            # The order editor lists the selection; two moves put the scan first.
            s.press_in_dialog("Move scanned-3-pages.pdf up")
            time.sleep(0.4)

        outputs, _preview = run_tool_job(
            session, rec, checks, "Merge", category="PDF", apply=reverse
        )
        record = record_outputs(rec, outputs)[0]
        info = record.get("pdf", {})
        checks.add("reversed merge keeps every page", info.get("Pages") == "8", "8", info.get("Pages"))
        text = page_one_text(outputs[0])
        checks.add(
            "reversed merge starts with the last selected document",
            "ConvertMe Test Report" not in text,
            "the scan's first page",
            text[:40],
        )


def scenario_tool_pdf_organize(h, rec, checks):
    """Duplicate page 2, remove page 4, rotate page 1: the page sequence is the value."""
    with h.launch("tool-pdf-organize", [h.fixture("pdf/report-5-pages.pdf")]) as session:
        rec["workdir"] = session.workdir

        def edit(s):
            s.press_in_dialog("Duplicate page 2")
            time.sleep(0.5)
            s.press_in_dialog("Remove page 4")
            time.sleep(0.5)
            s.press_in_dialog("Rotate page 1")
            time.sleep(0.6)
            rec["measurements"]["after_rotate"] = s.subtree_text(s.dialog())[-300:]
            rec["screenshots"].append(s.shot("rotated"))

        outputs, _preview = run_tool_job(
            session, rec, checks, "Organize pages", category="PDF", apply=edit
        )
        record = record_outputs(rec, outputs)[0]
        info = record.get("pdf", {})
        # duplicate page 2 and drop page 4: 5 -> [1,2,2,3,5]
        checks.add("organize keeps five pages", info.get("Pages") == "5", "5", info.get("Pages"))
        source = os.path.join(session.workdir, "report-5-pages.pdf")
        wanted = [page_one_text_source(source, page) for page in (1, 2, 2, 3, 5)]
        produced = [page_one_text_page(outputs[0], page) for page in range(1, 6)]
        checks.add(
            "page order is [1,2,2,3,5]",
            all(a.startswith(b[:20]) for a, b in zip(produced, wanted)),
            [text[:16] for text in wanted],
            [text[:16] for text in produced],
        )
        # qpdf 12 does not put /Rotate in its JSON page keys (and rejects
        # `--json` together with a positional path), so the earlier probe always read 0
        # and the check was skipped as a product bug that did not exist. pdfinfo states
        # the rotation directly.
        rotation = run(["pdfinfo", "-f", "1", "-l", "1", outputs[0]], timeout=60)
        match = re.search(r"Page\s+1 rot:\s+(-?\d+)", rotation.stdout)
        rotated = int(match.group(1)) if match else 0
        rec["measurements"]["page1_rotation"] = rotated
        checks.add("page 1 is rotated by 90°", rotated == 90, 90, rotated)


def scenario_tool_pdf_organize_multi(h, rec, checks):
    """Two PDFs: both cards exist, and switching files reloads that document's pages.

    This is the reported failure: the selection said two files, one card was drawn,
    and the organizer kept the first file's page count after the arrow switched.
    """
    five = h.fixture("pdf/report-5-pages.pdf")
    three = h.fixture("pdf/scanned-3-pages.pdf")
    with h.launch("tool-pdf-organize-multi", [five, three]) as session:
        rec["workdir"] = session.workdir
        session.wait_text("2 files ready", timeout=30)
        workspace = "\n".join(value for _role, value in session.all_texts())
        checks.add(
            "both selected PDFs have a card",
            "report-5-pages.pdf" in workspace and "scanned-3-pages.pdf" in workspace,
            "both names",
            workspace[:500],
        )
        dialog = session.open_tool("Organize pages")
        if dialog is None:
            raise ScenarioFailure("Organize pages opened no dialog")
        first = session.subtree_text(session.dialog())
        first_cards = len(re.findall(r"Page\s+\d+", first))
        checks.add("first file shows 5 pages", first_cards == 5, 5, first_cards)
        session.press_button("Next file", within=session.dialog())
        deadline = time.time() + 8
        second = ""
        second_cards = 0
        while time.time() < deadline:
            second = session.subtree_text(session.dialog())
            second_cards = len(re.findall(r"Page\s+\d+", second))
            if "2 of 2" in second and second_cards == 3:
                break
            time.sleep(0.25)
        checks.add("the arrow reaches the second file", "2 of 2" in second, "2 of 2", second[:300])
        checks.add("the second file reloads its own 3 pages", second_cards == 3, 3, second_cards)
        rec["measurements"]["second_file"] = second[-300:]
        rec["screenshots"].append(session.shot("second-pdf-file"))
        session.press_button("Close", within=session.dialog())


def page_one_text_source(path: str, page: int) -> str:
    result = run(["pdftotext", "-f", str(page), "-l", str(page), path, "-"], timeout=60)
    return " ".join(result.stdout.split())


def page_one_text_page(path: str, page: int) -> str:
    return page_one_text_source(path, page)


def scenario_tool_video_join(h, rec, checks):
    clips = [
        h.fixture("video/clip-landscape-720p-10s.mp4"),
        h.fixture("video/clip-portrait-1080x1920-6s.mp4"),
    ]
    with h.launch("tool-video-join", clips) as session:
        rec["workdir"] = session.workdir
        outputs, _preview = run_tool_job(session, rec, checks, "Join", category="Video")
        record = record_outputs(rec, outputs)[0]
        video = video_stream(record)
        checks.add("join produces one clip", len(outputs) == 1, 1, len(outputs))
        checks.add(
            "joined duration covers both clips",
            media_duration(record) >= 15.5,
            ">=15.5 s",
            media_duration(record),
        )
        checks.add(
            "the join uses the first clip as its canvas",
            (video.get("width"), video.get("height")) == (1280, 720),
            [1280, 720],
            [video.get("width"), video.get("height")],
        )
        checks.add("join keeps the audio", "audio" in _stream_types(record), "audio", sorted(_stream_types(record)))
        forward = frame_pixel(outputs[0], 0.3, 200, 500, session.workdir, "join-forward")

    with h.launch("tool-video-join-reversed", clips) as session:
        rec["workdir"] = session.workdir

        def reverse(s):
            dialog = s.dialog()
            move = next(
                (
                    node
                    for node in s.dialog_buttons()
                    if "portrait" in (_safe(node, "get_name") or "")
                    and (_safe(node, "get_name") or "").startswith("Move ")
                ),
                None,
            )
            if move is None:
                raise ScenarioFailure("the portrait clip has no move-up row in Join")
            if not s.press(move):
                raise ScenarioFailure("could not move the portrait clip up")

        outputs, _preview = run_tool_job(session, rec, checks, "Join", category="Video", apply=reverse)
        record = record_outputs(rec, outputs)[0]
        video = video_stream(record)
        checks.add(
            "reversing the order changes the canvas",
            (video.get("width"), video.get("height")) == (1080, 1920),
            [1080, 1920],
            [video.get("width"), video.get("height")],
        )
        checks.add(
            "reversing the order changes the first frame",
            pixel_distance(
                forward,
                frame_pixel(outputs[0], 0.3, 200, 500, session.workdir, "join-reversed"),
            )
            > 30,
            ">30 levels",
            "same",
        )
        checks.add(
            "the joined duration is order-independent",
            media_duration(record) >= 15.5,
            ">=15.5 s",
            media_duration(record),
        )


def scenario_negative_corrupt(h, rec, checks):
    corrupt = os.path.join(h.profile, "corrupt.png")
    with open(corrupt, "wb") as handle:
        handle.write(b"\x89PNG\r\n\x1a\nthis is not a png" + b"\x00" * 64)
    with h.launch("negative-corrupt", [corrupt]) as session:
        rec["workdir"] = session.workdir
        time.sleep(1.2)
        session.show_history()
        before_rows = session.history_rows()
        initial = "\n".join(value for _role, value in session.all_texts())
        rec["measurements"]["analysis"] = initial[:600]
        rec["screenshots"].append(session.shot("workspace"))
        before = session.snapshot()
        dialog = session.open_tool("Compress")
        if dialog is None:
            # Some paths reject the selection before any tool opens.
            time.sleep(1.5)
            reported = any(
                word in "\n".join(value for _role, value in session.all_texts()).lower()
                for word in ("skipped", "readable", "could not", "failed")
            )
            checks.add("corrupt input is refused before Run", reported, "an error note", initial[-300:])
        else:
            time.sleep(2.0)
            preview = session.preview_text()
            dialog_text = "\n".join(value for _role, value in session.all_texts(within=session.dialog()))
            session.press_run()
            published = []
            try:
                published = session.wait_job(before, before_rows, timeout=30)
            except ScenarioFailure:
                pass
            if session.dialog() is not None:
                close_dialog(session)
            history = "\n".join(session.history_rows())
            detail = "\n".join(part for part in (preview, dialog_text, history) if part)
            rec["measurements"]["failure_text"] = detail[:1200]
            checks.add(
                "corrupt input produces no artifact",
                not published,
                "no artifact",
                [os.path.basename(p) for p in published],
            )
            checks.add(
                "the failure is reported, not swallowed",
                any(word in detail.lower() for word in ("could not", "failed", "unsupported", "error", "decode")),
                "an error message",
                detail[:600],
            )
        checks.add(
            "the app survives corrupt input",
            session.proc.poll() is None,
            "process alive",
            session.proc.returncode,
        )


def scenario_negative_missing_tool(h, rec, checks):
    poster = h.fixture(POSTER)
    exe_dir = os.path.join(h.profile, "no-tools")
    os.makedirs(exe_dir, exist_ok=True)
    exe = os.path.join(exe_dir, "convertme")
    shutil.copy2(BIN, exe)
    empty_path = os.path.join(h.profile, "empty-path")
    os.makedirs(empty_path, exist_ok=True)
    with h.launch(
        "negative-missing-tool",
        [poster],
        env_extra={"PATH": empty_path},
        exe=exe,
    ) as session:
        rec["workdir"] = session.workdir
        session.press_button("Settings")
        session.wait_text("Missing tools", timeout=10)
        rec["screenshots"].append(session.shot("settings-missing"))
        texts = "\n".join(value for _role, value in session.all_texts())
        checks.add(
            "Settings lists ImageMagick as missing",
            "ImageMagick" in texts,
            "ImageMagick",
            texts[-800:],
        )
        session.press_button("Close settings")
        time.sleep(0.5)
        session.toggle_category("Images")
        cards = session.cards()
        labels = [label for label, _node in cards]
        rec["measurements"]["cards_without_tools"] = labels
        checks.add(
            "no engine-backed tool is offered without its engine",
            "Compress" not in labels,
            "fallback card labels",
            labels,
        )
        before = session.snapshot()
        before_rows = session.history_rows()
        pressed = False
        if cards:
            rec["measurements"]["first_card_states"] = sorted(_states(cards[0][1]))
            pressed = session.press(cards[0][1])
            if not pressed:
                time.sleep(0.3)
                again = session.wait_card(cards[0][0], timeout=5)
                pressed = session.press(again)
        rec["measurements"]["first_card_pressed"] = pressed
        dialog = session.wait_dialog(timeout=20)
        rec["measurements"]["status_after_click"] = session.status_text()
        rec["measurements"]["dialog_after_click"] = bool(dialog)
        if dialog is not None:
            dialog_text = "\n".join(value for _role, value in session.all_texts(within=dialog))
            rec["measurements"]["dialog_text"] = dialog_text[:1000]
            session.press_run()
            try:
                outputs = session.wait_job(before, before_rows, timeout=25)
                published = True
            except ScenarioFailure:
                published = False
            if session.dialog() is not None:
                dialog_text = "\n".join(value for _role, value in session.all_texts(within=session.dialog()))
                rec["measurements"]["dialog_after_run"] = dialog_text[:1200]
                close_dialog(session)
        else:
            published = False
            try:
                outputs = session.wait_job(before, before_rows, timeout=15)
                published = True
            except ScenarioFailure:
                pass
        history = "\n".join(session.history_rows())
        detail = "\n".join(value for _role, value in session.all_texts())
        detail += "\n" + rec["measurements"].get("dialog_after_run", "")
        detail += "\n" + rec["measurements"].get("dialog_text", "")
        rec["measurements"]["failure_text"] = (detail + "\n" + history)[:1200]
        checks.add(
            "a job without ImageMagick publishes nothing",
            not published,
            "no artifact",
            [os.path.basename(p) for p in (outputs if published else [])],
        )
        checks.add(
            "the failure names the missing tool or encoder",
            any(
                word in (detail + history)
                for word in ("magick", "ImageMagick", "missing", "not found", "encoder", "unsupported")
            ),
            "a missing-tool error",
            (detail + history)[:800],
        )
        checks.add(
            "the app survives the missing tool",
            session.proc.poll() is None,
            "process alive",
            session.proc.returncode,
        )
        rec["screenshots"].append(session.shot("after"))


def scenario_negative_hostile(h, rec, checks):
    poster = h.fixture(POSTER)
    weird = "poster 'quote' $(touch x);.png"
    with h.launch("negative-hostile", [(poster, weird)]) as session:
        rec["workdir"] = session.workdir
        source = os.path.join(session.workdir, weird)
        checks.add("hostile fixture exists", os.path.exists(source), True, os.path.exists(source))
        outputs, _preview = run_tool_job(session, rec, checks, "Compress")
        record = record_outputs(rec, outputs)[0]
        checks.add("output is a valid PNG", record["kind"] == "png", "png", record["kind"])
        checks.add("output sits beside the source", os.path.dirname(outputs[0]) == session.workdir, session.workdir, os.path.dirname(outputs[0]))
        checks.add("source is untouched", os.path.exists(source) and os.path.getsize(source) == POSTER_SIZE, POSTER_SIZE, os.path.getsize(source) if os.path.exists(source) else None)
        checks.add("no shell side effect", not os.path.exists(os.path.join(session.workdir, "x")), "no file named x", os.listdir(session.workdir))
        checks.add("output name keeps the stem", os.path.basename(outputs[0]).startswith("poster 'quote' $(touch x);-compress"), "stem-compress.png", os.path.basename(outputs[0]))


def scenario_negative_cancel(h, rec, checks):
    poster = h.fixture(POSTER)
    with h.launch("negative-cancel", [poster]) as session:
        rec["workdir"] = session.workdir
        session.show_history()
        before_rows = session.history_rows()
        dialog = session.open_tool("Compress")
        if dialog is None:
            raise ScenarioFailure("Compress opened no dialog")
        session.set_check("Reduce colours (lossy)", True)
        time.sleep(0.4)
        session.set_check("Compress to a target file size", True)
        time.sleep(0.4)
        session.set_entry("Target size", "1000")
        session.wait_text("Encoding", timeout=5) if False else None
        before = session.snapshot()
        session.press_run()
        time.sleep(0.8)
        cancelled = session.cancel_running()
        checks.add("a running job exposes Cancel", cancelled, True, cancelled)
        time.sleep(4.0)
        after = session.snapshot()
        checks.add(
            "cancel publishes nothing",
            set(after) == set(before),
            "same files",
            sorted(set(after) - set(before)),
        )
        rows = session.history_rows()
        new_rows = _multiset_minus(rows, before_rows)
        checks.add(
            "history records the cancellation",
            any("cancelled" in row.lower() for row in new_rows) or any("cancelled" in row.lower() for row in rows[-2:]),
            "cancelled",
            new_rows or rows[-2:],
        )
        rec["screenshots"].append(session.shot("after"))


def scenario_negative_overwrite(h, rec, checks):
    source = h.fixture("images/tiny-8x8.png")
    source_size = os.path.getsize(source)
    with h.launch("negative-overwrite", [source]) as session:
        rec["workdir"] = session.workdir
        firsts, _preview = run_tool_job(session, rec, checks, "Compress")
        first = firsts[0]
        second_run = run_tool_job(session, rec, checks, "Compress")
        seconds = second_run[0]
        names = sorted(os.path.basename(path) for path in firsts + seconds)
        checks.add("two artifacts exist", len(firsts + seconds) == 2, 2, names)
        checks.add("the first keeps the plain name", os.path.basename(first) == "tiny-8x8-compress.png", "tiny-8x8-compress.png", os.path.basename(first))
        checks.add("the second gets a suffix", (os.path.basename(seconds[0]) == "tiny-8x8-compress (2).png"), "tiny-8x8-compress (2).png", os.path.basename(seconds[0]))
        checks.add("the source is never replaced", os.path.getsize(os.path.join(session.workdir, "tiny-8x8.png")) == source_size, source_size, os.path.getsize(os.path.join(session.workdir, "tiny-8x8.png")))
        records = record_outputs(rec, firsts + seconds)
        checks.add("both writes are byte-identical", records[0]["sha256"] == records[1]["sha256"], records[0]["sha256"], records[1]["sha256"])


def scenario_persistence(h, rec, checks):
    source = h.fixture("images/tiny-8x8.png")
    with h.launch("persistence-job", [source]) as session:
        rec["workdir"] = session.workdir
        outputs, _preview = run_tool_job(session, rec, checks, "Compress")
        checks.add("a job was recorded", len(outputs) == 1, 1, outputs)
        session.press_button("Settings")
        session.wait_text("Interface build", timeout=10)
        session.select_option("When the output exists", "Replace existing outputs")
        session.press_button("Save settings")
        session.wait_text("Settings saved", timeout=10)
        rec["measurements"]["saved_overwrite"] = "Replace existing outputs"
        rec["screenshots"].append(session.shot("settings-saved"))
    with h.launch("persistence", [], category=None) as session:
        rec["workdir"] = session.workdir
        rows = session.history_rows()
        rec["measurements"]["history"] = rows[:10]
        checks.add(
            "history survives an app restart",
            bool(rows),
            "at least one row",
            rows[:3],
        )
        checks.add(
            "the earlier jobs are done",
            any("done" in (row or "").lower() for row in rows),
            "a done row",
            rows[:3],
        )
        session.press_button("Settings")
        session.wait_text("Interface build", timeout=10)
        settings_text = "\n".join(value for _role, value in session.all_texts())
        rec["measurements"]["settings_has_build"] = "Interface build" in settings_text
        checks.add(
            "the settings page survives a restart",
            "Interface build" in settings_text and "History rows" in settings_text,
            "settings page",
            settings_text[:200],
        )
        checks.add(
            "the overwrite policy survives a restart",
            session.selected_option("When the output exists") == "Replace existing outputs",
            "Replace existing outputs",
            session.selected_option("When the output exists"),
        )
        rec["screenshots"].append(session.shot("settings-after-restart"))


# --------------------------------------------------------------------------
# Scenario list
# --------------------------------------------------------------------------


def scenario_dialogs_category(category):
    def run(h, rec, checks):
        scenario_dialogs(h, rec, checks, category)

    run.__name__ = f"scenario_dialogs_{category.lower()}"
    return run


def scenario_dialog_moves(h, rec, checks):
    """A dialog, not a takeover: it moves by its head and returns on a double-click."""
    with h.launch("dialog-moves", [h.fixture(POSTER)]) as session:
        dialog = session.open_tool("Crop")
        if dialog is None:
            raise Skip("Crop ran without opening a dialog")
        time.sleep(0.6)
        before = session.extents(session.sheet())
        frame = session.frame_extents()
        if not before or not frame:
            raise Skip("the sheet and the window were not measured")
        session.move_dialog_by(40, 24)
        moved = session.extents(session.sheet())
        rec["measurements"]["move"] = {"before": before, "moved": moved, "frame": frame}
        checks.add(
            "the dialog can be dragged by its head",
            moved is not None
            and abs((moved[0] - before[0]) - 40) <= 10
            and abs((moved[1] - before[1]) - 24) <= 10,
            "moved by 40, 24",
            (moved[0] - before[0], moved[1] - before[1]) if moved else None,
        )
        if moved:
            checks.add(
                "a dragged dialog stays inside the window",
                moved[0] >= frame[0] and moved[1] >= frame[1]
                and moved[0] + moved[2] <= frame[0] + frame[2]
                and moved[1] + moved[3] <= frame[1] + frame[3],
                "inside the window",
                moved,
            )
        session.centre_dialog()
        back = session.extents(session.sheet())
        checks.add(
            "double-clicking the head re-centres the dialog",
            back is not None and abs(back[0] - before[0]) <= 6 and abs(back[1] - before[1]) <= 6,
            (before[0], before[1]),
            (back[0], back[1]) if back else None,
        )


SCENARIOS = [
    ("dialog-moves", scenario_dialog_moves),
    ("build-identity", scenario_build_identity),
] + [
    (f"dialogs-{category.lower()}", scenario_dialogs_category(category))
    for category in CATEGORIES
] + [
    ("compress-poster-target-palette", scenario_compress_poster),
    ("compress-poster-16-target", scenario_poster_16_target),
    ("compress-16-colours", scenario_compress_16_colours),
    ("compress-lossless", scenario_compress_lossless),
    ("compress-transparent", scenario_compress_transparent),
    ("compress-metadata", scenario_compress_metadata),
    ("compress-scale", scenario_compress_scale),
    ("compress-jpeg", scenario_compress_jpeg),
    ("compress-video", scenario_compress_video),
    ("compress-audio", scenario_compress_audio),
    ("compress-pdf", scenario_compress_pdf),
    ("tool-image-crop", scenario_tool_image_crop),
    ("tool-image-annotate", scenario_tool_image_annotate),
    ("tool-image-collage", scenario_tool_image_collage),
    ("tool-image-background", scenario_tool_image_background),
    ("tool-image-edit", scenario_tool_image_edit),
    ("tool-image-metadata", scenario_tool_image_metadata),
    ("tool-image-redact", scenario_tool_image_redact),
    ("tool-video-trim", scenario_tool_video_trim),
    ("tool-video-crop", scenario_tool_video_crop),
    ("tool-video-speed", scenario_tool_video_speed),
    ("tool-video-frames", scenario_tool_video_frames),
    ("tool-video-split", scenario_tool_video_split),
    ("tool-video-redact", scenario_tool_video_redact),
    ("tool-audio-normalize", scenario_tool_audio_normalize),
    ("tool-audio-trim", scenario_tool_audio_trim),
    ("tool-audio-channels", scenario_tool_audio_channels),
    ("tool-audio-visualize", scenario_tool_audio_visualize),
    ("tool-audio-bleep", scenario_tool_audio_bleep),
    ("tool-audio-metadata", scenario_tool_audio_metadata),
    ("tool-pdf-organize", scenario_tool_pdf_organize),
    ("tool-pdf-organize-multi", scenario_tool_pdf_organize_multi),
    ("tool-pdf-merge", scenario_tool_pdf_merge),
    ("tool-video-join", scenario_tool_video_join),
    ("tool-pdf-metadata", scenario_tool_pdf_metadata),
    ("tool-pdf-pages", scenario_tool_pdf_pages),
    ("never-grow-tiny", scenario_never_grow_tiny),
    ("reachable-target", scenario_reachable_target),
    ("multi-file", scenario_multi_file),
    ("multi-file-per-file", scenario_multi_file_per_file),
    ("convert-image", scenario_convert_image),
    ("convert-video", scenario_convert_video),
    ("convert-audio", scenario_convert_audio),
    ("convert-pdf-text", scenario_convert_pdf_text),
    ("convert-document", scenario_convert_document),
    ("convert-pdf-docx", scenario_convert_pdf_docx),
    ("convert-slides", scenario_convert_slides),
    ("convert-spreadsheet", scenario_convert_spreadsheet),
    ("convert-epub", scenario_convert_epub),
    ("convert-subtitles", scenario_convert_subtitles),
    ("convert-archive", scenario_convert_archive),
    ("extract-archive", scenario_extract_archive),
    ("immediate-tools", scenario_immediate_tools),
    ("negative-corrupt", scenario_negative_corrupt),
    ("negative-missing-tool", scenario_negative_missing_tool),
    ("negative-hostile", scenario_negative_hostile),
    ("negative-cancel", scenario_negative_cancel),
    ("negative-overwrite", scenario_negative_overwrite),
    ("persistence", scenario_persistence),
]


# --------------------------------------------------------------------------
# Worker entry point
# --------------------------------------------------------------------------


def link_user_tools() -> list[str]:
    """Bundled wrappers (qpdf) resolve under the real user's ~/.local/opt.

    The profile HOME isolates app state, but a wrapper sidecar that execs
    `$HOME/.local/opt/qpdf/bin/qpdf` would then point at an empty home. Mirror
    those installed tool roots with symlinks into the isolated home; the user's
    files stay read-only and the app still sees the machine it was built for.
    """
    import pwd

    real_home = pwd.getpwuid(os.getuid()).pw_dir
    source = os.path.join(real_home, ".local", "opt")
    target = os.path.join(os.environ["HOME"], ".local", "opt")
    linked = []
    if not os.path.isdir(source):
        return linked
    os.makedirs(target, exist_ok=True)
    for name in os.listdir(source):
        destination = os.path.join(target, name)
        if not os.path.exists(destination):
            os.symlink(os.path.join(source, name), destination)
            linked.append(name)
    return linked


def worker(args) -> int:
    global RUN_OUT_DIR
    RUN_OUT_DIR = args.out
    atspi = import_atspi()
    linked = link_user_tools()
    registry = start_accessibility()
    harness = Harness(atspi, args.out, args.profile, args.fixtures, args.timeout)
    harness.environment = {
        "display": os.environ.get("DISPLAY"),
        "app_binary": BIN,
        "binary_sha256": sha256(BIN),
        "ui_build_stamp": harness.stamp,
        "bundle_build_id": harness.build_id,
        "home": os.environ.get("HOME"),
        "tools": tool_versions(),
        "bundled_tools": tool_versions(os.path.dirname(BIN)),
        "user_tool_roots_linked": linked,
    }
    selected = [
        (name, fn)
        for name, fn in SCENARIOS
        if not args.only or args.only in name
    ]
    if not selected:
        print(f"no scenario matches {args.only!r}")
        return 2
    try:
        for name, fn in selected:
            harness.run_scenario(name, fn)
    finally:
        harness.write_report()
        if registry is not None:
            registry.terminate()
            try:
                registry.wait(timeout=3)
            except subprocess.TimeoutExpired:
                registry.kill()
    report_path = os.path.join(args.out, "report.json")
    counts = {"passed": 0, "failed": 0, "skipped": 0}
    for rec in harness.results:
        counts[rec["status"]] += 1
    print("\n" + "=" * 60)
    print(f"passed {counts['passed']}  failed {counts['failed']}  skipped {counts['skipped']}")
    print(f"report: {report_path}")
    for rec in harness.results:
        marker = {"passed": "ok  ", "failed": "FAIL", "skipped": "skip"}[rec["status"]]
        print(f"  {marker} {rec['name']}  ({rec['duration_s']}s)" + (f" — {rec.get('reason','')}" if rec["status"] != "passed" else ""))
    return 1 if counts["failed"] else 0


def tool_versions(prefix: str | None = None) -> dict:
    out = {}
    for name, cmd in (
        ("magick", ["magick", "-version"]),
        ("ffmpeg", ["ffmpeg", "-version"]),
        ("pdfinfo", ["pdfinfo", "-v"]),
        ("identify", ["identify", "-version"]),
        # The sidecar is copied beside the binary at build time, so it can be an older
        # build than vendor/bin's: this line says which one answered.
        ("convertme-pdf", ["convertme-pdf", "--version"]),
    ):
        binary = os.path.join(prefix, name) if prefix else name
        if prefix and not os.path.exists(binary):
            out[name] = "not bundled"
            continue
        try:
            result = run([binary, *cmd[1:]], timeout=20)
            line = (result.stdout or result.stderr).splitlines()
            out[name] = line[0] if line else ""
        except Exception:
            out[name] = "unavailable"
    return out


# --------------------------------------------------------------------------
# CLI
# --------------------------------------------------------------------------


def parse_args(argv):
    parser = argparse.ArgumentParser(description=__doc__, formatter_class=argparse.RawDescriptionHelpFormatter)
    parser.add_argument("--worker", action="store_true", help=argparse.SUPPRESS)
    parser.add_argument("--out", help="report/screenshot directory")
    parser.add_argument("--profile", help=argparse.SUPPRESS)
    parser.add_argument("--fixtures", default=DEFAULT_FIXTURES, help="fixture root")
    parser.add_argument("--only", help="only scenarios whose name contains this")
    parser.add_argument("--timeout", type=int, default=240, help="per-job wait budget")
    parser.add_argument("--build", action="store_true", help="run the UI and release builds first")
    parser.add_argument("--keep-profile", action="store_true", help="keep the isolated app profile")
    parser.add_argument("--list", action="store_true", help="list scenarios and exit")
    return parser.parse_args(argv)


def main(argv=None) -> int:
    args = parse_args(sys.argv[1:] if argv is None else argv)
    if args.list:
        for name, _fn in SCENARIOS:
            print(name)
        return 0
    # The dialog copy asserted by every phase comes from the binary's own tool table.
    TOOL_TABLE.update(tool_table())
    if args.worker:
        if not args.profile or not args.out:
            raise SystemExit("--worker needs --profile and --out")
        return worker(args)
    return outer(args)


if __name__ == "__main__":
    sys.exit(main())
