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
    """The two-pane contract, measured in the browser."""
    problems = []
    sheet = info.get("sheet") or {}
    rail = info.get("rail")
    screen = info.get("screen") or {}
    if not rail:
        problems.append(f"{key}/{tool}: no rail beside the sheet")
    else:
        if abs(rail.get("w", 0) - 336) > 2:
            problems.append(f"{key}/{tool}: rail is {rail.get('w')} px, expected 336")
        if abs(sheet.get("w", 0) - 600) > 2:
            problems.append(f"{key}/{tool}: sheet is {sheet.get('w')} px, expected 600")
        if rail.get("x", 0) + rail.get("w", 0) > screen.get("w", 0):
            problems.append(f"{key}/{tool}: the rail leaves the viewport")
    if not info.get("railTitle"):
        problems.append(f"{key}/{tool}: the rail has no title")
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
        if info.get("railTitle") != meta["title"]:
            problems.append(
                f"{key}/{tool}: rail title {info.get('railTitle')!r} != backend {meta['title']!r}"
            )
        expected = meta["action"]
        if expected.count("{n}") == 0 and info.get("primary") != expected:
            problems.append(
                f"{key}/{tool}: primary {info.get('primary')!r} != backend {expected!r}"
            )
        if len(info.get("benefits", [])) != len(meta["benefits"]):
            problems.append(
                f"{key}/{tool}: {len(info.get('benefits', []))} benefits, backend has "
                f"{len(meta['benefits'])}"
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
    // The sheet is the form; the rail is the copy beside it.
    const sheet = document.querySelector('.dialog-sheet') || document.querySelector('.modal');
    const modal = document.querySelector('.modal') || sheet;
    if (!modal) return { dialog: false };
    const rail = document.querySelector('.dialog-rail');
    const railText = (sel) => {
      const el = document.querySelector(sel);
      return el ? el.innerText.trim() : '';
    };
    const railTitle = railText('.rail-title');
    const railEyebrow = railText('.rail-eyebrow');
    const railCopy = railText('.rail-copy');
    const benefits = [...document.querySelectorAll('.rail-benefits li')].map((li) => li.innerText.trim());
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
    const scroller = document.querySelector('.modal-fields');
    return {
      dialog: true,
      labels: fields,
      controls,
      modal: modalRect,
      screen: { w: window.innerWidth, h: window.innerHeight },
      sheet: rect(sheet),
      rail: rail ? rect(rail) : null,
      railTitle,
      railEyebrow,
      railCopy,
      benefits,
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
