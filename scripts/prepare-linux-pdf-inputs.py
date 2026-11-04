#!/usr/bin/env python3
"""Capture or verify the frozen CPython 3.12 Linux PDF build wheelhouse."""
import argparse
import os
from pathlib import Path
import shutil
import subprocess
import sys
import tempfile

ROOT = Path(__file__).resolve().parent.parent
LOCK = ROOT / 'sidecars/pdf/requirements-linux.txt'
import hashlib
import json
import re
import urllib.parse
import urllib.request


def locked_packages():
    try:
        from packaging.utils import canonicalize_name
    except ImportError as exc:
        raise RuntimeError('capture/verify requires the standard packaging tag parser') from exc
    records = {}
    current = None
    for line in LOCK.read_text(encoding='utf-8').splitlines():
        match = re.match(r'^([A-Za-z0-9_.-]+)==([^\s\\]+)', line)
        if match:
            name, version = match.groups()
            current = canonicalize_name(name)
            records[current] = {'name': name, 'version': version, 'hashes': set()}
        if current:
            records[current]['hashes'].update(re.findall(r'--hash=sha256:([0-9a-f]{64})', line))
    if not records or any(not item['hashes'] for item in records.values()):
        raise RuntimeError('requirements-linux.txt has missing or malformed hash records')
    return records


def target_tags():
    from packaging.tags import compatible_tags, cpython_tags
    platforms = ['manylinux_2_28_x86_64', 'manylinux_2_27_x86_64',
                 'manylinux2014_x86_64', 'manylinux_2_17_x86_64']
    return (set(cpython_tags((3, 12), abis=['cp312', 'abi3'], platforms=platforms))
            | set(compatible_tags((3, 12), interpreter='cp312', platforms=platforms)))


def verify_wheelhouse(directory):
    from packaging.utils import canonicalize_name, parse_wheel_filename
    records = locked_packages()
    files = [path for path in directory.iterdir() if path.is_file() and not path.is_symlink()]
    if len(files) != len(records):
        raise RuntimeError(f'wheelhouse has {len(files)} files for {len(records)} locked distributions')
    remaining = set(records)
    allowed = target_tags()
    for path in files:
        try:
            name, version, _, tags = parse_wheel_filename(path.name)
        except Exception as exc:
            raise RuntimeError(f'unexpected non-wheel in CPython 3.12 wheelhouse: {path.name}') from exc
        key = canonicalize_name(str(name))
        record = records.get(key)
        if record is None or key not in remaining or str(version) != record['version']:
            raise RuntimeError(f'unlocked or duplicate wheel: {path.name}')
        if not (tags & allowed):
            raise RuntimeError(f'wheel is not compatible with Linux x86-64 CPython 3.12: {path.name}')
        sha = hashlib.file_digest(path.open('rb'), 'sha256').hexdigest()
        if sha not in record['hashes']:
            raise RuntimeError(f'wheel hash is not in requirements lock: {path.name}')
        remaining.remove(key)
    if remaining:
        raise RuntimeError('missing locked wheels: ' + ', '.join(sorted(remaining)))


def capture_wheels(destination):
    from packaging.utils import canonicalize_name, parse_wheel_filename
    allowed = target_tags()
    records = locked_packages()
    for key, record in sorted(records.items()):
        url = 'https://pypi.org/pypi/' + urllib.parse.quote(record['name']) + '/' + urllib.parse.quote(record['version']) + '/json'
        request = urllib.request.Request(url, headers={'Accept': 'application/json', 'User-Agent': 'ConvertMe-input-capture'})
        with urllib.request.urlopen(request, timeout=60) as response:
            if not response.geturl().startswith('https://'):
                raise RuntimeError(f'insecure PyPI redirect for {record["name"]}')
            metadata = json.load(response)
        candidates = []
        for item in metadata.get('urls', []):
            if item.get('packagetype') != 'bdist_wheel':
                continue
            try:
                wheel_name, version, _, tags = parse_wheel_filename(item['filename'])
            except Exception:
                continue
            if (canonicalize_name(str(wheel_name)) == key and str(version) == record['version']
                    and tags & allowed and item.get('digests', {}).get('sha256') in record['hashes']):
                candidates.append(item)
        if not candidates:
            raise RuntimeError(f'no pinned CPython 3.12 Linux wheel for {record["name"]}=={record["version"]}')
        item = sorted(candidates, key=lambda candidate: candidate['filename'])[0]
        request = urllib.request.Request(item['url'], headers={'User-Agent': 'ConvertMe-input-capture'})
        temporary = destination / (item['filename'] + '.part')
        try:
            with urllib.request.urlopen(request, timeout=120) as response, temporary.open('xb') as output:
                if not response.geturl().startswith('https://'):
                    raise RuntimeError(f'insecure wheel redirect for {item["filename"]}')
                digest = hashlib.sha256()
                while chunk := response.read(1024 * 1024):
                    digest.update(chunk)
                    output.write(chunk)
            if digest.hexdigest() != item['digests']['sha256']:
                raise RuntimeError(f'wheel bytes differ from PyPI metadata: {item["filename"]}')
            if digest.hexdigest() not in record['hashes']:
                raise RuntimeError(f'wheel bytes differ from requirements lock: {item["filename"]}')
            temporary.replace(destination / item['filename'])
        finally:
            temporary.unlink(missing_ok=True)


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    mode = parser.add_mutually_exclusive_group(required=True)
    mode.add_argument('--capture', action='store_true')
    mode.add_argument('--verify', action='store_true')
    parser.add_argument('--out', type=Path, required=True)
    args = parser.parse_args()
    out = args.out.absolute()
    if args.verify:
        if not out.is_dir() or out.is_symlink():
            raise SystemExit(f'missing locked PDF wheelhouse: {out}')
        verify_wheelhouse(out)
        return
    out.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='.pdf-wheelhouse-', dir=out.parent) as temporary:
        staged = Path(temporary) / 'wheels'
        staged.mkdir()
        try:
            subprocess.run([sys.executable, '-m', 'pip', '--version'], check=True,
                           stdout=subprocess.DEVNULL, stderr=subprocess.DEVNULL)
        except (OSError, subprocess.CalledProcessError):
            capture_wheels(staged)
        else:
            command = [sys.executable, '-m', 'pip', 'download', '--disable-pip-version-check',
                       '--no-deps', '--require-hashes', '--only-binary=:all:',
                       '--implementation', 'cp', '--python-version', '312', '--abi', 'cp312',
                       '--abi', 'abi3', '--abi', 'none', '--platform', 'manylinux_2_28_x86_64',
                       '--platform', 'manylinux_2_27_x86_64', '--platform', 'manylinux2014_x86_64',
                       '--platform', 'manylinux_2_17_x86_64', '--dest', str(staged), '-r', str(LOCK)]
            subprocess.run(command, check=True)
        verify_wheelhouse(staged)
        previous = Path(temporary) / 'previous'
        if out.exists():
            if out.is_symlink() or not out.is_dir():
                raise SystemExit(f'wheelhouse destination is not a directory: {out}')
            os.replace(out, previous)
        try:
            os.replace(staged, out)
        except BaseException:
            if previous.exists():
                os.replace(previous, out)
            raise


if __name__ == '__main__':
    main()
