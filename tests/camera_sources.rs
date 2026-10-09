use xcoc::device::{AdapterConfig, Camera, MediaSource};

#[test]
fn new_templates_roundtrip_and_report_existing_rtsp_adapter() {
    for input in [
        include_str!("../config/camera-preset.json.example"),
        include_str!("../config/camera-local-linux.json.example"),
        include_str!("../config/camera-local-windows.json.example"),
        include_str!("../config/camera-local-macos.json.example"),
    ] {
        let camera: Camera = serde_json::from_str(input).unwrap();
        assert_eq!(camera.adapter_kind(), "rtsp");
        let encoded = serde_json::to_string(&camera).unwrap();
        let restored: Camera = serde_json::from_str(&encoded).unwrap();
        assert_eq!(restored.adapter_kind(), "rtsp");
        if matches!(restored.adapter, AdapterConfig::Preset { .. }) {
            restored.validate().unwrap();
        }
    }
}

#[tokio::test]
async fn preset_resolves_credentials_locally_and_preserves_rtsp_wire_identity() {
    let camera: Camera =
        serde_json::from_str(include_str!("../config/camera-preset.json.example")).unwrap();
    let resolved = camera
        .adapter()
        .resolve(&reqwest::Client::new())
        .await
        .unwrap();
    assert_eq!(resolved.adapter_kind, "rtsp");
    assert_eq!(resolved.identity.manufacturer.as_deref(), Some("Hikvision"));
    let source = resolved.stream_source("main").unwrap();
    let MediaSource::Rtsp(url) = source else {
        panic!("preset should resolve to RTSP")
    };
    assert!(url.contains("camera-user:REPLACE_WITH_CAMERA_PASSWORD@"));
    let identity = serde_json::to_string(&resolved.identity).unwrap();
    assert!(!identity.contains("PASSWORD"));
    assert!(!identity.contains("192.0.2.10"));
}

#[test]
fn local_configuration_rejects_unknown_ffmpeg_options() {
    let mut document: serde_json::Value =
        serde_json::from_str(include_str!("../config/camera-local-linux.json.example")).unwrap();
    document["adapter"]["extra_args"] = serde_json::json!(["-i", "/etc/passwd"]);
    assert!(serde_json::from_value::<Camera>(document).is_err());
}
