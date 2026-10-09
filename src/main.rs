mod command_ledger;
use xcoc::{camera_presets, device, local_camera, onvif};
#[cfg(windows)]
mod windows_config_acl;
#[cfg(windows)]
mod windows_service_host;

use anyhow::{Context, ensure};
use clap::{Parser, Subcommand};
use device::CapabilityStatus;
use device::{
    Camera, DeviceCapabilities, DeviceIdentity, ResolvedDevice, StorageMode, StreamDescriptor,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
#[cfg(not(any(unix, windows)))]
use std::io::Write;
use std::{
    collections::{HashMap, HashSet},
    fs::{self, OpenOptions},
    io::{self, Read},
    path::{Path, PathBuf},
    process::Stdio,
    sync::Arc,
    time::{Duration, Instant, SystemTime},
};
use tokio::process::{Child, Command};
use tokio::sync::watch;
use url::Url;
use uuid::Uuid;
use zeroize::Zeroizing;

const PROTOCOL: &str = env!("XCOC_EDGE_PROTOCOL");
const API_PREFIX: &str = env!("XCOC_API_PREFIX");
const PRODUCT: &str = "xcos";
const MAX_INPUT_BYTES: u64 = 1024 * 1024;
const MAX_URL_BYTES: usize = 4_096;
const MAX_AUTHORIZATION_CODE_BYTES: usize = 36;
const MAX_NAME_BYTES: usize = 256;
const MAX_LOCATION_BYTES: usize = 512;
const MAX_USERNAME_BYTES: usize = 256;
const MAX_PASSWORD_BYTES: usize = 4_096;
const MAX_STORAGE_MODE_BYTES: usize = 16;
const MAX_PAIRING_RESPONSE_BYTES: usize = 16 * 1024;
const MAX_SNAPSHOT_RESPONSE_BYTES: usize = 1024 * 1024;

#[derive(Parser)]
#[command(name = "xcoc", version, about = "Xcoc camera edge client")]
struct Cli {
    #[arg(long, global = true, default_value_os_t = default_config_path())]
    config: PathBuf,
    #[command(subcommand)]
    command: CommandKind,
}

#[derive(Subcommand)]
enum CommandKind {
    /// Pair with a xcos using protected JSON from stdin.
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
    Run {
        /// Keep a background launch alive until setup creates its configuration.
        #[arg(long, hide = true)]
        wait_for_config: bool,
    },
    /// Show pairing and camera state without secrets.
    Status,
    /// Read persistent Windows service diagnostics without changing state.
    #[cfg(windows)]
    Logs {
        #[arg(long, default_value_t=100, value_parser=clap::value_parser!(u16).range(1..=1000))]
        tail: u16,
        #[arg(long)]
        since: Option<String>,
        #[arg(long)]
        instance_id: Option<Uuid>,
        #[arg(long)]
        event: Option<String>,
        #[arg(long)]
        request_id: Option<String>,
        #[arg(long)]
        task_id: Option<String>,
        #[arg(long)]
        level: Option<String>,
        #[arg(long)]
        follow: bool,
        #[arg(long, default_value = "json")]
        format: String,
        #[arg(long, default_value = "60s")]
        timeout: String,
    },
    /// Remove one local pairing and stop its camera while preserving recordings.
    Unpair { instance_id: Uuid },
    /// Configure and start the installed Windows service using saved pairing state.
    #[cfg(windows)]
    Service {
        /// Keep the service stopped after the next reboot unless started manually.
        #[arg(long)]
        no_boot_start: bool,
    },
    /// Manage locally-owned camera configuration.
    Camera {
        #[command(subcommand)]
        command: CameraCommand,
    },
}

#[derive(Subcommand)]
enum CameraCommand {
    /// List documented camera families and example models.
    Presets,
    /// Enumerate USB and built-in cameras on this computer.
    Devices,
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
    command_capacity: usize,
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

type SnapshotTaskResult = (u64, Uuid, Vec<Uuid>, anyhow::Result<SnapshotResponse>);
type CommandTaskResult = (
    u64,
    Uuid,
    Vec<(Uuid, CommandResult)>,
    HashMap<Uuid, (CommandResult, chrono::DateTime<chrono::Utc>)>,
);

#[derive(Clone, Copy, Eq, PartialEq)]
struct ConfigFingerprint {
    len: u64,
    modified: SystemTime,
    created: Option<SystemTime>,
    #[cfg(unix)]
    device: u64,
    #[cfg(unix)]
    inode: u64,
}

fn validate_snapshot_response(
    response: SnapshotResponse,
    instance_id: Uuid,
) -> anyhow::Result<SnapshotResponse> {
    ensure!(
        response.protocol == PROTOCOL && !response.accepted_at.is_empty(),
        "Server returned an incompatible snapshot response"
    );
    ensure!(
        response.publish.len() <= 2
            && response.commands.len() <= 100
            && response
                .publish
                .iter()
                .all(|grant| grant.camera_id == instance_id)
            && response
                .commands
                .iter()
                .all(|command| command.camera_id == instance_id),
        "Server returned snapshot actions for another camera instance or too many actions"
    );
    Ok(response)
}

#[derive(Clone, Deserialize)]
#[serde(deny_unknown_fields)]
struct PublishGrant {
    camera_id: Uuid,
    profile: String,
    publish_url: String,
}

#[derive(Clone, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct DeviceCommand {
    id: Uuid,
    camera_id: Uuid,
    kind: String,
    payload: Value,
    expires_at: String,
}

#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum CommandOutcome {
    Succeeded,
    Failed,
    Unknown,
}
#[derive(Clone, Copy, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
enum CommandErrorCode {
    InvalidCommand,
    UnsupportedCapability,
    ExpiredBeforeExecution,
    DeviceUnavailable,
    DeviceRejected,
    OutcomeUnknown,
}
#[derive(Clone, Debug, Eq, PartialEq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
struct CommandResult {
    id: Uuid,
    outcome: CommandOutcome,
    error_code: Option<CommandErrorCode>,
}
impl CommandResult {
    fn failed(id: Uuid, code: CommandErrorCode) -> Self {
        Self {
            id,
            outcome: CommandOutcome::Failed,
            error_code: Some(code),
        }
    }
    fn unknown(id: Uuid) -> Self {
        Self {
            id,
            outcome: CommandOutcome::Unknown,
            error_code: Some(CommandErrorCode::OutcomeUnknown),
        }
    }
    fn validate(&self) -> anyhow::Result<()> {
        ensure!(
            matches!(
                (self.outcome, self.error_code),
                (CommandOutcome::Succeeded, None)
                    | (
                        CommandOutcome::Unknown,
                        Some(CommandErrorCode::OutcomeUnknown)
                    )
            ) || matches!(self.outcome, CommandOutcome::Failed)
                && self
                    .error_code
                    .is_some_and(|code| code != CommandErrorCode::OutcomeUnknown),
            "invalid command result contract"
        );
        Ok(())
    }
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
    generation: Uuid,
    resolving: bool,
}

impl RuntimeCamera {
    fn should_resolve(&self, now: Instant) -> bool {
        !self.resolving && (self.resolved.is_none() || self.error.is_some()) && now >= self.retry_at
    }

    fn mark_error(&mut self, error: String) {
        self.error = Some(error);
        self.retry_at = Instant::now() + Duration::from_secs(30);
    }
}

fn main() -> std::process::ExitCode {
    match run_main() {
        Ok(()) => std::process::ExitCode::SUCCESS,
        Err(error) => {
            let mut failure = public_cli_failure(&error);
            if runtime_event(
                "xcoc.command.failed",
                None,
                xcss_log::Level::Error,
                "Client command failed.",
                failure.code,
            )
            .is_err()
            {
                failure = xcsc_cli::fail(11, "diagnostics_unavailable");
            }
            let exit = xcsc_cli::emit("xcoc", "command", "human", &Err(failure), &XcocErrors);
            std::process::ExitCode::from(exit)
        }
    }
}

struct XcocErrors;
impl xcsc_cli::ProductErrorCatalog for XcocErrors {
    fn message(&self, code: &'static str) -> Option<&'static str> {
        Some(match code {
            "pairing_state_incompatible" => {
                "The saved pairing requires explicit repair; its original bytes were preserved."
            }
            "configuration_state_incompatible" => {
                "The camera configuration requires repair and was preserved."
            }
            "important_state_incompatible" => {
                "The recording store requires inspection; its contents were preserved."
            }
            "pairing_protocol_unsupported" | "pairing_server_upgrade_required" => {
                "The Server and Client protocols do not match; install the matching release."
            }
            "pairing_authorization_rejected" => "The Server rejected the authorization code.",
            "pairing_server_unavailable" => "The Server is temporarily unavailable.",
            "pairing_response_too_large" => "The pairing response exceeded its size limit.",
            "pairing_endpoint_not_found"
            | "pairing_http_method_rejected"
            | "pairing_request_rejected"
            | "pairing_unexpected_http_status" => {
                "The Server rejected the pairing request; check its address and matching release."
            }
            "DISCOVERY_LIMIT_EXCEEDED" => {
                "Discovery stopped because too many devices replied; narrow the network and retry."
            }
            "DEVICE_BACKPRESSURE" => {
                "Device command capacity is full; new actions were not admitted."
            }
            "DEVICE_TIMEOUT" => "Device communication timed out.",
            "DEVICE_RESPONSE_TOO_LARGE" => "The device response exceeded its size limit.",
            "DEVICE_CONNECTION_FAILED" => "The device connection failed.",
            "DEVICE_AUTHORIZATION_FAILED" => "The device rejected the configured authorization.",
            "DEVICE_PERMISSION_DENIED" => {
                "The operation requires the service account or administrator permissions."
            }
            "DEPENDENCY_MISSING" => "A required device dependency is missing.",
            "diagnostics_unavailable" => "The Client could not write its diagnostic event.",
            "operation_failed" | "DEVICE_IO_FAILED" | "DEVICE_REQUEST_FAILED" => {
                "The Client operation failed; inspect saved state and service diagnostics before retrying."
            }
            "command_state_incompatible" => {
                "The durable device command state requires inspection; physical actions were stopped and state was preserved."
            }
            _ => return None,
        })
    }
    fn next_step(&self, _product: &str, error: &xcsc_cli::Failure) -> Option<String> {
        Some(match error.code {
            "pairing_state_incompatible" => "Inspect the protected pairing archive, then use xcoc setup --interactive --replace with a new authorization code.",
            "important_state_incompatible" | "command_state_incompatible" => "Inspect the preserved recordings and durable device command state; repair the storage problem without deleting pending evidence.",
            "configuration_state_incompatible" => "Correct the protected camera configuration, then run xcoc camera list.",
            _ if error.committed => "Pairing was saved. Run xcoc status and use camera apply or service to resume; preserve the saved identity.",
            _ => "Run xcoc status and inspect the service diagnostics; verify configuration and dependencies before retrying.",
        }.to_owned())
    }
}

fn public_cli_failure(error: &anyhow::Error) -> xcsc_cli::Failure {
    let mut failure = if let Some(source) = error.downcast_ref::<xcsc_cli::Failure>() {
        xcsc_cli::fail(source.exit, source.code)
    } else {
        let known = [
            "command_state_incompatible",
            "pairing_state_incompatible",
            "configuration_state_incompatible",
            "important_state_incompatible",
            "pairing_protocol_unsupported",
            "pairing_authorization_rejected",
            "pairing_endpoint_not_found",
            "pairing_http_method_rejected",
            "pairing_server_upgrade_required",
            "pairing_server_unavailable",
            "pairing_request_rejected",
            "pairing_unexpected_http_status",
            "pairing_response_too_large",
        ];
        let code = error
            .chain()
            .find_map(|cause| {
                let value = cause.to_string();
                known
                    .into_iter()
                    .find(|code| value == *code || value.starts_with(&format!("{code}:")))
            })
            .unwrap_or_else(|| match device_error_code(error) {
                "DEVICE_OPERATION_FAILED" => "operation_failed",
                code => code,
            });
        xcsc_cli::fail(8, code)
    };
    failure.committed = error
        .downcast_ref::<xcsc_cli::Failure>()
        .is_some_and(|source| source.committed)
        || error.chain().any(|cause| {
            let context = cause.to_string();
            context.starts_with("pairing was committed,")
                || context.starts_with("pairing was saved,")
        });
    failure
}

fn run_main() -> anyhow::Result<()> {
    #[cfg(windows)]
    if std::env::args_os()
        .nth(1)
        .is_some_and(|arg| arg == "--windows-service")
    {
        ensure!(
            std::env::args_os().count() == 2,
            "unexpected service arguments"
        );
        return windows_service_host::dispatch();
    }
    let cli = Cli::parse();
    #[cfg(windows)]
    if let CommandKind::Logs {
        tail,
        since,
        instance_id,
        event,
        request_id,
        task_id,
        level,
        follow,
        format,
        timeout,
    } = &cli.command
    {
        let mut raw = vec![
            "logs".to_owned(),
            "--tail".into(),
            tail.to_string(),
            "--format".into(),
            format.clone(),
            "--timeout".into(),
            timeout.clone(),
        ];
        for (name, value) in [
            ("--since", since.as_deref()),
            ("--event", event.as_deref()),
            ("--request-id", request_id.as_deref()),
            ("--task-id", task_id.as_deref()),
            ("--level", level.as_deref()),
        ] {
            if let Some(value) = value {
                raw.extend([name.into(), value.into()]);
            }
        }
        if let Some(id) = instance_id {
            raw.extend(["--instance-id".into(), id.to_string()]);
        }
        if *follow {
            raw.push("--follow".into());
        }
        let args = xcsc_cli::Args::parse(raw, &[], &[])?;
        let parent = cli.config.parent().context("config parent missing")?;
        let exit = if *follow {
            ensure!(format == "ndjson", "follow_requires_logs_ndjson");
            xcsc_cli::follow_log_source("xcoc", args, &XcocErrors, |args| {
                windows_runtime_logs(args, parent)
            })
        } else {
            xcsc_cli::emit(
                "xcoc",
                "logs",
                format,
                &windows_runtime_logs(&args, parent),
                &XcocErrors,
            )
        };
        if exit != 0 {
            std::process::exit(i32::from(exit));
        }
        return Ok(());
    }
    tokio::runtime::Runtime::new()
        .context("start xcoc runtime")?
        .block_on(async_main(cli))
}

async fn async_main(cli: Cli) -> anyhow::Result<()> {
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
        CommandKind::Run { wait_for_config } => run(&cli.config, wait_for_config, None).await,
        CommandKind::Status => status(&cli.config),
        #[cfg(windows)]
        CommandKind::Logs { .. } => unreachable!("logs is dispatched before the async runtime"),
        CommandKind::Unpair { instance_id } => unpair(&cli.config, instance_id),
        #[cfg(windows)]
        CommandKind::Service { no_boot_start } => {
            ensure!(
                cli.config == default_config_path(),
                "the installed Windows service uses only the default configuration path"
            );
            windows_service_host::preflight_setup()?;
            windows_service_host::configure_and_start(!no_boot_start)
        }
        CommandKind::Camera { command } => camera_command(&cli.config, command).await,
    }
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
    #[cfg(windows)]
    if path == default_config_path() {
        windows_service_host::preflight_setup()?;
    }
    #[cfg(windows)]
    secure_existing_default_config(path)?;
    preflight_media_tools().await?;
    validate_recording_store(&recording_root())?;
    let (mut state, recover_incompatible_pairing) = if path.exists() {
        match load_state(path) {
            Ok(state) => (state, false),
            Err(error) if replace && is_pairing_state_incompatible(&error) => (
                LocalState {
                    format: 1,
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
                format: 1,
                installation_id: Uuid::new_v4(),
                instances: Vec::new(),
            },
            false,
        )
    };
    let input_deadline = Instant::now() + Duration::from_secs(timeout_seconds);
    let input: Zeroizing<SetupInput> = Zeroizing::new(if interactive {
        SetupInput {
            server: xcsc_cli::prompt_text("Server HTTPS origin", MAX_URL_BYTES, input_deadline)?,
            authorization_code: xcsc_cli::prompt_text(
                "Authorization code (visible)",
                MAX_AUTHORIZATION_CODE_BYTES,
                input_deadline,
            )?,
            name: xcsc_cli::prompt_text("Client name", MAX_NAME_BYTES, input_deadline)?,
        }
    } else {
        read_stdin_json()?
    });
    let server = validate_server_origin(&input.server)?;
    validate_name(&input.name)?;
    validate_authorization_code(&input.authorization_code)?;
    #[cfg(windows)]
    let boot_start = if path == default_config_path() && interactive {
        let choice = xcsc_cli::prompt_text("Start at boot? [Y/n]", 8, input_deadline)?;
        parse_boot_start_choice(&choice)?
    } else {
        true
    };
    preflight_state_write(path)?;
    ensure!(
        state.instances.len() < 256,
        "camera instance limit reached; remove an unused local pairing before requesting a new Server token"
    );
    let client = server_client()?;
    let response = client
        .post(server.join(&format!("{API_PREFIX}/client/pair"))?)
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
        save_state(path, &state).context("pairing was committed, but camera configuration could not be saved; use camera apply to resume")?;
    }
    #[cfg(windows)]
    if path == default_config_path() {
        windows_service_host::configure_and_start(boot_start).with_context(|| {
            let flag = if boot_start { "" } else { " --no-boot-start" };
            format!(
                "pairing was saved, but the Windows service could not be configured; run `xcoc service{flag}` as administrator to retry without a new authorization code"
            )
        })?;
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

#[cfg(any(windows, test))]
fn parse_boot_start_choice(value: &str) -> anyhow::Result<bool> {
    match value.trim().to_ascii_lowercase().as_str() {
        "" | "y" | "yes" => Ok(true),
        "n" | "no" => Ok(false),
        _ => anyhow::bail!("start-at-boot choice must be yes or no"),
    }
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
    let selection = xcsc_cli::prompt_text(
        if discovered.is_empty() {
            "No ONVIF camera found; enter a manual RTSP main-stream URL"
        } else {
            "Select an ONVIF camera number, or enter a manual RTSP main-stream URL"
        },
        MAX_URL_BYTES,
        deadline,
    )?;
    let name = xcsc_cli::prompt_text("Camera name", MAX_NAME_BYTES, deadline)?;
    let location =
        xcsc_cli::prompt_text("Camera location (optional)", MAX_LOCATION_BYTES, deadline)?;
    let storage_mode = match xcsc_cli::prompt_text(
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
        xcsc_cli::prompt_text("Camera username (optional)", MAX_USERNAME_BYTES, deadline)?;
    let password =
        xcsc_cli::prompt_secret("Camera password (optional)", MAX_PASSWORD_BYTES, deadline)?
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
        let sub = xcsc_cli::prompt_text("RTSP sub-stream URL (optional)", MAX_URL_BYTES, deadline)?;
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
    #[cfg(windows)]
    secure_existing_default_config(path)?;
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

fn unpair(path: &Path, instance_id: Uuid) -> anyhow::Result<()> {
    #[cfg(windows)]
    secure_existing_default_config(path)?;
    let mut state = load_state(path)?;
    let original_len = state.instances.len();
    state
        .instances
        .retain(|instance| instance.instance_id != instance_id);
    ensure!(
        state.instances.len() != original_len,
        "camera instance is not paired"
    );
    if state.instances.is_empty() {
        fs::remove_file(path).context("remove final Xcoc pairing")?;
    } else {
        save_state(path, &state)?;
    }
    println!("removed local pairing {instance_id}; recordings were preserved");
    Ok(())
}

async fn camera_command(path: &Path, command: CameraCommand) -> anyhow::Result<()> {
    match command {
        CameraCommand::Presets => {
            println!(
                "{}",
                serde_json::to_string_pretty(&camera_presets::catalog())?
            );
            return Ok(());
        }
        CameraCommand::Devices => {
            println!(
                "{}",
                serde_json::to_string_pretty(&local_camera::list_devices().await?)?
            );
            return Ok(());
        }
        _ => {}
    }
    #[cfg(windows)]
    secure_existing_default_config(path)?;
    validate_recording_store(&recording_root())?;
    let mut state = load_state(path)?;
    match command {
        CameraCommand::Presets | CameraCommand::Devices => {
            unreachable!("read-only catalog handled above")
        }
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

async fn run(
    path: &Path,
    wait_for_config: bool,
    mut shutdown: Option<watch::Receiver<bool>>,
) -> anyhow::Result<()> {
    let service_mode = shutdown.is_some();
    #[cfg(windows)]
    secure_existing_default_config(path)?;
    #[cfg(unix)]
    let mut sigterm = tokio::signal::unix::signal(tokio::signal::unix::SignalKind::terminate())
        .context("listen for xcoc termination")?;
    #[cfg(unix)]
    let mut termination = Box::pin(async move {
        tokio::select! {
            _ = tokio::signal::ctrl_c() => {}
            _ = sigterm.recv() => {}
        }
    });
    #[cfg(not(unix))]
    let mut termination = Box::pin(async {
        let _ = tokio::signal::ctrl_c().await;
    });
    let Some(_run_lock) = acquire_run_lock(path)? else {
        runtime_event(
            "xcoc.runtime.already_running",
            None,
            xcss_log::Level::Info,
            "Another runtime already owns this configuration.",
            "INSTANCE_BUSY",
        )?;
        return Ok(());
    };
    let mut last_configuration_error = String::new();
    #[cfg(windows)]
    let mut recording_acl_ready = false;
    #[cfg(windows)]
    let mut config_acl_ready = path.exists();
    let mut state = loop {
        let loaded = (|| {
            validate_recording_store(&recording_root())?;
            #[cfg(windows)]
            if !recording_acl_ready {
                windows_config_acl::secure_recording_tree(&recording_root())?;
                recording_acl_ready = true;
            }
            #[cfg(windows)]
            if !config_acl_ready && path == default_config_path() && path.exists() {
                windows_config_acl::secure_config_file(path)?;
                config_acl_ready = true;
            }
            load_state(path)
        })();
        match loaded {
            Ok(state) => break state,
            Err(error) if !wait_for_config => return Err(error),
            Err(error) => {
                let message = error.to_string();
                if message != last_configuration_error {
                    runtime_event(
                        "xcoc.config.unavailable",
                        None,
                        xcss_log::Level::Warn,
                        "Waiting for a valid protected configuration.",
                        "CONFIGURATION_UNAVAILABLE",
                    )?;
                    last_configuration_error = message;
                }
                tokio::select! {
                    _ = &mut termination, if !service_mode => return Ok(()),
                    _ = wait_for_shutdown(&mut shutdown) => return Ok(()),
                    _ = tokio::time::sleep(Duration::from_secs(2)) => {}
                }
            }
        }
    };
    let server_client = server_client()?;
    let device_client = device_client()?;
    let mut children: HashMap<String, Child> = HashMap::new();
    let mut runtime = state
        .instances
        .iter()
        .filter_map(|instance| instance.camera.as_ref())
        .map(|camera| (camera.id, new_runtime_camera()))
        .collect::<HashMap<_, _>>();
    let ledger = command_ledger::CommandLedger::open(path)?;
    let (mut command_results, mut completed_commands) = ledger.receipts();
    let command_ledger = Arc::new(tokio::sync::Mutex::new(ledger));
    let mut publish_grants: HashMap<Uuid, Vec<PublishGrant>> = HashMap::new();
    let mut resolve_tasks = tokio::task::JoinSet::new();
    let mut snapshot_tasks: tokio::task::JoinSet<SnapshotTaskResult> = tokio::task::JoinSet::new();
    let mut snapshot_inflight = HashSet::new();
    let mut snapshot_command_reservations: HashMap<Uuid, usize> = HashMap::new();
    let mut snapshot_abort_handles: HashMap<Uuid, tokio::task::AbortHandle> = HashMap::new();
    let mut command_tasks: tokio::task::JoinSet<anyhow::Result<CommandTaskResult>> =
        tokio::task::JoinSet::new();
    let mut command_abort_handles: HashMap<Uuid, tokio::task::AbortHandle> = HashMap::new();
    let mut command_queues: HashMap<Uuid, Vec<DeviceCommand>> = HashMap::new();
    let mut command_inflight_ids: HashMap<Uuid, Uuid> = HashMap::new();
    let mut snapshot_revisions = state
        .instances
        .iter()
        .map(|instance| (instance.instance_id, 0_u64))
        .collect::<HashMap<_, _>>();
    let mut config_stamp = config_fingerprint(path)?;
    let mut interval = tokio::time::interval(Duration::from_secs(2));
    let outcome = loop {
        let has_pending_snapshots = !snapshot_tasks.is_empty();
        let has_pending_commands = !command_tasks.is_empty();
        let (tick, completed_snapshot, completed_command) = tokio::select! {
            _ = &mut termination, if !service_mode => break Ok(()),
            _ = wait_for_shutdown(&mut shutdown) => break Ok(()),
            _ = interval.tick() => (true, None, None),
            completed = snapshot_tasks.join_next(), if has_pending_snapshots => (false, completed, None),
            completed = command_tasks.join_next(), if has_pending_commands => (false, None, completed),
        };
        let process_tick = async {
            let mut changed = HashSet::new();
            let needs_reload = tick || config_fingerprint(path)? != config_stamp;
            if needs_reload {
                changed =
                    reload_configuration(path, &mut state, &mut runtime, &mut children).await?;
                config_stamp = config_fingerprint(path)?;
            }
            if needs_reload {
                if !changed.is_empty() {
                    resolve_tasks.abort_all();
                    for camera in runtime.values_mut() {
                        camera.resolving = false;
                    }
                    for id in &changed {
                        *snapshot_revisions.entry(*id).or_default() += 1;
                        if let Some(handle) = snapshot_abort_handles.remove(id) {
                            handle.abort();
                        }
                        if let Some(handle) = command_abort_handles.remove(id) {
                            handle.abort();
                        }
                        command_queues.remove(id);
                        snapshot_inflight.remove(id);
                        snapshot_command_reservations.remove(id);
                        publish_grants.remove(id);
                    }
                    let canceled_commands = command_inflight_ids
                        .iter()
                        .filter(|(_, camera_id)| changed.contains(camera_id))
                        .map(|(command_id, camera_id)| (*command_id, *camera_id))
                        .collect::<Vec<_>>();
                    for (command_id, camera_id) in canceled_commands {
                        command_inflight_ids.remove(&command_id);
                        if state
                            .instances
                            .iter()
                            .any(|instance| instance.instance_id == camera_id)
                        {
                            let result = command_ledger.lock().await.cancel(command_id)?;
                            completed_commands.insert(
                                command_id,
                                (
                                    result.clone(),
                                    chrono::Utc::now() + chrono::Duration::seconds(120),
                                ),
                            );
                            command_results
                                .entry(camera_id)
                                .or_default()
                                .insert(command_id, result);
                        }
                    }
                }
                reap_children(&mut children, &mut runtime).await;
                collect_resolved_cameras(&mut resolve_tasks, &mut runtime)?;
                launch_due_cameras(&device_client, &state, &mut runtime, &mut resolve_tasks);
                // Local recording runs independently of Server snapshot success.
                reconcile_local_recorders(&state, &mut runtime, &mut children).await;
            }
            let configured_instances = state
                .instances
                .iter()
                .filter(|value| value.camera.is_some())
                .map(|value| value.instance_id)
                .collect::<HashSet<_>>();
            let grant_count = publish_grants.len();
            publish_grants.retain(|instance_id, _| configured_instances.contains(instance_id));
            let mut grants_changed = !changed.is_empty() || publish_grants.len() != grant_count;
            let mut commands = Vec::new();
            let paired_ids = state
                .instances
                .iter()
                .map(|instance| instance.instance_id)
                .collect::<HashSet<_>>();
            command_results.retain(|id, _| paired_ids.contains(id));
            let mut completed = completed_snapshot;
            loop {
                let result = completed.take().or_else(|| snapshot_tasks.try_join_next());
                let Some(result) = result else { break };
                let (revision, instance_id, sent_ids, result) = match result {
                    Ok(result) => result,
                    Err(error) => {
                        if error.is_cancelled() {
                            continue;
                        }
                        runtime_event(
                            "xcoc.snapshot.task_failed",
                            None,
                            xcss_log::Level::Error,
                            "A snapshot worker failed.",
                            "TASK_FAILED",
                        )?;
                        snapshot_tasks.abort_all();
                        snapshot_inflight.clear();
                        snapshot_command_reservations.clear();
                        snapshot_abort_handles.clear();
                        for revision in snapshot_revisions.values_mut() {
                            *revision += 1;
                        }
                        break;
                    }
                };
                if snapshot_revisions.get(&instance_id) != Some(&revision) {
                    continue;
                }
                snapshot_inflight.remove(&instance_id);
                snapshot_command_reservations.remove(&instance_id);
                snapshot_abort_handles.remove(&instance_id);
                match result {
                    Ok(response) => {
                        command_ledger
                            .lock()
                            .await
                            .acknowledge(instance_id, &sent_ids)?;
                        acknowledge_command_results(&mut command_results, instance_id, &sent_ids);
                        publish_grants.insert(instance_id, response.publish);
                        grants_changed = true;
                        commands.extend(response.commands);
                    }
                    Err(error) => {
                        runtime_event(
                            "xcoc.snapshot.request_failed",
                            Some(instance_id),
                            xcss_log::Level::Warn,
                            "The Server snapshot request failed.",
                            device_error_code(&error),
                        )?;
                    }
                }
            }
            if tick {
                for instance in &state.instances {
                    if !snapshot_inflight.insert(instance.instance_id) {
                        continue;
                    }
                    let pending = command_results
                        .get(&instance.instance_id)
                        .map(pending_result_batch)
                        .unwrap_or_default();
                    let sent_ids = pending.iter().map(|result| result.id).collect::<Vec<_>>();
                    let mut request = build_snapshot_request(instance, &runtime, pending);
                    request.command_capacity = available_command_capacity(
                        instance.instance_id,
                        &completed_commands,
                        &command_inflight_ids,
                        &command_results,
                        &snapshot_command_reservations,
                        &commands,
                    );
                    request.command_capacity = request.command_capacity.min(
                        command_ledger.lock().await.remaining().saturating_sub(
                            command_inflight_ids.len()
                                + snapshot_command_reservations.values().sum::<usize>()
                                + commands.len(),
                        ),
                    );
                    snapshot_command_reservations
                        .insert(instance.instance_id, request.command_capacity);
                    let client = server_client.clone();
                    let server = instance.server.clone();
                    let access_token = Zeroizing::new(instance.access_token.clone());
                    let instance_id = instance.instance_id;
                    let revision = *snapshot_revisions.entry(instance_id).or_default();
                    let handle = snapshot_tasks.spawn(async move {
                        let result = send_snapshot(
                            &client,
                            &server,
                            access_token.as_str(),
                            instance_id,
                            request,
                        )
                        .await;
                        (revision, instance_id, sent_ids, result)
                    });
                    snapshot_abort_handles.insert(instance_id, handle);
                }
            }
            if grants_changed {
                let grants = publish_grants
                    .values()
                    .flatten()
                    .cloned()
                    .collect::<Vec<_>>();
                reconcile_publishers(&mut runtime, &grants, &mut children).await;
            }
            let mut completed = completed_command;
            loop {
                let result = completed.take().or_else(|| command_tasks.try_join_next());
                let Some(result) = result else { break };
                let (revision, camera_id, results, recently_completed) = match result {
                    Ok(Ok(result)) => result,
                    Ok(Err(error)) => return Err(error),
                    Err(error) => {
                        if error.is_cancelled() {
                            continue;
                        }
                        runtime_event(
                            "xcoc.command.task_failed",
                            None,
                            xcss_log::Level::Error,
                            "A device command worker failed.",
                            "TASK_FAILED",
                        )?;
                        command_tasks.abort_all();
                        command_abort_handles.clear();
                        command_queues.clear();
                        command_inflight_ids.clear();
                        return Err(anyhow::anyhow!(
                            "command_state_incompatible: command worker stopped unexpectedly; durable state was preserved"
                        ));
                    }
                };
                if snapshot_revisions.get(&camera_id) != Some(&revision) {
                    continue;
                }
                command_abort_handles.remove(&camera_id);
                for (result_camera_id, result) in results {
                    command_inflight_ids.remove(&result.id);
                    command_results
                        .entry(result_camera_id)
                        .or_default()
                        .insert(result.id, result);
                }
                completed_commands.extend(recently_completed);
            }
            completed_commands.retain(|_, (_, expires_at)| *expires_at > chrono::Utc::now());
            enqueue_device_commands(
                commands.clone(),
                &completed_commands,
                &mut command_inflight_ids,
                &mut command_queues,
                &mut command_results,
            )?;
            command_ledger.lock().await.admit(&commands)?;
            launch_queued_command_tasks(
                &runtime,
                &mut command_queues,
                &mut command_abort_handles,
                &mut snapshot_revisions,
                &mut command_tasks,
                &command_ledger,
            );
            Ok::<(), anyhow::Error>(())
        };
        tokio::select! {
            _ = &mut termination, if !service_mode => break Ok(()),
            _ = wait_for_shutdown(&mut shutdown) => break Ok(()),
            result = process_tick => {
                if let Err(error) = result {
                    break Err(error);
                }
            }
        }
    };
    for (_, mut child) in children {
        let _ = child.kill().await;
    }
    outcome
}

const MAX_INFLIGHT_COMMANDS: usize = 256;
const MAX_INFLIGHT_PER_CAMERA: usize = 8;
const MAX_COMMAND_RECORDS: usize = 4096;

#[derive(Debug)]
struct CommandBackpressure;
impl std::fmt::Display for CommandBackpressure {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        formatter.write_str("device command capacity exceeded; new actions were not admitted")
    }
}
impl std::error::Error for CommandBackpressure {}

fn available_command_capacity(
    camera_id: Uuid,
    completed: &HashMap<Uuid, (CommandResult, chrono::DateTime<chrono::Utc>)>,
    inflight: &HashMap<Uuid, Uuid>,
    results: &HashMap<Uuid, HashMap<Uuid, CommandResult>>,
    reservations: &HashMap<Uuid, usize>,
    incoming: &[DeviceCommand],
) -> usize {
    let reserved = reservations.values().sum::<usize>();
    let result_count = results.values().map(HashMap::len).sum::<usize>();
    let used = 2 * (inflight.len() + reserved + incoming.len()) + completed.len() + result_count;
    let camera_used = inflight.values().filter(|id| **id == camera_id).count()
        + incoming
            .iter()
            .filter(|command| command.camera_id == camera_id)
            .count();
    100.min(MAX_INFLIGHT_COMMANDS.saturating_sub(inflight.len() + reserved + incoming.len()))
        .min(MAX_INFLIGHT_PER_CAMERA.saturating_sub(camera_used))
        .min(MAX_COMMAND_RECORDS.saturating_sub(used) / 2)
}

fn enqueue_device_commands(
    commands: Vec<DeviceCommand>,
    completed: &HashMap<Uuid, (CommandResult, chrono::DateTime<chrono::Utc>)>,
    inflight_ids: &mut HashMap<Uuid, Uuid>,
    queues: &mut HashMap<Uuid, Vec<DeviceCommand>>,
    results: &mut HashMap<Uuid, HashMap<Uuid, CommandResult>>,
) -> anyhow::Result<()> {
    // Validate the whole batch before mutating any queue. Every admitted ID
    // reserves two records for its eventual pending receipt and deduplication.
    let mut used = 2 * inflight_ids.len()
        + completed.len()
        + results.values().map(HashMap::len).sum::<usize>();
    let mut new_ids = HashSet::new();
    let mut global = inflight_ids.len();
    let mut cameras: HashMap<Uuid, usize> = HashMap::new();
    for id in inflight_ids.values() {
        *cameras.entry(*id).or_default() += 1;
    }
    for command in &commands {
        if !new_ids.insert(command.id) {
            continue;
        }
        if completed.contains_key(&command.id) {
            if !results
                .get(&command.camera_id)
                .is_some_and(|receipts| receipts.contains_key(&command.id))
            {
                used += 1;
            }
        } else if !inflight_ids.contains_key(&command.id) {
            global += 1;
            used += 2;
            *cameras.entry(command.camera_id).or_default() += 1;
        }
        if global > MAX_INFLIGHT_COMMANDS
            || used > MAX_COMMAND_RECORDS
            || cameras
                .values()
                .any(|count| *count > MAX_INFLIGHT_PER_CAMERA)
        {
            return Err(CommandBackpressure.into());
        }
    }
    for command in commands {
        if let Some((result, _)) = completed.get(&command.id) {
            results
                .entry(command.camera_id)
                .or_default()
                .insert(command.id, result.clone());
        } else if let std::collections::hash_map::Entry::Vacant(entry) =
            inflight_ids.entry(command.id)
        {
            entry.insert(command.camera_id);
            queues.entry(command.camera_id).or_default().push(command);
        }
    }
    Ok(())
}

fn launch_queued_command_tasks(
    runtime: &HashMap<Uuid, RuntimeCamera>,
    queues: &mut HashMap<Uuid, Vec<DeviceCommand>>,
    handles: &mut HashMap<Uuid, tokio::task::AbortHandle>,
    revisions: &mut HashMap<Uuid, u64>,
    tasks: &mut tokio::task::JoinSet<anyhow::Result<CommandTaskResult>>,
    ledger: &Arc<tokio::sync::Mutex<command_ledger::CommandLedger>>,
) {
    let ready_ids = queues
        .keys()
        .filter(|id| !handles.contains_key(id))
        .copied()
        .collect::<Vec<_>>();
    for camera_id in ready_ids {
        let queued = queues.remove(&camera_id).unwrap_or_default();
        if queued.is_empty() {
            continue;
        }
        let mut command_runtime = HashMap::new();
        if let Some(current) = runtime.get(&camera_id) {
            command_runtime.insert(
                camera_id,
                RuntimeCamera {
                    resolved: current.resolved.clone(),
                    error: None,
                    retry_at: Instant::now(),
                    generation: current.generation,
                    resolving: false,
                },
            );
        }
        let revision = *revisions.entry(camera_id).or_default();
        let ledger = Arc::clone(ledger);
        let handle = tasks.spawn(async move {
            let mut recently_completed = HashMap::new();
            let results =
                execute_commands(&command_runtime, queued, &mut recently_completed, &ledger)
                    .await?;
            Ok((revision, camera_id, results, recently_completed))
        });
        handles.insert(camera_id, handle);
    }
}

fn pending_result_batch(results: &HashMap<Uuid, CommandResult>) -> Vec<CommandResult> {
    results.values().take(100).cloned().collect()
}

fn acknowledge_command_results(
    pending: &mut HashMap<Uuid, HashMap<Uuid, CommandResult>>,
    instance_id: Uuid,
    sent_ids: &[Uuid],
) {
    if let Some(results) = pending.get_mut(&instance_id) {
        for id in sent_ids {
            results.remove(id);
        }
        if results.is_empty() {
            pending.remove(&instance_id);
        }
    }
}

async fn wait_for_shutdown(shutdown: &mut Option<watch::Receiver<bool>>) {
    if let Some(receiver) = shutdown {
        while !*receiver.borrow_and_update() {
            if receiver.changed().await.is_err() {
                break;
            }
        }
    } else {
        std::future::pending::<()>().await;
    }
}

fn acquire_run_lock(path: &Path) -> anyhow::Result<Option<xcsc_runtime::SingleInstanceLock>> {
    let default_lock_parent = recording_root();
    let parent = if path == default_config_path() {
        default_lock_parent
            .parent()
            .context("recording path has no parent")?
    } else {
        path.parent().context("config path has no parent")?
    };
    fs::create_dir_all(parent).context("create Xcoc configuration directory")?;
    let private = xcsc_fs_safety::PrivateDirectory::create(
        std::path::absolute(parent)?.join(".xcoc-runtime"),
    )
    .context("open protected Xcoc runtime state")?;
    match xcsc_runtime::SingleInstanceLock::acquire(&private) {
        Ok(lock) => Ok(Some(lock)),
        Err(xcsc_runtime::Error::AlreadyRunning) => Ok(None),
        Err(error) => Err(error).context("lock Xcoc runtime"),
    }
}

fn new_runtime_camera() -> RuntimeCamera {
    RuntimeCamera {
        resolved: None,
        error: None,
        retry_at: Instant::now(),
        generation: Uuid::new_v4(),
        resolving: false,
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

    let current_instances = current
        .instances
        .iter()
        .map(|instance| Ok((instance.instance_id, serde_json::to_vec(instance)?)))
        .collect::<anyhow::Result<HashMap<_, _>>>()?;
    let next_instances = next
        .instances
        .iter()
        .map(|instance| Ok((instance.instance_id, serde_json::to_vec(instance)?)))
        .collect::<anyhow::Result<HashMap<_, _>>>()?;
    let changed = current_instances
        .keys()
        .chain(next_instances.keys())
        .filter(|id| current_instances.get(id) != next_instances.get(id))
        .copied()
        .collect::<HashSet<_>>();

    for id in &changed {
        runtime.remove(id);
        if next
            .instances
            .iter()
            .any(|instance| instance.instance_id == *id && instance.camera.is_some())
        {
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
) -> anyhow::Result<HashSet<Uuid>> {
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
    Ok(changed)
}

fn build_snapshot_request(
    instance: &CameraInstance,
    runtime: &HashMap<Uuid, RuntimeCamera>,
    command_results: Vec<CommandResult>,
) -> SnapshotRequest {
    let cameras = instance
        .camera
        .as_ref()
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
                .unwrap_or_else(DeviceCapabilities::unknown);
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
        .into_iter()
        .collect();
    SnapshotRequest {
        command_capacity: 0,
        protocol: PROTOCOL,
        cameras,
        command_results,
    }
}

async fn send_snapshot(
    client: &reqwest::Client,
    server: &str,
    access_token: &str,
    instance_id: Uuid,
    request: SnapshotRequest,
) -> anyhow::Result<SnapshotResponse> {
    let url = Url::parse(server)?.join(&format!("{API_PREFIX}/client/snapshot"))?;
    let mut response = client
        .put(url)
        .bearer_auth(access_token)
        .json(&request)
        .send()
        .await
        .context("snapshot request failed")?
        .error_for_status()
        .context("snapshot rejected")?;
    if response
        .content_length()
        .is_some_and(|length| length > MAX_SNAPSHOT_RESPONSE_BYTES as u64)
    {
        anyhow::bail!("snapshot response exceeds size limit");
    }
    let mut bytes = Vec::new();
    while let Some(chunk) = response.chunk().await.context("read snapshot response")? {
        ensure!(
            bytes
                .len()
                .checked_add(chunk.len())
                .is_some_and(|length| length <= MAX_SNAPSHOT_RESPONSE_BYTES),
            "snapshot response exceeds size limit"
        );
        bytes.extend_from_slice(&chunk);
    }
    let response: SnapshotResponse =
        serde_json::from_slice(&bytes).context("invalid snapshot response")?;
    if response.commands.len() > request.command_capacity {
        return Err(CommandBackpressure.into());
    }
    validate_snapshot_response(response, instance_id)
}

async fn reconcile_publishers(
    runtime: &mut HashMap<Uuid, RuntimeCamera>,
    grants: &[PublishGrant],
    children: &mut HashMap<String, Child>,
) {
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
        if !runtime
            .get(&grant.camera_id)
            .is_some_and(|camera| camera.error.is_none() && camera.resolved.is_some())
        {
            continue;
        }
        let started = (|| {
            validate_publish_url(&grant.publish_url)?;
            let source = runtime
                .get(&grant.camera_id)
                .and_then(|runtime| runtime.resolved.as_ref())
                .expect("resolved camera was checked above")
                .stream_source(&grant.profile)?;
            spawn_publisher(&source, &grant.publish_url)
        })();
        match started {
            Ok(child) => {
                children.insert(key, child);
            }
            Err(error) => {
                if let Some(camera) = runtime.get_mut(&grant.camera_id) {
                    camera.mark_error(safe_error(&error));
                }
            }
        }
    }
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
    runtime: &mut HashMap<Uuid, RuntimeCamera>,
    children: &mut HashMap<String, Child>,
) {
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
                .filter(|runtime| runtime.error.is_none())
                .and_then(|runtime| runtime.resolved.as_ref())
                .and_then(|device| device.stream_source("main").ok())
            else {
                continue;
            };
            match spawn_recorder(&source, camera.id) {
                Ok(child) => {
                    entry.insert(child);
                }
                Err(error) => {
                    if let Some(current) = runtime.get_mut(&camera.id) {
                        current.mark_error(safe_error(&error));
                    }
                }
            }
        }
    }
}

fn spawn_publisher(source: &device::MediaSource, destination: &str) -> anyhow::Result<Child> {
    Command::new("ffmpeg")
        .args(["-nostdin", "-hide_banner", "-loglevel", "warning"])
        .args(source.input_args())
        .args([
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
        .stderr(Stdio::null())
        .kill_on_drop(true)
        .spawn()
        .context("start ffmpeg publisher")
}

fn spawn_recorder(source: &device::MediaSource, camera_id: Uuid) -> anyhow::Result<Child> {
    let root = prepare_recording_directory(&recording_root(), camera_id)?;
    let pattern = root.join("%Y-%m-%d_%H-%M-%S.mp4");
    let mut command = Command::new("ffmpeg");
    command
        .args(["-nostdin", "-hide_banner", "-loglevel", "warning"])
        .args(source.input_args())
        .args([
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
        .stderr(Stdio::null())
        .kill_on_drop(true);
    #[cfg(unix)]
    private_child_umask(&mut command);
    command.spawn().context("start ffmpeg recorder")
}

fn prepare_recording_directory(store: &Path, camera_id: Uuid) -> anyhow::Result<PathBuf> {
    fs::create_dir_all(store).context("create Xcoc recording store")?;
    let metadata = fs::symlink_metadata(store).context("inspect Xcoc recording store")?;
    ensure!(
        metadata.is_dir() && !metadata.file_type().is_symlink(),
        "recording store is not a regular directory"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(store, fs::Permissions::from_mode(0o700))?;
    }
    let directory = store.join(camera_id.to_string());
    fs::create_dir_all(&directory).context("create camera recording directory")?;
    let metadata =
        fs::symlink_metadata(&directory).context("inspect camera recording directory")?;
    ensure!(
        metadata.is_dir() && !metadata.file_type().is_symlink(),
        "camera recording path is not a regular directory"
    );
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        fs::set_permissions(&directory, fs::Permissions::from_mode(0o700))?;
    }
    Ok(directory)
}

#[cfg(unix)]
fn private_child_umask(command: &mut Command) {
    // SAFETY: umask only changes the forked child process and is async-signal-safe.
    unsafe {
        command.pre_exec(|| {
            libc::umask(0o077);
            Ok(())
        });
    }
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
            camera.mark_error("media process exited; retrying".to_owned());
        }
    }
}

type ResolveResult = (Uuid, Uuid, anyhow::Result<ResolvedDevice>);

fn launch_due_cameras(
    client: &reqwest::Client,
    state: &LocalState,
    runtime: &mut HashMap<Uuid, RuntimeCamera>,
    tasks: &mut tokio::task::JoinSet<ResolveResult>,
) {
    for camera in state
        .instances
        .iter()
        .filter_map(|instance| instance.camera.as_ref())
        .filter(|camera| camera.enabled)
    {
        if tasks.len() >= 32 {
            break;
        }
        let should_resolve = runtime
            .get(&camera.id)
            .is_some_and(|current| current.should_resolve(Instant::now()));
        if !should_resolve {
            continue;
        }
        let current = runtime
            .get_mut(&camera.id)
            .expect("runtime mirrors camera config");
        current.resolving = true;
        let generation = current.generation;
        // A publishing failure must not reopen an occupied physical camera or
        // interrupt its independent local recorder. Reprobe the existing relay.
        let existing_capture = current.resolved.as_ref().filter(|device| {
            device.streams.iter().any(|stream| matches!(&stream.source, device::MediaSource::Local(relay) if relay.is_alive()))
        }).cloned();
        if existing_capture.is_none()
            && matches!(camera.adapter, device::AdapterConfig::LocalCamera { .. })
        {
            current.resolved = None;
        }
        let camera = camera.clone();
        let client = client.clone();
        tasks.spawn(async move {
            let resolved = tokio::time::timeout(Duration::from_secs(60), async {
                let mut device = match existing_capture {
                    Some(device) => device,
                    None => camera.adapter().resolve(&client).await?,
                };
                device.probe_streams().await?;
                Ok(device)
            })
            .await
            .unwrap_or_else(|_| Err(anyhow::anyhow!("camera resolution timed out")));
            (camera.id, generation, resolved)
        });
    }
}

fn collect_resolved_cameras(
    tasks: &mut tokio::task::JoinSet<ResolveResult>,
    runtime: &mut HashMap<Uuid, RuntimeCamera>,
) -> anyhow::Result<()> {
    while let Some(result) = tasks.try_join_next() {
        match result {
            Ok((id, generation, resolved)) => {
                let Some(current) = runtime
                    .get_mut(&id)
                    .filter(|value| value.generation == generation)
                else {
                    continue;
                };
                current.resolving = false;
                match resolved {
                    Ok(device) => {
                        current.resolved = Some(device);
                        current.error = None;
                    }
                    Err(error) => {
                        runtime_event(
                            "xcoc.device.connection_failed",
                            Some(id),
                            xcss_log::Level::Warn,
                            "Camera resolution or stream probing failed.",
                            device_error_code(&error),
                        )?;
                        current.mark_error(safe_error(&error));
                    }
                }
            }
            Err(_error) => {
                runtime_event(
                    "xcoc.device.task_failed",
                    None,
                    xcss_log::Level::Error,
                    "A camera resolution worker failed.",
                    "TASK_FAILED",
                )?;
                for current in runtime.values_mut() {
                    current.resolving = false;
                }
            }
        }
    }
    Ok(())
}

fn runtime_event(
    event: &str,
    instance: Option<Uuid>,
    level: xcss_log::Level,
    message: &str,
    code: &str,
) -> anyhow::Result<()> {
    let record = if let Some(id) = instance {
        xcss_log::LogRecord::instance("xcoc", "runtime", event, message, level, &id.to_string())?
            .with_instance_type("camera")?
    } else {
        xcss_log::LogRecord::server("xcoc", "runtime", event, message, level)?
    };
    record.with_error_code(&code.to_ascii_lowercase())?.emit()?;
    Ok(())
}

fn device_error_code(error: &anyhow::Error) -> &'static str {
    if error.downcast_ref::<CommandBackpressure>().is_some() {
        "DEVICE_BACKPRESSURE"
    } else if error.downcast_ref::<onvif::DiscoveryLimit>().is_some() {
        "DISCOVERY_LIMIT_EXCEEDED"
    } else if let Some(error) = error.downcast_ref::<xcsc_runtime::process::ProcessCaptureError>() {
        use xcsc_runtime::process::ProcessCaptureError;
        match error {
            ProcessCaptureError::Timeout => "DEVICE_TIMEOUT",
            ProcessCaptureError::OutputLimit(_) => "DEVICE_RESPONSE_TOO_LARGE",
            ProcessCaptureError::Spawn(io::ErrorKind::NotFound) => "DEPENDENCY_MISSING",
            ProcessCaptureError::Spawn(io::ErrorKind::PermissionDenied) => {
                "DEVICE_PERMISSION_DENIED"
            }
            _ => "DEVICE_IO_FAILED",
        }
    } else if let Some(error) = error.downcast_ref::<reqwest::Error>() {
        if error.is_timeout() {
            "DEVICE_TIMEOUT"
        } else if error.is_connect() {
            "DEVICE_CONNECTION_FAILED"
        } else if error
            .status()
            .is_some_and(|status| matches!(status.as_u16(), 401 | 403))
        {
            "DEVICE_AUTHORIZATION_FAILED"
        } else {
            "DEVICE_REQUEST_FAILED"
        }
    } else if let Some(error) = error.downcast_ref::<std::io::Error>() {
        match error.kind() {
            io::ErrorKind::PermissionDenied => "DEVICE_PERMISSION_DENIED",
            io::ErrorKind::TimedOut => "DEVICE_TIMEOUT",
            io::ErrorKind::NotFound => "DEPENDENCY_MISSING",
            _ => "DEVICE_IO_FAILED",
        }
    } else {
        "DEVICE_OPERATION_FAILED"
    }
}

async fn execute_commands(
    runtime: &HashMap<Uuid, RuntimeCamera>,
    commands: Vec<DeviceCommand>,
    completed: &mut HashMap<Uuid, (CommandResult, chrono::DateTime<chrono::Utc>)>,
    ledger: &Arc<tokio::sync::Mutex<command_ledger::CommandLedger>>,
) -> anyhow::Result<Vec<(Uuid, CommandResult)>> {
    let mut results = Vec::with_capacity(commands.len());
    for command in commands {
        if let Some(result) = ledger.lock().await.replay(&command)? {
            results.push((command.camera_id, result));
            continue;
        }
        if let Some((result, _)) = completed.get(&command.id) {
            results.push((command.camera_id, result.clone()));
            continue;
        }
        let expires_at = chrono::DateTime::parse_from_rfc3339(&command.expires_at)
            .map(|value| value.with_timezone(&chrono::Utc));
        if !matches!(&expires_at, Ok(value) if *value > chrono::Utc::now()) {
            let result =
                CommandResult::failed(command.id, CommandErrorCode::ExpiredBeforeExecution);
            ledger.lock().await.complete(&result)?;
            results.push((command.camera_id, result));
            continue;
        }
        let expires_at = expires_at.expect("checked above");
        let camera_id = command.camera_id;
        let device = runtime
            .get(&camera_id)
            .and_then(|runtime| runtime.resolved.as_ref());
        let result = match device {
            None => CommandResult::failed(command.id, CommandErrorCode::DeviceUnavailable),
            Some(_) if command.kind != "ptz" => {
                CommandResult::failed(command.id, CommandErrorCode::InvalidCommand)
            }
            Some(device) if device.capabilities.ptz != CapabilityStatus::Supported => {
                CommandResult::failed(command.id, CommandErrorCode::UnsupportedCapability)
            }
            Some(device) => match serde_json::from_value::<device::PtzCommand>(command.payload) {
                Err(_) => CommandResult::failed(command.id, CommandErrorCode::InvalidCommand),
                Ok(payload)
                    if ![payload.pan, payload.tilt, payload.zoom]
                        .iter()
                        .all(|value| value.is_finite() && (-1.0..=1.0).contains(value)) =>
                {
                    CommandResult::failed(command.id, CommandErrorCode::InvalidCommand)
                }
                Ok(payload) => {
                    ledger.lock().await.begin(command.id)?;
                    command_event(camera_id, command.id, "xcoc.command.started", None)?;
                    match device.ptz(payload).await {
                        Ok(()) => CommandResult {
                            id: command.id,
                            outcome: CommandOutcome::Succeeded,
                            error_code: None,
                        },
                        // Transmission, timeout and incomplete replies cannot prove
                        // that a physical action was rejected or never occurred.
                        Err(_) => CommandResult::unknown(command.id),
                    }
                }
            },
        };
        ledger.lock().await.complete(&result)?;
        command_event(
            camera_id,
            command.id,
            "xcoc.command.completed",
            Some(&result),
        )?;
        let outcome = (camera_id, result);
        // The Server can redeliver while it waits for a result after the device
        // command's execution deadline. Keep the physical action deduplicated
        // throughout that acknowledgement window.
        let dedup_until = expires_at
            .checked_add_signed(chrono::Duration::seconds(120))
            .unwrap_or(expires_at);
        completed.insert(outcome.1.id, (outcome.1.clone(), dedup_until));
        results.push(outcome);
    }
    Ok(results)
}

fn command_event(
    camera_id: Uuid,
    id: Uuid,
    event: &str,
    result: Option<&CommandResult>,
) -> anyhow::Result<()> {
    let record = xcss_log::LogRecord::instance(
        "xcoc",
        "device-command",
        event,
        "Device command execution state changed.",
        xcss_log::Level::Info,
        &camera_id.to_string(),
    )?
    .with_task_id(&id.to_string())?
    .with_instance_type("camera")?;
    let record = if let Some(result) = result {
        record
            .with_attribute("outcome", serde_json::to_value(result.outcome)?)?
            .with_attribute("error_code", serde_json::to_value(result.error_code)?)?
    } else {
        record
    };
    record.emit()?;
    Ok(())
}

fn safe_error(error: &anyhow::Error) -> String {
    match device_error_code(error) {
        "DEVICE_TIMEOUT" => "Device communication timed out.",
        "DEVICE_CONNECTION_FAILED" => "The device connection failed.",
        "DEVICE_AUTHORIZATION_FAILED" => "The device rejected the configured authorization.",
        "DEVICE_REQUEST_FAILED" => "The device request failed.",
        "DEVICE_RESPONSE_TOO_LARGE" => "The device response exceeded its size limit.",
        "DEVICE_BACKPRESSURE" => "Device command capacity is full; new actions were not admitted.",
        "DEVICE_PERMISSION_DENIED" => "Permission is required for the device operation.",
        "DEPENDENCY_MISSING" => "A required device dependency is missing.",
        "DEVICE_IO_FAILED" => "Device input or output failed.",
        _ => "The device operation failed.",
    }
    .to_owned()
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
        value.len() == MAX_AUTHORIZATION_CODE_BYTES
            && value
                .bytes()
                .all(|byte| byte.is_ascii_digit() || byte.is_ascii_lowercase()),
        "authorization_code must be 36 lowercase ASCII letters or digits"
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
        .user_agent(concat!("xcoc/", env!("CARGO_PKG_VERSION")))
        .build()?)
}

fn device_client() -> anyhow::Result<reqwest::Client> {
    Ok(reqwest::Client::builder()
        .timeout(Duration::from_secs(10))
        .connect_timeout(Duration::from_secs(10))
        .redirect(reqwest::redirect::Policy::none())
        .no_proxy()
        .user_agent(concat!("xcoc/", env!("CARGO_PKG_VERSION")))
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

fn config_fingerprint(path: &Path) -> anyhow::Result<Option<ConfigFingerprint>> {
    let metadata = match fs::symlink_metadata(path) {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(error).context("inspect Xcoc configuration revision"),
    };
    ensure!(
        metadata.is_file() && !metadata.file_type().is_symlink(),
        "Xcoc configuration revision is not a regular file"
    );
    #[cfg(unix)]
    use std::os::unix::fs::MetadataExt;
    Ok(Some(ConfigFingerprint {
        len: metadata.len(),
        modified: metadata.modified()?,
        created: metadata.created().ok(),
        #[cfg(unix)]
        device: metadata.dev(),
        #[cfg(unix)]
        inode: metadata.ino(),
    }))
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
    ensure!(
        metadata.len() <= MAX_INPUT_BYTES,
        "pairing_state_incompatible: config exceeds 1 MiB; run setup --replace to archive it and pair again"
    );
    let bytes = read_configuration_bytes(path).map_err(|_| {
        anyhow::anyhow!(
            "pairing_state_incompatible: config cannot be safely read; check file ownership, permissions and links"
        )
    })?;
    ensure!(
        bytes.len() as u64 <= MAX_INPUT_BYTES,
        "pairing_state_incompatible: config exceeds 1 MiB; run setup --replace to archive it and pair again"
    );
    let document: Value = serde_json::from_slice(&bytes).map_err(|_| {
        anyhow::anyhow!(
            "pairing_state_incompatible: config JSON is corrupt; run setup --replace to archive it and pair again"
        )
    })?;
    ensure!(
        document.get("format").and_then(Value::as_u64) == Some(1),
        "pairing_state_incompatible: config format is not supported; run setup --replace to archive it and pair again"
    );
    let state: LocalState = serde_json::from_value(document).map_err(|_| {
        anyhow::anyhow!(
            "pairing_state_incompatible: config schema is not supported; run setup --replace to archive it and pair again"
        )
    })?;
    ensure!(
        state.format == 1 && !state.installation_id.is_nil(),
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

fn read_configuration_bytes(path: &Path) -> anyhow::Result<Vec<u8>> {
    #[cfg(unix)]
    {
        use xcsc_fs_safety::{ConfigurationDirectory, EntryName};
        let directory =
            ConfigurationDirectory::open(path.parent().context("config path has no parent")?)?;
        let name = EntryName::new(path.file_name().context("config file name is missing")?)?;
        Ok(directory.read_bounded(&name, MAX_INPUT_BYTES as usize)?)
    }
    #[cfg(windows)]
    {
        use xcsc_fs_safety::{EntryName, PrivateDirectory};
        let directory = PrivateDirectory::open_existing(std::path::absolute(
            path.parent().context("config parent missing")?,
        )?)?;
        Ok(directory.read_private_bounded(
            &EntryName::new(path.file_name().context("config filename missing")?)?,
            MAX_INPUT_BYTES as usize,
        )?)
    }
    #[cfg(not(any(unix, windows)))]
    {
        let mut bytes = Vec::new();
        fs::File::open(path)?
            .take(MAX_INPUT_BYTES + 1)
            .read_to_end(&mut bytes)?;
        ensure!(
            bytes.len() as u64 <= MAX_INPUT_BYTES,
            "config exceeds 1 MiB"
        );
        Ok(bytes)
    }
}

#[cfg(windows)]
fn secure_existing_default_config(path: &Path) -> anyhow::Result<()> {
    if path == default_config_path() {
        let parent = path.parent().context("config path has no parent")?;
        windows_config_acl::secure_config_directory(parent)?;
        if path.exists() {
            windows_config_acl::secure_config_file(path)?;
        }
    }
    Ok(())
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
    #[cfg(windows)]
    if path == default_config_path()
        && let Err(error) = windows_config_acl::secure_config_file(&archive)
    {
        let _ = fs::rename(&archive, path);
        return Err(error).context("restrict archived Xcoc credentials");
    }
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
    save_configuration_bytes(path, &bytes)
}

fn save_configuration_bytes(path: &Path, bytes: &[u8]) -> anyhow::Result<()> {
    ensure!(
        bytes.len() as u64 <= MAX_INPUT_BYTES,
        "protected state exceeds 1 MiB"
    );
    let parent = path.parent().context("config path has no parent")?;
    fs::create_dir_all(parent)?;
    #[cfg(unix)]
    {
        use xcsc_fs_safety::{ConfigurationDirectory, EntryName};
        let directory = ConfigurationDirectory::open(parent)?;
        let name = EntryName::new(path.file_name().context("config file name is missing")?)?;
        directory
            .replace(&name, bytes)
            .context("persist Xcoc configuration")
    }
    #[cfg(windows)]
    {
        use xcsc_fs_safety::{AtomicFile, EntryName, PrivateDirectory};
        if path.parent() == default_config_path().parent() {
            windows_config_acl::secure_config_directory(parent)?;
        }
        let directory = PrivateDirectory::open_existing(std::path::absolute(parent)?)?;
        AtomicFile::replace(
            &directory,
            &EntryName::new(path.file_name().context("config filename missing")?)?.as_relative(),
            bytes,
        )
        .context("persist Xcoc configuration")
    }
    #[cfg(not(any(unix, windows)))]
    {
        anyhow::bail!("protected configuration storage is unsupported on this platform")
    }
}

fn preflight_state_write(path: &Path) -> anyhow::Result<()> {
    let parent = path.parent().context("config path has no parent")?;
    fs::create_dir_all(parent).context("create Xcoc configuration directory")?;
    #[cfg(unix)]
    xcsc_fs_safety::ConfigurationDirectory::open(parent)
        .context("open protected configuration directory before pairing")?;
    #[cfg(windows)]
    if path == default_config_path() {
        windows_config_acl::secure_config_directory(parent)?;
    }
    match fs::symlink_metadata(path) {
        Ok(metadata) => ensure!(
            metadata.is_file() && !metadata.file_type().is_symlink(),
            "configuration path must be a regular non-symlink file before pairing"
        ),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(error).context("inspect configuration path before pairing"),
    }
    let probe = parent.join(format!(".xcoc-preflight-{}.tmp", Uuid::new_v4()));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options
        .open(&probe)
        .context("configuration directory is not writable before pairing")?;
    drop(file);
    fs::remove_file(&probe).context("remove configuration write probe")?;
    Ok(())
}

fn default_config_path() -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        PathBuf::from(std::env::var_os("ProgramData").unwrap_or_else(|| "C:\\ProgramData".into()))
            .join("XcocClient/config.json")
    }
    #[cfg(target_os = "macos")]
    {
        PathBuf::from("/Library/Application Support/XcocClient/config.json")
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        PathBuf::from("/etc/isarmg/xcoc/config.json")
    }
}

fn recording_root() -> PathBuf {
    #[cfg(target_os = "windows")]
    {
        PathBuf::from(std::env::var_os("ProgramData").unwrap_or_else(|| "C:\\ProgramData".into()))
            .join("XcocClient/recordings")
    }
    #[cfg(target_os = "macos")]
    {
        PathBuf::from("/Library/Application Support/XcocClient/recordings")
    }
    #[cfg(all(unix, not(target_os = "macos")))]
    {
        PathBuf::from("/var/lib/isarmg/xcoc/recordings")
    }
}

#[cfg(test)]
#[path = "main/tests.rs"]
mod tests;

#[cfg(windows)]
fn windows_runtime_logs(
    args: &xcsc_cli::Args,
    path: &std::path::Path,
) -> xcsc_cli::Result<serde_json::Value> {
    use xcsc_cli::{fail, storage_error};
    use xcsc_fs_safety::{EntryName, Error, PrivateDirectory};
    let directory = PrivateDirectory::open_existing(path.join("logs")).map_err(storage_error)?;
    let level = args
        .get("--level")
        .map(|value| {
            serde_json::from_value::<xcss_log::Level>(serde_json::json!(value.to_ascii_uppercase()))
        })
        .transpose()
        .map_err(|_| fail(2, "invalid_log_level"))?;
    xcsc_cli::query_rotating_logs(
        args,
        "xcoc",
        |name| match directory.read_private_bounded(
            &EntryName::new(name).map_err(storage_error)?,
            8 * 1024 * 1024,
        ) {
            Ok(bytes) => Ok(Some(bytes)),
            Err(Error::Io(error)) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(_) => Err(fail(8, "unsafe_or_unreadable_runtime_log")),
        },
        |bytes| {
            xcss_log::query(
                std::io::Cursor::new(bytes),
                xcss_log::LogFilter {
                    since: args.get("--since"),
                    instance_id: args.get("--instance-id"),
                    event: args.get("--event"),
                    request_id: args.get("--request-id"),
                    task_id: args.get("--task-id"),
                    minimum_level: level,
                    ..Default::default()
                },
                xcss_log::QueryLimits {
                    max_input_bytes: 1024 * 1024,
                    max_records: 16384,
                },
            )
            .map_err(|_| fail(8, "log_integrity_or_filter_failed"))?
            .into_iter()
            .map(|record| serde_json::to_value(record).map_err(storage_error))
            .collect()
        },
    )
}

#[cfg(all(test, windows))]
mod protected_windows_state_tests {
    use super::*;
    #[test]
    fn windows_configuration_and_ledger_use_verified_private_atomic_storage() {
        let temp = tempfile::tempdir().unwrap();
        let directory =
            xcsc_fs_safety::PrivateDirectory::create(temp.path().join("state")).unwrap();
        let config = directory.path().join("config.json");
        save_configuration_bytes(&config, b"private-fixture").unwrap();
        assert_eq!(
            read_configuration_bytes(&config).unwrap(),
            b"private-fixture"
        );
        save_configuration_bytes(&config, b"replacement").unwrap();
        assert_eq!(read_configuration_bytes(&config).unwrap(), b"replacement");
        let ledger = directory.path().join("config.json.commands.json");
        save_configuration_bytes(&ledger, b"private-ledger").unwrap();
        assert_eq!(
            read_configuration_bytes(&ledger).unwrap(),
            b"private-ledger"
        );
        std::fs::hard_link(&config, directory.path().join("alias.json")).unwrap();
        assert!(read_configuration_bytes(&config).is_err());
        assert!(save_configuration_bytes(&config, b"must-not-publish").is_err());
        assert_eq!(std::fs::read(&config).unwrap(), b"replacement");
    }
}
