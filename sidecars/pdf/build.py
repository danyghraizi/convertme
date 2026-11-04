#!/usr/bin/env python3
"""Build and smoke-check the native PDF sidecar on its target host."""

from __future__ import annotations

import argparse
import hashlib
import os
from pathlib import Path
import shutil
import struct
import subprocess
import sys
import tempfile
import venv

HERE = Path(__file__).resolve().parent
SPEC = HERE / "convertme-pdf.spec"
WINDOWS_LOCK = HERE / "requirements-windows.txt"
LINUX_LOCK = HERE / "requirements-linux.txt"


def build_stamp() -> str:
    inputs = [HERE / "build.py", HERE / "build.sh", SPEC, HERE / "__main__runner.py"]
    inputs.extend(sorted((HERE / "convertme_pdf").rglob("*.py")))
    inputs.append(WINDOWS_LOCK if os.name == "nt" else LINUX_LOCK)
    digest = hashlib.sha256()
    for path in sorted(inputs):
        name = path.relative_to(HERE).as_posix().encode("utf-8")
        content = path.read_bytes()
        digest.update(len(name).to_bytes(8, "big"))
        digest.update(name)
        digest.update(len(content).to_bytes(8, "big"))
        digest.update(content)
    return "sha256:" + digest.hexdigest()


def run(*args: str, **kwargs: object) -> None:
    subprocess.run(args, check=True, cwd=HERE, **kwargs)


def windows_python_is_amd64() -> bool:
    with open(sys.executable, "rb") as executable:
        if executable.read(2) != b"MZ":
            return False
        executable.seek(0x3C)
        offset = executable.read(4)
        if len(offset) != 4:
            return False
        executable.seek(struct.unpack("<I", offset)[0])
        header = executable.read(6)
        return len(header) == 6 and header[:4] == b"PE\0\0" and struct.unpack("<H", header[4:])[0] == 0x8664


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("output_dir", nargs="?", type=Path)
    parser.add_argument("--venv-out", type=Path, help="create and retain the exact build environment at this new path")
    args = parser.parse_args()
    windows = os.name == "nt"
    if windows and (sys.version_info[:2] != (3, 12) or sys.implementation.name != "cpython" or not windows_python_is_amd64()):
        raise SystemExit("Windows sidecar builds require x86-64 CPython 3.12")
    if not windows and (sys.version_info[:2] != (3, 12) or sys.implementation.name != "cpython"):
        raise SystemExit("Linux sidecar builds require CPython 3.12")
    if not windows and not LINUX_LOCK.is_file():
        raise SystemExit(f"missing captured Linux sidecar dependency lock: {LINUX_LOCK}")
    retained_venv = args.venv_out.resolve() if args.venv_out else None
    if retained_venv is not None and retained_venv.exists():
        raise SystemExit(f"build venv already exists: {retained_venv}")
    if retained_venv is not None and retained_venv.is_relative_to((HERE / "build" / ("windows" if windows else sys.platform)).resolve()):
        raise SystemExit("build venv must be outside the disposable PyInstaller work directory")

    output = (args.output_dir or HERE / "dist" / ("windows" if windows else "")).resolve()
    output.mkdir(parents=True, exist_ok=True)
    executable = output / ("convertme-pdf.exe" if windows else "convertme-pdf")
    work = HERE / "build" / ("windows" if windows else sys.platform)
    shutil.rmtree(work, ignore_errors=True)
    work.mkdir(parents=True)
    stamp = build_stamp()
    stamp_file = work / "convertme-pdf-build-stamp.txt"
    stamp_file.write_text(stamp + "\n", encoding="ascii")
    env = os.environ.copy()
    env["CONVERTME_PDF_STAMP_FILE"] = str(stamp_file)

    with tempfile.TemporaryDirectory(prefix="convertme-pdf-build-") as temp:
        staged = Path(temp) / "dist" / executable.name
        venv_dir = retained_venv if retained_venv is not None else Path(temp) / "venv"
        if venv_dir.exists():
            raise SystemExit(f"build venv already exists: {venv_dir}")
        venv_dir.parent.mkdir(parents=True, exist_ok=True)
        venv.EnvBuilder(with_pip=True).create(venv_dir)
        if windows:
            python = str(venv_dir / "Scripts" / "python.exe")
            run(python, "-m", "pip", "install", "--require-hashes", "--only-binary=:all:", "-r", str(WINDOWS_LOCK))
        else:
            wheelhouse = os.environ.get("CONVERTME_PDF_WHEELHOUSE")
            if not wheelhouse:
                raise SystemExit("CONVERTME_PDF_WHEELHOUSE must name the locked offline Linux wheelhouse")
            python = str(venv_dir / "bin" / "python")
            run(python, "-m", "pip", "install", "--no-index", "--find-links", wheelhouse, "--require-hashes", "--only-binary=:all:", "-r", str(LINUX_LOCK))

        run(python, "-m", "PyInstaller", "--noconfirm", "--clean", "--distpath", str(staged.parent), "--workpath", str(work / "pyinstaller"), str(SPEC), env=env)
        version = subprocess.check_output([str(staged), "--version"], text=True, cwd=HERE).strip()
        if version != f"convertme-pdf 0.1.1 {stamp}":
            raise RuntimeError(f"unexpected sidecar version: {version!r}")
        fixture = Path(temp) / "smoke.pdf"
        run(python, "-c", "import pymupdf,sys; d=pymupdf.open(); p=d.new_page(); p.insert_text((72,72), 'ConvertMe smoke fixture'); d.save(sys.argv[1]); d.close()", str(fixture))
        for kind in ("docx", "pptx", "xlsx", "epub", "md"):
            result = Path(temp) / f"smoke.{kind}"
            run(str(staged), kind, str(fixture), str(result))
            if not result.is_file() or result.stat().st_size == 0:
                raise RuntimeError(f"sidecar produced no {kind} output")
        with tempfile.TemporaryDirectory(prefix=".convertme-pdf-", dir=output) as publish_dir:
            candidate = Path(publish_dir) / executable.name
            shutil.copy2(staged, candidate)
            os.replace(candidate, executable)
    print(f"sidecar at {executable}\n{version}")


if __name__ == "__main__":
    main()
