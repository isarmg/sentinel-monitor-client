use crate::onvif;
use anyhow::{Context, ensure};
use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{collections::HashSet, process::Stdio, time::Duration};
use tokio::process::Command;
use url::Url;
use uuid::Uuid;

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Camera {
    #[serde(default = "Uuid::new_v4")]
    pub id: Uuid,
    pub name: String,
    #[serde(default)]
    pub location: String,
    #[serde(default)]
    pub manufacturer: Option<String>,
    #[serde(default)]
    pub model: Option<String>,
    #[serde(default = "default_true")]
    pub enabled: bool,
    pub storage_mode: StorageMode,
    pub adapter: AdapterConfig,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum AdapterConfig {
    Rtsp {
        streams: Vec<ConfiguredStream>,
        #[serde(default)]
        username: Option<String>,
        #[serde(default)]
        password: Option<String>,
    },
    Onvif {
        device_service_url: String,
        #[serde(default)]
        username: Option<String>,
        #[serde(default)]
        password: Option<String>,
        #[serde(default)]
        main_profile_token: Option<String>,
        #[serde(default)]
        sub_profile_token: Option<String>,
    },
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct ConfiguredStream {
    pub profile: StreamProfile,
    pub url: String,
}

#[derive(Clone, Copy, Debug, Eq, Hash, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StreamProfile {
    Main,
    Sub,
}

impl StreamProfile {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Main => "main",
            Self::Sub => "sub",
        }
    }
}

#[derive(Clone, Copy, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum StorageMode {
    Client,
    Server,
}

impl StorageMode {
    pub const fn as_str(self) -> &'static str {
        match self {
            Self::Client => "client",
            Self::Server => "server",
        }
    }
}

#[derive(Clone, Debug, Default, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeviceIdentity {
    pub manufacturer: Option<String>,
    pub model: Option<String>,
    pub firmware_version: Option<String>,
    pub serial_number: Option<String>,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct DeviceCapabilities {
    pub video: bool,
    pub main_stream: bool,
    pub sub_stream: bool,
    pub local_recording: bool,
    pub server_recording: bool,
    pub ptz: bool,
    pub events: bool,
    pub audio_input: bool,
    pub audio_output: bool,
}

#[derive(Clone, Debug, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct StreamDescriptor {
    pub profile: String,
    pub video_codec: Option<String>,
    pub audio_codec: Option<String>,
    pub width: Option<u32>,
    pub height: Option<u32>,
    pub frame_rate: Option<f64>,
}

#[derive(Clone)]
pub struct ResolvedStream {
    pub descriptor: StreamDescriptor,
    pub url: String,
}

#[derive(Clone)]
pub enum ControlTarget {
    None,
    Onvif(onvif::ControlTarget),
}

#[derive(Clone)]
pub struct ResolvedDevice {
    pub adapter_kind: &'static str,
    pub identity: DeviceIdentity,
    pub capabilities: DeviceCapabilities,
    pub streams: Vec<ResolvedStream>,
    pub control: ControlTarget,
}

impl ResolvedDevice {
    pub fn stream_url(&self, profile: &str) -> anyhow::Result<String> {
        self.streams
            .iter()
            .find(|stream| stream.descriptor.profile == profile)
            .map(|stream| stream.url.clone())
            .with_context(|| format!("camera has no {profile} stream"))
    }

    pub async fn ptz(&self, command: PtzCommand) -> anyhow::Result<()> {
        match &self.control {
            ControlTarget::Onvif(target) => onvif::ptz(target, command).await,
            ControlTarget::None => anyhow::bail!("camera adapter does not support PTZ"),
        }
    }

    pub async fn probe_streams(&mut self) -> anyhow::Result<()> {
        for stream in &mut self.streams {
            let output = tokio::time::timeout(
                Duration::from_secs(12),
                Command::new("ffprobe")
                    .args([
                        "-v",
                        "error",
                        "-rtsp_transport",
                        "tcp",
                        "-show_entries",
                        "stream=codec_type,codec_name,width,height,r_frame_rate",
                        "-of",
                        "json",
                        &stream.url,
                    ])
                    .stdin(Stdio::null())
                    .kill_on_drop(true)
                    .output(),
            )
            .await
            .context("camera stream probe timed out")??;
            ensure!(output.status.success(), "camera stream probe failed");
            let document: Value = serde_json::from_slice(&output.stdout)
                .context("ffprobe returned invalid stream metadata")?;
            let streams = document
                .get("streams")
                .and_then(Value::as_array)
                .context("ffprobe returned no stream metadata")?;
            let video = streams
                .iter()
                .find(|value| value.get("codec_type").and_then(Value::as_str) == Some("video"))
                .context("camera profile has no video stream")?;
            stream.descriptor.video_codec = bounded_probe_string(video, "codec_name");
            stream.descriptor.width = probe_u32(video, "width", 32768);
            stream.descriptor.height = probe_u32(video, "height", 32768);
            stream.descriptor.frame_rate = video
                .get("r_frame_rate")
                .and_then(Value::as_str)
                .and_then(parse_frame_rate)
                .filter(|value| value.is_finite() && *value > 0.0 && *value <= 240.0);
            stream.descriptor.audio_codec = streams
                .iter()
                .find(|value| value.get("codec_type").and_then(Value::as_str) == Some("audio"))
                .and_then(|value| bounded_probe_string(value, "codec_name"));
        }
        self.capabilities.audio_input = self
            .streams
            .iter()
            .any(|stream| stream.descriptor.audio_codec.is_some());
        Ok(())
    }
}

fn bounded_probe_string(value: &Value, key: &str) -> Option<String> {
    value
        .get(key)
        .and_then(Value::as_str)
        .filter(|value| {
            !value.is_empty() && value.len() <= 64 && !value.chars().any(char::is_control)
        })
        .map(str::to_owned)
}

fn probe_u32(value: &Value, key: &str, maximum: u32) -> Option<u32> {
    value
        .get(key)
        .and_then(Value::as_u64)
        .and_then(|value| u32::try_from(value).ok())
        .filter(|value| *value > 0 && *value <= maximum)
}

fn parse_frame_rate(value: &str) -> Option<f64> {
    let (numerator, denominator) = value.split_once('/')?;
    let numerator = numerator.parse::<f64>().ok()?;
    let denominator = denominator.parse::<f64>().ok()?;
    (denominator != 0.0).then_some(numerator / denominator)
}

#[derive(Clone, Copy, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PtzCommand {
    pub action: PtzAction,
    #[serde(default)]
    pub pan: f64,
    #[serde(default)]
    pub tilt: f64,
    #[serde(default)]
    pub zoom: f64,
}

#[derive(Clone, Copy, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PtzAction {
    Move,
    Stop,
}

#[async_trait]
pub trait DeviceAdapter: Send + Sync {
    async fn resolve(&self, client: &reqwest::Client) -> anyhow::Result<ResolvedDevice>;
}

struct RtspAdapter<'a> {
    camera: &'a Camera,
    streams: &'a [ConfiguredStream],
    username: Option<&'a str>,
    password: Option<&'a str>,
}

#[async_trait]
impl DeviceAdapter for RtspAdapter<'_> {
    async fn resolve(&self, _client: &reqwest::Client) -> anyhow::Result<ResolvedDevice> {
        let streams = self
            .streams
            .iter()
            .map(|stream| {
                let mut url = Url::parse(&stream.url)?;
                if let Some(username) = self.username.filter(|value| !value.is_empty()) {
                    url.set_username(username)
                        .map_err(|_| anyhow::anyhow!("invalid camera username"))?;
                }
                if let Some(password) = self.password {
                    url.set_password(Some(password))
                        .map_err(|_| anyhow::anyhow!("invalid camera password"))?;
                }
                Ok(ResolvedStream {
                    descriptor: StreamDescriptor {
                        profile: stream.profile.as_str().to_owned(),
                        video_codec: None,
                        audio_codec: None,
                        width: None,
                        height: None,
                        frame_rate: None,
                    },
                    url: url.into(),
                })
            })
            .collect::<anyhow::Result<Vec<_>>>()?;
        let has_sub = streams
            .iter()
            .any(|stream| stream.descriptor.profile == "sub");
        Ok(ResolvedDevice {
            adapter_kind: "rtsp",
            identity: DeviceIdentity {
                manufacturer: self.camera.manufacturer.clone(),
                model: self.camera.model.clone(),
                ..DeviceIdentity::default()
            },
            capabilities: DeviceCapabilities {
                video: true,
                main_stream: true,
                sub_stream: has_sub,
                local_recording: true,
                server_recording: true,
                ptz: false,
                events: false,
                audio_input: false,
                audio_output: false,
            },
            streams,
            control: ControlTarget::None,
        })
    }
}

impl Camera {
    pub fn adapter(&self) -> Box<dyn DeviceAdapter + '_> {
        match &self.adapter {
            AdapterConfig::Rtsp {
                streams,
                username,
                password,
            } => Box::new(RtspAdapter {
                camera: self,
                streams,
                username: username.as_deref(),
                password: password.as_deref(),
            }),
            AdapterConfig::Onvif {
                device_service_url,
                username,
                password,
                main_profile_token,
                sub_profile_token,
            } => Box::new(onvif::OnvifAdapter::new(
                device_service_url,
                username.as_deref(),
                password.as_deref(),
                main_profile_token.as_deref(),
                sub_profile_token.as_deref(),
            )),
        }
    }

    pub fn adapter_kind(&self) -> &'static str {
        match self.adapter {
            AdapterConfig::Rtsp { .. } => "rtsp",
            AdapterConfig::Onvif { .. } => "onvif",
        }
    }

    pub fn validate(&self) -> anyhow::Result<()> {
        validate_text(&self.name, 64, "name")?;
        validate_optional_text(Some(&self.location), 128, "location")?;
        validate_optional_text(self.manufacturer.as_deref(), 128, "manufacturer")?;
        validate_optional_text(self.model.as_deref(), 128, "model")?;
        match &self.adapter {
            AdapterConfig::Rtsp { streams, .. } => {
                ensure!(
                    !streams.is_empty() && streams.len() <= 2,
                    "RTSP adapter requires one or two streams"
                );
                let profiles = streams
                    .iter()
                    .map(|stream| stream.profile)
                    .collect::<HashSet<_>>();
                ensure!(
                    profiles.len() == streams.len() && profiles.contains(&StreamProfile::Main),
                    "RTSP streams require one unique main profile"
                );
                for stream in streams {
                    validate_rtsp_url(&stream.url)?;
                }
            }
            AdapterConfig::Onvif {
                device_service_url,
                main_profile_token,
                sub_profile_token,
                ..
            } => {
                let url = Url::parse(device_service_url)?;
                ensure!(
                    matches!(url.scheme(), "http" | "https")
                        && url.host_str().is_some()
                        && url.username().is_empty()
                        && url.password().is_none(),
                    "ONVIF device service must be an HTTP(S) URL without credentials"
                );
                validate_optional_text(main_profile_token.as_deref(), 4096, "main_profile_token")?;
                validate_optional_text(sub_profile_token.as_deref(), 4096, "sub_profile_token")?;
            }
        }
        Ok(())
    }
}

fn validate_rtsp_url(value: &str) -> anyhow::Result<()> {
    let url = Url::parse(value)?;
    ensure!(
        matches!(url.scheme(), "rtsp" | "rtsps") && url.host_str().is_some(),
        "camera stream must be an RTSP(S) URL"
    );
    Ok(())
}

fn validate_text(value: &str, max: usize, field: &str) -> anyhow::Result<()> {
    ensure!(
        !value.trim().is_empty()
            && value.chars().count() <= max
            && !value.chars().any(char::is_control),
        "invalid {field}"
    );
    Ok(())
}

fn validate_optional_text(value: Option<&str>, max: usize, field: &str) -> anyhow::Result<()> {
    if let Some(value) = value {
        ensure!(
            value.chars().count() <= max && !value.chars().any(char::is_control),
            "invalid {field}"
        );
    }
    Ok(())
}

const fn default_true() -> bool {
    true
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn adapter_contract_requires_one_main_stream_and_rejects_vendor_fields() {
        let valid: Camera = serde_json::from_value(serde_json::json!({
            "name": "front",
            "storage_mode": "server",
            "adapter": {
                "kind": "rtsp",
                "streams": [{"profile": "main", "url": "rtsp://camera/main"}]
            }
        }))
        .unwrap();
        valid.validate().unwrap();

        let no_main: Camera = serde_json::from_value(serde_json::json!({
            "name": "front",
            "storage_mode": "server",
            "adapter": {
                "kind": "rtsp",
                "streams": [{"profile": "sub", "url": "rtsp://camera/sub"}]
            }
        }))
        .unwrap();
        assert!(no_main.validate().is_err());
        assert!(
            serde_json::from_value::<Camera>(serde_json::json!({
                "name": "front",
                "storage_mode": "server",
                "adapter": {"kind": "vendor_cloud", "token": "secret"}
            }))
            .is_err()
        );
    }

    #[test]
    fn onvif_configuration_uses_a_device_service_url() {
        let camera: Camera = serde_json::from_value(serde_json::json!({
            "name": "ptz",
            "storage_mode": "client",
            "adapter": {
                "kind": "onvif",
                "device_service_url": "http://192.0.2.20/onvif/device_service",
                "username": "operator",
                "password": "secret"
            }
        }))
        .unwrap();
        camera.validate().unwrap();
        assert_eq!(camera.adapter_kind(), "onvif");
    }
}
