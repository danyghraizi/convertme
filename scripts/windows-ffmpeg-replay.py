#!/usr/bin/env python3
"""Replay a fully captured Windows FFmpeg toolchain without network access."""
import argparse
import io
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import re
import shutil
import struct
import subprocess
import sys
import tarfile
import tempfile
import zipfile

ROOT = Path(__file__).resolve().parents[1]
COMMIT = '5ece62897c650b863c1ddda7b6964845344aef2d'
EPOCH = 1785786727


def fail(message):
    raise ValueError(message)


def sha(path):
    with path.open('rb') as f:
        return hashlib.file_digest(f, 'sha256').hexdigest()


def relative(value):
    if not isinstance(value, str) or not value or '\\' in value:
        fail(f'unsafe input path: {value!r}')
    p = PurePosixPath(value)
    if p.is_absolute() or any(x in ('', '.', '..') for x in value.split('/')):
        fail(f'unsafe input path: {value!r}')
    return p


def check_file(root, record):
    if not isinstance(record, dict):
        fail('missing file record')
    path = root.joinpath(*relative(record.get('path')).parts)
    cursor = root
    for part in path.relative_to(root).parts:
        cursor = cursor / part
        if cursor.is_symlink():
            fail(f'link in input path: {cursor}')
    digest = record.get('sha256')
    size = record.get('size')
    if not isinstance(digest, str) or not re.fullmatch('[0-9a-f]{64}', digest) or type(size) is not int or size <= 0:
        fail(f'invalid hash/size record: {path}')
    if not path.is_file() or path.stat().st_size != size or sha(path) != digest:
        fail(f'missing or changed input: {path}')
    return path


def run(*args, **kwargs):
    return subprocess.run([str(a) for a in args], check=True, **kwargs)


def verify_git_sources(lock, root):
    with tempfile.TemporaryDirectory(prefix='ffmpeg-git-') as tmp:
        for label, bundle in (('recipe', lock['recipe']['bundle']), ('ffmpeg', lock['ffmpeg_source']['bundle']), ('buildx', lock['buildx']['source'])):
            gitdir = Path(tmp) / label
            run('git', 'clone', '--bare', '--quiet', check_file(root, bundle), gitdir)
            def rev(ref):
                return run('git', '--git-dir', gitdir, 'rev-parse', '--verify', ref,
                           stdout=subprocess.PIPE, text=True).stdout.strip()
            if label == 'buildx':
                if rev('refs/tags/v0.38.1^{commit}') != lock['buildx'].get('source_commit'):
                    fail('Buildx source tag commit mismatch')
                continue
            if label == 'ffmpeg':
                if rev('refs/tags/n9.0') != '5b9252abfe2fda0ccb71ed45cd7a954cc45d2b6f' or rev('refs/tags/n9.0^{commit}') != 'd32b387f2b0a484599d4587d651891f0c63c4238':
                    fail('FFmpeg bundle tag or commit mismatch')
                continue
            if rev(COMMIT + '^{commit}') != COMMIT:
                fail('recipe bundle does not contain the pinned commit')
            tree = run('git', '--git-dir', gitdir, 'ls-tree', '-rz', '--full-tree', COMMIT,
                       stdout=subprocess.PIPE).stdout
            tracked = set()
            for entry in tree.split(bytes([0])):
                if not entry:
                    continue
                metadata, raw_name = entry.split(bytes([9]), 1)
                mode, kind, object_id = metadata.split(b' ')
                if kind != b'blob' or mode not in (b'100644', b'100755'):
                    fail('recipe bundle contains unsupported non-regular tracked files')
                rel = 'recipe/' + raw_name.decode('utf-8')
                relative(rel)
                tracked.add(rel)
                path = root.joinpath(*PurePosixPath(rel).parts)
                if not path.is_file() or path.is_symlink():
                    fail(f'missing tracked recipe file: {rel}')
                h = hashlib.sha1()
                h.update(f'blob {path.stat().st_size}'.encode() + bytes([0]))
                with path.open('rb') as stream:
                    for chunk in iter(lambda: stream.read(1024 * 1024), b''):
                        h.update(chunk)
                if h.hexdigest().encode() != object_id:
                    fail(f'recipe file differs from pinned commit: {rel}')
            if tracked != {item['path'] for item in lock['recipe']['files']}:
                fail('recipe checkout does not match pinned Git tree')


def validate(lock, root):
    if lock.get('schema_version') != 1 or lock.get('source_epoch') != EPOCH:
        fail('incorrect FFmpeg input lock schema or source epoch')
    recipe = lock.get('recipe', {})
    if recipe.get('repo') != 'https://github.com/BtbN/FFmpeg-Builds' or recipe.get('commit') != COMMIT:
        fail('incorrect FFmpeg recipe identity')
    files = recipe.get('files')
    if not isinstance(files, list) or not files:
        fail('recipe file inventory missing')
    recorded = set()
    for item in files:
        path = check_file(root, item)
        rel = path.relative_to(root).as_posix()
        if not rel.startswith('recipe/') or rel in recorded:
            fail('duplicate or misplaced recipe file')
        recorded.add(rel)
    recipe_dir = root / 'recipe'
    actual = set()
    for path in recipe_dir.rglob('*'):
        if path.is_symlink() or not (path.is_file() or path.is_dir()):
            fail(f'unsafe recipe entry: {path}')
        if path.is_file():
            actual.add(path.relative_to(root).as_posix())
    if actual != recorded or not {'recipe/build.sh', 'recipe/download.sh', 'recipe/makeimage.sh', 'recipe/variants/win64-gpl.sh', 'recipe/variants/windows-install-static.sh', 'recipe/variants/defaults-gpl.sh'} <= recorded or not any(x.startswith('recipe/scripts.d/') for x in recorded):
        fail('incomplete or unrecorded recipe files')
    adaptation = recipe.get('adaptation', {})
    script = ROOT / 'scripts/windows-ffmpeg-replay-build.sh'
    if adaptation.get('sha256') != sha(script) or adaptation.get('size') != script.stat().st_size:
        fail('replay adaptation does not match input lock')
    source = lock.get('ffmpeg_source', {})
    if (source.get('tag'), source.get('tag_object'), source.get('commit')) != ('n9.0', '5b9252abfe2fda0ccb71ed45cd7a954cc45d2b6f', 'd32b387f2b0a484599d4587d651891f0c63c4238'):
        fail('incorrect FFmpeg source identity')
    if source.get('bundle', {}).get('path') != 'ffmpeg.bundle':
        fail('FFmpeg Git bundle missing')
    check_file(root, source['bundle'])
    image = lock.get('toolchain_image', {})
    if image.get('platform') != 'linux/amd64' or not re.fullmatch(r'sha256:[0-9a-f]{64}', str(image.get('image_id', ''))) or image.get('config_digest') != image.get('image_id') or not re.fullmatch(r'sha256:[0-9a-f]{64}', str(image.get('manifest_digest', ''))):
        fail('invalid toolchain image identity')
    archive = check_file(root, image.get('archive'))
    with tarfile.open(archive, 'r:*') as tar:
        members = [m for m in tar.getmembers() if m.name == 'manifest.json']
        if len(members) != 1 or not members[0].isfile():
            fail('Docker archive manifest missing or ambiguous')
        content = tar.extractfile(members[0]).read()
        if 'sha256:' + hashlib.sha256(content).hexdigest() != image['manifest_digest']:
            fail('Docker archive manifest digest mismatch')
        manifests = json.loads(content)
        if len(manifests) != 1 or manifests[0].get('Config') != image['image_id'][7:] + '.json':
            fail('Docker archive config identity mismatch')
    buildx = lock.get('buildx', {})
    if buildx.get('version') != 'v0.38.1' or buildx.get('source_tag') != 'v0.38.1':
        fail('Buildx release/source provenance missing')
    if sha(check_file(root, buildx.get('binary'))) != 'e25d5cd675092bb5d1d03632fdc46cdf3fede7b514dc5f023a105e00d3ce5ad4':
        fail('incorrect pinned Buildx binary')
    metadata = json.loads(check_file(root, buildx.get('release_metadata')).read_text(encoding='utf-8'))
    assets = [asset for asset in metadata.get('assets', []) if asset.get('name') == 'buildx-v0.38.1.linux-amd64']
    if metadata.get('tag_name') != 'v0.38.1' or len(assets) != 1 or assets[0].get('digest') != 'sha256:e25d5cd675092bb5d1d03632fdc46cdf3fede7b514dc5f023a105e00d3ce5ad4':
        fail('official Buildx release metadata does not match the pinned binary')
    check_file(root, buildx.get('source'))
    docker = lock.get('docker')
    if not isinstance(docker, dict) or not isinstance(docker.get('version'), str) or not docker['version'] or not isinstance(docker.get('archives'), list) or not docker['archives']:
        fail('Docker capture provenance missing')
    for item in docker['archives']:
        check_file(root, item)
    inputs = lock.get('inputs')
    if not isinstance(inputs, list) or not inputs:
        fail('toolchain source closure missing')
    owners = set()
    for entry in inputs:
        if not isinstance(entry, dict) or any(not isinstance(entry.get(k), str) or not entry[k] for k in ('owner', 'version', 'recipe')):
            fail('toolchain input owner/version/recipe missing')
        if entry['owner'] in owners:
            fail(f'duplicate toolchain input owner: {entry["owner"]}')
        owners.add(entry['owner'])
        if entry['recipe'] not in recorded:
            fail(f'{entry["owner"]}: recipe is not part of the verified recipe inventory')
        check_file(root, entry.get('artifact'))
        for kind in ('sources', 'licenses'):
            records = entry.get(kind)
            if not isinstance(records, list) or not records:
                fail(f'{entry["owner"]}: {kind} missing')
            for record in records:
                if not isinstance(record, dict) or not isinstance(record.get('url'), str) or not record['url'].startswith('https://'):
                    fail(f'{entry["owner"]}: {kind} origin missing')
                check_file(root, record)
    inventory = image.get('files')
    if not isinstance(inventory, dict) or not inventory:
        fail('complete toolchain filesystem ownership inventory missing')
    for name, item in inventory.items():
        relative(name)
        if not isinstance(item, dict) or item.get('owner') not in owners or not re.fullmatch('[0-9a-f]{64}', str(item.get('sha256', ''))) or type(item.get('size')) is not int or item['size'] < 0:
            fail(f'toolchain filesystem file has no valid owner/hash/size: {name}')
    return image, archive


def verify_image_files(image):
    created = run('docker', 'create', '--network=none', '--entrypoint', '/bin/true',
                  image['image_id'], stdout=subprocess.PIPE, text=True).stdout.strip()
    try:
        process = subprocess.Popen(['docker', 'export', created], stdout=subprocess.PIPE)
        seen = set()
        try:
            with tarfile.open(fileobj=process.stdout, mode='r|') as tar:
                for member in tar:
                    name = member.name.removeprefix('./')
                    if not name:
                        continue
                    relative(name.rstrip('/'))
                    if member.isfile():
                        expected = image['files'].get(name)
                        if expected is None or name in seen:
                            fail(f'unowned or duplicate toolchain image file: {name}')
                        h = hashlib.sha256()
                        stream = tar.extractfile(member)
                        for chunk in iter(lambda: stream.read(1024 * 1024), b''):
                            h.update(chunk)
                        if member.size != expected['size'] or h.hexdigest() != expected['sha256']:
                            fail(f'toolchain image file mismatch: {name}')
                        seen.add(name)
                    elif member.islnk():
                        target = member.linkname.removeprefix('./')
                        relative(target)
                        expected = image['files'].get(name)
                        source = image['files'].get(target)
                        if name in seen or expected is None or source is None or (expected['size'], expected['sha256']) != (source['size'], source['sha256']):
                            fail(f'unowned or mismatched toolchain hard link: {name}')
                        seen.add(name)
                    elif not (member.isdir() or member.issym()):
                        fail(f'unsupported toolchain image entry: {name}')
        finally:
            process.stdout.close()
            status = process.wait()
        if status:
            fail('Docker image export failed')
        if seen != set(image['files']):
            fail('toolchain filesystem inventory contains missing files')
    finally:
        run('docker', 'rm', '-f', created, stdout=subprocess.DEVNULL)


def verify_packages(image, inputs):
    result = run('docker', 'run', '--rm', '--network=none', '--pull=never',
                 '--entrypoint', 'dpkg-query', image['image_id'], '-W',
                 '-f=${db:Status-Status}\\t${binary:Package}\\t${Version}\\n',
                 stdout=subprocess.PIPE, text=True).stdout
    installed = {}
    for line in result.splitlines():
        state, name, version = line.split(chr(9))
        if state == 'installed':
            if name in installed:
                fail(f'duplicate installed package: {name}')
            installed[name] = version
    declared = {}
    for item in inputs:
        if item.get('kind') == 'deb':
            name = item.get('package')
            if not isinstance(name, str) or not name or name in declared:
                fail('invalid or duplicate Debian package ownership record')
            declared[name] = item['version']
    if not installed or installed != declared:
        missing = sorted(set(installed) - set(declared))
        extra = sorted(set(declared) - set(installed))
        changed = sorted(name for name in installed.keys() & declared.keys() if installed[name] != declared[name])
        fail(f'toolchain Debian package closure mismatch: missing={missing}, extra={extra}, version_mismatch={changed}')


def pe(path):
    with path.open('rb') as f:
        if f.read(2) != b'MZ':
            fail(f'not a PE executable: {path}')
        f.seek(60)
        offset = f.read(4)
        if len(offset) != 4:
            fail(f'truncated PE: {path}')
        f.seek(struct.unpack('<I', offset)[0])
        if f.read(6) != b'PE\0\0\x64\x86':
            fail(f'not an x64 PE executable: {path}')


def record(path):
    return {'sha256': sha(path), 'size': path.stat().st_size}


def source_records(lock):
    if lock.get('schema_version') != 1 or lock.get('source_epoch') != EPOCH:
        fail('incorrect FFmpeg input lock schema or source epoch')
    recipe = lock.get('recipe', {})
    if recipe.get('repo') != 'https://github.com/BtbN/FFmpeg-Builds' or recipe.get('commit') != COMMIT:
        fail('incorrect FFmpeg recipe identity')
    source = lock.get('ffmpeg_source', {})
    if (source.get('tag'), source.get('tag_object'), source.get('commit')) != ('n9.0', '5b9252abfe2fda0ccb71ed45cd7a954cc45d2b6f', 'd32b387f2b0a484599d4587d651891f0c63c4238'):
        fail('incorrect FFmpeg source identity')
    files = recipe.get('files')
    inputs = lock.get('inputs')
    if not isinstance(files, list) or not files or not isinstance(inputs, list) or not inputs:
        fail('recipe or source closure inventory missing')
    records = [source.get('bundle'), recipe.get('bundle'), *files]
    recipe_paths = {item.get('path') for item in files if isinstance(item, dict)}
    if not {'recipe/build.sh', 'recipe/download.sh', 'recipe/makeimage.sh', 'recipe/variants/win64-gpl.sh', 'recipe/variants/windows-install-static.sh', 'recipe/variants/defaults-gpl.sh'} <= recipe_paths or not any(isinstance(p, str) and p.startswith('recipe/scripts.d/') for p in recipe_paths):
        fail('required upstream recipe files missing')
    buildx = lock.get('buildx', {})
    if buildx.get('version') != 'v0.38.1' or buildx.get('source_tag') != 'v0.38.1' or not re.fullmatch('[0-9a-f]{40}', str(buildx.get('source_commit', ''))) or buildx.get('source', {}).get('url') != 'https://github.com/docker/buildx.git' or buildx.get('binary', {}).get('sha256') != 'e25d5cd675092bb5d1d03632fdc46cdf3fede7b514dc5f023a105e00d3ce5ad4':
        fail('Buildx provenance missing')
    records.extend((buildx.get('source'), buildx.get('release_metadata')))
    owners = set()
    for item in inputs:
        if not isinstance(item, dict) or any(not isinstance(item.get(k), str) or not item[k] for k in ('owner', 'version', 'recipe')) or item['owner'] in owners:
            fail('invalid source closure owner/version/recipe')
        owners.add(item['owner'])
        if item['recipe'] not in recipe_paths:
            fail(f'{item["owner"]}: missing recipe file')
        records.append(item.get('artifact'))
        for kind in ('sources', 'licenses'):
            values = item.get(kind)
            if not isinstance(values, list) or not values:
                fail(f'{item["owner"]}: {kind} missing')
            for value in values:
                if not isinstance(value, dict) or not isinstance(value.get('url'), str) or not value['url'].startswith('https://'):
                    fail(f'{item["owner"]}: {kind} origin missing')
                records.append(value)
    result = {}
    for item in records:
        if not isinstance(item, dict):
            fail('missing source/recipe file record')
        path = str(relative(item.get('path')))
        if path.casefold() in {'ffmpeg.lock.json', 'replay-build.sh', 'config.log'} or any(old.casefold() == path.casefold() and old != path for old in result):
            fail(f'reserved or case-colliding source/recipe path: {path}')
        digest, size = item.get('sha256'), item.get('size')
        if not isinstance(digest, str) or not re.fullmatch('[0-9a-f]{64}', digest) or type(size) is not int or size <= 0:
            fail(f'invalid source/recipe record: {path}')
        if path in result and result[path] != (size, digest):
            fail(f'conflicting source/recipe record: {path}')
        result[path] = (size, digest)
    if not any(item.get('kind') == 'ffmpeg' and item['version'] == '9.0' and item.get('artifact') == source.get('bundle') for item in inputs):
        fail('FFmpeg source owner/license record missing')
    image = lock.get('toolchain_image', {})
    inventory = image.get('files')
    if image.get('platform') != 'linux/amd64' or not re.fullmatch(r'sha256:[0-9a-f]{64}', str(image.get('image_id', ''))) or image.get('config_digest') != image.get('image_id') or not re.fullmatch(r'sha256:[0-9a-f]{64}', str(image.get('manifest_digest', ''))) or not isinstance(inventory, dict) or not inventory:
        fail('toolchain image identity/ownership inventory missing')
    for path, item in inventory.items():
        relative(path)
        if not isinstance(item, dict) or item.get('owner') not in owners or not re.fullmatch('[0-9a-f]{64}', str(item.get('sha256', ''))) or type(item.get('size')) is not int or item['size'] < 0:
            fail(f'toolchain file has invalid owner/hash/size: {path}')
    docker = lock.get('docker', {})
    if not isinstance(docker.get('version'), str) or not docker['version'] or not isinstance(docker.get('archives'), list) or not docker['archives']:
        fail('Docker provenance missing')
    return result


def validate_output(output, lock_path, lock):
    manifest_file = output / 'manifest.json'
    if manifest_file.is_symlink() or not manifest_file.is_file():
        fail('FFmpeg output manifest missing')
    manifest = json.loads(manifest_file.read_text(encoding='utf-8'))
    machines = {name: '0x8664' for name in ('ffmpeg.exe', 'ffprobe.exe', 'ffplay.exe')}
    expected = {'schema_version': 1, 'helper_arch': 'x86_64', 'input_lock_sha256': sha(lock_path),
                'recipe_commit': COMMIT, 'ffmpeg_commit': lock['ffmpeg_source']['commit'],
                'source_epoch': EPOCH, 'toolchain_image_id': lock['toolchain_image']['image_id'],
                'pe_machine': machines, 'configuration': 'config.log in sources-and-recipes.tar.xz'}
    if any(manifest.get(k) != v for k, v in expected.items()):
        fail('FFmpeg output manifest provenance/configuration mismatch')
    outputs = manifest.get('outputs')
    if not isinstance(outputs, dict) or set(outputs) != {'ffmpeg.zip', 'sources-and-recipes.tar.xz'}:
        fail('FFmpeg output inventory mismatch')
    for name, item in outputs.items():
        if not isinstance(item, dict) or set(item) != {'sha256', 'size'}:
            fail(f'invalid FFmpeg output record: {name}')
        check_file(output, {'path': name, **item})
    with zipfile.ZipFile(output / 'ffmpeg.zip') as archive:
        names = set()
        for item in archive.infolist():
            name = item.filename.rstrip('/')
            relative(name)
            if name.casefold() in names or ((item.external_attr >> 16) & 0o170000) == 0o120000:
                fail(f'unsafe or duplicate FFmpeg ZIP member: {name}')
            names.add(name.casefold())
        for name in machines:
            with archive.open('bin/' + name) as stream:
                f = io.BytesIO(stream.read())
                if f.read(2) != b'MZ':
                    fail(f'not a PE executable: {name}')
                f.seek(60)
                data = f.read(4)
                if len(data) != 4:
                    fail(f'truncated PE executable: {name}')
                f.seek(struct.unpack('<I', data)[0])
                if f.read(6) != bytes.fromhex('504500006486'):
                    fail(f'not an x64 PE executable: {name}')
    expected_sources = source_records(lock)
    expected_sources['ffmpeg.lock.json'] = (lock_path.stat().st_size, sha(lock_path))
    adaptation = lock['recipe']['adaptation']
    expected_sources['replay-build.sh'] = (adaptation['size'], adaptation['sha256'])
    seen = set()
    configuration = None
    with tarfile.open(output / 'sources-and-recipes.tar.xz', 'r:xz') as archive:
        for item in archive:
            name = item.name
            relative(name)
            if name in seen or not item.isfile():
                fail(f'unsafe or duplicate FFmpeg source archive member: {name}')
            seen.add(name)
            stream = archive.extractfile(item)
            if name == 'config.log':
                configuration = stream.read().decode('utf-8', 'replace')
            elif name not in expected_sources or (item.size, hashlib.file_digest(stream, 'sha256').hexdigest()) != expected_sources[name]:
                fail(f'FFmpeg source archive differs from locked input: {name}')
    if seen != set(expected_sources) | {'config.log'}:
        fail('FFmpeg source archive is incomplete')
    if not configuration or '--enable-gpl' not in configuration or '--enable-version3' not in configuration or '--enable-nonfree' in configuration:
        fail('FFmpeg recorded configuration is missing GPL/version3 or enables nonfree')


def main():
    parser = argparse.ArgumentParser()
    parser.add_argument('--lock', type=Path, required=True)
    parser.add_argument('--inputs', type=Path, default=ROOT / 'vendor/windows/ffmpeg-build-inputs')
    parser.add_argument('--out', type=Path)
    parser.add_argument('--capture', action='store_true')
    parser.add_argument('--validate-output', type=Path)
    args = parser.parse_args()
    if bool(args.out) == bool(args.validate_output) or (args.capture and args.validate_output):
        parser.error('choose --out or --validate-output; capture requires --out')
    lock_path = args.lock.absolute()
    if args.validate_output:
        if lock_path.is_symlink() or not lock_path.is_file():
            fail(f'missing captured FFmpeg lock: {lock_path}')
        lock = json.loads(lock_path.read_text(encoding='utf-8'))
        validate_output(args.validate_output, lock_path, lock)
        print(f'Validated FFmpeg input: {args.validate_output}')
        return
    root = args.inputs.resolve(strict=True)
    descriptor = root / 'capture.json' if args.capture else lock_path
    if descriptor.is_symlink() or not descriptor.is_file():
        fail(f'missing captured provenance descriptor: {descriptor}; capture requires the retained recipe, FFmpeg Git bundle, toolchain image, complete package/stage source and license records, image ownership inventory, Docker and Buildx records')
    lock = json.loads(descriptor.read_text(encoding='utf-8'))
    source_records(lock)
    image, archive = validate(lock, root)
    verify_git_sources(lock, root)
    output = args.out.absolute()
    output.parent.mkdir(parents=True, exist_ok=True)
    if output.is_symlink() or (output.exists() and not output.is_dir()):
        fail('output destination must be a directory, not a link')
    with tempfile.TemporaryDirectory(prefix='.ffmpeg-', dir=output.parent) as temp:
        stage = Path(temp)
        work = stage / 'build'
        work.mkdir()
        run('docker', 'load', '--input', archive)
        info = json.loads(run('docker', 'image', 'inspect', image['image_id'], stdout=subprocess.PIPE, text=True).stdout)
        if len(info) != 1 or info[0].get('Id') != image['image_id'] or info[0].get('Os') != 'linux' or info[0].get('Architecture') != 'amd64':
            fail('loaded toolchain image identity/platform mismatch')
        verify_image_files(image)
        verify_packages(image, lock['inputs'])
        if args.capture:
            if lock_path.is_symlink() or (lock_path.exists() and not lock_path.is_file()):
                fail('capture lock destination must be a regular file')
            lock_path.parent.mkdir(parents=True, exist_ok=True)
            with tempfile.NamedTemporaryFile(dir=lock_path.parent, prefix='.ffmpeg-lock-', delete=False) as f:
                temporary_lock = Path(f.name)
                f.write(descriptor.read_bytes())
            try:
                os.replace(temporary_lock, lock_path)
            finally:
                temporary_lock.unlink(missing_ok=True)
        # A single read-only mount contains only the verified source bundle and recipe.
        replay_inputs = stage / 'inputs'
        replay_inputs.mkdir()
        shutil.copy2(root / 'ffmpeg.bundle', replay_inputs / 'ffmpeg.bundle')
        shutil.copytree(root / 'recipe', replay_inputs / 'recipe')
        shutil.copy2(ROOT / 'scripts/windows-ffmpeg-replay-build.sh', replay_inputs / 'replay-build.sh')
        run('docker', 'run', '--rm', '--network=none', '--pull=never', '--mount', f'type=bind,src={replay_inputs},dst=/inputs,readonly', '--mount', f'type=bind,src={work},dst=/out', image['image_id'], 'bash', '/inputs/replay-build.sh')
        package = work / 'package'
        for name in ('ffmpeg.exe', 'ffprobe.exe', 'ffplay.exe'):
            pe(package / 'bin' / name)
        result = stage / 'result'
        result.mkdir()
        with zipfile.ZipFile(result / 'ffmpeg.zip', 'w', zipfile.ZIP_DEFLATED, compresslevel=9) as z:
            for p in sorted(package.rglob('*')):
                if p.is_symlink() or not (p.is_dir() or p.is_file()):
                    fail(f'unsafe package entry: {p}')
                if p.is_file():
                    info = zipfile.ZipInfo(p.relative_to(package).as_posix(), (2026, 8, 3, 0, 0, 0))
                    info.compress_type = zipfile.ZIP_DEFLATED
                    info.external_attr = 0o100644 << 16
                    z.writestr(info, p.read_bytes(), compress_type=zipfile.ZIP_DEFLATED, compresslevel=9)
        # The archive retains every verified input except the image itself.
        with tarfile.open(result / 'sources-and-recipes.tar.xz', 'w:xz') as tar:
            paths = source_records(lock)
            for name in sorted(paths):
                p = root.joinpath(*relative(name).parts)
                entry = tar.gettarinfo(str(p), arcname=name)
                entry.uid = entry.gid = 0
                entry.uname = entry.gname = ''
                entry.mtime = EPOCH
                with p.open('rb') as f:
                    tar.addfile(entry, f)
            for p, name in ((lock_path, 'ffmpeg.lock.json'), (ROOT / 'scripts/windows-ffmpeg-replay-build.sh', 'replay-build.sh'), (work / 'config.log', 'config.log')):
                entry = tar.gettarinfo(str(p), arcname=name)
                entry.uid = entry.gid = 0
                entry.uname = entry.gname = ''
                entry.mtime = EPOCH
                with p.open('rb') as f:
                    tar.addfile(entry, f)
        manifest = {'schema_version': 1, 'helper_arch': 'x86_64', 'input_lock_sha256': sha(lock_path), 'recipe_commit': COMMIT, 'ffmpeg_commit': lock['ffmpeg_source']['commit'], 'source_epoch': EPOCH, 'toolchain_image_id': image['image_id'], 'compiler': 'cross-toolchain in captured image (not MSVC)', 'configuration': 'config.log in sources-and-recipes.tar.xz', 'pe_machine': {'ffmpeg.exe': '0x8664', 'ffprobe.exe': '0x8664', 'ffplay.exe': '0x8664'}, 'outputs': {name: record(result / name) for name in ('ffmpeg.zip', 'sources-and-recipes.tar.xz')}}
        (result / 'manifest.json').write_text(json.dumps(manifest, indent=2) + '\n', encoding='utf-8')
        backup = stage / 'previous'
        if output.exists():
            os.replace(output, backup)
        try:
            os.replace(result, output)
        except BaseException:
            if backup.exists():
                os.replace(backup, output)
            raise
    print(f'Prepared FFmpeg input: {output}')


if __name__ == '__main__':
    try:
        main()
    except (OSError, ValueError, KeyError, TypeError, tarfile.TarError, subprocess.CalledProcessError) as exc:
        print(f'FFmpeg replay: {exc}', file=sys.stderr)
        sys.exit(1)
