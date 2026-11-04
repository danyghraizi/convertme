#!/usr/bin/env python3
"""Capture or replay the native Windows Poppler conda prefix.

Capture requires PyYAML and a source inventory prepared from exact feedstocks:
--sources maps conda distribution names to {feedstock_revision, feedstock_url,
feedstock_archive, artifacts}. File records have path (relative to the
inventory), sha256 and size; archive source artifacts also have url. Git
sources are fetched at their recipe-pinned revisions and retained as bundles.
The feedstock's original recipe must match the template retained by conda-build;
the rendered package recipe supplies exact source URLs, hashes and revisions.

Capture accesses the network; replay only reads the hash-addressed local cache.
"""
import argparse
import ast
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import platform
import re
import shutil
import subprocess
import sys
import tempfile
import urllib.parse
import urllib.request
import zipfile

ROOT = Path(__file__).resolve().parents[1]
LOCK = ROOT / 'packaging/windows/poppler-explicit.lock'
CACHE = ROOT / 'vendor/windows/cache/poppler'
OUT = ROOT / 'vendor/windows/inputs/poppler'
MICROMAMBA_URL = 'https://github.com/mamba-org/micromamba-releases/releases/download/2.10.0-0/micromamba-win-64.exe'
MICROMAMBA_SHA = 'f8b4284bd4eb1134122eaf93b3ea6c7be567e596de5829e1a6f478e34e41974a'
MICROMAMBA_SIZE = 11723264
HEX = re.compile(r'^[0-9a-f]{64}$')
NO_SOURCE_METAPACKAGES = {'fonts-conda-ecosystem-1-0'}


def fail(message):
    raise RuntimeError(message)


def digest(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def relative(value):
    if not isinstance(value, str) or not value or '\\' in value or ':' in value:
        fail(f'unsafe path: {value!r}')
    path = PurePosixPath(value)
    if path.is_absolute() or any(part in ('', '.', '..') for part in value.split('/')):
        fail(f'unsafe path: {value!r}')
    return path


def regular(path):
    if path.is_symlink() or not path.is_file() or getattr(path.stat(), 'st_file_attributes', 0) & 0x400:
        fail(f'not a regular, non-reparse file: {path}')


def checked(path, record):
    regular(path)
    sha = record.get('sha256')
    size = record.get('size')
    if not isinstance(sha, str) or not HEX.fullmatch(sha) or type(size) is not int or size <= 0:
        fail(f'invalid file identity: {path}')
    if path.stat().st_size != size or digest(path) != sha:
        fail(f'file identity mismatch: {path}')


def cached(record):
    path = CACHE / record['sha256']
    checked(path, record)
    return path


def store(path):
    regular(path)
    record = {'sha256': digest(path), 'size': path.stat().st_size}
    if not record['size']:
        fail(f'empty artifact: {path}')
    CACHE.mkdir(parents=True, exist_ok=True)
    dest = CACHE / record['sha256']
    if dest.exists():
        checked(dest, record)
    else:
        with tempfile.NamedTemporaryFile(dir=CACHE, delete=False) as tmp:
            temp = Path(tmp.name)
            with path.open('rb') as src:
                shutil.copyfileobj(src, tmp)
        try:
            checked(temp, record)
            os.replace(temp, dest)
        finally:
            temp.unlink(missing_ok=True)
    return record


def run(args, env):
    subprocess.run([str(x) for x in args], check=True, env=env)


def environment(pkgs):
    env = os.environ.copy()
    for key in list(env):
        if key.startswith(('CONDA_', 'MAMBA_')):
            del env[key]
    env['CONDA_PKGS_DIRS'] = str(pkgs)
    env['MAMBA_NO_BANNER'] = '1'
    return env


def capture_git(dist, url, ref):
    parsed = urllib.parse.urlparse(url)
    if parsed.scheme != 'https' or not parsed.netloc or not isinstance(ref, str) or not ref or ref.startswith('-'):
        fail(f'{dist}: Git source requires HTTPS and an explicit revision')
    env = os.environ.copy()
    env.update(GIT_TERMINAL_PROMPT='0', GIT_CONFIG_NOSYSTEM='1')
    with tempfile.TemporaryDirectory(prefix='poppler-git-') as tmp:
        repo = Path(tmp) / 'repo'
        run(['git', 'init', '--bare', repo], env)
        run(['git', '-C', repo, 'fetch', '--no-tags', '--', url, ref], env)
        revision = subprocess.check_output(['git', '-C', str(repo), 'rev-parse', 'FETCH_HEAD^{commit}'], env=env, text=True).strip()
        if not re.fullmatch(r'[0-9a-f]{40}', revision):
            fail(f'{dist}: Git source did not resolve to a commit')
        tree = subprocess.check_output(['git', '-C', str(repo), 'ls-tree', '-r', revision], env=env, text=True)
        if any(line.startswith('160000 ') for line in tree.splitlines()):
            fail(f'{dist}: Git source {url} contains submodules requiring independent source provenance')
        run(['git', '-C', repo, 'update-ref', 'refs/heads/captured', revision], env)
        bundle = Path(tmp) / 'source.bundle'
        run(['git', '-C', repo, 'bundle', 'create', bundle, 'refs/heads/captured'], env)
        return dict(store(bundle), kind='git', url=url, requested_revision=ref, revision=revision)


def embedded_feedstock(about):
    """Return the conda-forge commit identity embedded by conda-build, if present."""
    extra = about.get('extra') if isinstance(about, dict) else None
    extra = extra if isinstance(extra, dict) else {}
    url, revision = extra.get('remote_url'), extra.get('sha')
    if not url and not revision:
        return None
    parsed = urllib.parse.urlparse(url or '')
    if (not isinstance(revision, str) or not re.fullmatch(r'[0-9a-f]{40}', revision)
            or parsed.scheme != 'https' or parsed.netloc != 'github.com'
            or not re.fullmatch(r'/conda-forge/[A-Za-z0-9_.-]+-feedstock', parsed.path)
            or parsed.query or parsed.fragment):
        fail('package about.json contains incomplete or invalid feedstock identity')
    return {'repository_url': url, 'revision': revision}


def recipe_template(package):
    """Prefer exact retained templates, including conda-build's parent recipe."""
    records = package.get('recipe_files', {})
    for name in ('recipe/meta.yaml.template', 'recipe/parent/meta.yaml'):
        if name in records:
            return cached(records[name]).read_bytes(), name
    return None, None


def render_literal_jinja(recipe):
    """Render the literal version/name variables used by old noarch recipes."""
    text = recipe.decode('utf-8')
    values = {}
    set_pattern = re.compile(r'^\s*\{%\s*set\s+([A-Za-z_][A-Za-z0-9_]*)\s*=\s*(.*?)\s*%\}\s*$', re.M)

    def bind(match):
        try:
            value = ast.literal_eval(match.group(2))
        except (SyntaxError, ValueError):
            fail('historical feedstock recipe uses a non-literal Jinja assignment')
        if not isinstance(value, (str, int, float, bool)):
            fail('historical feedstock recipe uses an unsupported Jinja value')
        values[match.group(1)] = str(value)
        return ''

    text = set_pattern.sub(bind, text)
    expression_pattern = re.compile(r'\{\{\s*([A-Za-z_][A-Za-z0-9_]*)(?:\s*\|\s*(lower)|\s*\|\s*replace\(\s*(\'[^\']*\'|"[^"]*")\s*,\s*(\'[^\']*\'|"[^"]*")\s*\))?\s*\}\}')

    def render_expression(match):
        variable = match.group(1)
        if variable not in values:
            fail('historical feedstock recipe references an unbound Jinja variable')
        value = values[variable]
        if match.group(2) == 'lower':
            value = value.lower()
        elif match.group(3) is not None:
            old, new = ast.literal_eval(match.group(3)), ast.literal_eval(match.group(4))
            value = value.replace(old, new)
        return value

    text = expression_pattern.sub(render_expression, text)
    if '{{' in text or '{%' in text or '%}' in text or '}}' in text:
        fail('historical feedstock recipe has unsupported Jinja expressions')
    return text.encode('utf-8')


def normalized_sources(document, dist):
    source = document.get('source') if isinstance(document, dict) else None
    if isinstance(source, dict):
        source = [source]
    if source is None and dist in NO_SOURCE_METAPACKAGES:
        return []
    if not isinstance(source, list) or not source:
        fail(f'{dist}: recipe has no source declaration')
    result = []
    fields = ('url', 'sha256', 'sha1', 'md5', 'git_url', 'git_rev', 'patches')
    for item in source:
        if not isinstance(item, dict):
            fail(f'{dist}: invalid source declaration')
        result.append({key: item[key] for key in fields if key in item})
    return result


def verify_rendered_recipe(dist, package_recipe, feedstock_recipe):
    """Bind old package metadata to a feedstock whose commit was not embedded."""
    try:
        import yaml
        # BaseLoader keeps version spellings such as 3.000 intact instead of
        # coercing them to floats before comparing package-retained metadata.
        package_doc = yaml.load(package_recipe, Loader=yaml.BaseLoader)
        source_doc = yaml.load(render_literal_jinja(feedstock_recipe), Loader=yaml.BaseLoader)
    except ImportError:
        fail('capture requires PyYAML to compare historical rendered recipes')
    except yaml.YAMLError as exc:
        fail(f'{dist}: invalid package or feedstock recipe: {exc}')
    if not isinstance(package_doc, dict) or not isinstance(source_doc, dict):
        fail(f'{dist}: package/feedstock recipe is not a mapping')
    package = package_doc.get('package', {})
    source_package = source_doc.get('package', {})
    for field in ('name', 'version'):
        package_value, source_value = package.get(field), source_package.get(field)
        if (str(package_value) if package_value is not None else None) != (str(source_value) if source_value is not None else None):
            fail(f'{dist}: package recipe {field} differs from feedstock')
    package_build, source_build = package_doc.get('build', {}), source_doc.get('build', {})
    for field in ('noarch', 'number'):
        package_value, source_value = package_build.get(field), source_build.get(field)
        if (str(package_value) if package_value is not None else None) != (str(source_value) if source_value is not None else None):
            fail(f'{dist}: package recipe build.{field} differs from feedstock')
    if normalized_sources(package_doc, dist) != normalized_sources(source_doc, dist):
        fail(f'{dist}: package source declarations differ from feedstock recipe')
    package_about, source_about = package_doc.get('about', {}), source_doc.get('about', {})
    for field in ('license', 'license_file'):
        package_value, source_value = package_about.get(field), source_about.get(field)
        # conda-build rewrites the final poppler-data license path when it
        # copies license files into its packaged recipe metadata.
        if dist == 'poppler-data-0.4.12-hd8ed1ab_0' and field == 'license_file':
            package_value = [value.removeprefix('licenses/') for value in package_value]
        if package_value != source_value:
            fail(f'{dist}: package recipe about.{field} differs from feedstock')


def verify_feedstock_identity(dist, about, item, revision, feedstock_url):
    identity = embedded_feedstock(about)
    if identity is None:
        return False
    expected_url = f"{identity['repository_url']}/archive/{identity['revision']}.zip"
    if revision != identity['revision'] or feedstock_url != expected_url:
        fail(f'{dist}: inventory feedstock revision differs from package about.json')
    return True


def closure(inventory_path, packages):
    try:
        import yaml
    except ImportError:
        fail('capture requires PyYAML to read rendered conda recipes')
    inventory = json.loads(inventory_path.read_text(encoding='utf-8'))
    if not isinstance(inventory, dict) or set(inventory) != set(packages):
        fail('source inventory must cover exactly the solved distributions')
    result = {}
    for dist, package in packages.items():
        item = inventory[dist]
        if not isinstance(item, dict):
            fail(f'{dist}: source inventory entry must be a mapping')
        revision = item.get('feedstock_revision')
        if not isinstance(revision, str) or not re.fullmatch(r'[0-9a-f]{40}', revision):
            fail(f'{dist}: missing exact feedstock revision')
        recipe = package['recipe']
        feedstock_url = item.get('feedstock_url', '')
        parsed = urllib.parse.urlparse(feedstock_url)
        if (parsed.scheme != 'https' or parsed.netloc != 'github.com'
                or not re.fullmatch(r'/conda-forge/[A-Za-z0-9_.-]+-feedstock/archive/' + revision + r'\.zip', parsed.path)
                or parsed.query or parsed.fragment):
            fail(f'{dist}: invalid conda-forge feedstock revision URL')
        archive = item.get('feedstock_archive', {})
        archive_path = inventory_path.parent.joinpath(*relative(archive.get('path')).parts)
        checked(archive_path, archive)
        about = json.loads(cached(package['metadata']['about.json']).read_text(encoding='utf-8'))
        identity_embedded = verify_feedstock_identity(dist, about, item, revision, feedstock_url)
        with urllib.request.urlopen(feedstock_url) as response:
            remote_hash = hashlib.sha256()
            remote_size = 0
            while chunk := response.read(1024 * 1024):
                remote_hash.update(chunk)
                remote_size += len(chunk)
        if remote_size != archive['size'] or remote_hash.hexdigest() != archive['sha256']:
            fail(f'{dist}: feedstock archive differs from the official revision')
        with zipfile.ZipFile(archive_path) as zf:
            matches = [n for n in zf.namelist() if n.endswith('/recipe/meta.yaml')]
            template_bytes, template_name = recipe_template(package)
            if len(matches) != 1:
                fail(f'{dist}: feedstock archive must contain exactly one recipe/meta.yaml')
            feedstock_recipe = zf.read(matches[0])
            if template_bytes is not None:
                if feedstock_recipe != template_bytes:
                    fail(f'{dist}: retained recipe template differs from feedstock')
            elif not identity_embedded:
                verify_rendered_recipe(dist, recipe, feedstock_recipe)
            if not matches[0].split('/')[0].endswith(revision):
                fail(f'{dist}: feedstock archive is not identified by its revision')
            feedstock_prefix = matches[0][:-len('meta.yaml')]
            feedstock_files = set(zf.namelist())
            for name, record in package['recipe_files'].items():
                suffix = name.removeprefix('recipe/')
                candidate = feedstock_prefix + suffix
                if name in ('recipe/meta.yaml', 'recipe/meta.yaml.template', 'recipe/parent/meta.yaml'):
                    continue
                # conda-build stores a merged platform configuration in package
                # metadata; the feedstock archive itself remains hash-locked.
                if name == 'recipe/conda_build_config.yaml' and identity_embedded:
                    continue
                if candidate in feedstock_files:
                    packaged_bytes = cached(record).read_bytes()
                    source_bytes = zf.read(candidate)
                    if Path(name).suffix.lower() in ('.bat', '.sh', '.yaml', '.patch'):
                        packaged_bytes = packaged_bytes.replace(b'\r\n', b'\n')
                        source_bytes = source_bytes.replace(b'\r\n', b'\n')
                    if hashlib.sha256(source_bytes).hexdigest() != hashlib.sha256(packaged_bytes).hexdigest():
                        fail(f'{dist}: packaged recipe file differs from feedstock: {suffix}')
        try:
            rendered = yaml.safe_load(recipe)
        except yaml.YAMLError as exc:
            fail(f'{dist}: invalid rendered recipe: {exc}')
        declarations = rendered.get('source') if isinstance(rendered, dict) else None
        if isinstance(declarations, dict):
            declarations = [declarations]
        no_source = declarations is None and dist in NO_SOURCE_METAPACKAGES
        if no_source:
            source_doc = yaml.safe_load(render_literal_jinja(template_bytes if template_bytes is not None else feedstock_recipe))
            if source_doc.get('source') is not None:
                fail(f'{dist}: allowlisted metapackage unexpectedly has a feedstock source')
            verify_rendered_recipe(dist, recipe, template_bytes if template_bytes is not None else feedstock_recipe)
            declarations = []
        if not isinstance(declarations, list) or (not declarations and not no_source):
            fail(f'{dist}: rendered recipe has no source declarations')
        if not isinstance(item, dict) or 'artifacts' not in item:
            fail(f'{dist}: source inventory must explicitly record artifacts')
        artifacts = item['artifacts']
        if not isinstance(artifacts, list) or (no_source and artifacts):
            fail(f'{dist}: invalid source artifact inventory')
        unused = list(artifacts)
        sources = []
        for source in declarations:
            if not isinstance(source, dict):
                fail(f'{dist}: invalid source declaration')
            for patch in source.get('patches', []):
                patch_path = relative(patch).as_posix()
                keys = ('recipe/' + patch_path, 'recipe/parent/' + patch_path)
                key = next((candidate for candidate in keys if candidate in package['recipe_files']), None)
                if key is None:
                    fail(f'{dist}: missing recipe patch {patch}')
                source_patch = feedstock_prefix + patch_path
                if source_patch not in feedstock_files:
                    fail(f'{dist}: recipe patch is absent from feedstock revision: {patch}')
                packaged_bytes = cached(package['recipe_files'][key]).read_bytes()
                archive_bytes = zf.read(source_patch)
                if packaged_bytes.replace(b'\r\n', b'\n') != archive_bytes.replace(b'\r\n', b'\n'):
                    fail(f'{dist}: retained recipe patch differs from feedstock: {patch}')
            if 'git_url' in source:
                sources.append(capture_git(dist, source['git_url'], source.get('git_rev')))
                continue
            urls = source.get('url')
            if isinstance(urls, str):
                urls = [urls]
            checks = {alg: str(source[alg]).lower() for alg in ('sha256', 'sha1', 'md5') if source.get(alg)}
            if not isinstance(urls, list) or not urls or not checks:
                fail(f'{dist}: source declaration lacks an archive URL and checksum: {source}')
            candidates = [entry for entry in unused if entry.get('url') in urls]
            if len(candidates) != 1:
                fail(f'{dist}: expected exactly one retained source for {urls}')
            entry = candidates[0]
            if not entry['url'].startswith('https://'):
                fail(f"{dist}: source URL must use HTTPS: {entry['url']}")
            path = inventory_path.parent.joinpath(*relative(entry.get('path')).parts)
            checked(path, entry)
            for algorithm, expected in checks.items():
                with path.open('rb') as stream:
                    actual = hashlib.file_digest(stream, algorithm).hexdigest()
                if actual != expected:
                    fail(f"{dist}: {algorithm} mismatch for {entry['url']}")
            unused.remove(entry)
            sources.append(dict(store(path), kind='archive', url=entry['url']))
        if unused:
            fail(f'{dist}: source inventory contains unreferenced artifacts')
        source_record = {'feedstock_revision': revision, 'feedstock_url': feedstock_url,
                         'feedstock_archive': store(archive_path), 'artifacts': sources}
        if no_source:
            source_record['no_source'] = True
        result[dist] = source_record
    return result


def collect(prefix, pkgs):
    meta = prefix / 'conda-meta'
    packages = {}
    owners = {}
    for path in sorted(meta.glob('*.json')):
        record = json.loads(path.read_text(encoding='utf-8'))
        dist = f"{record['name']}-{record['version']}-{record['build']}"
        if dist in packages:
            fail(f'duplicate package: {dist}')
        url = record.get('url', '')
        parsed = urllib.parse.urlparse(url)
        if parsed.scheme != 'https' or parsed.netloc != 'conda.anaconda.org' or not parsed.path.startswith(('/conda-forge/win-64/', '/conda-forge/noarch/')):
            fail(f'{dist}: not a conda-forge win-64/noarch package: {url}')
        filename = Path(parsed.path).name
        archive = pkgs / filename
        info = pkgs / dist / 'info'
        files = record.get('files')
        if not isinstance(files, list) or not files:
            fail(f'{dist}: no owned files')
        for name in files:
            rel = relative(name).as_posix()
            folded = rel.casefold()
            if folded in owners:
                fail(f'file ownership conflict: {rel} and {owners[folded][0]}')
            owners[folded] = (rel, dist)
        metadata = {}
        for name in ('index.json', 'paths.json', 'about.json', 'recipe/meta.yaml'):
            source = info.joinpath(*name.split('/'))
            regular(source)
            metadata[name] = store(source)
        licenses = [p for p in (info / 'licenses').rglob('*') if p.is_file()] if (info / 'licenses').is_dir() else []
        if not licenses:
            fail(f'{dist}: package has no retained license')
        license_records = {p.relative_to(info).as_posix(): store(p) for p in licenses}
        recipe = (info / 'recipe/meta.yaml').read_bytes()
        recipe_files = {p.relative_to(info).as_posix(): store(p) for p in (info / 'recipe').rglob('*') if p.is_file()}
        packages[dist] = {'url': url, 'archive': store(archive), 'metadata': metadata,
                          'licenses': license_records, 'recipe_files': recipe_files, 'recipe': recipe}
    if not packages or not any(d.startswith('poppler-26.09.0-h924501e_0') for d in packages):
        fail('solved environment does not contain poppler=26.09.0=h924501e_0')
    return packages, owners


def inventory(prefix, owners):
    files = {}
    seen = set()
    for path in prefix.rglob('*'):
        if path.is_symlink() or getattr(path.stat(), 'st_file_attributes', 0) & 0x400:
            fail(f'reparse point in prefix: {path}')
        if path.is_dir():
            continue
        regular(path)
        name = path.relative_to(prefix).as_posix()
        key = name.casefold()
        if key in seen or key not in owners or owners[key][0] != name:
            fail(f'colliding or unowned prefix file: {name}')
        seen.add(key)
        files[name] = digest(path)
    if seen != set(owners):
        fail(f'package-owned files missing from prefix: {sorted(set(owners) - seen)}')
    return files, {name: owners[name.casefold()][1] for name in files}


def load_lock():
    regular(LOCK)
    lock = json.loads(LOCK.read_text(encoding='utf-8'))
    bootstrap = {'sha256': MICROMAMBA_SHA, 'size': MICROMAMBA_SIZE}
    packages = lock.get('packages')
    if lock.get('schema_version') != 1 or lock.get('helper_arch') != 'x86_64' or lock.get('micromamba') != bootstrap or not isinstance(packages, dict) or not packages:
        fail('invalid Poppler lock')
    cached(bootstrap)
    if 'poppler-26.09.0-h924501e_0' not in packages:
        fail('lock does not contain the requested Poppler build')
    for dist, package in packages.items():
        cached(package['archive'])
        for group in ('metadata', 'licenses', 'recipe_files'):
            records = package.get(group)
            if not isinstance(records, dict) or not records:
                fail(f'{dist}: missing {group}')
            for name, record in records.items():
                relative(name)
                cached(record)
        metadata = package['metadata']
        if not {'index.json', 'paths.json', 'about.json', 'recipe/meta.yaml'} <= set(metadata):
            fail(f'{dist}: incomplete package metadata')
        index = json.loads(cached(metadata['index.json']).read_text(encoding='utf-8'))
        if f"{index['name']}-{index['version']}-{index['build']}" != dist:
            fail(f'{dist}: index identity mismatch')
        source = package.get('source', {})
        about = json.loads(cached(metadata['about.json']).read_text(encoding='utf-8'))
        verify_feedstock_identity(dist, about, source, source.get('feedstock_revision'), source.get('feedstock_url'))
        no_source = dist in NO_SOURCE_METAPACKAGES
        if not re.fullmatch(r'[0-9a-f]{40}', source.get('feedstock_revision', '')) or not source.get('feedstock_url'):
            fail(f'{dist}: missing source provenance')
        if no_source:
            if source.get('no_source') is not True or source.get('artifacts') != []:
                fail(f'{dist}: source-less package lacks its explicit no-source record')
        elif source.get('no_source') is not None or not source.get('artifacts'):
            fail(f'{dist}: missing retained source artifact')
        cached(source['feedstock_archive'])
        for record in source['artifacts']:
            if record.get('kind') not in ('git', 'archive') or not record.get('url'):
                fail(f'{dist}: invalid source artifact')
            if record['kind'] == 'git' and not re.fullmatch(r'[0-9a-f]{40}', record.get('revision', '')):
                fail(f'{dist}: missing Git source revision')
            cached(record)
        url = urllib.parse.urlparse(package['url'])
        if url.scheme != 'https' or url.netloc != 'conda.anaconda.org' or not url.path.startswith(('/conda-forge/win-64/', '/conda-forge/noarch/')) or Path(url.path).name not in (dist + '.conda', dist + '.tar.bz2'):
            fail(f'{dist}: invalid package URL')
    return lock


def validate_payload(root):
    lock = load_lock()
    if root.is_symlink() or not root.is_dir() or getattr(root.stat(), 'st_file_attributes', 0) & 0x400:
        fail(f'invalid prepared input directory: {root}')
    if {p.name for p in root.iterdir()} != {'manifest.json', 'prefix'}:
        fail('prepared input contains unexpected files')
    manifest_path = root / 'manifest.json'
    regular(manifest_path)
    manifest = json.loads(manifest_path.read_text(encoding='utf-8'))
    if manifest.get('schema_version') != 1 or manifest.get('helper_arch') != 'x86_64' or manifest.get('input_lock_sha256') != digest(LOCK):
        fail('prepared input manifest does not match lock')
    files, declared = manifest.get('files'), manifest.get('owners')
    if not isinstance(files, dict) or not files or not isinstance(declared, dict) or set(files) != set(declared):
        fail('invalid prepared file ownership inventory')
    allowed = {}
    for dist, package in lock['packages'].items():
        paths = json.loads(cached(package['metadata']['paths.json']).read_text(encoding='utf-8'))
        if not isinstance(paths.get('paths'), list):
            fail(f'{dist}: invalid paths metadata')
        for record in paths['paths']:
            if record.get('path_type') == 'directory':
                continue
            name = relative(record['_path']).as_posix()
            key = name.casefold()
            if key in allowed:
                fail(f'package path ownership conflict: {name}')
            allowed[key] = (name, dist, record)
    owners = {}
    for name, owner in declared.items():
        key = relative(name).as_posix().casefold()
        if key in owners or key not in allowed or allowed[key][:2] != (name, owner):
            fail(f'file has invalid package ownership: {name}')
        owners[key] = (name, owner)
    if set(owners) != set(allowed):
        fail('prepared prefix omits package-owned paths')
    prefix = root / 'prefix'
    if prefix.is_symlink() or not prefix.is_dir() or getattr(prefix.stat(), 'st_file_attributes', 0) & 0x400:
        fail('invalid prepared prefix')
    actual, actual_owners = inventory(prefix, owners)
    if actual != files or actual_owners != declared:
        fail('prepared prefix differs from its manifest')
    for name, sha in actual.items():
        record = allowed[name.casefold()][2]
        if record.get('sha256') and not record.get('prefix_placeholder') and sha != record['sha256']:
            fail(f'prepared file differs from package bytes: {name}')
    for exe in ('Library/bin/pdftoppm.exe', 'Library/bin/pdftotext.exe', 'Library/bin/pdfinfo.exe'):
        if exe not in actual:
            fail(f'missing Poppler executable: {exe}')


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--capture-lock', action='store_true')
    parser.add_argument('--sources', type=Path, help='capture source-closure inventory')
    parser.add_argument('--validate-payload', type=Path, help='verify an existing prepared input without modifying it')
    args = parser.parse_args()
    if args.validate_payload is not None:
        if args.capture_lock or args.sources:
            fail('--validate-payload cannot be combined with capture options')
        validate_payload(args.validate_payload)
        return
    if os.name != 'nt' or platform.machine().lower() not in ('amd64', 'x86_64'):
        fail('native Windows x64 is required to create the Poppler prefix')
    if args.capture_lock != (args.sources is not None):
        fail('--sources is required only with --capture-lock')
    CACHE.mkdir(parents=True, exist_ok=True)
    executable = CACHE / MICROMAMBA_SHA
    bootstrap = {'sha256': MICROMAMBA_SHA, 'size': MICROMAMBA_SIZE}
    if args.capture_lock and not executable.exists():
        with tempfile.NamedTemporaryFile(dir=CACHE, delete=False) as f:
            temp = Path(f.name)
            try:
                with urllib.request.urlopen(MICROMAMBA_URL) as response:
                    shutil.copyfileobj(response, f)
            except BaseException:
                temp.unlink(missing_ok=True)
                raise
        try:
            checked(temp, bootstrap)
            os.replace(temp, executable)
        finally:
            temp.unlink(missing_ok=True)
    checked(executable, bootstrap)
    if args.capture_lock and LOCK.exists():
        fail(f'refusing to replace existing lock: {LOCK}')
    OUT.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='.poppler-', dir=OUT.parent) as tmp:
        work = Path(tmp)
        verified_executable = work / 'micromamba.exe'
        shutil.copyfile(executable, verified_executable)
        checked(verified_executable, bootstrap)
        executable = verified_executable
        prefix, pkgs = work / 'prefix', work / 'pkgs'
        pkgs.mkdir()
        env = environment(pkgs)
        if args.capture_lock:
            run([executable, 'create', '-y', '-p', prefix, '--override-channels', '-c', 'conda-forge', '--strict-channel-priority', 'poppler=26.09.0=h924501e_0'], env)
            packages, owners = collect(prefix, pkgs)
            source_records = closure(args.sources.resolve(strict=True), packages)
            for dist, package in packages.items():
                del package['recipe']
                package['source'] = source_records[dist]
            lock = {'schema_version': 1, 'helper_arch': 'x86_64', 'micromamba': bootstrap, 'packages': packages}
        else:
            lock = load_lock()
            lines = ['@EXPLICIT']
            for dist, package in sorted(lock['packages'].items()):
                archive = cached(package['archive'])
                # File names preserve conda's distribution identification; bytes are hash-verified first.
                filename = Path(urllib.parse.urlparse(package['url']).path).name
                if not filename or filename not in (dist + '.conda', dist + '.tar.bz2'):
                    fail(f'{dist}: invalid archive URL')
                local = pkgs / filename
                shutil.copyfile(archive, local)
                with local.open('rb') as stream:
                    md5 = hashlib.file_digest(stream, 'md5').hexdigest()
                lines.append(local.as_uri() + '#' + md5)
            explicit = work / 'explicit.txt'
            explicit.write_text('\n'.join(lines) + '\n', encoding='utf-8')
            run([executable, 'create', '-y', '-p', prefix, '--offline', '--file', explicit], env)
            # Installed records refer to local explicit URLs; determine ownership directly.
            owners = {}
            installed = set()
            for path in (prefix / 'conda-meta').glob('*.json'):
                record = json.loads(path.read_text(encoding='utf-8'))
                dist = f"{record['name']}-{record['version']}-{record['build']}"
                if dist not in lock['packages'] or dist in installed:
                    fail(f'unexpected installed package: {dist}')
                installed.add(dist)
                for name in record.get('files', []):
                    name = relative(name).as_posix()
                    if name.casefold() in owners:
                        fail(f'file ownership conflict: {name}')
                    owners[name.casefold()] = (name, dist)
            if installed != set(lock['packages']):
                fail('installed package set differs from lock')
        shutil.rmtree(prefix / 'conda-meta')
        for path in prefix.rglob('*'):
            if not path.is_file():
                continue
            regular(path)
            forms = {str(prefix), prefix.as_posix()}
            needles = [form.encode(encoding) for form in forms for encoding in ('utf-8', 'utf-16le')]
            overlap = max(map(len, needles)) - 1
            previous = b''
            with path.open('rb') as stream:
                while chunk := stream.read(1024 * 1024):
                    data = previous + chunk
                    if any(needle in data for needle in needles):
                        fail(f'non-relocatable conda prefix reference in {path.relative_to(prefix)}')
                    previous = data[-overlap:]
        files, ownership = inventory(prefix, owners)
        for exe in ('Library/bin/pdftoppm.exe', 'Library/bin/pdftotext.exe', 'Library/bin/pdfinfo.exe'):
            if exe not in files:
                fail(f'missing Poppler executable: {exe}')
        if OUT.exists():
            fail(f'refusing to replace existing prepared input: {OUT}')
        if args.capture_lock:
            LOCK.parent.mkdir(parents=True, exist_ok=True)
            # Exclusive creation prevents accidental replacement of a previously captured lock.
            with LOCK.open('x', encoding='utf-8', newline='\n') as f:
                json.dump(lock, f, indent=2, sort_keys=True)
                f.write('\n')
        stage = work / 'output'
        stage.mkdir()
        shutil.move(prefix, stage / 'prefix')
        manifest = {'schema_version': 1, 'helper_arch': 'x86_64', 'input_lock_sha256': digest(LOCK), 'files': files, 'owners': ownership}
        (stage / 'manifest.json').write_text(json.dumps(manifest, indent=2, sort_keys=True) + '\n', encoding='utf-8')
        try:
            validate_payload(stage)
        except BaseException:
            if args.capture_lock:
                LOCK.unlink(missing_ok=True)
            raise
        os.replace(stage, OUT)


if __name__ == '__main__':
    try:
        main()
    except (RuntimeError, OSError, ValueError, KeyError, subprocess.CalledProcessError, zipfile.BadZipFile) as exc:
        sys.exit(f'Poppler preparation failed: {exc}')
