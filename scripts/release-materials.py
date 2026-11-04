#!/usr/bin/env python3
"""Assemble source and license materials from captured, hash-locked inputs.

Capture is deliberately separate: this command neither resolves versions nor
substitutes an upstream URL for corresponding source bytes.
"""
import argparse
import importlib.util
import json
import hashlib
import os
import platform
from pathlib import Path
import shutil
import subprocess
import sys
import tarfile
import tomllib
import tempfile
import uuid

ROOT = Path(__file__).resolve().parents[1]
LOCK = ROOT / 'packaging/sources.lock.json'
TARGETS = {'x86_64-pc-windows-msvc', 'aarch64-pc-windows-msvc', 'x86_64-unknown-linux-gnu'}
REQUIRED = {'ffmpeg', 'ghostscript', 'poppler', 'pandoc', 'pymupdf', 'convertme-pdf', 'rust', 'python'}

spec = importlib.util.spec_from_file_location('windows_vendor', ROOT / 'scripts/vendor-windows.py')
vendor = importlib.util.module_from_spec(spec)
spec.loader.exec_module(vendor)


def die(message):
    raise RuntimeError(message)


def cargo_about_host():
    machine = platform.machine().lower()
    arch = {'amd64': 'x86_64', 'aarch64': 'arm64'}.get(machine, machine)
    return ('windows-' if os.name == 'nt' else 'linux-') + arch


def regular(root, relative, allow_empty=False):
    parts = vendor.safe_relative(relative).parts
    path = root
    for part in parts:
        path = path / part
        if path.is_symlink() or (path.exists() and getattr(path.lstat(), 'st_file_attributes', 0) & 0x400):
            die(f'link or reparse point in material path: {path}')
    if not path.is_file() or (not allow_empty and path.stat().st_size == 0):
        die(f'missing or empty captured material: {path}')
    return path


def snapshot(dest):
    supplied = os.environ.get('CONVERTME_SOURCE_SNAPSHOT')
    if supplied:
        source = Path(supplied)
        if source.is_symlink() or not source.is_file():
            die(f'invalid supplied source snapshot: {source}')
        seen = set()
        with tarfile.open(source, 'r:xz') as archive:
            for member in archive:
                if not member.isfile() or not member.name.startswith('convertme/'):
                    die(f'unsafe supplied source member: {member.name}')
                name = member.name[len('convertme/'):]
                vendor.safe_relative(name)
                if name.casefold() in seen:
                    die(f'duplicate supplied source member: {name}')
                seen.add(name.casefold())
                path = regular(ROOT, name, allow_empty=True)
                digest = hashlib.sha256()
                with archive.extractfile(member) as stream:
                    for chunk in iter(lambda: stream.read(1024 * 1024), b''):
                        digest.update(chunk)
                if member.size != path.stat().st_size or digest.hexdigest() != vendor.digest(path):
                    die(f'supplied source snapshot differs from checkout: {name}')
        if 'cargo.lock' not in seen or 'scripts/release-materials.py' not in seen:
            die('supplied source snapshot lacks required source files')
        shutil.copyfile(source, dest)
        if vendor.digest(source) != vendor.digest(dest):
            die('source snapshot changed while being copied')
        return
    names = subprocess.check_output(['git', 'ls-files', '-z', '--cached', '--others', '--exclude-standard'], cwd=ROOT).split(b'\0')
    seen = set()
    with tarfile.open(dest, 'w:xz', format=tarfile.PAX_FORMAT) as archive:
        for raw in sorted(set(names)):
            if not raw:
                continue
            name = os.fsdecode(raw).replace(os.sep, '/')
            vendor.safe_relative(name)
            if name.casefold() in seen:
                die(f'source snapshot case collision: {name}')
            seen.add(name.casefold())
            path = ROOT.joinpath(*name.split('/'))
            if path == dest.parent or dest.parent in path.parents:
                continue
            # Deleted tracked files are intentionally absent from the actual tree.
            if not path.exists() and not path.is_symlink():
                continue
            path = regular(ROOT, name, allow_empty=True)
            archive.add(path, arcname='convertme/' + name, recursive=False)


def assemble(target, payload, inputs, out):
    lock = json.loads(LOCK.read_text(encoding='utf-8'))
    if lock.get('schema_version') != 1 or not isinstance(lock.get('components'), dict):
        die('invalid sources lock')
    if target not in TARGETS:
        die(f'unsupported target: {target}')
    manifest_path = regular(payload, 'manifest.json')
    manifest = json.loads(manifest_path.read_text(encoding='utf-8'))
    if manifest.get('schema_version') != 1 or manifest.get('helper_arch') != 'x86_64':
        die('invalid payload manifest')
    if manifest.get('files') != vendor.tree_hashes(payload):
        die('payload contents differ from manifest')
    if target.endswith('-windows-msvc'):
        subprocess.run([sys.executable, str(ROOT / 'scripts/vendor-windows.py'), '--target', target, '--validate-payload', str(payload)], check=True)
    components = lock['components']
    missing = sorted(REQUIRED - components.keys())
    if missing:
        die('uncaptured corresponding sources: ' + ', '.join(missing))
    ownership = lock.get('payload_ownership', {}).get(target)
    if not isinstance(ownership, dict) or set(ownership) != set(manifest['files']):
        die('sources lock must attribute every payload file exactly once in payload_ownership')
    for path, owner in ownership.items():
        if owner not in components:
            die(f'unowned payload file {path}: {owner}')
    rust_sources = components['rust'].get('sources', [])
    rust_packages = {(item.get('crate_name'), item.get('version'), item.get('source')): item for item in rust_sources if isinstance(item, dict)}
    for package in tomllib.loads((ROOT / 'Cargo.lock').read_text(encoding='utf-8')).get('package', []):
        source_id = package.get('source')
        if not source_id:
            continue
        key = (package['name'], package['version'], source_id)
        item = rust_packages.get(key)
        if not item:
            die(f'rust: missing exact locked crate source {key}')
        if source_id.startswith('registry+') and package.get('checksum') != item.get('sha256'):
            die(f'rust: crate archive checksum differs from Cargo.lock: {key}')
        if source_id.startswith('git+'):
            revision = source_id.rsplit('#', 1)[-1]
            bundle = regular(inputs, item['path'])
            refs = subprocess.check_output(['git', 'bundle', 'list-heads', str(bundle)], text=True)
            if revision not in {line.split()[0] for line in refs.splitlines() if line.split()}:
                die(f'rust: git bundle does not advertise locked commit: {key}')
    python_analysis = lock.get('python_analysis', {}).get(target)
    if not isinstance(python_analysis, dict) or not python_analysis.get('path') or not python_analysis.get('sha256'):
        die(f'python: missing locked PyInstaller inclusion analysis for {target}')
    analysis_path = regular(inputs, python_analysis['path'])
    if vendor.digest(analysis_path) != python_analysis['sha256']:
        die('python: PyInstaller analysis hash mismatch')
    analysis = json.loads(analysis_path.read_text(encoding='utf-8'))
    sidecar = manifest.get('executables', {}).get('convertme-pdf')
    if not sidecar or analysis.get('executable_sha256') != manifest['files'].get(sidecar):
        die('python: analysis is not bound to the shipped sidecar')
    packages = analysis.get('packages')
    if not isinstance(packages, list) or not packages:
        die('python: missing PyInstaller package inclusion classifications')
    package_names = set()
    for package in packages:
        if not isinstance(package, dict) or not package.get('name') or not package.get('version') or package.get('classification') not in ('included', 'excluded', 'build-only'):
            die('python: invalid package inclusion record')
        key = package['name'].casefold()
        if key in package_names:
            die(f'python: duplicate package classification: {key}')
        package_names.add(key)
        if package['classification'] == 'included' and package.get('component') not in components:
            die(f'python: included package lacks corresponding source owner: {key}')
        if package['classification'] == 'included' and components[package['component']].get('version') != package['version']:
            die(f'python: source version differs from included distribution: {key}')
    if not analysis.get('carchive_entries') or not analysis.get('native_libraries'):
        die('python: missing CArchive or native library inspection')
    try:
        from PyInstaller.archive.readers import CArchiveReader
    except ImportError:
        die('python: run materials generation with the locked PyInstaller build environment')
    carchive = CArchiveReader(str(regular(payload, sidecar)))
    actual_entries = []
    actual_modules = set()
    native_names = set()
    for name, entry in sorted(carchive.toc.items()):
        kind = entry[-1]
        data = carchive.extract(name)
        actual_entries.append({'name': name, 'type': kind, 'size': len(data), 'sha256': hashlib.sha256(data).hexdigest()})
        if kind == 'z':
            actual_modules.update(carchive.open_embedded_archive(name).toc)
        elif kind in ('M', 'm', 's'):
            actual_modules.add(name)
        if kind == 'b' or name.lower().endswith(('.dll', '.pyd', '.so', '.dylib')):
            native_names.add(name)
    if actual_entries != analysis['carchive_entries'] or sorted(actual_modules) != analysis.get('pyz_modules'):
        die('python: CArchive or embedded PYZ differs from captured analysis')
    module_owners = analysis.get('module_owners')
    if not isinstance(module_owners, dict) or set(module_owners) != actual_modules:
        die('python: every embedded module must have exactly one source owner')
    for name, owner in module_owners.items():
        if owner not in components:
            die(f'python: missing source owner for embedded module {name}: {owner}')
    native_records = analysis['native_libraries']
    if len(native_records) != len(native_names) or {item.get('name') for item in native_records} != native_names:
        die('python: native library ownership is incomplete or duplicated')
    entries_by_name = {item['name']: item for item in actual_entries}
    for item in native_records:
        if item.get('component') not in components or any(item.get(k) != entries_by_name[item['name']][k] for k in ('sha256', 'size', 'type')):
            die(f"python: native library has wrong source owner or hash: {item.get('name')}")
    cargo = lock.get('cargo_about')
    if not isinstance(cargo, dict) or not all(cargo.get(k) for k in ('binaries', 'template', 'template_sha256', 'config', 'config_sha256', 'version')):
        die('missing pinned cargo-about binaries, version, template or configuration')
    host = cargo_about_host()
    pin = cargo['binaries'].get(host)
    if not pin:
        die(f'no pinned cargo-about executable for host {host}')
    binary = regular(inputs, pin['binary'])
    template = regular(ROOT, cargo['template'])
    config = regular(ROOT, cargo['config'])
    if vendor.digest(binary) != pin['sha256'] or vendor.digest(template) != cargo['template_sha256'] or vendor.digest(config) != cargo['config_sha256']:
        die('cargo-about executable, template or configuration hash mismatch')
    if cargo['version'] not in subprocess.check_output([str(binary), '--version'], text=True):
        die('cargo-about version mismatch')
    records = []
    seen = set()
    shutil.copyfile(analysis_path, out / 'python-inclusion.json')
    records.append({'component': 'python', 'kind': 'analysis', 'path': 'python-inclusion.json', 'sha256': python_analysis['sha256']})
    for component, data in sorted(components.items()):
        if not isinstance(data, dict) or not data.get('version') or not data.get('source_identity'):
            die(f'{component}: missing exact version or source identity')
        for kind, key in [('source', 'sources'), ('license', 'licenses'), ('recipe', 'recipes')]:
            items = data.get(key)
            if not isinstance(items, list) or not items:
                die(f'{component}: missing {key} records')
            for item in items:
                if not isinstance(item, dict) or not all(item.get(k) for k in ('path', 'sha256', 'size')):
                    die(f'{component}: invalid {kind} record')
                source = regular(inputs, item['path'])
                if source.stat().st_size != item['size'] or vendor.digest(source) != item['sha256']:
                    die(f'{component}: incorrect captured {kind}: {item["path"]}')
                rel = f'materials/{item["path"]}'
                if rel.casefold() in seen:
                    die(f'duplicate material destination: {rel}')
                seen.add(rel.casefold())
                destination = out.joinpath(*vendor.safe_relative(rel).parts)
                destination.parent.mkdir(parents=True, exist_ok=True)
                shutil.copyfile(source, destination)
                records.append({'component': component, 'kind': kind, 'path': rel, 'sha256': item['sha256']})
    # Verify the official installer variants from the locked source, byte for byte.
    gs = components['ghostscript']
    variants = gs.get('duplicate_members', {}).get('variants', [])
    if len(variants) != 2:
        die('ghostscript: missing two source-verified duplicate variants')
    source = regular(inputs, gs['sources'][0]['path'])
    with tarfile.open(source, 'r:*') as archive:
        members = archive.getmembers()
        for variant in variants:
            matches = [m for m in members if m.name.endswith('/' + variant['source']) and m.isfile()]
            if len(matches) != 1:
                die(f'ghostscript: ambiguous source member {variant["source"]}')
            contents = archive.extractfile(matches[0]).read()
            if len(contents) != variant['size'] or hashlib.sha256(contents).hexdigest() != variant['sha256']:
                die(f'ghostscript: source member mismatch: {variant["source"]}')
    inventory = out / 'THIRD-PARTY-RUST.md'
    with inventory.open('wb') as stream:
        subprocess.run([str(binary), 'generate', str(template), '--config', str(config), '--workspace', '--frozen', '--fail'], cwd=ROOT, stdout=stream, check=True)
    if not inventory.stat().st_size:
        die('cargo-about produced an empty inventory')
    records.append({'component': 'rust', 'kind': 'license', 'path': inventory.name, 'sha256': vendor.digest(inventory)})
    archive = out / 'convertme-source-tree.tar.xz'
    snapshot(archive)
    records.append({'component': 'convertme-pdf', 'kind': 'source', 'path': archive.name, 'sha256': vendor.digest(archive)})
    notice = out / 'NOTICE.txt'
    notice.write_text('Corresponding source, licenses and build recipes are supplied in materials/ and convertme-source-tree.tar.xz; see manifest.json for exact ownership and hashes.\n', encoding='utf-8')
    records.append({'component': 'convertme-pdf', 'kind': 'notice', 'path': notice.name, 'sha256': vendor.digest(notice)})
    if vendor.tree_hashes(out) != {item['path']: item['sha256'] for item in records}:
        die('assembled materials contain missing, unowned, colliding or changed files')
    result = {'schema_version': 1, 'target': target, 'payload_manifest_sha256': vendor.digest(manifest_path), 'sources_lock_sha256': vendor.digest(LOCK), 'cargo_lock_sha256': vendor.digest(ROOT / 'Cargo.lock'), 'components': components, 'payload_ownership': ownership, 'files': records}
    (out / 'manifest.json').write_text(json.dumps(result, indent=2, sort_keys=True) + '\n', encoding='utf-8')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--target', required=True, choices=sorted(TARGETS))
    parser.add_argument('--payload', type=Path, required=True)
    parser.add_argument('--out', type=Path, required=True)
    parser.add_argument('--inputs', type=Path, default=ROOT / 'vendor' / 'source-inputs')
    args = parser.parse_args()
    out = args.out.absolute()
    if out.is_symlink():
        die(f'materials output is a symlink: {out}')
    out.parent.mkdir(parents=True, exist_ok=True)
    stage = Path(tempfile.mkdtemp(prefix='.materials-', dir=out.parent))
    backup = out.with_name('.materials-backup-' + uuid.uuid4().hex)
    try:
        assemble(args.target, args.payload.resolve(), args.inputs.resolve(), stage)
        if out.exists():
            os.replace(out, backup)
        try:
            os.replace(stage, out)
        except BaseException:
            if backup.exists():
                os.replace(backup, out)
            raise
        if backup.exists():
            shutil.rmtree(backup)
    finally:
        if stage.exists():
            shutil.rmtree(stage)


if __name__ == '__main__':
    try:
        main()
    except (OSError, ValueError, KeyError, RuntimeError, subprocess.CalledProcessError) as error:
        print(f'release materials: {error}', file=sys.stderr)
        sys.exit(1)
