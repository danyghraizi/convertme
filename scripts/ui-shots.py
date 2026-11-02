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
    # The field grammar is a stacked row: the label starts at the same left edge as
    # its control, and the control fills the row. A shrink-wrapped control (the old
    # flex-column `align-self: start`) leaves a short pill beside a long input.
    for row in info.get("rows", []):
        if row.get("block"):
            continue
        if row.get("offset", 0) > 4:
            problems.append(
                f"{key}/{tool}: {row['label']!r} does not start above its control "
                f"({row['offset']}px off)"
            )
        if row.get("fill", 1) < 0.9:
            problems.append(
                f"{key}/{tool}: {row['label']!r} control fills only "
                f"{round(row['fill'] * 100)}% of its row"
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
    // The stacked grammar: the label's left edge and the control's left edge agree,
    // and the control fills the row. Measure the control the user sees, not the
    // first input inside it (the colour row hides a native <input type=color>).
    const rows = [...document.querySelectorAll('.modal .field')].map((row) => {
      // A checkbox is its own `<label class="field checkbox-row">`; its control is
      // the box, not a form row with a label above it. Measuring it compared the
      // 18 px box with the full row.
      if (row.classList.contains('checkbox-row')) return null;
      // Only a row that owns its label directly is a form row.
      const label = row.querySelector(':scope > .field-label');
      const control = row.querySelector(
        '.colour-control, .gradient-editor, .file-row, .segmented, .range-row,'
        + ' .number-row, .checkbox-row, select, input, textarea'
      );
      if (!label || !control) return null;
      const a = label.getBoundingClientRect();
      const b = control.getBoundingClientRect();
      const r = row.getBoundingClientRect();
      return {
        label: label.innerText.trim(),
        kind: control.tagName.toLowerCase() + (control.type ? ':' + control.type : ''),
        offset: Math.round(Math.abs(a.left - b.left)),
        fill: r.width > 0 ? b.width / r.width : 1,
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


def layout_sample(m: Marionette) -> dict:
    """Where the controls sit while a preview renders.

    The dialog's preview band is a sibling above the scroller. A status line that is
    inserted into flow grows the band and pushes every control down; the user sees the
    sliders jump. Sampling this during and after a render makes that measurable.
    """
    return m.js(
        """
        const fields = document.querySelector('.modal-fields');
        const preview = document.querySelector('.modal-preview');
        const status = document.querySelector(
          '.preview-status, .compress-working, .boxes-status, .annotate-status');
        return {
          fieldsTop: fields ? Math.round(fields.getBoundingClientRect().top) : null,
          fieldsH: fields ? Math.round(fields.getBoundingClientRect().height) : null,
          previewH: preview ? Math.round(preview.getBoundingClientRect().height) : null,
          status: status ? ((status.innerText || '').trim() || status.className) : null,
        };
        """
    )


def pointer_actions(m: Marionette, moves) -> None:
    """Real pointer input through Marionette: a synthetic `dispatchEvent` has no
    active pointer id, so `setPointerCapture` in a drag surface would throw."""
    actions = [{"type": "pointerMove", "duration": 0, "x": moves[0][0], "y": moves[0][1],
                "origin": "viewport"}, {"type": "pointerDown", "button": 0}]
    for x, y in moves[1:]:
        actions.append({"type": "pointerMove", "duration": 80, "x": x, "y": y,
                        "origin": "viewport"})
    actions.append({"type": "pointerUp", "button": 0})
    m.cmd(
        "WebDriver:PerformActions",
        {"actions": [{"type": "pointer", "id": "mouse",
                      "parameters": {"pointerType": "mouse"}, "actions": actions}]},
    )
    m.cmd("WebDriver:ReleaseActions")


def pointer_press_move(m: Marionette, moves) -> None:
    """Press and move without releasing, so the drag can be measured mid-gesture."""
    actions = [{"type": "pointerMove", "duration": 0, "x": moves[0][0], "y": moves[0][1],
                "origin": "viewport"}, {"type": "pointerDown", "button": 0}]
    for x, y in moves[1:]:
        actions.append({"type": "pointerMove", "duration": 80, "x": x, "y": y,
                        "origin": "viewport"})
    m.cmd(
        "WebDriver:PerformActions",
        {"actions": [{"type": "pointer", "id": "mouse",
                      "parameters": {"pointerType": "mouse"}, "actions": actions}]},
    )


def pointer_release(m: Marionette) -> None:
    m.cmd(
        "WebDriver:PerformActions",
        {"actions": [{"type": "pointer", "id": "mouse",
                      "parameters": {"pointerType": "mouse"},
                      "actions": [{"type": "pointerUp", "button": 0}]}]},
    )
    m.cmd("WebDriver:ReleaseActions")


def surface_points(m: Marionette, selector: str, points) -> list[tuple[int, int]]:
    box = m.js(
        """
        const el = document.querySelector(%s);
        if (!el) return null;
        const r = el.getBoundingClientRect();
        return { x: r.x, y: r.y, w: r.width, h: r.height };
        """
        % json.dumps(selector)
    )
    if not box or box.get("w", 0) <= 1:
        raise RuntimeError(f"{selector} has no measurable extents")
    return [
        (int(box["x"] + box["w"] * fx), int(box["y"] + box["h"] * fy)) for fx, fy in points
    ]


def set_field_value(m: Marionette, label: str, value: str) -> bool:
    """Set one dialog control by its row label, through the native value setter."""
    return bool(
        m.js(
            """
            const label = %s;
            const value = %s;
            const row = [...document.querySelectorAll('.modal .field')]
              .find((r) => {
                const own = r.querySelector(':scope > .field-label');
                return own && own.innerText.trim() === label;
              })
              || [...document.querySelectorAll('.modal .field')]
                .find((r) => (r.innerText || '').trim().startsWith(label));
            if (!row) return false;
            const input = row.querySelector('input[type=range], input[type=number],'
              + ' input[type=text], select');
            if (!input) return false;
            if (input.tagName === 'SELECT') {
              const option = [...input.options].find((o) => o.value === value);
              if (!option) return false;
              input.value = value;
              input.dispatchEvent(new Event('change', { bubbles: true }));
              return true;
            }
            const proto = input.type === 'range'
              ? HTMLInputElement.prototype : HTMLInputElement.prototype;
            Object.getOwnPropertyDescriptor(proto, 'value').set.call(input, value);
            input.dispatchEvent(new Event('input', { bubbles: true }));
            input.dispatchEvent(new Event('change', { bubbles: true }));
            return true;
            """
            % (json.dumps(label), json.dumps(value))
        )
    )


def watch_layout(m: Marionette, step, rounds: int = 24, gap: float = 0.05) -> dict:
    """Run `step(index)`, sampling the sheet throughout and after the render."""
    samples = [layout_sample(m)]
    for index in range(rounds):
        step(index)
        time.sleep(gap)
        samples.append(layout_sample(m))
    time.sleep(0.8)
    for _ in range(6):
        samples.append(layout_sample(m))
        time.sleep(0.08)
    tops = [s["fieldsTop"] for s in samples if s.get("fieldsTop") is not None]
    heights = [s["previewH"] for s in samples if s.get("previewH") is not None]
    return {
        "samples": len(samples),
        "fields_top": [min(tops), max(tops)] if tops else None,
        "preview_height": [min(heights), max(heights)] if heights else None,
        "status_seen": sorted({s["status"] for s in samples if s.get("status")})[:4],
    }


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

            # Annotate: a real drag must draw, Number must place a badge on a click,
            # and Eraser must remove the clicked mark. A synthetic `dispatchEvent` has
            # no pointer id, so this uses WebDriver pointer actions.
            m.js(
                "const card = [...document.querySelectorAll('.tool-card')]"
                ".find((c) => ((c.querySelector('.tool-label') || c).innerText || '')"
                ".trim().startsWith('Annotate'));"
                "if (card) card.click(); return !!card;"
            )
            if m.wait_for("document.querySelector('.annotate-frame')", 6):
                time.sleep(0.8)
                try:
                    def choose_tool(name: str) -> bool:
                        return bool(
                            m.js(
                                "const b = [...document.querySelectorAll('.annotate-tool')]"
                                f".find((x) => x.innerText.trim() === {json.dumps(name)});"
                                "if (b) b.click(); return !!b;"
                            )
                        )

                    def shape_count() -> int:
                        return int(
                            m.js(
                                "return document.querySelectorAll("
                                "'.annotate-shapes .annotate-shape').length;"
                            )
                            or 0
                        )

                    drawn = choose_tool("Rectangle")
                    points = surface_points(
                        m, ".annotate-frame", [(0.25, 0.30), (0.55, 0.65)]
                    )
                    pointer_actions(m, points)
                    time.sleep(0.4)
                    after_rect = shape_count()

                    numbered = choose_tool("Number")
                    number_at = surface_points(m, ".annotate-frame", [(0.78, 0.22)])[0]
                    pointer_actions(m, [number_at, number_at])
                    time.sleep(0.4)
                    after_number = shape_count()

                    erased = choose_tool("Eraser")
                    pointer_actions(m, [number_at, number_at])
                    time.sleep(0.4)
                    after_erase = shape_count()

                    report.append(
                        {
                            "category": "images",
                            "tool": "annotate-interaction",
                            "dialog": True,
                            "tools": {"rectangle": drawn, "number": numbered, "eraser": erased},
                            "shapes": {
                                "after_drag": after_rect,
                                "after_number": after_number,
                                "after_erase": after_erase,
                            },
                        }
                    )
                    if not drawn or after_rect < 1:
                        failures.append("images/annotate: dragging drew no rectangle")
                    if not numbered or after_number <= after_rect:
                        failures.append("images/annotate: Number did not place a badge")
                    if not erased or after_erase >= after_number:
                        failures.append("images/annotate: Eraser did not remove the badge")

                    # A drawing tool must not paint the selection ring over the shape it
                    # just added (the report: "one rectangle is bugged with a selection
                    # outline"), and the number the user types must be the one placed.
                    choose_tool("Rectangle")
                    rect_points = surface_points(
                        m, ".annotate-frame", [(0.1, 0.12), (0.3, 0.3)]
                    )
                    pointer_actions(m, rect_points)
                    time.sleep(0.3)
                    ring = m.js(
                        "return !!document.querySelector('.annotate-selection');"
                    )
                    choose_tool("Number")
                    typed = m.js(
                        """
                        const input = document.querySelector(
                          '.annotate-digit input[aria-label="Next number"]');
                        if (!input) return false;
                        const setter = Object.getOwnPropertyDescriptor(
                          HTMLInputElement.prototype, 'value').set;
                        setter.call(input, '7');
                        input.dispatchEvent(new Event('input', { bubbles: true }));
                        return true;
                        """
                    )
                    badge_at = surface_points(m, ".annotate-frame", [(0.6, 0.35)])[0]
                    pointer_actions(m, [badge_at, badge_at])
                    time.sleep(0.4)
                    badge_text = m.js(
                        """
                        const texts = [...document.querySelectorAll(
                          '.annotate-shapes .annotate-shape')]
                          .filter((n) => n.tagName.toLowerCase() === 'text')
                          .map((n) => (n.textContent || '').trim());
                        return texts.includes('7');
                        """
                    )
                    # Set a numeric annotation property by its accessible name.
                    def set_annotation_size(value: str) -> bool:
                        return bool(
                            m.js(
                                """
                                const input = document.querySelector(
                                  '.annotate-size input.annotate-size-value');
                                if (!input) return false;
                                const setter = Object.getOwnPropertyDescriptor(
                                  HTMLInputElement.prototype, 'value').set;
                                setter.call(input, %s);
                                input.dispatchEvent(new Event('input', { bubbles: true }));
                                return true;
                                """
                                % json.dumps(value)
                            )
                        )

                    # A thick arrow's head lines must use round caps: butt caps at the
                    # tip left wedge-shaped gaps at high stroke widths.
                    choose_tool("Arrow")
                    set_annotation_size("13")
                    arrow_points = surface_points(
                        m, ".annotate-frame", [(0.62, 0.72), (0.9, 0.8)]
                    )
                    pointer_actions(m, arrow_points)
                    time.sleep(0.3)
                    arrow = m.js(
                        """
                        const lines = [...document.querySelectorAll(
                          '.annotate-shapes line')];
                        return {
                          count: lines.length,
                          round: lines.length > 0 && lines.every((l) =>
                            l.getAttribute('stroke-linecap') === 'round'
                            && l.getAttribute('stroke-linejoin') === 'round'),
                          width: lines.length ? lines[0].getAttribute('stroke-width') : null,
                        };
                        """
                    )

                    # A caption above the old 200 px cap: type the size, place a text,
                    # commit it, then double-click it. The editor must be the only copy
                    # on screen (no ghosted SVG text underneath).
                    choose_tool("Text")
                    sized = set_annotation_size("300")
                    size_max = m.js(
                        """
                        const input = document.querySelector(
                          '.annotate-size input.annotate-size-value');
                        return input ? input.getAttribute('max') : null;
                        """
                    )
                    text_at = surface_points(
                        m, ".annotate-frame", [(0.5, 0.5)]
                    )[0]
                    pointer_actions(m, [text_at, text_at])
                    time.sleep(0.3)
                    committed = m.js(
                        """
                        const area = document.querySelector('.annotate-text-editor');
                        if (!area) return false;
                        const setter = Object.getOwnPropertyDescriptor(
                          HTMLTextAreaElement.prototype, 'value').set;
                        setter.call(area, 'Z');
                        area.dispatchEvent(new Event('input', { bubbles: true }));
                        area.dispatchEvent(new KeyboardEvent('keydown',
                          {key: 'Enter', bubbles: true}));
                        return true;
                        """
                    )
                    time.sleep(0.4)
                    text_state = m.js(
                        """
                        const texts = [...document.querySelectorAll(
                          '.annotate-shapes text')];
                        const z = texts.find((n) => (n.textContent || '').trim() === 'Z');
                        return { present: !!z, size: z ? z.getAttribute('font-size') : null };
                        """
                    )
                    choose_tool("Select")
                    m.js(
                        """
                        const frame = document.querySelector('.annotate-frame');
                        if (!frame) return false;
                        const r = frame.getBoundingClientRect();
                        const event = new MouseEvent('dblclick', {
                          bubbles: true,
                          clientX: r.x + r.width * 0.52,
                          clientY: r.y + r.height * 0.55,
                        });
                        frame.dispatchEvent(event);
                        return true;
                        """
                    )
                    time.sleep(0.4)
                    editing = m.js(
                        """
                        return {
                          editor: !!document.querySelector('.annotate-text-editor'),
                          ghost: [...document.querySelectorAll('.annotate-shapes text')]
                            .some((n) => (n.textContent || '').trim() === 'Z'),
                          ring: !!document.querySelector('.annotate-selection'),
                        };
                        """
                    )

                    report.append(
                        {"category": "images", "tool": "annotate-polish",
                         "dialog": True, "selection_ring_in_draw_tool": bool(ring),
                         "typed_number": bool(typed), "badge_is_7": bool(badge_text),
                         "arrow": arrow, "size_typed": bool(sized), "size_max": size_max,
                         "text": text_state, "editing": editing}
                    )
                    if ring:
                        failures.append(
                            "images/annotate: a drawing tool painted the selection ring"
                        )
                    if not typed:
                        failures.append("images/annotate: no Next number input")
                    if not badge_text:
                        failures.append("images/annotate: the typed number was not placed")
                    if not arrow or not arrow.get("round") or arrow.get("width") != "13":
                        failures.append(
                            "images/annotate: the arrow head is not round-capped"
                        )
                    if not sized or str(size_max) != "2000":
                        failures.append(
                            "images/annotate: the text size is not free (max "
                            f"{size_max})"
                        )
                    if not (text_state or {}).get("present") or text_state.get("size") != "300":
                        failures.append(
                            "images/annotate: a 300 px caption was not placed"
                        )
                    if not editing or not editing.get("editor"):
                        failures.append("images/annotate: double-click opened no editor")
                    if editing and editing.get("ghost"):
                        failures.append(
                            "images/annotate: the edited text is drawn twice"
                        )
                    if editing and editing.get("ring"):
                        failures.append(
                            "images/annotate: the selection ring shows while editing"
                        )
                except RuntimeError as error:
                    failures.append(f"images/annotate: interaction check failed ({error})")
            else:
                failures.append("images/annotate: the annotation canvas never appeared")
            close_dialog(m)

        if key == "images":
            # A transient status ("Rendering…", "Encoding the preview…") must not move
            # the controls. Sample the fields' top edge while a preview is requested and
            # after it lands; a status inserted into flow is a jump of one text line.
            def open_card(prefix: str) -> bool:
                return bool(
                    m.js(
                        "const card = [...document.querySelectorAll('.tool-card')]"
                        ".find((c) => ((c.querySelector('.tool-label') || c).innerText || '')"
                        f".trim().startsWith({json.dumps(prefix)}));"
                        "if (card) card.click(); return !!card;"
                    )
                )

            def stable_after(label: str, detail: dict) -> None:
                record = {"category": "images", "tool": label, "dialog": True, **detail}
                report.append(record)
                if not detail.get("fields_top"):
                    failures.append(f"images/{label}: the fields have no measured position")
                    return
                top_span = detail["fields_top"][1] - detail["fields_top"][0]
                if top_span > 2:
                    failures.append(
                        f"images/{label}: the fields moved {top_span}px while rendering"
                    )
                if detail.get("preview_height"):
                    span = detail["preview_height"][1] - detail["preview_height"][0]
                    if span > 2:
                        failures.append(
                            f"images/{label}: the preview band grew {span}px while rendering"
                        )

            m.cmd("WebDriver:Navigate", {"url": f"{base}?mock-files={key}"})
            m.wait_for("document.querySelector('.tool-card')", 10)
            open_category(m, "Images")

            if open_card("Edit photo"):
                if m.wait_for("document.querySelector('.modal')", 6):
                    time.sleep(0.9)
                    if set_field_value(m, "Exposure", "100"):
                        measured = watch_layout(
                            m,
                            lambda index: set_field_value(
                                m, "Exposure", "100" if index % 2 else "-100"
                            ),
                        )
                        stable_after("edit-photo-preview", measured)
                    else:
                        failures.append("images/edit-photo: no Exposure slider")
                    close_dialog(m)
                else:
                    failures.append("images/edit-photo: no dialog")
            else:
                failures.append("images/edit-photo: no card")

            if open_card("Add background"):
                if m.wait_for("document.querySelector('.modal')", 6):
                    time.sleep(0.7)
                    set_field_value(m, "Background", "image")
                    set_field_value(m, "Background image", "/fixtures/photo.jpg")
                    time.sleep(0.4)
                    if set_field_value(m, "Background blur", "40"):
                        measured = watch_layout(
                            m,
                            lambda index: set_field_value(
                                m, "Background blur", "40" if index % 2 else "0"
                            ),
                            rounds=18,
                        )
                        stable_after("add-background-blur", measured)
                    else:
                        failures.append("images/add-background: no Background blur slider")
                    # The image-backed canvas must show a picture, not a broken layer.
                    shown = m.js(
                        """
                        const canvas = document.querySelector('.background-canvas');
                        const layer = canvas
                          ? canvas.querySelector('.background-fill-image, canvas img')
                          : null;
                        return { canvas: !!canvas, layer: !!layer };
                        """
                    )
                    if not (shown or {}).get("canvas"):
                        failures.append("images/add-background: image mode draws no canvas")

                    # The picture must be drawn with its own aspect (the engine
                    # stretches the *crop*, never the source), and no layer may show
                    # the sharp picture through the blur.
                    aspect_clicked = m.js(
                        """
                        const field = [...document.querySelectorAll('.modal-fields .field')]
                          .find((r) => ((r.querySelector('.field-label') || r).innerText || '')
                            .trim() === 'Canvas ratio');
                        const button = field && [...field.querySelectorAll('button')]
                          .find((b) => (b.innerText || '').trim() === '16:9');
                        if (button) button.click();
                        return !!button;
                        """
                    )
                    time.sleep(0.5)
                    layers = m.js(
                        """
                        const canvas = document.querySelector('.background-canvas');
                        const img = document.querySelector('.background-fill-image');
                        const bleed = document.querySelector('.background-bleed');
                        if (!canvas || !img) return null;
                        const c = canvas.getBoundingClientRect();
                        const r = img.getBoundingClientRect();
                        return {
                          sourceRatio: Number((r.width / r.height).toFixed(3)),
                          canvasRatio: Number((c.width / c.height).toFixed(3)),
                          fillFilter: getComputedStyle(img).filter,
                          bleedFilter: bleed ? getComputedStyle(bleed).filter : null,
                        };
                        """
                    )
                    report.append(
                        {"category": "images", "tool": "add-background-layers",
                         "dialog": True, "aspect_clicked": bool(aspect_clicked),
                         **(layers or {})}
                    )
                    if not aspect_clicked:
                        failures.append("images/add-background: no 16:9 ratio control")
                    if layers:
                        # The mock picture is 4032x3024 (4:3), the canvas 16:9.
                        if abs(layers["sourceRatio"] - 4 / 3) > 0.05:
                            failures.append(
                                "images/add-background: the picture is stretched to the"
                                f" canvas ({layers['sourceRatio']})"
                            )
                        if abs(layers["canvasRatio"] - 16 / 9) > 0.06:
                            failures.append(
                                "images/add-background: the canvas is not 16:9"
                                f" ({layers['canvasRatio']})"
                            )
                        if not str(layers["fillFilter"]).startswith("blur("):
                            failures.append(
                                "images/add-background: the blur never reached the picture"
                            )
                        if layers.get("bleedFilter") == "none":
                            failures.append(
                                "images/add-background: a sharp copy shows through the blur"
                            )
                    else:
                        failures.append("images/add-background: no picture layer to measure")
                    close_dialog(m)
                else:
                    failures.append("images/add-background: no dialog")
            else:
                failures.append("images/add-background: no card")

            if open_card("Compress"):
                if m.wait_for("document.querySelector('.modal')", 6):
                    # The facts appear only when the real encode lands; the row is
                    # reserved so the dialog does not grow underneath the pointer.
                    measured = watch_layout(
                        m,
                        lambda index: set_field_value(
                            m, "Quality", "20" if index % 2 else "90"
                        ),
                        rounds=20,
                    )
                    stable_after("compress-preview", measured)
                    loaded = m.js(
                        """
                        const img = document.querySelector('.compress-frame img');
                        return img ? !!(img.complete && img.naturalWidth > 0) : false;
                        """
                    )
                    if not loaded:
                        failures.append("images/compress: the frame never showed a picture")
                    close_dialog(m)
                else:
                    failures.append("images/compress: no dialog")
            else:
                failures.append("images/compress: no card")

            if open_card("Collage"):
                if m.wait_for("document.querySelector('.collage-cell')", 6):
                    cells = m.js(
                        "return document.querySelectorAll('.collage-cell').length;"
                    )
                    point = surface_points(
                        m, ".collage-cell", [(0.5, 0.5)]
                    )
                    pointer_actions(m, [point[0], point[0]])
                    time.sleep(0.4)
                    selected = m.js(
                        """
                        const cell = document.querySelector('.collage-cell');
                        const zoom = document.querySelector('input[aria-label="Zoom"]');
                        return {
                          selected: !!document.querySelector('.collage-cell.is-selected'),
                          zoom_enabled: zoom ? !zoom.disabled : false,
                          readout: (document.querySelector('.collage-readout') || {}).innerText || '',
                        };
                        """
                    )
                    report.append(
                        {"category": "images", "tool": "collage-interaction",
                         "dialog": True, "cells": cells, **selected}
                    )
                    if not cells:
                        failures.append("images/collage: the layout drew no cells")
                    if not selected.get("selected"):
                        failures.append("images/collage: clicking a cell did not select it")
                    if not selected.get("zoom_enabled"):
                        failures.append("images/collage: Zoom stayed disabled after selecting")

                    # Rows/Columns are the layout, not decoration: a column count must
                    # change the cell geometry the preview draws.
                    def cell_fractions():
                        return m.js(
                            """
                            const frame = document.querySelector('.collage-frame');
                            if (!frame) return null;
                            const f = frame.getBoundingClientRect();
                            return [...document.querySelectorAll('.collage-cell')].map((c) => {
                              const b = c.getBoundingClientRect();
                              return [Number((b.width / f.width).toFixed(3)),
                                      Number((b.height / f.height).toFixed(3))];
                            });
                            """
                        )

                    before_cells = cell_fractions()
                    columns_set = set_field_value(m, "Columns", "3")
                    time.sleep(0.6)
                    after_columns = cell_fractions()
                    rows_set = set_field_value(m, "Rows (0 = auto)", "2")
                    time.sleep(0.6)
                    after_rows = cell_fractions()

                    # A pan must show feedback while the button is still down: the live
                    # crop is what the report saw missing until pointerup.
                    drag_at = surface_points(m, ".collage-cell", [(0.5, 0.5)])[0]
                    # Select once so the thumbnail the live layer draws is loaded; a
                    # cold load would otherwise race the sample below.
                    pointer_actions(m, [drag_at, drag_at])
                    time.sleep(0.6)
                    drag_at = surface_points(m, ".collage-cell", [(0.5, 0.5)])[0]
                    actions = [{"type": "pointerMove", "duration": 0,
                                "x": drag_at[0], "y": drag_at[1], "origin": "viewport"},
                               {"type": "pointerDown", "button": 0},
                               {"type": "pointerMove", "duration": 90,
                                "x": drag_at[0] + 35, "y": drag_at[1], "origin": "viewport"}]
                    m.cmd("WebDriver:PerformActions",
                          {"actions": [{"type": "pointer", "id": "mouse",
                                        "parameters": {"pointerType": "mouse"},
                                        "actions": actions}]})
                    # Sample inside the 120 ms debounce: the composed render has not
                    # landed yet, so only the live crop can be on screen.
                    time.sleep(0.04)
                    live = m.js(
                        """
                        const layer = document.querySelector('.collage-live img');
                        return layer ? { present: true, width: layer.style.width } : { present: false };
                        """
                    )
                    m.cmd("WebDriver:PerformActions",
                          {"actions": [{"type": "pointer", "id": "mouse",
                                        "parameters": {"pointerType": "mouse"},
                                        "actions": [{"type": "pointerUp", "button": 0}]}]})
                    m.cmd("WebDriver:ReleaseActions")
                    time.sleep(0.5)
                    report.append(
                        {"category": "images", "tool": "collage-grid",
                         "dialog": True, "before": before_cells,
                         "after_columns": after_columns, "after_rows": after_rows,
                         "live_mid_drag": live}
                    )
                    if not columns_set:
                        failures.append("images/collage: no Columns field")
                    if not (before_cells and after_columns
                            and after_columns[0][0] < before_cells[0][0] - 0.03):
                        failures.append(
                            "images/collage: Columns did not narrow the cells"
                        )
                    if not (after_rows and after_rows[0][1] < after_columns[0][1] - 0.03):
                        failures.append("images/collage: Rows did not shorten the cells")
                    if not (live or {}).get("present"):
                        failures.append(
                            "images/collage: no live crop while the pointer is down"
                        )
                    close_dialog(m)
                else:
                    failures.append("images/collage: no cell overlay appeared")
            else:
                failures.append("images/collage: no card")
        if key == "video":
            # Typing the output size must work before anything is drawn: the reference
            # dialog lets you enter exact dimensions first.
            m.cmd("WebDriver:Navigate", {"url": f"{base}?mock-files={key}"})
            m.wait_for("document.querySelector('.tool-card')", 10)
            open_category(m, "Video")
            m.js(
                "const card = [...document.querySelectorAll('.tool-card')]"
                ".find((c) => ((c.querySelector('.tool-label') || c).innerText || '')"
                ".trim().startsWith('Crop'));"
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
            else:
                failures.append("video/crop: the crop dialog never opened")

            # Trim: all three sliders share the clip's axis. A start/end slider whose
            # min follows the other handle puts the same time at a different pixel.
            m.js(
                "const card = [...document.querySelectorAll('.tool-card')]"
                ".find((c) => ((c.querySelector('.tool-label') || c).innerText || '')"
                ".trim().startsWith('Trim'));"
                "if (card) card.click(); return !!card;"
            )
            if m.wait_for("document.querySelector('input[aria-label=\"Trim start\"]')", 6):
                def set_slider(label, value):
                    return m.js(
                        """
                        const input = document.querySelector(
                          'input[aria-label=%s]');
                        if (!input) return false;
                        const setter = Object.getOwnPropertyDescriptor(
                          HTMLInputElement.prototype, 'value').set;
                        setter.call(input, String(%s));
                        input.dispatchEvent(new Event('input', { bubbles: true }));
                        return true;
                        """
                        % (json.dumps(label), json.dumps(value))
                    )

                set_slider("Trim start", 5.0)
                time.sleep(0.2)
                set_slider("Trim end", 9.16)
                time.sleep(0.2)
                set_slider("Playhead", 9.16)
                time.sleep(0.3)
                axes = m.js(
                    """
                    const one = (label) => {
                        const input = document.querySelector(
                          'input[aria-label="' + label + '"]');
                        if (!input) return null;
                        const min = Number(input.min);
                        const max = Number(input.max);
                        const value = Number(input.value);
                        return {
                          min, max, value,
                          fraction: max > min ? (value - min) / (max - min) : 0,
                        };
                    };
                    return {
                      start: one('Trim start'),
                      end: one('Trim end'),
                      playhead: one('Playhead'),
                    };
                    """
                )
                report.append(
                    {"category": key, "tool": "trim-slider-axis", "dialog": True,
                     **axes}
                )
                if not (
                    axes.get("end")
                    and axes.get("playhead")
                    and abs(axes["end"]["fraction"] - axes["playhead"]["fraction"])
                    <= 0.01
                    and axes["start"]["min"] == axes["end"]["min"] == 0
                    and axes["start"]["max"] == axes["end"]["max"] == axes["playhead"]["max"]
                ):
                    failures.append(
                        "video/trim: the same time sits at a different slider position"
                    )
                close_dialog(m)
            else:
                failures.append("video/trim: no trim sliders")

            # Save frames: the All-frames button must state its count and drive the
            # primary action; going back restores the per-frame chips.
            m.js(
                "const card = [...document.querySelectorAll('.tool-card')]"
                ".find((c) => ((c.querySelector('.tool-label') || c).innerText || '')"
                ".trim().startsWith('Save frames'));"
                "if (card) card.click(); return !!card;"
            )
            if m.wait_for("document.querySelector('.snapshot-all')", 6):
                time.sleep(0.4)
                frames = m.js(
                    """
                    const all = document.querySelector('.snapshot-all');
                    const primary = document.querySelector('[data-testid="primary"]');
                    return {
                      label: all ? all.innerText.trim() : null,
                      disabled: all ? all.disabled : null,
                      primary: primary ? primary.innerText.trim() : null,
                    };
                    """
                )
                m.js(
                    "const b = document.querySelector('.snapshot-all'); if (b) b.click();"
                )
                time.sleep(0.4)
                chosen = m.js(
                    """
                    const primary = document.querySelector('[data-testid="primary"]');
                    const back = [...document.querySelectorAll('.modal button')]
                      .find((b) => (b.innerText || '').trim()
                        === 'Choose moments instead');
                    return {
                      primary: primary ? primary.innerText.trim() : null,
                      back: !!back,
                      chips: !!document.querySelector('.range-chips'),
                    };
                    """
                )
                m.js(
                    "const b = [...document.querySelectorAll('.modal button')]"
                    ".find((x) => (x.innerText || '').trim() === 'Choose moments instead');"
                    "if (b) b.click();"
                )
                time.sleep(0.3)
                restored = m.js(
                    "return !!document.querySelector('.range-chips');"
                )
                report.append(
                    {"category": key, "tool": "video-all-frames", "dialog": True,
                     "button": frames, "after_click": chosen, "chips_restored": restored}
                )
                if not (frames or {}).get("label", "").startswith("All frames"):
                    failures.append("video/frames: no All frames button")
                if "300" not in ((frames or {}).get("label") or ""):
                    failures.append(
                        "video/frames: All frames does not state the frame count"
                    )
                if "300" not in ((chosen or {}).get("primary") or ""):
                    failures.append(
                        "video/frames: All frames did not set the primary action"
                    )
                if (chosen or {}).get("chips") is not False:
                    failures.append(
                        "video/frames: the per-frame chips still show in all-frames mode"
                    )
                if not (chosen or {}).get("back"):
                    failures.append("video/frames: no way back to chosen moments")
                if not restored:
                    failures.append("video/frames: going back did not restore the chips")
                close_dialog(m)
            else:
                failures.append("video/frames: no All frames button")

        if key == "audio":
            m.cmd("WebDriver:Navigate", {"url": f"{base}?mock-files=audio"})
            m.wait_for("document.querySelector('.tool-card')", 10)
            open_category(m, "Audio")

            def open_tool(prefix: str) -> bool:
                return bool(
                    m.js(
                        "const card = [...document.querySelectorAll('.tool-card')]"
                        ".find((c) => ((c.querySelector('.tool-label') || c).innerText || '')"
                        f".trim().startsWith({json.dumps(prefix)}));"
                        "if (card) card.click(); return !!card;"
                    )
                )

            def set_range(label: str, value: float) -> bool:
                return bool(
                    m.js(
                        """
                        const input = document.querySelector(
                          'input[aria-label=%s]');
                        if (!input) return false;
                        const setter = Object.getOwnPropertyDescriptor(
                          HTMLInputElement.prototype, 'value').set;
                        setter.call(input, String(%s));
                        input.dispatchEvent(new Event('input', { bubbles: true }));
                        return true;
                        """
                        % (json.dumps(label), json.dumps(value))
                    )
                )

            # Trim: Play must start at the selection's start and stop at its end.
            if open_tool("Trim") and m.wait_for(
                "document.querySelector('input[aria-label=\"Trim start\"]')", 8
            ):
                time.sleep(0.4)
                set_range("Trim end", 4.0)
                time.sleep(0.2)
                set_range("Trim start", 2.0)
                time.sleep(0.3)
                m.js("document.querySelector('.waveform-play').click()")
                time.sleep(0.35)
                playing = m.js(
                    """
                    const button = document.querySelector('.waveform-play');
                    const time = document.querySelector('.waveform-timecode');
                    return {
                      label: button ? button.getAttribute('aria-label') : null,
                      time: time ? time.innerText.trim() : null,
                      pause: !!(button && button.querySelector('rect')),
                      play: !!(button && button.querySelector('polygon')),
                    };
                    """
                )
                time.sleep(2.7)
                stopped = m.js(
                    """
                    const button = document.querySelector('.waveform-play');
                    const time = document.querySelector('.waveform-timecode');
                    return {
                      label: button ? button.getAttribute('aria-label') : null,
                      time: time ? time.innerText.trim() : null,
                      pause: !!(button && button.querySelector('rect')),
                      play: !!(button && button.querySelector('polygon')),
                    };
                    """
                )
                report.append(
                    {"category": "audio", "tool": "trim-playback", "dialog": True,
                     "playing": playing, "stopped": stopped}
                )
                if not (playing.get("time") or "").startswith("0:02."):
                    failures.append(
                        f"audio/trim: Play started at {playing.get('time')!r}, not 0:02"
                    )
                if playing.get("label") != "Pause" or not playing.get("pause"):
                    failures.append("audio/trim: the transport does not show a pause icon")
                if not (stopped.get("time") or "").startswith("0:04.00"):
                    failures.append(
                        f"audio/trim: playback stopped at {stopped.get('time')!r}, not 0:04"
                    )
                if stopped.get("label") != "Play" or not stopped.get("play"):
                    failures.append("audio/trim: the transport did not return to a play icon")
                close_dialog(m)
            else:
                failures.append("audio/trim: no trim waveform")

            # Bleep: the × removes its range, one version plays at a time.
            if open_tool("Bleep") and m.wait_for(
                "document.querySelector('.waveform-surface')", 8
            ):
                time.sleep(0.4)
                points = surface_points(
                    m, ".waveform-surface", [(0.3, 0.5), (0.45, 0.5)]
                )
                pointer_actions(m, points)
                time.sleep(0.4)
                before = int(
                    m.js(
                        "return document.querySelectorAll('.waveform-range.is-bleep').length;"
                    )
                    or 0
                )
                removed = bool(
                    m.js(
                        "const b = document.querySelector('.waveform-range-remove');"
                        "if (b) { b.click(); return true; } return false;"
                    )
                )
                time.sleep(0.4)
                after = int(
                    m.js(
                        "return document.querySelectorAll('.waveform-range.is-bleep').length;"
                    )
                    or 0
                )
                pointer_actions(m, points)
                time.sleep(0.4)

                def press_row(text: str) -> bool:
                    return bool(
                        m.js(
                            """
                            const button = [...document.querySelectorAll(
                              '.audio-preview-row button')]
                              .find((b) => (b.innerText || '').trim().startsWith(%s));
                            if (button) { button.click(); return true; }
                            return false;
                            """
                            % json.dumps(text)
                        )
                    )

                def row_states() -> dict:
                    return m.js(
                        """
                        return Object.fromEntries([...document.querySelectorAll(
                          '.audio-preview-row button')].map((b) => [
                            (b.innerText || '').trim(),
                            { pause: !!b.querySelector('rect'),
                              play: !!b.querySelector('polygon') },
                          ]));
                        """
                    )

                press_row("Original")
                time.sleep(0.4)
                original = row_states()
                press_row("With Bleeps")
                time.sleep(0.8)
                with_bleeps = row_states()
                press_row("With Bleeps")
                time.sleep(0.3)
                report.append(
                    {"category": "audio", "tool": "bleep-controls", "dialog": True,
                     "ranges": {"before": before, "removed": removed, "after": after},
                     "original": original, "with_bleeps": with_bleeps}
                )
                if before < 1 or not removed or after != 0:
                    failures.append("audio/bleep: the range × did not remove the range")
                if not (original.get("Original") or {}).get("pause"):
                    failures.append("audio/bleep: Original does not show a pause icon")
                if not (with_bleeps.get("With Bleeps") or {}).get("pause"):
                    failures.append("audio/bleep: With Bleeps does not show a pause icon")
                if (with_bleeps.get("Original") or {}).get("pause"):
                    failures.append(
                        "audio/bleep: Original kept playing when With Bleeps started"
                    )
                close_dialog(m)
            else:
                failures.append("audio/bleep: no waveform")

            # Normalize: the waveform is the shared clock. Clicking it seeks the
            # produced preview; Play starts there and the playhead follows it.
            if open_tool("Normalize") and m.wait_for(
                "document.querySelector('.audio-preview-row button')", 8
            ):
                time.sleep(0.6)
                seeked = m.js(
                    """
                    const surface = document.querySelector('.waveform-surface');
                    const playhead = document.querySelector('.waveform-playhead');
                    if (!surface || !playhead) return null;
                    const f = surface.getBoundingClientRect();
                    surface.dispatchEvent(new PointerEvent('pointerdown', {
                      bubbles: true, clientX: f.left + f.width * 0.5,
                      clientY: f.top + f.height / 2,
                    }));
                    const p = playhead.getBoundingClientRect();
                    return Math.round(((p.left - f.left) / f.width) * 1000) / 1000;
                    """
                )
                m.js("document.querySelector('.audio-preview-row button').click()")
                time.sleep(0.9)
                playing = m.js(
                    """
                    const button = document.querySelector('.audio-preview-row button');
                    const playhead = document.querySelector('.waveform-playhead');
                    const f = document.querySelector('.waveform-surface').getBoundingClientRect();
                    const p = playhead.getBoundingClientRect();
                    return { pause: !!button.querySelector('rect'),
                             play: !!button.querySelector('polygon'),
                             fraction: Math.round(((p.left - f.left) / f.width) * 1000) / 1000 };
                    """
                )
                m.js("document.querySelector('.audio-preview-row button').click()")
                time.sleep(0.4)
                paused = m.js(
                    """
                    const button = document.querySelector('.audio-preview-row button');
                    return { pause: !!button.querySelector('rect'),
                             play: !!button.querySelector('polygon') };
                    """
                )
                notch = m.js(
                    "return !!(document.querySelector('.waveform-note') || {}).innerText;"
                )
                report.append(
                    {"category": "audio", "tool": "normalize-playback", "dialog": True,
                     "seeked_fraction": seeked, "playing": playing, "paused": paused,
                     "truncated_note": notch}
                )
                if not seeked or abs(seeked - 0.5) > 0.03:
                    failures.append(
                        f"audio/normalize: clicking the middle did not seek ({seeked})"
                    )
                if not playing.get("pause") or playing.get("play"):
                    failures.append("audio/normalize: Output does not show a pause icon")
                if not (playing.get("fraction") or 0) > (seeked or 0) + 0.01:
                    failures.append(
                        "audio/normalize: the playhead did not follow playback from the seek"
                    )
                if not paused.get("play") or paused.get("pause"):
                    failures.append("audio/normalize: Pause does not return to a play icon")
                if notch:
                    failures.append(
                        "audio/normalize: a 14-second waveform claims to be truncated"
                    )
                close_dialog(m)
            else:
                failures.append("audio/normalize: no output button")

            # Channels: a mono output has one Volume row and one M lane, not "Left".
            if open_tool("Channels") and m.wait_for(
                "document.querySelector('.waveform-surface')", 8
            ):
                time.sleep(0.6)
                stereo = m.js(
                    """
                    const labels = [...document.querySelectorAll(
                      '.modal-fields .field-label')].map((n) => n.innerText.trim());
                    const lanes = [...document.querySelectorAll(
                      '.waveform-lane-label')].map((n) => n.innerText.trim());
                    return { labels, lanes };
                    """
                )
                mono = m.js(
                    """
                    const field = [...document.querySelectorAll('.modal-fields .field')]
                      .find((r) => ((r.querySelector('.field-label') || r).innerText || '')
                        .trim() === 'Output');
                    const button = field && [...field.querySelectorAll('button')]
                      .find((b) => (b.innerText || '').trim() === 'Mono');
                    if (button) button.click();
                    return !!button;
                    """
                )
                time.sleep(0.5)
                mono_state = m.js(
                    """
                    const labels = [...document.querySelectorAll(
                      '.modal-fields .field-label')].map((n) => n.innerText.trim());
                    const lanes = [...document.querySelectorAll(
                      '.waveform-lane-label')].map((n) => n.innerText.trim());
                    return { labels, lanes };
                    """
                )
                report.append(
                    {"category": "audio", "tool": "channels-mono", "dialog": True,
                     "stereo": stereo, "mono": mono_state}
                )
                if "Volume" not in mono_state.get("labels", []):
                    failures.append("audio/channels: a mono output has no Volume row")
                if "Left volume" in mono_state.get("labels", []):
                    failures.append(
                        "audio/channels: a mono output still says Left volume"
                    )
                if mono_state.get("lanes") != ["M"]:
                    failures.append(
                        f"audio/channels: mono lanes are {mono_state.get('lanes')}, not ['M']"
                    )
                if "Left volume" not in stereo.get("labels", []):
                    failures.append("audio/channels: stereo lost its Left volume row")
                close_dialog(m)
            else:
                failures.append("audio/channels: no waveform")

            # The portrait visualizer preview is centred in its row.
            if open_tool("Visualize") and m.wait_for(
                "document.querySelector('.visualizer-frame')", 8
            ):
                time.sleep(0.6)
                m.js(
                    """
                    const field = [...document.querySelectorAll('.modal-fields .field')]
                      .find((r) => ((r.querySelector('.field-label') || r).innerText || '')
                        .trim() === 'Shape');
                    const button = field && [...field.querySelectorAll('button')]
                      .find((b) => (b.innerText || '').trim() === 'Portrait · 9:16');
                    if (button) button.click();
                    """
                )
                time.sleep(0.5)
                gaps = m.js(
                    """
                    const frame = document.querySelector('.visualizer-frame');
                    const row = frame.closest('.field');
                    const f = frame.getBoundingClientRect();
                    const r = row.getBoundingClientRect();
                    return { left: Math.round(f.left - r.left),
                             right: Math.round(r.right - f.right),
                             width: Math.round(f.width) };
                    """
                )
                report.append(
                    {"category": "audio", "tool": "visualizer-centre", "dialog": True,
                     **gaps}
                )
                if abs(gaps["left"] - gaps["right"]) > 2:
                    failures.append(
                        f"audio/visualize: the preview is not centred ({gaps})"
                    )
                close_dialog(m)
            else:
                failures.append("audio/visualize: no preview frame")

        if key == "pdf":
            # The organizer's own controls: rotating a card must leave a rotation on that
            # card, and the value is what the dialog submits.
            m.cmd("WebDriver:Navigate", {"url": f"{base}?mock-files={key}"})
            m.wait_for("document.querySelector('.tool-card')", 10)
            open_category(m, "PDF")
            m.js(
                "const card = [...document.querySelectorAll('.tool-card')]"
                ".find((c) => ((c.querySelector('.tool-label') || c).innerText || '')"
                ".trim() === 'Organize pages');"
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

                # A dragged card must leave a ghost that follows the pointer, open a
                # visible landing slot, and say where the drop will insert. The old
                # native drag image covered the grid and named no position.
                points = m.js(
                    """
                    const cards = [...document.querySelectorAll('.page-card')];
                    const center = (i) => {
                      const r = cards[i].getBoundingClientRect();
                      return [Math.round(r.left + r.width / 2),
                              Math.round(r.top + r.height / 2)];
                    };
                    return { from: center(0), to: [center(3)[0] + 20, center(3)[1]] };
                    """
                )
                pointer_press_move(m, [points["from"], points["to"]])
                time.sleep(0.4)
                dragging = m.js(
                    """
                    const ghost = document.querySelector('.page-ghost');
                    const slot = document.querySelector('.page-slot');
                    const status = document.querySelector('.page-drag-status');
                    const card = document.querySelector('.page-card');
                    const g = ghost ? ghost.getBoundingClientRect() : null;
                    const s = slot ? slot.getBoundingClientRect() : null;
                    const c = card ? card.getBoundingClientRect() : null;
                    return {
                      ghost: !!ghost,
                      ghostTarget: ghost
                        ? (ghost.querySelector('.page-ghost-target') || {}).innerText : '',
                      ghostPage: ghost
                        ? (ghost.querySelector('.page-ghost-page') || {}).innerText : '',
                      ghostWidth: g ? Math.round(g.width) : 0,
                      cardWidth: c ? Math.round(c.width) : 0,
                      slotCovered: !!(g && s
                        && s.left + s.width / 2 > g.left && s.left + s.width / 2 < g.right
                        && s.top + s.height / 2 > g.top && s.top + s.height / 2 < g.bottom),
                      slot: !!slot,
                      slotText: slot ? slot.innerText : '',
                      cards: document.querySelectorAll('.page-card').length,
                      selected: document.querySelectorAll('.page-card.is-selected').length,
                      status: status ? status.innerText : '',
                    };
                    """
                )
                report.append(
                    {"category": key, "tool": "organize-drag", "dialog": True,
                     **dragging}
                )
                m.screenshot(out / "pdf-organize-dragging.png")
                pointer_release(m)
                time.sleep(0.5)
                dropped = m.js(
                    """
                    const cards = [...document.querySelectorAll('.page-card')];
                    const status = document.querySelector('.page-drag-status');
                    return {
                      order: cards.map((card) => (card.querySelector('.page-meta') || {}).innerText || ''),
                      badges: cards.map((card) => (card.querySelector('.page-badge') || {}).innerText || ''),
                      selected: (document.querySelector('.page-card.is-selected .page-meta') || {}).innerText || '',
                      ghost: !!document.querySelector('.page-ghost'),
                      slot: !!document.querySelector('.page-slot'),
                      status: status ? status.innerText : '',
                    };
                    """
                )
                report.append(
                    {"category": key, "tool": "organize-drop", "dialog": True,
                     **dropped}
                )
                expected_order = ["Page 2", "Page 3", "Page 4", "Page 1", "Page 5"]
                if not dragging.get("ghost"):
                    failures.append("pdf/organize: dragging showed no lifted card")
                if not dragging.get("slot"):
                    failures.append("pdf/organize: dragging showed no landing slot")
                if "position" not in (dragging.get("status") or "").lower():
                    failures.append(
                        f"pdf/organize: the drag readout names no position ({dragging.get('status')!r})"
                    )
                if dragging.get("cards") != 4:
                    failures.append(
                        "pdf/organize: the lifted card is still drawn in the grid "
                        f"({dragging.get('cards')} cards left of 5)"
                    )
                if "Page 1" not in (dragging.get("ghostPage") or ""):
                    failures.append(
                        f"pdf/organize: the lifted card does not name its page ({dragging.get('ghostPage')!r})"
                    )
                if dragging.get("ghostWidth", 0) >= dragging.get("cardWidth", 1) * 0.75:
                    failures.append(
                        "pdf/organize: the drag ghost is card-sized "
                        f"({dragging.get('ghostWidth')} px of {dragging.get('cardWidth')} px)"
                    )
                if dragging.get("slotCovered"):
                    failures.append("pdf/organize: the drag ghost covers the landing slot")
                if [line.split(" ·")[0] for line in dropped.get("order", [])] != expected_order:
                    failures.append(
                        f"pdf/organize: the drop inserted in the wrong place {dropped.get('order')!r}"
                    )
                elif dropped.get("badges", [])[3] != "4":
                    failures.append(
                        f"pdf/organize: page 1 landed with badge {dropped.get('badges', [None])[3]!r}"
                    )
                if "Page 1" not in (dropped.get("selected") or ""):
                    failures.append(
                        f"pdf/organize: the dropped card is not selected ({dropped.get('selected')!r})"
                    )
                if dropped.get("ghost") or dropped.get("slot"):
                    failures.append("pdf/organize: the drag overlay survived the drop")

                # The hint promises the arrow keys; the selected card must move one slot
                # and keep selection.
                pressed = m.js(
                    """
                    const card = document.querySelector('.page-card.is-selected');
                    if (!card) return { found: false };
                    card.dispatchEvent(new KeyboardEvent('keydown', {
                      key: 'ArrowRight', bubbles: true, cancelable: true,
                    }));
                    return { found: true };
                    """
                )
                time.sleep(0.4)
                keyed = m.js(
                    """
                    const cards = [...document.querySelectorAll('.page-card')];
                    const selected = document.querySelector('.page-card.is-selected');
                    return {
                      order: cards.map((card) => (card.querySelector('.page-meta') || {}).innerText || ''),
                      badge: selected
                        ? (selected.querySelector('.page-badge') || {}).innerText : '',
                    };
                    """
                )
                report.append(
                    {"category": key, "tool": "organize-keys", "dialog": True,
                     "order": keyed.get("order"), "badge": keyed.get("badge")}
                )
                if not pressed.get("found"):
                    failures.append("pdf/organize: no selected card for the arrow keys")
                elif [line.split(" ·")[0] for line in keyed.get("order", [])] != [
                    "Page 2", "Page 3", "Page 4", "Page 5", "Page 1"
                ]:
                    failures.append(
                        f"pdf/organize: ArrowRight reordered to {keyed.get('order')!r}"
                    )
                elif keyed.get("badge") != "5":
                    failures.append(
                        f"pdf/organize: the moved card's badge is {keyed.get('badge')!r}, not 5"
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
