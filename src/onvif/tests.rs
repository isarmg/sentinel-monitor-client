use super::*;

#[test]
fn discovery_correlates_probe_and_rejects_unsafe_or_excessive_endpoints() {
    let id = Uuid::new_v4();
    let response = |relation: String, addresses: String| {
        format!(
            "<Envelope><Header><RelatesTo>{relation}</RelatesTo></Header><Body><ProbeMatch><Address>urn:uuid:{id}</Address><XAddrs>{addresses}</XAddrs></ProbeMatch></Body></Envelope>"
        )
    };
    let remote = "192.0.2.3:3702".parse().unwrap();
    let valid = response(
        format!("uuid:{id}"),
        "http://192.0.2.3/onvif/device_service".into(),
    );
    assert!(parse_probe_response(valid.as_bytes(), remote, id).is_some());
    let wrong = response(
        format!("uuid:{}", Uuid::new_v4()),
        "http://192.0.2.3/onvif/device_service".into(),
    );
    assert!(parse_probe_response(wrong.as_bytes(), remote, id).is_none());
    let credential = response(
        format!("uuid:{id}"),
        "http://operator:private@192.0.2.3/onvif/device_service".into(),
    );
    assert!(parse_probe_response(credential.as_bytes(), remote, id).is_none());
    let excessive = response(
        format!("uuid:{id}"),
        ["http://192.0.2.3/onvif/device_service"; MAX_DISCOVERY_XADDRS + 1].join(" "),
    );
    assert!(parse_probe_response(excessive.as_bytes(), remote, id).is_none());
}

#[test]
fn discovery_flood_stops_without_growing_either_collection_or_publishing_partial_success() {
    let mut devices = Vec::new();
    let mut seen = HashSet::new();
    let device = |index| DiscoveredDevice {
        endpoint: format!("urn:uuid:{index}"),
        xaddrs: vec!["http://192.0.2.3/onvif/device_service".into()],
        scopes: Vec::new(),
        remote_addr: "192.0.2.3:3702".into(),
    };
    for index in 0..MAX_DISCOVERY_RESULTS {
        admit_discovered_device(device(index), &mut devices, &mut seen).unwrap();
    }
    for _ in 0..1000 {
        admit_discovered_device(device(0), &mut devices, &mut seen).unwrap();
    }
    for index in MAX_DISCOVERY_RESULTS..MAX_DISCOVERY_RESULTS + 1000 {
        let error = admit_discovered_device(device(index), &mut devices, &mut seen).unwrap_err();
        assert!(error.downcast_ref::<DiscoveryLimit>().is_some());
    }
    assert_eq!(devices.len(), MAX_DISCOVERY_RESULTS);
    assert_eq!(seen.len(), MAX_DISCOVERY_RESULTS);
}

#[test]
fn ptz_reply_must_acknowledge_the_exact_operation_in_the_soap_body() {
    let body = r#"<s:Envelope xmlns:s="http://www.w3.org/2003/05/soap-envelope"><s:Body><p:StopResponse xmlns:p="http://www.onvif.org/ver20/ptz/wsdl"/></s:Body></s:Envelope>"#;
    assert!(validate_ptz_response(body, "StopResponse").is_ok());
    assert!(validate_ptz_response(body, "ContinuousMoveResponse").is_err());
    assert!(
        validate_ptz_response(
            "<Envelope><Body><StopResponse/></Body></Envelope>",
            "StopResponse"
        )
        .is_err()
    );
    let duplicate = body.replace(
        "</s:Body>",
        "<p:StopResponse xmlns:p=\"http://www.onvif.org/ver20/ptz/wsdl\"/></s:Body>",
    );
    assert!(validate_ptz_response(&duplicate, "StopResponse").is_err());
}

#[test]
fn soap_fault_is_not_reported_as_success() {
    let fault = r#"<s:Envelope xmlns:s="http://www.w3.org/2003/05/soap-envelope"><s:Body><s:Fault><s:Reason><s:Text>not authorized</s:Text></s:Reason></s:Fault></s:Body></s:Envelope>"#;
    assert!(validate_soap_response(fault).is_err());
    assert!(validate_soap_response("<Envelope><Body><StopResponse/></Body></Envelope>").is_ok());
}

#[test]
fn response_chunks_stop_at_the_xml_limit_without_appending_the_extra_chunk() {
    let mut bytes = Vec::new();
    let half = vec![b'x'; MAX_XML_BYTES / 2];
    append_response_chunk(&mut bytes, &half).unwrap();
    append_response_chunk(&mut bytes, &half).unwrap();
    assert_eq!(bytes.len(), MAX_XML_BYTES);
    let error = append_response_chunk(&mut bytes, b"x").unwrap_err();
    assert!(
        error
            .to_string()
            .contains("ONVIF response exceeds size limit")
    );
    assert_eq!(bytes.len(), MAX_XML_BYTES);
}

#[test]
fn profile_selection_normalizes_largest_and_smallest_profiles() {
    let xml = r#"<Envelope><Body><GetProfilesResponse>
          <Profiles token="medium"><VideoEncoderConfiguration><Encoding>H264</Encoding><Resolution><Width>1280</Width><Height>720</Height></Resolution><RateControl><FrameRateLimit>25</FrameRateLimit></RateControl></VideoEncoderConfiguration></Profiles>
          <Profiles token="main"><VideoEncoderConfiguration><Encoding>H265</Encoding><Resolution><Width>3840</Width><Height>2160</Height></Resolution></VideoEncoderConfiguration></Profiles>
          <Profiles token="sub"><VideoEncoderConfiguration><Encoding>H264</Encoding><Resolution><Width>640</Width><Height>360</Height></Resolution></VideoEncoderConfiguration></Profiles>
        </GetProfilesResponse></Body></Envelope>"#;
    let profiles = parse_profiles(xml).unwrap();
    let selected = select_profiles(&profiles, None, None).unwrap();
    assert_eq!(selected[0].0, "main");
    assert_eq!(selected[0].1.token, "main");
    assert_eq!(selected[1].0, "sub");
    assert_eq!(selected[1].1.token, "sub");
}

#[test]
fn audio_only_profiles_do_not_become_video_streams() {
    let xml = r#"<Envelope><Body><GetProfilesResponse>
          <Profiles token="main"><AudioEncoderConfiguration><Encoding>AAC</Encoding></AudioEncoderConfiguration><VideoEncoderConfiguration><Encoding>H264</Encoding><Resolution><Width>1920</Width><Height>1080</Height></Resolution></VideoEncoderConfiguration></Profiles>
          <Profiles token="audio"><AudioEncoderConfiguration><Encoding>AAC</Encoding></AudioEncoderConfiguration></Profiles>
        </GetProfilesResponse></Body></Envelope>"#;
    let profiles = parse_profiles(xml).unwrap();
    let selected = select_profiles(&profiles, None, None).unwrap();
    assert_eq!(selected.len(), 1);
    assert_eq!(selected[0].1.token, "main");
    assert_eq!(selected[0].1.video_codec.as_deref(), Some("H264"));
    assert_eq!(selected[0].1.audio_codec.as_deref(), Some("AAC"));
}

#[test]
fn ptz_capability_belongs_to_the_selected_video_profile() {
    let xml = r#"<Envelope><Body><GetProfilesResponse>
          <Profiles token="main"><VideoEncoderConfiguration><Encoding>H264</Encoding><Resolution><Width>1920</Width><Height>1080</Height></Resolution></VideoEncoderConfiguration></Profiles>
          <Profiles token="movable"><VideoEncoderConfiguration><Encoding>H264</Encoding><Resolution><Width>640</Width><Height>360</Height></Resolution></VideoEncoderConfiguration><PTZConfiguration token="ptz"/></Profiles>
        </GetProfilesResponse></Body></Envelope>"#;
    let profiles = parse_profiles(xml).unwrap();
    assert!(!select_profiles(&profiles, None, None).unwrap()[0].1.ptz);
    assert!(
        select_profiles(&profiles, Some("movable"), None).unwrap()[0]
            .1
            .ptz
    );
}

#[test]
fn capability_parser_accepts_attribute_and_child_xaddr() {
    let attribute = r#"<Capabilities><Media XAddr="http://192.0.2.2/media"/></Capabilities>"#;
    assert_eq!(
        capability_url(
            attribute,
            "Media",
            &Url::parse("http://192.0.2.2/onvif/device_service").unwrap(),
        )
        .unwrap()
        .unwrap()
        .as_str(),
        "http://192.0.2.2/media"
    );
    let child = r#"<Capabilities><PTZ><XAddr>http://192.0.2.2/ptz</XAddr></PTZ></Capabilities>"#;
    assert!(
        capability_url(
            child,
            "PTZ",
            &Url::parse("http://192.0.2.2/onvif/device_service").unwrap(),
        )
        .unwrap()
        .is_some()
    );
    assert!(
        capability_url(
            r#"<Capabilities><Media XAddr="http://169.254.169.254/latest"/></Capabilities>"#,
            "Media",
            &Url::parse("http://192.0.2.2/onvif/device_service").unwrap(),
        )
        .is_err()
    );
}
