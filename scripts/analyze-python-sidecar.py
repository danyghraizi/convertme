#!/usr/bin/env python3
"""Inspect a PyInstaller sidecar using its locked build-venv Python.

The owner map is captured alongside exact component source records. Run this
with the Python environment that built the sidecar; no imports are installed.
"""
import argparse
import hashlib
import importlib.metadata
import json
from pathlib import Path
import re
import sys

from PyInstaller.archive.readers import CArchiveReader


def normalized(name):
    return re.sub(r'[-_.]+', '-', name).lower()


def sha(data):
    return hashlib.sha256(data).hexdigest()


def main():
    p = argparse.ArgumentParser(description=__doc__)
    p.add_argument('--sidecar', type=Path, required=True)
    p.add_argument('--owners', type=Path, required=True, help='JSON distributions and native_sha256 ownership maps')
    p.add_argument('--out', type=Path, required=True)
    if sys.prefix == sys.base_prefix:
        raise RuntimeError('run this analyzer with the isolated sidecar build virtual environment')
    cfg = Path(sys.prefix) / 'pyvenv.cfg'
    if not cfg.is_file() or not re.search(r'^include-system-site-packages\s*=\s*false\s*$', cfg.read_text(encoding='utf-8'), re.IGNORECASE | re.MULTILINE):
        raise RuntimeError('sidecar build virtual environment must exclude system site packages')
    args = p.parse_args()
    owners = json.loads(args.owners.read_text(encoding='utf-8'))
    distributions = owners['distributions']
    native_owners = owners['native_sha256']
    archive = CArchiveReader(str(args.sidecar))
    entries = []
    native = []
    module_names = set()
    for name, metadata in sorted(archive.toc.items()):
        code = metadata[-1]
        data = archive.extract(name)
        record = {'name': name, 'type': code, 'size': len(data), 'sha256': sha(data)}
        entries.append(record)
        if code == 'z':
            pyz = archive.open_embedded_archive(name)
            module_names.update(pyz.toc.keys())
        elif code in ('M', 'm', 's'):
            module_names.add(name)
        if code == 'b' or name.lower().endswith(('.dll', '.pyd', '.so', '.dylib')):
            owner = native_owners.get(record['sha256'])
            if not owner:
                raise RuntimeError(f'unowned sidecar native library: {name} {record["sha256"]}')
            native.append({**record, 'component': owner})
    if not entries or not native or not module_names:
        raise RuntimeError('sidecar lacks CArchive, PYZ modules, or native library entries')
    top_level = {name.split('.')[0] for name in module_names}
    provider = importlib.metadata.packages_distributions()
    modules_by_dist = {}
    for module, names in provider.items():
        for dist in names:
            modules_by_dist.setdefault(normalized(dist), set()).add(module)
    packages = []
    installed = {}
    for dist in importlib.metadata.distributions():
        name = dist.metadata.get('Name')
        if not name:
            continue
        key = normalized(name)
        if key in installed:
            raise RuntimeError(f'ambiguous installed Python distribution: {key}')
        installed[key] = dist.version
        pin = distributions.get(key)
        if not isinstance(pin, dict) or pin.get('version') != dist.version or not pin.get('component'):
            raise RuntimeError(f'missing exact source owner for Python distribution: {key}=={dist.version}')
        found = sorted(top_level & modules_by_dist.get(key, set()))
        declared = pin.get('when_absent')
        if found or any(item['component'] == pin['component'] for item in native):
            classification = 'included'
        elif declared in ('excluded', 'build-only'):
            classification = declared
        else:
            raise RuntimeError(f'unclassified Python distribution: {key}=={dist.version}')
        packages.append({'name': key, 'version': dist.version, 'component': pin['component'], 'classification': classification, 'observed_modules': found})
    if not packages:
        raise RuntimeError('no distributions visible in the selected build environment')
    unknown = sorted(set(distributions) - set(installed))
    if unknown:
        raise RuntimeError('owner map describes distributions absent from build venv: ' + ', '.join(unknown))
    module_owners = {}
    explicit_modules = owners.get('module_roots', {})
    for module in sorted(module_names):
        root = module.split('.')[0]
        owner = explicit_modules.get(root)
        if not owner:
            candidates = {distributions[normalized(name)]['component'] for name in provider.get(root, []) if normalized(name) in distributions}
            if len(candidates) == 1:
                owner = candidates.pop()
        if not owner:
            raise RuntimeError(f'unowned or ambiguous Python module: {module}')
        module_owners[module] = owner
    result = {'schema_version': 1, 'executable_sha256': sha(args.sidecar.read_bytes()), 'python_version': sys.version, 'packages': sorted(packages, key=lambda x: x['name']), 'carchive_entries': entries, 'pyz_modules': sorted(module_names), 'module_owners': module_owners, 'native_libraries': native, 'owners_sha256': sha(args.owners.read_bytes())}
    args.out.parent.mkdir(parents=True, exist_ok=True)
    temporary = args.out.with_name(args.out.name + '.tmp')
    try:
        temporary.write_text(json.dumps(result, indent=2, sort_keys=True) + '\n', encoding='utf-8')
        temporary.replace(args.out)
    finally:
        temporary.unlink(missing_ok=True)


if __name__ == '__main__':
    main()
