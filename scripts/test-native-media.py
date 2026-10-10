#!/usr/bin/env python3
"""Run synthetic media-worker regression tests against a built xcoc executable.

Usage: python3 scripts/test-native-media.py /absolute/path/to/xcoc
Requires Python's standard library, ffmpeg/ffprobe with libx264 + libx265, and
openssl. All endpoints are local fixtures supplied through JSON on worker stdin.
No system trust settings are changed. Linux OpenSSL gets a per-child public CA
fixture through SSL_CERT_FILE; other TLS backends still test untrusted rejection.
"""
import contextlib
import http.server
import json
import os
from pathlib import Path
import socket
import ssl
import subprocess
import sys
import tempfile
import threading
import time

ROOT = None
WORKER = None
RESULTS = {}
OPENSSL_TRUST = sys.platform.startswith("linux")


class MediaHandler(http.server.BaseHTTPRequestHandler):
    def do_GET(self):
        file = ROOT / self.path.split("?", 1)[0].lstrip("/")
        if not file.is_file():
            self.send_error(404)
            return
        data = file.read_bytes()
        self.send_response(200)
        self.send_header("Content-Type", "video/mp2t")
        self.send_header("Content-Length", str(len(data)))
        self.end_headers()
        try:
            if "segments" in self.path:
                for offset in range(0, len(data), 32768):
                    self.wfile.write(data[offset:offset + 32768])
                    self.wfile.flush()
                    # Only ~3 wall-clock seconds for 906 seconds of media time.
                    # Separate filename seconds avoid strftime overwrite in a
                    # deliberately faster-than-real-time synthetic fixture.
                    time.sleep(0.07)
            elif "paced" in self.path:
                for offset in range(0, len(data), 4096):
                    self.wfile.write(data[offset:offset + 4096])
                    self.wfile.flush()
                    time.sleep(16 * min(4096, len(data) - offset) / len(data))
            else:
                self.wfile.write(data)
        except (BrokenPipeError, ConnectionResetError):
            pass

    def log_message(self, *args):
        pass


@contextlib.contextmanager
def media_server():
    server = http.server.ThreadingHTTPServer(("127.0.0.1", 0), MediaHandler)
    thread = threading.Thread(target=server.serve_forever, daemon=True)
    thread.start()
    try:
        yield f"http://127.0.0.1:{server.server_address[1]}"
    finally:
        server.shutdown()
        server.server_close()
        thread.join()


def run(mode, source=None, destination=None, rtsp=False, trust=False):
    command = [str(WORKER), "media-worker"]
    if mode == "check":
        command.append("--check")
        payload = ""
    else:
        payload = json.dumps({"operation": mode, "source": source,
                              "rtsp": rtsp, "destination": destination})
    env = os.environ.copy()
    env["FFREPORT"] = f"file={ROOT / 'unexpected-ffreport.log'}:level=56"
    env.pop("SSL_CERT_DIR", None)
    if trust:
        env["SSL_CERT_FILE"] = str(ROOT / "native-ca.pem")
    else:
        env.pop("SSL_CERT_FILE", None)
    started = time.monotonic()
    process = subprocess.Popen(command, stdin=subprocess.PIPE, stdout=subprocess.PIPE,
                               stderr=subprocess.PIPE, text=True, env=env)
    stdin = process.stdin
    try:
        if payload:
            stdin.write(payload + "\n")
            stdin.flush()
        # Keep the supervisor pipe alive while the worker completes naturally.
        # communicate() must not close it and request graceful cancellation.
        process.stdin = None
        stdout, stderr = process.communicate(timeout=25)
        completed = subprocess.CompletedProcess(command, process.returncode, stdout, stderr)
    except BaseException:
        process.kill()
        process.wait()
        raise
    finally:
        stdin.close()
    expected_error = "" if completed.returncode == 0 else "media worker failed\n"
    assert completed.stderr == expected_error, "worker emitted unexpected diagnostics"
    if mode != "probe" or completed.returncode != 0:
        assert completed.stdout == "", "worker emitted unexpected output"
    assert not (ROOT / "unexpected-ffreport.log").exists(), "FFREPORT created a file"
    return completed, round(time.monotonic() - started, 3)


def probe_file(path):
    return json.loads(subprocess.check_output([
        "ffprobe", "-v", "error", "-show_entries",
        "stream=index,codec_type,codec_name,width,height,nb_frames,start_time:format=duration,start_time",
        "-of", "json", str(path)]))


class RtspServer:
    def __init__(self, secure=True):
        self.commands = []
        self.sdp = ""
        self.packet_channels = {}
        self.packets = {}
        self.packet_bytes = 0
        self.errors = []
        self.socket = socket.socket()
        self.socket.bind(("127.0.0.1", 0))
        self.socket.listen()
        self.socket.settimeout(15)
        self.port = self.socket.getsockname()[1]
        self.secure = secure
        self.thread = threading.Thread(target=self.serve, daemon=True)
        self.thread.start()

    def serve(self):
        try:
            sock, _ = self.socket.accept()
            sock.settimeout(15)
            if self.secure:
                context = ssl.SSLContext(ssl.PROTOCOL_TLS_SERVER)
                context.load_cert_chain(ROOT / "native-server.pem", ROOT / "native-server.key")
                sock = context.wrap_socket(sock, server_side=True)
            with sock:
                data = bytearray()
                while True:
                    while len(data) < 4:
                        chunk = sock.recv(65536)
                        if not chunk:
                            return
                        data.extend(chunk)
                    if data[0] == 36:
                        length = int.from_bytes(data[2:4], "big")
                        while len(data) < length + 4:
                            chunk = sock.recv(65536)
                            if not chunk:
                                return
                            data.extend(chunk)
                        channel = data[1]
                        self.packet_bytes += length
                        if self.packet_bytes > 4 * 1024 * 1024:
                            raise RuntimeError("RTP fixture exceeded its packet buffer limit")
                        self.packet_channels[channel] = self.packet_channels.get(channel, 0) + 1
                        self.packets.setdefault(channel, []).append(bytes(data[4:length + 4]))
                        del data[:length + 4]
                        continue
                    while b"\r\n\r\n" not in data:
                        chunk = sock.recv(65536)
                        if not chunk:
                            return
                        data.extend(chunk)
                    end = data.index(b"\r\n\r\n") + 4
                    lines = data[:end].decode("ascii").split("\r\n")
                    headers = dict(line.split(":", 1) for line in lines[1:] if ":" in line)
                    headers = {k.lower(): v.strip() for k, v in headers.items()}
                    body_length = int(headers.get("content-length", 0))
                    while len(data) < end + body_length:
                        data.extend(sock.recv(65536))
                    body = bytes(data[end:end + body_length])
                    del data[:end + body_length]
                    verb = lines[0].split()[0]
                    self.commands.append(verb)
                    status = "200 OK"
                    extra = ""
                    if verb == "OPTIONS":
                        extra = "Public: OPTIONS, ANNOUNCE, SETUP, RECORD, TEARDOWN, DESCRIBE\r\n"
                    elif verb == "ANNOUNCE":
                        self.sdp = body.decode("ascii")
                    elif verb == "SETUP":
                        extra = f"Transport: {headers['transport']}\r\nSession: synthetic\r\n"
                    elif verb == "RECORD":
                        extra = "Session: synthetic\r\n"
                    elif verb == "DESCRIBE":
                        # Input TLS acceptance is proven by reaching RTSP after
                        # peer verification; this fixture is a publisher sink.
                        status = "404 Not Found"
                    response = (f"RTSP/1.0 {status}\r\nCSeq: {headers['cseq']}\r\n"
                                + extra + "Content-Length: 0\r\n\r\n")
                    sock.sendall(response.encode("ascii"))
                    if verb in ("TEARDOWN", "DESCRIBE"):
                        return
        except (ssl.SSLError, ConnectionResetError, BrokenPipeError, TimeoutError) as error:
            self.errors.append(type(error).__name__)
        finally:
            self.socket.close()

    def close(self):
        self.thread.join(timeout=17)
        assert not self.thread.is_alive(), "RTSP fixture thread did not finish"


def decode_published_tracks(server):
    """Replay unchanged RTP/RTCP to an independent FFmpeg SDP receiver.

    Only SDP destinations are rewritten. FFmpeg itself validates payload types,
    codec configuration, RTP framing and timestamps, and decodes every track.
    This avoids trusting a test-side media depacketizer for playability claims.
    """
    sections = server.sdp.split("\r\nm=")[1:]
    ports, reservations = [], []
    try:
        for _ in sections:
            for attempt in range(64):
                rtp = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
                rtcp = socket.socket(socket.AF_INET, socket.SOCK_DGRAM)
                rtp.bind(("127.0.0.1", 0))
                port = rtp.getsockname()[1]
                try:
                    if port == 65535:
                        raise OSError("no adjacent RTCP port")
                    rtcp.bind(("127.0.0.1", port + 1))
                except OSError:
                    rtp.close()
                    rtcp.close()
                    continue
                reservations.extend((rtp, rtcp))
                ports.append(port)
                break
            else:
                raise RuntimeError("could not allocate local RTP fixture ports")
        lines, index = [], 0
        for line in server.sdp.splitlines():
            if line.startswith("m="):
                parts = line.split()
                parts[1] = str(ports[index])
                index += 1
                line = " ".join(parts)
            elif line.startswith("c="):
                line = "c=IN IP4 127.0.0.1"
            lines.append(line)
        sdp = ROOT / f"receiver-{server.port}.sdp"
        output = ROOT / f"receiver-{server.port}.nut"
        sdp.write_text("\r\n".join(lines) + "\r\n")
    finally:
        for reserved in reservations:
            reserved.close()
    receiver = subprocess.Popen([
        "ffmpeg", "-v", "error", "-threads", "1", "-protocol_whitelist", "file,udp,rtp",
        "-analyzeduration", "100000", "-probesize", "1000", "-i", str(sdp),
        "-map", "0", "-t", "1", "-fps_mode", "passthrough", "-c:v", "rawvideo",
        "-c:a", "pcm_s16le", "-f", "nut", "-y", str(output)],
        stdin=subprocess.DEVNULL, stdout=subprocess.DEVNULL, stderr=subprocess.PIPE)
    try:
        # Wait until FFmpeg has bound the last RTCP socket rather than assuming
        # it starts within an arbitrary sleep on a busy CI machine.
        deadline = time.monotonic() + 5
        while True:
            with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as probe:
                try:
                    probe.bind(("127.0.0.1", ports[-1] + 1))
                except OSError:
                    break
            assert receiver.poll() is None and time.monotonic() < deadline, "receiver did not bind"
            time.sleep(0.01)
        with socket.socket(socket.AF_INET, socket.SOCK_DGRAM) as sender:
            # Sender reports retain the common NTP epoch used for A/V sync.
            for channel, packets in server.packets.items():
                if channel % 2:
                    for packet in packets:
                        sender.sendto(packet, ("127.0.0.1", ports[channel // 2] + 1))
            for packet_index in range(max(map(len, server.packets.values()))):
                for channel, packets in server.packets.items():
                    if channel % 2 == 0 and packet_index < len(packets):
                        sender.sendto(packets[packet_index], ("127.0.0.1", ports[channel // 2]))
                time.sleep(0.01)
        _, errors = receiver.communicate(timeout=10)
        assert receiver.returncode == 0 and errors == b"", "independent RTP receiver failed to decode"
    finally:
        if receiver.poll() is None:
            receiver.kill()
            receiver.communicate()
    metadata = json.loads(subprocess.check_output([
        "ffprobe", "-v", "error", "-count_frames", "-show_frames", "-show_entries",
        "stream=index,codec_type,nb_read_frames:frame=stream_index,media_type,nb_samples",
        "-of", "json", str(output)]))
    expected_types = [section.split()[0] for section in sections]
    assert [stream["codec_type"] for stream in metadata["streams"]] == expected_types, "receiver lost a track"
    counts = []
    for index, stream in enumerate(metadata["streams"]):
        frames = int(stream["nb_read_frames"])
        assert frames >= (8 if stream["codec_type"] == "video" else 30), "receiver decoded too few frames"
        samples = sum(frame.get("nb_samples", 0) for frame in metadata["frames"]
                      if frame["stream_index"] == index)
        if stream["codec_type"] == "audio":
            assert samples >= 32000, "receiver decoded too few audio samples"
        counts.append({"type": stream["codec_type"], "frames": frames, "samples": samples})
    return counts


def run_cases():
    completed, elapsed = run("check")
    assert completed.returncode == 0, "libav feature preflight failed"
    RESULTS["preflight"] = "passed"
    tls_cases = [("untrusted", "localhost", False)]
    if OPENSSL_TRUST:
        tls_cases += [("trusted", "localhost", True),
                      ("hostname_mismatch", "127.0.0.1", True)]
    with media_server() as base:
        for codec in ("h264", "hevc"):
            source = f"{base}/native-{codec}-aac.ts"
            completed, elapsed = run("probe", source)
            assert completed.returncode == 0, f"{codec} probe failed"
            metadata = json.loads(completed.stdout)
            assert [stream["codec_name"] for stream in metadata["streams"]] == [codec, "aac"], metadata
            assert metadata["streams"][0]["width"] == 160
            assert metadata["streams"][0]["height"] == 120
            assert metadata["streams"][0]["r_frame_rate"] == "10/1"
            RESULTS[f"{codec}_probe"] = {"streams": metadata["streams"], "seconds": elapsed}
            target = ROOT / f"native-record-{codec}"
            target.mkdir(exist_ok=True)
            for file in target.glob("*.mp4"):
                file.unlink()
            completed, elapsed = run("record", source, str(target / "%Y-%m-%d_%H-%M-%S.mp4"))
            assert completed.returncode == 0, f"{codec} record failed"
            files = list(target.glob("*.mp4"))
            assert len(files) == 1, files
            metadata = probe_file(files[0])
            assert [(s["index"], s["codec_name"]) for s in metadata["streams"]] == [(0, codec), (1, "aac")], metadata
            assert int(metadata["streams"][0]["nb_frames"]) == 20, metadata
            assert int(metadata["streams"][1]["nb_frames"]) >= 90, metadata
            decoded = subprocess.run(["ffmpeg", "-v", "error", "-i", str(files[0]), "-map", "0", "-f", "null", "-"], capture_output=True)
            assert decoded.returncode == 0 and decoded.stderr == b"", decoded.stderr
            RESULTS[f"{codec}_record"] = {"metadata": metadata, "decoded_all_tracks": True, "seconds": elapsed}

        source = f"{base}/native-three-tracks.ts"
        target = ROOT / "native-record-three"
        target.mkdir(exist_ok=True)
        for file in target.glob("*.mp4"):
            file.unlink()
        completed, elapsed = run("record", source, str(target / "%Y-%m-%d_%H-%M-%S.mp4"))
        assert completed.returncode == 0
        metadata = probe_file(next(target.glob("*.mp4")))
        assert [(s["index"], s["codec_name"]) for s in metadata["streams"]] == [(0, "aac"), (1, "h264"), (2, "aac")], metadata
        RESULTS["three_track_record_identity"] = metadata

        target = ROOT / "native-record-offset"
        target.mkdir()
        completed, _ = run("record", f"{base}/native-offset.ts",
                            str(target / "%Y-%m-%d_%H-%M-%S.mp4"))
        assert completed.returncode == 0, "shifted-timestamp recording failed"
        metadata = probe_file(next(target.glob("*.mp4")))
        audio_start = float(metadata["streams"][1]["start_time"])
        video_start = float(metadata["streams"][0]["start_time"])
        assert abs(audio_start) < 0.001, "first segment retained source epoch"
        assert abs(video_start - audio_start - 1024 / 48000) < 0.002, "A/V offset changed"
        RESULTS["shared_timestamp_epoch"] = metadata

        target = ROOT / "native-record-boundary"
        target.mkdir()
        completed, _ = run("record", f"{base}/native-boundary.ts?segments",
                            str(target / "%Y-%m-%d_%H-%M-%S.mp4"))
        assert completed.returncode == 0, "segment boundary recording failed"
        segments = [probe_file(file) for file in sorted(target.glob("*.mp4"))]
        assert len(segments) == 2, "unexpected segment count"
        assert [int(item["streams"][0]["nb_frames"]) for item in segments] == [1800, 12], "segment lost frames"
        assert all(abs(float(item["streams"][0]["start_time"])) < 0.001 for item in segments), "segment timestamp was not reset"
        assert [round(float(item["format"]["duration"])) for item in segments] == [900, 6], "segment duration changed"
        RESULTS["synthetic_900_second_boundary"] = segments

        for fixture, codecs, channels in ((("native-hevc-aac.ts", ["H265", "MPEG4-GENERIC"], [0, 2]),
                                           ("native-three-tracks.ts", ["H264", "MPEG4-GENERIC"], [0, 2, 4])) if OPENSSL_TRUST else ()):
            server = RtspServer()
            completed, elapsed = run("publish", f"{base}/{fixture}", f"rtsps://localhost:{server.port}/synthetic?jwt=SYNTHETIC_JWT", trust=True)
            server.close()
            assert completed.returncode == 0 and all(codec in server.sdp for codec in codecs), "publish tracks missing"
            assert all(channel in server.packet_channels for channel in channels), server.packet_channels
            received = decode_published_tracks(server)
            RESULTS[f"tls_publish_{fixture}"] = {"packet_channels": server.packet_channels, "receiver": received}

        for name, hostname, trusted in tls_cases:
            server = RtspServer()
            destination = f"rtsps://{hostname}:{server.port}/synthetic?jwt=SYNTHETIC_JWT"
            completed, elapsed = run("publish", f"{base}/native-h264-aac.ts", destination, trust=trusted)
            server.close()
            if name == "trusted":
                assert completed.returncode == 0, f"trusted publish failed: {server.commands} {server.errors}"
                assert "RECORD" in server.commands and "H264" in server.sdp and "MPEG4-GENERIC" in server.sdp
                assert 0 in server.packet_channels and 2 in server.packet_channels, server.packet_channels
                received = decode_published_tracks(server)
            else:
                assert completed.returncode != 0 and not server.commands, (name, server.commands)
            RESULTS[f"tls_publish_{name}"] = {"exit": completed.returncode, "commands": server.commands,
                                                "packet_channels": server.packet_channels, "seconds": elapsed,
                                                "receiver": received if name == "trusted" else None}

        for name, hostname, trusted in tls_cases:
            server = RtspServer()
            source = f"rtsps://{hostname}:{server.port}/synthetic?jwt=SYNTHETIC_JWT"
            completed, elapsed = run("probe", source, rtsp=True, trust=trusted)
            server.close()
            assert completed.returncode != 0
            if name == "trusted":
                assert server.commands == ["OPTIONS", "DESCRIBE"], server.commands
            else:
                assert not server.commands, server.commands
            RESULTS[f"tls_input_{name}"] = {"commands": server.commands, "seconds": elapsed}

        target = ROOT / "native-record-continuous"
        target.mkdir(exist_ok=True)
        for file in target.glob("*.mp4"):
            file.unlink()
        completed, elapsed = run("record", f"{base}/native-long.ts?paced", str(target / "%Y-%m-%d_%H-%M-%S.mp4"))
        assert completed.returncode == 0 and elapsed >= 15, elapsed
        metadata = probe_file(next(target.glob("*.mp4")))
        assert int(metadata["streams"][0]["nb_frames"]) == 160, metadata
        RESULTS["continuous_over_12_seconds"] = {"seconds": elapsed, "metadata": metadata}

    stalled = socket.socket()
    stalled.bind(("127.0.0.1", 0))
    stalled.listen()
    port = stalled.getsockname()[1]
    def stall():
        conn, _ = stalled.accept()
        with conn:
            conn.recv(8192)
            time.sleep(7)
        stalled.close()
    thread = threading.Thread(target=stall, daemon=True)
    thread.start()
    completed, elapsed = run("probe", f"http://127.0.0.1:{port}/synthetic")
    assert completed.returncode != 0 and 4.5 <= elapsed <= 7, elapsed
    thread.join()
    RESULTS["stalled_peer_timeout"] = {"seconds": elapsed, "exit": completed.returncode}

    completed, _ = run("probe", "rtsp://fixture-user:FIXTURE_PASSWORD@127.0.0.1:1/FIXTURE_TOKEN", rtsp=True)
    assert completed.returncode != 0 and completed.stdout == ""
    RESULTS["credential_logging"] = "generic diagnostics only, no FFREPORT file"
    print(json.dumps({"status": "passed", "checks": len(RESULTS),
                      "trusted_tls_and_receiver_tested": OPENSSL_TRUST,
                      "receiver_decode": {name: item["receiver"] for name, item in RESULTS.items()
                                          if isinstance(item, dict) and item.get("receiver")}}))


def fixture_command(arguments):
    completed = subprocess.run(arguments, stdout=subprocess.DEVNULL,
                               stderr=subprocess.PIPE, timeout=45)
    if completed.returncode != 0:
        # Fixture tool arguments contain no endpoints or credentials.
        raise RuntimeError(f"{arguments[0]} fixture generation failed: "
                           + completed.stderr.decode(errors="replace"))


def prepare_fixtures():
    video = ["-f", "lavfi", "-i", "testsrc2=size=160x120:rate=10"]
    audio = ["-f", "lavfi", "-i", "sine=frequency=1000:sample_rate=48000"]
    h264 = ["-c:v", "libx264", "-threads", "1", "-preset", "ultrafast",
            "-g", "10", "-bf", "0"]
    for codec in ("h264", "hevc"):
        encoder = h264 if codec == "h264" else [
            "-c:v", "libx265", "-threads", "1", "-preset", "ultrafast",
            "-x265-params", "log-level=error:pools=none:frame-threads=1",
            "-g", "10", "-bf", "0"]
        fixture_command(["ffmpeg", "-v", "error", *video, *audio, "-t", "2",
                         "-map", "0:v", "-map", "1:a", *encoder, "-c:a", "aac",
                         "-f", "mpegts", "-y", str(ROOT / f"native-{codec}-aac.ts")])
    fixture_command(["ffmpeg", "-v", "error", *audio, *video, *audio, "-t", "2",
                     "-map", "0:a", "-map", "1:v", "-map", "2:a", *h264,
                     "-c:a", "aac", "-f", "mpegts", "-y", str(ROOT / "native-three-tracks.ts")])
    fixture_command(["ffmpeg", "-v", "error", *video, *audio, "-t", "16",
                     "-map", "0:v", "-map", "1:a", *h264, "-c:a", "aac",
                     "-f", "mpegts", "-y", str(ROOT / "native-long.ts")])
    fixture_command(["ffmpeg", "-v", "error", "-itsoffset", "3600", "-i",
                     str(ROOT / "native-h264-aac.ts"), "-map", "0", "-c", "copy",
                     "-f", "mpegts", "-y", str(ROOT / "native-offset.ts")])
    fixture_command(["ffmpeg", "-v", "error", "-f", "lavfi", "-i",
                     "color=c=blue:size=32x32:rate=2", "-t", "906", "-c:v",
                     "libx264", "-threads", "1", "-g", "2", "-bf", "0",
                     "-f", "mpegts", "-y", str(ROOT / "native-boundary.ts")])

    (ROOT / "native-ca.cnf").write_text(
        "[req]\nprompt=no\ndistinguished_name=dn\nx509_extensions=ca\n"
        "[dn]\nCN=XcocSyntheticCA\n[ca]\nbasicConstraints=critical,CA:TRUE\n"
        "keyUsage=critical,keyCertSign,cRLSign\n")
    (ROOT / "native-server.ext").write_text(
        "subjectAltName=DNS:localhost\nextendedKeyUsage=serverAuth\n"
        "basicConstraints=critical,CA:FALSE\n")
    fixture_command(["openssl", "req", "-x509", "-newkey", "rsa:2048", "-nodes",
                     "-days", "2", "-keyout", str(ROOT / "native-ca.key"),
                     "-out", str(ROOT / "native-ca.pem"), "-config", str(ROOT / "native-ca.cnf")])
    fixture_command(["openssl", "req", "-newkey", "rsa:2048", "-nodes",
                     "-keyout", str(ROOT / "native-server.key"),
                     "-out", str(ROOT / "native-server.csr"), "-subj", "/CN=localhost"])
    fixture_command(["openssl", "x509", "-req", "-in", str(ROOT / "native-server.csr"),
                     "-CA", str(ROOT / "native-ca.pem"), "-CAkey", str(ROOT / "native-ca.key"),
                     "-CAcreateserial", "-out", str(ROOT / "native-server.pem"),
                     "-days", "2", "-extfile", str(ROOT / "native-server.ext")])


def main():
    global ROOT, WORKER
    if len(sys.argv) != 2:
        raise SystemExit("Usage: test-native-media.py /absolute/path/to/xcoc")
    WORKER = Path(sys.argv[1]).resolve(strict=True)
    with tempfile.TemporaryDirectory(prefix="xcoc-native-media-") as directory:
        ROOT = Path(directory)
        prepare_fixtures()
        run_cases()


if __name__ == "__main__":
    main()

