#!/usr/bin/env python3
"""Drive the built front end with headless Firefox and save screenshots.

No desktop input is used: Firefox runs headless and the page is clicked through
Marionette (WebDriver). The front end runs against the dev mock, so tool dialogs
render the real components while file data is fake; engine behaviour is covered
by the Rust tests and the CLI.

    scripts/ui-shots.py --out "/path/to/shots" [--category images] [--build]

Writes <category>-main.png and <category>-<tool>.png plus report.json. Exit code
is non-zero when a tool dialog is missing fields, overflows the modal, or its
preview image fails to load.

The Images pass also opens Compress with "Reduce colours" and a target size both
on, asserts the Colours select appears, and asserts the unreachable-target note
is shown for a 1,000 B target. The mock models a PNG size floor for that check;
the real encode is covered by the engine tests and the CLI.
"""
from __future__ import annotations

import argparse
import base64
import json
import os
import shutil
import socket
import subprocess
import sys
import tempfile
import threading
import time
from http.server import SimpleHTTPRequestHandler, ThreadingHTTPServer
from pathlib import Path

ROOT = Path(__file__).resolve().parent.parent
UI_DIST = ROOT / "crates/convertme-gui/ui-dist"
MARIONETTE = 2828
CATEGORIES = [
    ("images", "Images"),
    ("video", "Video"),
    ("audio", "Audio"),
    ("pdf", "PDF"),
    ("documents", "Documents"),
    ("slides", "Slides"),
    ("spreadsheets", "Spreadsheets"),
    ("epub", "EPUB"),
    ("subtitles", "Subtitles"),
    ("archives", "Archives"),
]


class Marionette:
    def __init__(self, port: int = MARIONETTE, timeout: float = 60.0):
        self.sock = socket.create_connection(("127.0.0.1", port), timeout)
        self.sock.settimeout(timeout)
        self.buf = b""
        self.seq = 0
        self._read_message()  # greeting

    def _read_message(self):
        while True:
            colon = self.buf.find(b":")
            if colon > 0:
                try:
                    size = int(self.buf[:colon])
                except ValueError:
                    size = None
                if size is not None and len(self.buf) >= colon + 1 + size:
                    payload = self.buf[colon + 1 : colon + 1 + size]
                    self.buf = self.buf[colon + 1 + size :]
                    return json.loads(payload)
            chunk = self.sock.recv(65536)
            if not chunk:
                raise RuntimeError("Marionette closed the connection")
            self.buf += chunk

    def cmd(self, name, params=None):
        self.seq += 1
        payload = json.dumps([0, self.seq, name, params or {}]).encode()
        self.sock.sendall(str(len(payload)).encode() + b":" + payload)
        while True:
            msg = self._read_message()
            if isinstance(msg, list) and len(msg) >= 2 and msg[1] == self.seq:
                if msg[2] is not None:
                    raise RuntimeError(f"{name}: {msg[2]}")
                return msg[3]

    def js(self, script, args=None):
        out = self.cmd(
            "WebDriver:ExecuteScript",
            {"script": script, "args": args or [], "scriptTimeout": 30000},
        )
        return out.get("value") if isinstance(out, dict) else out

    def js_async(self, script, args=None, timeout=15000):
        out = self.cmd(
            "WebDriver:ExecuteAsyncScript",
            {"script": script, "args": args or [], "scriptTimeout": timeout},
        )
        return out.get("value") if isinstance(out, dict) else out

    def wait_for(self, expression, timeout=8.0):
        script = """
        const done = arguments[arguments.length - 1];
        const deadline = Date.now() + %d;
        (function poll() {
          let value = null;
          try { value = (%s); } catch (e) { value = null; }
          if (value) return done(true);
          if (Date.now() > deadline) return done(false);
          setTimeout(poll, 50);
        })();
        """ % (int(timeout * 1000), expression)
        return bool(self.js_async(script, timeout=int(timeout * 1000) + 5000))

    def screenshot(self, path: Path):
        raw = self.cmd("WebDriver:TakeScreenshot", {"full": False})
        data = raw.get("value") if isinstance(raw, dict) else raw
        path.write_bytes(base64.b64decode(data))


def free_port() -> int:
    with socket.socket() as s:
        s.bind(("127.0.0.1", 0))
        return s.getsockname()[1]


REPO = Path(__file__).resolve().parent.parent
BIN = REPO / "target/release/convertme"


def tool_table() -> dict:
    """The backend tool table, when the release binary exists.

    The mock renders the same copy from `ui/src/catalog.ts`, and this table is what
    proves the two agree: a rail that disagrees with the backend is the failure the
    mock cannot see on its own.
    """
    if not BIN.is_file():
        return {}
    try:
        result = subprocess.run(
            [str(BIN), "probe", "--tools"], capture_output=True, text=True, timeout=60
        )
        payload = json.loads(result.stdout)
    except (OSError, subprocess.SubprocessError, json.JSONDecodeError):
        return {}
    return {
        (item["category"], item["label"]): item for item in payload.get("tools", [])
    }


def design_failures(info: dict, key: str, tool: str, table: dict) -> list[str]:
    """The dialog contract, measured in the browser.

    One sheet that is never the whole window, a title from the tool table, a primary
    action that names what it does, and no control painted over another.
    """
    problems = []
    sheet = info.get("sheet") or {}
    screen = info.get("screen") or {}
    # The sheet is a dialog, not the window: it keeps a margin on both sides.
    if sheet.get("x", 0) < 8 or sheet.get("y", 0) < 8:
        problems.append(f"{key}/{tool}: the sheet touches the window edge")
    if sheet.get("x", 0) + sheet.get("w", 0) > screen.get("w", 0) - 8:
        problems.append(f"{key}/{tool}: the sheet touches the right edge")
    if sheet.get("w", 0) > screen.get("w", 0) * 0.85:
        problems.append(f"{key}/{tool}: the sheet is {sheet.get('w')} px wide, near full screen")
    if not info.get("sheetTitle"):
        problems.append(f"{key}/{tool}: the sheet has no title")
    if not info.get("primary"):
        problems.append(f"{key}/{tool}: the footer has no primary action")
    # A row whose label and control do not share a centre line is the grammar broken.
    for row in info.get("rows", []):
        if row.get("block"):
            continue
        if row.get("offset", 0) > 4:
            problems.append(
                f"{key}/{tool}: {row['label']!r} and its control are {row['offset']}px apart"
            )
    meta = table.get((key, tool))
    if meta:
        if info.get("sheetTitle") != meta["title"]:
            problems.append(
                f"{key}/{tool}: sheet title {info.get('sheetTitle')!r} != backend {meta['title']!r}"
            )
        expected = meta["action"]
        if expected.count("{n}") == 0 and info.get("primary") != expected:
            problems.append(
                f"{key}/{tool}: primary {info.get('primary')!r} != backend {expected!r}"
            )
    return problems


def serve(directory: Path):
    class Quiet(SimpleHTTPRequestHandler):
        def log_message(self, *_args):
            pass

    httpd = ThreadingHTTPServer(("127.0.0.1", free_port()), lambda *a, **k: Quiet(*a, directory=str(directory), **k))
    threading.Thread(target=httpd.serve_forever, daemon=True).start()
    return httpd


def start_firefox(profile: Path):
    env = dict(os.environ, MOZ_HEADLESS="1", MOZ_FORCE_DISABLE_E10S="1")
    # A fresh profile has no marionette preference, and Firefox 15x accepts the
    # connection then closes it without a greeting. Write the prefs before launch.
    profile.mkdir(parents=True, exist_ok=True)
    (profile / "user.js").write_text(
        "\n".join(
            [
                'user_pref("marionette.enabled", true);',
                f'user_pref("marionette.port", {MARIONETTE});',
                'user_pref("marionette.debugging.clicktostart", false);',
                'user_pref("browser.shell.checkDefaultBrowser", false);',
                'user_pref("browser.startup.page", 0);',
                "",
            ]
        )
    )
    proc = subprocess.Popen(
        ["firefox", "--headless", "--no-remote", "--marionette", "--profile", str(profile), "about:blank"],
        env=env,
        stdout=subprocess.DEVNULL,
        stderr=subprocess.DEVNULL,
    )
    deadline = time.time() + 30
    while time.time() < deadline:
        try:
            with socket.create_connection(("127.0.0.1", MARIONETTE), 1):
                return proc
        except OSError:
            time.sleep(0.2)
    proc.terminate()
    raise RuntimeError("Marionette did not start (port 2828 busy? close other Firefox runs)")


def measure_modal(m: Marionette):
    script = """
    const rect = (el) => {
      if (!el) return null;
      const r = el.getBoundingClientRect();
      return { x: Math.round(r.x), y: Math.round(r.y), w: Math.round(r.width), h: Math.round(r.height) };
    };
    // One sheet: the tool's copy lives in the card that opened it, not beside it.
    const sheet = document.querySelector('.dialog-sheet') || document.querySelector('.modal');
    const modal = document.querySelector('.modal') || sheet;
    if (!modal) return { dialog: false };
    const sheetTitle = (document.querySelector('.sheet-title') || {}).innerText || '';
    const primary = document.querySelector('[data-testid="primary"]');
    // A row is aligned when the label's and its control's vertical centres agree.
    const rows = [...document.querySelectorAll('.modal .field')].map((row) => {
      const label = row.querySelector('.field-label');
      // Measure the control the user sees, not the first input inside it: the colour
      // row hides a native <input type=color> whose own box is not the control.
      const control = row.querySelector(
        '.colour-control, .segmented, .range-row, .number-row, .checkbox-row, select, input, textarea'
      );
      if (!label || !control) return null;
      const a = label.getBoundingClientRect();
      const b = control.getBoundingClientRect();
      return {
        label: label.innerText.trim(),
        kind: control.tagName.toLowerCase() + (control.type ? ':' + control.type : ''),
        offset: Math.round(Math.abs((a.top + a.height / 2) - (b.top + b.height / 2))),
        block: row.classList.contains('is-block'),
      };
    }).filter(Boolean);
    const fields = [...document.querySelectorAll('.modal .field-label')].map((l) => l.innerText);
    const controls = [...document.querySelectorAll('.modal input, .modal select, .modal textarea')].map((el) => ({
      tag: el.tagName.toLowerCase(), type: el.type || '', value: el.value ?? '',
    }));
    const img = document.querySelector('.compress-image');
    const preview = document.querySelector('.compress-preview');
    const modalRect = rect(modal);
    let overflow = false;
    if (modalRect) {
      for (const el of modal.querySelectorAll('.modal-fields *')) {
        const r = el.getBoundingClientRect();
        if (r.width > 0 && (r.right > modalRect.x + modalRect.w + 1 || r.left < modalRect.x - 1)) {
          overflow = true;
          break;
        }
      }
    }
    // Overlapping controls: a row drawn on top of another row is a layout defect the
    // eye catches and a screenshot diff does not. Marks drawn over a canvas are the
    // editor's own overlay and are excluded.
    const OVERLAY = '.crop-region, .crop-handle, .crop-shade, .boxes-region, .boxes-handle,'
      + ' .annotate-shape, .annotate-handle, .waveform-range, .waveform-selection,'
      + ' .waveform-playhead, .waveform-marker, .timeline-playhead, .timeline-selection,'
      + ' .timeline-marker, .colour-popover, .fill-preset, .stop-row';
    const OUTSIDE = '.button, .modal-footer';
    const measured = [...document.querySelectorAll(
      '.modal-fields .field-label, .modal-fields .field-help, .modal-fields input,'
      + ' .modal-fields select, .modal-fields output, .modal-fields .segmented,'
      + ' .modal-fields .metadata-row, .modal-fields .crop-surface, .modal-fields .modal-file'
    )]
      .filter((el) => !el.closest(OVERLAY) && !el.closest(OUTSIDE))
      .map((el) => ({ el, r: el.getBoundingClientRect() }))
      .filter((item) => item.r.width > 1 && item.r.height > 1);
    const overlaps = [];
    for (let i = 0; i < measured.length; i += 1) {
      for (let j = i + 1; j < measured.length; j += 1) {
        const a = measured[i];
        const b = measured[j];
        if (a.el.contains(b.el) || b.el.contains(a.el)) continue;
        const w = Math.min(a.r.right, b.r.right) - Math.max(a.r.left, b.r.left);
        const h = Math.min(a.r.bottom, b.r.bottom) - Math.max(a.r.top, b.r.top);
        if (w <= 1 || h <= 1) continue;
        const smaller = Math.min(a.r.width * a.r.height, b.r.width * b.r.height);
        if ((w * h) / smaller < 0.35) continue;
        const name = (el) => (el.innerText || el.getAttribute('aria-label') || el.value
          || el.tagName).trim().slice(0, 28);
        overlaps.push(`${name(a.el)} / ${name(b.el)}`);
      }
    }
    const scroller = document.querySelector('.modal-fields');
    return {
      overlaps: [...new Set(overlaps)].slice(0, 12),
      dialog: true,
      labels: fields,
      controls,
      modal: modalRect,
      screen: { w: window.innerWidth, h: window.innerHeight },
      sheet: rect(sheet),
      sheetTitle: sheetTitle.trim(),
      primary: primary ? primary.innerText.trim() : '',
      primaryBox: rect(primary),
      rows,
      preview: preview ? rect(preview) : null,
      previewLoaded: img ? !!(img.complete && img.naturalWidth > 0) : null,
      overflow,
      scroll: scroller ? { client: scroller.clientHeight, content: scroller.scrollHeight } : null,
      facts: (document.querySelector('.compress-facts') || {}).innerText || '',
    };
    """
    return m.js(script)


def open_category(m: Marionette, label: str):
    ok = m.js(
        """
        const b = [...document.querySelectorAll('.category-button')]
          .find((x) => x.innerText.trim() === %s);
        if (!b) return false;
        b.click();
        return true;
        """
        % json.dumps(label)
    )
    if not ok:
        raise RuntimeError(f"category button {label!r} not found")
    if not m.wait_for(
        "(() => { const meta = document.querySelector('.tools-panel .panel-meta');"
        f" return meta && meta.innerText.startsWith({json.dumps(label)}); }})()"
    ):
        raise RuntimeError(f"tool list did not switch to {label}")


def close_dialog(m: Marionette):
    """The X in the sheet head: the reference dialog has no Cancel button."""
    m.js(
        """
        const close = document.querySelector('.sheet-close');
        if (close) close.click();
        return true;
        """
    )
    m.wait_for("!document.querySelector('.modal')")


def open_browser() -> tuple[Marionette, subprocess.Popen, Path]:
    profile = Path(tempfile.mkdtemp(prefix="convertme-shots-"))
    firefox = start_firefox(profile)
    try:
        m = Marionette()
        m.cmd("WebDriver:NewSession", {"capabilities": {}})
        m.cmd("WebDriver:SetWindowRect", {"x": 0, "y": 0, "width": 1280, "height": 940})
    except Exception:
        stop_browser(firefox, profile)
        raise
    return m, firefox, profile


def stop_browser(firefox: subprocess.Popen, profile: Path) -> None:
    firefox.terminate()
    try:
        firefox.wait(timeout=5)
    except subprocess.TimeoutExpired:
        firefox.kill()
        firefox.wait(timeout=5)
    shutil.rmtree(profile, ignore_errors=True)


def run_category(
    key: str,
    label: str,
    base: str,
    out: Path,
    table: dict,
    failures: list[str],
) -> list[dict]:
    """One category in a fresh browser.

    A long single-browser session accumulated renderer channel errors on this host
    (the PDF category died after the fourth dialog); restarting per category keeps a
    crash from taking the rest of the report with it.
    """
    report: list[dict] = []
    m, firefox, profile = open_browser()
    try:
        m.cmd("WebDriver:Navigate", {"url": f"{base}?mock-files={key}"})
        if not m.wait_for("document.querySelector('.tool-card')", 10):
            failures.append(f"{key}: tool cards never appeared")
            return report
        open_category(m, label)
        time.sleep(0.3)
        m.screenshot(out / f"{key}-main.png")
        tools = m.js(
            "return [...document.querySelectorAll('.tool-card')].map((c) => "
            "(c.querySelector('.tool-label') || c).innerText.trim());"
        )
        print(f"{key}: {len(tools)} tools", flush=True)
        if key == "images":
            # Lossless-only selection: the quality slider must be absent.
            m.cmd("WebDriver:Navigate", {"url": f"{base}?mock-files=images-lossless"})
            m.wait_for("document.querySelector('.tool-card')", 10)
            open_category(m, "Images")
            m.js("document.querySelectorAll('.tool-card')[0].click(); return true;")
            if m.wait_for("document.querySelector('.modal')", 4):
                time.sleep(0.8)
                lossless = measure_modal(m)
                lossless.update({"category": "images-lossless", "tool": "compress",
                                 "screenshot": "images-lossless-compress.png"})
                m.screenshot(out / lossless["screenshot"])
                report.append(lossless)
                if "Quality" in lossless.get("labels", []):
                    failures.append("images-lossless/compress: Quality is shown for a lossless selection")
                if lossless.get("preview") and not lossless.get("previewLoaded"):
                    failures.append("images-lossless/compress: preview did not load")
                # Both options at once, with a target no PNG can reach: the palette
                # control appears and the dialog says the target is out of reach.
                m.js("""
                const rows = [...document.querySelectorAll('.checkbox-row')];
                for (const prefix of ['Reduce colours', 'Compress to a target']) {
                  const row = rows.find((r) => r.innerText.trim().startsWith(prefix));
                  if (row && !row.querySelector('input').checked) row.click();
                }
                const input = document.querySelector('.modal input[type=number]');
                if (input) {
                  const setter = Object.getOwnPropertyDescriptor(HTMLInputElement.prototype, 'value').set;
                  setter.call(input, '1000');
                  input.dispatchEvent(new Event('input', {bubbles: true}));
                }
                return true;
                """)
                noted = m.wait_for("document.querySelector('.compress-target-miss')", 6)
                time.sleep(0.3)
                combined = measure_modal(m)
                combined.update({
                    "category": "images-lossless",
                    "tool": "compress-combined",
                    "screenshot": "images-lossless-compress-combined.png",
                    "note": m.js("const n = document.querySelector('.compress-target-miss'); "
                                 "return n ? n.innerText.trim() : null;"),
                })
                m.screenshot(out / combined["screenshot"])
                report.append(combined)
                if not noted:
                    failures.append("images-lossless/compress: no out-of-reach target note")
                if "Colours" not in combined.get("labels", []):
                    failures.append("images-lossless/compress: Colours did not appear with Reduce colours")
                if combined.get("preview") and not combined.get("previewLoaded"):
                    failures.append("images-lossless/compress-combined: preview did not load")
                close_dialog(m)
            else:
                failures.append("images-lossless/compress: no dialog")
        if key == "images":
            # The aspect pills must produce the ratio they name, in the picture's own
            # pixels. A box drawn on a letterboxed preview is square on the black bars
            # and wrong in the file, which is exactly what shipped once.
            m.cmd("WebDriver:Navigate", {"url": f"{base}?mock-files=images"})
            m.wait_for("document.querySelector('.tool-card')", 10)
            open_category(m, "Images")
            m.js("document.querySelectorAll('.tool-card')[1].click(); return true;")
            if m.wait_for("document.querySelector('.crop-frame')", 6):
                for label, expected in (("1:1", 1.0), ("9:16", 9 / 16)):
                    m.js(
                        "const b = [...document.querySelectorAll('.segmented button')]"
                        f".find((x) => x.innerText.trim() === {json.dumps(label)});"
                        "if (b) b.click(); return !!b;"
                    )
                    time.sleep(0.5)
                    probe = m.js(
                        """
                        const frame = document.querySelector('.crop-frame');
                        const region = document.querySelector('.crop-region');
                        const inputs = [...document.querySelectorAll('.crop-pixel input')];
                        if (!frame || !region || inputs.length < 2) return null;
                        const fr = frame.getBoundingClientRect();
                        const rr = region.getBoundingClientRect();
                        return {
                          frame: { w: Math.round(fr.width), h: Math.round(fr.height) },
                          region: { w: Math.round(rr.width), h: Math.round(rr.height) },
                          screenRatio: rr.width / rr.height,
                          pixelRatio: Number(inputs[0].value) / Number(inputs[1].value),
                          pixels: inputs.map((i) => Number(i.value)),
                        };
                        """
                    )
                    report.append(
                        {"category": "images", "tool": f"crop-{label}", "dialog": True,
                         "probe": probe}
                    )
                    if not probe:
                        failures.append(f"images/crop: no crop frame for {label}")
                        continue
                    if abs(probe["screenRatio"] - expected) > 0.02:
                        failures.append(
                            f"images/crop: {label} draws a {probe['screenRatio']:.3f} box"
                        )
                    if abs(probe["pixelRatio"] - expected) > 0.02:
                        failures.append(
                            f"images/crop: {label} saves {probe['pixels'][0]}x"
                            f"{probe['pixels'][1]} px"
                        )
                close_dialog(m)
            else:
                failures.append("images/crop: the crop surface never appeared")
        if key == "video":
            # Typing the output size must work before anything is drawn: the reference
            # dialog lets you enter exact dimensions first.
            m.cmd("WebDriver:Navigate", {"url": f"{base}?mock-files={key}"})
            m.wait_for("document.querySelector('.tool-card')", 10)
            open_category(m, "Video")
            m.js(
                "const card = [...document.querySelectorAll('.tool-card')]"
                ".find((c) => c.innerText.trim().startsWith('Crop'));"
                "if (card) card.click(); return !!card;"
            )
            if m.wait_for("document.querySelector('.modal')", 6):
                time.sleep(1.0)
                typed = m.js(
                    """
                    const input = [...document.querySelectorAll('.modal input[type=number]')]
                      .find((el) => el.getAttribute('aria-label') === 'Width');
                    if (!input) return { found: false };
                    const setter = Object.getOwnPropertyDescriptor(
                      HTMLInputElement.prototype, 'value').set;
                    setter.call(input, '640');
                    input.dispatchEvent(new Event('input', { bubbles: true }));
                    return { found: true, value: input.value };
                    """
                )
                time.sleep(0.4)
                after = m.js(
                    """
                    const input = [...document.querySelectorAll('.modal input[type=number]')]
                      .find((el) => el.getAttribute('aria-label') === 'Width');
                    return input ? input.value : null;
                    """
                )
                report.append(
                    {
                        "category": key,
                        "tool": "crop-typed-width",
                        "dialog": True,
                        "typed": typed,
                        "value_after": after,
                    }
                )
                if not (typed or {}).get("found") or after != "640":
                    failures.append(f"video/crop: typing 640 as the width left {after!r}")
                close_dialog(m)

        if key == "pdf":
            # The organizer's own controls: rotating a card must leave a rotation on that
            # card, and the value is what the dialog submits.
            m.cmd("WebDriver:Navigate", {"url": f"{base}?mock-files={key}"})
            m.wait_for("document.querySelector('.tool-card')", 10)
            open_category(m, "PDF")
            m.js(
                "const card = [...document.querySelectorAll('.tool-card')]"
                ".find((c) => c.innerText.trim().startsWith('Organize'));"
                "if (card) card.click(); return !!card;"
            )
            if m.wait_for("document.querySelector('.page-card')", 6):
                time.sleep(0.8)
                rotated = m.js(
                    """
                    const button = [...document.querySelectorAll('.page-card button')]
                      .find((b) => b.getAttribute('aria-label') === 'Rotate page 1');
                    if (!button) return { found: false };
                    button.click();
                    return { found: true };
                    """
                )
                time.sleep(0.5)
                state = m.js(
                    """
                    const card = document.querySelector('.page-card');
                    const meta = card ? card.innerText : '';
                    return { meta, rotated: /90°/.test(meta) };
                    """
                )
                report.append(
                    {"category": key, "tool": "organize-rotate", "dialog": True,
                     "rotated": state.get("rotated"), "meta": state.get("meta")}
                )
                if not rotated.get("found") or not state.get("rotated"):
                    failures.append(
                        f"pdf/organize: rotating page 1 left {state.get('meta')!r}"
                    )
                close_dialog(m)

        for index, tool in enumerate(tools):
            m.js(
                "document.querySelectorAll('.tool-card')[%d].click(); return true;" % index
            )
            had_dialog = m.wait_for("document.querySelector('.modal')", 4)
            if not had_dialog:
                report.append({"category": key, "tool": tool, "dialog": False})
                time.sleep(0.3)
                continue
            time.sleep(0.8)
            info = measure_modal(m)
            info.update({"category": key, "tool": tool, "screenshot": f"{key}-{tool.lower().replace(' ', '-')}.png"})
            m.screenshot(out / info["screenshot"])
            report.append(info)
            modal = info.get("modal") or {}
            screen = info.get("screen") or {}
            if modal and (modal.get("w", 0) > screen.get("w", 0) or modal.get("h", 0) > screen.get("h", 0)):
                failures.append(f"{key}/{tool}: modal larger than the viewport")
            failures.extend(design_failures(info, key, tool, table))
            for pair in info.get("overlaps", []):
                failures.append(f"{key}/{tool}: overlapping controls: {pair}")
            if info.get("overflow"):
                failures.append(f"{key}/{tool}: a field overflows the modal horizontally")
            if info.get("preview") and not info.get("previewLoaded"):
                failures.append(f"{key}/{tool}: the preview image did not load")
            close_dialog(m)
        entries = [r for r in report if r["category"] == key]
        print(f"  captured {sum(1 for r in entries if r.get('dialog'))} dialogs of {len(entries)} tools", flush=True)
    finally:
        stop_browser(firefox, profile)
    return report


def main() -> int:
    parser = argparse.ArgumentParser()
    parser.add_argument("--out", required=True)
    parser.add_argument("--category")
    parser.add_argument("--build", action="store_true")
    args = parser.parse_args()

    if args.build:
        subprocess.run(["npm", "--prefix", str(ROOT / "ui"), "run", "build"], check=True)
    if not (UI_DIST / "index.html").exists():
        print("ui-dist is missing; run with --build", file=sys.stderr)
        return 2

    out = Path(args.out).expanduser()
    out.mkdir(parents=True, exist_ok=True)
    httpd = serve(UI_DIST)
    base = f"http://127.0.0.1:{httpd.server_port}/index.html"
    failures: list[str] = []
    report: list[dict] = []
    table = tool_table()
    try:
        categories = [(k, v) for k, v in CATEGORIES if not args.category or k == args.category]
        for key, label in categories:
            try:
                report.extend(run_category(key, label, base, out, table, failures))
            except RuntimeError as error:
                failures.append(f"{key}: browser session failed ({error})")
        (out / "report.json").write_text(json.dumps(report, indent=2))
    finally:
        httpd.shutdown()

    print(f"screenshots: {out}", flush=True)
    if failures:
        print("FAILURES:", file=sys.stderr)
        for failure in failures:
            print(f"  - {failure}", file=sys.stderr)
        return 1
    dialogs = sum(1 for r in report if r.get("dialog"))
    print(f"ok: {dialogs} dialogs of {len(report)} tools, no layout failures", flush=True)
    return 0


if __name__ == "__main__":
    sys.exit(main())
