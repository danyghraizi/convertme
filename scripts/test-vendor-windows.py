#!/usr/bin/env python3
"""Synthetic fail-closed archive policy regressions; no Windows tools required."""
import hashlib
import importlib.util
from pathlib import Path
import subprocess
import tempfile
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location('vendor_windows', Path(__file__).with_name('vendor-windows.py'))
vendor = importlib.util.module_from_spec(spec)
spec.loader.exec_module(vendor)


class ExtractionPolicy(unittest.TestCase):
    def setUp(self):
        self.temp = tempfile.TemporaryDirectory()
        self.addCleanup(self.temp.cleanup)
        self.root = Path(self.temp.name)
        self.original = self.root / 'tools'
        self.original.mkdir()
        (self.original / 'previous').write_bytes(b'unchanged')
        self.variant = [b'generic', b'x64']
        self.names = ['lib/gssetgs.bat', 'lib/gssetgs.bat', 'bin/gs.exe']
        self.extra_field = ''
        self.fail_extract = False
        self.stream_only = False
        self.policy = {'path': 'lib/gssetgs.bat', 'count': 2, 'selected_variant': 'gssetgs64.bat',
                       'variants': [{'source': p, 'size': len(b), 'sha256': hashlib.sha256(b).hexdigest()}
                                    for p, b in zip(['lib/gssetgs.bat', 'lib/gssetgs64.bat'], self.variant)]}

    def run_7z(self, args, **kwargs):
        if args[1] == 'l':
            listing = '\n\n'.join('Path = ' + n + ('\n' + self.extra_field if i == 0 else '') for i, n in enumerate(self.names))
            return subprocess.CompletedProcess(args, 0, listing)
        if self.fail_extract:
            raise subprocess.CalledProcessError(2, args)
        if '-so' in args:
            return subprocess.CompletedProcess(args, 0, b''.join(self.variant))
        if self.stream_only and '-aou' in args:
            raise subprocess.CalledProcessError(2, args)
        out = Path(next(x[2:] for x in args if str(x).startswith('-o')))
        if '-aou' in args:
            for i, content in enumerate(self.variant):
                p = out / 'lib' / ('gssetgs.bat' if i == 0 else 'gssetgs_1.bat')
                p.parent.mkdir(parents=True, exist_ok=True)
                p.write_bytes(content)
        else:
            p = out / 'bin/gs.exe'
            p.parent.mkdir(parents=True, exist_ok=True)
            p.write_bytes(b'exe')
        return subprocess.CompletedProcess(args, 0)

    def extract(self, policy=True):
        with patch.object(vendor, 'run', side_effect=self.run_7z):
            return vendor.extract('7z', 'input', self.root / 'staging', self.policy if policy else None)

    def assert_rejected(self):
        with self.assertRaises((RuntimeError, subprocess.CalledProcessError)):
            self.extract()
        self.assertEqual((self.original / 'previous').read_bytes(), b'unchanged')

    def test_reversed_order_selects_x64(self):
        self.variant.reverse()
        self.extract()
        self.assertEqual((self.root / 'staging/lib/gssetgs.bat').read_bytes(), b'x64')

    def test_stream_fallback_selects_x64(self):
        self.stream_only = True
        self.variant.reverse()
        self.extract()
        self.assertEqual((self.root / 'staging/lib/gssetgs.bat').read_bytes(), b'x64')

    def test_stream_fallback_rejects_changed_bytes(self):
        self.stream_only = True
        self.variant[0] = b'altered'
        self.assert_rejected()

    def test_changed_bytes_rejected(self):
        self.variant[0] = b'corrupt'
        self.assert_rejected()

    def test_unrelated_duplicate_rejected(self):
        self.names.append('BIN/GS.EXE')
        self.assert_rejected()

    def test_third_duplicate_rejected(self):
        self.names.append('lib/gssetgs.bat')
        self.assert_rejected()

    def test_unsafe_rejected(self):
        self.names.append('../outside')
        self.assert_rejected()

    def test_link_rejected(self):
        self.extra_field = 'Symbolic Link = ../outside'
        self.assert_rejected()

    def test_extraction_failure_preserves_previous(self):
        self.fail_extract = True
        self.assert_rejected()

    def test_default_rejects_duplicate(self):
        with self.assertRaises(RuntimeError):
            self.extract(False)


if __name__ == '__main__':
    unittest.main()
