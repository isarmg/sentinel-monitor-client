#!/usr/bin/env bash
set -euo pipefail
root="$(cd "$(dirname "$0")/.." && pwd)"
: "${XCOC_TEST_MEDIAMTX:?Set XCOC_TEST_MEDIAMTX to the reviewed MediaMTX 1.20.0 binary}"
test "$("$XCOC_TEST_MEDIAMTX" --version)" = v1.20.0
case "$(uname -s)-$(uname -m)" in
    Linux-x86_64) expected_sha256=25947caac403f37ec881c9be213af2cad67e344a6c7098905b0d31c17f40e336 ;;
    Darwin-arm64) expected_sha256=ba58815df906213c728974e15e485c0179f524cd8143db4bc79af68ef43ffa39 ;;
    *) echo "Media validation requires a reviewed Linux x86_64 or macOS ARM64 companion" >&2; exit 1 ;;
esac
python3 - "$XCOC_TEST_MEDIAMTX" "$expected_sha256" <<'PY'
import hashlib, sys
digest = hashlib.sha256()
with open(sys.argv[1], 'rb') as binary:
    for chunk in iter(lambda: binary.read(1024 * 1024), b''):
        digest.update(chunk)
if digest.hexdigest() != sys.argv[2]:
    raise SystemExit('MediaMTX binary checksum does not match the reviewed companion')
PY
fixture="$(mktemp -d)"
media_pid=""
cleanup() {
    if test -n "$media_pid"; then kill "$media_pid" 2>/dev/null || true; wait "$media_pid" 2>/dev/null || true; fi
    rm -rf "$fixture"
}
trap cleanup EXIT
openssl req -x509 -newkey rsa:2048 -nodes -days 1 -keyout "$fixture/server.key" -out "$fixture/server.crt" \
    -subj /CN=localhost -addext subjectAltName=DNS:localhost -addext basicConstraints=critical,CA:FALSE > "$fixture/certificate.log" 2>&1
openssl x509 -in "$fixture/server.crt" -outform DER -out "$fixture/server.der"
ffmpeg -v error -f lavfi -i testsrc2=size=320x240:rate=10 -t 8 -c:v libx264 -threads 1 -tune zerolatency \
    -x264-params aud=1:keyint=10 -f h264 "$fixture/source.h264"
cat > "$fixture/mediamtx.yml" <<EOF
logLevel: warn
rtspTransports: [tcp]
rtspAddress: 127.0.0.1:17554
rtspsAddress: 127.0.0.1:18322
rtspEncryption: optional
rtspServerKey: $fixture/server.key
rtspServerCert: $fixture/server.crt
rtmp: no
hls: no
webrtc: no
srt: no
moq: no
api: no
metrics: no
pprof: no
playback: no
paths:
  all_others: {}
EOF
"$XCOC_TEST_MEDIAMTX" "$fixture/mediamtx.yml" > "$fixture/mediamtx.log" 2>&1 &
media_pid=$!
python3 - <<'PY'
import socket, time
for attempt in range(100):
    try:
        with socket.create_connection(('127.0.0.1', 18322), timeout=0.1):
            break
    except OSError:
        time.sleep(0.05)
else:
    raise SystemExit('MediaMTX test listener did not start')
PY
cd "$root"
export XCOC_TEST_PUBLISH_URL='rtsps://localhost:18322/mobile-test?jwt=fixture'
export XCOC_TEST_READ_URL='rtsp://127.0.0.1:17554/mobile-test'
export XCOC_TEST_CERT_DER="$fixture/server.der"
export XCOC_TEST_H264="$fixture/source.h264"
cargo test --locked --lib local_camera::tests::one_capture -- --ignored
cargo test --locked --lib rtsp_publish::tests::native_h264 -- --ignored
