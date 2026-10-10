import importlib.util
import pathlib
import struct
import tempfile
import unittest
from unittest import mock

SCRIPT = pathlib.Path(__file__).resolve().parents[1] / 'check-runtime.py'
spec = importlib.util.spec_from_file_location('runtime_check', SCRIPT)
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


def windows_fixture(import_name, delay=False):
    data = bytearray(2048)
    data[:2] = b'MZ'
    struct.pack_into('<I', data, 0x3C, 0x80)
    data[0x80:0x84] = b'PE\0\0'
    struct.pack_into('<H', data, 0x86, 1)
    struct.pack_into('<H', data, 0x94, 240)
    struct.pack_into('<H', data, 0x98, 0x20B)
    directory = 0x98 + 112
    struct.pack_into('<II', data, directory + (13 if delay else 1) * 8, 0x1000, 64 if delay else 40)
    section = 0x98 + 240
    struct.pack_into('<IIII', data, section + 8, 1024, 0x1000, 1024, 512)
    if delay:
        struct.pack_into('<IIIIIIII', data, 512, 1, 0x1100, 0, 0, 0, 0, 0, 0)
    else:
        struct.pack_into('<IIIII', data, 512, 0, 0, 0, 0x1100, 0)
    name = import_name.encode() + b'\0'
    data[768:768 + len(name)] = name
    return data


class RuntimeTests(unittest.TestCase):
    def test_pe_reads_normal_import(self):
        with tempfile.TemporaryDirectory() as directory:
            binary = pathlib.Path(directory) / 'xcoc.exe'
            binary.write_bytes(windows_fixture('KERNEL32.dll'))
            self.assertEqual(module.pe_imports(binary), ['kernel32.dll'])

    def test_pe_reads_delay_import(self):
        with tempfile.TemporaryDirectory() as directory:
            binary = pathlib.Path(directory) / 'xcoc.exe'
            binary.write_bytes(windows_fixture('avformat-63.dll', delay=True))
            self.assertEqual(module.pe_imports(binary), ['avformat-63.dll'])

    def test_pe_rejects_non_executable(self):
        with tempfile.TemporaryDirectory() as directory:
            binary = pathlib.Path(directory) / 'invalid.exe'
            binary.write_bytes(b'not an executable')
            with self.assertRaisesRegex(ValueError, 'PE executable'):
                module.pe_imports(binary)

    def test_windows_rejects_media_and_runner_crt(self):
        for dependency in ['avformat-63.dll', 'VCRUNTIME140.dll', 'x264.dll', 'custom.dll']:
            with self.subTest(dependency=dependency), mock.patch.object(module.platform, 'system', return_value='Windows'), mock.patch.object(module, 'pe_imports', return_value=[dependency.lower()]), mock.patch.object(module.subprocess, 'run') as run:
                with self.assertRaisesRegex(ValueError, 'Non-system runtime'):
                    module.check(pathlib.Path('xcoc.exe'))
                run.assert_not_called()

    def test_windows_runs_preflight_only_after_system_audit(self):
        with mock.patch.object(module.platform, 'system', return_value='Windows'), mock.patch.object(module, 'pe_imports', return_value=['kernel32.dll', 'api-ms-win-core-synch-l1-2-0.dll']), mock.patch.object(module.subprocess, 'run') as run:
            module.check(pathlib.Path('xcoc.exe'))
            self.assertEqual(run.call_args.args[0][1:], ['media-worker', '--check'])
            self.assertTrue(run.call_args.kwargs['check'])

    def test_linux_rejects_shared_crypto(self):
        with mock.patch.object(module.platform, 'system', return_value='Linux'), mock.patch.object(module.subprocess, 'check_output', return_value=' (NEEDED) Shared library: [libssl.so.3]\n'), mock.patch.object(module.subprocess, 'run') as run:
            with self.assertRaisesRegex(ValueError, 'libssl.so.3'):
                module.check(pathlib.Path('xcoc'))
            run.assert_not_called()

    def test_macos_rejects_homebrew_library(self):
        with mock.patch.object(module.platform, 'system', return_value='Darwin'), mock.patch.object(module.subprocess, 'check_output', return_value='xcoc:\n\t/opt/homebrew/lib/libavformat.63.dylib (compatibility version 63.0.0)\n'), mock.patch.object(module.subprocess, 'run') as run:
            with self.assertRaisesRegex(ValueError, 'libavformat'):
                module.check(pathlib.Path('xcoc'))
            run.assert_not_called()


if __name__ == '__main__':
    unittest.main()
