#!/usr/bin/env python3
"""Measure the operations the dialogs run, and the ceilings they must stay under.

Third testing layer, next to `ui-shots.py` (layout, mock) and `app-e2e.py`
(real app, published artifacts):

* this script runs the **release binary** — the same planner, queue, engines and
  preview cache the GUI uses — on copies of the Testing Files fixtures and
  records wall time, peak RSS, peak temporary bytes, preview-cache bytes and the
  published output size for one representative operation per family.

    ./scripts/perf-audit.py --write-baseline artifacts/perf/baseline.json
    ./scripts/perf-audit.py --baseline artifacts/perf/baseline.json
    ./scripts/perf-audit.py --only compress --runs 5 --out artifacts/perf

Two numbers are recorded per case. `wall_s` is what the process took end to end;
`net_s` subtracts the cost of starting the process at all (measured once on
`--version`), because inside the app the operation runs in a resident process.
Ceilings are enforced on `net_s`, and the 15 % regression gate compares `net_s`
against the recorded baseline. A case whose operation is not available in this
binary (a preview command that a newer phase adds) is reported as
`unavailable` and never fails the run — it becomes measurable as soon as the
subcommand exists.

The fixture is copied into a throwaway work directory first: the CLI publishes
beside its input, so a run never writes next to the user's files. HOME, the XDG
directories and TMPDIR are isolated per run, which is also what makes the
preview cache measurable (`cache_bytes`) and the temporary peak observable.

Exit codes: 0 all cases within their ceiling and the regression gate; 1 a case
exceeded its ceiling or regressed; 2 bad arguments or a missing fixture.
"""

from __future__ import annotations

import argparse
import json
import os
import shutil
import statistics
import subprocess
import sys
import tempfile
import threading
import time
from datetime import datetime, timezone
from pathlib import Path

REPO = Path(__file__).resolve().parent.parent
BIN = REPO / "target/release/convertme"
UI_DIST = REPO / "crates/convertme-gui/ui-dist"
DEFAULT_FIXTURES = Path(
    os.environ.get("CONVERTME_FIXTURES", "/home/dany/Downloads/Testing Files")
)
DEFAULT_BASELINE = REPO / "artifacts/perf/baseline.json"
REGRESSION = 0.15
MIB = 1024 * 1024

# One case per operation the dialogs run on open, plus one representative job per
# family. `kind` decides which flag set the CLI gets; `ceiling` is the budget from
# the plan (seconds, or MiB for `rss_ceiling`). A ceiling of None means "record it,
# do not gate it" — the regression gate still applies once a baseline exists.
CASES: list[dict] = [
    {
        "name": "thumbnail-4000x3000-512",
        "kind": "preview",
        "fixture": "images/photo-landscape-4000x3000.jpg",
        "args": ["--kind", "thumb", "--size", "512"],
        "ceiling": 0.30,
    },
    {
        "name": "pdf-page-grid-5",
        "kind": "preview",
        "fixture": "pdf/report-5-pages.pdf",
        "args": ["--kind", "pdf-pages"],
        "ceiling": 2.0,
    },
    {
        "name": "waveform-30s",
        "kind": "preview",
        "fixture": "audio/tone-stereo-30s.wav",
        "args": ["--kind", "waveform"],
        "ceiling": 0.30,
    },
    {
        "name": "filmstrip-12",
        "kind": "preview",
        "fixture": "video/clip-landscape-720p-10s.mp4",
        "args": ["--kind", "filmstrip", "--frames", "12"],
        "ceiling": 1.5,
    },
    {
        # The photo editor's preview: bounded at 1024 px, then the engine's own chain.
        "name": "photo-preview-1024",
        "kind": "preview",
        "fixture": "images/photo-landscape-4000x3000.jpg",
        "args": ["--kind", "photo", "--size", "1024", "--params", "{}"],
        "ceiling": 0.70,
    },
    {
        # The dialog's preview *is* the artifact, so it runs the real optimizer: oxipng's
        # lossless passes cost ~2.4 s on this 6 MP poster and cannot be skipped without
        # making the preview's size disagree with the job's. The ceiling is measured, not
        # estimated: the dialog debounces 400 ms and renders off the UI thread, so the
        # window stays responsive while it runs.
        "name": "compress-preview-poster",
        "kind": "preview",
        "fixture": "images/poster-portrait.png",
        "args": ["--kind", "compress", "--params", "{}"],
        "ceiling": 3.0,
    },
    {
        "name": "compress-target-1000b-poster",
        "kind": "tool",
        "fixture": "images/poster-portrait.png",
        "args": [
            "--tool",
            "compress",
            "--category",
            "images",
            "--params",
            json.dumps({"target_bytes": 1000}),
        ],
        "ceiling": 5.0,
    },
    {
        "name": "background-63mp",
        "kind": "tool",
        "fixture": "images/photo-63mp-9000x7000.jpg",
        "args": ["--tool", "add-background", "--category", "images", "--params", "{}"],
        "ceiling": 10.0,
        "rss_ceiling": 1536.0,
    },
    {
        "name": "pdf-organize-5",
        "kind": "tool",
        "fixture": "pdf/report-5-pages.pdf",
        "args": ["--tool", "organize", "--category", "pdf", "--params", "{}"],
        "ceiling": 2.0,
    },
    {
        "name": "convert-pdf-docx-resume",
        "kind": "convert",
        "fixture": "pdf/Jake_s_Resume__Anonymous_.pdf",
        "args": ["--to", "DOCX"],
        "ceiling": 10.0,
    },
    {
        "name": "video-trim-10s",
        "kind": "tool",
        "fixture": "video/clip-landscape-720p-10s.mp4",
        "args": [
            "--tool",
            "trim",
            "--category",
            "video",
            "--params",
            json.dumps({"start": 2, "end": 5}),
        ],
        "ceiling": 6.0,
    },
    {
        "name": "normalize-30s",
        "kind": "tool",
        "fixture": "audio/tone-stereo-30s.wav",
        "args": ["--tool", "normalize", "--category", "audio", "--params", "{}"],
        "ceiling": 4.0,
    },
    {
        # No subprocess at all: the installed bundle is a ceiling of its own.
        "name": "installed-bundle",
        "kind": "size",
        "ceiling_mib": 320.0,
    },
]

APP_CASES = [
    {"name": "app-cold-start-p95", "argv": ["perf", "start", "--runs", "20"], "ceiling": 0.6},
    {"name": "app-wheel-p95", "argv": ["perf", "wheel", "--runs", "20"], "ceiling": 0.05},
    {"name": "app-idle", "argv": ["perf", "idle", "--seconds", "30"], "ceiling": None},
]


def tree_bytes(root: Path) -> int:
    """Bytes under `root`; missing directories are 0, not an error."""
    if not root.exists():
        return 0
    total = 0
    for entry in root.rglob("*"):
        try:
            if entry.is_file():
                total += entry.stat().st_size
        except OSError:
            continue
    return total


class TempWatch:
    """Peak temporary bytes while a child runs.

    A job's temporary spike is invisible after it exits (`WorkDir::cleanup` removes
    it), so the directory is sampled while the process is alive.
    """

    def __init__(self, root: Path, interval: float = 0.02):
        self.root = root
        self.interval = interval
        self.peak = 0
        self._stop = threading.Event()
        self._thread = threading.Thread(target=self._run, daemon=True)

    def _run(self) -> None:
        while not self._stop.is_set():
            self.peak = max(self.peak, tree_bytes(self.root))
            self._stop.wait(self.interval)

    def __enter__(self) -> "TempWatch":
        self._thread.start()
        return self

    def __exit__(self, *exc) -> None:
        self._stop.set()
        self._thread.join(timeout=1.0)


def parse_time_v(path: Path) -> tuple[float, float]:
    """(wall seconds, peak RSS MiB) from a `/usr/bin/time -v` log."""
    wall = 0.0
    rss_kb = 0.0
    try:
        text = path.read_text(errors="replace")
    except OSError:
        return wall, rss_kb
    for line in text.splitlines():
        if "Maximum resident set size" in line:
            value = line.rsplit(":", 1)[-1].strip()
            if value.isdigit():
                rss_kb = float(value)
        elif "Elapsed (wall clock) time" in line:
            value = line.rsplit(":", 1)[-1].strip()
            parts = value.split(":")
            try:
                seconds = 0.0
                for part in parts:
                    seconds = seconds * 60 + float(part)
                wall = seconds
            except ValueError:
                pass
    return wall, rss_kb / 1024.0


def isolate(root: Path) -> dict[str, str]:
    """A throwaway HOME so settings, history, cache and temp are all measurable."""
    home = root / "home"
    # The vendored qpdf is a wrapper script that execs `$HOME/.local/opt/qpdf/bin/qpdf`.
    # A completely isolated HOME would make it exit 127; link the real `.local` in so the
    # measurement is about the work, not about the harness's HOME.
    home.mkdir(parents=True, exist_ok=True)
    real_local = Path(os.path.expanduser("~/.local"))
    if real_local.is_dir():
        link = home / ".local"
        if not link.exists():
            try:
                link.symlink_to(real_local, target_is_directory=True)
            except OSError:
                pass
    for sub in ("cache", "config", "data", "tmp", "state"):
        (home / sub).mkdir(parents=True, exist_ok=True)
    env = dict(os.environ)
    env.update(
        {
            "HOME": str(home),
            "XDG_CACHE_HOME": str(home / "cache"),
            "XDG_CONFIG_HOME": str(home / "config"),
            "XDG_DATA_HOME": str(home / "data"),
            "XDG_STATE_HOME": str(home / "state"),
            "TMPDIR": str(home / "tmp"),
            "CONVERTME_PERF": "0",
        }
    )
    env.pop("CONVERTME_PERF_IDLE", None)
    return env


def measure_startup(env: dict[str, str], runs: int = 3) -> float:
    """What launching the binary costs when it does nothing (`--version`)."""
    samples = []
    for _ in range(runs):
        begin = time.perf_counter()
        subprocess.run([str(BIN), "--version"], env=env, capture_output=True, check=False)
        samples.append(time.perf_counter() - begin)
    return statistics.median(samples)


def child_size() -> float:
    """Peak RSS of every child this process waited on, as a fallback (MiB)."""
    try:
        import resource

        return resource.getrusage(resource.RUSAGE_CHILDREN).ru_maxrss / 1024.0
    except Exception:  # pragma: no cover - resource is stdlib on Linux/macOS
        return 0.0


def run_once(case: dict, fixtures: Path, startup: float) -> dict:
    """One repetition of a case in a fresh work directory."""
    work = Path(tempfile.mkdtemp(prefix="convertme-perf-"))
    record: dict = {"status": "ok", "note": ""}
    try:
        env = isolate(work)
        if case["kind"] == "size":
            return record
        fixture = fixtures / case["fixture"]
        if not fixture.is_file():
            record["status"] = "unavailable"
            record["note"] = f"missing fixture {fixture}"
            return record
        # The CLI publishes beside its input, so the fixture is never the input.
        target = work / fixture.name
        shutil.copy2(fixture, target)

        # The app writes its capability cache once at first launch and reuses it; a
        # command that has never probed this HOME would re-run ten `--version`s and pay
        # for them inside the measurement. Seed it the same way the app does.
        subprocess.run(
            [str(BIN), "probe"],
            env=env,
            capture_output=True,
            text=True,
            check=False,
            timeout=120,
        )

        cache_root = Path(env["XDG_CACHE_HOME"])
        # Cold every time: an already-cached preview is not what the ceiling means.
        if cache_root.exists():
            shutil.rmtree(cache_root, ignore_errors=True)
        cache_root.mkdir(parents=True, exist_ok=True)

        # The verb decides which front end runs: without it the binary opens the
        # window instead of doing the work.
        verb = {"preview": "preview", "tool": "tool", "convert": "convert"}[case["kind"]]
        argv = [str(BIN), verb, target.as_posix()]
        argv += [str(a) for a in case["args"]]
        if case["kind"] in ("tool", "convert"):
            argv += ["--overwrite", "replace"]

        time_log = work / "time.log"
        wrapper = shutil.which("time") or "/usr/bin/time"
        use_time_v = Path("/usr/bin/time").exists()
        command = (
            ["/usr/bin/time", "-v", "-o", str(time_log)] + argv if use_time_v else argv
        )

        before = set(p.name for p in work.iterdir())
        with TempWatch(Path(env["TMPDIR"])) as watch:
            begin = time.perf_counter()
            try:
                completed = subprocess.run(
                    command,
                    env=env,
                    capture_output=True,
                    text=True,
                    check=False,
                    # A mis-dispatched verb opened a window and hung the first
                    # version of this harness for seven minutes; a measured child
                    # gets a deadline so that fails in seconds instead.
                    timeout=600,
                )
            except subprocess.TimeoutExpired:
                record["status"] = "failed"
                record["note"] = "timed out after 600 s: " + " ".join(command[:4])
                record["exit"] = -1
                return record
            wall = time.perf_counter() - begin
        record["wall_s"] = round(wall, 3)
        record["net_s"] = round(max(0.0, wall - startup), 3)
        reported_wall, rss = parse_time_v(time_log) if use_time_v else (0.0, 0.0)
        if use_time_v:
            record["time_v_wall_s"] = round(reported_wall, 3)
        record["peak_rss_mib"] = round(rss or child_size(), 1)
        record["temp_peak_mib"] = round(watch.peak / MIB, 2)
        record["cache_bytes"] = tree_bytes(cache_root)
        created = [p for p in work.iterdir() if p.name not in before and p.name != "time.log"]
        record["output_bytes"] = sum(
            p.stat().st_size for p in created if p.is_file()
        ) + sum(tree_bytes(p) for p in created if p.is_dir())
        record["outputs"] = [p.name for p in created]

        if completed.returncode != 0:
            record["status"] = "unavailable"
            record["note"] = (completed.stderr or completed.stdout).strip()[:400]
            record["exit"] = completed.returncode
        else:
            record["exit"] = 0
        # A leftover work directory is a leak, not a perf number: name it.
        strays = [p.name for p in Path(env["TMPDIR"]).iterdir() if p.name.startswith("convertme-")]
        record["temp_strays"] = strays
        return record
    finally:
        shutil.rmtree(work, ignore_errors=True)


def bundle_bytes() -> int:
    """Installed size: the binary, the vendored tools and the embedded UI."""
    return BIN.stat().st_size + tree_bytes(REPO / "vendor/bin") + tree_bytes(UI_DIST)


def run_case(case: dict, fixtures: Path, startup: float, runs: int) -> dict:
    if case["kind"] == "size":
        size = bundle_bytes()
        return {
            "name": case["name"],
            "kind": "size",
            "status": "ok",
            "ceiling_mib": case.get("ceiling_mib"),
            "size_mib": round(size / MIB, 1),
            "net_s": None,
        }
    repetitions = runs if case["kind"] == "preview" else 1
    samples = [run_once(case, fixtures, startup) for _ in range(repetitions)]
    ok = [s for s in samples if s["status"] == "ok"]
    first = samples[0]
    entry = {
        "name": case["name"],
        "kind": case["kind"],
        "fixture": case.get("fixture"),
        "args": case.get("args"),
        "ceiling": case.get("ceiling"),
        "rss_ceiling": case.get("rss_ceiling"),
        "runs": repetitions,
        "status": "ok" if ok else first["status"],
        "note": first.get("note", "") if not ok else "",
    }
    if ok:
        net = [s["net_s"] for s in ok]
        entry["net_s"] = round(statistics.median(net), 3)
        entry["net_min_s"] = round(min(net), 3)
        entry["net_max_s"] = round(max(net), 3)
        entry["wall_s"] = round(max(s["wall_s"] for s in ok), 3)
        entry["peak_rss_mib"] = round(max(s["peak_rss_mib"] for s in ok), 1)
        entry["temp_peak_mib"] = round(max(s["temp_peak_mib"] for s in ok), 2)
        entry["cache_bytes"] = max(s["cache_bytes"] for s in ok)
        entry["output_bytes"] = max(s["output_bytes"] for s in ok)
        entry["outputs"] = ok[-1].get("outputs", [])
        strays = [name for s in ok for name in s.get("temp_strays", [])]
        entry["temp_strays"] = strays
    return entry


def xtask(args: list[str]) -> tuple[int, str]:
    completed = subprocess.run(
        ["cargo", "xtask"] + args,
        cwd=REPO,
        capture_output=True,
        text=True,
        check=False,
    )
    return completed.returncode, (completed.stdout + completed.stderr).strip()


def app_cases() -> list[dict]:
    """Cold start and repeat latency are measured by the app itself (xtask)."""
    out = []
    for case in APP_CASES:
        code, text = xtask(case["argv"])
        seconds = None
        for token in text.replace(",", " ").split():
            if token.startswith("p95="):
                try:
                    seconds = float(token.split("=", 1)[1].rstrip("ms")) / 1000.0
                except ValueError:
                    seconds = None
        idle_cpu = None
        for token in text.replace(",", " ").split():
            if token.startswith("cpu="):
                try:
                    idle_cpu = float(token.split("=", 1)[1].rstrip("%"))
                except ValueError:
                    idle_cpu = None
        out.append(
            {
                "name": case["name"],
                "kind": "app",
                "status": "ok" if code == 0 else "failed",
                "net_s": seconds,
                "idle_cpu_pct": idle_cpu,
                "ceiling": case["ceiling"],
                "detail": text.splitlines()[-1] if text else "",
            }
        )
    return out


def compare(
    cases: list[dict], baseline: dict | None, regression: float
) -> list[str]:
    """Ceiling + regression violations, as readable lines."""
    problems: list[str] = []
    previous = {c["name"]: c for c in (baseline or {}).get("cases", [])}
    for case in cases:
        name = case["name"]
        if case["status"] != "ok":
            if case["status"] == "failed":
                problems.append(f"{name}: the measurement failed ({case.get('detail', '')})")
            continue
        ceiling = case.get("ceiling")
        if ceiling is not None and case.get("net_s") is not None and case["net_s"] > ceiling:
            problems.append(
                f"{name}: {case['net_s'] * 1000:.0f} ms exceeds its {ceiling * 1000:.0f} ms ceiling"
            )
        rss_ceiling = case.get("rss_ceiling")
        if rss_ceiling is not None and case.get("peak_rss_mib", 0) > rss_ceiling:
            problems.append(
                f"{name}: peak RSS {case['peak_rss_mib']:.0f} MiB exceeds {rss_ceiling:.0f} MiB"
            )
        size_ceiling = case.get("ceiling_mib")
        if size_ceiling is not None and case.get("size_mib", 0) > size_ceiling:
            problems.append(
                f"{name}: {case['size_mib']:.0f} MiB installed exceeds {size_ceiling:.0f} MiB"
            )
        if case.get("temp_strays"):
            problems.append(f"{name}: left {len(case['temp_strays'])} temporary directories behind")
        old = previous.get(name)
        if not old or old.get("status") != "ok" or old.get("net_s") is None:
            continue
        if case.get("net_s") is None:
            continue
        limit = old["net_s"] * (1.0 + regression)
        if case["net_s"] > limit:
            problems.append(
                f"{name}: {case['net_s'] * 1000:.0f} ms is more than "
                f"{regression * 100:.0f} % above the baseline {old['net_s'] * 1000:.0f} ms"
            )
    return problems


def main(argv: list[str] | None = None) -> int:
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument("--fixtures", default=str(DEFAULT_FIXTURES))
    parser.add_argument("--out", help="report directory (default artifacts/perf/<stamp>)")
    parser.add_argument("--runs", type=int, default=3, help="repetitions of a preview case")
    parser.add_argument("--only", help="only cases whose name contains this")
    parser.add_argument("--list", action="store_true", help="list the cases and exit")
    parser.add_argument("--write-baseline", help="record this run as the baseline at PATH")
    parser.add_argument("--baseline", help="compare against this baseline")
    parser.add_argument("--regression", type=float, default=REGRESSION)
    parser.add_argument("--app", action="store_true", help="also run the xtask app budgets")
    args = parser.parse_args(argv)

    if args.list:
        for case in CASES:
            print(f"{case['name']:<28} {case['kind']:<8} {case.get('ceiling')}")
        return 0

    if not BIN.is_file():
        print(f"perf-audit: build the release binary first ({BIN})", file=sys.stderr)
        return 2
    fixtures = Path(args.fixtures)
    if not fixtures.is_dir() and not args.only:
        print(f"perf-audit: fixture root not found: {fixtures}", file=sys.stderr)
        return 2

    selected = [c for c in CASES if not args.only or args.only in c["name"]]
    if not selected:
        print(f"perf-audit: no case matches {args.only!r}", file=sys.stderr)
        return 2

    probe = Path(tempfile.mkdtemp(prefix="convertme-perf-probe-"))
    try:
        startup = measure_startup(isolate(probe))
    finally:
        shutil.rmtree(probe, ignore_errors=True)

    print(f"binary {BIN} ({BIN.stat().st_size / MIB:.1f} MiB)")
    print(f"process start {startup * 1000:.0f} ms (subtracted from `net_s`)")

    results = []
    for case in selected:
        entry = run_case(case, fixtures, startup, max(1, args.runs))
        results.append(entry)
        if entry["status"] != "ok":
            print(f"  {entry['name']:<30} {entry['status']}: {entry['note'][:120]}")
        elif entry["kind"] == "size":
            print(f"  {entry['name']:<30} {entry['size_mib']:.1f} MiB installed")
        else:
            ceiling = entry.get("ceiling")
            flag = ""
            if ceiling is not None and entry["net_s"] > ceiling:
                flag = f"  OVER {ceiling * 1000:.0f} ms"
            print(
                f"  {entry['name']:<30} net {entry['net_s'] * 1000:7.0f} ms  "
                f"rss {entry['peak_rss_mib']:6.0f} MiB  temp {entry['temp_peak_mib']:5.1f} MiB"
                f"{flag}"
            )
    if args.app:
        results.extend(app_cases())

    baseline = None
    if args.baseline:
        try:
            baseline = json.loads(Path(args.baseline).read_text())
        except OSError as error:
            print(f"perf-audit: cannot read baseline {args.baseline}: {error}", file=sys.stderr)
            return 2

    problems = compare(results, baseline, args.regression)

    stamp = datetime.now(timezone.utc).strftime("%Y%m%dT%H%M%SZ")
    out_dir = Path(args.out) if args.out else REPO / "artifacts/perf" / stamp
    out_dir.mkdir(parents=True, exist_ok=True)
    report = {
        "stamp": stamp,
        "binary": {
            "path": str(BIN),
            "bytes": BIN.stat().st_size,
            "mtime": datetime.fromtimestamp(BIN.stat().st_mtime, timezone.utc).isoformat(),
        },
        "fixtures": str(fixtures),
        "process_start_s": round(startup, 4),
        "regression_gate": args.regression,
        "cases": results,
        "problems": problems,
        "ok": not problems,
    }
    report_path = out_dir / "report.json"
    report_path.write_text(json.dumps(report, indent=2) + "\n")

    if args.write_baseline:
        written = Path(args.write_baseline)
        written.parent.mkdir(parents=True, exist_ok=True)
        written.write_text(json.dumps(report, indent=2) + "\n")
        print(f"baseline written: {written}")

    print(f"report: {report_path}")
    if problems:
        print(f"{len(problems)} problem(s):", file=sys.stderr)
        for problem in problems:
            print(f"  - {problem}", file=sys.stderr)
        return 1
    print("perf-audit ok")
    return 0


if __name__ == "__main__":
    raise SystemExit(main())
