//! FFmpeg capture backends. Only typed options reach process arguments.
use anyhow::ensure;
use serde::{Deserialize, Serialize};
use std::{path::Path, process::Stdio, sync::Arc};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

/// One encoder per physical camera; publishers and recorders consume independent
/// bounded connections. Dropping the last source stops capture and all readers.
pub struct CaptureRelay {
    pub url: String,
    task: tokio::task::AbortHandle,
}
impl Drop for CaptureRelay {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl CaptureRelay {
    pub fn is_alive(&self) -> bool {
        !self.task.is_finished()
    }
}

pub async fn start_capture(config: &LocalCameraConfig) -> anyhow::Result<Arc<CaptureRelay>> {
    config.validate()?;
    let mut command = tokio::process::Command::new("ffmpeg");
    command
        .args(["-nostdin", "-hide_banner", "-loglevel", "error"])
        .args(config.input_args())
        .args(config.encode_args())
        .args(["-mpegts_flags", "+resend_headers", "-f", "mpegts", "pipe:1"]);
    start_relay(&mut command).await
}

async fn start_relay(command: &mut tokio::process::Command) -> anyhow::Result<Arc<CaptureRelay>> {
    let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0)).await?;
    let path = format!("/{}", uuid::Uuid::new_v4());
    let url = format!("http://{}{}", listener.local_addr()?, path);
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()?;
    let mut stdout = child.stdout.take().expect("piped capture output");
    let task = tokio::spawn(async move {
        // Maximum 64 * 16 KiB buffered; lagging sinks disconnect and reconcile independently.
        let (sender, _) = tokio::sync::broadcast::channel::<Arc<[u8]>>(64);
        let slots = Arc::new(tokio::sync::Semaphore::new(8));
        let mut readers = tokio::task::JoinSet::new();
        let mut buffer = [0_u8; 16 * 1024];
        loop {
            tokio::select! {
                read = stdout.read(&mut buffer) => {
                    match read { Ok(0) | Err(_) => break, Ok(count) => { let _ = sender.send(Arc::from(&buffer[..count])); } }
                }
                connection = listener.accept() => {
                    let Ok((mut socket, _)) = connection else { break };
                    let Ok(slot) = slots.clone().try_acquire_owned() else { continue };
                    let path = path.clone();
                    let mut receiver = sender.subscribe();
                    readers.spawn(async move {
                        let _slot = slot;
                        let request = async {
                            let mut header = Vec::new();
                            while !header.ends_with(b"\r\n\r\n") {
                                if header.len() >= 2048 { return Err(std::io::Error::other("relay request too large")); }
                                header.push(socket.read_u8().await?);
                            }
                            if !header.starts_with(format!("GET {path} HTTP/1.").as_bytes()) { return Err(std::io::Error::other("invalid relay request")); }
                            socket.write_all(b"HTTP/1.1 200 OK\r\nContent-Type: video/mp2t\r\nCache-Control: no-store\r\nConnection: close\r\n\r\n").await
                        };
                        if !matches!(tokio::time::timeout(std::time::Duration::from_secs(5), request).await, Ok(Ok(()))) { return; }
                        while let Ok(chunk) = receiver.recv().await {
                            if !matches!(tokio::time::timeout(std::time::Duration::from_secs(5), socket.write_all(&chunk)).await, Ok(Ok(()))) { break; }
                        }
                    });
                }
                _ = readers.join_next(), if !readers.is_empty() => {}
            }
        }
        readers.abort_all();
        let _ = child.kill().await;
    });
    Ok(Arc::new(CaptureRelay {
        url,
        task: task.abort_handle(),
    }))
}

#[derive(Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CaptureBackend {
    V4l2,
    Dshow,
    Avfoundation,
}

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct LocalCameraConfig {
    pub backend: CaptureBackend,
    pub device: String,
    #[serde(default = "default_width")]
    pub width: u32,
    #[serde(default = "default_height")]
    pub height: u32,
    #[serde(default = "default_frame_rate")]
    pub frame_rate: u32,
}
const fn default_width() -> u32 {
    1280
}
const fn default_height() -> u32 {
    720
}
const fn default_frame_rate() -> u32 {
    30
}

impl LocalCameraConfig {
    pub fn validate(&self) -> anyhow::Result<()> {
        ensure!(
            (16..=7680).contains(&self.width)
                && (16..=4320).contains(&self.height)
                && self.width.is_multiple_of(2)
                && self.height.is_multiple_of(2)
                && (1..=60).contains(&self.frame_rate),
            "invalid local camera capture size or frame rate"
        );
        ensure!(
            !self.device.is_empty()
                && self.device.len() <= 512
                && !self.device.chars().any(char::is_control),
            "invalid local camera device"
        );
        match self.backend {
            CaptureBackend::V4l2 => {
                ensure!(Path::new(&self.device).parent() == Some(Path::new("/dev")) && self.device.strip_prefix("/dev/video").is_some_and(|v| !v.is_empty() && v.bytes().all(|b| b.is_ascii_digit())), "V4L2 device must be /dev/videoN");
                ensure!(cfg!(target_os = "linux"), "V4L2 capture requires Linux");
            }
            CaptureBackend::Dshow => {
                ensure!(
                    !self.device.contains([':', '"']) && !self.device.starts_with('-'),
                    "invalid DirectShow video device name"
                );
                ensure!(
                    cfg!(target_os = "windows"),
                    "DirectShow capture requires Windows"
                );
            }
            CaptureBackend::Avfoundation => {
                ensure!(
                    self.device.bytes().all(|b| b.is_ascii_digit()) && self.device.len() <= 3,
                    "AVFoundation device must be a video device index"
                );
                ensure!(
                    cfg!(target_os = "macos"),
                    "AVFoundation desktop capture requires macOS"
                );
            }
        }
        Ok(())
    }

    pub fn input_args(&self) -> Vec<String> {
        let (format, device) = match self.backend {
            CaptureBackend::V4l2 => ("video4linux2", self.device.clone()),
            CaptureBackend::Dshow => ("dshow", format!("video={}", self.device)),
            CaptureBackend::Avfoundation => ("avfoundation", format!("{}:none", self.device)),
        };
        vec![
            "-f".into(),
            format.into(),
            "-framerate".into(),
            self.frame_rate.to_string(),
            "-video_size".into(),
            format!("{}x{}", self.width, self.height),
            "-i".into(),
            device,
        ]
    }

    pub fn encode_args(&self) -> Vec<String> {
        vec![
            "-map".into(),
            "0:v:0".into(),
            "-an".into(),
            "-c:v".into(),
            "libx264".into(),
            "-preset".into(),
            "veryfast".into(),
            "-tune".into(),
            "zerolatency".into(),
            "-pix_fmt".into(),
            "yuv420p".into(),
            "-g".into(),
            (self.frame_rate * 2).to_string(),
        ]
    }
}

/// FFmpeg's platform enumerators intentionally report their results on stderr.
pub async fn list_devices() -> anyhow::Result<serde_json::Value> {
    #[cfg(target_os = "linux")]
    {
        let mut devices = std::fs::read_dir("/dev")?
            .filter_map(Result::ok)
            .filter_map(|entry| {
                let name = entry.file_name().to_string_lossy().into_owned();
                name.strip_prefix("video")
                    .filter(|suffix| {
                        !suffix.is_empty() && suffix.bytes().all(|b| b.is_ascii_digit())
                    })
                    .map(|_| entry.path().display().to_string())
            })
            .collect::<Vec<_>>();
        devices.sort();
        Ok(serde_json::json!({"backend": "v4l2", "devices": devices}))
    }
    #[cfg(any(target_os = "windows", target_os = "macos"))]
    {
        let mut command = tokio::process::Command::new("ffmpeg");
        command.args(["-hide_banner", "-list_devices", "true", "-f"]);
        #[cfg(target_os = "windows")]
        command.args(["dshow", "-i", "dummy"]);
        #[cfg(target_os = "macos")]
        command.args(["avfoundation", "-i", ""]);
        let output = xcsc_runtime::process::capture_bounded(
            &mut command,
            xcsc_runtime::process::ProcessLimits {
                timeout: std::time::Duration::from_secs(12),
                stdout_bytes: 65536,
                stderr_bytes: 65536,
            },
        )
        .await?;
        Ok(serde_json::json!({"devices": String::from_utf8_lossy(&output.stderr)}))
    }
    #[cfg(not(any(target_os = "linux", target_os = "windows", target_os = "macos")))]
    anyhow::bail!("use the native mobile camera client on this platform")
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn backend_input_options_and_encoding_are_explicit() {
        let mut camera = LocalCameraConfig {
            backend: CaptureBackend::V4l2,
            device: "/dev/video0".into(),
            width: 1280,
            height: 720,
            frame_rate: 30,
        };
        assert_eq!(
            camera.input_args(),
            [
                "-f",
                "video4linux2",
                "-framerate",
                "30",
                "-video_size",
                "1280x720",
                "-i",
                "/dev/video0"
            ]
        );
        camera.backend = CaptureBackend::Dshow;
        camera.device = "USB Camera".into();
        assert_eq!(camera.input_args().last().unwrap(), "video=USB Camera");
        camera.backend = CaptureBackend::Avfoundation;
        camera.device = "0".into();
        assert_eq!(camera.input_args().last().unwrap(), "0:none");
        assert!(camera.encode_args().iter().any(|arg| arg == "libx264"));
    }
    #[test]
    fn rejects_invalid_sizes_and_device_selectors() {
        for device in [
            "/dev/video0/../video1",
            "/tmp/video0",
            "-i",
            "/dev/video0\n",
        ] {
            let camera = LocalCameraConfig {
                backend: CaptureBackend::V4l2,
                device: device.into(),
                width: 1280,
                height: 720,
                frame_rate: 30,
            };
            assert!(camera.validate().is_err());
        }
    }

    #[tokio::test]
    #[ignore = "requires FFmpeg/FFprobe with libx264; run in media CI"]
    async fn one_capture_serves_simultaneous_probe_and_recording_then_releases_port() {
        use crate::device::{
            ControlTarget, DeviceCapabilities, DeviceIdentity, MediaSource, ResolvedDevice,
            ResolvedStream, StreamDescriptor,
        };
        let mut command = tokio::process::Command::new("ffmpeg");
        command.args([
            "-nostdin",
            "-v",
            "error",
            "-re",
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=320x240:rate=10",
            "-c:v",
            "libx264",
            "-tune",
            "zerolatency",
            "-g",
            "10",
            "-f",
            "mpegts",
            "pipe:1",
        ]);
        let relay = start_relay(&mut command).await.unwrap();
        let source = MediaSource::Local(relay.clone());
        let mut device = ResolvedDevice {
            adapter_kind: "rtsp",
            identity: DeviceIdentity::default(),
            capabilities: DeviceCapabilities::unknown(),
            control: ControlTarget::None,
            streams: vec![ResolvedStream {
                source: source.clone(),
                descriptor: StreamDescriptor {
                    profile: "main".into(),
                    video_codec: None,
                    audio_codec: None,
                    width: None,
                    height: None,
                    frame_rate: None,
                },
            }],
        };
        let directory = tempfile::tempdir().unwrap();
        let output = directory.path().join("recording.mp4");
        let mut recorder = tokio::process::Command::new("ffmpeg");
        recorder
            .args(["-nostdin", "-v", "error"])
            .args(source.input_args())
            .args(["-t", "3", "-map", "0:v:0", "-c", "copy"])
            .arg(&output);
        let recording = xcsc_runtime::process::capture_bounded(
            &mut recorder,
            xcsc_runtime::process::ProcessLimits {
                timeout: std::time::Duration::from_secs(20),
                stdout_bytes: 65536,
                stderr_bytes: 65536,
            },
        );
        let (probe, recording) = tokio::join!(device.probe_streams(), recording);
        if let Err(error) = probe {
            let output = tokio::process::Command::new("ffprobe")
                .args(source.input_args())
                .args(["-v", "error", "-show_streams", "-of", "json"])
                .output()
                .await
                .unwrap();
            panic!(
                "synthetic probe: {error}; ffprobe: {}",
                String::from_utf8_lossy(&output.stderr)
            );
        }
        assert!(recording.unwrap().status.success());
        assert_eq!(
            device.streams[0].descriptor.video_codec.as_deref(),
            Some("h264")
        );
        assert_eq!(device.streams[0].descriptor.width, Some(320));
        assert!(output.metadata().unwrap().len() > 1024);
        let address = url::Url::parse(&relay.url).unwrap();
        let port = address.port().unwrap();
        let task = relay.task.clone();
        drop(device);
        drop(source);
        drop(relay);
        for _ in 0..100 {
            if task.is_finished() {
                break;
            }
            tokio::time::sleep(std::time::Duration::from_millis(10)).await;
        }
        assert!(
            tokio::net::TcpStream::connect((std::net::Ipv4Addr::LOCALHOST, port))
                .await
                .is_err()
        );
    }
}
