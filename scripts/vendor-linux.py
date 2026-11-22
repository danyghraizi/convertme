#!/usr/bin/env python3
"""Stage the Linux release payload on its supported native build base."""
import argparse
import urllib.request
import json
import hashlib
import os
from pathlib import Path, PurePosixPath
import platform
import shutil
import posixpath
import re
import stat
import subprocess
import sys
import tempfile
import zipfile

ROOT = Path(__file__).resolve().parent.parent
TARGET = 'x86_64-unknown-linux-gnu'
LOCK = json.loads((ROOT / 'packaging/linux/tools.lock.json').read_text())
TOOLS = tuple(key for key in LOCK['executables'] if key not in ('convertme-pdf', 'qpdf', 'magick'))


def run(*args, **kwargs):
    return subprocess.run(args, check=True, **kwargs)


def fail(message):
    raise RuntimeError(message)


def check_host():
    os_release = dict(line.rstrip().split('=', 1) for line in Path('/etc/os-release').read_text().splitlines() if '=' in line and not line.startswith('#'))
    if os_release.get('ID', '').strip('"') != 'ubuntu' or os_release.get('VERSION_ID', '').strip('"') != '24.04' or platform.machine() != 'x86_64':
        fail('Linux release packaging requires native Ubuntu 24.04 x86_64')
    if subprocess.check_output(['rustc', '--print', 'host-tuple'], text=True).strip() != TARGET:
        fail('Linux release packaging requires the native x86_64-unknown-linux-gnu Rust toolchain')


def download(entry, directory):
    dest = directory / entry['sha256']
    if not dest.exists():
        fail(f'missing locked offline input: {dest} ({entry["url"]})')
    with dest.open('rb') as source:
        if hashlib.file_digest(source, 'sha256').hexdigest() != entry['sha256']:
            fail('cached download hash mismatch: ' + str(dest))
    return dest

def capture_locked_tools(directory):
    directory.mkdir(parents=True, exist_ok=True)
    for entry in (LOCK['qpdf'], LOCK['imagemagick']):
        destination = directory / entry['sha256']
        if destination.is_file():
            with destination.open('rb') as source:
                if hashlib.file_digest(source, 'sha256').hexdigest() != entry['sha256']:
                    fail(f'corrupt cached download: {destination}')
            continue
        temporary = directory / (entry['sha256'] + '.tmp')
        request = urllib.request.Request(entry['url'], headers={'User-Agent': 'ConvertMe-vendor/1'})
        try:
            with urllib.request.urlopen(request, timeout=120) as response, temporary.open('xb') as output:
                shutil.copyfileobj(response, output, 1024 * 1024)
            with temporary.open('rb') as source:
                actual = hashlib.file_digest(source, 'sha256').hexdigest()
            if actual != entry['sha256']:
                fail(f'locked Linux tool hash mismatch: {entry["url"]}')
            os.replace(temporary, destination)
        finally:
            temporary.unlink(missing_ok=True)

def check_package_lock():
    lock_path = ROOT / 'packaging/linux/build.lock.json'
    if not lock_path.is_file():
        fail(f'missing captured Ubuntu build lock: {lock_path}')
    packages = json.loads(lock_path.read_text())['ubuntu']['packages']
    if not isinstance(packages, dict) or not packages:
        fail('Ubuntu package lock is empty')
    for name, expected in packages.items():
        fields = subprocess.check_output(
            ['dpkg-query', '-W', '-f=${Status}\\t${Version}\\t${Architecture}\\t${source:Package}\\t${source:Version}', name],
            text=True,
        ).split('\t')
        actual = dict(zip(('status', 'version', 'architecture', 'source', 'source_version'), fields))
        if actual != {'status': 'install ok installed', **expected}:
            fail(f'installed Ubuntu package differs from lock: {name}: {actual!r} != {expected!r}')
    return packages


def package_owner(path, packages):
    result = subprocess.run(['dpkg-query', '-S', str(path)], text=True, stdout=subprocess.PIPE, stderr=subprocess.DEVNULL)
    owners = set()
    if result.returncode == 0:
        for line in result.stdout.splitlines():
            if ': ' not in line:
                continue
            for name in line.split(': ', 1)[0].split(', '):
                if name in packages:
                    owners.add(name)
                elif name.rsplit(':', 1)[0] in packages:
                    owners.add(name.rsplit(':', 1)[0])
    if len(owners) != 1:
        fail(f'cannot attribute {path} to exactly one locked Ubuntu package: {sorted(owners)}')
    return next(iter(owners))



def extract_qpdf(archive, destination):
    destination.mkdir()
    with zipfile.ZipFile(archive) as z:
        links = []
        for info in z.infolist():
            path = PurePosixPath(info.filename)
            mode = info.external_attr >> 16
            if path.is_absolute() or '..' in path.parts:
                fail('unsafe qpdf archive member: ' + info.filename)
            if stat.S_ISLNK(mode):
                target = z.read(info).decode('utf-8')
                resolved = posixpath.normpath(posixpath.join(str(path.parent), target))
                if target.startswith('/') or resolved == '..' or resolved.startswith('../'):
                    fail('unsafe qpdf archive symlink: ' + info.filename)
                links.append((info, target))
            else:
                z.extract(info, destination)
                if mode:
                    (destination / info.filename).chmod(mode & 0o777)
        for info, target in links:
            os.symlink(target, destination / info.filename)
    candidates = [p.parent.parent for p in destination.rglob('bin/qpdf') if p.is_file()]
    if len(candidates) != 1 or not (candidates[0] / 'lib').is_dir():
        fail('qpdf archive must contain exactly one bin/qpdf with a sibling lib directory')
    return candidates[0]

def stage_runtime_data(destination, packages, owners):
    """Copy only files owned by the required Ubuntu data packages."""
    selections = (
        ('ghostscript', ('usr/share/ghostscript/',)),
        ('libgs-common', ('usr/share/ghostscript/',)),
        ('fonts-urw-base35', ('usr/share/fonts/',)),
        ('tesseract-ocr-eng', ('usr/share/tesseract-ocr/',)),
        ('poppler-data', ('usr/share/poppler/',)),
    )
    copied = {}
    for package, prefixes in selections:
        if package not in packages:
            fail(f'runtime data owner missing from Ubuntu lock: {package}')
        paths = subprocess.check_output(['dpkg-query', '-L', package], text=True).splitlines()
        copied[package] = []
        for entry in paths:
            relative = entry.lstrip('/')
            if not any(relative.startswith(prefix) for prefix in prefixes):
                continue
            source = Path(entry)
            if source.is_symlink():
                # Do not turn an external link into an accidental host dependency.
                resolved = source.resolve(strict=True)
                if not str(resolved).startswith('/usr/share/'):
                    fail(f'unsafe runtime data symlink: {source}')
                if not resolved.is_file():
                    continue
            elif not source.is_file():
                continue
            target = destination / relative
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(source, target)
            copied[package].append(relative)
            relative_target = 'runtime-data/' + relative
            previous = owners.get(relative_target)
            if previous is not None and previous != package:
                fail(f'conflicting runtime data owners for {relative_target}')
            owners[relative_target] = package
    for package in ('libgs-common', 'fonts-urw-base35', 'poppler-data'):
        if not copied[package]:
            fail(f'no usable runtime data found in Ubuntu package {package}')
    if not any(path.endswith('/eng.traineddata') for path in copied['tesseract-ocr-eng']):
        fail('Ubuntu tesseract-ocr-eng package lacks eng.traineddata')
    if not any('/Resource/' in path for path in copied['libgs-common'] + copied['ghostscript']):
        fail('Ubuntu Ghostscript packages lack a staged Resource tree')


def stage_libraries(staged, sources, packages, owners):
    """Copy each distro executable family's non-glibc loader closure."""
    core = re.compile(r'^(?:ld-linux|libc\.so|libm\.so|libpthread\.so|libdl\.so|librt\.so|libresolv\.so|libnss_)')
    families = {}
    env = os.environ.copy()
    env.pop('LD_PRELOAD', None)
    env.pop('LD_LIBRARY_PATH', None)
    for name, (source, _) in sources.items():
        family = LOCK['executables'][name].split('/')[0]
        dest = staged / family / 'lib'
        dest.mkdir(parents=True, exist_ok=True)
        families.setdefault(family, {})
        result = subprocess.check_output(['ldd', str(source)], env=env, text=True)
        if 'not found' in result:
            fail(f'unresolved dependencies for {source}:\n{result}')
        for line in result.splitlines():
            match = re.match(r'\s*(\S+)\s+=>\s+(/\S+)', line)
            if not match or core.match(match.group(1)):
                continue
            soname, path = match.groups()
            library = Path(path).resolve(strict=True)
            with library.open('rb') as f:
                digest = hashlib.file_digest(f, 'sha256').hexdigest()
            old = families[family].get(soname)
            if old is not None and old != digest:
                fail(f'conflicting {soname} in {family}')
            families[family][soname] = digest
            target = dest / soname
            if not target.exists():
                shutil.copy2(library, target)
            owner = package_owner(Path(path), packages)
            key = target.relative_to(staged).as_posix()
            previous_owner = owners.get(key)
            if previous_owner is not None and previous_owner != owner:
                fail(f'conflicting library owners for {key}')
            owners[key] = owner
    return families


def audit_glibc(root):
    core = re.compile(r'^(?:ld-linux|libc\.so|libm\.so|libpthread\.so|libdl\.so|librt\.so|libresolv\.so|libnss_)')
    for path in root.rglob('*'):
        if not path.is_file() or path.is_symlink():
            continue
        with path.open('rb') as stream:
            if stream.read(4) != b'\x7fELF':
                continue
        if core.match(path.name):
            fail(f'glibc or loader must not be bundled: {path}')
        versions = subprocess.check_output(['readelf', '--version-info', str(path)], text=True)
        for major, minor in re.findall(r'\bGLIBC_(\d+)\.(\d+)\b', versions):
            if (int(major), int(minor)) > (2, 39):
                fail(f'{path} requires unsupported GLIBC_{major}.{minor}')


def write_manifest(staged, libraries):
    executable_dirs = {}
    for name, rel in LOCK['executables'].items():
        family = rel.split('/')[0]
        executable_dirs[name] = [family + '/lib'] if family in libraries else []
    files = {}
    for path in sorted(staged.rglob('*')):
        if path.is_symlink():
            if not path.resolve(strict=True).is_relative_to(staged.resolve()):
                fail(f'payload link escapes tools root: {path}')
        if path.is_file():
            with path.open('rb') as source:
                files[path.relative_to(staged).as_posix()] = hashlib.file_digest(source, 'sha256').hexdigest()
    manifest = {'schema_version': 1, 'helper_arch': 'x86_64',
                'executables': LOCK['executables'], 'resource_dirs': LOCK['resource_dirs'],
                'library_dirs': executable_dirs, 'files': files}
    (staged / 'manifest.json').write_text(json.dumps(manifest, indent=2) + '\n')




def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--check-host', action='store_true')
    parser.add_argument('--capture-tools', action='store_true')
    parser.add_argument('--sidecar', type=Path)
    args = parser.parse_args()
    check_host()
    if args.check_host:
        return
    cache = Path(os.environ.get('CONVERTME_LINUX_TOOLS_CACHE', ROOT / 'vendor/cache/linux'))
    if args.capture_tools:
        capture_locked_tools(cache)
        return
    if args.sidecar is None:
        fail('usage: vendor-linux.py --capture-tools | --sidecar <built PDF sidecar>')
    packages = check_package_lock()
    for package in ('ghostscript', 'libgs-common', 'fonts-urw-base35', 'poppler-data', 'tesseract-ocr-eng'):
        status = subprocess.check_output(
            ['dpkg-query', '-W', '-f=${Status}', package], text=True
        ).strip()
        if status != 'install ok installed':
            fail(f'required Ubuntu runtime data package is not installed: {package}')
    if not cache.is_dir():
        fail(f'missing locked Linux tool cache: {cache}')
    destination = ROOT / 'vendor/linux' / TARGET / 'tools'
    destination.parent.mkdir(parents=True, exist_ok=True)
    # Never copy an arbitrary PATH executable, a shell wrapper, or a developer-home tool.
    sources = {}
    for name in TOOLS:
        source = Path('/usr/bin') / name
        if not source.is_file() or not os.access(source, os.X_OK):
            fail(f'missing Ubuntu package executable {source}')
        resolved = source.resolve(strict=True)
        with resolved.open('rb') as source_file:
            is_elf = source_file.read(4) == b'\x7fELF'
        if not str(resolved).startswith('/usr/') or not is_elf:
            fail(f'{source} is not a system ELF executable')
        owner = package_owner(source, packages)
        sources[name] = (resolved, owner)
    sidecar = args.sidecar
    if not sidecar.is_file() or not os.access(sidecar, os.X_OK):
        fail(f'build the Linux PDF sidecar on Ubuntu 24.04 and install it as {sidecar}')
    with sidecar.open('rb') as source_file:
        if source_file.read(4) != b'\x7fELF':
            fail(f'PDF sidecar is not an ELF executable: {sidecar}')
    with tempfile.TemporaryDirectory(prefix='.linux-stage-', dir=destination.parent) as work:
        work = Path(work)
        staged = work / 'payload'
        staged.mkdir()
        owners = {}
        for name, (source, owner) in sources.items():
            target = staged / LOCK['executables'][name]
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(source, target)
            owners[LOCK['executables'][name]] = owner
        pdf = staged / LOCK['executables']['convertme-pdf']
        pdf.parent.mkdir(parents=True, exist_ok=True)
        shutil.copy2(sidecar, pdf)
        stage_runtime_data(staged / 'runtime-data', packages, owners)
        qroot = extract_qpdf(download(LOCK['qpdf'], cache), work / 'qpdf-extract')
        shutil.copytree(qroot, staged / 'qpdf', symlinks=True)
        image = download(LOCK['imagemagick'], cache)
        executable = work / 'imagemagick.AppImage'
        shutil.copyfile(image, executable)
        executable.chmod(0o755)
        run(str(executable), '--appimage-extract', cwd=work, stdout=subprocess.DEVNULL)
        appdir = work / 'squashfs-root'
        if not (appdir / 'AppRun').is_file() or not os.access(appdir / 'AppRun', os.X_OK):
            fail('ImageMagick AppImage did not extract a usable AppRun')
        shutil.move(appdir, staged / 'imagemagick')
        version = subprocess.check_output([str(staged / 'imagemagick/AppRun'), '-version'], text=True)
        if 'ImageMagick 7.1.2-35' not in version:
            fail('extracted ImageMagick has an unexpected version')
        magick = str(staged / 'imagemagick/AppRun')
        image_probe = work / 'magick-probe.png'
        run(magick, '-size', '2x2', 'xc:red', str(image_probe))
        run(magick, str(image_probe), 'null:', stdout=subprocess.DEVNULL)
        dynamic = subprocess.check_output(['readelf', '-d', str(staged / 'qpdf/bin/qpdf')], text=True)
        if 'RUNPATH' not in dynamic or '$ORIGIN' not in dynamic:
            fail('qpdf binary lacks its expected relative RUNPATH')
        run(str(staged / 'qpdf/bin/qpdf'), '--version', stdout=subprocess.DEVNULL)
        libraries = stage_libraries(staged, sources, packages, owners)
        audit_glibc(staged)
        provenance = {path: {'package': owner, **packages[owner]} for path, owner in sorted(owners.items())}
        (staged / 'ubuntu-owners.json').write_text(json.dumps(provenance, indent=2) + '\n')
        write_manifest(staged, libraries)
        # Keep the previous complete payload until the new one is ready.
        previous = work / 'previous'
        if destination.exists():
            destination.rename(previous)
        try:
            staged.rename(destination)
        except BaseException:
            if previous.exists():
                previous.rename(destination)
            raise
    print(f'Staged Linux payload under {destination}')


if __name__ == '__main__':
    try:
        main()
    except (OSError, RuntimeError, subprocess.CalledProcessError, zipfile.BadZipFile) as exc:
        print(f'vendor-linux: {exc}', file=sys.stderr)
        sys.exit(1)
