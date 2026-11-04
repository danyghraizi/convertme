#!/usr/bin/env python3
"""Create and verify package inventories and package-bound release candidates."""
import argparse
import hashlib
import importlib.util
import json
import os
import mmap
from pathlib import Path, PurePosixPath
import re
import stat
import struct
import subprocess
import shutil
import sys
import tarfile
import tempfile
import zipfile

ROOT = Path(__file__).resolve().parent.parent
VERSION = '0.1.1'
TARGETS = {
    'x86_64-unknown-linux-gnu': ('linux', 'x64'),
    'x86_64-pc-windows-msvc': ('windows', 'x64'),
    'aarch64-pc-windows-msvc': ('windows', 'arm64'),
}
REQUIRED_HELPERS = {'ffmpeg', 'ffprobe', 'ffplay', 'magick', 'gs', 'qpdf', 'pdftoppm', 'pdftotext', 'pdfinfo', 'pandoc', '7zz', 'tesseract', 'convertme-pdf'}
UI_INPUTS = {'images/poster-portrait.png', 'images/photo-landscape-4000x3000.jpg', 'images/screenshot-text.png', 'images/tiny-8x8.png', 'video/clip-landscape-720p-10s.mp4', 'audio/speech.mp3', 'audio/speech.flac', 'pdf/report-5-pages.pdf'}
LOCKS = ('Cargo.lock', 'ui/package-lock.json', 'packaging/sources.lock.json', 'packaging/linux/tools.lock.json', 'packaging/linux/build.lock.json', 'packaging/windows/tools.lock.json', 'packaging/windows/ffmpeg.lock.json', 'packaging/windows/poppler-explicit.lock', 'packaging/windows/webview2.lock.json', 'sidecars/pdf/requirements-linux.txt', 'sidecars/pdf/requirements-windows.txt')


def fail(message):
    raise ValueError(str(message))


def sha(path):
    h = hashlib.sha256()
    with Path(path).open('rb') as stream:
        for block in iter(lambda: stream.read(1024 * 1024), b''):
            h.update(block)
    return h.hexdigest()


def hash_bytes(data):
    return hashlib.sha256(data).hexdigest()


def digest(value, context):
    if not isinstance(value, str) or not re.fullmatch(r'[0-9a-f]{64}', value):
        fail(f'{context}: invalid SHA256')
    return value


def safe(value):
    if not isinstance(value, str) or not value or '\\' in value or ':' in value or '\x00' in value:
        fail(f'unsafe path: {value!r}')
    p = PurePosixPath(value)
    if p.is_absolute() or any(part in ('', '.', '..') for part in value.split('/')):
        fail(f'unsafe path: {value!r}')
    return p


def file_at(root, name):
    p = Path(root)
    for part in safe(name).parts:
        p = p / part
        if p.is_symlink() or (p.exists() and getattr(p.lstat(), 'st_file_attributes', 0) & 0x400):
            fail(f'link or reparse point: {p}')
    if not p.is_file():
        fail(f'missing file: {p}')
    return p


def load(path):
    def pairs(items):
        out = {}
        for key, value in items:
            if key in out:
                fail(f'{path}: duplicate JSON key {key}')
            out[key] = value
        return out
    with Path(path).open(encoding='utf-8-sig') as stream:
        return json.load(stream, object_pairs_hook=pairs)


def write_atomic(path, data):
    path = Path(path)
    path.parent.mkdir(parents=True, exist_ok=True)
    fd, tmp = tempfile.mkstemp(prefix='.' + path.name + '.', dir=path.parent)
    try:
        with os.fdopen(fd, 'wb') as stream:
            stream.write(data)
        os.replace(tmp, path)
    finally:
        if os.path.exists(tmp):
            os.unlink(tmp)


def json_bytes(value):
    return (json.dumps(value, indent=2, ensure_ascii=False) + '\n').encode('utf-8')


def tree(root):
    root = Path(root)
    if root.is_symlink() or not root.is_dir():
        fail(f'missing directory or linked root: {root}')
    result, folded = {}, set()
    for p in sorted(root.rglob('*')):
        rel = p.relative_to(root).as_posix()
        safe(rel)
        if p.is_symlink() or getattr(p.lstat(), 'st_file_attributes', 0) & 0x400:
            fail(f'link or reparse point: {p}')
        if rel.casefold() in folded:
            fail(f'case-colliding path: {rel}')
        folded.add(rel.casefold())
        if p.is_file():
            result[rel] = sha(p)
        elif not p.is_dir():
            fail(f'non-regular file: {p}')
    if not result:
        fail(f'empty directory: {root}')
    return result


def machine(path):
    with Path(path).open('rb') as f:
        data = f.read(64)
        if data[:4] == b'\x7fELF':
            if len(data) < 20 or data[4:6] != b'\x02\x01':
                fail(f'not a 64-bit little-endian ELF: {path}')
            return {62: 'x86_64', 183: 'aarch64'}.get(struct.unpack_from('<H', data, 18)[0], 'unknown')
        if data[:2] == b'MZ' and len(data) == 64:
            f.seek(struct.unpack_from('<I', data, 60)[0])
            pe = f.read(6)
            if pe[:4] == b'PE\0\0' and len(pe) == 6:
                return {0x8664: 'AMD64', 0xaa64: 'ARM64'}.get(struct.unpack_from('<H', pe, 4)[0], 'unknown')
    fail(f'not a valid PE/ELF executable: {path}')


def source_identity():
    identity_file = os.environ.get('CONVERTME_SOURCE_IDENTITY_FILE')
    if identity_file:
        value = load(identity_file)
        commit = value.get('source_commit')
        source_hash = value.get('source_tree_sha256')
        dirty = value.get('dirty')
        if not isinstance(commit, str) or not re.fullmatch(r'[0-9a-f]{40,64}', commit):
            fail('invalid supplied source commit')
        digest(source_hash, 'supplied source tree')
        if not isinstance(dirty, bool):
            fail('invalid supplied dirty state')
        return commit, source_hash, dirty
    def git(*args):
        return subprocess.check_output(['git', '-C', str(ROOT), *args])
    commit = git('rev-parse', 'HEAD').decode().strip()
    if not re.fullmatch(r'[0-9a-f]{40,64}', commit):
        fail('invalid source commit')
    dirty = bool(git('status', '--porcelain=v1', '--untracked-files=all'))
    names = git('ls-files', '-z', '--cached', '--others', '--exclude-standard').split(b'\0')
    h = hashlib.sha256()
    for raw in sorted(set(filter(None, names))):
        name = os.fsdecode(raw).replace(os.sep, '/')
        safe(name)
        path = ROOT / name
        if not path.exists() and not path.is_symlink():
            continue
        if path.is_symlink():
            data = os.readlink(path).encode('utf-8')
            kind = b'L'
        elif path.is_file():
            data = path.read_bytes()
            kind = b'F'
        else:
            fail(f'unsupported source entry: {name}')
        h.update(kind + len(raw).to_bytes(8, 'big') + raw + len(data).to_bytes(8, 'big') + data)
    return commit, h.hexdigest(), dirty


def source_snapshot_hash(path):
    h = hashlib.sha256()
    seen = set()
    with tarfile.open(path, 'r:xz') as archive:
        for member in sorted(archive.getmembers(), key=lambda item: item.name):
            if not member.isfile() or not member.name.startswith('convertme/'):
                fail(f'unsafe source snapshot member: {member.name}')
            raw = member.name[len('convertme/'):].encode('utf-8')
            name = raw.decode('utf-8')
            safe(name)
            if name.casefold() in seen:
                fail(f'duplicate source snapshot member: {name}')
            seen.add(name.casefold())
            stream = archive.extractfile(member)
            if stream is None:
                fail(f'missing source snapshot contents: {name}')
            data = stream.read()
            if len(data) != member.size:
                fail(f'truncated source snapshot member: {name}')
            h.update(b'F' + len(raw).to_bytes(8, 'big') + raw + len(data).to_bytes(8, 'big') + data)
    if not seen or 'cargo.lock' not in seen:
        fail('source snapshot is empty or lacks Cargo.lock')
    return h.hexdigest()


def write_source_identity(args):
    commit, source_hash, dirty = source_identity()
    if args.snapshot:
        source_hash = source_snapshot_hash(args.snapshot)
    write_atomic(args.out, json_bytes({'source_commit': commit, 'source_tree_sha256': source_hash, 'dirty': dirty}))


def locks():
    return {name: sha(file_at(ROOT, name)) for name in LOCKS}


def prefix(target):
    if target not in TARGETS:
        fail(f'unsupported target: {target}')
    platform, arch = TARGETS[target]
    return f'ConvertMe-{VERSION}-{platform}-{arch}'


def names(target):
    p = prefix(target)
    if TARGETS[target][0] == 'windows':
        packages = [p + '-setup.exe', p + '-portable.zip', p + '-sources-and-licenses.zip']
    else:
        packages = [p + '.' + ext for ext in ('deb', 'rpm', 'AppImage')] + [p + '-sources-and-licenses.tar.xz']
    return packages, p + '-inventory.json', p + '-SHA256SUMS.txt'


def ui_identity(root):
    files = tree(root)
    dates = []
    for name in files:
        if not name.endswith('.js'):
            continue
        text = file_at(root, name).read_text(encoding='utf-8', errors='replace')
        for match in re.finditer(r'\d{4}-\d{2}-\d{2} \d{2}:\d{2}', text):
            dates.append((match.group(), 'Interface build' in text[max(0, match.start()-400):match.start()+400]))
    if not dates:
        fail('UI bundle does not expose an interface build identity')
    return files, next((value for value, near in dates if near), dates[0][0])


def validate_compliance(root, payload_hash):
    manifest = load(file_at(root, 'manifest.json'))
    if manifest.get('schema_version') != 1 or not isinstance(manifest.get('files'), list) or not manifest['files']:
        fail('invalid compliance manifest')
    seen, sources, kinds = set(), set(), set()
    for item in manifest['files']:
        if not isinstance(item, dict):
            fail('invalid compliance file record')
        name = str(safe(item.get('path')))
        if name.casefold() in seen:
            fail(f'duplicate compliance path: {name}')
        seen.add(name.casefold())
        if not item.get('component') or not item.get('kind'):
            fail(f'compliance ownership missing: {name}')
        if sha(file_at(root, name)) != digest(item.get('sha256'), name):
            fail(f'compliance hash mismatch: {name}')
        kinds.add(item['kind'])
        if item['kind'] == 'source':
            sources.add(item['component'])
    missing = {'ffmpeg', 'ghostscript', 'poppler', 'pandoc', 'pymupdf', 'convertme-pdf'} - sources
    if missing or 'license' not in kinds:
        fail(f'compliance missing sources/licenses: {sorted(missing)}')
    file_at(root, 'THIRD-PARTY-RUST.md')
    if payload_hash not in json.dumps(manifest, sort_keys=True):
        fail('compliance manifest does not bind payload manifest SHA256')
    return manifest


def validate_payload(root, target):
    files = tree(root)
    manifest = load(file_at(root, 'manifest.json'))
    executables = manifest.get('executables')
    if manifest.get('schema_version') != 1 or manifest.get('helper_arch') != 'x86_64' or not isinstance(executables, dict) or not REQUIRED_HELPERS <= executables.keys():
        fail('invalid or incomplete tools manifest')
    helpers = {}
    for name in sorted(REQUIRED_HELPERS):
        rel = str(safe(executables[name]))
        p = file_at(root, rel)
        expected = 'AMD64' if TARGETS[target][0] == 'windows' else 'x86_64'
        # ImageMagick's AppRun may be a launcher rather than an ELF.
        with p.open('rb') as stream:
            launcher = name == 'magick' and TARGETS[target][0] == 'linux' and stream.read(2) == b'#!'
        if launcher:
            m = 'launcher'
        else:
            m = machine(p)
            if m != expected:
                fail(f'{name}: unexpected machine {m}, expected {expected}')
        helpers[name] = {'path': rel, 'sha256': sha(p), 'machine': m}
    recorded = manifest.get('files')
    if not isinstance(recorded, dict):
        fail('tools manifest lacks file hashes')
    if set(recorded) != set(files) - {'manifest.json'}:
        fail('tools manifest does not account for every payload file')
    for name, value in recorded.items():
        expected = value.get('sha256') if isinstance(value, dict) else value
        if files.get(str(safe(name))) != digest(expected, name):
            fail(f'tools manifest hash mismatch: {name}')
    return manifest, files, helpers


def archive_entries(path):
    result, folded = {}, set()
    if str(path).endswith('.zip'):
        with zipfile.ZipFile(path) as archive:
            for item in archive.infolist():
                name = item.filename.rstrip('/')
                if not name:
                    fail(f'empty archive path: {path}')
                safe(name)
                if name.casefold() in folded:
                    fail(f'duplicate archive member: {name}')
                folded.add(name.casefold())
                mode = item.external_attr >> 16
                if stat.S_ISLNK(mode) or (stat.S_IFMT(mode) not in (0, stat.S_IFREG, stat.S_IFDIR)):
                    fail(f'non-regular archive member: {name}')
                if not item.is_dir():
                    with archive.open(item) as stream:
                        h = hashlib.sha256()
                        for block in iter(lambda: stream.read(1024 * 1024), b''):
                            h.update(block)
                        result[name] = h.hexdigest()
    else:
        with tarfile.open(path, 'r:xz') as archive:
            for item in archive:
                name = item.name.rstrip('/')
                while name.startswith('./'):
                    name = name[2:]
                if not name and item.isdir():
                    continue
                safe(name)
                if name.casefold() in folded:
                    fail(f'duplicate archive member: {name}')
                folded.add(name.casefold())
                if not item.isfile() and not item.isdir():
                    fail(f'non-regular archive member: {name}')
                if item.isfile():
                    stream = archive.extractfile(item)
                    h = hashlib.sha256()
                    for block in iter(lambda: stream.read(1024 * 1024), b''):
                        h.update(block)
                    result[name] = h.hexdigest()
    if not result:
        fail(f'empty archive: {path}')
    return result


def require_archive_files(archive, expected):
    entries = archive_entries(archive)
    for name, value in expected.items():
        matches = [v for k, v in entries.items() if k == name or k.endswith('/' + name)]
        if len(matches) != 1 or matches[0] != value:
            fail(f'{archive}: missing, ambiguous or altered member {name}')
    return entries
def verify_materials(archive_path, inventory):
    """Recheck archived materials against their archived source lock and tree."""
    compliance = inventory.get('compliance')
    if not isinstance(compliance, dict) or compliance.get('schema_version') != 1 or compliance.get('target') != inventory['target']:
        fail('invalid target compliance manifest')
    records = compliance.get('files')
    if not isinstance(records, list) or not records:
        fail('missing compliance file records')
    expected = {}
    for item in records:
        name = str(safe(item.get('path')))
        if name.casefold() in {x.casefold() for x in expected}:
            fail(f'duplicate compliance record: {name}')
        expected[name] = digest(item.get('sha256'), name)
    if 'convertme-source-tree.tar.xz' not in expected or 'THIRD-PARTY-RUST.md' not in expected:
        fail('missing source snapshot or Rust license inventory')
    if compliance.get('payload_manifest_sha256') != inventory['payload_files'].get('manifest.json'):
        fail('compliance payload binding mismatch')
    if compliance.get('sources_lock_sha256') != inventory['input_lock_hashes'].get('packaging/sources.lock.json') or compliance.get('cargo_lock_sha256') != inventory['input_lock_hashes'].get('Cargo.lock'):
        fail('compliance input lock binding mismatch')
    entries = require_archive_files(archive_path, expected)
    snapshot_names = [name for name in entries if name == 'convertme-source-tree.tar.xz' or name.endswith('/convertme-source-tree.tar.xz')]
    if len(snapshot_names) != 1:
        fail('missing or ambiguous source snapshot')
    manifest_name = snapshot_names[0].removesuffix('convertme-source-tree.tar.xz') + 'manifest.json'
    if manifest_name not in entries:
        fail('source archive missing compliance manifest')
    with tempfile.TemporaryDirectory(prefix='convertme-source-check-') as temp:
        snapshot = Path(temp) / 'snapshot.tar.xz'
        if str(archive_path).endswith('.zip'):
            with zipfile.ZipFile(archive_path) as outer:
                if json.loads(outer.read(manifest_name).decode('utf-8-sig')) != compliance:
                    fail('source archive compliance manifest differs from inventory')
                with outer.open(snapshot_names[0]) as source, snapshot.open('wb') as dest:
                    shutil.copyfileobj(source, dest)
        else:
            with tarfile.open(archive_path, 'r:xz') as outer:
                manifests = [m for m in outer if m.name.removeprefix('./') == manifest_name and m.isfile()]
                if len(manifests) != 1 or json.load(outer.extractfile(manifests[0])) != compliance:
                    fail('source archive compliance manifest differs from inventory')
                matches = [m for m in outer if m.name.removeprefix('./') == snapshot_names[0] and m.isfile()]
                if len(matches) != 1:
                    fail('cannot locate source snapshot')
                with outer.extractfile(matches[0]) as source, snapshot.open('wb') as dest:
                    shutil.copyfileobj(source, dest)
        h = hashlib.sha256()
        source_lock = None
        with tarfile.open(snapshot, 'r:xz') as source:
            members = {}
            folded = set()
            for item in source:
                name = item.name.rstrip('/')
                if name == 'convertme' and item.isdir():
                    continue
                if not name.startswith('convertme/'):
                    fail(f'unsafe source snapshot member: {name}')
                rel = name[len('convertme/'):]
                safe(rel)
                if rel.casefold() in folded:
                    fail(f'duplicate source snapshot member: {rel}')
                folded.add(rel.casefold())
                if item.isdir():
                    continue
                if not item.isfile() and not item.issym():
                    fail(f'unsupported source snapshot member: {rel}')
                members[rel] = item
            if not set(inventory['input_lock_hashes']) <= set(members):
                fail('source snapshot is missing one or more input locks')
            for name in sorted(members, key=os.fsencode):
                item = members[name]
                raw = os.fsencode(name)
                if item.issym():
                    content = item.linkname.encode('utf-8')
                    kind = b'L'
                else:
                    with source.extractfile(item) as stream:
                        content = stream.read()
                    kind = b'F'
                h.update(kind + len(raw).to_bytes(8, 'big') + raw + len(content).to_bytes(8, 'big') + content)
                if name == 'packaging/sources.lock.json':
                    source_lock = json.loads(content)
                if name in inventory['input_lock_hashes'] and hash_bytes(content) != inventory['input_lock_hashes'][name]:
                    fail(f'source snapshot input lock mismatch: {name}')
        if h.hexdigest() != inventory['source_tree_sha256']:
            fail('source snapshot does not match candidate source tree')
    if not isinstance(source_lock, dict) or source_lock.get('components') != compliance.get('components') or source_lock.get('payload_ownership', {}).get(inventory['target']) != compliance.get('payload_ownership'):
        fail('archived source provenance differs from locked records')
    if set(compliance.get('payload_ownership', {})) != set(inventory['payload_files']) - {'manifest.json'}:
        fail('source lock does not attribute every payload file')
    for component, data in source_lock['components'].items():
        if not isinstance(data, dict) or not data.get('version'):
            fail(f'{component}: missing locked version')
        for kind, key in (('source', 'sources'), ('license', 'licenses'), ('recipe', 'recipes')):
            if not isinstance(data.get(key), list) or not data[key]:
                fail(f'{component}: missing locked {kind}')
            for item in data[key]:
                path = 'materials/' + str(safe(item.get('path')))
                if expected.get(path) != digest(item.get('sha256'), path):
                    fail(f'{component}: archived {kind} differs from source lock: {path}')
                if not any(r.get('path') == path and r.get('component') == component and r.get('kind') == kind for r in records):
                    fail(f'{component}: missing archived {kind} ownership: {path}')


def extracted_files(root, expected, package):
    """Match each file to a unique contained path in the extracted package."""
    found = {name: [] for name in expected}
    for directory, dirs, files in os.walk(root, followlinks=False):
        dirs[:] = [name for name in dirs if not (Path(directory) / name).is_symlink()]
        for name in files:
            path = Path(directory) / name
            rel = path.relative_to(root).as_posix()
            parts = rel.split('/')
            for index in range(len(parts)):
                suffix = '/'.join(parts[index:])
                if suffix in found:
                    found[suffix].append(rel)
    for suffix, paths in found.items():
        if len(paths) != 1 or sha(file_at(root, paths[0])) != expected[suffix]:
            fail(f'{package}: missing, ambiguous or altered packaged file {suffix}')


def extract_package(package, destination, platform):
    if platform == 'windows':
        extractor = shutil.which('7zz') or shutil.which('7z') or shutil.which('7za')
        if not extractor:
            fail('package inspection requires 7zz/7z on PATH')
        subprocess.run([extractor, 'x', '-y', '-bd', '-o' + str(destination), str(package)], check=True, stdout=subprocess.DEVNULL)
    elif str(package).endswith('.AppImage'):
        extractor = shutil.which('unsquashfs')
        if not extractor:
            fail('AppImage inspection requires unsquashfs on PATH')
        offsets = []
        with Path(package).open('rb') as stream, mmap.mmap(stream.fileno(), 0, access=mmap.ACCESS_READ) as data:
            pos = 0
            while True:
                pos = data.find(b'hsqs', pos)
                if pos < 0:
                    break
                probe = subprocess.run([extractor, '-s', '-offset', str(pos), str(package)], stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
                if probe.returncode == 0:
                    offsets.append(pos)
                pos += 4
        if len(offsets) != 1:
            fail(f'{package}: expected one valid SquashFS image, found {len(offsets)}')
        subprocess.run([extractor, '-no-progress', '-offset', str(offsets[0]), '-d', str(destination), str(package)], check=True, stdout=subprocess.DEVNULL)
    else:
        extractor = shutil.which('bsdtar')
        if not extractor:
            fail('DEB/RPM inspection requires bsdtar on PATH')
        destination.mkdir()
        if str(package).endswith('.deb'):
            dpkg = shutil.which('dpkg-deb')
            if not dpkg:
                fail('DEB inspection requires dpkg-deb on PATH')
            producer = subprocess.Popen([dpkg, '--fsys-tarfile', str(package)], stdout=subprocess.PIPE)
            try:
                subprocess.run([extractor, '-xf', '-', '-C', str(destination)], stdin=producer.stdout, check=True, stdout=subprocess.DEVNULL)
            finally:
                producer.stdout.close()
                status = producer.wait()
            if status:
                fail(f'{package}: dpkg-deb extraction failed ({status})')
        else:
            subprocess.run([extractor, '-xf', str(package), '-C', str(destination)], check=True, stdout=subprocess.DEVNULL)


def verify_package_contents(root, target, inventory):
    packages, _, _ = names(target)
    app_name = 'convertme.exe' if TARGETS[target][0] == 'windows' else 'usr/bin/convertme'
    expected = {app_name: inventory['app']['sha256']}
    expected.update({'tools/' + name: value for name, value in inventory['payload_files'].items()})
    for name in packages[:-1]:
        if name.endswith('-portable.zip'):
            continue
        package = file_at(root, name).resolve()
        package_expected = dict(expected)
        if TARGETS[target][0] == 'windows':
            package_expected['convertme-webview2-runtime.exe'] = inventory['runtime']['sha256']
        with tempfile.TemporaryDirectory(prefix='convertme-package-check-') as temp:
            destination = Path(temp) / 'contents'
            extract_package(package, destination, TARGETS[target][0])
            extracted_files(destination, package_expected, name)




def create(args):
    target, out = args.target, Path(args.artifacts)
    packages, inventory_name, sums_name = names(target)
    payload, payload_files, helpers = validate_payload(Path(args.payload), target)
    compliance = validate_compliance(Path(args.compliance), sha(file_at(args.payload, 'manifest.json')))
    ui_files, ui_build = ui_identity(ROOT / 'crates/convertme-gui/ui-dist')
    exe = ROOT / 'target' / target / 'release' / ('convertme.exe' if TARGETS[target][0] == 'windows' else 'convertme')
    app_machine = machine(exe)
    expected_machine = ('AMD64' if TARGETS[target][1] == 'x64' else 'ARM64') if TARGETS[target][0] == 'windows' else 'x86_64'
    if app_machine != expected_machine:
        fail(f'application has unexpected architecture {app_machine}')
    artifact_paths = [file_at(out, name) for name in packages]
    compliance_files = tree(args.compliance)
    require_archive_files(artifact_paths[-1], compliance_files)
    runtime = None
    if TARGETS[target][0] == 'windows':
        portable = artifact_paths[1]
        expected = {'convertme.exe': sha(exe)}
        expected.update({'tools/' + k: v for k, v in payload_files.items()})
        entries = require_archive_files(portable, expected)
        runtime_name = 'MicrosoftEdgeWebView2RuntimeInstaller' + ('X64' if TARGETS[target][1] == 'x64' else 'ARM64') + '.exe'
        matches = [v for k, v in entries.items() if k.endswith('/runtime/' + runtime_name)]
        if len(matches) != 1:
            fail(f'portable archive missing matching runtime: {runtime_name}')
        lock = load(file_at(ROOT, 'packaging/windows/webview2.lock.json'))
        record = lock.get('architectures', {}).get(TARGETS[target][1], {})
        if lock.get('schema_version') != 1 or record.get('architecture') != TARGETS[target][1] or record.get('file') != runtime_name or record.get('sha256') != matches[0]:
            fail(f'portable runtime does not match architecture-specific lock: {runtime_name}')
        runtime = {'file': runtime_name, 'sha256': matches[0], 'architecture': TARGETS[target][1]}
    commit, source_hash, dirty = source_identity()
    inventory = {'schema_version': 1, 'target': target, 'version': VERSION, 'helper_arch': 'x86_64',
                 'app': {'sha256': sha(exe), 'machine': app_machine, 'ui_build': ui_build},
                 'helpers': helpers, 'payload_manifest': payload, 'payload_files': payload_files,
                 'ui_files': ui_files, 'compliance': compliance, 'input_lock_hashes': locks(),
                 'source_commit': commit, 'source_tree_sha256': source_hash, 'dirty': dirty,
                 'artifacts': [{'file': p.name, 'sha256': sha(p)} for p in artifact_paths]}
    if runtime:
        inventory['runtime'] = runtime
    verify_materials(artifact_paths[-1], inventory)
    verify_package_contents(out, target, inventory)
    write_atomic(out / inventory_name, json_bytes(inventory))
    sums = ''.join(f'{sha(out / name)}  {name}\n' for name in packages + [inventory_name])
    write_atomic(out / sums_name, sums.encode('ascii'))


def verify_set(root, target):
    packages, inv_name, sums_name = names(target)
    inventory = load(file_at(root, inv_name))
    if inventory.get('schema_version') != 1 or inventory.get('target') != target or inventory.get('version') != VERSION or inventory.get('helper_arch') != 'x86_64':
        fail(f'{inv_name}: invalid identity')
    records = inventory.get('artifacts')
    if not isinstance(records, list) or len(records) != len(packages) or {x.get('file') for x in records if isinstance(x, dict)} != set(packages):
        fail(f'{inv_name}: incorrect artifact set')
    for record in records:
        name = record['file']
        if sha(file_at(root, name)) != digest(record.get('sha256'), name):
            fail(f'{name}: artifact hash mismatch')
    lines = file_at(root, sums_name).read_text(encoding='ascii').splitlines()
    sums = {}
    for line in lines:
        m = re.fullmatch(r'([0-9a-f]{64})  (.+)', line)
        if not m or m[2] in sums:
            fail(f'{sums_name}: malformed or duplicate checksum')
        sums[m[2]] = m[1]
    if set(sums) != set(packages + [inv_name]):
        fail(f'{sums_name}: incorrect checksum set')
    for name, value in sums.items():
        if sha(file_at(root, name)) != value:
            fail(f'{name}: checksum mismatch')
    app = inventory.get('app', {})
    digest(app.get('sha256'), 'application')
    if not app.get('ui_build') or app.get('machine') != (('AMD64' if TARGETS[target][1] == 'x64' else 'ARM64') if TARGETS[target][0] == 'windows' else 'x86_64'):
        fail(f'{inv_name}: invalid app identity')
    for key in ('source_tree_sha256',):
        digest(inventory.get(key), key)
    if not isinstance(inventory.get('dirty'), bool) or not re.fullmatch(r'[0-9a-f]{40,64}', inventory.get('source_commit', '')):
        fail(f'{inv_name}: invalid source identity')
    for key in ('input_lock_hashes', 'payload_files', 'ui_files', 'helpers'):
        if not isinstance(inventory.get(key), dict) or not inventory[key]:
            fail(f'{inv_name}: missing {key}')
    if set(inventory['input_lock_hashes']) != set(LOCKS) or not REQUIRED_HELPERS <= inventory['helpers'].keys():
        fail(f'{inv_name}: incomplete locks/helpers')
    for key in ('input_lock_hashes', 'payload_files', 'ui_files'):
        for name, value in inventory[key].items():
            safe(name)
            digest(value, name)
    if TARGETS[target][0] == 'windows':
        runtime = inventory.get('runtime', {})
        digest(runtime.get('sha256'), 'runtime')
        if runtime.get('architecture') != TARGETS[target][1]:
            fail(f'{inv_name}: incorrect runtime architecture')
        expected = {'convertme.exe': app['sha256'], 'runtime/' + runtime['file']: runtime['sha256']}
        expected.update({'tools/' + k: v for k, v in inventory['payload_files'].items()})
        require_archive_files(file_at(root, packages[1]), expected)
    verify_materials(file_at(root, packages[-1]), inventory)
    verify_package_contents(root, target, inventory)
    return inventory


def locate_sets(root):
    root = Path(root)
    result = {}
    for target in TARGETS:
        inv_name = names(target)[1]
        hits = list(root.rglob(inv_name))
        if len(hits) != 1:
            fail(f'{target}: expected one inventory, found {len(hits)}')
        result[target] = hits[0].parent
    return result


def fixture_manifest(path):
    # Validate every archive member before consuming the manifest.
    entries = archive_entries(path)
    with zipfile.ZipFile(path) as archive:
        raw = archive.read('input-hashes.json')
    def unique(pairs):
        result = {}
        for k, v in pairs:
            if k in result:
                fail(f'duplicate fixture manifest entry: {k}')
            result[k] = v
        return result
    manifest = json.loads(raw, object_pairs_hook=unique)
    if not isinstance(manifest, dict):
        fail('invalid fixture manifest')
    # Import the audit's authoritative full CLI input set, without running it.
    spec = importlib.util.spec_from_file_location('release_quality_audit', ROOT / 'scripts/quality-audit.py')
    audit = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = audit
    spec.loader.exec_module(audit)
    required = set(UI_INPUTS) | set(audit.MATRIX_FIXTURES.values())
    for case in audit.CASES:
        required.update([case.source] if isinstance(case.source, str) else case.source)
    if not required <= manifest.keys():
        fail(f'fixture manifest missing required inputs: {sorted(required - manifest.keys())}')
    if set(entries) != set(manifest) | {'input-hashes.json'}:
        fail('fixture archive and manifest contents differ')
    for name, value in manifest.items():
        safe(name)
        if entries.get(name) != digest(value, name):
            fail(f'fixture hash mismatch: {name}')
    return hash_bytes(raw)


def candidate(args):
    root = Path(args.artifacts_root)
    sets = locate_sets(root)
    inventories, assets, identities = [], [], []
    for target, directory in sorted(sets.items()):
        inventory = verify_set(directory, target)
        identities.append((inventory['source_commit'], inventory['source_tree_sha256'], inventory['dirty'], inventory['input_lock_hashes']))
        packages, inv, sums = names(target)
        rel = (directory / inv).relative_to(root).as_posix()
        inventories.append({'path': rel, 'sha256': sha(directory / inv)})
        for name in packages + [inv, sums]:
            path = directory / name
            assets.append({'path': path.relative_to(root).as_posix(), 'sha256': sha(path), 'size': path.stat().st_size, 'target': target})
    if any(identity != identities[0] for identity in identities[1:]):
        fail('inventories bind different source trees or input locks')
    commit, source_hash, dirty, lock_hashes = identities[0]
    document = {'schema_version': 1, 'version': VERSION, 'source_commit': commit, 'source_tree_sha256': source_hash, 'dirty': dirty,
                'input_lock_hashes': lock_hashes, 'assets': sorted(assets, key=lambda x: x['path']),
                'inventories': sorted(inventories, key=lambda x: x['path']),
                'fixtures_archive_sha256': sha(args.fixtures_archive), 'fixtures_manifest_sha256': fixture_manifest(args.fixtures_archive)}
    write_atomic(args.out, json_bytes(document))


def verify(args):
    root = Path(args.artifacts)
    if not args.candidate:
        found = [target for target in TARGETS if (root / names(target)[1]).is_file()]
        if len(found) != 1:
            fail('artifact directory must contain exactly one target inventory')
        verify_set(root, found[0])
        if args.evidence:
            fail('evidence verification requires --candidate')
        return
    doc = load(args.candidate)
    if doc.get('schema_version') != 1 or doc.get('version') != VERSION:
        fail('invalid candidate identity')
    sets = locate_sets(root)
    expected_assets, expected_inventories = [], []
    for target, directory in sorted(sets.items()):
        inv = verify_set(directory, target)
        for key in ('source_commit', 'source_tree_sha256', 'dirty', 'input_lock_hashes'):
            if inv.get(key) != doc.get(key):
                fail(f'{target}: candidate {key} mismatch')
        packages, inventory_name, sums = names(target)
        for name in packages + [inventory_name, sums]:
            p = file_at(directory, name)
            rel = p.relative_to(root).as_posix()
            expected_assets.append({'path': rel, 'sha256': sha(p), 'size': p.stat().st_size, 'target': target})
        p = directory / inventory_name
        expected_inventories.append({'path': p.relative_to(root).as_posix(), 'sha256': sha(p)})
    if doc.get('assets') != sorted(expected_assets, key=lambda x: x['path']) or doc.get('inventories') != sorted(expected_inventories, key=lambda x: x['path']):
        fail('candidate asset/inventory list or hashes differ')
    digest(doc.get('fixtures_archive_sha256'), 'fixture archive')
    digest(doc.get('fixtures_manifest_sha256'), 'fixture manifest')
    if not args.evidence:
        return
    spec = importlib.util.spec_from_file_location('release_acceptance_contract', ROOT / 'scripts/acceptance-contract.py')
    contract = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = contract
    spec.loader.exec_module(contract)
    contract.verify_evidence(Path(args.candidate), Path(args.evidence), doc, root)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    commands = parser.add_subparsers(dest='command', required=True)
    p = commands.add_parser('source-identity')
    p.add_argument('--out', type=Path, required=True)
    p.add_argument('--snapshot', type=Path)
    p.set_defaults(run=write_source_identity)
    p = commands.add_parser('create')
    p.add_argument('--target', required=True, choices=TARGETS)
    p.add_argument('--artifacts', required=True)
    p.add_argument('--payload', required=True)
    p.add_argument('--compliance', required=True)
    p.set_defaults(run=create)
    p = commands.add_parser('verify')
    p.add_argument('--artifacts', required=True)
    p.add_argument('--candidate')
    p.add_argument('--evidence')
    p.set_defaults(run=verify)
    p = commands.add_parser('candidate')
    p.add_argument('--artifacts-root', required=True)
    p.add_argument('--fixtures-archive', required=True)
    p.add_argument('--out', required=True)
    p.set_defaults(run=candidate)
    args = parser.parse_args()
    args.run(args)


if __name__ == '__main__':
    try:
        main()
    except (ValueError, OSError, KeyError, TypeError, json.JSONDecodeError, subprocess.CalledProcessError, zipfile.BadZipFile, tarfile.TarError) as error:
        print(f'release-inventory: {error}', file=sys.stderr)
        sys.exit(1)
