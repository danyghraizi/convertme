#!/usr/bin/env python3
"""Create a candidate-bound offline Windows acceptance handoff ZIP.

The supplied Python lock must identify a retained official x64 CPython 3.12
embeddable ZIP as {"version": "3.12.x", "file": "...", "sha256": "...",
"url": "https://www.python.org/ftp/python/..."}. Capture/authenticate that
lock separately; this command never downloads or silently replaces its pin.
"""
from __future__ import annotations

import argparse
import hashlib
import importlib.util
import json
import os
from pathlib import Path, PurePosixPath
import re
import shutil
import sys
import tempfile
import zipfile

ROOT = Path(__file__).resolve().parent.parent


def load_inventory_module():
    spec = importlib.util.spec_from_file_location('release_inventory', ROOT / 'scripts/release-inventory.py')
    module = importlib.util.module_from_spec(spec)
    sys.modules[spec.name] = module
    spec.loader.exec_module(module)
    return module


def hash_file(path):
    h = hashlib.sha256()
    with path.open('rb') as stream:
        for chunk in iter(lambda: stream.read(1024 * 1024), b''):
            h.update(chunk)
    return h.hexdigest()


def checked_file(root, name):
    if not isinstance(name, str) or not name or '\\' in name or ':' in name or any(part in ('', '.', '..') for part in name.split('/')):
        raise ValueError(f'unsafe file path: {name!r}')
    root = root.resolve()
    current = root
    for part in PurePosixPath(name).parts:
        current = current / part
        if current.is_symlink():
            raise ValueError(f'symlink is not allowed: {name}')
    if not current.is_file() or not current.resolve().is_relative_to(root):
        raise ValueError(f'missing or escaping file: {name}')
    return current


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    parser.add_argument('--candidate', required=True, type=Path)
    parser.add_argument('--artifacts-root', required=True, type=Path)
    parser.add_argument('--fixtures-archive', required=True, type=Path)
    parser.add_argument('--target', required=True, choices=('x86_64-pc-windows-msvc', 'aarch64-pc-windows-msvc'))
    parser.add_argument('--python-archive', required=True, type=Path)
    parser.add_argument('--python-lock', required=True, type=Path)
    parser.add_argument('--out', required=True, type=Path)
    args = parser.parse_args()
    inventory = load_inventory_module()
    candidate = inventory.load(args.candidate)
    if candidate.get('schema_version') != 1 or candidate.get('version') != '0.1.1':
        raise ValueError('invalid candidate schema/version')
    commit, tree, dirty = inventory.source_identity()
    if (commit, tree, dirty) != (candidate.get('source_commit'), candidate.get('source_tree_sha256'), candidate.get('dirty')):
        raise ValueError('checkout source identity differs from candidate; use its exact source snapshot')
    if hash_file(args.fixtures_archive) != candidate.get('fixtures_archive_sha256'):
        raise ValueError('fixture archive does not match candidate')
    if inventory.fixture_manifest(args.fixtures_archive) != candidate.get('fixtures_manifest_sha256'):
        raise ValueError('fixture manifest does not match candidate')
    lock = inventory.load(args.python_lock)
    version = lock.get('version', '')
    expected_name = f'python-{version}-embed-amd64.zip'
    expected_url = f'https://www.python.org/ftp/python/{version}/{expected_name}'
    if (not re.fullmatch(r'3\.12\.\d+', version) or lock.get('file') != expected_name or
            lock.get('url') != expected_url or args.python_archive.name != expected_name or
            not re.fullmatch(r'[0-9a-f]{64}', str(lock.get('sha256', ''))) or
            hash_file(args.python_archive) != lock['sha256']):
        raise ValueError('Python archive does not match explicit official CPython 3.12 x64 lock')
    python_entries = inventory.archive_entries(args.python_archive)
    required_runtime = {'python.exe', 'python312.dll', 'python312.zip', 'python312._pth', 'vcruntime140.dll'}
    if not required_runtime <= python_entries.keys():
        raise ValueError(f'Python runtime archive missing: {sorted(required_runtime - python_entries.keys())}')
    assets = [entry for entry in candidate.get('assets', []) if entry.get('target') == args.target]
    packages, inventory_name, sums = inventory.names(args.target)
    expected = set(packages + [inventory_name, sums])
    if len(assets) != len(expected) or {PurePosixPath(a.get('path', '')).name for a in assets} != expected:
        raise ValueError('candidate does not contain the exact target artifact set')
    asset_files = []
    for entry in assets:
        path = checked_file(args.artifacts_root, entry['path'])
        if path.stat().st_size != entry.get('size') or hash_file(path) != entry.get('sha256'):
            raise ValueError(f'candidate asset changed: {entry["path"]}')
        asset_files.append((path, PurePosixPath(entry['path']).name))
    inv_records = [entry for entry in candidate.get('inventories', []) if PurePosixPath(entry.get('path', '')).name == inventory_name]
    if len(inv_records) != 1:
        raise ValueError('candidate has missing or duplicate target inventory')
    inv_path = checked_file(args.artifacts_root, inv_records[0]['path'])
    if hash_file(inv_path) != inv_records[0].get('sha256'):
        raise ValueError('target inventory changed')
    inv = inventory.load(inv_path)
    if inv.get('target') != args.target or any(inv.get(key) != candidate.get(key) for key in ('source_commit', 'source_tree_sha256', 'dirty', 'input_lock_hashes')):
        raise ValueError('target inventory does not match candidate identity')
    for name in packages:
        asset = next(a for a in assets if PurePosixPath(a['path']).name == name)
        if len([r for r in inv.get('artifacts', []) if r.get('file') == name and r.get('sha256') == asset['sha256']]) != 1:
            raise ValueError(f'inventory does not bind package: {name}')
    files = [
        'scripts/run-windows-acceptance.ps1', 'scripts/app-e2e-windows.ps1',
        'scripts/quality-audit.py', 'scripts/validate.py', 'scripts/acceptance-contract.py',
        'crates/convertme-core/tests/formats.snapshot.json', 'ui/src/fields.generated.json',
        'sidecars/pdf/requirements-windows.txt',
    ]
    resources = [(checked_file(ROOT, name), name) for name in files]
    if args.out.exists():
        raise ValueError(f'output already exists: {args.out}')
    args.out.parent.mkdir(parents=True, exist_ok=True)
    launcher = ('@echo off\r\n'
                'setlocal\r\n'
                'if "%~2"=="" (echo Usage: run-acceptance.cmd ^<zip^|nsis^> ^<new-output-directory^> & exit /b 2)\r\n'
                'if /I "%~1"=="zip" (set "PKG=' + next(n for _, n in asset_files if n.endswith('-portable.zip')) + '") else if /I "%~1"=="nsis" (set "PKG=' + next(n for _, n in asset_files if n.endswith('-setup.exe')) + '") else (echo Select zip or nsis & exit /b 2)\r\n'
                'set "PATH=%~dp0runtime;%SystemRoot%\\System32;%SystemRoot%"\r\n'
                'powershell.exe -NoProfile -File "%~dp0scripts\\run-windows-acceptance.ps1" -Candidate "%~dp0candidate.json" -Package "%~dp0packages\\%PKG%" -Fixtures "%~dp0fixtures" -Out "%~2"\r\n'
                'exit /b %ERRORLEVEL%\r\n').encode('ascii')
    manifest = {'schema_version': 1, 'candidate_sha256': hash_file(args.candidate), 'target': args.target,
                'fixtures_archive_sha256': candidate['fixtures_archive_sha256'],
                'fixtures_manifest_sha256': candidate['fixtures_manifest_sha256'],
                'python_archive_sha256': lock['sha256'], 'python_version': version,
                'source_tree_sha256': tree, 'files': {name: hash_file(path) for path, name in resources}}
    fd, temp = tempfile.mkstemp(prefix=f'.{args.out.name}.', suffix='.tmp', dir=args.out.parent)
    os.close(fd)
    try:
        with zipfile.ZipFile(temp, 'w', compression=zipfile.ZIP_DEFLATED, allowZip64=True) as out:
            def copy_stream(source, name):
                with out.open(name, 'w', force_zip64=True) as dest:
                    shutil.copyfileobj(source, dest, 1024 * 1024)
            for path, name in resources + [(args.candidate, 'candidate.json'), (args.python_lock, 'python-lock.json')]:
                with path.open('rb') as source:
                    copy_stream(source, name)
            for path, name in asset_files:
                with path.open('rb') as source:
                    copy_stream(source, 'packages/' + name)
            with zipfile.ZipFile(args.fixtures_archive) as fixtures:
                for item in fixtures.infolist():
                    if not item.is_dir():
                        with fixtures.open(item) as source:
                            copy_stream(source, 'fixtures/' + item.filename)
            with zipfile.ZipFile(args.python_archive) as python:
                for item in python.infolist():
                    if item.is_dir():
                        continue
                    with python.open(item) as source:
                        if item.filename == 'python312._pth':
                            original = source.read().decode('utf-8-sig')
                            out.writestr('runtime/python312._pth', original.rstrip() + '\n../scripts\n')
                        else:
                            copy_stream(source, 'runtime/' + item.filename)
            out.writestr('run-acceptance.cmd', launcher)
            out.writestr('bundle-manifest.json', json.dumps(manifest, indent=2) + '\n')
        if args.out.exists():
            raise ValueError(f'output appeared during build: {args.out}')
        os.replace(temp, args.out)
    finally:
        if os.path.exists(temp):
            os.unlink(temp)
    print(f'Created {args.out} (SHA256 {hash_file(args.out)})')


if __name__ == '__main__':
    try:
        main()
    except (OSError, ValueError, KeyError, TypeError, zipfile.BadZipFile) as error:
        print(f'prepare-windows-acceptance: {error}', file=sys.stderr)
        sys.exit(1)
