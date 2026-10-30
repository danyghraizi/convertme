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


def serve(directory: Path):
    class Quiet(SimpleHTTPRequestHandler):
        def log_message(self, *_args):
            pass

    httpd = ThreadingHTTPServer(("127.0.0.1", free_port()), lambda *a, **k: Quiet(*a, directory=str(directory), **k))
    threading.Thread(target=httpd.serve_forever, daemon=True).start()
    return httpd


def start_firefox(profile: Path):
    env = dict(os.environ, MOZ_HEADLESS="1")
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
    const modal = document.querySelector('.modal');
    if (!modal) return { dialog: false };
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
    const scroller = document.querySelector('.modal-fields');
    return {
      dialog: true,
      labels: fields,
      controls,
      modal: modalRect,
      screen: { w: window.innerWidth, h: window.innerHeight },
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


def click_cancel(m: Marionette):
    m.js(
        """
        const footer = document.querySelector('.modal-footer');
        if (footer) {
          const cancel = [...footer.querySelectorAll('button')].find((b) => b.innerText.trim() === 'Cancel');
          if (cancel) cancel.click();
        }
        return true;
        """
    )
    m.wait_for("!document.querySelector('.modal')")


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
    profile = Path(tempfile.mkdtemp(prefix="convertme-shots-"))
    firefox = start_firefox(profile)
    failures = []
    report = []
    try:
        m = Marionette()
        m.cmd("WebDriver:NewSession", {"capabilities": {}})
        m.cmd("WebDriver:SetWindowRect", {"x": 0, "y": 0, "width": 1280, "height": 940})
        categories = [(k, v) for k, v in CATEGORIES if not args.category or k == args.category]
        for key, label in categories:
            m.cmd("WebDriver:Navigate", {"url": f"{base}?mock-files={key}"})
            if not m.wait_for("document.querySelector('.tool-card')", 10):
                failures.append(f"{key}: tool cards never appeared")
                continue
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
                    click_cancel(m)
                else:
                    failures.append("images-lossless/compress: no dialog")
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
                if info.get("overflow"):
                    failures.append(f"{key}/{tool}: a field overflows the modal horizontally")
                if info.get("preview") and not info.get("previewLoaded"):
                    failures.append(f"{key}/{tool}: the preview image did not load")
                click_cancel(m)
            entries = [r for r in report if r["category"] == key]
            print(f"  captured {sum(1 for r in entries if r.get('dialog'))} dialogs of {len(entries)} tools", flush=True)
        (out / "report.json").write_text(json.dumps(report, indent=2))
    finally:
        firefox.terminate()
        try:
            firefox.wait(timeout=5)
        except subprocess.TimeoutExpired:
            firefox.kill()
        httpd.shutdown()
        shutil.rmtree(profile, ignore_errors=True)

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
