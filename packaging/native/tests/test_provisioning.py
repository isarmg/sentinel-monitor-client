import json
import pathlib
import re
import unittest

ROOT = pathlib.Path(__file__).resolve().parents[1]


class ProvisioningTests(unittest.TestCase):
    def test_unix_source_and_windows_baseline_match_review_record(self):
        sources = json.loads((ROOT / 'sources.json').read_text())
        unix = (ROOT / 'build-unix.sh').read_text()
        windows = (ROOT / 'build-windows.ps1').read_text()
        manifest = json.loads((ROOT / 'vcpkg.json').read_text())
        self.assertEqual(re.search(r'^version=(.+)$', unix, re.MULTILINE).group(1), sources['ffmpeg']['version'])
        self.assertEqual(re.search(r'^sha256=(.+)$', unix, re.MULTILINE).group(1), sources['ffmpeg']['sha256'])
        self.assertIn("$revision = '" + sources['vcpkg']['revision'] + "'", windows)
        self.assertEqual(manifest['builtin-baseline'], sources['vcpkg']['revision'])
        ffmpeg = next(dep for dep in manifest['dependencies'] if dep['name'] == 'ffmpeg')
        self.assertFalse(ffmpeg['default-features'])
        self.assertEqual(ffmpeg['features'], ['avcodec', 'avformat'])
        self.assertIn({'name': 'pkgconf', 'host': True}, manifest['dependencies'])
        self.assertIn('--host-triplet x64-windows-static', windows)
        self.assertIn('$env:PKG_CONFIG_PATH', windows)
        self.assertIn('$env:PKG_CONFIG', windows)
        self.assertIn('-C target-feature=+crt-static', windows)


if __name__ == '__main__':
    unittest.main()
