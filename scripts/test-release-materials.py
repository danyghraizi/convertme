#!/usr/bin/env python3
"""Release host names must resolve the existing cargo-about binary pins."""
import importlib.util
import json
from pathlib import Path
import unittest
from unittest.mock import patch

spec = importlib.util.spec_from_file_location('release_materials', Path(__file__).with_name('release-materials.py'))
materials = importlib.util.module_from_spec(spec)
spec.loader.exec_module(materials)


class CargoAboutHost(unittest.TestCase):
    def test_windows_architecture_aliases_resolve_locked_binaries(self):
        pins = json.loads(materials.LOCK.read_text(encoding='utf-8'))['cargo_about']['binaries']
        for machine, key in (
            ('AMD64', 'windows-x86_64'),
            ('x86_64', 'windows-x86_64'),
            ('ARM64', 'windows-arm64'),
            ('aarch64', 'windows-arm64'),
        ):
            with self.subTest(machine=machine), patch.object(materials.os, 'name', 'nt'), patch.object(materials.platform, 'machine', return_value=machine):
                host = materials.cargo_about_host()
                self.assertEqual(host, key)
                self.assertIn(host, pins)

    def test_linux_x64_resolves_existing_pin(self):
        pins = json.loads(materials.LOCK.read_text(encoding='utf-8'))['cargo_about']['binaries']
        with patch.object(materials.os, 'name', 'posix'), patch.object(materials.platform, 'machine', return_value='x86_64'):
            self.assertEqual(materials.cargo_about_host(), 'linux-x86_64')
            self.assertIn(materials.cargo_about_host(), pins)


if __name__ == '__main__':
    unittest.main()
