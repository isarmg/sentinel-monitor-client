#!/usr/bin/env python3
"""Validate and package current unsigned mobile Release outputs; never sign/publish."""
from __future__ import annotations

import argparse
import hashlib
import json
import plistlib
import re
import shutil
import struct
import subprocess
import tomllib
import zipfile
from pathlib import Path

ROOT = Path(__file__).resolve().parents[2]
APP_ID = "org.sarmg.xcoc"


def require(condition: bool, message: str) -> None:
    if not condition:
        raise ValueError(message)


def version_metadata(root: Path = ROOT) -> tuple[str, int]:
    version = tomllib.loads((root / "Cargo.toml").read_text())["package"]["version"]
    require(re.fullmatch(r"[0-9]+\.[0-9]+\.[0-9]+", version) is not None,
            "Mobile releases require a stable major.minor.patch version")
    major, minor, patch = map(int, version.split("."))
    # Apple's CFBundleVersion permits up to 4/2/2 digits; Android caps versionCode.
    code = major * 1_000_000 + minor * 1_000 + patch
    require(0 < code <= 2_100_000_000 and major <= 9999 and minor <= 99 and patch <= 99,
            "Version cannot be represented by both mobile platforms")
    ffi = tomllib.loads((root / "crates/mobile-ffi/Cargo.toml").read_text())
    require(ffi["package"]["version"] == version, "Mobile FFI package version differs from client")
    android = (root / "clients/android/app/build.gradle.kts").read_text()
    ios = (root / "clients/ios/project.yml").read_text()
    require(re.search(r'versionName\s*=\s*"' + re.escape(version) + r'"', android) is not None,
            "Android versionName differs from Cargo package version")
    match = re.search(r"versionCode\s*=\s*([0-9_]+)", android)
    require(match is not None and int(match[1].replace("_", "")) == code,
            "Android versionCode must be major*1000000 + minor*1000 + patch")
    for setting in ("MARKETING_VERSION", "CURRENT_PROJECT_VERSION"):
        require(re.search(setting + r':\s*"' + re.escape(version) + r'"', ios) is not None,
                f"iOS {setting} differs from Cargo package version")
    return version, code


def check_native_library(data: bytes) -> None:
    require(data[:5] == b"\x7fELF\x02", "Android native library must be ELF64")
    require(len(data) >= 20 and data[5] == 1 and struct.unpack_from("<H", data, 18)[0] == 183,
            "Android native library must be little-endian arm64")


def check_android_zip(path: Path, bundle: bool = False) -> None:
    with zipfile.ZipFile(path) as archive:
        require(archive.testzip() is None, f"Corrupt Android artifact: {path.name}")
        names = archive.namelist()
        prefix = "base/lib/" if bundle else "lib/"
        libraries = {name for name in names if name.startswith(prefix) and name.endswith(".so")}
        require(libraries == {prefix + "arm64-v8a/libxcoc_mobile_ffi.so"},
                f"Unexpected or missing Android native libraries: {sorted(libraries)}")
        with archive.open(next(iter(libraries))) as native:
            check_native_library(native.read(64))
        require(not any(re.fullmatch(r"META-INF/[^/]+\.(RSA|DSA|EC)", name, re.I) for name in names),
                "Expected an unsigned Android artifact, found a JAR signature")
        # APK Signature Scheme v2/v3 places this magic immediately before its central directory.
        with path.open("rb") as raw:
            raw.seek(max(0, archive.start_dir - 16))
            require(raw.read(16) != b"APK Sig Block 42", "Expected an unsigned APK, found a signing block")


def check_android_manifest(text: str, version: str, code: int) -> None:
    package = next((line for line in text.splitlines() if line.startswith("package:")), "")
    fields = dict(re.findall(r"([A-Za-z]+)='([^']*)'", package))
    require(fields.get("name") == APP_ID, "Unexpected Android application ID")
    require(fields.get("versionName") == version and fields.get("versionCode") == str(code),
            "Built APK version differs from release version")
    require("sdkVersion:'26'" in text and "targetSdkVersion:'36'" in text,
            "Unexpected Android SDK support boundary")
    require("application-debuggable" not in text, "A Debug APK cannot be released")


def check_ios_app(app: Path, version: str, platform: str) -> None:
    with (app / "Info.plist").open("rb") as stream:
        info = plistlib.load(stream)
    require(info.get("CFBundleIdentifier") == APP_ID, "Unexpected iOS bundle identifier")
    require(info.get("CFBundleShortVersionString") == version and info.get("CFBundleVersion") == version,
            "Built iOS version differs from release version")
    require(info.get("CFBundleSupportedPlatforms") == [platform], "Wrong iOS device/simulator platform")
    require(info.get("MinimumOSVersion") == "16.0", "Unexpected iOS deployment target")
    executable = info.get("CFBundleExecutable", "")
    require(executable == "XcocCamera" and (app / executable).is_file(), "Missing iOS executable")
    require(not (app / "_CodeSignature").exists() and not (app / "embedded.mobileprovision").exists(),
            "Expected an unsigned iOS application")


def checksum(path: Path) -> str:
    with path.open("rb") as stream:
        return hashlib.file_digest(stream, "sha256").hexdigest()


def write_manifest(output: Path, platform: str, version: str, artifacts: list[Path]) -> None:
    revision = subprocess.check_output(["git", "rev-parse", "HEAD"], cwd=ROOT, text=True).strip()
    require(re.fullmatch(r"[0-9a-f]{40}", revision) is not None, "Missing source revision")
    payload = {
        "schema_version": 1,
        "application_id": APP_ID,
        "version": version,
        "source_revision": revision,
        "configuration": "Release",
        "platform": platform,
        "architecture": "arm64",
        "signed": False,
        "distribution": (
            "Unsigned APK/AAB: requires the owner's release signing key before installation/distribution. "
            "An AAB is not directly installable."
            if platform == "android" else
            "Unsigned device xcarchive: not an installable IPA; requires Apple signing and provisioning. "
            "Simulator app runs only in an arm64 iOS Simulator, not on a device."
        ),
        "artifacts": {path.name: {"sha256": checksum(path), "bytes": path.stat().st_size} for path in artifacts},
    }
    (output / f"xcoc-{platform}-release.json").write_text(json.dumps(payload, indent=2) + "\n")


def package_android(output: Path, aapt2: str) -> None:
    version, code = version_metadata()
    outputs = ROOT / "clients/android/app/build/outputs"
    apk = outputs / "apk/release/app-release-unsigned.apk"
    aab = outputs / "bundle/release/app-release.aab"
    for path, bundle in ((apk, False), (aab, True)):
        require(path.is_file(), f"Missing unsigned Release output: {path}")
        check_android_zip(path, bundle=bundle)
    badging = subprocess.check_output([aapt2, "dump", "badging", str(apk)], text=True)
    check_android_manifest(badging, version, code)
    artifacts = []
    for source, extension in ((apk, "apk"), (aab, "aab")):
        destination = output / f"xcoc-android-arm64-v8a-unsigned.{extension}"
        shutil.copy2(source, destination)
        artifacts.append(destination)
    write_manifest(output, "android", version, artifacts)


def package_ios(output: Path, build: Path) -> None:
    version, _ = version_metadata()
    archive = build / "XcocCamera.xcarchive"
    device_app = archive / "Products/Applications/XcocCamera.app"
    simulator_app = build / "simulator/Build/Products/Release-iphonesimulator/XcocCamera.app"
    for app, platform in ((device_app, "iPhoneOS"), (simulator_app, "iPhoneSimulator")):
        check_ios_app(app, version, platform)
        architectures = subprocess.check_output(["lipo", "-archs", str(app / "XcocCamera")], text=True).strip()
        require(architectures == "arm64", f"Expected only arm64 iOS code, found {architectures}")
    artifacts = []
    for source, name in ((archive, "xcoc-ios-arm64-unsigned.xcarchive.zip"),
                         (simulator_app, "xcoc-ios-simulator-arm64.app.zip")):
        destination = output / name
        # ditto preserves executable modes and archive layout for Xcode/Simulator.
        subprocess.run(["ditto", "-c", "-k", "--sequesterRsrc", "--keepParent", str(source), str(destination)], check=True)
        artifacts.append(destination)
    write_manifest(output, "ios", version, artifacts)


def main() -> None:
    parser = argparse.ArgumentParser(description=__doc__)
    parser.add_argument("platform", choices=["check", "android", "ios"])
    parser.add_argument("--output", type=Path, default=ROOT / "dist")
    parser.add_argument("--aapt2", default="aapt2")
    parser.add_argument("--build", type=Path, default=ROOT / "clients/ios/ReleaseBuild")
    args = parser.parse_args()
    if args.platform == "check":
        version, code = version_metadata()
        print(f"Mobile release version: {version}; Android versionCode: {code}")
        return
    args.output.mkdir(parents=True, exist_ok=True)
    if args.platform == "android":
        package_android(args.output, args.aapt2)
    else:
        package_ios(args.output, args.build)


if __name__ == "__main__":
    main()
