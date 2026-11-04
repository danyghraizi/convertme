#!/usr/bin/env python3
"""Acquire and validate the pinned x64 Windows helper payload."""
import argparse
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import re
import shutil
import struct
import subprocess
import sys
import tempfile
import urllib.request
import uuid

ROOT = Path(__file__).resolve().parents[1]
LOCK = ROOT / "packaging/windows/tools.lock.json"
TARGETS = {"x86_64-pc-windows-msvc", "aarch64-pc-windows-msvc"}


def fail(message):
    raise RuntimeError(message)


def digest(path):
    h = hashlib.sha256()
    with path.open("rb") as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b""):
            h.update(chunk)
    return h.hexdigest()


def safe_relative(value):
    if not isinstance(value, str) or not value or "\\" in value or ":" in value:
        fail(f"unsafe path: {value!r}")
    p = PurePosixPath(value)
    if p.is_absolute() or any(part in ("", ".", "..") or part.endswith((".", " ")) or re.fullmatch(r"(?i)(con|prn|aux|nul|com[1-9]|lpt[1-9])(\..*)?", part) for part in value.split("/")):
        fail(f"unsafe path: {value!r}")
    return p


def download(item, cache):
    sha = item["sha256"]
    if not re.fullmatch(r"[0-9a-f]{64}", sha):
        fail("invalid pinned SHA-256")
    dest = cache / sha
    if dest.exists():
        if not dest.is_file() or dest.is_symlink() or digest(dest) != sha:
            fail(f"corrupt cached archive: {dest}")
        return dest
    request = urllib.request.Request(item["url"], headers={"User-Agent": "ConvertMe-vendor", "Accept": "application/octet-stream"})
    temporary = cache / (sha + "." + uuid.uuid4().hex + ".tmp")
    try:
        with urllib.request.urlopen(request, timeout=120) as response, temporary.open("xb") as out:
            shutil.copyfileobj(response, out, 1024 * 1024)
        if digest(temporary) != sha:
            fail(f"SHA-256 mismatch for {item['url']}")
        os.replace(temporary, dest)
    finally:
        temporary.unlink(missing_ok=True)
    return dest


def run(args, **kwargs):
    return subprocess.run([str(x) for x in args], check=True, **kwargs)


def extract(extractor, archive, destination, duplicate=None):
    result = run([extractor, "l", "-slt", "-ba", "-sccUTF-8", archive], stdout=subprocess.PIPE, stderr=subprocess.PIPE, encoding="utf-8", errors="strict")
    entries = []
    counts = {}
    for block in result.stdout.replace("\r\n", "\n").split("\n\n"):
        fields = dict(line.split(" = ", 1) for line in block.splitlines() if " = " in line)
        if "Path" not in fields:
            continue
        name = fields["Path"].replace("\\", "/").rstrip("/")
        safe_relative(name)
        if any(k in fields for k in ("Symbolic Link", "Hard Link", "Reparse Point", "Reparse Tag")) or fields.get("Attributes", "").startswith("l"):
            fail(f"archive contains a link or reparse point: {name}")
        entries.append(name)
        counts[name.casefold()] = counts.get(name.casefold(), 0) + 1
    if not entries:
        fail(f"empty or unreadable archive: {archive}")
    special = None
    if duplicate is not None:
        special = duplicate.get("path")
        if special != "lib/gssetgs.bat" or duplicate.get("count") != 2:
            fail("invalid Ghostscript duplicate policy")
        if entries.count(special) != 2 or counts.get(special.casefold()) != 2:
            fail("Ghostscript duplicate member is missing or ambiguous")
    if any(count != (2 if special and name == special.casefold() else 1)
           for name, count in counts.items()):
        fail(f"ambiguous archive members: {archive}")
    destination.mkdir()
    if special:
        variants = duplicate.get("variants")
        if not isinstance(variants, list) or len(variants) != 2:
            fail("Ghostscript source variant records missing")
        expected = {}
        for variant in variants:
            source = variant.get("source")
            size = variant.get("size")
            sha = variant.get("sha256")
            if source not in ("lib/gssetgs.bat", "lib/gssetgs64.bat") or not isinstance(size, int) or size <= 0 or not isinstance(sha, str) or not re.fullmatch(r"[0-9a-f]{64}", sha):
                fail("invalid Ghostscript source variant")
            expected[source] = (size, sha)
        if len(expected) != 2 or len(set(expected.values())) != 2 or duplicate.get("selected_variant") != "gssetgs64.bat":
            fail("ambiguous Ghostscript source variants")
        with tempfile.TemporaryDirectory(prefix=".gs-duplicates-", dir=destination.parent) as temporary:
            isolated = Path(temporary)
            try:
                run([extractor, "x", "-aou", "-bd", f"-o{isolated}", archive, special], stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
                paths = list(isolated.rglob("*"))
                if any(p.is_symlink() or getattr(p.lstat(), "st_file_attributes", 0) & 0x400 for p in paths):
                    fail("Ghostscript duplicate extraction produced a link")
                files = [p for p in paths if p.is_file()]
                if any(not p.is_file() and not p.is_dir() for p in paths) or len(files) != 2:
                    raise ValueError("extractor did not preserve both variants")
                blobs = [p.read_bytes() for p in files]
            except (subprocess.CalledProcessError, ValueError):
                result = run([extractor, "x", "-so", "-bd", archive, special], stdout=subprocess.PIPE, stderr=subprocess.PIPE)
                data = result.stdout
                blobs = []
                matches = []
                for size, _ in expected.values():
                    pair = [data[:size], data[size:]]
                    if sorted((len(b), hashlib.sha256(b).hexdigest()) for b in pair) == sorted(expected.values()):
                        matches.append(pair)
                if len(matches) != 1:
                    fail("cannot uniquely recover both Ghostscript source variants")
                blobs = matches[0]
            actual = [(len(b), hashlib.sha256(b).hexdigest()) for b in blobs]
            if sorted(actual) != sorted(expected.values()):
                fail("Ghostscript duplicate bytes differ from verified source")
            selected = blobs[actual.index(expected["lib/gssetgs64.bat"])]
        run([extractor, "x", "-y", "-bd", f"-x!{special}", f"-o{destination}", archive], stdout=subprocess.DEVNULL)
        for path in destination.rglob("*"):
            if path.is_symlink() or getattr(path.lstat(), "st_file_attributes", 0) & 0x400:
                fail(f"archive extracted a link or reparse point: {path}")
        target = destination.joinpath(*safe_relative(special).parts)
        if target.exists() or target.is_symlink():
            fail("excluded Ghostscript member was unexpectedly extracted")
        target.parent.mkdir(parents=True, exist_ok=True)
        target.write_bytes(selected)
        if (target.stat().st_size, digest(target)) != expected["lib/gssetgs64.bat"]:
            fail("selected Ghostscript variant changed")
    else:
        run([extractor, "x", "-y", "-bd", f"-o{destination}", archive], stdout=subprocess.DEVNULL)
    for path in destination.rglob("*"):
        if path.is_symlink() or getattr(path.lstat(), "st_file_attributes", 0) & 0x400:
            fail(f"archive extracted a link or reparse point: {path}")
    return set(entries)


def tree_hashes(root):
    files = {}
    seen = set()
    for path in sorted(root.rglob("*")):
        if path.is_symlink() or getattr(path.lstat(), "st_file_attributes", 0) & 0x400:
            fail(f"payload contains link or reparse point: {path}")
        rel = path.relative_to(root).as_posix()
        safe_relative(rel)
        if rel.casefold() in seen:
            fail(f"case-insensitive payload collision: {rel}")
        seen.add(rel.casefold())
        if path.is_file() and rel != "manifest.json":
            files[rel] = digest(path)
        elif not path.is_file() and not path.is_dir():
            fail(f"non-regular payload entry: {path}")
    return files


def pe_x64(path):
    with path.open("rb") as f:
        if f.read(2) != b"MZ":
            fail(f"not a PE executable: {path}")
        f.seek(0x3c)
        raw = f.read(4)
        if len(raw) != 4:
            fail(f"truncated PE executable: {path}")
        f.seek(struct.unpack("<I", raw)[0])
        if f.read(6) != b"PE\0\0\x64\x86":
            fail(f"helper is not x64 PE: {path}")


def validate(root, lock):
    if root.is_symlink() or not root.is_dir() or getattr(root.lstat(), "st_file_attributes", 0) & 0x400:
        fail(f"payload root is not a regular directory: {root}")
    files = tree_hashes(root)
    manifest_path = root / "manifest.json"
    if manifest_path.is_symlink() or not manifest_path.is_file():
        fail("payload manifest missing")
    manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    if manifest.get("schema_version") != 1 or manifest.get("helper_arch") != "x86_64" or manifest.get("executables") != lock["executables"]:
        fail("payload manifest does not match the locked executable inventory")
    if manifest.get("resource_dirs") != lock["resource_dirs"]:
        fail("payload manifest does not match the locked resource directories")
    for name, value in lock["resource_dirs"].items():
        for rel in value if isinstance(value, list) else [value]:
            if not root.joinpath(*safe_relative(rel).parts).is_dir():
                fail(f"required resource directory missing: {name}: {rel}")
    for rel in lock["executables"].values():
        path = root.joinpath(*safe_relative(rel).parts)
        if not path.is_file():
            fail(f"required executable missing: {rel}")
        pe_x64(path)
    for rel in [lock["sources"]["eng"]["destination"], "7zip/7z.dll"]:
        if not root.joinpath(*safe_relative(rel).parts).is_file():
            fail(f"required resource missing: {rel}")
    if digest(root / lock["sources"]["eng"]["destination"]) != lock["sources"]["eng"]["sha256"]:
        fail("English traineddata hash mismatch")
    if manifest.get("files") != files:
        fail("payload contents do not match manifest hashes")


def smoke(root, lock, work):
    exe = lambda key: root / lock["executables"][key]
    env = os.environ.copy()
    env["TESSDATA_PREFIX"] = str(root / "tesseract/tessdata")
    codecs = run([exe("ffmpeg"), "-hide_banner", "-encoders"], stdout=subprocess.PIPE, stderr=subprocess.STDOUT, encoding="utf-8", errors="replace").stdout
    for codec in ("libx264", "libx265", "libvpx-vp9", "libaom-av1", "libopus", "libvorbis", "libmp3lame", "libwebp"):
        if not re.search(r"\b" + re.escape(codec) + r"\b", codecs):
            fail(f"FFmpeg missing required encoder: {codec}")
    configuration = run([exe("ffmpeg"), "-buildconf"], stdout=subprocess.PIPE, stderr=subprocess.STDOUT, encoding="utf-8", errors="replace").stdout
    if "--enable-gpl" not in configuration or "--enable-version3" not in configuration or "--enable-nonfree" in configuration:
        fail("FFmpeg configuration must enable GPL/version3 and exclude nonfree")
    run([exe("ffplay"), "-version"], stdout=subprocess.DEVNULL)
    image = work / "ocr.png"
    run([exe("magick"), "-size", "1000x160", "xc:white", "-font", "Arial", "-pointsize", "64", "-fill", "black", "-gravity", "center", "-annotate", "+0+0", "CONVERTME OCR CHECK", image])
    text = run([exe("tesseract"), image, "stdout", "-l", "eng"], stdout=subprocess.PIPE, stderr=subprocess.PIPE, encoding="utf-8", errors="replace", env=env).stdout
    if "CONVERTME OCR CHECK" not in " ".join(text.upper().split()):
        fail(f"Tesseract OCR smoke failed: {text!r}")
    for fmt in ("avif", "heic"):
        encoded = work / ("delegate." + fmt)
        run([exe("magick"), image, encoded])
        run([exe("magick"), encoded, "-resize", "50%", work / (fmt + ".png")])
    run([exe("magick"), "-size", "4096x4096", "xc:white", "-resize", "50%", work / "large.png"])


def publish(staged, destination):
    backup = destination.with_name(destination.name + ".backup-" + uuid.uuid4().hex)
    had_previous = destination.exists()
    if had_previous:
        os.replace(destination, backup)
    try:
        os.replace(staged, destination)
    except BaseException:
        if had_previous:
            os.replace(backup, destination)
        raise
    if had_previous:
        shutil.rmtree(backup)

def prepared_input(name, source):
    root = ROOT / "vendor/windows/inputs" / name
    lock_path = ROOT / source["build_lock"]
    if root.is_symlink() or not root.is_dir() or getattr(root.lstat(), "st_file_attributes", 0) & 0x400:
        fail(f"{name}: prepared input directory is missing or is a link")
    manifest_path = root / "manifest.json"
    producer = ROOT / "scripts" / ("build-windows-ffmpeg.sh" if name == "ffmpeg" else "prepare-windows-poppler.py")
    if not producer.is_file():
        fail(f"{name}: source-closed preparation implementation is unavailable")
    if lock_path.is_symlink() or manifest_path.is_symlink() or not lock_path.is_file() or not manifest_path.is_file():
        fail(f"{name}: missing captured lock or prepared input manifest")
    manifest = json.loads(manifest_path.read_text(encoding="utf-8"))
    if manifest.get("schema_version") != 1 or manifest.get("helper_arch") != "x86_64" or manifest.get("input_lock_sha256") != digest(lock_path):
        fail(f"{name}: prepared input is not bound to its build lock")
    return root, manifest


def prepared_file(root, manifest, name):
    records = manifest.get("outputs")
    if not isinstance(records, dict) or not isinstance(records.get(name), dict):
        fail(f"missing prepared output record: {name}")
    path = root.joinpath(*safe_relative(name).parts)
    for part in (path, *path.parents):
        if part == root.parent:
            break
        if part.is_symlink() or getattr(part.lstat(), "st_file_attributes", 0) & 0x400:
            fail(f"prepared output contains a link: {part}")
    record = records[name]
    if path.is_symlink() or not path.is_file() or path.stat().st_size != record.get("size") or digest(path) != record.get("sha256"):
        fail(f"prepared output hash or size mismatch: {path}")
    return path



def main():
    parser = argparse.ArgumentParser()
    parser.add_argument("--target", required=True, choices=sorted(TARGETS))
    parser.add_argument("--python", default=sys.executable)
    parser.add_argument("--helper-payload", type=Path)
    parser.add_argument("--validate-payload", type=Path)
    args = parser.parse_args()
    lock = json.loads(LOCK.read_text(encoding="utf-8"))
    if args.validate_payload is not None:
        validate(args.validate_payload.absolute(), lock)
        print(f"Validated payload: {args.validate_payload}")
        return
    if os.name != "nt":
        fail("Windows payload acquisition and smoke require a native Windows builder")
    parent = ROOT / "vendor/windows" / args.target
    parent.mkdir(parents=True, exist_ok=True)
    cache = ROOT / "vendor/windows/cache"
    cache.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix=".payload-", dir=parent) as temp:
        work = Path(temp)
        tools = work / "tools"
        if args.helper_payload:
            source = args.helper_payload.absolute()
            validate(source, lock)
            shutil.copytree(source, tools, symlinks=False)
            validate(tools, lock)
        else:
            sources = lock["sources"]
            ffmpeg_root, ffmpeg_manifest = prepared_input("ffmpeg", sources["ffmpeg"])
            run([args.python, ROOT / "scripts/windows-ffmpeg-replay.py",
                 "--lock", ROOT / sources["ffmpeg"]["build_lock"],
                 "--validate-output", ffmpeg_root])
            ffmpeg_archive = prepared_file(ffmpeg_root, ffmpeg_manifest, "ffmpeg.zip")
            prepared_file(ffmpeg_root, ffmpeg_manifest, "sources-and-recipes.tar.xz")
            poppler_root, poppler_manifest = prepared_input("poppler", sources["poppler"])
            run([args.python, ROOT / "scripts/prepare-windows-poppler.py",
                 "--validate-payload", poppler_root])
            poppler_prefix = poppler_root / "prefix"
            if not poppler_prefix.is_dir() or poppler_prefix.is_symlink():
                fail("Poppler captured prefix is missing")
            poppler_files = tree_hashes(poppler_prefix)
            if not poppler_files or poppler_manifest.get("files") != poppler_files:
                fail("Poppler prefix differs from its captured file inventory")
            owners = poppler_manifest.get("owners")
            if not isinstance(owners, dict) or set(owners) != set(poppler_files) or not all(isinstance(v, str) and v for v in owners.values()):
                fail("Poppler prefix has unowned files")
            archives = {name: download(item, cache) for name, item in sources.items()
                        if name not in ("ffmpeg", "poppler")}
            archives["ffmpeg"] = ffmpeg_archive
            bootstrap = work / "7zr.exe"
            shutil.copyfile(archives["7zr"], bootstrap)
            extra = work / "extra"
            entries = extract(bootstrap, archives["extra"], extra)
            if not set(sources["extra"]["required_members"]).issubset(entries):
                fail("bootstrap extractor missing")
            extractor = extra / "x64/7za.exe"
            pe_x64(extractor)
            seven = work / "seven"
            entries = extract(extractor, archives["7zip"], seven)
            if not set(sources["7zip"]["required_members"]).issubset(entries):
                fail("full 7-Zip executable, DLL or license missing")
            extractor = seven / "7z.exe"
            pe_x64(extractor)
            tools.mkdir()
            shutil.copytree(seven, tools / "7zip")
            for name in ("ffmpeg", "imagemagick", "ghostscript", "qpdf", "pandoc", "tesseract"):
                unpacked = work / ("unpack-" + name)
                entries = extract(extractor, archives[name], unpacked,
                                  sources[name]["duplicate_members"] if name == "ghostscript" else None)
                archive_root = sources[name]["archive_root"]
                selected = unpacked.joinpath(*safe_relative(archive_root).parts) if archive_root else unpacked
                if not selected.is_dir():
                    fail(f"locked archive root missing: {name}/{archive_root}")
                for rel in (v for v in lock["executables"].values() if v.startswith(name + "/")):
                    member = rel.split("/", 1)[1]
                    archive_member = f"{archive_root}/{member}" if archive_root else member
                    if archive_member not in entries or not (selected / member).is_file():
                        fail(f"locked archive member missing: {name}/{archive_member}")
                shutil.copytree(selected, tools / name)
            shutil.copytree(poppler_prefix, tools / "poppler")
            model = tools / sources["eng"]["destination"]
            model.parent.mkdir(parents=True, exist_ok=True)
            shutil.copyfile(archives["eng"], model)
            venv = ROOT / "vendor/windows/inputs/python-venv"
            if venv.exists() or venv.is_symlink():
                fail(f"retained sidecar build environment already exists: {venv}")
            run([args.python, ROOT / "sidecars/pdf/build.py", "--venv-out", venv])
            print(f"Retained sidecar build environment: {venv}")
            sidecar = ROOT / "sidecars/pdf/dist/windows/convertme-pdf.exe"
            if not sidecar.is_file():
                fail(f"built PDF sidecar missing: {sidecar}")
            (tools / "pdf").mkdir()
            shutil.copyfile(sidecar, tools / "pdf/convertme-pdf.exe")
            manifest = {"schema_version": 1, "helper_arch": "x86_64", "executables": lock["executables"], "resource_dirs": lock["resource_dirs"], "files": tree_hashes(tools)}
            (tools / "manifest.json").write_text(json.dumps(manifest, indent=2) + "\n", encoding="utf-8")
            validate(tools, lock)
        smoke(tools, lock, work)
        publish(tools, parent / "tools")
    print(f"Validated payload: {parent / 'tools'}")


if __name__ == "__main__":
    try:
        main()
    except (OSError, ValueError, RuntimeError, subprocess.CalledProcessError) as error:
        print(f"Windows vendoring failed: {error}", file=sys.stderr)
        sys.exit(1)
