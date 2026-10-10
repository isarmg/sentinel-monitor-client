#!/usr/bin/env python3
"""Prepare each release's exact native notices and source/relinking materials."""
import argparse
import io
import pathlib
import platform
import re
import shutil
import subprocess
import tarfile
import tempfile

ROOT = pathlib.Path(__file__).resolve().parents[2]
VERSION = "9.0.2"


def copy_tree(source, target):
    shutil.copytree(source, target, ignore=shutil.ignore_patterns("*.o", "*.obj", "*.d", "__pycache__", ".git"))


def make_bundle(work, asset, output):
    output.mkdir(parents=True, exist_ok=True)
    notices = output / "native-licenses"
    shutil.copytree(ROOT / "packaging/native/licenses", notices, dirs_exist_ok=True)
    if platform.system() == "Windows":
        installed = work / "installed/x64-windows-static"
        license_file = installed / "share/ffmpeg/copyright"
        if not license_file.is_file():
            raise RuntimeError("Missing installed FFmpeg license")
        shutil.copy2(license_file, notices / "ffmpeg-copyright.txt")
        sources = list((work / "vcpkg/buildtrees/ffmpeg/src").glob("*/configure"))
        if len(sources) != 1:
            raise RuntimeError("Expected exactly one reviewed FFmpeg source tree")
        ffmpeg_source = sources[0].parent
    else:
        installed = work / "install"
        ffmpeg_source = work / "source" / f"ffmpeg-{VERSION}"
        shutil.copytree(installed / "share/xcoc-native", notices, dirs_exist_ok=True)
    if not (ffmpeg_source / "configure").is_file():
        raise RuntimeError("Missing corresponding FFmpeg source")
    with tempfile.TemporaryDirectory(prefix="xcoc-relink-") as directory:
        bundle = pathlib.Path(directory) / f"{asset}-relink-source"
        source = bundle / "xcoc"
        source.mkdir(parents=True)
        # Release workflows run on a verified immutable tag. git archive avoids
        # including credentials, transient state, or arbitrary runner files.
        archive = subprocess.check_output(["git", "archive", "--format=tar", "HEAD"], cwd=ROOT)
        with tarfile.open(fileobj=io.BytesIO(archive)) as tar:
            tar.extractall(source, filter="data")
        config = subprocess.check_output(
            ["cargo", "vendor", "--locked", "--versioned-dirs", str(source / "vendor")],
            cwd=ROOT, text=True,
        )
        (source / ".cargo").mkdir(exist_ok=True)
        # cargo vendor prints an absolute directory when given one. Keep the
        # archive relocatable so it can actually be rebuilt after extraction.
        config = re.sub(r'^directory\s*=\s*".*"\s*$', 'directory = "vendor"', config, flags=re.MULTILINE)
        (source / ".cargo/config.toml").write_text(config)
        native = bundle / "native"
        native.mkdir()
        shutil.copytree(ffmpeg_source, native / f"ffmpeg-{VERSION}", ignore=shutil.ignore_patterns("*.o", "*.obj", "*.d", "*.a", "*.lib", "__pycache__", ".git"))
        # Keep the original compiler configuration even when build products are
        # filtered from the source tree; Windows vcpkg config lives out of tree.
        if platform.system() == "Windows":
            copy_tree(work / "vcpkg/ports/ffmpeg", native / "vcpkg-ffmpeg-port")
            configurations = native / "configuration"
            configurations.mkdir()
            for log in (work / "vcpkg/buildtrees/ffmpeg").glob("*.log"):
                shutil.copy2(log, configurations / log.name)
            for name in ("x64-windows-static-rel", "x64-windows-static-dbg"):
                build = work / "vcpkg/buildtrees/ffmpeg" / name
                if build.is_dir():
                    target = configurations / name
                    target.mkdir()
                    for pattern in ("config.*", "*.rsp", "build.sh", "ffbuild/config.*"):
                        for entry in build.glob(pattern):
                            if entry.is_file():
                                destination = target / entry.relative_to(build)
                                destination.parent.mkdir(parents=True, exist_ok=True)
                                shutil.copy2(entry, destination)
        libraries = bundle / "native-libraries"
        for name in ("include", "lib"):
            copy_tree(installed / name, libraries / name)
        for pc in (libraries / "lib/pkgconfig").glob("*.pc"):
            lines = pc.read_text().splitlines()
            replacements = {"prefix": "${pcfiledir}/../..", "exec_prefix": "${prefix}", "libdir": "${prefix}/lib", "includedir": "${prefix}/include"}
            pc.write_text("\n".join(key + "=" + replacements[key] if (key := line.split("=", 1)[0]) in replacements else line for line in lines) + "\n")
        shutil.copytree(notices, bundle / "licenses")
        shutil.copy2(ROOT / "packaging/native/NOTICE.txt", bundle / "NOTICE.txt")
        (bundle / "SOURCE-BUILD.txt").write_text(
            "This is the exact source/relinking companion to " + asset + ".\n\n"
            "The xcoc directory includes the tagged application source and all Rust dependencies.\n"
            "Install Rust 1.99.0, a C compiler, and pkg-config (pkgconf on Windows).\n"
            "Set PKG_CONFIG_PATH to the absolute native-libraries/lib/pkgconfig directory.\n"
            "On Windows also set RUSTFLAGS=-C target-feature=+crt-static.\n"
            "From xcoc, run cargo build --offline --locked --release.\n"
            "The prebuilt native archives can be replaced with your modified FFmpeg libraries.\n"
            "native/ffmpeg-9.0.2 contains the corresponding FFmpeg source.\n"
            "The build configuration is in that tree (Unix) or native/configuration (Windows).\n"
            "Use xcoc/packaging/native/build-unix.sh or the pinned vcpkg recipe documented in\n"
            "xcoc/docs/media-worker.md to rebuild, then point PKG_CONFIG_PATH at your new install.\n"
            "The included native libraries are platform-specific; use the matching toolchain.\n"
            "No code-signing key or other secret is necessary to rebuild or install xcoc.\n"
        )
        with tarfile.open(output / f"{asset}-relink-source.tar.gz", "w:gz") as archive:
            archive.add(bundle, arcname=bundle.name)


if __name__ == "__main__":
    parser = argparse.ArgumentParser()
    parser.add_argument("--work", type=pathlib.Path, required=True)
    parser.add_argument("--asset", required=True)
    parser.add_argument("--output", type=pathlib.Path, required=True)
    args = parser.parse_args()
    make_bundle(args.work.resolve(), args.asset, args.output.resolve())
