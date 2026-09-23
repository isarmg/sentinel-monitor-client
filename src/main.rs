mod device;
mod onvif;

use anyhow::{Context, ensure};
use clap::{Parser, Subcommand};
use device::{
    Camera, DeviceCapabilities, DeviceIdentity, ResolvedDevice, StorageMode, StreamDescriptor,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use std::{
    collections::{HashMap, HashSet},
    fs::{self, OpenOptions},
    io::{self, Read, Write},
    path::{Path, PathBuf},
    process::Stdio,
    time::{Duration, Instant},
};
use tokio::process::{Child, Command};
use url::Url;
use uuid::Uuid;
use zeroize::Zeroizing;

const PROTOCOL: &str = "sentinel-edge-v3";
const PRODUCT: &str = "sentinel-monitor";
const MAX_INPUT_BYTES: u64 = 1024 * 1024;
const MAX_URL_BYTES: usize = 4_096;
const MAX_AUTHORIZATION_CODE_BYTES: usize = 64;
const MAX_NAME_BYTES: usize = 256;
const MAX_LOCATION_BYTES: usize = 512;
const MAX_USERNAME_BYTES: usize = 256;
const MAX_PASSWORD_BYTES: usize = 4_096;
const MAX_STORAGE_MODE_BYTES: usize = 16;
const MAX_PAIRING_RESPONSE_BYTES: usize = 16 * 1024;

#[derive(Parser)]
#[command(
    name = "sentinel-client",
    version,
    about = "Sentinel camera edge client"
)]
struct Cli {
    #[arg(long, global = true, default_value_os_t = default_config_path())]
    config: PathBuf,
    #[command(subcommand)]
    command: CommandKind,
}

#[derive(Subcommand)]
enum CommandKind {
    /// Pair with a Sentinel Server using protected JSON from stdin.
    Setup {
        #[arg(long)]
        input_stdin: bool,
        #[arg(long)]
        interactive: bool,
        #[arg(long)]
        replace: bool,
        /// Maximum time for the complete interactive input sequence.
        #[arg(long, default_value_t = 300, value_parser = clap::value_parser!(u64).range(1..=3600))]
        timeout_seconds: u64,
    },
    /// Run camera publishing, local recording and heartbeat reconciliation.
    Run,
    /// Show pairing and camera state without secrets.
    Status,
    /// Manage locally-owned camera configuration.
    Camera {
        #[command(subcommand)]
        command: CameraCommand,
    },
    /// Installer-only removal of explicitly deselected configuration or recordings.
    #[command(hide = true)]
    InstallerReset {
        #[arg(long)]
        configuration: bool,
        #[arg(long)]
        data: bool,
    },
}

#[derive(Subcommand)]
enum CameraCommand {
    /// Add or replace a camera from a protected JSON document on stdin.
    Apply {
        /// Server camera instance to configure.
        #[arg(long)]
        instance_id: Uuid,
        #[arg(long)]
        input_stdin: bool,
    },
    /// List cameras without RTSP URLs or credentials.
    List,
    /// Remove the local camera configuration from one paired instance.
    Remove { instance_id: Uuid },
    /// Discover standards-compliant ONVIF cameras on the client network.
    Discover {
        #[arg(long, default_value_t = 3, value_parser = clap::value_parser!(u64).range(1..=15))]
        timeout_seconds: u64,
    },
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct LocalState {
    format: u32,
    installation_id: Uuid,
    instances: Vec<CameraInstance>,
}

#[derive(Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CameraInstance {
    server: String,
    instance_id: Uuid,
    access_token: String,
    name: String,
    camera: Option<Camera>,
}

#[derive(Deserialize, zeroize::Zeroize)]
#[serde(deny_unknown_fields)]
struct SetupInput {
    server: String,
    authorization_code: String,
    name: String,
}

#[derive(Serialize)]
struct PairRequest<'a> {
    protocol: &'static str,
    product: &'static str,
    installation_id: Uuid,
    name: &'a str,
    client_version: &'static str,
    authorization_code: &'a str,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct PairResponse {
    protocol: String,
    client_id: Uuid,
    access_token: String,
}

#[derive(Deserialize)]
struct ServerErrorCode {
    code: String,
}

fn pairing_error_code(status: reqwest::StatusCode, body: &[u8]) -> &'static str {
    if serde_json::from_slice::<ServerErrorCode>(body)
        .is_ok_and(|error| error.code == "unsupported_client_protocol")
    {
        return "pairing_protocol_unsupported";
    }
    match status.as_u16() {
        401 | 403 => "pairing_authorization_rejected",
        404 => "pairing_endpoint_not_found",
        405 => "pairing_http_method_rejected",
        406 | 426 => "pairing_server_upgrade_required",
        408 | 429 | 500..=599 => "pairing_server_unavailable",
        400..=499 => "pairing_request_rejected",
        _ => "pairing_unexpected_http_status",
    }
}

async fn parse_pairing_response(mut response: reqwest::Response) -> anyhow::Result<PairResponse> {
    let status = response.status();
    let mut body = Vec::new();
    while let Some(chunk) = response.chunk().await.context("read pairing response")? {
        ensure!(
            body.len()
                .checked_add(chunk.len())
                .is_some_and(|size| size <= MAX_PAIRING_RESPONSE_BYTES),
            "pairing_response_too_large"
        );
        body.extend_from_slice(&chunk);
    }
    if status.is_success() {
        return serde_json::from_slice(&body).context("invalid pairing response");
    }
    anyhow::bail!(pairing_error_code(status, &body))
}

#[derive(Serialize)]
struct SnapshotRequest {
    protocol: &'static str,
    cameras: Vec<CameraSnapshot>,
    command_results: Vec<CommandResult>,
}

#[derive(Serialize)]
struct CameraSnapshot {
    id: Uuid,
    name: String,
    location: String,
    adapter_kind: String,
    identity: DeviceIdentity,
    capabilities: DeviceCapabilities,
    streams: Vec<StreamDescriptor>,
    has_sub_stream: bool,
    enabled: bool,
    storage_mode: &'static str,
    status: &'static str,
    health_message: Option<String>,
}

#[derive(Deserialize)]
#[serde(deny_unknown_fields)]
struct SnapshotResponse {
    protocol: String,
    accepted_at: String,
    publish: Vec<PublishGrant>,
    commands: Vec<DeviceCommand>,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct PublishGrant {
    camera_id: Uuid,
    profile: String,
    publish_url: String,
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct DeviceCommand {
    id: Uuid,
    camera_id: Uuid,
    kind: String,
    payload: Value,
    expires_at: String,
}

#[derive(Clone, Serialize)]
#[serde(deny_unknown_fields)]
struct CommandResult {
    id: Uuid,
    status: &'static str,
    error: Option<String>,
}

#[derive(Serialize)]
struct CameraView<'a> {
    id: Uuid,
    name: &'a str,
    location: &'a str,
    adapter_kind: &'static str,
    manufacturer: Option<&'a str>,
    model: Option<&'a str>,
    enabled: bool,
    storage_mode: &'static str,
}

struct RuntimeCamera {
    resolved: Option<ResolvedDevice>,
    error: Option<String>,
    retry_at: Instant,
}

#[tokio::main]
async fn main() -> anyhow::Result<()> {
    let cli = Cli::parse();
    match cli.command {
        CommandKind::Setup {
            input_stdin,
            interactive,
            replace,
            timeout_seconds,
        } => {
            setup(
                &cli.config,
                input_stdin,
                interactive,
                replace,
                timeout_seconds,
            )
            .await
        }
        CommandKind::Run => run(&cli.config).await,
        CommandKind::Status => status(&cli.config),
        CommandKind::Camera { command } => camera_command(&cli.config, command).await,
        CommandKind::InstallerReset {
            configuration,
            data,
        } => installer_reset(&cli.config, configuration, data),
    }
}

fn installer_reset(path: &Path, configuration: bool, data: bool) -> anyhow::Result<()> {
    ensure!(
        configuration ^ data,
        "installer-reset requires exactly one of --configuration or --data"
    );
    ensure!(
        path == default_config_path(),
        "installer-reset accepts only the platform default state path"
    );
    if configuration {
        match fs::symlink_metadata(path) {
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error).context("inspect old Sentinel configuration"),
            Ok(metadata) => {
                ensure!(
                    metadata.is_file() && !metadata.file_type().is_symlink(),
                    "configuration_state_incompatible: refusing to remove a config path that is not a regular non-symlink file"
                );
                fs::remove_file(path).context("remove old Sentinel configuration")?;
            }
        }
        println!("old Sentinel configuration was not retained");
    } else {
        let root = recording_root();
        validate_removable_tree(&root)?;
        match fs::remove_dir_all(&root) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(error).context("remove old Sentinel recordings"),
        }
        println!("old Sentinel recordings were not retained");
    }
    Ok(())
}

fn validate_removable_tree(root: &Path) -> anyhow::Result<()> {
    let metadata = match fs::symlink_metadata(root) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => return Err(error).context("inspect installer data reset target"),
    };
    ensure!(
        metadata.is_dir() && !metadata.file_type().is_symlink(),
        "installer data reset target is not a regular non-symlink directory"
    );
    let mut pending = vec![root.to_path_buf()];
    let mut entries = 0usize;
    while let Some(directory) = pending.pop() {
        for entry in fs::read_dir(&directory)? {
            let entry = entry?;
            entries = entries
                .checked_add(1)
                .context("installer data entry count overflow")?;
            ensure!(
                entries <= 100_000,
                "installer data exceeds the 100000-entry safety limit"
            );
            let metadata = fs::symlink_metadata(entry.path())?;
            ensure!(
                !metadata.file_type().is_symlink(),
                "installer data contains a symbolic link or junction"
            );
            if metadata.is_dir() {
                pending.push(entry.path());
            } else {
                ensure!(
                    metadata.is_file(),
                    "installer data contains an unsupported file type"
                );
            }
        }
    }
    Ok(())
}

async fn setup(
    path: &Path,
    input_stdin: bool,
    interactive: bool,
    replace: bool,
    timeout_seconds: u64,
) -> anyhow::Result<()> {
    ensure!(
        input_stdin ^ interactive,
        "setup requires exactly one of --interactive or --input-stdin"
    );
    preflight_media_tools().await?;
    validate_recording_store(&recording_root())?;
    let (mut state, recover_incompatible_pairing) = if path.exists() {
        match load_state(path) {
            Ok(state) => (state, false),
            Err(error) if replace && is_pairing_state_incompatible(&error) => (
                LocalState {
                    format: 3,
                    installation_id: Uuid::new_v4(),
                    instances: Vec::new(),
                },
                true,
            ),
            Err(error) => return Err(error),
        }
    } else {
        (
            LocalState {
                format: 3,
                installation_id: Uuid::new_v4(),
                instances: Vec::new(),
            },
            false,
        )
    };
    let input_deadline = Instant::now() + Duration::from_secs(timeout_seconds);
    let input: Zeroizing<SetupInput> = Zeroizing::new(if interactive {
        SetupInput {
            server: sarmg_client_cli::prompt_text(
                "Server HTTPS origin",
                MAX_URL_BYTES,
                input_deadline,
            )?,
            authorization_code: sarmg_client_cli::prompt_text(
                "Authorization code (visible)",
                MAX_AUTHORIZATION_CODE_BYTES,
                input_deadline,
            )?,
            name: sarmg_client_cli::prompt_text("Client name", MAX_NAME_BYTES, input_deadline)?,
        }
    } else {
        read_stdin_json()?
    });
    let server = validate_server_origin(&input.server)?;
    validate_name(&input.name)?;
    validate_authorization_code(&input.authorization_code)?;
    let client = server_client()?;
    let response = client
        .post(server.join("/api/v2/client/pair")?)
        .json(&PairRequest {
            protocol: PROTOCOL,
            product: PRODUCT,
            installation_id: state.installation_id,
            name: input.name.trim(),
            client_version: env!("CARGO_PKG_VERSION"),
            authorization_code: &input.authorization_code,
        })
        .send()
        .await
        .context("pairing request failed")?;
    let response = parse_pairing_response(response).await?;
    ensure!(
        response.protocol == PROTOCOL
            && !response.client_id.is_nil()
            && response.access_token.len() == 43,
        "Server returned an incompatible pairing response"
    );
    let position = state
        .instances
        .iter()
        .position(|instance| instance.instance_id == response.client_id);
    ensure!(
        position.is_none() || replace,
        "this camera instance is already paired; use setup --replace after changing its Server authorization code"
    );
    let preserved_camera = position.and_then(|index| state.instances[index].camera.take());
    let instance = CameraInstance {
        server: server.as_str().trim_end_matches('/').into(),
        instance_id: response.client_id,
        access_token: response.access_token,
        name: input.name.trim().into(),
        camera: preserved_camera,
    };
    match position {
        Some(index) => state.instances[index] = instance,
        None => {
            ensure!(state.instances.len() < 256, "camera instance limit reached");
            state.instances.push(instance);
        }
    }
    let archived_state = if recover_incompatible_pairing {
        Some(replace_incompatible_pairing_state(path, &state)?)
    } else {
        save_state(path, &state)?;
        None
    };
    if interactive {
        let mut camera = interactive_camera_setup(input_deadline).await.context(
            "pairing was committed, but camera configuration is not ready; use camera apply --instance-id to resume",
        )?;
        camera.id = response.client_id;
        state
            .instances
            .iter_mut()
            .find(|value| value.instance_id == response.client_id)
            .expect("newly paired instance is stored")
            .camera = Some(camera);
        save_state(path, &state)?;
    }
    println!(
        "paired camera instance {}; media tools verified; configured: {}",
        response.client_id,
        state
            .instances
            .iter()
            .any(|value| value.instance_id == response.client_id && value.camera.is_some())
    );
    if let Some(archive) = archived_state {
        println!(
            "incompatible pairing state was preserved at {}; re-pairing completed",
            archive.display()
        );
    }
    Ok(())
}

async fn preflight_media_tools() -> anyhow::Result<()> {
    for tool in ["ffmpeg", "ffprobe"] {
        let status = Command::new(tool)
            .arg("-version")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null())
            .status()
            .await
            .with_context(|| format!("{tool} is required in the background service PATH"))?;
        ensure!(status.success(), "{tool} preflight failed");
    }
    Ok(())
}

async fn interactive_camera_setup(deadline: Instant) -> anyhow::Result<Camera> {
    let discovered = onvif::discover(Duration::from_secs(3)).await?;
    for (index, camera) in discovered.iter().enumerate() {
        eprintln!(
            "{}. {} ({})",
            index + 1,
            camera.xaddrs.first().unwrap_or(&camera.endpoint),
            camera.remote_addr
        );
    }
    let selection = sarmg_client_cli::prompt_text(
        if discovered.is_empty() {
            "No ONVIF camera found; enter a manual RTSP main-stream URL"
        } else {
            "Select an ONVIF camera number, or enter a manual RTSP main-stream URL"
        },
        MAX_URL_BYTES,
        deadline,
    )?;
    let name = sarmg_client_cli::prompt_text("Camera name", MAX_NAME_BYTES, deadline)?;
    let location =
        sarmg_client_cli::prompt_text("Camera location (optional)", MAX_LOCATION_BYTES, deadline)?;
    let storage_mode = match sarmg_client_cli::prompt_text(
        "Recording location [server/client] (default server)",
        MAX_STORAGE_MODE_BYTES,
        deadline,
    )?
    .as_str()
    {
        "" | "server" => StorageMode::Server,
        "client" => StorageMode::Client,
        _ => anyhow::bail!("recording location must be server or client"),
    };
    let username =
        sarmg_client_cli::prompt_text("Camera username (optional)", MAX_USERNAME_BYTES, deadline)?;
    let password = sarmg_client_cli::prompt_secret(
        "Camera password (optional)",
        MAX_PASSWORD_BYTES,
        deadline,
    )?
    .to_string();
    let adapter = if let Ok(index) = selection.parse::<usize>() {
        let selected = discovered
            .get(
                index
                    .checked_sub(1)
                    .context("camera selection starts at 1")?,
            )
            .context("camera selection is outside the discovered list")?;
        device::AdapterConfig::Onvif {
            device_service_url: selected
                .xaddrs
                .first()
                .cloned()
                .unwrap_or_else(|| selected.endpoint.clone()),
            username: (!username.is_empty()).then_some(username),
            password: (!password.is_empty()).then_some(password),
            main_profile_token: None,
            sub_profile_token: None,
        }
    } else {
        let sub = sarmg_client_cli::prompt_text(
            "RTSP sub-stream URL (optional)",
            MAX_URL_BYTES,
            deadline,
        )?;
        let mut streams = vec![device::ConfiguredStream {
            profile: device::StreamProfile::Main,
            url: selection,
        }];
        if !sub.is_empty() {
            streams.push(device::ConfiguredStream {
                profile: device::StreamProfile::Sub,
                url: sub,
            });
        }
        device::AdapterConfig::Rtsp {
            streams,
            username: (!username.is_empty()).then_some(username),
            password: (!password.is_empty()).then_some(password),
        }
    };
    let camera = Camera {
        id: Uuid::new_v4(),
        name,
        location,
        manufacturer: None,
        model: None,
        enabled: true,
        storage_mode,
        adapter,
    };
    camera.validate()?;
    let mut resolved = camera.adapter().resolve(&device_client()?).await?;
    resolved.probe_streams().await?;
    Ok(camera)
}

fn status(path: &Path) -> anyhow::Result<()> {
    validate_recording_store(&recording_root())?;
    let state = load_state(path)?;
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "paired": true,
            "installation_id": state.installation_id,
            "instances": state.instances.iter().map(|instance| serde_json::json!({
                "server": instance.server,
                "instance_id": instance.instance_id,
                "name": instance.name,
                "configured": instance.camera.is_some(),
            })).collect::<Vec<_>>(),
        }))?
    );
    Ok(())
}

async fn camera_command(path: &Path, command: CameraCommand) -> anyhow::Result<()> {
    validate_recording_store(&recording_root())?;
    let mut state = load_state(path)?;
    match command {
        CameraCommand::Apply {
            instance_id,
            input_stdin,
        } => {
            ensure!(input_stdin, "camera apply requires --input-stdin");
            let mut camera: Camera = read_stdin_json()?;
            camera.validate()?;
            camera.id = instance_id;
            let instance = state
                .instances
                .iter_mut()
                .find(|value| value.instance_id == instance_id)
                .context("camera instance is not paired")?;
            instance.camera = Some(camera);
            save_state(path, &state)?;
        }
        CameraCommand::List => {
            let views = state
                .instances
                .iter()
                .filter_map(|instance| {
                    instance.camera.as_ref().map(|camera| CameraView {
                        id: instance.instance_id,
                        name: &camera.name,
                        location: &camera.location,
                        adapter_kind: camera.adapter_kind(),
                        manufacturer: camera.manufacturer.as_deref(),
                        model: camera.model.as_deref(),
                        enabled: camera.enabled,
                        storage_mode: camera.storage_mode.as_str(),
                    })
                })
                .collect::<Vec<_>>();
            println!("{}", serde_json::to_string_pretty(&views)?);
        }
        CameraCommand::Remove { instance_id } => {
            let instance = state
                .instances
                .iter_mut()
                .find(|value| value.instance_id == instance_id)
                .context("camera instance is not paired")?;
            ensure!(instance.camera.take().is_some(), "camera is not configured");
            save_state(path, &state)?;
        }
        CameraCommand::Discover { timeout_seconds } => {
            let devices = onvif::discover(Duration::from_secs(timeout_seconds)).await?;
            println!("{}", serde_json::to_string_pretty(&devices)?);
        }
    }
    Ok(())
}

async fn run(path: &Path) -> anyhow::Result<()> {
    validate_recording_store(&recording_root())?;
    let mut state = load_state(path)?;
    let server_client = server_client()?;
    let device_client = device_client()?;
    let mut children: HashMap<String, Child> = HashMap::new();
    let mut runtime = state
        .instances
        .iter()
        .filter_map(|instance| instance.camera.as_ref())
        .map(|camera| (camera.id, new_runtime_camera()))
        .collect::<HashMap<_, _>>();
    let mut command_results: HashMap<Uuid, Vec<CommandResult>> = HashMap::new();
    let mut publish_grants: HashMap<Uuid, Vec<PublishGrant>> = HashMap::new();
    let mut completed_commands: HashMap<Uuid, CommandResult> = HashMap::new();
    let mut interval = tokio::time::interval(Duration::from_secs(2));
    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => break,
            _ = interval.tick() => {
                reload_configuration(path, &mut state, &mut runtime, &mut children).await?;
                reap_children(&mut children, &mut runtime).await;
                resolve_due_cameras(&device_client, &state, &mut runtime).await;
                // Local recording is a data-plane responsibility of this
                // process. It must start and recover even when the remote
                // control plane cannot accept a snapshot.
                reconcile_local_recorders(&state, &runtime, &mut children).await?;
                let configured_instances = state.instances.iter()
                    .filter(|value| value.camera.is_some())
                    .map(|value| value.instance_id)
                    .collect::<HashSet<_>>();
                let grant_count = publish_grants.len();
                publish_grants.retain(|instance_id, _| configured_instances.contains(instance_id));
                let mut grants_changed = publish_grants.len() != grant_count;
                let mut commands = Vec::new();
                for instance in state.instances.iter().filter(|value| value.camera.is_some()) {
                    let pending = command_results.get(&instance.instance_id).map(Vec::as_slice).unwrap_or(&[]);
                    match send_snapshot(&server_client, instance, &runtime, pending).await {
                        Ok(response) => {
                            command_results.remove(&instance.instance_id);
                            publish_grants.insert(instance.instance_id, response.publish);
                            grants_changed = true;
                            commands.extend(response.commands);
                        }
                        Err(error) => eprintln!("snapshot failed for instance {}: {error:#}", instance.instance_id),
                    }
                }
                if grants_changed {
                    let grants = publish_grants.values().flatten().cloned().collect::<Vec<_>>();
                    reconcile_publishers(&mut runtime, &grants, &mut children).await?;
                }
                for (instance_id, result) in execute_commands(&runtime, commands, &mut completed_commands).await {
                    command_results.entry(instance_id).or_default().push(result);
                }
            }
        }
    }
    for (_, mut child) in children {
        let _ = child.kill().await;
    }
    Ok(())
}

fn new_runtime_camera() -> RuntimeCamera {
    RuntimeCamera {
        resolved: None,
        error: None,
        retry_at: Instant::now(),
    }
}

fn apply_reloaded_state(
    current: &mut LocalState,
    next: LocalState,
    runtime: &mut HashMap<Uuid, RuntimeCamera>,
) -> anyhow::Result<HashSet<Uuid>> {
    ensure!(
        current.format == next.format && current.installation_id == next.installation_id,
        "client installation identity changed while the client was running; restart the client"
    );

    let current_cameras = current
        .instances
        .iter()
        .filter_map(|instance| instance.camera.as_ref().map(|camera| (instance, camera)))
        .map(|(instance, camera)| Ok((camera.id, serde_json::to_vec(&(instance, camera))?)))
        .collect::<anyhow::Result<HashMap<_, _>>>()?;
    let next_cameras = next
        .instances
        .iter()
        .filter_map(|instance| instance.camera.as_ref().map(|camera| (instance, camera)))
        .map(|(instance, camera)| Ok((camera.id, serde_json::to_vec(&(instance, camera))?)))
        .collect::<anyhow::Result<HashMap<_, _>>>()?;
    let changed = current_cameras
        .keys()
        .chain(next_cameras.keys())
        .filter(|id| current_cameras.get(id) != next_cameras.get(id))
        .copied()
        .collect::<HashSet<_>>();

    for id in &changed {
        runtime.remove(id);
        if next_cameras.contains_key(id) {
            runtime.insert(*id, new_runtime_camera());
        }
    }
    current.instances = next.instances;
    Ok(changed)
}

async fn reload_configuration(
    path: &Path,
    state: &mut LocalState,
    runtime: &mut HashMap<Uuid, RuntimeCamera>,
    children: &mut HashMap<String, Child>,
) -> anyhow::Result<()> {
    let changed = apply_reloaded_state(state, load_state(path)?, runtime)?;
    let stale = children
        .keys()
        .filter(|key| {
            key.split(':')
                .next()
                .and_then(|value| Uuid::parse_str(value).ok())
                .is_some_and(|id| changed.contains(&id))
        })
        .cloned()
        .collect::<Vec<_>>();
    for key in stale {
        if let Some(mut child) = children.remove(&key) {
            let _ = child.kill().await;
        }
    }
    Ok(())
}

async fn send_snapshot(
    client: &reqwest::Client,
    instance: &CameraInstance,
    runtime: &HashMap<Uuid, RuntimeCamera>,
    command_results: &[CommandResult],
) -> anyhow::Result<SnapshotResponse> {
    let camera = instance
        .camera
        .as_ref()
        .context("camera instance is not configured")?;
    let cameras = vec![{
        let current = runtime.get(&camera.id);
        let resolved = current.and_then(|current| current.resolved.as_ref());
        let identity = resolved
            .map(|device| device.identity.clone())
            .unwrap_or_else(|| DeviceIdentity {
                manufacturer: camera.manufacturer.clone(),
                model: camera.model.clone(),
                ..DeviceIdentity::default()
            });
        let capabilities = resolved
            .map(|device| device.capabilities.clone())
            .unwrap_or(DeviceCapabilities {
                video: true,
                main_stream: true,
                sub_stream: false,
                local_recording: true,
                server_recording: true,
                ptz: false,
                events: false,
                audio_input: false,
                audio_output: false,
            });
        let streams: Vec<StreamDescriptor> = resolved
            .map(|device| {
                device
                    .streams
                    .iter()
                    .map(|stream| stream.descriptor.clone())
                    .collect()
            })
            .unwrap_or_default();
        let has_sub_stream = streams
            .iter()
            .any(|stream: &StreamDescriptor| stream.profile == "sub");
        let error = current.and_then(|current| current.error.clone());
        CameraSnapshot {
            id: camera.id,
            name: camera.name.trim().to_owned(),
            location: camera.location.trim().to_owned(),
            adapter_kind: resolved.map_or_else(
                || camera.adapter_kind().to_owned(),
                |device| device.adapter_kind.to_owned(),
            ),
            identity,
            capabilities,
            streams,
            has_sub_stream,
            enabled: camera.enabled,
            storage_mode: camera.storage_mode.as_str(),
            status: if !camera.enabled {
                "disabled"
            } else if error.is_some() {
                "error"
            } else if resolved.is_some() {
                "online"
            } else {
                "pending"
            },
            health_message: error,
        }
    }];
    let url = Url::parse(&instance.server)?.join("/api/v2/client/snapshot")?;
    let response = client
        .put(url)
        .bearer_auth(&instance.access_token)
        .json(&SnapshotRequest {
            protocol: PROTOCOL,
            cameras,
            command_results: command_results.to_vec(),
        })
        .send()
        .await
        .context("snapshot request failed")?
        .error_for_status()
        .context("snapshot rejected")?
        .json::<SnapshotResponse>()
        .await
        .context("invalid snapshot response")?;
    ensure!(
        response.protocol == PROTOCOL && !response.accepted_at.is_empty(),
        "Server returned an incompatible snapshot response"
    );
    Ok(response)
}

async fn reconcile_publishers(
    runtime: &mut HashMap<Uuid, RuntimeCamera>,
    grants: &[PublishGrant],
    children: &mut HashMap<String, Child>,
) -> anyhow::Result<()> {
    let desired = grants
        .iter()
        .map(|grant| format!("{}:{}:publish", grant.camera_id, grant.profile))
        .collect::<HashSet<_>>();
    let stale = children
        .keys()
        .filter(|key| key.ends_with(":publish") && !desired.contains(*key))
        .cloned()
        .collect::<Vec<_>>();
    for key in stale {
        if let Some(mut child) = children.remove(&key) {
            let _ = child.kill().await;
        }
    }

    for grant in grants {
        let key = format!("{}:{}:publish", grant.camera_id, grant.profile);
        if children.contains_key(&key) {
            continue;
        }
        validate_publish_url(&grant.publish_url)?;
        let source = runtime
            .get(&grant.camera_id)
            .and_then(|runtime| runtime.resolved.as_ref())
            .context("Server returned a grant for an unresolved camera")?
            .stream_url(&grant.profile)?;
        children.insert(key, spawn_publisher(&source, &grant.publish_url)?);
        if let Some(current) = runtime.get_mut(&grant.camera_id) {
            current.error = None;
        }
    }

    Ok(())
}

fn validate_publish_url(value: &str) -> anyhow::Result<()> {
    let url = Url::parse(value).context("Server returned an invalid publish URL")?;
    let jwt_count = url
        .query_pairs()
        .filter(|(name, token)| name == "jwt" && !token.is_empty())
        .count();
    ensure!(
        url.scheme() == "rtsps"
            && url.host_str().is_some()
            && url.username().is_empty()
            && url.password().is_none()
            && url.fragment().is_none()
            && jwt_count == 1,
        "Server publish URL must be credential-free RTSPS with one scoped media token"
    );
    Ok(())
}

fn desired_local_recorders(state: &LocalState) -> HashSet<String> {
    state
        .instances
        .iter()
        .filter_map(|instance| instance.camera.as_ref())
        .filter(|camera| camera.enabled && matches!(camera.storage_mode, StorageMode::Client))
        .map(|camera| format!("{}:main:record", camera.id))
        .collect()
}

async fn reconcile_local_recorders(
    state: &LocalState,
    runtime: &HashMap<Uuid, RuntimeCamera>,
    children: &mut HashMap<String, Child>,
) -> anyhow::Result<()> {
    let desired = desired_local_recorders(state);
    let stale = children
        .keys()
        .filter(|key| key.ends_with(":record") && !desired.contains(*key))
        .cloned()
        .collect::<Vec<_>>();
    for key in stale {
        if let Some(mut child) = children.remove(&key) {
            let _ = child.kill().await;
        }
    }

    for camera in state
        .instances
        .iter()
        .filter_map(|instance| instance.camera.as_ref())
        .filter(|camera| camera.enabled && matches!(camera.storage_mode, StorageMode::Client))
    {
        let key = format!("{}:main:record", camera.id);
        if let std::collections::hash_map::Entry::Vacant(entry) = children.entry(key) {
            let Some(source) = runtime
                .get(&camera.id)
                .and_then(|runtime| runtime.resolved.as_ref())
                .and_then(|device| device.stream_url("main").ok())
            else {
                continue;
            };
            entry.insert(spawn_recorder(&source, camera.id)?);
        }
    }
    Ok(())
}

fn spawn_publisher(source: &str, destination: &str) -> anyhow::Result<Child> {
    Command::new("ffmpeg")
        .args([
            "-nostdin",
            "-hide_banner",
            "-loglevel",
            "warning",
            "-rtsp_transport",
            "tcp",
            "-i",
            source,
            "-map",
            "0",
            "-c",
            "copy",
            "-f",
            "rtsp",
            "-rtsp_transport",
            "tcp",
            "-tls_verify",
            "1",
            destination,
        ])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .spawn()
        .context("start ffmpeg publisher")
}

fn spawn_recorder(source: &str, camera_id: Uuid) -> anyhow::Result<Child> {
    let root = recording_root().join(camera_id.to_string());
    fs::create_dir_all(&root)?;
    let pattern = root.join("%Y-%m-%d_%H-%M-%S.mp4");
    Command::new("ffmpeg")
        .args([
            "-nostdin",
            "-hide_banner",
            "-loglevel",
            "warning",
            "-rtsp_transport",
            "tcp",
            "-i",
            source,
            "-map",
            "0",
            "-c",
            "copy",
            "-f",
            "segment",
            "-segment_time",
            "900",
            "-reset_timestamps",
            "1",
            "-strftime",
            "1",
        ])
        .arg(pattern)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .spawn()
        .context("start ffmpeg recorder")
}

async fn reap_children(
    children: &mut HashMap<String, Child>,
    runtime: &mut HashMap<Uuid, RuntimeCamera>,
) {
    let stopped = children
        .iter_mut()
        .filter_map(|(key, child)| child.try_wait().ok().flatten().map(|_| key.clone()))
        .collect::<Vec<_>>();
    for key in stopped {
        children.remove(&key);
        if let Some(id) = key
            .split(':')
            .next()
            .and_then(|value| Uuid::parse_str(value).ok())
            && let Some(camera) = runtime.get_mut(&id)
        {
            camera.error = Some("media process exited; retrying".to_owned());
        }
    }
}

async fn resolve_due_cameras(
    client: &reqwest::Client,
    state: &LocalState,
    runtime: &mut HashMap<Uuid, RuntimeCamera>,
) {
    for camera in state
        .instances
        .iter()
        .filter_map(|instance| instance.camera.as_ref())
        .filter(|camera| camera.enabled)
    {
        let should_resolve = runtime.get(&camera.id).is_some_and(|current| {
            current.resolved.is_none() && Instant::now() >= current.retry_at
        });
        if !should_resolve {
            continue;
        }
        let result = camera.adapter().resolve(client).await;
        let current = runtime
            .get_mut(&camera.id)
            .expect("runtime mirrors camera config");
        match result {
            Ok(mut resolved) => match resolved.probe_streams().await {
                Ok(()) => {
                    current.resolved = Some(resolved);
                    current.error = None;
                }
                Err(error) => {
                    current.error = Some(safe_error(&error));
                    current.retry_at = Instant::now() + Duration::from_secs(30);
                }
            },
            Err(error) => {
                current.error = Some(safe_error(&error));
                current.retry_at = Instant::now() + Duration::from_secs(30);
            }
        }
    }
}

async fn execute_commands(
    runtime: &HashMap<Uuid, RuntimeCamera>,
    commands: Vec<DeviceCommand>,
    completed: &mut HashMap<Uuid, CommandResult>,
) -> Vec<(Uuid, CommandResult)> {
    let mut results = Vec::with_capacity(commands.len());
    for command in commands {
        if let Some(result) = completed.get(&command.id) {
            results.push((command.camera_id, result.clone()));
            continue;
        }
        let expires_at = chrono::DateTime::parse_from_rfc3339(&command.expires_at)
            .map(|value| value.with_timezone(&chrono::Utc));
        if expires_at.is_err() || expires_at.is_ok_and(|value| value <= chrono::Utc::now()) {
            let result = CommandResult {
                id: command.id,
                status: "failed",
                error: Some("command expired before execution".into()),
            };
            completed.insert(command.id, result.clone());
            results.push((command.camera_id, result));
            continue;
        }
        let result = match runtime
            .get(&command.camera_id)
            .and_then(|runtime| runtime.resolved.as_ref())
        {
            None => Err(anyhow::anyhow!("camera is not resolved")),
            Some(device) if command.kind == "ptz" => {
                match serde_json::from_value(command.payload) {
                    Ok(payload) => device.ptz(payload).await,
                    Err(error) => Err(error.into()),
                }
            }
            Some(_) => Err(anyhow::anyhow!("unsupported device command")),
        };
        let camera_id = command.camera_id;
        let outcome = (
            camera_id,
            match result {
                Ok(()) => CommandResult {
                    id: command.id,
                    status: "succeeded",
                    error: None,
                },
                Err(error) => CommandResult {
                    id: command.id,
                    status: "failed",
                    error: Some(safe_error(&error)),
                },
            },
        );
        completed.insert(outcome.1.id, outcome.1.clone());
        results.push(outcome);
    }
    results
}

fn safe_error(error: &anyhow::Error) -> String {
    error.to_string().chars().take(512).collect()
}

fn validate_name(name: &str) -> anyhow::Result<()> {
    ensure!(
        !name.trim().is_empty()
            && name.chars().count() <= 64
            && !name.chars().any(char::is_control),
        "name must contain 1-64 non-control characters"
    );
    Ok(())
}

fn validate_authorization_code(value: &str) -> anyhow::Result<()> {
    ensure!(
        value.len() == 64
            && value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || matches!(byte, b'a'..=b'f')),
        "authorization_code must be 64 lowercase hexadecimal characters"
    );
    Ok(())
}

fn validate_server_origin(value: &str) -> anyhow::Result<Url> {
    let mut url = Url::parse(value.trim())?;
    ensure!(
        url.username().is_empty()
            && url.password().is_none()
            && url.path() == "/"
            && url.query().is_none()
            && url.fragment().is_none(),
        "server must be an origin without credentials, path, query or fragment"
    );
    #[cfg(debug_assertions)]
    let allowed = url.scheme() == "https"
        || (url.scheme() == "http"
            && matches!(
                url.host(),
                Some(url::Host::Domain("localhost"))
                    | Some(url::Host::Ipv4(std::net::Ipv4Addr::LOCALHOST))
                    | Some(url::Host::Ipv6(std::net::Ipv6Addr::LOCALHOST))
            ));
    #[cfg(not(debug_assertions))]
    let allowed = url.scheme() == "https";
    ensure!(
        allowed && url.host_str().is_some(),
        "server must use trusted HTTPS"
    );
    url.set_path("/");
    Ok(url)
}

fn server_client() -> anyhow::Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .timeout(Duration::from_secs(20))
        .connect_timeout(Duration::from_secs(10))
        .redirect(reqwest::redirect::Policy::none())
        .user_agent(concat!("sentinel-client/", env!("CARGO_PKG_VERSION")))
        .build()?)
}

fn device_client() -> anyhow::Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .connect_timeout(Duration::from_secs(10))
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        .user_agent(concat!("sentinel-client/", env!("CARGO_PKG_VERSION")))
        .build()?)
}

fn read_stdin_json<T: for<'de> Deserialize<'de>>() -> anyhow::Result<T> {
    let mut bytes = Vec::new();
    io::stdin()
        .take(MAX_INPUT_BYTES + 1)
        .read_to_end(&mut bytes)?;
    ensure!(
        bytes.len() as u64 <= MAX_INPUT_BYTES,
        "stdin JSON exceeds 1 MiB"
    );
    serde_json::from_slice(&bytes).context("stdin is not the expected JSON document")
}

fn load_state(path: &Path) -> anyhow::Result<LocalState> {
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        if error.kind() == io::ErrorKind::NotFound {
            anyhow::anyhow!("client is not configured; run setup")
        } else {
            anyhow::anyhow!(
                "pairing_state_incompatible: cannot inspect config: {error}; run setup --replace to archive it and pair again"
            )
        }
    })?;
    ensure!(
        metadata.is_file() && !metadata.file_type().is_symlink(),
        "pairing_state_incompatible: config must be a regular file without symlinks; correct the path manually"
    );
    let bytes = fs::read(path).map_err(|error| {
        anyhow::anyhow!(
            "pairing_state_incompatible: config cannot be read: {error}; run setup --replace to archive it and pair again"
        )
    })?;
    ensure!(
        bytes.len() as u64 <= MAX_INPUT_BYTES,
        "pairing_state_incompatible: config exceeds 1 MiB; run setup --replace to archive it and pair again"
    );
    let document: Value = serde_json::from_slice(&bytes).map_err(|error| {
        anyhow::anyhow!(
            "pairing_state_incompatible: config JSON is corrupt: {error}; run setup --replace to archive it and pair again"
        )
    })?;
    ensure!(
        document.get("format").and_then(Value::as_u64) == Some(3),
        "pairing_state_incompatible: config format is not supported; run setup --replace to archive it and pair again"
    );
    let state: LocalState = serde_json::from_value(document).map_err(|error| {
        anyhow::anyhow!(
            "pairing_state_incompatible: config schema is not supported: {error}; run setup --replace to archive it and pair again"
        )
    })?;
    ensure!(
        state.format == 3 && !state.installation_id.is_nil(),
        "pairing_state_incompatible: installation identity is invalid; run setup --replace to archive it and pair again"
    );
    ensure!(
        !state.instances.is_empty() && state.instances.len() <= 256,
        "pairing_state_incompatible: config must contain 1-256 camera instances; run setup --replace to archive it and pair again"
    );
    let mut instance_ids = HashSet::new();
    for instance in &state.instances {
        ensure!(
            !instance.instance_id.is_nil() && instance.access_token.len() == 43,
            "pairing_state_incompatible: camera instance credential is invalid; run setup --replace to archive it and pair again"
        );
        ensure!(
            instance_ids.insert(instance.instance_id),
            "pairing_state_incompatible: camera instance IDs must be unique; run setup --replace to archive it and pair again"
        );
        validate_name(&instance.name).map_err(|error| {
            anyhow::anyhow!(
                "pairing_state_incompatible: camera instance name is invalid: {error}; run setup --replace to archive it and pair again"
            )
        })?;
        validate_server_origin(&instance.server).map_err(|error| {
            anyhow::anyhow!(
                "pairing_state_incompatible: camera instance Server is invalid: {error}; run setup --replace to archive it and pair again"
            )
        })?;
        if let Some(camera) = &instance.camera {
            camera.validate().map_err(|error| {
                anyhow::anyhow!(
                    "configuration_state_incompatible: camera configuration is invalid: {error}; the config was preserved"
                )
            })?;
            ensure!(
                camera.id == instance.instance_id,
                "configuration_state_incompatible: camera ID does not match its paired instance; the config was preserved"
            );
        }
    }
    Ok(state)
}

fn is_pairing_state_incompatible(error: &anyhow::Error) -> bool {
    error
        .chain()
        .any(|cause| cause.to_string().starts_with("pairing_state_incompatible:"))
}

fn replace_incompatible_pairing_state(path: &Path, state: &LocalState) -> anyhow::Result<PathBuf> {
    let metadata = fs::symlink_metadata(path).map_err(|error| {
        anyhow::anyhow!(
            "pairing_state_incompatible: cannot inspect config before recovery: {error}"
        )
    })?;
    ensure!(
        metadata.is_file() && !metadata.file_type().is_symlink(),
        "pairing_state_incompatible: refusing to archive a config path that is not a regular non-symlink file"
    );
    let parent = path.parent().context("config path has no parent")?;
    let file_name = path
        .file_name()
        .and_then(|value| value.to_str())
        .context("config file name is not valid UTF-8")?;
    let archive = parent.join(format!("{file_name}.incompatible-{}", Uuid::new_v4()));
    fs::rename(path, &archive).context("archive incompatible pairing state")?;
    if let Err(error) = save_state(path, state) {
        let restore_error = fs::rename(&archive, path).err();
        return match restore_error {
            Some(restore_error) => Err(error.context(format!(
                "failed to restore archived pairing state after recovery failed: {restore_error}"
            ))),
            None => {
                Err(error.context("new pairing state was not committed; original config restored"))
            }
        };
    }
    Ok(archive)
}

fn validate_recording_store(root: &Path) -> anyhow::Result<()> {
    let metadata = match fs::symlink_metadata(root) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(()),
        Err(error) => {
            anyhow::bail!(
                "important_state_incompatible: cannot inspect recording root {}: {error}; recordings were preserved",
                root.display()
            )
        }
    };
    ensure!(
        metadata.is_dir() && !metadata.file_type().is_symlink(),
        "important_state_incompatible: recording root {} is not a regular directory; recordings were preserved",
        root.display()
    );
    let camera_directories = fs::read_dir(root).map_err(|error| {
        anyhow::anyhow!(
            "important_state_incompatible: cannot read recording root {}: {error}; recordings were preserved",
            root.display()
        )
    })?;
    let mut entry_count = 0usize;
    for camera_directory in camera_directories {
        entry_count = entry_count
            .checked_add(1)
            .context("important_state_incompatible: recording entry count overflowed")?;
        ensure!(
            entry_count <= 100_000,
            "important_state_incompatible: recording store exceeds the 100000-entry safety limit; recordings were preserved"
        );
        let camera_directory = camera_directory.map_err(|error| {
            anyhow::anyhow!(
                "important_state_incompatible: cannot enumerate recording root {}: {error}; recordings were preserved",
                root.display()
            )
        })?;
        let path = camera_directory.path();
        let metadata = fs::symlink_metadata(&path).map_err(|error| {
            anyhow::anyhow!(
                "important_state_incompatible: cannot inspect recording entry {}: {error}; recordings were preserved",
                path.display()
            )
        })?;
        let valid_camera_directory = metadata.is_dir()
            && !metadata.file_type().is_symlink()
            && path
                .file_name()
                .and_then(|value| value.to_str())
                .and_then(|value| Uuid::parse_str(value).ok())
                .is_some();
        ensure!(
            valid_camera_directory,
            "important_state_incompatible: unrecognized recording entry {}; recordings were preserved",
            path.display()
        );
        let recordings = fs::read_dir(&path).map_err(|error| {
            anyhow::anyhow!(
                "important_state_incompatible: cannot read recording directory {}: {error}; recordings were preserved",
                path.display()
            )
        })?;
        for recording in recordings {
            entry_count = entry_count
                .checked_add(1)
                .context("important_state_incompatible: recording entry count overflowed")?;
            ensure!(
                entry_count <= 100_000,
                "important_state_incompatible: recording store exceeds the 100000-entry safety limit; recordings were preserved"
            );
            let recording = recording.map_err(|error| {
                anyhow::anyhow!(
                    "important_state_incompatible: cannot enumerate recording directory {}: {error}; recordings were preserved",
                    path.display()
                )
            })?;
            let recording_path = recording.path();
            let metadata = fs::symlink_metadata(&recording_path).map_err(|error| {
                anyhow::anyhow!(
                    "important_state_incompatible: cannot inspect recording {}: {error}; recordings were preserved",
                    recording_path.display()
                )
            })?;
            let is_mp4 = recording_path
                .extension()
                .and_then(|value| value.to_str())
                .is_some_and(|value| value.eq_ignore_ascii_case("mp4"));
            ensure!(
                metadata.is_file() && !metadata.file_type().is_symlink() && is_mp4,
                "important_state_incompatible: unrecognized recording file {}; recordings were preserved",
                recording_path.display()
            );
        }
    }
    Ok(())
}

fn save_state(path: &Path, state: &LocalState) -> anyhow::Result<()> {
    let bytes = serde_json::to_vec_pretty(state)?;
    ensure!(
        bytes.len() as u64 <= MAX_INPUT_BYTES,
        "config exceeds 1 MiB"
    );
    let parent = path.parent().context("config path has no parent")?;
    fs::create_dir_all(parent)?;
    let temporary = parent.join(format!(".sentinel-client-{}.tmp", Uuid::new_v4()));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&temporary)?;
    let result = (|| {
        file.write_all(&bytes)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temporary, path)
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result.context("persist Sentinel configuration")
}

fn default_config_path() -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        PathBuf::from(std::env::var_os("ProgramData").unwrap_or_else(|| "C:\\ProgramData".into()))
            .join("SentinelClient/config.json")
    }
    #[cfg(target_os = "macos")]
    {
        PathBuf::from("/Library/Application Support/SentinelClient/config.json")
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        PathBuf::from("/etc/isarmg/sentinel-client/config.json")
    }
}

fn recording_root() -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        PathBuf::from(std::env::var_os("ProgramData").unwrap_or_else(|| "C:\\ProgramData".into()))
            .join("SentinelClient/recordings")
    }
    #[cfg(target_os = "macos")]
    {
        PathBuf::from("/Library/Application Support/SentinelClient/recordings")
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        PathBuf::from("/var/lib/isarmg/sentinel-client/recordings")
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn state_with(cameras: Vec<Camera>) -> LocalState {
        LocalState {
            format: 3,
            installation_id: Uuid::new_v4(),
            instances: cameras
                .into_iter()
                .map(|camera| CameraInstance {
                    server: "https://sentinel.example/".to_owned(),
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
        assert!(validate_server_origin("https://sentinel.example").is_ok());
        assert!(validate_server_origin("http://192.0.2.1").is_err());
        assert!(validate_server_origin("https://user@sentinel.example").is_err());
    }

    #[test]
    fn server_origin_rejects_non_root_paths_and_request_metadata() {
        for suffix in ["/admin", "/api/v2", "/camera/", "/?token=x", "/#camera"] {
            assert!(validate_server_origin(&format!("https://sentinel.example{suffix}")).is_err());
        }
        assert_eq!(
            validate_server_origin(" https://sentinel.example:8443/ ")
                .unwrap()
                .as_str(),
            "https://sentinel.example:8443/"
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
        let temporary = tempfile::tempdir().unwrap();
        let path = temporary.path().join("config.json");
        fs::create_dir(&path).unwrap();
        assert!(save_state(&path, &state_with(Vec::new())).is_err());
        let entries: Vec<_> = fs::read_dir(temporary.path()).unwrap().collect();
        assert_eq!(entries.len(), 1);
        assert!(path.is_dir());
    }

    #[test]
    fn oversized_state_does_not_create_temporary_credentials() {
        let temporary = tempfile::tempdir().unwrap();
        let path = temporary.path().join("config.json");
        let mut state = state_with(Vec::new());
        state.instances.push(CameraInstance {
            server: "https://sentinel.example".into(),
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
        let directory = tempfile::tempdir().unwrap();
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

    #[test]
    fn publish_grants_require_encrypted_scoped_urls() {
        assert!(validate_publish_url("rtsps://sentinel.example:8322/camera?jwt=token").is_ok());
        assert!(validate_publish_url("rtsp://sentinel.example:8554/camera?jwt=token").is_err());
        assert!(
            validate_publish_url("rtsps://user:password@sentinel.example:8322/camera?jwt=token")
                .is_err()
        );
        assert!(validate_publish_url("rtsps://sentinel.example:8322/camera").is_err());
    }

    #[test]
    fn old_or_corrupt_account_state_requires_explicit_repair() {
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.json");
        fs::write(&path, br#"{"format":2,"legacy":true}"#).unwrap();
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
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.json");
        let original = br#"{"format":1,"account":"old"}"#;
        fs::write(&path, original).unwrap();
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
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("config.json");
        let id = Uuid::new_v4();
        let mut state = state_with(vec![camera(id, "front")]);
        state.instances[0].camera.as_mut().unwrap().id = Uuid::new_v4();
        fs::write(&path, serde_json::to_vec(&state).unwrap()).unwrap();

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
        let directory = tempfile::tempdir().unwrap();
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
}
