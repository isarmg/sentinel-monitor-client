#!/usr/bin/env bash
# Build the desktop media libraries from the signed, reviewed upstream release.
# No FFmpeg programs, GPL codecs, or nonfree components are linked into xcoc.
set -euo pipefail
prefix="${1:?Usage: build-unix.sh ABSOLUTE_INSTALL_PREFIX [BUILD_DIRECTORY]}"
case "$prefix" in /*) ;; *) echo 'The install prefix must be absolute.' >&2; exit 1 ;; esac
work="${2:-${prefix}-build}"
version=9.0.2
sha256=8c3850283eb25fa026482078a04051e0be17347b09ef81a0849bec15a96e002e
mkdir -p "$work" "$prefix"
work="$(cd "$work" && pwd)"
archive="$work/ffmpeg-$version.tar.xz"
if [[ ! -f "$archive" ]]; then
  curl --fail --location --retry 3 --output "$archive.part" "https://ffmpeg.org/releases/ffmpeg-$version.tar.xz"
  mv "$archive.part" "$archive"
fi
python3 - "$archive" "$sha256" <<'PY'
import hashlib, pathlib, sys
if hashlib.sha256(pathlib.Path(sys.argv[1]).read_bytes()).hexdigest() != sys.argv[2]:
    raise SystemExit('FFmpeg source checksum mismatch')
PY
if [[ ! -f "$work/ffmpeg-$version/configure" ]]; then
  tar -xJf "$archive" -C "$work"
fi
options=(
  "--prefix=$prefix" --enable-static --disable-shared --enable-pic
  --disable-programs --disable-doc --disable-autodetect
  --disable-avdevice --disable-avfilter --disable-swscale --disable-swresample
  --disable-encoders --disable-hwaccels --disable-x86asm
  --pkg-config-flags=--static
)
case "$(uname -s)" in
  Linux)
    # libssl-dev supplies both headers and static archives. build.rs requests
    # pkg-config's static transitive closure; the final dependency audit rejects
    # an accidental shared OpenSSL dependency.
    options+=(--enable-openssl --enable-version3)
    jobs="$(getconf _NPROCESSORS_ONLN)"
    ;;
  Darwin)
    options+=(--enable-securetransport)
    jobs="$(sysctl -n hw.ncpu)"
    ;;
  *) echo 'Use build-windows.ps1 on Windows; unsupported Unix target.' >&2; exit 1 ;;
esac
cd "$work/ffmpeg-$version"
./configure "${options[@]}"
make -j "${XCOC_BUILD_JOBS:-$jobs}"
make install
mkdir -p "$prefix/share/xcoc-native"
if [[ "$(uname -s)" == Linux ]]; then
  # pkg-config-rs intentionally links libraries beneath /usr dynamically even
  # with statik(true). Stage non-OS archives beside FFmpeg so release builds
  # really are self-contained. Resolve the full configured link closure rather
  # than assuming every distro's OpenSSL has the same compression dependencies.
  python3 - "$prefix" <<'PYTHON'
import pathlib, re, shutil, subprocess, sys
prefix = pathlib.Path(sys.argv[1])
libs = set()
for pc in (prefix / 'lib/pkgconfig').glob('*.pc'):
    libs.update(re.findall(r'(?:^|\s)-l([\w.+-]+)', pc.read_text()))
for name in sorted(libs - {'c', 'm', 'dl', 'pthread', 'rt', 'gcc', 'gcc_s', 'avformat', 'avcodec', 'avutil'}):
    filename = f'lib{name}.a'
    target = prefix / 'lib' / filename
    found = subprocess.check_output(['cc', '-print-file-name=' + filename], text=True).strip()
    archive = pathlib.Path(found).resolve()
    if found == filename or not archive.is_file():
        raise SystemExit('Missing static dependency: ' + filename)
    shutil.copy2(archive, target)
    # Preserve the distribution's applicable notices for each staged archive.
    packages = {'ssl': 'libssl-dev', 'crypto': 'libssl-dev', 'z': 'zlib1g-dev', 'zstd': 'libzstd-dev', 'atomic': 'libatomic1'}
    package = packages.get(name)
    if package is None:
        raise SystemExit('Review the license notice for new dependency: ' + name)
    copyright = pathlib.Path('/usr/share/doc') / package / 'copyright'
    if not copyright.is_file():
        raise SystemExit('Missing dependency notice: ' + str(copyright))
    shutil.copy2(copyright, prefix / 'share/xcoc-native' / f'{package}-copyright.txt')
PYTHON
fi
cp COPYING.LGPLv2.1 COPYING.LGPLv3 COPYING.GPLv3 LICENSE.md "$prefix/share/xcoc-native/"
printf '%s\n' "$sha256  ffmpeg-$version.tar.xz" > "$prefix/share/xcoc-native/SHA256SUMS"
printf '%s\n' "PKG_CONFIG_PATH=$prefix/lib/pkgconfig"
