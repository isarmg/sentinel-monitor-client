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

const PROTOCOL: &str = "sentinel-edge-v2";
const PRODUCT: &str = "sentinel-monitor";
const MAX_INPUT_BYTES: u64 = 1024 * 1024;

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
        replace: bool,
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
}

#[derive(Subcommand)]
enum CameraCommand {
    /// Add or replace a camera from a protected JSON document on stdin.
    Apply {
        #[arg(long)]
        input_stdin: bool,
    },
    /// List cameras without RTSP URLs or credentials.
    List,
    /// Remove a local camera; the next snapshot removes it from the Server.
    Remove { id: Uuid },
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
    server: String,
    installation_id: Uuid,
    client_id: Uuid,
    access_token: String,
    name: String,
    cameras: Vec<Camera>,
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
            replace,
        } => setup(&cli.config, input_stdin, replace).await,
        CommandKind::Run => run(&cli.config).await,
        CommandKind::Status => status(&cli.config),
        CommandKind::Camera { command } => camera_command(&cli.config, command).await,
    }
}

async fn setup(path: &Path, input_stdin: bool, replace: bool) -> anyhow::Result<()> {
    ensure!(
        input_stdin,
        "setup requires --input-stdin so secrets never appear in arguments"
    );
    ensure!(
        !path.exists() || replace,
        "this installation is already paired; after Server authorization rotation use setup --replace"
    );
    let existing = if path.exists() {
        Some(load_state(path)?)
    } else {
        None
    };
    let input: Zeroizing<SetupInput> = Zeroizing::new(read_stdin_json()?);
    let server = validate_server_origin(&input.server)?;
    validate_name(&input.name)?;
    validate_authorization_code(&input.authorization_code)?;
    let installation_id = existing
        .as_ref()
        .map(|state| state.installation_id)
        .unwrap_or_else(Uuid::new_v4);
    let client = http_client()?;
    let response = client
        .post(server.join("/api/v2/client/pair")?)
        .json(&PairRequest {
            protocol: PROTOCOL,
            product: PRODUCT,
            installation_id,
            name: input.name.trim(),
            client_version: env!("CARGO_PKG_VERSION"),
            authorization_code: &input.authorization_code,
        })
        .send()
        .await
        .context("pairing request failed")?
        .error_for_status()
        .context("pairing was rejected")?
        .json::<PairResponse>()
        .await
        .context("invalid pairing response")?;
    ensure!(
        response.protocol == PROTOCOL
            && !response.client_id.is_nil()
            && response.access_token.len() == 43,
        "Server returned an incompatible pairing response"
    );
    let state = LocalState {
        format: 2,
        server: server.as_str().trim_end_matches('/').into(),
        installation_id,
        client_id: response.client_id,
        access_token: response.access_token,
        name: input.name.trim().into(),
        cameras: existing.map(|state| state.cameras).unwrap_or_default(),
    };
    save_state(path, &state)?;
    println!("paired client {}", state.client_id);
    Ok(())
}

fn status(path: &Path) -> anyhow::Result<()> {
    let state = load_state(path)?;
    println!(
        "{}",
        serde_json::to_string_pretty(&serde_json::json!({
            "paired": true,
            "server": state.server,
            "client_id": state.client_id,
            "installation_id": state.installation_id,
            "name": state.name,
            "cameras": state.cameras.len(),
        }))?
    );
    Ok(())
}

async fn camera_command(path: &Path, command: CameraCommand) -> anyhow::Result<()> {
    let mut state = load_state(path)?;
    match command {
        CameraCommand::Apply { input_stdin } => {
            ensure!(input_stdin, "camera apply requires --input-stdin");
            let mut camera: Camera = read_stdin_json()?;
            camera.validate()?;
            if camera.id.is_nil() {
                camera.id = Uuid::new_v4();
            }
            if let Some(existing) = state.cameras.iter_mut().find(|value| value.id == camera.id) {
                *existing = camera;
            } else {
                ensure!(state.cameras.len() < 256, "camera limit reached");
                state.cameras.push(camera);
            }
            save_state(path, &state)?;
        }
        CameraCommand::List => {
            let views = state
                .cameras
                .iter()
                .map(|camera| CameraView {
                    id: camera.id,
                    name: &camera.name,
                    location: &camera.location,
                    adapter_kind: camera.adapter_kind(),
                    manufacturer: camera.manufacturer.as_deref(),
                    model: camera.model.as_deref(),
                    enabled: camera.enabled,
                    storage_mode: camera.storage_mode.as_str(),
                })
                .collect::<Vec<_>>();
            println!("{}", serde_json::to_string_pretty(&views)?);
        }
        CameraCommand::Remove { id } => {
            let before = state.cameras.len();
            state.cameras.retain(|camera| camera.id != id);
            ensure!(state.cameras.len() != before, "camera not found");
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
    let state = load_state(path)?;
    ensure!(
        !state.cameras.is_empty(),
        "configure at least one camera before run"
    );
    let client = http_client()?;
    let mut children: HashMap<String, Child> = HashMap::new();
    let mut runtime = state
        .cameras
        .iter()
        .map(|camera| {
            (
                camera.id,
                RuntimeCamera {
                    resolved: None,
                    error: None,
                    retry_at: Instant::now(),
                },
            )
        })
        .collect::<HashMap<_, _>>();
    let mut command_results = Vec::new();
    let mut interval = tokio::time::interval(Duration::from_secs(2));
    loop {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => break,
            _ = interval.tick() => {
                reap_children(&mut children, &mut runtime).await;
                resolve_due_cameras(&client, &state, &mut runtime).await;
                let response = match send_snapshot(&client, &state, &runtime, &command_results).await {
                    Ok(response) => response,
                    Err(error) => {
                        eprintln!("snapshot failed: {error:#}");
                        continue;
                    }
                };
                command_results.clear();
                reconcile_streams(&state, &mut runtime, &response.publish, &mut children).await?;
                command_results.extend(execute_commands(&runtime, response.commands).await);
            }
        }
    }
    for (_, mut child) in children {
        let _ = child.kill().await;
    }
    Ok(())
}

async fn send_snapshot(
    client: &reqwest::Client,
    state: &LocalState,
    runtime: &HashMap<Uuid, RuntimeCamera>,
    command_results: &[CommandResult],
) -> anyhow::Result<SnapshotResponse> {
    let cameras = state
        .cameras
        .iter()
        .map(|camera| {
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
        })
        .collect();
    let url = Url::parse(&state.server)?.join("/api/v2/client/snapshot")?;
    let response = client
        .put(url)
        .bearer_auth(&state.access_token)
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

async fn reconcile_streams(
    state: &LocalState,
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

    for camera in state
        .cameras
        .iter()
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
    for camera in state.cameras.iter().filter(|camera| camera.enabled) {
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
) -> Vec<CommandResult> {
    let mut results = Vec::with_capacity(commands.len());
    for command in commands {
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
        results.push(match result {
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
        });
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
            && url.query().is_none()
            && url.fragment().is_none(),
        "server must be an origin without credentials, query or fragment"
    );
    #[cfg(debug_assertions)]
    let allowed = url.scheme() == "https"
        || (url.scheme() == "http"
            && matches!(url.host_str(), Some("127.0.0.1" | "::1" | "localhost")));
    #[cfg(not(debug_assertions))]
    let allowed = url.scheme() == "https";
    ensure!(
        allowed && url.host_str().is_some(),
        "server must use trusted HTTPS"
    );
    url.set_path("/");
    Ok(url)
}

fn http_client() -> anyhow::Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .timeout(Duration::from_secs(20))
        .redirect(reqwest::redirect::Policy::none())
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
    let metadata = fs::symlink_metadata(path).context("client is not configured; run setup")?;
    ensure!(
        metadata.is_file() && !metadata.file_type().is_symlink(),
        "config must be a regular file without symlinks"
    );
    let bytes = fs::read(path)?;
    ensure!(
        bytes.len() as u64 <= MAX_INPUT_BYTES,
        "config exceeds 1 MiB"
    );
    let state: LocalState = serde_json::from_slice(&bytes)?;
    ensure!(
        state.format == 2 && !state.client_id.is_nil() && state.access_token.len() == 43,
        "config is not the current format"
    );
    Ok(state)
}

fn save_state(path: &Path, state: &LocalState) -> anyhow::Result<()> {
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
    let bytes = serde_json::to_vec_pretty(state)?;
    ensure!(
        bytes.len() as u64 <= MAX_INPUT_BYTES,
        "config exceeds 1 MiB"
    );
    file.write_all(&bytes)?;
    file.sync_all()?;
    drop(file);
    fs::rename(&temporary, path)?;
    Ok(())
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
}
