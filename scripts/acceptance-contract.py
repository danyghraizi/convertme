#!/usr/bin/env python3
"""Verify package-bound acceptance evidence and inspect produced artifacts.

Evidence is stored as <cell>/report.json with all referenced paths relative to
that report. The candidate and inventory are supplied separately by the release
inventory verifier; evidence never selects its own package identity or coverage.
"""
from __future__ import annotations

import argparse
import hashlib
import importlib.util
import json
import math
import re
import sys
from datetime import datetime
from pathlib import Path, PurePosixPath

import validate

WINDOWS_SCENARIOS = (
    'build-identity', 'convert-image', 'convert-video', 'convert-audio',
    'compress-scale', 'multi-file', 'multi-file-per-file', 'negative-cancel',
    'negative-overwrite', 'persistence', 'pdf-organize', 'dialogs-images',
    'dialogs-video', 'dialogs-audio', 'dialogs-pdf', 'instance-forward',
    'crop-placement', 'annotation-pixels', 'video-trim', 'audio-trim',
)
WINDOWS_MANUAL = (
    'explorer-drop-and-modifiers', 'audible-trim-playback', 'multi-monitor-dpi',
    'shell-clipboard-autostart', 'offline-install-lifecycle', 'cross-volume-unc',
)
# The same bounded behavior contracts apply to Linux, except Windows-specific
# Explorer, registry and WebView2 lifecycle observations.
LINUX_SCENARIOS = WINDOWS_SCENARIOS
LINUX_MANUAL = ('audible-trim-playback', 'multi-monitor-dpi')
CELLS = {
    'ubuntu2404-x64-deb': ('linux', 'deb', 'x86_64-unknown-linux-gnu'),
    'ubuntu2404-x64-appimage': ('linux', 'appimage', 'x86_64-unknown-linux-gnu'),
    'fedora43-x64-rpm': ('linux', 'rpm', 'x86_64-unknown-linux-gnu'),
    'fedora43-x64-appimage': ('linux', 'appimage', 'x86_64-unknown-linux-gnu'),
    'windows10-x64-nsis': ('windows', 'nsis', 'x86_64-pc-windows-msvc'),
    'windows10-x64-zip': ('windows', 'zip', 'x86_64-pc-windows-msvc'),
    'windows11-x64-nsis': ('windows', 'nsis', 'x86_64-pc-windows-msvc'),
    'windows11-x64-zip': ('windows', 'zip', 'x86_64-pc-windows-msvc'),
    'windows11-arm64-nsis': ('windows', 'nsis', 'aarch64-pc-windows-msvc'),
    'windows11-arm64-zip': ('windows', 'zip', 'aarch64-pc-windows-msvc'),
}


def fail(where, message):
    raise ValueError(f'{where}: {message}')


def digest(path):
    h = hashlib.sha256()
    with path.open('rb') as f:
        for chunk in iter(lambda: f.read(1024 * 1024), b''):
            h.update(chunk)
    return h.hexdigest()


def load(path):
    try:
        return json.loads(path.read_text(encoding='utf-8-sig'))
    except (OSError, ValueError) as error:
        fail(path, f'cannot read JSON: {error}')


def safe_file(root, name):
    if not isinstance(name, str) or not name or '\\' in name:
        fail(root, f'invalid relative path {name!r}')
    parts = PurePosixPath(name)
    if parts.is_absolute() or any(p in ('', '.', '..') for p in name.split('/')) or ':' in name:
        fail(root, f'unsafe relative path {name!r}')
    root = root.resolve()
    path = root.joinpath(*parts.parts)
    current = root
    for part in parts.parts:
        current = current / part
        if current.is_symlink():
            fail(name, 'symlink is not permitted')
    if not path.is_file() or not path.resolve().is_relative_to(root):
        fail(name, 'missing or outside evidence directory')
    return path


def checked_reference(root, ref, where):
    if not isinstance(ref, dict) or set(ref) != {'path', 'sha256'}:
        fail(where, 'expected {path, sha256}')
    path = safe_file(root, ref['path'])
    if not valid_hash(ref['sha256']) or digest(path) != ref['sha256']:
        fail(where, f'hash mismatch for {ref["path"]}')
    return path


def valid_hash(value):
    return isinstance(value, str) and re.fullmatch(r'[0-9a-f]{64}', value) is not None


def nonempty(value):
    return isinstance(value, str) and bool(value.strip())


def records(rows, expected, where, manual=False):
    if not isinstance(rows, list):
        fail(where, 'expected array')
    found = {}
    for row in rows:
        if not isinstance(row, dict) or not isinstance(row.get('id'), str):
            fail(where, 'invalid record')
        name = row['id']
        if name in found or name not in expected:
            fail(where, f'duplicate or unknown ID {name}')
        found[name] = row
    missing = set(expected) - found.keys()
    if missing:
        fail(where, f'missing IDs: {", ".join(sorted(missing))}')
    for name, row in found.items():
        if row.get('status') != 'passed':
            fail(f'{where}/{name}', f'status is {row.get("status")!r}, not passed')
        if manual:
            if not nonempty(row.get('observer')) or not nonempty(row.get('observed_at')):
                fail(f'{where}/{name}', 'missing observer or timestamp')
            try:
                time = datetime.fromisoformat(row['observed_at'].replace('Z', '+00:00'))
                if time.utcoffset() is None:
                    raise ValueError('timezone required')
            except ValueError:
                fail(f'{where}/{name}', 'timestamp must include timezone')
            obs = row.get('observations')
            if not isinstance(obs, dict) or not obs or not any(nonempty(value) for value in obs.values()):
                fail(f'{where}/{name}', 'observations must contain a human description of observed outputs')
        elif not isinstance(row.get('measurements'), dict) or not row['measurements']:
            fail(f'{where}/{name}', 'missing measurements')
        files = row.get('evidence_files')
        if not isinstance(files, list) or not files:
            fail(f'{where}/{name}', 'missing evidence files')
    return found


def verify_cli(root, report, manifest, app_hash, manifest_hash):
    audit = load(checked_reference(root, report.get('cli_report'), 'cli_report'))
    if not isinstance(audit, dict) or audit.get('release') is not True or audit.get('accounting_ok') is not True:
        fail('cli_report', 'not a complete release audit')
    if audit.get('binary_sha256') != app_hash or audit.get('fixtures_manifest_sha256') != manifest_hash:
        fail('cli_report', 'binary or fixture manifest mismatch')
    if audit.get('selectors') != {'only': None, 'no_matrix': False}:
        fail('cli_report', 'filtered audit')
    spec = importlib.util.spec_from_file_location('quality_audit_contract', Path(__file__).with_name('quality-audit.py'))
    module = importlib.util.module_from_spec(spec)
    spec.loader.exec_module(module)
    cases = module.all_cases(True)
    names = [case.name for case in cases]
    if len(set(names)) != len(names) or audit.get('selected_cases') != names:
        fail('cli_report', 'selected cases do not match authoritative full audit')
    rows = audit.get('cases')
    if not isinstance(rows, list) or [r.get('name') for r in rows if isinstance(r, dict)] != names:
        fail('cli_report', 'missing, duplicated or reordered case records')
    probe = audit.get('app_probe', {})
    lines = [line.split() for line in str(probe.get('stdout', '')).splitlines()]
    office_lines = [line for line in lines if line and line[0] == 'soffice']
    office_missing = probe.get('exit_code') == 0 and len(office_lines) == 1 and len(office_lines[0]) >= 2 and office_lines[0][1] == 'missing'
    for case, row in zip(cases, rows):
        where = f'cli_report/{case.name}'
        sources = [case.source] if isinstance(case.source, str) else case.source
        inputs = row.get('input_hashes')
        if not isinstance(inputs, dict) or set(inputs) != set(sources) or any(inputs.get(p) != manifest.get(p) or not valid_hash(manifest.get(p)) for p in sources):
            fail(where, 'input hashes do not match fixture manifest')
        if row.get('status') == 'skipped':
            approval = row.get('skip_approval', {})
            office_case = case.name == 'slides-pptx-to-pdf' or bool(re.fullmatch(r'matrix-(pptx|odp)-to-(pdf|jpg|png)', case.name))
            if not office_case or not office_missing or approval.get('approved') is not True or approval.get('reason') != 'optional_libreoffice_unavailable' or row.get('reason') not in ('missing_tool: LibreOffice is not installed', 'failed: missing_tool: LibreOffice is not installed'):
                fail(where, 'unapproved skip')
        elif row.get('status') != 'passed' or not row.get('checks') or any(c.get('ok') is not True for c in row['checks']):
            fail(where, 'case failed or has no successful checks')


def verify_evidence(candidate_path: Path, evidence_dir: Path, candidate: dict, artifacts_root: Path) -> None:
    candidate_path, evidence_dir, artifacts_root = map(Path, (candidate_path, evidence_dir, artifacts_root))
    candidate_hash = digest(candidate_path)
    manifest_hash = candidate.get('fixtures_manifest_sha256')
    if not valid_hash(manifest_hash):
        fail('candidate', 'missing fixture manifest hash')
    inventories = {}
    for entry in candidate.get('inventories', []):
        path = checked_reference(artifacts_root, entry, 'candidate inventory')
        inv = load(path)
        target = inv.get('target')
        if target in inventories:
            fail('candidate', f'duplicate inventory for {target}')
        inventories[target] = inv
    for cell, (platform, kind, target) in CELLS.items():
        root = evidence_dir / cell
        report = load(safe_file(root, 'report.json'))
        if not isinstance(report, dict) or report.get('schema_version') != 1 or report.get('cell') != cell:
            fail(cell, 'wrong report schema or cell')
        if report.get('candidate_sha256') != candidate_hash or report.get('fixtures_manifest_sha256') != manifest_hash:
            fail(cell, 'candidate or fixture manifest mismatch')
        if report.get('suite') != 'Full' or 'selectors' not in report or report['selectors'] not in ([], {}):
            fail(cell, 'suite is not unfiltered Full')
        if platform == 'windows' and report.get('full_desktop_acceptance') is not True:
            fail(cell, 'full_desktop_acceptance is not true')
        inv = inventories.get(target)
        if not isinstance(inv, dict):
            fail(cell, f'missing target inventory {target}')
        package = report.get('package', {})
        filename = package.get('file')
        matching = [a for a in candidate.get('assets', []) if a.get('target') == target and (a.get('path') == filename or Path(a.get('path', '')).name == filename)]
        if package.get('kind') != kind or len(matching) != 1 or matching[0].get('sha256') != package.get('sha256'):
            fail(cell, 'package does not match candidate')
        asset_name = Path(matching[0]['path']).name.lower()
        suffix = {'nsis': '.exe', 'zip': '.zip', 'deb': '.deb', 'rpm': '.rpm', 'appimage': '.appimage'}[kind]
        if not asset_name.endswith(suffix) or (kind == 'zip' and 'source' in asset_name):
            fail(cell, 'package kind does not match asset')
        if not any(a.get('file') == Path(matching[0]['path']).name and a.get('sha256') == package['sha256'] for a in inv.get('artifacts', [])):
            fail(cell, 'package absent from target inventory')
        app = inv.get('app', {})
        exe = report.get('executable', {})
        if not valid_hash(app.get('sha256')) or not nonempty(app.get('ui_build')) or exe.get('sha256') != app['sha256'] or exe.get('ui_build') != app['ui_build']:
            fail(cell, 'executable or UI build does not match inventory')
        os_info, account = report.get('os', {}), report.get('account', {})
        edition, build, arch = os_info.get('edition'), str(os_info.get('build', '')), os_info.get('native_arch')
        if not nonempty(edition) or 'server' in edition.lower() or not nonempty(build):
            fail(cell, 'missing or server OS edition/build')
        if platform == 'windows':
            if ('10' if cell.startswith('windows10') else '11') not in edition or not build.isdigit() or int(build) < (19045 if cell.startswith('windows10') else 26100) or (cell.startswith('windows10') and int(build) >= 22000):
                fail(cell, 'wrong Windows desktop version/build')
            if arch != ('arm64' if 'arm64' in cell else 'x64') or not re.fullmatch(r'S-1-\d+(?:-\d+)+', str(account.get('sid', ''))):
                fail(cell, 'wrong architecture or missing SID')
        else:
            if arch not in ('x86_64', 'x64') or not isinstance(account.get('uid'), int) or isinstance(account.get('uid'), bool) or account['uid'] <= 0:
                fail(cell, 'wrong architecture or missing standard-user UID')
            required_os = 'ubuntu' if cell.startswith('ubuntu') else 'fedora'
            required_version = '24.04' if required_os == 'ubuntu' else '43'
            if required_os not in edition.lower() or required_version not in edition:
                fail(cell, 'wrong Linux distribution/version')
        if account.get('elevated') is not False or not nonempty(account.get('session')):
            fail(cell, 'elevated or missing interactive session')
        fixture_path = checked_reference(root, report.get('fixtures_manifest'), f'{cell}/fixtures_manifest')
        if digest(fixture_path) != manifest_hash:
            fail(cell, 'fixture manifest does not match candidate')
        manifest = load(fixture_path)
        if not isinstance(manifest, dict) or not manifest or any(not valid_hash(v) for v in manifest.values()):
            fail(cell, 'invalid fixture manifest')
        required = WINDOWS_SCENARIOS if platform == 'windows' else LINUX_SCENARIOS
        manual = WINDOWS_MANUAL if platform == 'windows' else LINUX_MANUAL
        if not isinstance(report.get('required_scenarios'), list) or len(report['required_scenarios']) != len(required) or set(report['required_scenarios']) != set(required):
            fail(cell, 'required_scenarios does not match authoritative table')
        for label, group in (('results', records(report.get('results'), required, f'{cell}/results')), ('manual', records(report.get('manual'), manual, f'{cell}/manual', True))):
            for name, row in group.items():
                for ref in row['evidence_files']:
                    checked_reference(root, ref, f'{cell}/{label}/{name}')
        verify_cli(root, report, manifest, app['sha256'], manifest_hash)


def assert_output(request):
    if not isinstance(request, dict) or set(request) - {'kind', 'path', 'expected', 'source', 'binary'}:
        raise ValueError('invalid request fields')
    kind, path, expected = request.get('kind'), Path(request.get('path', '')), request.get('expected')
    if not path.is_file() or not isinstance(expected, dict) or not expected:
        raise ValueError('path must be a file and expected must be a nonempty object')
    if request.get('binary'):
        validate.configure_tools(Path(request['binary']), strict=True)
    probes = {'image': validate.image, 'media': validate.media, 'pdf': validate.pdf,
              'archive': validate.archive, 'docx': validate.docx, 'odt': validate.odt,
              'pptx': validate.pptx, 'xlsx': validate.xlsx, 'epub': validate.epub}
    if kind == 'ocr':
        result = validate.run_tool(['tesseract', str(path), 'stdout'])
        facts = {'text': result.stdout} if result and result.returncode == 0 else None
    elif kind in probes:
        facts = probes[kind](path)
    else:
        raise ValueError(f'unsupported kind: {kind!r}')
    checks = []
    if facts is None:
        return {'passed': False, 'checks': [{'name': 'readable', 'passed': False}], 'facts': None}
    for key, wanted in expected.items():
        actual = facts.get(key)
        if key == 'pixels':
            if kind != 'image' or not isinstance(wanted, list) or not wanted:
                raise ValueError('pixels requires an image and nonempty array')
            actual = []
            for item in wanted:
                pixel = validate.pixel(path, item['x'], item['y'])
                actual.append(list(pixel) if pixel is not None else None)
            good = all(pixel is not None and all(abs(a - b) <= item.get('tolerance', 0) for a, b in zip(pixel, item['rgb'])) and len(pixel) == len(item['rgb']) for pixel, item in zip(actual, wanted))
        elif key == 'source_pixels':
            if kind != 'image' or not request.get('source') or not isinstance(wanted, list) or not wanted:
                raise ValueError('source_pixels requires image, source and nonempty array')
            source = Path(request['source'])
            actual = []
            good = True
            for item in wanted:
                a = validate.pixel(path, item['x'], item['y'])
                b = validate.pixel(source, item['source_x'], item['source_y'])
                actual.append({'output': a, 'source': b})
                good = good and a is not None and b is not None and all(abs(x-y) <= item.get('tolerance', 0) for x, y in zip(a, b))
        elif key == 'page_texts':
            if kind != 'pdf' or not isinstance(wanted, list) or not wanted:
                raise ValueError('page_texts requires a PDF and nonempty array')
            actual = [validate.pdf_text(path, item['page']) for item in wanted]
            good = all(text is not None and item['contains'] in text for text, item in zip(actual, wanted))
        elif key == 'source_pages':
            if kind != 'pdf' or not request.get('source') or not isinstance(wanted, list) or not wanted:
                raise ValueError('source_pages requires a PDF, source and nonempty array')
            actual = []
            good = True
            for item in wanted:
                output_text = validate.pdf_text(path, item['page'])
                source_text = validate.pdf_text(Path(request['source']), item['source_page'])
                output_text = ' '.join(output_text.split()) if output_text is not None else None
                source_text = ' '.join(source_text.split()) if source_text is not None else None
                actual.append({'output': output_text, 'source': source_text})
                good = good and bool(source_text) and output_text == source_text
        elif key == 'page_rotations':
            if kind != 'pdf' or not isinstance(wanted, list) or not wanted:
                raise ValueError('page_rotations requires a PDF and nonempty array')
            actual = [validate.pdf_page_rotation(path, item['page']) for item in wanted]
            good = all(rotation is not None and rotation == item['rotation'] for rotation, item in zip(actual, wanted))
        elif key == 'text_coverage':
            if not request.get('source') or not isinstance(wanted, (int, float)):
                raise ValueError('text_coverage requires source and numeric minimum')
            source_kind = Path(request['source']).suffix.lower().lstrip('.')
            source_probe = probes.get(source_kind, probes.get(kind))
            source_facts = source_probe(Path(request['source'])) if source_probe and source_kind != 'pdf' else None
            source_text = source_facts.get('text') if source_facts else None
            output_text = facts.get('text')
            if source_kind == 'pdf':
                source_text = validate.pdf_text(Path(request['source']))
            if kind == 'pdf':
                output_text = validate.pdf_text(path)
            actual = validate.text_coverage(source_text, output_text) if source_text is not None and output_text is not None else None
            good = actual is not None and actual >= wanted
        elif isinstance(wanted, dict) and set(wanted) <= {'min', 'max', 'value', 'tolerance', 'contains'} and wanted:
            good = actual is not None
            if 'min' in wanted:
                good = good and actual >= wanted['min']
            if 'max' in wanted:
                good = good and actual <= wanted['max']
            if 'value' in wanted:
                good = good and isinstance(actual, (int, float)) and math.isfinite(actual) and abs(actual - wanted['value']) <= wanted.get('tolerance', 0)
            if 'contains' in wanted:
                good = good and wanted['contains'] in actual
        else:
            good = key in facts and actual == wanted
        checks.append({'name': key, 'passed': bool(good), 'actual': actual, 'expected': wanted})
    return {'passed': all(c['passed'] for c in checks), 'checks': checks, 'facts': facts}


def main():
    parser = argparse.ArgumentParser(description=__doc__.splitlines()[0])
    sub = parser.add_subparsers(dest='command', required=True)
    output = sub.add_parser('assert-output')
    output.add_argument('--request', required=True, type=Path)
    args = parser.parse_args()
    try:
        response = assert_output(load(args.request))
    except (ValueError, OSError, TypeError, KeyError, IndexError) as error:
        response = {'passed': False, 'checks': [{'name': 'request', 'passed': False, 'error': str(error)}], 'facts': None}
    print(json.dumps(response, ensure_ascii=False))
    return 0 if response['passed'] else 1


if __name__ == '__main__':
    sys.exit(main())
