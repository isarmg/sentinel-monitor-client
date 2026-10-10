//! Desktop media subprocess protocol. Endpoints travel only through inherited
//! stdin, never through argv, environment variables, or temporary files.
use crate::device::MediaSource;
use anyhow::{Context, ensure};
use serde::{Deserialize, Serialize};
use std::{
    io::{Read, Write},
    path::Path,
    process::{ExitCode, Stdio},
    time::Duration,
};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    process::{Child, Command},
};
use xcsc::runtime::process::{OutputStream, ProcessCaptureError};
use zeroize::{Zeroize, ZeroizeOnDrop, Zeroizing};

const REQUEST_BYTES: usize = 64 * 1024;
const PROBE_BYTES: usize = 256 * 1024;
const PROBE_TIMEOUT: Duration = Duration::from_secs(15);
const START_TIMEOUT: Duration = Duration::from_secs(5);

#[derive(Serialize, Deserialize, Zeroize, ZeroizeOnDrop)]
#[serde(rename_all = "snake_case")]
enum Operation {
    Probe,
    Publish,
    Record,
}

#[derive(Serialize, Deserialize, Zeroize, ZeroizeOnDrop)]
#[serde(deny_unknown_fields)]
struct Request {
    operation: Operation,
    source: String,
    rtsp: bool,
    destination: Option<String>,
}

impl Request {
    fn new(source: &MediaSource, operation: Operation, destination: Option<&str>) -> Self {
        let (source, rtsp) = match source {
            MediaSource::Rtsp(url) => (url.as_str(), true),
            MediaSource::Local(relay) => (relay.url.as_str(), false),
        };
        Self {
            operation,
            source: source.into(),
            rtsp,
            destination: destination.map(str::to_owned),
        }
    }

    fn validate(&self) -> anyhow::Result<()> {
        ensure!(valid_endpoint(&self.source), "invalid media source");
        let source =
            url::Url::parse(&self.source).map_err(|_| anyhow::anyhow!("invalid media source"))?;
        if self.rtsp {
            ensure!(
                matches!(source.scheme(), "rtsp" | "rtsps") && source.host_str().is_some(),
                "invalid RTSP source"
            );
        } else {
            ensure!(
                source.scheme() == "http"
                    && source.host_str() == Some("127.0.0.1")
                    && source.username().is_empty()
                    && source.password().is_none(),
                "invalid local capture source"
            );
        }
        match self.operation {
            Operation::Probe => ensure!(self.destination.is_none(), "invalid probe request"),
            Operation::Publish => {
                let destination = self
                    .destination
                    .as_deref()
                    .context("missing media destination")?;
                crate::rtsp_publish::validate_publish_url(destination)
                    .map_err(|_| anyhow::anyhow!("invalid publish destination"))?;
            }
            Operation::Record => {
                let destination = self
                    .destination
                    .as_deref()
                    .context("missing recording destination")?;
                ensure!(
                    valid_endpoint(destination) && Path::new(destination).is_absolute(),
                    "invalid recording destination"
                );
            }
        }
        Ok(())
    }
}

fn valid_endpoint(value: &str) -> bool {
    !value.is_empty() && value.len() <= 16 * 1024 && !value.chars().any(char::is_control)
}

fn worker_command(executable: &Path) -> Command {
    let mut command = Command::new(executable);
    command
        .arg("media-worker")
        .stdin(Stdio::piped())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .kill_on_drop(true);
    // Do not inherit optional FFmpeg report destinations or diagnostic flags.
    command.env_remove("FFREPORT");
    command
}

async fn send_request(child: &mut Child, request: &Request) -> anyhow::Result<()> {
    request.validate()?;
    let bytes = Zeroizing::new(serde_json::to_vec(request).context("encode media request")?);
    ensure!(bytes.len() <= REQUEST_BYTES, "media request is too large");
    let mut stdin = child.stdin.take().context("missing media input pipe")?;
    let result = tokio::time::timeout(START_TIMEOUT, async {
        stdin.write_all(&bytes).await?;
        stdin.shutdown().await
    })
    .await;
    drop(stdin);
    match result {
        Ok(Ok(())) => Ok(()),
        failure => {
            stop_and_reap(child).await?;
            match failure {
                Err(_) => Err(ProcessCaptureError::Timeout.into()),
                Ok(Err(error)) => Err(error).context("media request pipe failed"),
                Ok(Ok(())) => unreachable!(),
            }
        }
    }
}

pub async fn spawn_publisher(
    executable: &Path,
    source: &MediaSource,
    destination: &str,
) -> anyhow::Result<Child> {
    spawn(
        executable,
        Request::new(source, Operation::Publish, Some(destination)),
    )
    .await
}

pub async fn spawn_recorder(
    executable: &Path,
    source: &MediaSource,
    destination: &Path,
) -> anyhow::Result<Child> {
    let destination = destination
        .to_str()
        .context("recording path is not UTF-8")?;
    spawn(
        executable,
        Request::new(source, Operation::Record, Some(destination)),
    )
    .await
}

async fn spawn(executable: &Path, request: Request) -> anyhow::Result<Child> {
    request.validate()?;
    let mut command = worker_command(executable);
    #[cfg(unix)]
    if matches!(request.operation, Operation::Record) {
        // SAFETY: only the forked child is affected; umask is async-signal-safe.
        unsafe {
            command.pre_exec(|| {
                libc::umask(0o077);
                Ok(())
            });
        }
    }
    let mut child = command.spawn().context("start media worker")?;
    send_request(&mut child, &request).await?;
    Ok(child)
}

pub async fn probe(executable: &Path, source: &MediaSource) -> anyhow::Result<Vec<u8>> {
    probe_with_timeout(executable, source, PROBE_TIMEOUT).await
}

async fn probe_with_timeout(
    executable: &Path,
    source: &MediaSource,
    timeout: Duration,
) -> anyhow::Result<Vec<u8>> {
    let request = Request::new(source, Operation::Probe, None);
    request.validate()?;
    let mut child = worker_command(executable)
        .stdout(Stdio::piped())
        .spawn()
        .context("start media probe")?;
    let stdout = child.stdout.take().context("missing media probe pipe")?;
    let result = tokio::time::timeout(timeout, async {
        send_request(&mut child, &request).await?;
        let mut bytes = Vec::new();
        let (_, status) = tokio::try_join!(
            async {
                stdout
                    .take(PROBE_BYTES as u64 + 1)
                    .read_to_end(&mut bytes)
                    .await
                    .context("read media probe")?;
                if bytes.len() > PROBE_BYTES {
                    return Err(ProcessCaptureError::OutputLimit(OutputStream::Stdout).into());
                }
                Ok::<_, anyhow::Error>(())
            },
            async {
                wait_for_exit(&mut child)
                    .await
                    .context("wait for media probe")
            }
        )?;
        ensure!(status.success(), "camera stream probe failed");
        Ok::<_, anyhow::Error>(bytes)
    })
    .await;
    match result {
        Ok(Ok(bytes)) => Ok(bytes),
        failure => {
            stop_and_reap(&mut child).await?;
            match failure {
                Ok(Err(error)) => Err(error),
                Err(_) => Err(ProcessCaptureError::Timeout.into()),
                Ok(Ok(_)) => unreachable!(),
            }
        }
    }
}

async fn wait_for_exit(child: &mut Child) -> std::io::Result<std::process::ExitStatus> {
    // Repoll in case an administrative launcher has blocked SIGCHLD.
    loop {
        if let Ok(status) = tokio::time::timeout(Duration::from_millis(25), child.wait()).await {
            return status;
        }
    }
}

async fn stop_and_reap(child: &mut Child) -> anyhow::Result<()> {
    if child.try_wait().context("inspect media worker")?.is_none() {
        child.start_kill().context("stop media worker")?;
    }
    wait_for_exit(child).await.context("reap media worker")?;
    Ok(())
}

/// Dispatch before normal CLI logging/runtime initialization. No raw native
/// error, parser value, endpoint, or token can reach a public diagnostic.
pub fn entry() -> ExitCode {
    let args = std::env::args_os().skip(2).collect::<Vec<_>>();
    let result = if args.len() == 1 && args[0] == "--check" {
        native::check()
    } else if args.is_empty() {
        run_stdio()
    } else {
        Err(anyhow::anyhow!("invalid worker arguments"))
    };
    if result.is_ok() {
        ExitCode::SUCCESS
    } else {
        eprintln!("media worker failed");
        ExitCode::from(8)
    }
}

fn run_stdio() -> anyhow::Result<()> {
    let mut input = Zeroizing::new(Vec::new());
    std::io::stdin()
        .lock()
        .take(REQUEST_BYTES as u64 + 1)
        .read_to_end(&mut input)
        .context("read media request")?;
    ensure!(input.len() <= REQUEST_BYTES, "media request is too large");
    let request: Request =
        serde_json::from_slice(&input).map_err(|_| anyhow::anyhow!("invalid media request"))?;
    request.validate()?;
    native::check()?;
    match request.operation {
        Operation::Probe => {
            let output = native::probe(&request)?;
            std::io::stdout()
                .lock()
                .write_all(&output)
                .context("write media metadata")?;
        }
        Operation::Publish | Operation::Record => native::run(&request)?,
    }
    Ok(())
}

#[cfg(not(any(target_os = "android", target_os = "ios")))]
mod native {
    use super::*;
    use std::ffi::{CString, c_char, c_int};
    unsafe extern "C" {
        fn xcoc_media_check() -> c_int;
        fn xcoc_media_probe(
            input: *const c_char,
            rtsp: c_int,
            output: *mut c_char,
            capacity: usize,
        ) -> c_int;
        fn xcoc_media_run(
            input: *const c_char,
            rtsp: c_int,
            destination: *const c_char,
            record: c_int,
        ) -> c_int;
    }

    pub(super) fn check() -> anyhow::Result<()> {
        // SAFETY: no pointers; the shim checks the linked media library capabilities.
        ensure!(
            unsafe { xcoc_media_check() } == 0,
            "media library capability check failed"
        );
        Ok(())
    }

    pub(super) fn probe(request: &Request) -> anyhow::Result<Vec<u8>> {
        let input = Zeroizing::new(CString::new(request.source.as_str())?.into_bytes_with_nul());
        let mut output = vec![0_u8; PROBE_BYTES];
        // SAFETY: C strings remain alive; output is writable for exactly capacity
        // bytes. The shim writes a bounded NUL-terminated JSON string.
        let result = unsafe {
            xcoc_media_probe(
                input.as_ptr().cast(),
                i32::from(request.rtsp),
                output.as_mut_ptr().cast(),
                output.len(),
            )
        };
        ensure!(result == 0, "media probe failed");
        let end = output
            .iter()
            .position(|b| *b == 0)
            .context("invalid media metadata")?;
        output.truncate(end);
        Ok(output)
    }

    pub(super) fn run(request: &Request) -> anyhow::Result<()> {
        let input = Zeroizing::new(CString::new(request.source.as_str())?.into_bytes_with_nul());
        let destination = Zeroizing::new(
            CString::new(
                request
                    .destination
                    .as_deref()
                    .context("missing destination")?,
            )?
            .into_bytes_with_nul(),
        );
        // SAFETY: both NUL-terminated strings outlive this blocking call. The
        // shim retains neither pointer and releases all native media resources.
        let result = unsafe {
            xcoc_media_run(
                input.as_ptr().cast(),
                i32::from(request.rtsp),
                destination.as_ptr().cast(),
                i32::from(matches!(request.operation, Operation::Record)),
            )
        };
        ensure!(result == 0, "media transport failed");
        Ok(())
    }
}

#[cfg(any(target_os = "android", target_os = "ios"))]
mod native {
    use super::*;
    pub(super) fn check() -> anyhow::Result<()> {
        anyhow::bail!("desktop media worker is unavailable on mobile")
    }
    pub(super) fn probe(_: &Request) -> anyhow::Result<Vec<u8>> {
        anyhow::bail!("desktop media worker is unavailable on mobile")
    }
    pub(super) fn run(_: &Request) -> anyhow::Result<()> {
        check()
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn command_contains_only_fixed_worker_mode() {
        let command = worker_command(Path::new("xcoc"));
        assert_eq!(
            command.as_std().get_args().collect::<Vec<_>>(),
            ["media-worker"]
        );
        assert!(
            command
                .as_std()
                .get_envs()
                .all(|(name, value)| name == "FFREPORT" && value.is_none())
        );
    }

    #[test]
    fn private_request_rejects_unknown_fields_and_invalid_destinations() {
        let source = MediaSource::Rtsp(
            "rtsp://synthetic-user:synthetic-password@camera.invalid/main".into(),
        );
        let request = Request::new(&source, Operation::Probe, None);
        request.validate().unwrap();
        let mut value = serde_json::to_value(&request).unwrap();
        value["extra"] = serde_json::json!("ignored");
        assert!(serde_json::from_value::<Request>(value).is_err());
        assert!(
            Request::new(
                &source,
                Operation::Publish,
                Some("rtsp://server.invalid/live?jwt=fake")
            )
            .validate()
            .is_err()
        );
        assert!(
            Request::new(&source, Operation::Record, Some("relative.mp4"))
                .validate()
                .is_err()
        );
    }

    #[cfg(unix)]
    fn fixture_worker(body: &str) -> (tempfile::TempDir, std::path::PathBuf) {
        use std::os::unix::fs::PermissionsExt;
        let directory = tempfile::tempdir().unwrap();
        let path = directory.path().join("worker");
        std::fs::write(&path, format!("#!/bin/sh\n{body}\n")).unwrap();
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o700)).unwrap();
        (directory, path)
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn all_media_operations_deliver_secrets_only_through_stdin() {
        let source = MediaSource::Rtsp(
            "rtsp://synthetic-user:synthetic-password@camera.invalid/main".into(),
        );
        let (_directory, executable) = fixture_worker(
            "cat > \"$0.request\"\nprintf '%s' \"$*\" > \"$0.args\"\nprintf '{\"streams\":[]}'",
        );
        let bytes = probe(&executable, &source).await.unwrap();
        assert_eq!(bytes, b"{\"streams\":[]}");
        let request_path = executable.with_extension("request");
        let args_path = executable.with_extension("args");
        let request: Request =
            serde_json::from_slice(&std::fs::read(&request_path).unwrap()).unwrap();
        assert!(matches!(request.operation, Operation::Probe));
        assert!(request.source.contains("synthetic-password"));
        assert_eq!(std::fs::read(&args_path).unwrap(), b"media-worker");

        let destination = "rtsps://media.invalid/live?jwt=synthetic-publish-token";
        let mut child = spawn_publisher(&executable, &source, destination)
            .await
            .unwrap();
        assert!(child.wait().await.unwrap().success());
        let request: Request =
            serde_json::from_slice(&std::fs::read(&request_path).unwrap()).unwrap();
        assert!(matches!(request.operation, Operation::Publish));
        assert_eq!(request.destination.as_deref(), Some(destination));
        assert_eq!(std::fs::read(&args_path).unwrap(), b"media-worker");

        let recording = executable.with_extension("mp4");
        let mut child = spawn_recorder(&executable, &source, &recording)
            .await
            .unwrap();
        assert!(child.wait().await.unwrap().success());
        let request: Request =
            serde_json::from_slice(&std::fs::read(&request_path).unwrap()).unwrap();
        assert!(matches!(request.operation, Operation::Record));
        assert!(request.source.contains("synthetic-password"));
        assert_eq!(std::fs::read(&args_path).unwrap(), b"media-worker");
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn overflowing_probe_output_is_stopped_and_reaped() {
        let (_directory, executable) = fixture_worker(
            "cat >/dev/null\nprintf '%s' \"$$\" > \"$0.pid\"\nhead -c 270000 /dev/zero\nexec sleep 30",
        );
        let source = MediaSource::Rtsp("rtsp://synthetic:secret@camera.invalid/main".into());
        let started = std::time::Instant::now();
        let error = probe(&executable, &source).await.unwrap_err();
        assert!(error.to_string().contains("output limit"));
        assert!(started.elapsed() < Duration::from_secs(5));
        let pid: i32 = std::fs::read_to_string(executable.with_extension("pid"))
            .unwrap()
            .parse()
            .unwrap();
        // SAFETY: signal 0 only checks whether our synthetic child still exists.
        assert_eq!(unsafe { libc::kill(pid, 0) }, -1);
        assert_eq!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::ESRCH)
        );
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn stalled_probe_is_stopped_and_reaped_at_deadline() {
        let (_directory, executable) =
            fixture_worker("cat >/dev/null\nprintf '%s' \"$$\" > \"$0.pid\"\nexec sleep 30");
        let source = MediaSource::Rtsp("rtsp://synthetic:secret@camera.invalid/main".into());
        let started = std::time::Instant::now();
        let error = probe_with_timeout(&executable, &source, Duration::from_millis(200))
            .await
            .unwrap_err();
        assert!(error.to_string().contains("timed out"));
        assert!(started.elapsed() < Duration::from_secs(3));
        let pid: i32 = std::fs::read_to_string(executable.with_extension("pid"))
            .unwrap()
            .parse()
            .unwrap();
        // SAFETY: signal 0 only checks whether our synthetic child still exists.
        assert_eq!(unsafe { libc::kill(pid, 0) }, -1);
        assert_eq!(
            std::io::Error::last_os_error().raw_os_error(),
            Some(libc::ESRCH)
        );
    }
}
