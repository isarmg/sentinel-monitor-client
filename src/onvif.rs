use crate::device::{
    CapabilityStatus, ControlTarget as DeviceControlTarget, DeviceAdapter, DeviceCapabilities,
    DeviceIdentity, PtzAction, PtzCommand, ResolvedDevice, ResolvedStream, StreamDescriptor,
};
use anyhow::{Context, ensure};
use async_trait::async_trait;
use base64::{Engine as _, engine::general_purpose::STANDARD};
use chrono::{SecondsFormat, Utc};
use rand::{TryRng, rngs::SysRng};
use reqwest::header::CONTENT_TYPE;
use serde::Serialize;
use sha1::{Digest, Sha1};
use std::{collections::HashSet, net::SocketAddr, time::Duration};
use tokio::{net::UdpSocket, time};
use url::Url;
use uuid::Uuid;
use xcsc_secure_xml::Document;

const MAX_XML_BYTES: usize = 1024 * 1024;
const MAX_DISCOVERY_RESULTS: usize = 256;
const MAX_DISCOVERY_XADDRS: usize = 8;
const MAX_DISCOVERY_SCOPES: usize = 32;
const SOAP_TIMEOUT: Duration = Duration::from_secs(10);

pub struct OnvifAdapter<'a> {
    device_service_url: &'a str,
    username: Option<&'a str>,
    password: Option<&'a str>,
    main_profile_token: Option<&'a str>,
    sub_profile_token: Option<&'a str>,
}

impl<'a> OnvifAdapter<'a> {
    pub fn new(
        device_service_url: &'a str,
        username: Option<&'a str>,
        password: Option<&'a str>,
        main_profile_token: Option<&'a str>,
        sub_profile_token: Option<&'a str>,
    ) -> Self {
        Self {
            device_service_url,
            username,
            password,
            main_profile_token,
            sub_profile_token,
        }
    }
}

#[derive(Clone)]
pub struct ControlTarget {
    client: reqwest::Client,
    ptz_url: Url,
    username: Option<String>,
    password: Option<String>,
    profile_token: String,
}

impl ControlTarget {
    pub fn new(
        client: reqwest::Client,
        endpoint: Url,
        profile_token: String,
    ) -> anyhow::Result<Self> {
        validate_http_url(endpoint.as_str())?;
        validate_reported_text(Some(&profile_token), 4096, "profile_token")?;
        Ok(Self {
            client,
            ptz_url: endpoint,
            username: None,
            password: None,
            profile_token,
        })
    }
}

#[derive(Clone, Debug, Serialize)]
pub struct DiscoveredDevice {
    pub endpoint: String,
    pub xaddrs: Vec<String>,
    pub scopes: Vec<String>,
    pub remote_addr: String,
}

#[derive(Clone)]
struct MediaProfile {
    token: String,
    video_codec: Option<String>,
    audio_codec: Option<String>,
    width: Option<u32>,
    height: Option<u32>,
    frame_rate: Option<f64>,
    ptz: bool,
}

#[async_trait]
impl DeviceAdapter for OnvifAdapter<'_> {
    async fn resolve(&self, client: &reqwest::Client) -> anyhow::Result<ResolvedDevice> {
        let device_url = validate_http_url(self.device_service_url)?;
        let information = soap_request(
            client,
            &device_url,
            "http://www.onvif.org/ver10/device/wsdl/GetDeviceInformation",
            r#"<tds:GetDeviceInformation xmlns:tds="http://www.onvif.org/ver10/device/wsdl"/>"#,
            self.username,
            self.password,
        )
        .await?;
        let identity = parse_identity(&information)?;
        let capability_xml = soap_request(
            client,
            &device_url,
            "http://www.onvif.org/ver10/device/wsdl/GetCapabilities",
            r#"<tds:GetCapabilities xmlns:tds="http://www.onvif.org/ver10/device/wsdl"><tds:Category>All</tds:Category></tds:GetCapabilities>"#,
            self.username,
            self.password,
        )
        .await?;
        let media_url = capability_url(&capability_xml, "Media", &device_url)?
            .context("ONVIF device did not report a Media service")?;
        let ptz_url = capability_url(&capability_xml, "PTZ", &device_url)?;
        let profiles_xml = soap_request(
            client,
            &media_url,
            "http://www.onvif.org/ver10/media/wsdl/GetProfiles",
            r#"<trt:GetProfiles xmlns:trt="http://www.onvif.org/ver10/media/wsdl"/>"#,
            self.username,
            self.password,
        )
        .await?;
        let profiles = parse_profiles(&profiles_xml)?;
        ensure!(!profiles.is_empty(), "ONVIF device has no media profiles");
        let selected = select_profiles(&profiles, self.main_profile_token, self.sub_profile_token)?;
        let main_profile = selected[0].1;
        let mut streams = Vec::with_capacity(selected.len());
        for (role, profile) in selected {
            let stream_uri = get_stream_uri(
                client,
                &media_url,
                &profile.token,
                self.username,
                self.password,
            )
            .await?;
            streams.push(ResolvedStream {
                descriptor: StreamDescriptor {
                    profile: role.to_owned(),
                    video_codec: profile.video_codec.clone(),
                    audio_codec: profile.audio_codec.clone(),
                    width: profile.width,
                    height: profile.height,
                    frame_rate: profile.frame_rate,
                },
                source: crate::device::MediaSource::Rtsp(with_credentials(
                    stream_uri,
                    self.username,
                    self.password,
                )?),
            });
        }
        let has_sub = streams
            .iter()
            .any(|stream| stream.descriptor.profile == "sub");
        let audio_input = streams
            .iter()
            .any(|stream| stream.descriptor.audio_codec.is_some());
        let ptz_supported = ptz_url.is_some() && main_profile.ptz;
        let control = match ptz_url {
            Some(ptz_url) if ptz_supported => DeviceControlTarget::Onvif(ControlTarget {
                client: client.clone(),
                ptz_url,
                username: self.username.map(str::to_owned),
                password: self.password.map(str::to_owned),
                profile_token: main_profile.token.clone(),
            }),
            None => DeviceControlTarget::None,
            Some(_) => DeviceControlTarget::None,
        };
        Ok(ResolvedDevice {
            adapter_kind: "onvif",
            identity,
            capabilities: DeviceCapabilities {
                video: CapabilityStatus::Supported,
                main_stream: CapabilityStatus::Supported,
                sub_stream: CapabilityStatus::confirmed(has_sub),
                local_recording: CapabilityStatus::Supported,
                server_recording: CapabilityStatus::Supported,
                ptz: CapabilityStatus::confirmed(ptz_supported),
                events: CapabilityStatus::Unknown,
                audio_input: if audio_input {
                    CapabilityStatus::Supported
                } else {
                    CapabilityStatus::Unknown
                },
                audio_output: CapabilityStatus::Unknown,
            },
            streams,
            control,
        })
    }
}

pub async fn ptz(target: &ControlTarget, command: PtzCommand) -> anyhow::Result<()> {
    ensure!(
        [command.pan, command.tilt, command.zoom]
            .iter()
            .all(|value| (-1.0..=1.0).contains(value)),
        "PTZ velocity must be between -1 and 1"
    );
    let token = xml_escape(&target.profile_token);
    let (action, body) = match command.action {
        PtzAction::Stop => (
            "http://www.onvif.org/ver20/ptz/wsdl/Stop",
            format!(
                r#"<tptz:Stop xmlns:tptz="http://www.onvif.org/ver20/ptz/wsdl"><tptz:ProfileToken>{token}</tptz:ProfileToken><tptz:PanTilt>true</tptz:PanTilt><tptz:Zoom>true</tptz:Zoom></tptz:Stop>"#
            ),
        ),
        PtzAction::Move => (
            "http://www.onvif.org/ver20/ptz/wsdl/ContinuousMove",
            format!(
                r#"<tptz:ContinuousMove xmlns:tptz="http://www.onvif.org/ver20/ptz/wsdl" xmlns:tt="http://www.onvif.org/ver10/schema"><tptz:ProfileToken>{token}</tptz:ProfileToken><tptz:Velocity><tt:PanTilt x="{}" y="{}"/><tt:Zoom x="{}"/></tptz:Velocity><tptz:Timeout>PT5S</tptz:Timeout></tptz:ContinuousMove>"#,
                command.pan, command.tilt, command.zoom
            ),
        ),
    };
    let xml = soap_request(
        &target.client,
        &target.ptz_url,
        action,
        &body,
        target.username.as_deref(),
        target.password.as_deref(),
    )
    .await?;
    let expected = match command.action {
        PtzAction::Move => "ContinuousMoveResponse",
        PtzAction::Stop => "StopResponse",
    };
    validate_ptz_response(&xml, expected)?;
    Ok(())
}

fn validate_ptz_response(xml: &str, expected: &str) -> anyhow::Result<()> {
    let document = parse_xml(xml)?;
    let body = document
        .descendants()
        .find(|node| {
            node.is_element()
                && node.tag_name().name() == "Body"
                && node.tag_name().namespace() == Some("http://www.w3.org/2003/05/soap-envelope")
        })
        .context("PTZ response has no SOAP body")?;
    let responses = body
        .children()
        .filter(|node| node.is_element())
        .collect::<Vec<_>>();
    ensure!(
        responses.len() == 1
            && responses[0].tag_name().name() == expected
            && responses[0].tag_name().namespace() == Some("http://www.onvif.org/ver20/ptz/wsdl"),
        "PTZ response did not acknowledge the requested operation"
    );
    Ok(())
}

#[derive(Debug)]
pub struct DiscoveryLimit;
impl std::fmt::Display for DiscoveryLimit {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("ONVIF discovery result limit exceeded")
    }
}
impl std::error::Error for DiscoveryLimit {}

fn admit_discovered_device(
    device: DiscoveredDevice,
    devices: &mut Vec<DiscoveredDevice>,
    seen: &mut HashSet<String>,
) -> anyhow::Result<()> {
    if seen.contains(&device.endpoint) {
        return Ok(());
    }
    if devices.len() >= MAX_DISCOVERY_RESULTS {
        return Err(DiscoveryLimit.into());
    }
    seen.insert(device.endpoint.clone());
    devices.push(device);
    Ok(())
}

pub async fn discover(timeout: Duration) -> anyhow::Result<Vec<DiscoveredDevice>> {
    let socket = UdpSocket::bind("0.0.0.0:0").await?;
    socket.set_broadcast(true)?;
    let message_id = Uuid::new_v4();
    let probe = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?><e:Envelope xmlns:e="http://www.w3.org/2003/05/soap-envelope" xmlns:w="http://schemas.xmlsoap.org/ws/2004/08/addressing" xmlns:d="http://schemas.xmlsoap.org/ws/2005/04/discovery" xmlns:dn="http://www.onvif.org/ver10/network/wsdl"><e:Header><w:MessageID>uuid:{message_id}</w:MessageID><w:To e:mustUnderstand="true">urn:schemas-xmlsoap-org:ws:2005:04:discovery</w:To><w:Action e:mustUnderstand="true">http://schemas.xmlsoap.org/ws/2005/04/discovery/Probe</w:Action></e:Header><e:Body><d:Probe><d:Types>dn:NetworkVideoTransmitter</d:Types></d:Probe></e:Body></e:Envelope>"#
    );
    socket
        .send_to(probe.as_bytes(), "239.255.255.250:3702")
        .await?;
    let deadline = time::Instant::now() + timeout;
    let mut buffer = vec![0_u8; 64 * 1024];
    let mut devices = Vec::new();
    let mut seen = HashSet::new();
    loop {
        let remaining = deadline.saturating_duration_since(time::Instant::now());
        if remaining.is_zero() {
            break;
        }
        let Ok(Ok((length, remote))) =
            time::timeout(remaining, socket.recv_from(&mut buffer)).await
        else {
            break;
        };
        if let Some(device) = parse_probe_response(&buffer[..length], remote, message_id)
            && let Err(error) = admit_discovered_device(device, &mut devices, &mut seen)
        {
            xcss_log::LogRecord::server(
                "xcoc",
                "runtime",
                "xcoc.discovery.limit_reached",
                "Device discovery stopped at its result limit.",
                xcss_log::Level::Warn,
            )?
            .with_error_code("discovery_limit_exceeded")?
            .emit()?;
            return Err(error);
        }
    }
    Ok(devices)
}

async fn get_stream_uri(
    client: &reqwest::Client,
    media_url: &Url,
    token: &str,
    username: Option<&str>,
    password: Option<&str>,
) -> anyhow::Result<Url> {
    let token = xml_escape(token);
    let body = format!(
        r#"<trt:GetStreamUri xmlns:trt="http://www.onvif.org/ver10/media/wsdl" xmlns:tt="http://www.onvif.org/ver10/schema"><trt:StreamSetup><tt:Stream>RTP-Unicast</tt:Stream><tt:Transport><tt:Protocol>RTSP</tt:Protocol></tt:Transport></trt:StreamSetup><trt:ProfileToken>{token}</trt:ProfileToken></trt:GetStreamUri>"#
    );
    let xml = soap_request(
        client,
        media_url,
        "http://www.onvif.org/ver10/media/wsdl/GetStreamUri",
        &body,
        username,
        password,
    )
    .await?;
    let document = parse_xml(&xml)?;
    let uri = text(&document, "Uri").context("ONVIF stream response has no URI")?;
    let url = Url::parse(&uri)?;
    ensure!(
        matches!(url.scheme(), "rtsp" | "rtsps") && url.host_str().is_some(),
        "ONVIF returned a non-RTSP stream URI"
    );
    Ok(url)
}

async fn soap_request(
    client: &reqwest::Client,
    target: &Url,
    action: &str,
    body: &str,
    username: Option<&str>,
    password: Option<&str>,
) -> anyhow::Result<String> {
    let security = username
        .filter(|username| !username.is_empty())
        .map(|username| ws_security_header(username, password.unwrap_or("")))
        .transpose()?
        .unwrap_or_default();
    let envelope = format!(
        r#"<?xml version="1.0" encoding="UTF-8"?><s:Envelope xmlns:s="http://www.w3.org/2003/05/soap-envelope"><s:Header>{security}</s:Header><s:Body>{body}</s:Body></s:Envelope>"#
    );
    let response = time::timeout(
        SOAP_TIMEOUT,
        client
            .post(target.clone())
            .header(
                CONTENT_TYPE,
                format!("application/soap+xml; charset=utf-8; action=\"{action}\""),
            )
            .body(envelope)
            .send(),
    )
    .await
    .context("ONVIF request timed out")?
    .context("ONVIF request failed")?;
    ensure!(
        response.status().is_success(),
        "ONVIF request rejected with HTTP {}",
        response.status().as_u16()
    );
    if response
        .content_length()
        .is_some_and(|length| length > MAX_XML_BYTES as u64)
    {
        anyhow::bail!("ONVIF response exceeds size limit");
    }
    let mut response = response;
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.context("read ONVIF response")? {
        append_response_chunk(&mut bytes, &chunk)?;
    }
    let xml = String::from_utf8(bytes).context("ONVIF response is not UTF-8")?;
    validate_soap_response(&xml)?;
    Ok(xml)
}

fn validate_soap_response(xml: &str) -> anyhow::Result<()> {
    let document = parse_xml(xml)?;
    ensure!(
        !document
            .descendants()
            .any(|node| node.is_element() && node.tag_name().name() == "Fault"),
        "ONVIF device rejected the SOAP request"
    );
    Ok(())
}

fn append_response_chunk(bytes: &mut Vec<u8>, chunk: &[u8]) -> anyhow::Result<()> {
    ensure!(
        bytes
            .len()
            .checked_add(chunk.len())
            .is_some_and(|length| length <= MAX_XML_BYTES),
        "ONVIF response exceeds size limit"
    );
    bytes.extend_from_slice(chunk);
    Ok(())
}

fn parse_identity(xml: &str) -> anyhow::Result<DeviceIdentity> {
    let document = parse_xml(xml)?;
    let identity = DeviceIdentity {
        manufacturer: text(&document, "Manufacturer"),
        model: text(&document, "Model"),
        firmware_version: text(&document, "FirmwareVersion"),
        serial_number: text(&document, "SerialNumber"),
    };
    validate_reported_text(identity.manufacturer.as_deref(), 128, "manufacturer")?;
    validate_reported_text(identity.model.as_deref(), 128, "model")?;
    validate_reported_text(
        identity.firmware_version.as_deref(),
        128,
        "firmware version",
    )?;
    validate_reported_text(identity.serial_number.as_deref(), 256, "serial number")?;
    Ok(identity)
}

fn capability_url(
    xml: &str,
    service: &str,
    registered_device: &Url,
) -> anyhow::Result<Option<Url>> {
    let document = parse_xml(xml)?;
    let raw = document
        .descendants()
        .filter(|node| node.is_element() && node.tag_name().name() == service)
        .find_map(|node| {
            node.attribute("XAddr").map(str::to_owned).or_else(|| {
                node.descendants()
                    .find(|child| child.is_element() && child.tag_name().name() == "XAddr")
                    .and_then(|child| child.text())
                    .map(str::to_owned)
            })
        });
    let Some(raw) = raw else { return Ok(None) };
    let url = validate_http_url(raw.trim())?;
    ensure!(
        url.host_str() == registered_device.host_str(),
        "ONVIF {service} service changed the configured device host"
    );
    Ok(Some(url))
}

fn parse_profiles(xml: &str) -> anyhow::Result<Vec<MediaProfile>> {
    let document = parse_xml(xml)?;
    let mut profiles = Vec::new();
    for profile in document
        .descendants()
        .filter(|node| node.is_element() && node.tag_name().name() == "Profiles")
    {
        let Some(video) = profile.descendants().find(|node| {
            node.is_element() && node.tag_name().name() == "VideoEncoderConfiguration"
        }) else {
            continue;
        };
        let Some(token) = profile.attribute("token") else {
            continue;
        };
        ensure!(
            !token.is_empty() && token.len() <= 4096,
            "invalid ONVIF profile token"
        );
        let value = |name: &str| {
            video
                .descendants()
                .find(|node| node.is_element() && node.tag_name().name() == name)
                .and_then(|node| node.text())
                .map(str::trim)
                .map(str::to_owned)
        };
        let video_codec = value("Encoding");
        validate_reported_text(video_codec.as_deref(), 64, "video codec")?;
        let audio_codec = profile
            .descendants()
            .find(|node| node.is_element() && node.tag_name().name() == "AudioEncoderConfiguration")
            .and_then(|node| {
                node.descendants()
                    .find(|child| child.is_element() && child.tag_name().name() == "Encoding")
            })
            .and_then(|node| node.text())
            .map(str::trim)
            .map(str::to_owned);
        validate_reported_text(audio_codec.as_deref(), 64, "audio codec")?;
        let width = value("Width").and_then(|value| value.parse().ok());
        let height = value("Height").and_then(|value| value.parse().ok());
        let frame_rate = value("FrameRateLimit").and_then(|value| value.parse().ok());
        ensure!(
            width.is_none_or(|value: u32| value > 0 && value <= 32768)
                && height.is_none_or(|value: u32| value > 0 && value <= 32768)
                && frame_rate.is_none_or(|value: f64| {
                    value.is_finite() && value > 0.0 && value <= 240.0
                }),
            "ONVIF media profile is outside supported limits"
        );
        profiles.push(MediaProfile {
            token: token.to_owned(),
            video_codec,
            audio_codec,
            width,
            height,
            frame_rate,
            ptz: profile
                .descendants()
                .any(|node| node.is_element() && node.tag_name().name() == "PTZConfiguration"),
        });
    }
    Ok(profiles)
}

fn select_profiles<'a>(
    profiles: &'a [MediaProfile],
    main: Option<&str>,
    sub: Option<&str>,
) -> anyhow::Result<Vec<(&'static str, &'a MediaProfile)>> {
    let by_token = |token: &str| profiles.iter().find(|profile| profile.token == token);
    let main_profile = match main {
        Some(token) => by_token(token)
            .with_context(|| format!("ONVIF main profile token {token} was not found"))?,
        None => profiles
            .iter()
            .max_by_key(|profile| {
                u64::from(profile.width.unwrap_or(0)) * u64::from(profile.height.unwrap_or(0))
            })
            .context("ONVIF device has no profiles")?,
    };
    let sub_profile = match sub {
        Some(token) => Some(
            by_token(token)
                .with_context(|| format!("ONVIF sub profile token {token} was not found"))?,
        ),
        None if profiles.len() > 1 => profiles
            .iter()
            .filter(|profile| profile.token != main_profile.token)
            .min_by_key(|profile| {
                u64::from(profile.width.unwrap_or(u32::MAX))
                    * u64::from(profile.height.unwrap_or(u32::MAX))
            }),
        None => None,
    };
    let mut selected = vec![("main", main_profile)];
    if let Some(sub_profile) = sub_profile {
        ensure!(
            sub_profile.token != main_profile.token,
            "main and sub ONVIF profiles must differ"
        );
        selected.push(("sub", sub_profile));
    }
    Ok(selected)
}

fn parse_probe_response(
    bytes: &[u8],
    remote: SocketAddr,
    message_id: Uuid,
) -> Option<DiscoveredDevice> {
    let xml = std::str::from_utf8(bytes).ok()?;
    let document = parse_xml(xml).ok()?;
    if text(&document, "RelatesTo")? != format!("uuid:{message_id}") {
        return None;
    }
    let xaddrs_text = text(&document, "XAddrs")?;
    let mut xaddrs = Vec::new();
    for address in xaddrs_text.split_whitespace() {
        if xaddrs.len() >= MAX_DISCOVERY_XADDRS || address.len() > 2048 {
            return None;
        }
        let url = validate_http_url(address).ok()?;
        xaddrs.push(url.to_string());
    }
    if xaddrs.is_empty() {
        return None;
    }
    let endpoint = text(&document, "Address").unwrap_or_else(|| xaddrs[0].clone());
    if endpoint.len() > 2048 || endpoint.chars().any(char::is_control) {
        return None;
    }
    let mut scopes = Vec::new();
    if let Some(value) = text(&document, "Scopes") {
        for scope in value.split_whitespace() {
            if scopes.len() >= MAX_DISCOVERY_SCOPES || scope.len() > 2048 {
                return None;
            }
            scopes.push(scope.to_owned());
        }
    }
    Some(DiscoveredDevice {
        endpoint,
        xaddrs,
        scopes,
        remote_addr: remote.to_string(),
    })
}

fn text(document: &Document<'_>, name: &str) -> Option<String> {
    document
        .descendants()
        .find(|node| node.is_element() && node.tag_name().name() == name)
        .and_then(|node| node.text())
        .map(str::trim)
        .filter(|value| !value.is_empty())
        .map(str::to_owned)
}

fn parse_xml(xml: &str) -> anyhow::Result<Document<'_>> {
    xcsc_secure_xml::parse_bounded(
        xml,
        xcsc_secure_xml::XmlBudget {
            max_bytes: MAX_XML_BYTES,
            max_depth: 32,
            max_nodes: 4096,
            max_text_bytes: 256 * 1024,
            max_text_node_bytes: 64 * 1024,
            max_parse_time: SOAP_TIMEOUT,
        },
    )
    .map_err(|error| anyhow::anyhow!("invalid or oversized ONVIF XML: {error}"))
}

fn validate_http_url(value: &str) -> anyhow::Result<Url> {
    let url = Url::parse(value)?;
    ensure!(
        matches!(url.scheme(), "http" | "https")
            && url.host_str().is_some()
            && url.username().is_empty()
            && url.password().is_none(),
        "invalid ONVIF service URL"
    );
    Ok(url)
}

fn validate_reported_text(value: Option<&str>, max: usize, field: &str) -> anyhow::Result<()> {
    if let Some(value) = value {
        ensure!(
            !value.is_empty()
                && value.chars().count() <= max
                && !value.chars().any(char::is_control),
            "ONVIF reported an invalid {field}"
        );
    }
    Ok(())
}

fn with_credentials(
    mut url: Url,
    username: Option<&str>,
    password: Option<&str>,
) -> anyhow::Result<String> {
    if let Some(username) = username.filter(|value| !value.is_empty()) {
        url.set_username(username)
            .map_err(|_| anyhow::anyhow!("invalid camera username"))?;
    }
    if let Some(password) = password {
        url.set_password(Some(password))
            .map_err(|_| anyhow::anyhow!("invalid camera password"))?;
    }
    Ok(url.into())
}

fn ws_security_header(username: &str, password: &str) -> anyhow::Result<String> {
    let mut nonce = [0_u8; 16];
    SysRng
        .try_fill_bytes(&mut nonce)
        .map_err(|_| anyhow::anyhow!("system entropy unavailable"))?;
    let created = Utc::now().to_rfc3339_opts(SecondsFormat::Millis, true);
    let mut digest = Sha1::new();
    digest.update(nonce);
    digest.update(created.as_bytes());
    digest.update(password.as_bytes());
    let password_digest = STANDARD.encode(digest.finalize());
    let nonce = STANDARD.encode(nonce);
    Ok(format!(
        r#"<wsse:Security s:mustUnderstand="1" xmlns:wsse="http://docs.oasis-open.org/wss/2004/01/oasis-200401-wss-wssecurity-secext-1.0.xsd" xmlns:wsu="http://docs.oasis-open.org/wss/2004/01/oasis-200401-wss-wssecurity-utility-1.0.xsd"><wsse:UsernameToken><wsse:Username>{}</wsse:Username><wsse:Password Type="http://docs.oasis-open.org/wss/2004/01/oasis-200401-wss-username-token-profile-1.0#PasswordDigest">{password_digest}</wsse:Password><wsse:Nonce EncodingType="http://docs.oasis-open.org/wss/2004/01/oasis-200401-wss-soap-message-security-1.0#Base64Binary">{nonce}</wsse:Nonce><wsu:Created>{created}</wsu:Created></wsse:UsernameToken></wsse:Security>"#,
        xml_escape(username)
    ))
}

fn xml_escape(value: &str) -> String {
    value
        .replace('&', "&amp;")
        .replace('<', "&lt;")
        .replace('>', "&gt;")
        .replace('"', "&quot;")
        .replace('\'', "&apos;")
}

#[cfg(test)]
#[path = "onvif/tests.rs"]
mod tests;
