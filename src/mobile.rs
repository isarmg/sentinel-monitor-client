//! Native mobile control plane; FFmpeg is never required on Android or iOS.
use crate::{
    device::{CapabilityStatus, DeviceCapabilities, DeviceIdentity, StreamDescriptor},
    rtsp_publish::{self, Publisher},
};
use anyhow::{Context, ensure};
use serde::{Deserialize, Serialize};
use serde_json::{Value, json};
use std::{
    sync::{
        Arc, Mutex,
        atomic::{AtomicBool, Ordering},
    },
    time::Duration,
};
use tokio::sync::mpsc;
use tokio_rustls::rustls;
use url::Url;
use uuid::Uuid;

pub const PROTOCOL: &str = env!("XCOC_EDGE_PROTOCOL");
const API_PREFIX: &str = env!("XCOC_API_PREFIX");

#[derive(Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Pairing {
    pub server: String,
    pub installation_id: Uuid,
    pub instance_id: Uuid,
    pub access_token: String,
    pub name: String,
}
impl Pairing {
    pub fn validate(&self) -> anyhow::Result<()> {
        server_origin(&self.server)?;
        validate_name(&self.name)?;
        ensure!(
            !self.instance_id.is_nil()
                && !self.installation_id.is_nil()
                && self.access_token.len() == 43
                && self
                    .access_token
                    .bytes()
                    .all(|b| b.is_ascii_alphanumeric() || b"-_".contains(&b)),
            "invalid mobile pairing"
        );
        Ok(())
    }
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct CaptureMetadata {
    pub platform: MobilePlatform,
    pub model: String,
    pub width: u32,
    pub height: u32,
    pub frame_rate: u32,
    pub active: bool,
}
#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum MobilePlatform {
    Android,
    Ios,
}
impl CaptureMetadata {
    fn validate(&self) -> anyhow::Result<()> {
        ensure!(
            !self.model.is_empty()
                && self.model.chars().count() <= 128
                && !self.model.chars().any(char::is_control)
                && (16..=7680).contains(&self.width)
                && (16..=4320).contains(&self.height)
                && (1..=60).contains(&self.frame_rate),
            "invalid mobile capture metadata"
        );
        Ok(())
    }
}

fn validate_name(value: &str) -> anyhow::Result<()> {
    ensure!(
        !value.trim().is_empty()
            && value.chars().count() <= 64
            && !value.chars().any(char::is_control),
        "invalid mobile camera name"
    );
    Ok(())
}
fn server_origin(value: &str) -> anyhow::Result<Url> {
    ensure!(value.len() <= 4096, "invalid Server origin");
    let url = Url::parse(value)?;
    ensure!(
        url.scheme() == "https"
            && url.host_str().is_some()
            && url.path() == "/"
            && url.username().is_empty()
            && url.password().is_none()
            && url.query().is_none()
            && url.fragment().is_none(),
        "mobile Server requires an HTTPS origin"
    );
    Ok(url)
}
fn http_client() -> anyhow::Result<reqwest::Client> {
    // The same public CA trust bundle is used for HTTPS and RTSPS on both mobile platforms.
    let roots = rustls::RootCertStore::from_iter(webpki_roots::TLS_SERVER_ROOTS.iter().cloned());
    let config = rustls::ClientConfig::builder()
        .with_root_certificates(roots)
        .with_no_client_auth();
    Ok(reqwest::Client::builder()
        .tls_backend_preconfigured(config)
        .https_only(true)
        .redirect(reqwest::redirect::Policy::none())
        .connect_timeout(Duration::from_secs(5))
        .timeout(Duration::from_secs(12))
        .build()?)
}
async fn response_json<T: for<'de> Deserialize<'de>>(
    mut response: reqwest::Response,
    limit: usize,
) -> anyhow::Result<T> {
    ensure!(
        response.status().is_success(),
        "Server rejected mobile request"
    );
    ensure!(
        response
            .content_length()
            .is_none_or(|length| length <= limit as u64),
        "Server response too large"
    );
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await? {
        ensure!(
            chunk.len() <= limit.saturating_sub(bytes.len()),
            "Server response too large"
        );
        bytes.extend_from_slice(&chunk);
    }
    serde_json::from_slice(&bytes).context("invalid Server response")
}

pub async fn pair(
    server: &str,
    code: &str,
    name: &str,
    installation_id: Uuid,
) -> anyhow::Result<Pairing> {
    let origin = server_origin(server)?;
    validate_name(name)?;
    ensure!(
        !installation_id.is_nil()
            && code.len() == 36
            && code
                .bytes()
                .all(|b| b.is_ascii_lowercase() || b.is_ascii_digit()),
        "invalid mobile authorization code"
    );
    #[derive(Deserialize)]
    #[serde(deny_unknown_fields)]
    struct Response {
        protocol: String,
        client_id: Uuid,
        access_token: String,
    }
    let response = http_client()?.post(origin.join(&format!("{API_PREFIX}/client/pair"))?).json(&json!({"protocol": PROTOCOL, "product": "xcos", "installation_id": installation_id, "name": name.trim(), "client_version": env!("CARGO_PKG_VERSION"), "authorization_code": code})).send().await?;
    let response: Response = response_json(response, 16384).await?;
    ensure!(
        response.protocol == PROTOCOL,
        "incompatible mobile pairing protocol"
    );
    let pairing = Pairing {
        server: origin.into(),
        installation_id,
        instance_id: response.client_id,
        access_token: response.access_token,
        name: name.trim().into(),
    };
    pairing.validate()?;
    Ok(pairing)
}

pub fn snapshot_document(pairing: &Pairing, metadata: &CaptureMetadata) -> anyhow::Result<Value> {
    metadata.validate()?;
    let confirmed = if metadata.active {
        CapabilityStatus::Supported
    } else {
        CapabilityStatus::Unknown
    };
    let capabilities = DeviceCapabilities {
        video: confirmed,
        main_stream: confirmed,
        sub_stream: CapabilityStatus::Unsupported,
        local_recording: CapabilityStatus::Unsupported,
        server_recording: CapabilityStatus::Supported,
        ptz: CapabilityStatus::Unsupported,
        events: CapabilityStatus::Unsupported,
        audio_input: CapabilityStatus::Unsupported,
        audio_output: CapabilityStatus::Unsupported,
    };
    let (adapter, manufacturer) = match metadata.platform {
        MobilePlatform::Android => ("rtsp", "Android"),
        MobilePlatform::Ios => ("rtsp", "Apple"),
    };
    let streams = if metadata.active {
        vec![StreamDescriptor {
            profile: "main".into(),
            video_codec: Some("h264".into()),
            audio_codec: None,
            width: Some(metadata.width),
            height: Some(metadata.height),
            frame_rate: Some(f64::from(metadata.frame_rate)),
        }]
    } else {
        Vec::new()
    };
    Ok(
        json!({ "protocol": PROTOCOL, "command_capacity": 0, "command_results": [], "cameras": [{ "id": pairing.instance_id, "name": pairing.name, "location": "", "adapter_kind": adapter, "identity": DeviceIdentity { manufacturer: Some(manufacturer.into()), model: Some(metadata.model.clone()), ..DeviceIdentity::default() }, "capabilities": capabilities, "streams": streams, "has_sub_stream": false, "enabled": metadata.active, "storage_mode": "server", "status": if metadata.active { "online" } else { "disabled" }, "health_message": null }] }),
    )
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SnapshotResponse {
    protocol: String,
    accepted_at: String,
    publish: Vec<PublishGrant>,
    commands: Vec<Value>,
}
#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PublishGrant {
    camera_id: Uuid,
    profile: String,
    publish_url: String,
}

pub struct MobileClient {
    pub pairing: Pairing,
    client: reqwest::Client,
    publisher: Mutex<Option<PublisherWorker>>,
}
impl MobileClient {
    pub fn open(pairing: Pairing) -> anyhow::Result<Self> {
        pairing.validate()?;
        Ok(Self {
            pairing,
            client: http_client()?,
            publisher: Mutex::new(None),
        })
    }
    /// Send current evidence and reconcile a scoped grant without exposing the URL to native UI.
    pub async fn poll(
        &self,
        metadata: &CaptureMetadata,
        sps: &[u8],
        pps: &[u8],
    ) -> anyhow::Result<Value> {
        let origin = server_origin(&self.pairing.server)?;
        let request = snapshot_document(&self.pairing, metadata)?;
        let response = self
            .client
            .put(origin.join(&format!("{API_PREFIX}/client/snapshot"))?)
            .bearer_auth(&self.pairing.access_token)
            .json(&request)
            .send()
            .await?;
        let response: SnapshotResponse = response_json(response, 65536).await?;
        ensure!(
            response.protocol == PROTOCOL
                && chrono::DateTime::parse_from_rfc3339(&response.accepted_at).is_ok()
                && response.commands.is_empty()
                && response.publish.len() <= 1,
            "incompatible mobile snapshot response"
        );
        let grant = response.publish.first();
        if let Some(grant) = grant {
            ensure!(
                grant.camera_id == self.pairing.instance_id && grant.profile == "main",
                "Server returned another camera grant"
            );
            rtsp_publish::validate_publish_url(&grant.publish_url)?;
        }
        let mut publisher = self
            .publisher
            .lock()
            .map_err(|_| anyhow::anyhow!("mobile publisher unavailable"))?;
        match grant.filter(|_| metadata.active) {
            None => {
                publisher.take();
            }
            Some(grant) if publisher.as_ref().is_none_or(|p| p.task.is_finished()) => {
                publisher.take();
                *publisher = Some(PublisherWorker::start(&grant.publish_url, sps, pps));
            }
            Some(_) => {}
        }
        Ok(
            json!({"publishing": publisher.as_ref().is_some_and(|p| p.connected.load(Ordering::Acquire)), "active": metadata.active}),
        )
    }
    pub fn frame(&self, bytes: &[u8], timestamp_us: u64) -> anyhow::Result<bool> {
        let units = rtsp_publish::annex_b_units(bytes)?;
        let keyframe = units.iter().any(|unit| unit[0] & 31 == 5);
        let publisher = self
            .publisher
            .lock()
            .map_err(|_| anyhow::anyhow!("mobile publisher unavailable"))?;
        let Some(publisher) = publisher.as_ref() else {
            return Ok(false);
        };
        if publisher.task.is_finished() || !publisher.connected.load(Ordering::Acquire) {
            return Ok(false);
        }
        if publisher.needs_keyframe.load(Ordering::Acquire) && !keyframe {
            return Ok(false);
        }
        match publisher.frames.try_send(Frame {
            bytes: bytes.to_vec(),
            timestamp_us,
        }) {
            Ok(()) => {
                if keyframe {
                    publisher.needs_keyframe.store(false, Ordering::Release);
                }
                Ok(true)
            }
            Err(_) => {
                publisher.needs_keyframe.store(true, Ordering::Release);
                Ok(false)
            }
        }
    }
    pub fn stop(&self) {
        if let Ok(mut publisher) = self.publisher.lock() {
            publisher.take();
        }
    }
}
struct Frame {
    bytes: Vec<u8>,
    timestamp_us: u64,
}
struct PublisherWorker {
    frames: mpsc::Sender<Frame>,
    task: tokio::task::AbortHandle,
    connected: Arc<AtomicBool>,
    needs_keyframe: Arc<AtomicBool>,
}
impl Drop for PublisherWorker {
    fn drop(&mut self) {
        self.task.abort();
    }
}
impl PublisherWorker {
    fn start(url: &str, sps: &[u8], pps: &[u8]) -> Self {
        let (frames, mut receiver) = mpsc::channel::<Frame>(4);
        let connected = Arc::new(AtomicBool::new(false));
        let state = connected.clone();
        let needs_keyframe = Arc::new(AtomicBool::new(true));
        let url = url.to_owned();
        let sps = sps.to_vec();
        let pps = pps.to_vec();
        let task = tokio::spawn(async move {
            let Ok(mut publisher) = Publisher::connect(&url, &sps, &pps).await else {
                return;
            };
            state.store(true, Ordering::Release);
            let mut last_timestamp = None;
            while let Some(frame) = receiver.recv().await {
                if last_timestamp.is_some_and(|last| frame.timestamp_us <= last) {
                    continue;
                }
                last_timestamp = Some(frame.timestamp_us);
                if publisher
                    .frame(&frame.bytes, frame.timestamp_us)
                    .await
                    .is_err()
                {
                    break;
                }
            }
            state.store(false, Ordering::Release);
            publisher.close().await;
        });
        Self {
            frames,
            task: task.abort_handle(),
            connected,
            needs_keyframe,
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn mobile_snapshot_matches_edge_v1_and_never_contains_credentials() {
        let pairing = Pairing {
            server: "https://xcoc.example.com".into(),
            installation_id: Uuid::new_v4(),
            instance_id: Uuid::new_v4(),
            access_token: "a".repeat(43),
            name: "phone".into(),
        };
        let mut metadata = CaptureMetadata {
            platform: MobilePlatform::Ios,
            model: "iPhone".into(),
            width: 1280,
            height: 720,
            frame_rate: 30,
            active: true,
        };
        let document = snapshot_document(&pairing, &metadata).unwrap();
        assert_eq!(document["command_capacity"], 0);
        assert_eq!(document["cameras"][0]["adapter_kind"], "rtsp");
        assert_eq!(document["cameras"][0]["capabilities"]["ptz"], "unsupported");
        assert!(!document.to_string().contains(&pairing.access_token));
        metadata.active = false;
        let document = snapshot_document(&pairing, &metadata).unwrap();
        assert_eq!(document["cameras"][0]["status"], "disabled");
        assert_eq!(document["cameras"][0]["streams"], json!([]));
    }
    #[test]
    fn mobile_origin_and_pairing_are_strict() {
        for origin in [
            "http://example.com",
            "https://example.com/api",
            "https://u:p@example.com",
            "https://example.com/?token=x",
        ] {
            assert!(server_origin(origin).is_err());
        }
        assert!(server_origin("https://example.com:8443").is_ok());
    }
}
