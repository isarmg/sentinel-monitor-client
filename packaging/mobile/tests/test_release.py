"""Release packaging guards, runnable without Android SDK or Xcode."""
import importlib.util
import plistlib
import struct
import tempfile
import unittest
import zipfile
from pathlib import Path
from unittest.mock import patch

SPEC = importlib.util.spec_from_file_location("mobile_release", Path(__file__).parents[1] / "release.py")
release = importlib.util.module_from_spec(SPEC)
SPEC.loader.exec_module(release)


class ReleaseTests(unittest.TestCase):
    def setUp(self):
        self.temporary = tempfile.TemporaryDirectory()
        self.addCleanup(self.temporary.cleanup)
        self.root = Path(self.temporary.name)

    def native_header(self, architecture=183):
        header = bytearray(64)
        header[:6] = b"\x7fELF\x02\x01"
        struct.pack_into("<H", header, 18, architecture)
        return bytes(header)

    def android_zip(self, bundle=False, architecture=183, signature=False, extra_library=False):
        path = self.root / ("app.aab" if bundle else "app.apk")
        prefix = "base/lib/" if bundle else "lib/"
        with zipfile.ZipFile(path, "w") as archive:
            archive.writestr(prefix + "arm64-v8a/libxcoc_mobile_ffi.so", self.native_header(architecture))
            if extra_library:
                archive.writestr(prefix + "arm64-v8a/libunexpected.so", self.native_header())
            if signature:
                archive.writestr("META-INF/KEY.RSA", "signature")
        return path

    def manifest(self, version="1.0.0", code="1000000", app_id=release.APP_ID):
        return (f"package: name='{app_id}' versionCode='{code}' versionName='{version}'\n"
                "sdkVersion:'26'\ntargetSdkVersion:'36'\n")

    def ios_app(self, platform="iPhoneOS"):
        app = self.root / "XcocCamera.app"
        app.mkdir()
        (app / "XcocCamera").write_bytes(b"fixture executable")
        info = {"CFBundleIdentifier": release.APP_ID, "CFBundleShortVersionString": "1.0.0",
                "CFBundleVersion": "1.0.0", "CFBundleSupportedPlatforms": [platform],
                "MinimumOSVersion": "16.0", "CFBundleExecutable": "XcocCamera"}
        with (app / "Info.plist").open("wb") as stream:
            plistlib.dump(info, stream)
        return app

    def test_current_versions_agree(self):
        version, code = release.version_metadata()
        major, minor, patch_number = map(int, version.split("."))
        self.assertEqual(code, major * 1_000_000 + minor * 1_000 + patch_number)

    def test_version_drift_is_rejected(self):
        original = Path.read_text
        current_version, _ = release.version_metadata()
        def read_text(path, *args, **kwargs):
            value = original(path, *args, **kwargs)
            if path.name == "build.gradle.kts":
                return value.replace(f'versionName = "{current_version}"', 'versionName = "9999.99.99"')
            return value
        with patch.object(Path, "read_text", read_text), self.assertRaisesRegex(ValueError, "versionName"):
            release.version_metadata()

    def test_unsigned_arm64_apk_and_aab(self):
        for bundle in (False, True):
            with self.subTest(bundle=bundle):
                release.check_android_zip(self.android_zip(bundle), bundle=bundle)

    def test_wrong_android_architecture_rejected(self):
        with self.assertRaisesRegex(ValueError, "arm64"):
            release.check_android_zip(self.android_zip(architecture=62))

    def test_missing_android_native_library_rejected(self):
        path = self.root / "empty.apk"
        with zipfile.ZipFile(path, "w"):
            pass
        with self.assertRaisesRegex(ValueError, "native libraries"):
            release.check_android_zip(path)

    def test_unexpected_android_native_dependency_rejected(self):
        with self.assertRaisesRegex(ValueError, "native libraries"):
            release.check_android_zip(self.android_zip(extra_library=True))

    def test_android_signature_rejected(self):
        with self.assertRaisesRegex(ValueError, "signature"):
            release.check_android_zip(self.android_zip(signature=True))

    def test_apk_signing_block_rejected(self):
        path = self.android_zip()
        contents = path.read_bytes()
        central = contents.index(b"PK\x01\x02")
        # Simulate the signing-block magic directly before the central directory.
        contents = contents[:central] + b"APK Sig Block 42" + contents[central:]
        eocd = contents.rfind(b"PK\x05\x06")
        contents = bytearray(contents)
        struct.pack_into("<I", contents, eocd + 16, central + 16)
        path.write_bytes(contents)
        with self.assertRaisesRegex(ValueError, "signing block"):
            release.check_android_zip(path)

    def test_android_built_version_and_identity(self):
        release.check_android_manifest(self.manifest(), "1.0.0", 1000000)
        for text in (self.manifest(version="2.0.0"), self.manifest(code="2"),
                     self.manifest(app_id="wrong.app"), self.manifest() + "application-debuggable\n",
                     self.manifest().replace("sdkVersion:'26'", "sdkVersion:'25'")):
            with self.subTest(text=text), self.assertRaises(ValueError):
                release.check_android_manifest(text, "1.0.0", 1000000)

    def test_ios_unsigned_device_and_simulator(self):
        app = self.ios_app()
        release.check_ios_app(app, "1.0.0", "iPhoneOS")
        with self.assertRaisesRegex(ValueError, "platform"):
            release.check_ios_app(app, "1.0.0", "iPhoneSimulator")

    def test_ios_wrong_version_rejected(self):
        with self.assertRaisesRegex(ValueError, "version"):
            release.check_ios_app(self.ios_app(), "1.0.1", "iPhoneOS")

    def test_ios_signed_app_rejected(self):
        app = self.ios_app()
        (app / "_CodeSignature").mkdir()
        with self.assertRaisesRegex(ValueError, "unsigned"):
            release.check_ios_app(app, "1.0.0", "iPhoneOS")

    def test_ios_provisioned_app_rejected(self):
        app = self.ios_app()
        (app / "embedded.mobileprovision").write_text("fixture")
        with self.assertRaisesRegex(ValueError, "unsigned"):
            release.check_ios_app(app, "1.0.0", "iPhoneOS")

    def test_ios_missing_executable_rejected(self):
        app = self.ios_app()
        (app / "XcocCamera").unlink()
        with self.assertRaisesRegex(ValueError, "executable"):
            release.check_ios_app(app, "1.0.0", "iPhoneOS")

    def test_checksum_records_exact_bytes(self):
        path = self.root / "artifact.apk"
        path.write_bytes(b"abc")
        self.assertEqual(release.checksum(path), "ba7816bf8f01cfea414140de5dae2223b00361a396177a9cb410ff61f20015ad")


if __name__ == "__main__":
    unittest.main()
