use super::*;

pub(crate) fn tempdir() -> tempfile::TempDir {
    // macOS exposes its temporary directory through /var, a system symlink.
    // Production storage deliberately rejects symlink ancestors; fixtures must
    // use the real temporary root without weakening that protection.
    let root = std::env::temp_dir();
    #[cfg(unix)]
    let root = root.canonicalize().unwrap();
    tempfile::tempdir_in(root).unwrap()
}

fn write_private_config(path: &Path, bytes: impl AsRef<[u8]>) {
    fs::write(path, bytes).unwrap();
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(path, fs::Permissions::from_mode(0o600)).unwrap();
    }
}

#[cfg(unix)]
#[tokio::test]
async fn cancelling_reconciliation_preserves_owned_media_finalization() {
    let directory = tempdir();
    let finalized = directory.path().join("finalized");
    let child = Command::new("sh")
        .args(["-c", "read -r command; [ \"$command\" = stop ] || exit 1; sleep 0.1; printf finalized > \"$1\"", "fixture"])
        .arg(&finalized)
        .stdin(Stdio::piped())
        .kill_on_drop(true)
        .spawn()
        .unwrap();
    let mut stopping = MediaTasks::new();
    assert!(
        tokio::time::timeout(
            Duration::from_millis(10),
            stop_media_children([child], &mut stopping),
        )
        .await
        .is_err()
    );
    assert_eq!(stopping.stopping.len(), 1);
    stop_media_children([], &mut stopping).await;
    assert!(stopping.stopping.is_empty());
    assert_eq!(fs::read(finalized).unwrap(), b"finalized");
}

#[cfg(unix)]
#[tokio::test]
async fn cancelling_pending_startup_retains_child_until_graceful_shutdown() {
    let directory = tempdir();
    let finalized = directory.path().join("finalized");
    let destination = finalized.clone();
    let release_flush = Arc::new(tokio::sync::Notify::new());
    let flush = release_flush.clone();
    let (spawned, ready) = tokio::sync::oneshot::channel();
    let mut tasks = MediaTasks::new();
    let mut start = Box::pin(start_media_worker(&mut tasks, async move {
        let child = Command::new("sh")
            .args([
                "-c",
                "read -r command; [ \"$command\" = stop ] || exit 1; printf finalized > \"$1\"",
                "fixture",
            ])
            .arg(destination)
            .stdin(Stdio::piped())
            .kill_on_drop(true)
            .spawn()?;
        let _ = spawned.send(());
        // Model Windows' pending async flush after the worker has started.
        flush.notified().await;
        Ok(child)
    }));
    tokio::time::timeout(Duration::from_secs(2), async {
        tokio::select! {
            result = &mut start => panic!("startup returned before flush was released: {}", result.is_ok()),
            result = ready => result.unwrap(),
        }
    }).await.unwrap();
    drop(start); // Shutdown cancels the tick waiting for startup.
    assert_eq!(tasks.starting.len(), 1);
    release_flush.notify_one();
    finish_media_shutdown([], &mut tasks).await;
    assert!(tasks.starting.is_empty() && tasks.stopping.is_empty());
    assert_eq!(fs::read(finalized).unwrap(), b"finalized");
}

/// Exercises the actual parent stop paths against the linked native worker,
/// with synthetic media only. Each peer deliberately stalls instead of sending
/// EOF, so only the private stop command can finalize the recording promptly.
#[tokio::test]
#[ignore = "requires ffmpeg/ffprobe and XCOC_TEST_CLIENT_EXE"]
async fn native_recorders_finalize_on_shutdown_reload_and_disable() {
    use tokio::io::{AsyncReadExt, AsyncWriteExt};

    let executable = PathBuf::from(std::env::var_os("XCOC_TEST_CLIENT_EXE").unwrap());
    let fixture = tempdir();
    let source = fixture.path().join("source.ts");
    let generated = Command::new("ffmpeg")
        .args([
            "-nostdin",
            "-v",
            "error",
            "-f",
            "lavfi",
            "-i",
            "testsrc2=size=320x240:rate=10",
            "-t",
            "20",
            "-c:v",
            "libx264",
            "-threads",
            "1",
            "-tune",
            "zerolatency",
            "-g",
            "10",
            "-f",
            "mpegts",
        ])
        .arg(&source)
        .output()
        .await
        .unwrap();
    assert!(
        generated.status.success(),
        "synthetic media generation failed"
    );
    let data = fs::read(source).unwrap();

    for mode in ["shutdown", "reload", "disable", "parent_pipe_closed"] {
        let output = fixture.path().join(mode);
        fs::create_dir(&output).unwrap();
        let listener = tokio::net::TcpListener::bind((std::net::Ipv4Addr::LOCALHOST, 0))
            .await
            .unwrap();
        let address = listener.local_addr().unwrap();
        let bytes = data.clone();
        let peer =
            tokio::spawn(async move {
                let (mut socket, _) = listener.accept().await.unwrap();
                let mut header = Vec::new();
                while !header.ends_with(b"\r\n\r\n") {
                    header.push(socket.read_u8().await.unwrap());
                }
                socket.write_all(format!(
                "HTTP/1.1 200 OK\r\nContent-Type: video/mp2t\r\nContent-Length: {}\r\n\r\n",
                bytes.len() + 1_000_000
            ).as_bytes()).await.unwrap();
                socket.write_all(&bytes).await.unwrap();
                // Keep the connection open with no next packet. Normal stop must
                // interrupt av_read_frame and then finish its MP4 trailer.
                let _ = socket.read_u8().await;
            });
        let destination = output.join("%Y-%m-%d_%H-%M-%S.mp4");
        let mut command = Command::new(&executable);
        command
            .arg("media-worker")
            .stdin(Stdio::piped())
            .stdout(Stdio::null())
            .stderr(Stdio::piped())
            .kill_on_drop(true);
        #[cfg(unix)]
        private_child_umask(&mut command);
        let mut child = command.spawn().unwrap();
        let mut diagnostics = child.stderr.take().unwrap();
        let request = serde_json::json!({
            "operation": "record", "source": format!("http://{address}/source"),
            "rtsp": false, "destination": destination,
        });
        child
            .stdin
            .as_mut()
            .unwrap()
            .write_all(format!("{request}\n").as_bytes())
            .await
            .unwrap();

        let file = tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let file = fs::read_dir(&output)
                    .unwrap()
                    .next()
                    .map(|entry| entry.unwrap().path());
                if let Some(file) = file.filter(|file| file.metadata().unwrap().len() > 48) {
                    break file;
                }
                assert!(
                    child.try_wait().unwrap().is_none(),
                    "recorder exited before stop"
                );
                tokio::time::sleep(Duration::from_millis(10)).await;
            }
        })
        .await
        .expect("recorder wrote synthetic packets before stop");
        let camera_id = Uuid::new_v4();
        let mut configured = camera(camera_id, "native recorder");
        configured.storage_mode = StorageMode::Client;
        let mut state = state_with(vec![configured]);
        let mut runtime = HashMap::from([(camera_id, new_runtime_camera())]);
        let mut children = HashMap::from([(format!("{camera_id}:main:record"), child)]);
        let started = Instant::now();
        let mut stopping = MediaTasks::new();
        match mode {
            "shutdown" => {
                finish_media_shutdown(children.drain().map(|(_, child)| child), &mut stopping).await
            }
            "reload" => {
                let mut next: LocalState =
                    serde_json::from_value(serde_json::to_value(&state).unwrap()).unwrap();
                next.instances[0].camera.as_mut().unwrap().name = "renamed".into();
                let config = fixture.path().join("configuration.json");
                write_private_config(&config, serde_json::to_vec(&next).unwrap());
                reload_configuration(
                    &config,
                    &mut state,
                    &mut runtime,
                    &mut children,
                    &mut stopping,
                )
                .await
                .unwrap();
            }
            "disable" => {
                state.instances[0].camera.as_mut().unwrap().enabled = false;
                reconcile_local_recorders(
                    &state,
                    &mut runtime,
                    &mut children,
                    &mut MediaTasks::new(),
                )
                .await;
            }
            "parent_pipe_closed" => {
                drop(children.values_mut().next().unwrap().stdin.take());
                stop_media_children(children.drain().map(|(_, child)| child), &mut stopping).await;
            }
            _ => unreachable!(),
        }
        assert!(
            started.elapsed() < Duration::from_secs(3),
            "{mode} did not interrupt stalled input"
        );
        assert!(children.is_empty());
        let mut errors = Vec::new();
        diagnostics.read_to_end(&mut errors).await.unwrap();
        assert!(
            errors.is_empty(),
            "{mode} incorrectly reported a native shutdown error"
        );
        tokio::time::timeout(Duration::from_secs(1), peer)
            .await
            .unwrap()
            .unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(file.metadata().unwrap().permissions().mode() & 0o777, 0o600);
        }
        let probe = Command::new("ffprobe")
            .args([
                "-v",
                "error",
                "-show_entries",
                "format=duration",
                "-of",
                "json",
            ])
            .arg(&file)
            .output()
            .await
            .unwrap();
        assert!(probe.status.success(), "{mode} left an invalid MP4");
        let metadata: Value = serde_json::from_slice(&probe.stdout).unwrap();
        assert!(
            metadata["format"]["duration"]
                .as_str()
                .unwrap()
                .parse::<f64>()
                .unwrap()
                > 0.0
        );
        let decoded = Command::new("ffmpeg")
            .args(["-nostdin", "-v", "error", "-i"])
            .arg(&file)
            .args(["-map", "0", "-f", "null", "-"])
            .output()
            .await
            .unwrap();
        assert!(
            decoded.status.success(),
            "{mode} recording did not fully decode"
        );
        assert!(
            decoded.stderr.is_empty(),
            "{mode} recording has decode errors"
        );
    }
}

#[test]
fn unconfigured_pairing_reports_an_empty_camera_list() {
    let instance = CameraInstance {
        server: "https://xcos.example.com/".to_owned(),
        instance_id: Uuid::new_v4(),
        access_token: "a".repeat(43),
        name: "empty".to_owned(),
        camera: None,
    };
    let request = build_snapshot_request(&instance, &HashMap::new(), Vec::new());
    let json = serde_json::to_value(request).unwrap();
    assert_eq!(json["cameras"], serde_json::json!([]));
}

#[test]
fn command_receipts_are_batched_and_only_sent_ids_are_removed() {
    let instance_id = Uuid::new_v4();
    let mut results = HashMap::new();
    for _ in 0..101 {
        let id = Uuid::new_v4();
        results.insert(
            id,
            CommandResult::failed(id, CommandErrorCode::DeviceUnavailable),
        );
    }
    let first = pending_result_batch(&results);
    assert_eq!(first.len(), 100);
    let mut pending = HashMap::from([(instance_id, results)]);
    let sent = first.iter().map(|result| result.id).collect::<Vec<_>>();
    acknowledge_command_results(&mut pending, instance_id, &sent);
    assert_eq!(pending[&instance_id].len(), 1);
    let last = pending_result_batch(&pending[&instance_id]);
    acknowledge_command_results(&mut pending, instance_id, &[last[0].id]);
    assert!(!pending.contains_key(&instance_id));
}

#[test]
fn command_failure_uses_safe_product_messages_and_preserves_committed_pairing_evidence() {
    use xcsc::cli::ProductErrorCatalog;
    let error =
        anyhow::anyhow!("rtsp://operator:private-camera-password@192.0.2.3; raw device body")
            .context("pairing was committed, but camera configuration is not ready");
    let failure = public_cli_failure(&error);
    assert!(failure.committed);
    assert_eq!(failure.code, "operation_failed");
    let message = XcocErrors.message(failure.code).unwrap();
    assert!(!message.contains("private-camera-password"));
    assert!(!message.contains("rtsp://"));
    assert_eq!(
        public_cli_failure(&CommandBackpressure.into()).code,
        "DEVICE_BACKPRESSURE"
    );
    assert_eq!(
        public_cli_failure(&onvif::DiscoveryLimit.into()).code,
        "DISCOVERY_LIMIT_EXCEEDED"
    );
}

#[test]
fn reported_device_errors_remove_control_characters() {
    let message = safe_error(&anyhow::anyhow!(
        "camera\r\n token=private-credential\t rejected"
    ));
    assert_eq!(message, "The device operation failed.");
    assert!(!message.contains("private-credential"));
    assert!(!message.chars().any(char::is_control));
}

#[test]
fn local_unpair_preserves_other_pairings_and_removes_the_final_config() {
    let directory = tempdir();
    let path = directory.path().join("config.json");
    let old = Uuid::new_v4();
    let retained = Uuid::new_v4();
    save_state(
        &path,
        &state_with(vec![camera(old, "old"), camera(retained, "retained")]),
    )
    .unwrap();
    unpair(&path, old).unwrap();
    let state = load_state(&path).unwrap();
    assert_eq!(state.instances.len(), 1);
    assert_eq!(state.instances[0].instance_id, retained);
    unpair(&path, retained).unwrap();
    assert!(!path.exists());
}

#[test]
fn unconfigured_pairing_token_change_invalidates_inflight_snapshot_generation() {
    let id = Uuid::new_v4();
    let installation_id = Uuid::new_v4();
    let instance = |token: &str| CameraInstance {
        server: "https://xcos.example.com/".to_owned(),
        instance_id: id,
        access_token: token.to_owned(),
        name: "unconfigured".to_owned(),
        camera: None,
    };
    let mut current = LocalState {
        format: 1,
        installation_id,
        instances: vec![instance(&"a".repeat(43))],
    };
    let next = LocalState {
        format: 1,
        installation_id,
        instances: vec![instance(&"b".repeat(43))],
    };
    let mut runtime = HashMap::new();
    let changed = apply_reloaded_state(&mut current, next, &mut runtime).unwrap();
    assert_eq!(changed, HashSet::from([id]));
    assert!(runtime.is_empty());
}

#[test]
fn pairing_write_preflight_fails_before_remote_authorization_is_used() {
    let directory = tempdir();
    let parent = directory.path().join("not-a-directory");
    fs::write(&parent, b"occupied").unwrap();
    assert!(preflight_state_write(&parent.join("config.json")).is_err());
}

#[cfg(unix)]
#[test]
fn pairing_write_preflight_rejects_symlink_ancestors_before_remote_pairing() {
    let directory = tempdir();
    let real = directory.path().join("real");
    fs::create_dir(&real).unwrap();
    let alias = directory.path().join("alias");
    std::os::unix::fs::symlink(&real, &alias).unwrap();
    let config = alias.join("config.json");
    assert!(preflight_state_write(&config).is_err());
    assert!(!real.join("config.json").exists());
    assert_eq!(fs::read_dir(&real).unwrap().count(), 0);
}

#[cfg(unix)]
#[tokio::test]
async fn local_recording_uses_private_directory_and_child_umask() {
    use std::os::unix::fs::PermissionsExt;

    let directory = tempdir();
    let store = directory.path().join("recordings");
    let camera_directory = prepare_recording_directory(&store, Uuid::new_v4()).unwrap();
    assert_eq!(
        fs::metadata(&store).unwrap().permissions().mode() & 0o777,
        0o700
    );
    assert_eq!(
        fs::metadata(&camera_directory)
            .unwrap()
            .permissions()
            .mode()
            & 0o777,
        0o700
    );

    let mut command = Command::new("sh");
    command.arg("-c").arg("umask");
    private_child_umask(&mut command);
    let output = command.output().await.unwrap();
    assert!(output.status.success());
    let mode = String::from_utf8(output.stdout).unwrap();
    assert_eq!(u32::from_str_radix(mode.trim(), 8).unwrap(), 0o077);
}

#[test]
fn snapshot_response_rejects_actions_for_other_paired_cameras() {
    let requested = Uuid::new_v4();
    let other = Uuid::new_v4();
    let grant = PublishGrant {
        camera_id: other,
        profile: "main".to_owned(),
        publish_url: "rtsps://media.example/camera?jwt=token".to_owned(),
    };
    let command = DeviceCommand {
        id: Uuid::new_v4(),
        camera_id: other,
        kind: "ptz".to_owned(),
        payload: serde_json::json!({"action": "stop"}),
        expires_at: (chrono::Utc::now() + chrono::Duration::minutes(1)).to_rfc3339(),
    };
    let response = |publish, commands| SnapshotResponse {
        protocol: PROTOCOL.to_owned(),
        accepted_at: chrono::Utc::now().to_rfc3339(),
        publish,
        commands,
    };
    assert!(validate_snapshot_response(response(vec![grant.clone()], vec![]), requested).is_err());
    assert!(
        validate_snapshot_response(response(vec![], vec![command.clone()]), requested).is_err()
    );
    assert!(validate_snapshot_response(response(vec![grant], vec![command]), other).is_ok());
}

#[tokio::test]
async fn publisher_waits_for_camera_resolution_and_retry_deadline() {
    let camera_id = Uuid::new_v4();
    let grant = PublishGrant {
        camera_id,
        profile: "main".to_owned(),
        publish_url: "rtsps://media.example/camera?jwt=token".to_owned(),
    };
    let mut runtime = HashMap::from([(camera_id, new_runtime_camera())]);
    let mut children = HashMap::new();
    reconcile_publishers(
        &mut runtime,
        std::slice::from_ref(&grant),
        &mut children,
        &mut MediaTasks::new(),
    )
    .await;
    assert!(runtime.get(&camera_id).unwrap().error.is_none());
    assert!(children.is_empty());

    let current = runtime.get_mut(&camera_id).unwrap();
    current.resolved = Some(ResolvedDevice {
        adapter_kind: "rtsp",
        identity: DeviceIdentity::default(),
        capabilities: DeviceCapabilities {
            video: CapabilityStatus::Supported,
            main_stream: CapabilityStatus::Supported,
            sub_stream: CapabilityStatus::Unsupported,
            local_recording: CapabilityStatus::Supported,
            server_recording: CapabilityStatus::Supported,
            ptz: CapabilityStatus::Unsupported,
            events: CapabilityStatus::Unsupported,
            audio_input: CapabilityStatus::Unsupported,
            audio_output: CapabilityStatus::Unsupported,
        },
        streams: Vec::new(),
        control: device::ControlTarget::None,
    });
    current.mark_error("publisher exited".to_owned());
    let retry_at = current.retry_at;
    reconcile_publishers(
        &mut runtime,
        &[grant],
        &mut children,
        &mut MediaTasks::new(),
    )
    .await;
    assert_eq!(runtime.get(&camera_id).unwrap().retry_at, retry_at);
    assert!(children.is_empty());
}

#[test]
fn boot_start_prompt_defaults_to_yes() {
    assert!(parse_boot_start_choice("").unwrap());
    assert!(parse_boot_start_choice("  YES ").unwrap());
    assert!(!parse_boot_start_choice("n").unwrap());
    assert!(parse_boot_start_choice("later").is_err());
}

#[test]
fn only_one_runtime_can_hold_a_configuration_lock() {
    let directory = tempdir();
    let path = directory.path().join("config.json");
    let first = acquire_run_lock(&path).unwrap().unwrap();
    assert!(acquire_run_lock(&path).unwrap().is_none());
    drop(first);
    assert!(acquire_run_lock(&path).unwrap().is_some());
}

fn state_with(cameras: Vec<Camera>) -> LocalState {
    LocalState {
        format: 1,
        installation_id: Uuid::new_v4(),
        instances: cameras
            .into_iter()
            .map(|camera| CameraInstance {
                server: "https://xcos.example.com/".to_owned(),
                instance_id: camera.id,
                access_token: "a".repeat(43),
                name: camera.name.clone(),
                camera: Some(camera),
            })
            .collect(),
    }
}

fn camera(id: Uuid, name: &str) -> Camera {
    serde_json::from_value(serde_json::json!({
        "id": id,
        "name": name,
        "storage_mode": "server",
        "adapter": {
            "kind": "rtsp",
            "streams": [{"profile": "main", "url": "rtsp://camera/main"}]
        }
    }))
    .unwrap()
}

#[test]
fn camera_views_never_serialize_secrets() {
    let camera: Camera = serde_json::from_value(serde_json::json!({
        "name":"front", "storage_mode":"server",
        "adapter": {
            "kind":"rtsp",
            "streams":[{"profile":"main", "url":"rtsp://camera/main"}],
            "username":"u", "password":"p"
        }
    }))
    .unwrap();
    camera.validate().unwrap();
    let view = CameraView {
        id: camera.id,
        name: &camera.name,
        location: &camera.location,
        adapter_kind: camera.adapter_kind(),
        manufacturer: camera.manufacturer.as_deref(),
        model: camera.model.as_deref(),
        enabled: true,
        storage_mode: camera.storage_mode.as_str(),
    };
    let json = serde_json::to_string(&view).unwrap();
    assert!(!json.contains("://") && !json.contains("password") && !json.contains("username"));
}

#[test]
fn release_server_policy_requires_https() {
    assert!(validate_server_origin("https://xcos.example.com").is_ok());
    assert!(validate_server_origin("http://192.0.2.1").is_err());
    assert!(validate_server_origin("https://user@xcos.example.com").is_err());
}

#[test]
fn authorization_code_matches_the_current_server_contract() {
    assert!(validate_authorization_code("abcdefghijklmnopqrstuvwxyz0123456789").is_ok());
    assert!(validate_authorization_code(&"z9".repeat(18)).is_ok());
    for value in [
        "a".repeat(35),
        "a".repeat(37),
        "a".repeat(64),
        "A".repeat(36),
        "-".repeat(36),
        "é".repeat(18),
        format!("{}\n", "a".repeat(35)),
    ] {
        assert!(validate_authorization_code(&value).is_err());
    }
}

#[test]
fn server_origin_rejects_non_root_paths_and_request_metadata() {
    for suffix in ["/admin", "/api/v1", "/camera/", "/?token=x", "/#camera"] {
        assert!(validate_server_origin(&format!("https://xcos.example.com{suffix}")).is_err());
    }
    assert_eq!(
        validate_server_origin(" https://xcos.example.com:8443/ ")
            .unwrap()
            .as_str(),
        "https://xcos.example.com:8443/"
    );
}

#[cfg(debug_assertions)]
#[test]
fn debug_server_origin_accepts_both_loopback_address_families() {
    for address in ["http://localhost", "http://127.0.0.1", "http://[::1]"] {
        assert!(validate_server_origin(address).is_ok(), "{address}");
    }
    assert!(validate_server_origin("http://[::2]").is_err());
}

#[test]
fn failed_state_replacement_removes_temporary_credentials() {
    let temporary = tempdir();
    let path = temporary.path().join("config.json");
    fs::create_dir(&path).unwrap();
    assert!(save_state(&path, &state_with(Vec::new())).is_err());
    let entries: Vec<_> = fs::read_dir(temporary.path()).unwrap().collect();
    assert_eq!(entries.len(), 1);
    assert!(path.is_dir());
}

#[test]
fn oversized_state_does_not_create_temporary_credentials() {
    let temporary = tempdir();
    let path = temporary.path().join("config.json");
    let mut state = state_with(Vec::new());
    state.instances.push(CameraInstance {
        server: "https://xcos.example.com".into(),
        instance_id: Uuid::new_v4(),
        access_token: "x".repeat(MAX_INPUT_BYTES as usize),
        name: "camera".into(),
        camera: None,
    });
    assert!(save_state(&path, &state).is_err());
    assert_eq!(fs::read_dir(temporary.path()).unwrap().count(), 0);
}

#[test]
fn pairing_http_errors_keep_protocol_mismatch_distinct() {
    use reqwest::StatusCode;
    assert_eq!(
        pairing_error_code(StatusCode::NOT_FOUND, b"{}"),
        "pairing_endpoint_not_found"
    );
    assert_eq!(
        pairing_error_code(StatusCode::METHOD_NOT_ALLOWED, b"{}"),
        "pairing_http_method_rejected"
    );
    assert_eq!(
        pairing_error_code(StatusCode::UPGRADE_REQUIRED, b"{}"),
        "pairing_server_upgrade_required"
    );
    assert_eq!(
        pairing_error_code(
            StatusCode::BAD_REQUEST,
            br#"{"code":"other","message":"authorization-secret"}"#
        ),
        "pairing_request_rejected"
    );
    assert_eq!(
        pairing_error_code(
            StatusCode::BAD_REQUEST,
            br#"{"code":"unsupported_client_protocol"}"#
        ),
        "pairing_protocol_unsupported"
    );
}

#[tokio::test]
async fn removing_a_camera_keeps_its_paired_instance_slot() {
    let directory = tempdir();
    let path = directory.path().join("config.json");
    let id = Uuid::new_v4();
    save_state(&path, &state_with(vec![camera(id, "front")])).unwrap();

    camera_command(&path, CameraCommand::Remove { instance_id: id })
        .await
        .unwrap();

    let state = load_state(&path).unwrap();
    assert_eq!(state.instances.len(), 1);
    assert!(state.instances[0].camera.is_none());
}

#[test]
fn runtime_reload_adds_changes_and_removes_cameras() {
    let removed = Uuid::new_v4();
    let changed = Uuid::new_v4();
    let added = Uuid::new_v4();
    let mut current = state_with(vec![camera(removed, "old"), camera(changed, "before")]);
    let mut next = state_with(vec![camera(changed, "after"), camera(added, "new")]);
    next.installation_id = current.installation_id;
    let mut runtime = HashMap::from([
        (removed, new_runtime_camera()),
        (changed, new_runtime_camera()),
    ]);

    let reloaded = apply_reloaded_state(&mut current, next, &mut runtime).unwrap();

    assert_eq!(reloaded, HashSet::from([removed, changed, added]));
    assert!(!runtime.contains_key(&removed));
    assert!(runtime.contains_key(&changed));
    assert!(runtime.contains_key(&added));
    assert_eq!(current.instances.len(), 2);
}

#[test]
fn local_recording_plan_does_not_depend_on_server_publish_grants() {
    let id = Uuid::new_v4();
    let mut local = camera(id, "offline recorder");
    local.storage_mode = StorageMode::Client;
    let state = state_with(vec![local]);

    assert_eq!(
        desired_local_recorders(&state),
        HashSet::from([format!("{id}:main:record")])
    );
}

#[tokio::test]
async fn exited_media_process_reprobes_an_already_resolved_camera() {
    let camera_id = Uuid::new_v4();
    let mut current = new_runtime_camera();
    current.resolved = Some(ResolvedDevice {
        adapter_kind: "rtsp",
        identity: DeviceIdentity::default(),
        capabilities: DeviceCapabilities {
            video: CapabilityStatus::Supported,
            main_stream: CapabilityStatus::Supported,
            sub_stream: CapabilityStatus::Unsupported,
            local_recording: CapabilityStatus::Supported,
            server_recording: CapabilityStatus::Supported,
            ptz: CapabilityStatus::Unsupported,
            events: CapabilityStatus::Unsupported,
            audio_input: CapabilityStatus::Unsupported,
            audio_output: CapabilityStatus::Unsupported,
        },
        streams: Vec::new(),
        control: device::ControlTarget::None,
    });
    assert!(!current.should_resolve(Instant::now()));
    let mut process = Command::new(std::env::current_exe().unwrap())
        .arg("--list")
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .unwrap();
    process.wait().await.unwrap();
    let mut children = HashMap::from([(format!("{camera_id}:main:publish"), process)]);
    let mut runtime = HashMap::from([(camera_id, current)]);

    reap_children(&mut children, &mut runtime).await;

    assert!(children.is_empty());
    let current = runtime.get(&camera_id).unwrap();
    assert_eq!(
        current.error.as_deref(),
        Some("media process exited; retrying")
    );
    assert!(!current.should_resolve(current.retry_at - Duration::from_millis(1)));
    assert!(current.should_resolve(current.retry_at));
}

#[tokio::test]
async fn recorder_is_not_restarted_while_camera_waits_to_reprobe() {
    let camera_id = Uuid::new_v4();
    let mut configured = camera(camera_id, "local recorder");
    configured.storage_mode = StorageMode::Client;
    let state = state_with(vec![configured]);
    let mut current = new_runtime_camera();
    current.resolved = Some(ResolvedDevice {
        adapter_kind: "rtsp",
        identity: DeviceIdentity::default(),
        capabilities: DeviceCapabilities {
            video: CapabilityStatus::Supported,
            main_stream: CapabilityStatus::Supported,
            sub_stream: CapabilityStatus::Unsupported,
            local_recording: CapabilityStatus::Supported,
            server_recording: CapabilityStatus::Supported,
            ptz: CapabilityStatus::Unsupported,
            events: CapabilityStatus::Unsupported,
            audio_input: CapabilityStatus::Unsupported,
            audio_output: CapabilityStatus::Unsupported,
        },
        streams: vec![device::ResolvedStream {
            descriptor: StreamDescriptor {
                profile: "main".to_owned(),
                video_codec: None,
                audio_codec: None,
                width: None,
                height: None,
                frame_rate: None,
            },
            source: device::MediaSource::Rtsp("rtsp://camera/main".to_owned()),
        }],
        control: device::ControlTarget::None,
    });
    current.mark_error("media process exited; retrying".to_owned());
    let retry_at = current.retry_at;
    let mut runtime = HashMap::from([(camera_id, current)]);
    let mut children = HashMap::new();

    reconcile_local_recorders(&state, &mut runtime, &mut children, &mut MediaTasks::new()).await;

    assert!(children.is_empty());
    assert_eq!(runtime.get(&camera_id).unwrap().retry_at, retry_at);
}

fn test_command_ledger(
    commands: &[DeviceCommand],
) -> (
    tempfile::TempDir,
    Arc<tokio::sync::Mutex<command_ledger::CommandLedger>>,
) {
    let directory = tempdir();
    let mut ledger =
        command_ledger::CommandLedger::open(&directory.path().join("config.json")).unwrap();
    ledger.admit(commands).unwrap();
    (directory, Arc::new(tokio::sync::Mutex::new(ledger)))
}

#[tokio::test]
async fn completed_commands_are_reused_through_acknowledgement_grace() {
    let id = Uuid::new_v4();
    let camera_id = Uuid::new_v4();
    let expires_at = (chrono::Utc::now() + chrono::Duration::seconds(10)).to_rfc3339();
    let command = DeviceCommand {
        id,
        camera_id,
        kind: "ptz".to_owned(),
        payload: serde_json::json!({ "action": "stop" }),
        expires_at,
    };
    let mut completed = HashMap::new();
    let (_directory, ledger) = test_command_ledger(std::slice::from_ref(&command));
    let first = execute_commands(
        &HashMap::new(),
        vec![command.clone()],
        &mut completed,
        &ledger,
    )
    .await
    .unwrap();
    let second = execute_commands(&HashMap::new(), vec![command], &mut completed, &ledger)
        .await
        .unwrap();
    assert_eq!(first[0].1.error_code, second[0].1.error_code);
    assert_eq!(completed.len(), 1);
    completed.retain(|_, (_, expires_at)| {
        *expires_at > chrono::Utc::now() + chrono::Duration::seconds(70)
    });
    assert_eq!(completed.len(), 1);
    completed.retain(|_, (_, expires_at)| {
        *expires_at > chrono::Utc::now() + chrono::Duration::seconds(131)
    });
    assert!(completed.is_empty());
}

#[test]
fn blocked_device_has_bounded_admission_and_keeps_other_device_capacity() {
    let blocked = Uuid::new_v4();
    let other = Uuid::new_v4();
    let command = |camera_id| DeviceCommand {
        id: Uuid::new_v4(),
        camera_id,
        kind: "ptz".into(),
        payload: serde_json::json!({"action":"move","direction":"left"}),
        expires_at: (chrono::Utc::now() + chrono::Duration::seconds(30)).to_rfc3339(),
    };
    let mut inflight = HashMap::new();
    let mut queues = HashMap::new();
    let mut receipts = HashMap::new();
    enqueue_device_commands(
        (0..MAX_INFLIGHT_PER_CAMERA)
            .map(|_| command(blocked))
            .collect(),
        &HashMap::new(),
        &mut inflight,
        &mut queues,
        &mut receipts,
    )
    .unwrap();
    assert_eq!(
        available_command_capacity(
            blocked,
            &HashMap::new(),
            &inflight,
            &receipts,
            &HashMap::new(),
            &[]
        ),
        0
    );
    assert!(
        available_command_capacity(
            other,
            &HashMap::new(),
            &inflight,
            &receipts,
            &HashMap::new(),
            &[]
        ) > 0
    );
    for _ in 0..2000 {
        assert!(
            enqueue_device_commands(
                vec![command(blocked)],
                &HashMap::new(),
                &mut inflight,
                &mut queues,
                &mut receipts
            )
            .is_err()
        );
    }
    assert_eq!(queues[&blocked].len(), MAX_INFLIGHT_PER_CAMERA);
    assert_eq!(inflight.len(), MAX_INFLIGHT_PER_CAMERA);
    enqueue_device_commands(
        vec![command(other)],
        &HashMap::new(),
        &mut inflight,
        &mut queues,
        &mut receipts,
    )
    .unwrap();
    assert_eq!(queues[&other].len(), 1);
    let reservation = HashMap::from([(other, MAX_INFLIGHT_COMMANDS)]);
    assert_eq!(
        available_command_capacity(
            other,
            &HashMap::new(),
            &inflight,
            &receipts,
            &reservation,
            &[]
        ),
        0
    );
}

#[tokio::test]
async fn command_queue_deduplicates_and_preserves_order_without_blocking_other_cameras() {
    let slow_camera = Uuid::new_v4();
    let fast_camera = Uuid::new_v4();
    let deadline = (chrono::Utc::now() + chrono::Duration::seconds(10)).to_rfc3339();
    let make_command = |camera_id| DeviceCommand {
        id: Uuid::new_v4(),
        camera_id,
        kind: "ptz".to_owned(),
        payload: serde_json::json!({"action":"stop"}),
        expires_at: deadline.clone(),
    };
    let first = make_command(slow_camera);
    let second = make_command(slow_camera);
    let fast = make_command(fast_camera);
    let mut inflight_ids = HashMap::new();
    let mut queues = HashMap::new();
    let mut receipts = HashMap::new();
    enqueue_device_commands(
        vec![first.clone(), first.clone(), second.clone(), fast.clone()],
        &HashMap::new(),
        &mut inflight_ids,
        &mut queues,
        &mut receipts,
    )
    .unwrap();
    assert_eq!(inflight_ids.len(), 3);
    assert_eq!(
        queues[&slow_camera]
            .iter()
            .map(|command| command.id)
            .collect::<Vec<_>>(),
        vec![first.id, second.id]
    );

    let mut tasks = tokio::task::JoinSet::new();
    let blocked = tasks.spawn(std::future::pending::<anyhow::Result<CommandTaskResult>>());
    let admitted = queues.values().flatten().cloned().collect::<Vec<_>>();
    let (_directory, ledger) = test_command_ledger(&admitted);
    let mut handles = HashMap::from([(slow_camera, blocked)]);
    let mut revisions = HashMap::new();
    launch_queued_command_tasks(
        &HashMap::new(),
        &mut queues,
        &mut handles,
        &mut revisions,
        &mut tasks,
        &ledger,
    );
    assert_eq!(queues[&slow_camera].len(), 2);
    assert!(!queues.contains_key(&fast_camera));
    let (_, completed_camera, fast_results, _) =
        tokio::time::timeout(Duration::from_secs(1), tasks.join_next())
            .await
            .unwrap()
            .unwrap()
            .unwrap()
            .unwrap();
    assert_eq!(completed_camera, fast_camera);
    assert_eq!(fast_results[0].1.id, fast.id);

    handles.remove(&fast_camera);
    handles.remove(&slow_camera).unwrap().abort();
    assert!(
        tasks
            .join_next()
            .await
            .unwrap()
            .err()
            .unwrap()
            .is_cancelled()
    );
    launch_queued_command_tasks(
        &HashMap::new(),
        &mut queues,
        &mut handles,
        &mut revisions,
        &mut tasks,
        &ledger,
    );
    let (_, completed_camera, results, _) = tasks.join_next().await.unwrap().unwrap().unwrap();
    assert_eq!(completed_camera, slow_camera);
    assert_eq!(
        results
            .iter()
            .map(|(_, result)| result.id)
            .collect::<Vec<_>>(),
        vec![first.id, second.id]
    );
}

#[tokio::test]
async fn unconfirmed_ptz_is_unknown_and_durable_replay_sends_no_second_action() {
    use std::io::{Read, Write};
    use std::sync::atomic::{AtomicUsize, Ordering};
    for failure in ["invalid_reply", "disconnect", "timeout"] {
        let listener = std::net::TcpListener::bind("127.0.0.1:0").unwrap();
        let address = listener.local_addr().unwrap();
        let requests = Arc::new(AtomicUsize::new(0));
        let counted = Arc::clone(&requests);
        let simulator = std::thread::spawn(move || {
            let (mut socket, _) = listener.accept().unwrap();
            socket
                .set_read_timeout(Some(Duration::from_secs(2)))
                .unwrap();
            let mut request = [0; 8192];
            assert!(socket.read(&mut request).unwrap() > 0);
            counted.fetch_add(1, Ordering::SeqCst);
            if failure == "disconnect" {
                return;
            }
            if failure == "timeout" {
                std::thread::sleep(Duration::from_millis(200));
                return;
            }
            let body = "<Envelope><Body><unrelated/></Body></Envelope>";
            write!(
                socket,
                "HTTP/1.1 200 OK\r\nContent-Length: {}\r\nConnection: close\r\n\r\n{}",
                body.len(),
                body
            )
            .unwrap();
        });
        let camera = Uuid::new_v4();
        let command = DeviceCommand {
            id: Uuid::new_v4(),
            camera_id: camera,
            kind: "ptz".into(),
            payload: serde_json::json!({"action":"move","pan":0.5}),
            expires_at: (chrono::Utc::now() + chrono::Duration::seconds(30)).to_rfc3339(),
        };
        let (_directory, ledger) = test_command_ledger(std::slice::from_ref(&command));
        let client = reqwest::Client::builder()
            .no_proxy()
            .timeout(Duration::from_millis(100))
            .build()
            .unwrap();
        let control = onvif::ControlTarget::new(
            client,
            Url::parse(&format!("http://{address}/ptz")).unwrap(),
            "fixture-profile".into(),
        )
        .unwrap();
        let mut capabilities = DeviceCapabilities::unknown();
        capabilities.ptz = CapabilityStatus::Supported;
        let device = ResolvedDevice {
            adapter_kind: "onvif",
            identity: DeviceIdentity::default(),
            capabilities,
            streams: Vec::new(),
            control: device::ControlTarget::Onvif(control),
        };
        let mut runtime_camera = new_runtime_camera();
        runtime_camera.resolved = Some(device);
        let runtime = HashMap::from([(camera, runtime_camera)]);
        let first = execute_commands(
            &runtime,
            vec![command.clone()],
            &mut HashMap::new(),
            &ledger,
        )
        .await
        .unwrap();
        assert_eq!(first[0].1, CommandResult::unknown(command.id));
        let second = execute_commands(
            &runtime,
            vec![command.clone()],
            &mut HashMap::new(),
            &ledger,
        )
        .await
        .unwrap();
        assert_eq!(second[0].1, CommandResult::unknown(command.id));
        simulator.join().unwrap();
        assert_eq!(requests.load(Ordering::SeqCst), 1);
        let reopened =
            command_ledger::CommandLedger::open(&_directory.path().join("config.json")).unwrap();
        assert_eq!(
            reopened.replay(&command).unwrap(),
            Some(CommandResult::unknown(command.id))
        );
    }
}

#[tokio::test]
async fn queued_command_expired_before_execution_skips_the_device() {
    let camera_id = Uuid::new_v4();
    let command = DeviceCommand {
        id: Uuid::new_v4(),
        camera_id,
        kind: "ptz".to_owned(),
        payload: serde_json::json!({"action":"move"}),
        expires_at: (chrono::Utc::now() - chrono::Duration::seconds(1)).to_rfc3339(),
    };
    let mut completed = HashMap::new();
    let (_directory, ledger) = test_command_ledger(std::slice::from_ref(&command));
    let results = execute_commands(&HashMap::new(), vec![command], &mut completed, &ledger)
        .await
        .unwrap();
    assert_eq!(results[0].1.outcome, CommandOutcome::Failed);
    assert_eq!(
        results[0].1.error_code,
        Some(CommandErrorCode::ExpiredBeforeExecution)
    );
}

#[test]
fn publish_grants_require_encrypted_scoped_urls() {
    assert!(validate_publish_url("rtsps://xcos.example.com:8322/camera?jwt=token").is_ok());
    assert!(validate_publish_url("rtsp://xcos.example.com:8554/camera?jwt=token").is_err());
    assert!(
        validate_publish_url("rtsps://user:password@xcos.example.com:8322/camera?jwt=token")
            .is_err()
    );
    assert!(validate_publish_url("rtsps://xcos.example.com:8322/camera").is_err());
}

#[test]
fn incompatible_or_corrupt_account_state_requires_explicit_repair() {
    let directory = tempdir();
    let path = directory.path().join("config.json");
    write_private_config(&path, br#"{"format":2,"legacy":true}"#);
    let error = load_state(&path).err().unwrap();
    assert!(is_pairing_state_incompatible(&error));
    assert!(error.to_string().contains("run setup --replace"));

    fs::write(&path, b"not-json").unwrap();
    let error = load_state(&path).err().unwrap();
    assert!(is_pairing_state_incompatible(&error));
    assert!(error.to_string().contains("config JSON is corrupt"));
}

#[test]
fn incompatible_pairing_recovery_archives_original_bytes() {
    let directory = tempdir();
    let path = directory.path().join("config.json");
    let original = br#"{"format":1,"account":"old"}"#;
    write_private_config(&path, original);
    let camera_id = Uuid::new_v4();
    let replacement = state_with(vec![camera(camera_id, "front")]);

    let archive = replace_incompatible_pairing_state(&path, &replacement).unwrap();

    assert_eq!(fs::read(archive).unwrap(), original);
    assert_eq!(
        load_state(&path).unwrap().installation_id,
        replacement.installation_id
    );
}

#[test]
fn invalid_camera_configuration_is_not_discardable_pairing_state() {
    let directory = tempdir();
    let path = directory.path().join("config.json");
    let id = Uuid::new_v4();
    let mut state = state_with(vec![camera(id, "front")]);
    state.instances[0].camera.as_mut().unwrap().id = Uuid::new_v4();
    write_private_config(&path, serde_json::to_vec(&state).unwrap());

    let error = load_state(&path).err().unwrap();

    assert!(!is_pairing_state_incompatible(&error));
    assert!(
        error
            .to_string()
            .starts_with("configuration_state_incompatible:")
    );
}

#[test]
fn unknown_recording_entries_are_reported_and_preserved() {
    let directory = tempdir();
    let recording = directory.path().join("old-layout.bin");
    fs::write(&recording, b"important recording bytes").unwrap();

    let error = validate_recording_store(directory.path()).unwrap_err();

    assert!(
        error
            .to_string()
            .starts_with("important_state_incompatible:")
    );
    assert_eq!(fs::read(recording).unwrap(), b"important recording bytes");
}

#[test]
fn configuration_reads_are_bounded_and_do_not_expose_rejected_field_values() {
    let directory = tempdir();
    let path = directory.path().join("config.json");
    write_private_config(&path, b"{}");
    fs::OpenOptions::new()
        .write(true)
        .open(&path)
        .unwrap()
        .set_len(MAX_INPUT_BYTES + 1)
        .unwrap();
    assert!(
        load_state(&path)
            .err()
            .unwrap()
            .to_string()
            .contains("exceeds 1 MiB")
    );
    assert_eq!(fs::metadata(&path).unwrap().len(), MAX_INPUT_BYTES + 1);
    let secret = "private-credential-must-not-be-echoed";
    let id = Uuid::new_v4();
    let mut document = serde_json::to_value(state_with(vec![camera(id, "front")])).unwrap();
    document["installation_id"] = serde_json::json!(secret);
    write_private_config(&path, serde_json::to_vec(&document).unwrap());
    let error = load_state(&path).err().unwrap().to_string();
    assert!(error.contains("config schema is not supported"));
    assert!(!error.contains(secret));
}

#[cfg(unix)]
#[test]
fn shared_configuration_io_rejects_links_public_credentials_and_unsafe_replacement() {
    use std::os::unix::fs::{MetadataExt, PermissionsExt, symlink};
    let directory = tempdir();
    let path = directory.path().join("config.json");
    let id = Uuid::new_v4();
    let state = state_with(vec![camera(id, "front")]);
    save_state(&path, &state).unwrap();
    assert_eq!(fs::metadata(&path).unwrap().mode() & 0o777, 0o600);
    assert!(load_state(&path).is_ok());
    let link = directory.path().join("alias.json");
    fs::hard_link(&path, &link).unwrap();
    assert!(load_state(&path).is_err());
    assert!(save_state(&path, &state).is_err());
    fs::remove_file(&link).unwrap();
    fs::set_permissions(&path, fs::Permissions::from_mode(0o644)).unwrap();
    assert!(load_state(&path).is_err());
    assert!(save_state(&path, &state).is_err());
    fs::set_permissions(&path, fs::Permissions::from_mode(0o600)).unwrap();
    let alias_parent = directory.path().join("linked-parent");
    symlink(directory.path(), &alias_parent).unwrap();
    assert!(load_state(&alias_parent.join("config.json")).is_err());
    assert!(save_state(&alias_parent.join("config.json"), &state).is_err());
}

#[test]
fn local_recording_names_survive_daylight_saving_fallback() {
    let root = Path::new("recordings");
    let pattern = local_recording_pattern(root);
    let name = pattern.file_name().unwrap().to_str().unwrap();
    let before = chrono::DateTime::parse_from_rfc3339("2026-11-01T01:15:00-04:00").unwrap();
    let after = chrono::DateTime::parse_from_rfc3339("2026-11-01T01:15:00-05:00").unwrap();
    assert_ne!(
        before.format(name).to_string(),
        after.format(name).to_string(),
        "the repeated daylight-saving hour must not reuse an earlier segment"
    );
    assert_eq!(pattern.parent(), Some(root));
    assert_eq!(pattern.extension().unwrap(), "mp4");
}

#[test]
fn local_recording_names_survive_same_second_restarts() {
    let root = Path::new("recordings");
    let first = local_recording_pattern(root);
    let restarted = local_recording_pattern(root);
    let now = chrono::DateTime::parse_from_rfc3339("2026-11-01T01:15:00-04:00").unwrap();
    let filename = |path: &Path| {
        now.format(path.file_name().unwrap().to_str().unwrap())
            .to_string()
    };
    assert_ne!(
        filename(&first),
        filename(&restarted),
        "restarting a recorder within the same second must not replace its last segment"
    );
}

#[tokio::test]
async fn cancelled_resolution_does_not_reset_replacement_task() {
    let id = Uuid::new_v4();
    let mut tasks = tokio::task::JoinSet::new();
    let old = tasks.spawn(std::future::pending::<ResolveResult>());
    tasks.abort_all();
    let mut current = new_runtime_camera();
    current.resolving = true;
    let mut runtime = HashMap::from([(id, current)]);
    tasks.spawn(std::future::pending::<ResolveResult>());
    while !old.is_finished() {
        tokio::task::yield_now().await;
    }
    collect_resolved_cameras(&mut tasks, &mut runtime).unwrap();
    assert!(runtime[&id].resolving);
    assert!(!runtime[&id].should_resolve(Instant::now()));
    tasks.shutdown().await;
}
