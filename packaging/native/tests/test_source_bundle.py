import importlib.util
import io
import pathlib
import tarfile
import tempfile
import unittest
from unittest import mock

SCRIPT = pathlib.Path(__file__).resolve().parents[1] / 'package-source.py'
spec = importlib.util.spec_from_file_location('source_bundle', SCRIPT)
module = importlib.util.module_from_spec(spec)
spec.loader.exec_module(module)


def git_archive():
    stream = io.BytesIO()
    with tarfile.open(fileobj=stream, mode='w') as archive:
        entry = tarfile.TarInfo('Cargo.toml')
        data = b'[package]\nname = "fixture"\nversion = "1.0.0"\n'
        entry.size = len(data)
        archive.addfile(entry, io.BytesIO(data))
    return stream.getvalue()


class SourceBundleTests(unittest.TestCase):
    def test_source_archive_is_complete_and_relocatable(self):
        with tempfile.TemporaryDirectory() as directory:
            root = pathlib.Path(directory)
            repo = root / 'repo'
            work = root / 'work'
            output = root / 'output'
            (repo / 'packaging/native/licenses').mkdir(parents=True)
            (repo / 'packaging/native/licenses/COPYING.LGPLv2.1').write_text('license fixture')
            (repo / 'packaging/native/NOTICE.txt').write_text('notice fixture')
            source = work / 'source/ffmpeg-9.0.2'
            source.mkdir(parents=True)
            (source / 'configure').write_text('configure fixture')
            (source / 'config.h').write_text('config fixture')
            (source / 'generated.o').write_text('not source')
            installed = work / 'install'
            (installed / 'include/libavformat').mkdir(parents=True)
            (installed / 'include/libavformat/avformat.h').write_text('header fixture')
            (installed / 'lib/pkgconfig').mkdir(parents=True)
            (installed / 'lib/libavformat.a').write_text('archive fixture')
            (installed / 'lib/pkgconfig/libavformat.pc').write_text('prefix=/old/runner\nlibdir=/old/runner/lib\nincludedir=/old/runner/include\nName: libavformat\n')
            (installed / 'share/xcoc-native').mkdir(parents=True)
            (installed / 'share/xcoc-native/openssl-copyright.txt').write_text('dependency notice')

            def command(args, **kwargs):
                if args[:2] == ['git', 'archive']:
                    return git_archive()
                self.assertEqual(args[:4], ['cargo', 'vendor', '--locked', '--versioned-dirs'])
                vendor = pathlib.Path(args[4])
                (vendor / 'example').mkdir(parents=True)
                (vendor / 'example/Cargo.toml').write_text('vendored source')
                return '[source.vendored-sources]\ndirectory = "C:\\\\runner\\\\vendor"\n'

            with mock.patch.object(module, 'ROOT', repo), mock.patch.object(module.platform, 'system', return_value='Linux'), mock.patch.object(module.subprocess, 'check_output', side_effect=command):
                module.make_bundle(work, 'xcoc-linux-test', output)
            with tarfile.open(output / 'xcoc-linux-test-relink-source.tar.gz') as archive:
                names = set(archive.getnames())
                prefix = 'xcoc-linux-test-relink-source/'
                for expected in ['xcoc/Cargo.toml', 'xcoc/vendor/example/Cargo.toml', 'native/ffmpeg-9.0.2/configure', 'native/ffmpeg-9.0.2/config.h', 'native-libraries/lib/libavformat.a', 'licenses/openssl-copyright.txt', 'SOURCE-BUILD.txt']:
                    self.assertIn(prefix + expected, names)
                self.assertNotIn(prefix + 'native/ffmpeg-9.0.2/generated.o', names)
                config = archive.extractfile(prefix + 'xcoc/.cargo/config.toml').read().decode()
                self.assertIn('directory = "vendor"', config)
                self.assertNotIn('runner', config)
                pc = archive.extractfile(prefix + 'native-libraries/lib/pkgconfig/libavformat.pc').read().decode()
                self.assertIn('prefix=${pcfiledir}/../..', pc)
                self.assertNotIn('/old/runner', pc)

    def test_missing_corresponding_source_fails_closed(self):
        with tempfile.TemporaryDirectory() as directory:
            with self.assertRaises(FileNotFoundError):
                module.make_bundle(pathlib.Path(directory) / 'missing', 'test', pathlib.Path(directory) / 'output')


if __name__ == '__main__':
    unittest.main()
