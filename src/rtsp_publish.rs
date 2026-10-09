//! RTSPS RECORD with interleaved H.264 RTP (RFC 6184), for native mobile encoders.
use anyhow::{Context, ensure};
use base64::{Engine, engine::general_purpose::STANDARD};
use std::{collections::HashMap, sync::Arc, time::Duration};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::TcpStream,
};
use tokio_rustls::{
    TlsConnector,
    client::TlsStream,
    rustls::{self, pki_types::ServerName},
};
use url::Url;

const IO_TIMEOUT: Duration = Duration::from_secs(5);
pub const MAX_FRAME_BYTES: usize = 4 * 1024 * 1024;
const MAX_RESPONSE_BYTES: usize = 32 * 1024;
const RTP_PAYLOAD_BYTES: usize = 1200;

pub fn validate_publish_url(value: &str) -> anyhow::Result<Url> {
    ensure!(
        value.len() <= 16384 && !value.chars().any(char::is_control),
        "invalid publish URL"
    );
    let url = Url::parse(value).context("invalid publish URL")?;
    ensure!(
        url.scheme() == "rtsps"
            && url.host_str().is_some()
            && url.username().is_empty()
            && url.password().is_none()
            && url.fragment().is_none()
            && url
                .query_pairs()
                .filter(|(key, value)| key == "jwt" && !value.is_empty())
                .count()
                == 1,
        "publish requires authenticated RTSPS"
    );
    Ok(url)
}

pub struct Publisher {
    stream: TlsStream<TcpStream>,
    url: Url,
    session: String,
    cseq: u32,
    packetizer: Packetizer,
    last_keepalive: std::time::Instant,
    sps: Vec<u8>,
    pps: Vec<u8>,
}

impl Publisher {
    pub async fn connect(value: &str, sps: &[u8], pps: &[u8]) -> anyhow::Result<Self> {
        let roots =
            rustls::RootCertStore::from_iter(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
        let tls = rustls::ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth();
        Self::connect_with_config(value, sps, pps, tls).await
    }

    async fn connect_with_config(
        value: &str,
        sps: &[u8],
        pps: &[u8],
        tls: rustls::ClientConfig,
    ) -> anyhow::Result<Self> {
        let url = validate_publish_url(value)?;
        ensure!(
            (4..=4096).contains(&sps.len())
                && sps[0] & 31 == 7
                && !pps.is_empty()
                && pps.len() <= 4096
                && pps[0] & 31 == 8,
            "invalid H264 parameter sets"
        );
        let host = url
            .host_str()
            .context("missing publish host")?
            .trim_matches(['[', ']'])
            .to_owned();
        let tcp = tokio::time::timeout(
            IO_TIMEOUT,
            TcpStream::connect((host.as_str(), url.port().unwrap_or(322))),
        )
        .await??;
        tcp.set_nodelay(true)?;
        let stream = tokio::time::timeout(
            IO_TIMEOUT,
            TlsConnector::from(Arc::new(tls)).connect(ServerName::try_from(host)?, tcp),
        )
        .await??;
        let mut publisher = Self {
            stream,
            url,
            session: String::new(),
            cseq: 0,
            packetizer: Packetizer::new(rand::random(), rand::random()),
            last_keepalive: std::time::Instant::now(),
            sps: sps.to_vec(),
            pps: pps.to_vec(),
        };
        let sdp = format!(
            "v=0\r\no=- 0 0 IN IP4 127.0.0.1\r\ns=Xcoc mobile\r\nc=IN IP4 0.0.0.0\r\nt=0 0\r\na=control:*\r\nm=video 0 RTP/AVP 96\r\na=rtpmap:96 H264/90000\r\na=fmtp:96 packetization-mode=1;profile-level-id={:02x}{:02x}{:02x};sprop-parameter-sets={},{}\r\na=control:trackID=0\r\n",
            sps[1],
            sps[2],
            sps[3],
            STANDARD.encode(sps),
            STANDARD.encode(pps)
        );
        let aggregate = publisher.url.to_string();
        publisher
            .request(
                "ANNOUNCE",
                &aggregate,
                "Content-Type: application/sdp\r\n",
                sdp.as_bytes(),
            )
            .await?;
        let mut track = publisher.url.clone();
        track.set_path(&format!("{}/trackID=0", track.path().trim_end_matches('/')));
        let headers = publisher
            .request(
                "SETUP",
                track.as_str(),
                "Transport: RTP/AVP/TCP;unicast;interleaved=0-1;mode=record\r\n",
                &[],
            )
            .await?;
        let session = headers
            .get("session")
            .and_then(|v| v.split(';').next())
            .context("missing RTSP session")?;
        ensure!(
            !session.is_empty()
                && session.len() <= 256
                && session
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"-_.".contains(&b)),
            "invalid RTSP session"
        );
        publisher.session = session.to_owned();
        ensure!(
            headers
                .get("transport")
                .is_some_and(|v| v.to_ascii_lowercase().contains("interleaved=0-1")),
            "server rejected TCP interleaving"
        );
        publisher
            .request("RECORD", &aggregate, "Range: npt=0.000-\r\n", &[])
            .await?;
        Ok(publisher)
    }

    async fn request(
        &mut self,
        method: &str,
        target: &str,
        extra: &str,
        body: &[u8],
    ) -> anyhow::Result<HashMap<String, String>> {
        self.cseq = self
            .cseq
            .checked_add(1)
            .context("RTSP sequence exhausted")?;
        let session = if self.session.is_empty() {
            String::new()
        } else {
            format!("Session: {}\r\n", self.session)
        };
        let request = format!(
            "{method} {target} RTSP/1.0\r\nCSeq: {}\r\nUser-Agent: XcocClient/{}\r\n{session}{extra}Content-Length: {}\r\n\r\n",
            self.cseq,
            env!("CARGO_PKG_VERSION"),
            body.len()
        );
        tokio::time::timeout(IO_TIMEOUT, async {
            self.stream.write_all(request.as_bytes()).await?;
            self.stream.write_all(body).await?;
            read_response(&mut self.stream, self.cseq).await
        })
        .await
        .context("RTSP response timeout")?
    }

    pub async fn frame(&mut self, annex_b: &[u8], timestamp_us: u64) -> anyhow::Result<()> {
        let units = annex_b_units(annex_b)?;
        let timestamp = ((u128::from(timestamp_us) * 90_000 / 1_000_000) & 0xffff_ffff) as u32;
        if units.iter().any(|unit| unit[0] & 31 == 5) {
            for parameter in [&self.sps, &self.pps] {
                for packet in self.packetizer.packets(parameter, timestamp, false) {
                    tokio::time::timeout(IO_TIMEOUT, self.stream.write_all(&packet)).await??;
                }
            }
        }
        for (index, unit) in units.iter().enumerate() {
            for packet in self
                .packetizer
                .packets(unit, timestamp, index + 1 == units.len())
            {
                tokio::time::timeout(IO_TIMEOUT, self.stream.write_all(&packet)).await??;
            }
        }
        if self.last_keepalive.elapsed() >= Duration::from_secs(15) {
            let target = self.url.to_string();
            self.request("OPTIONS", &target, "", &[]).await?;
            self.last_keepalive = std::time::Instant::now();
        }
        Ok(())
    }

    pub async fn close(&mut self) {
        let target = self.url.to_string();
        let _ = self.request("TEARDOWN", &target, "", &[]).await;
        let _ = self.stream.shutdown().await;
    }
}

async fn read_response<S: AsyncRead + AsyncWrite + Unpin>(
    stream: &mut S,
    cseq: u32,
) -> anyhow::Result<HashMap<String, String>> {
    let mut header = Vec::new();
    let mut skipped = 0;
    loop {
        let byte = stream.read_u8().await?;
        // RTCP packets share the control connection. Bound the amount skipped per reply.
        if header.is_empty() && byte == b'$' {
            let _channel = stream.read_u8().await?;
            let length = usize::from(stream.read_u16().await?);
            skipped += length;
            ensure!(
                skipped <= MAX_RESPONSE_BYTES,
                "too many interleaved control bytes"
            );
            let mut bytes = vec![0; length];
            stream.read_exact(&mut bytes).await?;
            continue;
        }
        header.push(byte);
        ensure!(header.len() <= MAX_RESPONSE_BYTES, "RTSP header too large");
        if header.ends_with(b"\r\n\r\n") {
            break;
        }
    }
    let text = std::str::from_utf8(&header).context("invalid RTSP header")?;
    let mut lines = text.split("\r\n");
    let status = lines.next().context("missing RTSP status")?;
    ensure!(
        status.split_whitespace().take(2).eq(["RTSP/1.0", "200"]),
        "RTSP request rejected"
    );
    let mut headers = HashMap::new();
    for line in lines.filter(|line| !line.is_empty()) {
        let (key, value) = line.split_once(':').context("invalid RTSP header field")?;
        ensure!(
            headers
                .insert(key.to_ascii_lowercase(), value.trim().to_owned())
                .is_none(),
            "duplicate RTSP header"
        );
    }
    ensure!(
        headers.get("cseq").and_then(|v| v.parse::<u32>().ok()) == Some(cseq),
        "RTSP sequence mismatch"
    );
    let length = headers
        .get("content-length")
        .map_or(Ok(0), |v| v.parse::<usize>())?;
    ensure!(length <= MAX_RESPONSE_BYTES, "RTSP body too large");
    let mut body = vec![0; length];
    stream.read_exact(&mut body).await?;
    Ok(headers)
}

/// Native encoders supply a complete access unit in Annex B format.
pub fn annex_b_units(frame: &[u8]) -> anyhow::Result<Vec<&[u8]>> {
    ensure!(
        !frame.is_empty() && frame.len() <= MAX_FRAME_BYTES,
        "invalid encoded frame size"
    );
    let mut starts = Vec::new();
    let mut cursor = 0;
    while cursor + 3 <= frame.len() {
        let length = if frame[cursor..].starts_with(&[0, 0, 0, 1]) {
            4
        } else if frame[cursor..].starts_with(&[0, 0, 1]) {
            3
        } else {
            cursor += 1;
            continue;
        };
        starts.push((cursor, cursor + length));
        cursor += length;
        ensure!(starts.len() <= 256, "too many H264 NAL units");
    }
    ensure!(
        starts.first().is_some_and(|(offset, _)| *offset == 0),
        "H264 frame must use Annex B start codes"
    );
    let mut units = Vec::new();
    for (index, (_, start)) in starts.iter().enumerate() {
        let end = starts
            .get(index + 1)
            .map_or(frame.len(), |(offset, _)| *offset);
        ensure!(*start < end, "empty H264 NAL unit");
        let unit = &frame[*start..end];
        ensure!(
            unit[0] & 0x80 == 0 && (1..=23).contains(&(unit[0] & 31)),
            "invalid H264 NAL unit"
        );
        units.push(unit);
    }
    Ok(units)
}

struct Packetizer {
    sequence: u16,
    ssrc: u32,
}
impl Packetizer {
    fn new(sequence: u16, ssrc: u32) -> Self {
        Self { sequence, ssrc }
    }
    fn packet(&mut self, payload: &[u8], timestamp: u32, marker: bool) -> Vec<u8> {
        let mut bytes = Vec::with_capacity(16 + payload.len());
        bytes.extend_from_slice(&[b'$', 0]);
        bytes.extend_from_slice(&((12 + payload.len()) as u16).to_be_bytes());
        bytes.extend_from_slice(&[0x80, 96 | if marker { 0x80 } else { 0 }]);
        bytes.extend_from_slice(&self.sequence.to_be_bytes());
        bytes.extend_from_slice(&timestamp.to_be_bytes());
        bytes.extend_from_slice(&self.ssrc.to_be_bytes());
        bytes.extend_from_slice(payload);
        self.sequence = self.sequence.wrapping_add(1);
        bytes
    }
    fn packets(&mut self, nal: &[u8], timestamp: u32, marker: bool) -> Vec<Vec<u8>> {
        if nal.len() <= RTP_PAYLOAD_BYTES {
            return vec![self.packet(nal, timestamp, marker)];
        }
        let fragments = nal[1..].chunks(RTP_PAYLOAD_BYTES - 2).collect::<Vec<_>>();
        fragments
            .iter()
            .enumerate()
            .map(|(index, part)| {
                let last = index + 1 == fragments.len();
                let mut payload = vec![
                    (nal[0] & 0xe0) | 28,
                    (nal[0] & 31)
                        | if index == 0 {
                            0x80
                        } else if last {
                            0x40
                        } else {
                            0
                        },
                ];
                payload.extend_from_slice(part);
                self.packet(&payload, timestamp, marker && last)
            })
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn fragmentation_reassembles_and_marks_only_the_last_packet() {
        let mut packetizer = Packetizer::new(u16::MAX, 42);
        let mut nal = vec![0x65];
        nal.extend((0..4000).map(|i| (i % 251 + 1) as u8));
        let packets = packetizer.packets(&nal, 90_000, true);
        assert!(packets.len() > 1);
        let mut rebuilt = vec![nal[0]];
        for (index, packet) in packets.iter().enumerate() {
            assert_eq!(
                u16::from_be_bytes([packet[6], packet[7]]),
                u16::MAX.wrapping_add(index as u16)
            );
            assert_eq!(packet[5] & 0x80 != 0, index + 1 == packets.len());
            assert_eq!(packet[16] & 31, 28);
            rebuilt.extend_from_slice(&packet[18..]);
        }
        assert_eq!(rebuilt, nal);
    }
    #[test]
    fn rejects_plaintext_credentials_and_malformed_access_units() {
        for url in [
            "rtsp://host/a?jwt=x",
            "rtsps://user:secret@host/a?jwt=x",
            "rtsps://host/a",
            "rtsps://host/a?jwt=x&jwt=y",
        ] {
            assert!(validate_publish_url(url).is_err());
        }
        assert!(validate_publish_url("rtsps://host:8322/a?jwt=x").is_ok());
        assert_eq!(
            annex_b_units(&[0, 0, 0, 1, 0x67, 1, 0, 0, 1, 0x68, 2]).unwrap(),
            [&[0x67, 1][..], &[0x68, 2][..]]
        );
        for frame in [&[0x65, 1][..], &[0, 0, 1][..], &[0, 0, 1, 0xff][..]] {
            assert!(annex_b_units(frame).is_err());
        }
    }
    #[tokio::test]
    async fn response_limits_sequence_and_interleaved_rtcp() {
        let (mut writer, mut reader) = tokio::io::duplex(1024);
        writer
            .write_all(b"$\x01\x00\x02xxRTSP/1.0 200 OK\r\nCSeq: 7\r\nSession: abc\r\n\r\n")
            .await
            .unwrap();
        assert_eq!(
            read_response(&mut reader, 7).await.unwrap()["session"],
            "abc"
        );
        writer
            .write_all(b"RTSP/1.0 200 OK\r\nCSeq: 8\r\nContent-Length: 999999\r\n\r\n")
            .await
            .unwrap();
        assert!(read_response(&mut reader, 8).await.is_err());
    }

    #[tokio::test]
    #[ignore = "requires local MediaMTX, generated TLS certificate and H264 fixture"]
    async fn native_h264_publisher_is_readable_by_mediamtx() {
        let destination = std::env::var("XCOC_TEST_PUBLISH_URL").unwrap();
        let read_url = std::env::var("XCOC_TEST_READ_URL").unwrap();
        let cert = std::fs::read(std::env::var("XCOC_TEST_CERT_DER").unwrap()).unwrap();
        let source = std::fs::read(std::env::var("XCOC_TEST_H264").unwrap()).unwrap();
        let units = annex_b_units(&source).unwrap();
        let sps = units.iter().find(|nal| nal[0] & 31 == 7).unwrap();
        let pps = units.iter().find(|nal| nal[0] & 31 == 8).unwrap();
        // Production connect must reject this private fixture certificate.
        assert!(Publisher::connect(&destination, sps, pps).await.is_err());
        let mut roots = rustls::RootCertStore::empty();
        roots.add(cert.into()).unwrap();
        let tls = rustls::ClientConfig::builder()
            .with_root_certificates(roots)
            .with_no_client_auth();
        let mut publisher = Publisher::connect_with_config(&destination, sps, pps, tls)
            .await
            .unwrap();
        let mut command = tokio::process::Command::new("ffprobe");
        command.args([
            "-v",
            "error",
            "-rtsp_transport",
            "tcp",
            "-read_intervals",
            "%+#1",
            "-show_entries",
            "stream=codec_name,width,height",
            "-of",
            "json",
            &read_url,
        ]);
        let reader = xcsc_runtime::process::capture_bounded(
            &mut command,
            xcsc_runtime::process::ProcessLimits {
                timeout: Duration::from_secs(15),
                stdout_bytes: 65536,
                stderr_bytes: 65536,
            },
        );
        let sender = async {
            let mut frame = Vec::new();
            let mut index = 0;
            for nal in units {
                if nal[0] & 31 == 9 && !frame.is_empty() {
                    publisher.frame(&frame, index * 100_000).await.unwrap();
                    frame.clear();
                    index += 1;
                    tokio::time::sleep(Duration::from_millis(100)).await;
                }
                frame.extend_from_slice(&[0, 0, 0, 1]);
                frame.extend_from_slice(nal);
            }
            if !frame.is_empty() {
                publisher.frame(&frame, index * 100_000).await.unwrap();
            }
        };
        let (_, output) = tokio::join!(sender, reader);
        let output = output.unwrap();
        assert!(
            output.status.success(),
            "fixture reader failed: {}",
            String::from_utf8_lossy(&output.stderr)
        );
        let document: serde_json::Value = serde_json::from_slice(&output.stdout).unwrap();
        assert_eq!(document["streams"][0]["codec_name"], "h264");
        assert_eq!(document["streams"][0]["width"], 320);
        publisher.close().await;
    }
}
