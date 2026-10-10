#!/usr/bin/env python3
"""Validate and package current unsigned mobile Release outputs; never sign/publish."""
from __future__ import annotations

import argparse
import hashlib
import json
import os
import plistlib
import re
import shutil
import stat
import struct
import subprocess
import tempfile
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
    # Android build-tools 36 aapt2 names the minimum field minSdkVersion.
    # Parse complete fields, never substrings in labels or other manifest data.
    sdk_fields = {"minSdkVersion": [], "targetSdkVersion": []}
    for line in text.splitlines():
        match = re.fullmatch(r"\s*(minSdkVersion|targetSdkVersion):\s*'([^']*)'\s*", line)
        if match:
            sdk_fields[match[1]].append(match[2])
        elif re.match(r"\s*(minSdkVersion|targetSdkVersion)\s*:", line):
            raise ValueError("Malformed Android SDK field; expected exactly 26/36")
    require(sdk_fields == {"minSdkVersion": ["26"], "targetSdkVersion": ["36"]},
            "Unexpected Android SDK support boundary: "
            f"minSdkVersion={sdk_fields['minSdkVersion']!r}, "
            f"targetSdkVersion={sdk_fields['targetSdkVersion']!r}; expected exactly 26/36")
    require("application-debuggable" not in text, "A Debug APK cannot be released")


def check_ios_info(info: dict, version: str, platform: str) -> None:
    require(info.get("CFBundleIdentifier") == APP_ID, "Unexpected iOS bundle identifier")
    require(info.get("CFBundleShortVersionString") == version and info.get("CFBundleVersion") == version,
            "Built iOS version differs from release version")
    require(info.get("CFBundleSupportedPlatforms") == [platform], "Wrong iOS device/simulator platform")
    require(info.get("MinimumOSVersion") == "16.0", "Unexpected iOS deployment target")
    require(info.get("CFBundleExecutable") == "XcocCamera", "Unexpected iOS executable name")


def check_ios_app(app: Path, version: str, platform: str) -> None:
    with (app / "Info.plist").open("rb") as stream:
        check_ios_info(plistlib.load(stream), version, platform)
    require((app / "XcocCamera").is_file(), "Missing iOS executable")
    require(not (app / "_CodeSignature").exists() and not (app / "embedded.mobileprovision").exists(),
            "Expected an unsigned iOS application")



def check_ios_binary(data: bytes) -> None:
    require(len(data) >= 16 and data[:4] == b"\xcf\xfa\xed\xfe" and
            struct.unpack_from("<I", data, 4)[0] == 0x0100000C and
            struct.unpack_from("<I", data, 12)[0] == 2,
            "IPA executable must be an arm64 Mach-O executable")


def check_ios_ipa(path: Path, version: str) -> None:
    prefix = "Payload/XcocCamera.app/"
    with zipfile.ZipFile(path) as archive:
        require(archive.testzip() is None, "IPA ZIP integrity check failed")
        names = archive.namelist()
        require(len(names) == len(set(names)), "IPA contains duplicate entries")
        require(all(name.startswith(prefix) and ".." not in Path(name).parts for name in names),
                "IPA must contain only Payload/XcocCamera.app")
        require({prefix + "Info.plist", prefix + "XcocCamera"}.issubset(names), "IPA Payload is incomplete")
        require(not any("_CodeSignature" in Path(name).parts or Path(name).name == "embedded.mobileprovision"
                        for name in names), "Expected an unsigned IPA")
        check_ios_info(plistlib.loads(archive.read(prefix + "Info.plist")), version, "iPhoneOS")
        binary = archive.getinfo(prefix + "XcocCamera")
        require(stat.S_ISREG(binary.external_attr >> 16) and (binary.external_attr >> 16) & 0o111 != 0,
                "IPA executable must retain executable permissions")
        with archive.open(binary) as stream:
            check_ios_binary(stream.read(64))


def package_ios_ipa(app: Path, output: Path, version: str) -> None:
    require(app.name == "XcocCamera.app" and not app.is_symlink() and app.is_dir(),
            "Expected the verified XcocCamera.app device bundle")
    require(output.suffix == ".ipa", "IPA output must have the .ipa extension")
    app = app.resolve()
    require(not output.resolve().is_relative_to(app), "IPA output must be outside the app bundle")
    check_ios_app(app, version, "iPhoneOS")
    binary = app / "XcocCamera"
    require(not binary.is_symlink() and binary.stat().st_mode & 0o111 != 0,
            "IPA executable must retain executable permissions")
    with binary.open("rb") as stream:
        check_ios_binary(stream.read(64))
    entries = [app, *sorted(app.rglob("*"))]
    for entry in entries:
        mode = entry.lstat().st_mode
        require(entry.name not in ("_CodeSignature", "embedded.mobileprovision"), "Expected an unsigned IPA")
        require(entry.suffix != ".a" or stat.S_ISDIR(mode), "Static libraries must be linked, not embedded")
        if entry.is_symlink():
            require(not os.path.isabs(os.readlink(entry)) and entry.resolve(strict=True).is_relative_to(app),
                    "Bundle symlink points outside the application")
        else:
            require(stat.S_ISREG(mode) or stat.S_ISDIR(mode), "Unsupported special file in application bundle")
    output.parent.mkdir(parents=True, exist_ok=True)
    descriptor, temporary_name = tempfile.mkstemp(suffix=".ipa", dir=output.parent)
    os.close(descriptor)
    temporary = Path(temporary_name)
    try:
        with zipfile.ZipFile(temporary, "w", zipfile.ZIP_DEFLATED, compresslevel=6) as archive:
            for entry in entries:
                name = "Payload/" + entry.relative_to(app.parent).as_posix()
                if entry.is_symlink():
                    record = zipfile.ZipInfo(name)
                    record.create_system = 3
                    record.external_attr = entry.lstat().st_mode << 16
                    archive.writestr(record, os.fsencode(os.readlink(entry)))
                else:
                    archive.write(entry, name)
        check_ios_ipa(temporary, version)
        os.replace(temporary, output)
    finally:
        temporary.unlink(missing_ok=True)


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
            "Unsigned device IPA and xcarchive: require Apple signing and provisioning before device installation. "
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
    print(f"Verified Android manifest: {APP_ID} {version} ({code}), minSdkVersion=26, targetSdkVersion=36")
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
    ipa = output / "xcoc-ios-arm64-unsigned.ipa"
    package_ios_ipa(device_app, ipa, version)
    artifacts.append(ipa)
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
