#!/usr/bin/env python3
"""Focused provenance tests for the native Windows Poppler capture."""
import hashlib
import importlib.util
from pathlib import Path
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location('windows_poppler', Path(__file__).with_name('prepare-windows-poppler.py'))
poppler = importlib.util.module_from_spec(spec)
spec.loader.exec_module(poppler)


class PopplerProvenanceTests(unittest.TestCase):
    def test_embedded_feedstock_revision_must_match_the_archive_url(self):
        revision = 'a' * 40
        about = {'extra': {'remote_url': 'https://github.com/conda-forge/poppler-feedstock', 'sha': revision}}
        url = f'https://github.com/conda-forge/poppler-feedstock/archive/{revision}.zip'
        item = {'feedstock_revision': revision, 'feedstock_url': url}
        self.assertTrue(poppler.verify_feedstock_identity('poppler-26.09.0-h924501e_0', about, item, revision, url))
        with self.assertRaises(RuntimeError):
            poppler.verify_feedstock_identity('poppler-26.09.0-h924501e_0', about, item, 'b' * 40, url)

    def test_parent_recipe_is_the_exact_feedstock_template(self):
        data = b'{% set version = "26.09.0" %}\npackage: {name: poppler-split, version: "{{ version }}"}\n'
        record = {'sha256': hashlib.sha256(data).hexdigest(), 'size': len(data)}
        package = {'recipe_files': {'recipe/meta.yaml': {}, 'recipe/parent/meta.yaml': record}}
        class CachedRecord:
            def read_bytes(self):
                return data

        with patch.object(poppler, 'cached', return_value=CachedRecord()):
            selected, name = poppler.recipe_template(package)
        self.assertEqual(name, 'recipe/parent/meta.yaml')
        self.assertEqual(selected, data)

    def test_five_noarch_package_recipe_cases(self):
        cases = [
            ('font-ttf-dejavu-sans-mono-2.37-hab24e00_0', 'font-ttf-dejavu-sans-mono', '2.37', 'https://sourceforge.net/projects/dejavu/files/dejavu/2.37/dejavu-sans-ttf-2.37.zip', '5c6e497a2f36552cb5ffb112c413a6af39c0f3c47653662b90b4fa6499822fd7', 'BSD-3-Clause', 'LICENSE'),
            ('font-ttf-inconsolata-3.000-h77eed37_0', 'font-ttf-inconsolata', '3.000', 'https://github.com/googlefonts/Inconsolata/archive/refs/tags/v3.000.tar.gz', 'f2d8d8ba6caa785a966ef17996822ab14ba91ec423431a29e1b6c4b14bc4cda5', 'OFL-1.1', 'OFL.txt'),
            ('font-ttf-source-code-pro-2.038-h77eed37_0', 'font-ttf-source-code-pro', '2.038', 'https://github.com/adobe-fonts/source-code-pro/archive/refs/tags/2.038R-ro/1.058R-it/1.018R-VAR.tar.gz', 'd8f879fefd66ecb7291ea2d73984b9c0a97fb499424e58290ba6b81787d0c725', 'OFL-1.1', 'LICENSE.md'),
            ('fonts-conda-ecosystem-1-0', 'fonts-conda-ecosystem', '1', None, None, 'BSD-3-Clause', 'LICENSE.txt'),
            ('poppler-data-0.4.12-hd8ed1ab_0', 'poppler-data', '0.4.12', 'https://poppler.freedesktop.org/poppler-data-0.4.12.tar.gz', 'c835b640a40ce357e1b83666aabd95edffa24ddddd49b8daff63adb851cdab74', 'BSD-3-Clause AND (GPL-2.0-only OR GPL-3.0-only)', ['COPYING', 'COPYING.adobe', 'COPYING.gpl2', 'licenses/COPYING.gpl3']),
        ]
        yaml = __import__('yaml')
        for dist, name, version, url, sha256, license_name, license_file in cases:
            with self.subTest(dist=dist):
                feedstock = [
                    f"{{% set version = '{version}' %}}",
                    'package:', f'  name: {name}', '  version: "{{ version }}"',
                    'build:', '  noarch: generic', '  number: 0',
                ]
                package = [
                    'package:', f'  name: {name}', f"  version: '{version}'",
                    'build:', '  noarch: generic', "  number: '0'",
                ]
                if url is not None:
                    source_lines = ['source:', f'  url: {url}', f'  sha256: {sha256}']
                    feedstock += source_lines
                    package += source_lines
                license_yaml = yaml.safe_dump({'license': license_name, 'license_file': license_file}, sort_keys=False).rstrip().splitlines()
                if dist == 'poppler-data-0.4.12-hd8ed1ab_0':
                    license_yaml = yaml.safe_dump({'license': license_name, 'license_file': [*license_file[:-1], 'COPYING.gpl3']}, sort_keys=False).rstrip().splitlines()
                feedstock += ['about:'] + ['  ' + line for line in license_yaml]
                package_license_yaml = license_yaml
                if dist == 'poppler-data-0.4.12-hd8ed1ab_0':
                    package_license_yaml = yaml.safe_dump({'license': license_name, 'license_file': license_file}, sort_keys=False).rstrip().splitlines()
                package += ['about:'] + ['  ' + line for line in package_license_yaml]
                poppler.verify_rendered_recipe(dist, '\n'.join(package).encode(), '\n'.join(feedstock).encode())
                sources = poppler.normalized_sources(yaml.safe_load('\n'.join(package)), dist)
                self.assertEqual(bool(sources), url is not None)

    def test_source_less_allowlist_is_exact(self):
        self.assertEqual(poppler.NO_SOURCE_METAPACKAGES, {'fonts-conda-ecosystem-1-0'})
        with self.assertRaises(RuntimeError):
            poppler.normalized_sources({'package': {'name': 'unexpected'}}, 'unexpected-1-0')

    def test_literal_jinja_filters_used_by_retained_recipes(self):
        rendered = poppler.render_literal_jinja(b"{% set name = 'MiXeD' %}\npackage: {{ name|lower }}\nversion: {{ name|replace('i', '_') }}\n")
        self.assertEqual(rendered, b'\npackage: mixed\nversion: M_XeD\n')


if __name__ == '__main__':
    unittest.main()
