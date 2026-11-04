#!/usr/bin/env python3
"""Capture the pinned cargo-about release executable into a materials input root."""
import argparse
import hashlib
import json
import os
from pathlib import Path
import tempfile
import urllib.request
import tarfile

ROOT = Path(__file__).resolve().parents[1]


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--host', required=True, choices=('windows-x86_64', 'windows-arm64', 'linux-x86_64'))
    parser.add_argument('--out', type=Path, required=True)
    args = parser.parse_args()
    lock = json.loads((ROOT / 'packaging/sources.lock.json').read_text(encoding='utf-8'))['cargo_about']
    pin = lock['binaries'][args.host]
    dest = args.out / pin['binary']
    dest.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='.cargo-about-', dir=dest.parent) as directory:
        archive = Path(directory) / 'release.tar.gz'
        req = urllib.request.Request(f"https://api.github.com/repos/EmbarkStudios/cargo-about/releases/assets/{pin['asset_id']}", headers={'Accept': 'application/octet-stream', 'User-Agent': 'ConvertMe-materials'})
        with urllib.request.urlopen(req, timeout=120) as response, archive.open('wb') as output:
            import shutil
            shutil.copyfileobj(response, output)
        with archive.open('rb') as stream:
            archive_hash = hashlib.file_digest(stream, 'sha256').hexdigest()
        if archive_hash != pin['archive_sha256']:
            raise RuntimeError('cargo-about release archive hash mismatch')
        expected_name = 'cargo-about.exe' if args.host.startswith('windows-') else 'cargo-about'
        with tarfile.open(archive, 'r:gz') as source:
            members = source.getmembers()
            matches = [item for item in members if item.isfile() and Path(item.name).name == expected_name]
            if len(matches) != 1:
                raise RuntimeError('cargo-about archive has an ambiguous executable')
            item = matches[0]
            if item.name.startswith('/') or any(part in ('', '.', '..') for part in item.name.split('/')):
                raise RuntimeError('unsafe cargo-about archive member')
            binary = source.extractfile(item).read()
        if hashlib.sha256(binary).hexdigest() != pin['sha256']:
            raise RuntimeError('cargo-about executable hash mismatch')
        temp = Path(directory) / expected_name
        temp.write_bytes(binary)
        if not args.host.startswith('windows-'):
            temp.chmod(0o755)
        os.replace(temp, dest)
    print(dest)


if __name__ == '__main__':
    main()
