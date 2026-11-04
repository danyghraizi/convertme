#!/usr/bin/env python3
"""Assemble an AppImage from a DEB and retained, locked deploy inputs."""
import argparse
import hashlib
import json
import os
from pathlib import Path, PurePosixPath
import shutil
import subprocess
import tempfile

ROOT = Path(__file__).resolve().parent.parent
LINUXDEPLOY = 'c20cd71e3a4e3b80c3483cef793cda3f4e990aca14014d23c544ca3ce1270b4d'
APPIMAGETOOL = '62aebc77c2149e96b593b94da9bbd1881e0984a56399ff6d7a03b7ddd7c9ce68'


def digest(path):
    with path.open('rb') as stream:
        return hashlib.file_digest(stream, 'sha256').hexdigest()


def verified(path, expected):
    if not path.is_file() or path.is_symlink() or digest(path) != expected:
        raise RuntimeError(f'missing or corrupt locked AppImage input: {path}')
    return path


def extract_tool(path, work, name):
    directory = work / name
    directory.mkdir()
    executable = directory / 'input.AppImage'
    shutil.copyfile(path, executable)
    executable.chmod(0o755)
    subprocess.run([str(executable), '--appimage-extract'], cwd=directory, check=True, stdout=subprocess.DEVNULL)
    app_run = directory / 'squashfs-root/AppRun'
    if not app_run.is_file():
        raise RuntimeError(f'cannot extract {name}')
    return app_run


def main():
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument('--deb', type=Path, required=True)
    parser.add_argument('--out', type=Path, required=True)
    parser.add_argument('--inputs', type=Path, default=ROOT / 'vendor/linux/inputs')
    args = parser.parse_args()
    lock = json.loads((ROOT / 'packaging/linux/build.lock.json').read_text())
    tools = args.inputs / 'appimage'
    linuxdeploy = verified(tools / 'linuxdeploy-x86_64.AppImage', LINUXDEPLOY)
    appimagetool = verified(tools / 'appimagetool-x86_64.AppImage', APPIMAGETOOL)
    plugin = verified(tools / 'linuxdeploy-plugin-gtk.sh', lock['appimage']['gtk_plugin_sha256'])
    webkit_root = args.inputs / 'webkit-output'
    webkit_manifest = json.loads(verified(webkit_root / 'manifest.json', lock['appimage']['webkit_output_manifest_sha256']).read_text())
    patch = ROOT / 'packaging/linux/patches/webkit-production-process-path.patch'
    if webkit_manifest.get('version') != '2.52.6-0ubuntu0.24.04.1' or webkit_manifest.get('patch_sha256') != digest(patch):
        raise RuntimeError('WebKit output does not match the required source version and patch')
    args.out.parent.mkdir(parents=True, exist_ok=True)
    with tempfile.TemporaryDirectory(prefix='.appimage-', dir=args.out.parent) as temp:
        work = Path(temp)
        appdir = work / 'AppDir'
        subprocess.run(['dpkg-deb', '-x', str(args.deb.resolve()), str(appdir)], check=True)
        if not (appdir / 'usr/bin/convertme').is_file():
            raise RuntimeError('DEB does not contain usr/bin/convertme')
        deploy = extract_tool(linuxdeploy, work, 'linuxdeploy')
        image_tool = extract_tool(appimagetool, work, 'appimagetool')
        plugin_dir = work / 'plugins'
        plugin_dir.mkdir()
        local_plugin = plugin_dir / 'linuxdeploy-plugin-gtk.sh'
        shutil.copyfile(plugin, local_plugin)
        local_plugin.chmod(0o755)
        env = os.environ.copy()
        env['PATH'] = str(plugin_dir) + os.pathsep + env.get('PATH', '')
        env['DEPLOY_GTK_VERSION'] = '3'
        subprocess.run([str(deploy), '--appdir', str(appdir), '--plugin', 'gtk'], check=True, env=env)
        hook = appdir / 'apprun-hooks/linuxdeploy-plugin-gtk.sh'
        hook_text = hook.read_text()
        old = 'export GDK_BACKEND=x11 # Crash with Wayland backend on Wayland\n'
        if hook_text.count(old) != 1:
            raise RuntimeError('unexpected GTK hook; cannot safely preserve display backend')
        hook.write_text(hook_text.replace(old, ''))
        records = webkit_manifest['files']
        if not records:
            raise RuntimeError('empty WebKit output manifest')
        for record in records:
            rel = PurePosixPath(record['path'])
            if rel.is_absolute() or not rel.parts or '..' in rel.parts or '\\' in str(rel):
                raise RuntimeError(f'unsafe WebKit output path: {rel}')
            allowed = ('usr/lib/convertme/gui/', 'usr/lib/x86_64-linux-gnu/webkit2gtk-4.1/')
            if not any(str(rel).startswith(prefix) for prefix in allowed):
                raise RuntimeError(f'WebKit output is outside its permitted trees: {rel}')
            source = verified(webkit_root.joinpath(*rel.parts), record['sha256'])
            target = appdir.joinpath(*rel.parts)
            target.parent.mkdir(parents=True, exist_ok=True)
            shutil.copy2(source, target)
        for required in ('usr/lib/x86_64-linux-gnu/webkit2gtk-4.1/WebKitWebProcess', 'usr/lib/x86_64-linux-gnu/webkit2gtk-4.1/WebKitNetworkProcess', 'usr/lib/x86_64-linux-gnu/webkit2gtk-4.1/injected-bundle', 'usr/lib/convertme/gui'):
            if not (appdir / required).exists():
                raise RuntimeError(f'patched WebKit output missing {required}')
        shutil.copy2(ROOT / 'packaging/linux/AppRun', appdir / 'AppRun')
        (appdir / 'AppRun').chmod(0o755)
        candidate = work / 'ConvertMe.AppImage'
        env['ARCH'] = 'x86_64'
        subprocess.run([str(image_tool), str(appdir), str(candidate)], check=True, env=env)
        os.replace(candidate, args.out)


if __name__ == '__main__':
    main()
