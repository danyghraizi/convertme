#!/usr/bin/env python3
"""Generate native package dependencies from the complete staged ELF payload."""
import json
import os
from pathlib import Path, PurePosixPath
import re
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parent.parent
TARGET = 'x86_64-unknown-linux-gnu'
NAMES = ('ffmpeg', 'ffprobe', 'ffplay', 'gs', 'pdftoppm', 'pdftotext', 'pandoc', '7zz', 'tesseract', 'convertme-pdf')


def is_elf(path):
    try:
        with path.open('rb') as source:
            return source.read(4) == b'\x7fELF'
    except OSError as exc:
        raise RuntimeError(f'cannot read staged file {path}: {exc}') from exc


def elf_tree(root):
    if not root.is_dir():
        raise RuntimeError(f'missing staged resource tree: {root}')
    result = []
    for path in sorted(root.rglob('*')):
        if path.is_symlink():
            resolved = path.resolve(strict=True)
            if not resolved.is_relative_to(root.resolve()):
                raise RuntimeError(f'resource symlink escapes payload: {path}')
        elif path.is_file() and is_elf(path):
            result.append(path)
    if not result:
        raise RuntimeError(f'no ELF objects found in {root}')
    return result


def sidecar_objects(executable, destination):
    # The sidecar is PyInstaller one-file: its extension modules and shared
    # libraries are in the CArchive, not beside the executable.
    try:
        from PyInstaller.archive.readers import CArchiveReader
    except ImportError as exc:
        raise RuntimeError('PyInstaller archive reader is required to inspect the PDF sidecar') from exc
    archive = CArchiveReader(str(executable))
    found = []
    for name, entry in archive.toc.items():
        if entry[-1] != 'b':
            continue
        relative = PurePosixPath(name)
        if relative.is_absolute() or not relative.parts or '..' in relative.parts or '\\' in name:
            raise RuntimeError(f'unsafe sidecar archive member: {name}')
        data = archive.extract(name)
        if data[:4] != b'\x7fELF':
            continue
        path = destination.joinpath(*relative.parts)
        path.parent.mkdir(parents=True, exist_ok=True)
        path.write_bytes(data)
        found.append(path)
    if not found:
        raise RuntimeError('PDF sidecar archive contained no inspectable ELF libraries')
    return found


def validate_closure(objects, library_dirs):
    # ldd uses the platform loader and reports transitive dependencies. Isolate
    # each payload's private libraries rather than leaking them into other tools.
    env = os.environ.copy()
    env.pop('LD_PRELOAD', None)
    env['LD_LIBRARY_PATH'] = os.pathsep.join(map(str, sorted(library_dirs)))
    for path in objects:
        if re.match(r'^(?:ld-linux|libc\.so|libm\.so|libpthread\.so|libdl\.so|librt\.so|libresolv\.so|libnss_)', path.name):
            raise RuntimeError(f'glibc or loader must remain external: {path}')
        versions = subprocess.check_output(['readelf', '--version-info', str(path)], text=True)
        for major, minor in re.findall(r'\bGLIBC_(\d+)\.(\d+)\b', versions):
            if (int(major), int(minor)) > (2, 39):
                raise RuntimeError(f'{path} requires unsupported GLIBC_{major}.{minor}')
        dynamic = subprocess.check_output(['readelf', '-d', str(path)], text=True, stderr=subprocess.STDOUT)
        if '(NEEDED)' not in dynamic:
            continue
        result = subprocess.run(['ldd', str(path)], text=True, stdout=subprocess.PIPE, stderr=subprocess.STDOUT, env=env)
        if result.returncode or 'not found' in result.stdout:
            raise RuntimeError(f'unresolved ELF dependency in {path}:\n{result.stdout}')


def main():
    if len(sys.argv) != 3:
        raise RuntimeError('usage: linux-dependencies.py <deb,rpm> <output.json>')
    bundles = set(sys.argv[1].split(','))
    if not bundles or bundles - {'deb', 'rpm'}:
        raise RuntimeError('AppImage packaging is blocked: runtime data and ELF closure are not wired into its AppRun')
    payload = ROOT / 'vendor/linux' / TARGET / 'tools'
    manifest = json.loads((payload / 'manifest.json').read_text())
    executables = {name: payload / rel for name, rel in manifest['executables'].items()}
    for name in NAMES:
        path = executables[name]
        if not path.is_file() or not os.access(path, os.X_OK) or not is_elf(path):
            raise RuntimeError(f'missing staged ELF executable: {path}')
    with tempfile.TemporaryDirectory(prefix='convertme-shlibdeps-') as temporary:
        work = Path(temporary)
        groups = []
        for family in ('ffmpeg', 'ghostscript', 'poppler', 'pandoc', '7zip', 'tesseract', 'qpdf', 'imagemagick'):
            objects = elf_tree(payload / family)
            directories = {path.parent for path in objects}
            validate_closure(objects, directories)
            groups.append((objects, directories))
        sidecar = sidecar_objects(executables['convertme-pdf'], work / 'sidecar')
        sidecar.append(executables['convertme-pdf'])
        directories = {path.parent for path in sidecar}
        validate_closure(sidecar, directories)
        groups.append((sidecar, directories))
        linux = {}
        if 'deb' in bundles:
            debian = work / 'debian'
            debian.mkdir()
            (debian / 'control').write_text('Source: convertme\nSection: utils\nPriority: optional\nMaintainer: ConvertMe <noreply@example.invalid>\nStandards-Version: 4.6.2\n\nPackage: convertme\nArchitecture: amd64\nDescription: ConvertMe\n')
            deps = []
            for group, private_dirs in groups:
                command = ['dpkg-shlibdeps', '-O'] + [f'-l{p}' for p in sorted(private_dirs)] + [f'-e{p}' for p in group]
                output = subprocess.check_output(command, cwd=work, text=True).strip()
                prefix = 'shlibs:Depends='
                if not output.startswith(prefix):
                    raise RuntimeError('dpkg-shlibdeps did not return dependency requirements')
                deps.extend(item.strip() for item in output[len(prefix):].split(',') if item.strip())
            if not deps:
                raise RuntimeError('dpkg-shlibdeps returned no dependency requirements')
            deps.extend(['ghostscript', 'libgs-common', 'fonts-urw-base35', 'poppler-data', 'tesseract-ocr-eng'])
            linux['deb'] = {'depends': list(dict.fromkeys(deps))}
        if 'rpm' in bundles:
            scanner = Path('/usr/lib/rpm/find-requires')
            providers = Path('/usr/lib/rpm/find-provides')
            if not scanner.is_file() or not providers.is_file():
                raise RuntimeError('RPM dependency scanners find-requires and find-provides are required')
            deps = []
            for group, _ in groups:
                input_paths = ''.join(f'{p}\n' for p in group)
                required = subprocess.check_output([str(scanner)], input=input_paths, text=True)
                supplied = subprocess.check_output([str(providers)], input=input_paths, text=True)
                private = {line.strip() for line in supplied.splitlines() if line.strip()}
                deps.extend(line.strip() for line in required.splitlines() if line.strip() and line.strip() not in private)
            if not deps:
                raise RuntimeError('RPM dependency scanner returned no requirements')
            deps.extend(['ghostscript', 'urw-base35-fonts', 'poppler-data', 'tesseract-langpack-eng'])
            linux['rpm'] = {'depends': list(dict.fromkeys(deps))}
        Path(sys.argv[2]).write_text(json.dumps({'bundle': {'linux': linux}}, indent=2) + '\n')


if __name__ == '__main__':
    try:
        main()
    except (OSError, RuntimeError, subprocess.CalledProcessError) as exc:
        print(f'linux-dependencies: {exc}', file=sys.stderr)
        sys.exit(1)
