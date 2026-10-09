//! Local, documented RTSP families. Model names are examples, not hardware certifications.
use crate::device::{ConfiguredStream, StreamProfile};
use anyhow::{Context, ensure};
use serde::{Deserialize, Serialize};
use url::Url;

#[derive(Clone, Copy, Deserialize, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum CameraPreset {
    Hikvision,
    Dahua,
    Uniview,
    Axis,
    Reolink,
    Tapo,
}

#[derive(Serialize)]
pub struct PresetInfo {
    pub preset: CameraPreset,
    pub manufacturer: &'static str,
    pub examples: &'static [&'static str],
    pub documentation: &'static str,
}

pub fn catalog() -> Vec<PresetInfo> {
    vec![
        PresetInfo {
            preset: CameraPreset::Hikvision,
            manufacturer: "Hikvision",
            examples: &["DS-2CD RTSP series", "DS-2DE RTSP series"],
            documentation: "https://supportusa.hikvision.com/a/solutions/articles/17000129064",
        },
        PresetInfo {
            preset: CameraPreset::Dahua,
            manufacturer: "Dahua",
            examples: &["IPC-HFW RTSP series", "IPC-HDW RTSP series"],
            documentation: "https://www.dahuasecurity.com/asset/upload/uploads/soft/20191107/4-Dahua-Network-Camera-Web-3.0-Operation-Manual_V2.0.11.pdf",
        },
        PresetInfo {
            preset: CameraPreset::Uniview,
            manufacturer: "Uniview",
            examples: &["IPC RTSP series"],
            documentation: "https://www.uniview.com/res/202310/26/20231026_1890310_How%20to%20Get%20a%20Uniview%20Camera%27s%20RTSP%20Stream_974039_168459_0.pdf",
        },
        PresetInfo {
            preset: CameraPreset::Axis,
            manufacturer: "Axis",
            examples: &["AXIS OS network cameras"],
            documentation: "https://developer.axis.com/video-streaming-and-recording/video-streaming/reference/rtsp-endpoints/",
        },
        PresetInfo {
            preset: CameraPreset::Reolink,
            manufacturer: "Reolink",
            examples: &["RLC RTSP series", "RTSP-enabled E1 series"],
            documentation: "https://support.reolink.com/articles/900000630706-Introduction-to-RTSP/",
        },
        PresetInfo {
            preset: CameraPreset::Tapo,
            manufacturer: "TP-Link Tapo",
            examples: &["C100", "C200", "C210", "C310"],
            documentation: "https://www.tp-link.com/us/support/faq/2680/",
        },
    ]
}

impl CameraPreset {
    pub fn manufacturer(self) -> &'static str {
        match self {
            Self::Hikvision => "Hikvision",
            Self::Dahua => "Dahua",
            Self::Uniview => "Uniview",
            Self::Axis => "Axis",
            Self::Reolink => "Reolink",
            Self::Tapo => "TP-Link Tapo",
        }
    }

    pub fn streams(
        self,
        host: &str,
        port: u16,
        channel: u16,
        sub_stream: bool,
    ) -> anyhow::Result<Vec<ConfiguredStream>> {
        ensure!(
            (1..=256).contains(&channel) && port > 0,
            "invalid camera channel or port"
        );
        // set_host accepts DNS names, IPv4 and bracketed IPv6, never URL userinfo or paths.
        ensure!(
            !host.is_empty()
                && host.len() <= 253
                && (!host.contains(':')
                    || (host.starts_with('[')
                        && host.ends_with(']')
                        && host[1..host.len() - 1]
                            .parse::<std::net::Ipv6Addr>()
                            .is_ok()))
                && !host
                    .chars()
                    .any(|c| c.is_whitespace() || c.is_control() || "/\\@?#%".contains(c)),
            "invalid camera host"
        );
        let mut base = Url::parse("rtsp://localhost")?;
        base.set_host(Some(host)).context("invalid camera host")?;
        base.set_port(Some(port))
            .map_err(|_| anyhow::anyhow!("invalid camera port"))?;
        if matches!(self, Self::Tapo) {
            ensure!(
                channel == 1,
                "Tapo preset supports channel 1 only; use explicit RTSP paths for multi-lens cameras"
            );
        }
        if matches!(self, Self::Reolink) {
            ensure!(channel <= 99, "Reolink channel must be between 1 and 99");
        }
        let mut streams = Vec::new();
        for profile in [StreamProfile::Main, StreamProfile::Sub] {
            let sub = profile == StreamProfile::Sub;
            if sub && !sub_stream {
                continue;
            }
            let mut url = base.clone();
            match self {
                Self::Hikvision => url.set_path(&format!(
                    "/Streaming/Channels/{}",
                    u32::from(channel) * 100 + if sub { 2 } else { 1 }
                )),
                Self::Dahua => {
                    url.set_path("/cam/realmonitor");
                    url.query_pairs_mut()
                        .append_pair("channel", &channel.to_string())
                        .append_pair("subtype", if sub { "1" } else { "0" });
                }
                Self::Uniview => {
                    ensure!(
                        channel == 1,
                        "Uniview IPC preset supports channel 1 only; use explicit RTSP paths for NVRs"
                    );
                    url.set_path(if sub {
                        "/media/video2"
                    } else {
                        "/media/video1"
                    });
                }
                Self::Axis => {
                    url.set_path("/axis-media/media.amp");
                    url.query_pairs_mut()
                        .append_pair("camera", &channel.to_string())
                        .append_pair("videocodec", "h264");
                    if sub {
                        url.query_pairs_mut().append_pair("resolution", "640x360");
                    }
                }
                Self::Reolink => url.set_path(&format!(
                    "/Preview_{channel:02}_{}",
                    if sub { "sub" } else { "main" }
                )),
                Self::Tapo => url.set_path(if sub { "/stream2" } else { "/stream1" }),
            }
            streams.push(ConfiguredStream {
                profile,
                url: url.into(),
            });
        }
        Ok(streams)
    }
}

pub const fn default_port() -> u16 {
    554
}
pub const fn default_channel() -> u16 {
    1
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn documented_paths_and_channel_numbering() {
        let cases = [
            (
                CameraPreset::Hikvision,
                "/Streaming/Channels/101",
                "/Streaming/Channels/102",
            ),
            (
                CameraPreset::Dahua,
                "/cam/realmonitor?channel=1&subtype=0",
                "/cam/realmonitor?channel=1&subtype=1",
            ),
            (CameraPreset::Uniview, "/media/video1", "/media/video2"),
            (CameraPreset::Reolink, "/Preview_01_main", "/Preview_01_sub"),
            (CameraPreset::Tapo, "/stream1", "/stream2"),
        ];
        for (preset, main, sub) in cases {
            let streams = preset.streams("192.0.2.10", 554, 1, true).unwrap();
            assert_eq!(streams[0].url, format!("rtsp://192.0.2.10:554{main}"));
            assert_eq!(streams[1].url, format!("rtsp://192.0.2.10:554{sub}"));
        }
        assert!(
            CameraPreset::Hikvision
                .streams("[::1]", 8554, 17, false)
                .unwrap()[0]
                .url
                .ends_with("/1701")
        );
        assert!(
            CameraPreset::Reolink
                .streams("camera", 554, 2, false)
                .unwrap()[0]
                .url
                .ends_with("Preview_02_main")
        );
    }
    #[test]
    fn rejects_url_injection_and_unsupported_channels() {
        for host in [
            "user@camera",
            "camera/path",
            "camera:554",
            "camera?x=1",
            "camera\n",
            "",
        ] {
            assert!(
                CameraPreset::Dahua.streams(host, 554, 1, true).is_err(),
                "{host}"
            );
        }
        assert!(CameraPreset::Tapo.streams("camera", 554, 2, false).is_err());
        assert!(CameraPreset::Dahua.streams("camera", 0, 0, true).is_err());
    }
}
