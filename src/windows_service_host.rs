use std::{ffi::OsString, path::PathBuf, process::Stdio, time::Duration};

use anyhow::{Context, ensure};
use windows_service::{
    define_windows_service,
    service::{
        ServiceAccess, ServiceControl, ServiceControlAccept, ServiceExitCode, ServiceState,
        ServiceStatus, ServiceType,
    },
    service_control_handler::{self, ServiceControlHandlerResult},
    service_dispatcher,
    service_manager::{ServiceManager, ServiceManagerAccess},
};

const NAME: &str = "XcocClient";

define_windows_service!(service_entry, service_main);

pub fn dispatch() -> anyhow::Result<()> {
    service_dispatcher::start(NAME, service_entry).context("start Xcoc Windows service dispatcher")
}

fn service_main(_: Vec<OsString>) {
    if serve().is_err() {
        if super::runtime_event(
            "xcoc.windows.service_failed",
            None,
            xcss_log::Level::Error,
            "Windows service failed.",
            "WINDOWS_SERVICE_FAILED",
        )
        .is_err()
        {
            std::process::exit(1);
        }
    }
}

fn serve() -> windows_service::Result<()> {
    let (stop_sender, stop_receiver) = tokio::sync::watch::channel(false);
    let handle = service_control_handler::register(NAME, move |event| match event {
        ServiceControl::Stop | ServiceControl::Shutdown => {
            let _ = stop_sender.send(true);
            ServiceControlHandlerResult::NoError
        }
        ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
        _ => ServiceControlHandlerResult::NotImplemented,
    })?;
    let status = |state, exit_code| ServiceStatus {
        service_type: ServiceType::OWN_PROCESS,
        current_state: state,
        controls_accepted: if state == ServiceState::Running {
            ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN
        } else {
            ServiceControlAccept::empty()
        },
        exit_code,
        checkpoint: 0,
        wait_hint: Duration::ZERO,
        process_id: None,
    };
    handle.set_service_status(status(
        ServiceState::StartPending,
        ServiceExitCode::Win32(0),
    ))?;
    let protected = initialize_logging();
    let result = protected
        .as_ref()
        .map_err(|_| anyhow::anyhow!("Windows protected diagnostics initialization failed"))
        .and_then(|_| {
            handle.set_service_status(status(ServiceState::Running, ServiceExitCode::Win32(0)))?;
            tokio::runtime::Runtime::new().map_err(anyhow::Error::from)
        })
        .and_then(|runtime| runtime.block_on(supervise_runtime(stop_receiver)));
    if let Err(error) = &result {
        if super::runtime_event(
            "xcoc.windows.runtime_failed",
            None,
            xcss_log::Level::Error,
            "Windows service runtime failed.",
            super::device_error_code(error),
        )
        .is_err()
        {
            handle.set_service_status(status(
                ServiceState::Stopped,
                ServiceExitCode::ServiceSpecific(1),
            ))?;
            return Ok(());
        }
    }
    let exit_code = if result.is_ok() {
        ServiceExitCode::Win32(0)
    } else {
        ServiceExitCode::ServiceSpecific(1)
    };
    handle.set_service_status(status(ServiceState::Stopped, exit_code))?;
    Ok(())
}

fn initialize_logging() -> anyhow::Result<xcsc_fs_safety::PrivateDirectory> {
    let path = super::default_config_path();
    super::secure_existing_default_config(&path)?;
    let directory = xcsc_fs_safety::PrivateDirectory::create(std::path::absolute(
        path.parent().context("configuration parent missing")?,
    )?)?;
    let sink = xcss_log::RotatingLogFile::create_private(
        directory.path().join("logs"),
        "xcoc",
        xcss_log::LogRetention::default(),
    )?;
    xcss_log::install_rotating_file(sink)?;
    xcss_log::LogRecord::server(
        "xcoc",
        "windows-service",
        "xcoc.windows.started",
        "Windows service runtime started.",
        xcss_log::Level::Info,
    )?
    .emit()?;
    Ok(directory)
}

async fn supervise_runtime(
    stop_receiver: tokio::sync::watch::Receiver<bool>,
) -> anyhow::Result<()> {
    let path = super::default_config_path();
    loop {
        if *stop_receiver.borrow() {
            return Ok(());
        }
        if let Err(error) = super::run(&path, true, Some(stop_receiver.clone())).await {
            super::runtime_event(
                "xcoc.windows.runtime_restart",
                None,
                xcss_log::Level::Error,
                "Windows service runtime requires recovery.",
                super::device_error_code(&error),
            )?;
        }
        let mut retry_stop = Some(stop_receiver.clone());
        tokio::select! {
            _ = super::wait_for_shutdown(&mut retry_stop) => return Ok(()),
            _ = tokio::time::sleep(Duration::from_secs(10)) => {}
        }
    }
}

pub fn configure_and_start(start_at_boot: bool) -> anyhow::Result<()> {
    let mode = if start_at_boot { "auto" } else { "demand" };
    let system_root = std::env::var_os("SystemRoot").context("locate Windows system directory")?;
    let sc = PathBuf::from(system_root).join("System32/sc.exe");
    let status = std::process::Command::new(sc)
        .args(["config", NAME, "start=", mode])
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .status()
        .context("configure Xcoc Windows service startup")?;
    ensure!(
        status.success(),
        "configure Xcoc Windows service startup failed; run setup as administrator"
    );

    let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)
        .context("connect to Windows service manager")?;
    let service = manager
        .open_service(NAME, ServiceAccess::QUERY_STATUS | ServiceAccess::START)
        .context("open Xcoc Windows service")?;
    if service.query_status()?.current_state == ServiceState::Stopped {
        service
            .start(&[] as &[&str])
            .context("start Xcoc Windows service")?;
    }
    for _ in 0..50 {
        match service.query_status()?.current_state {
            ServiceState::Running => return Ok(()),
            ServiceState::Stopped => {
                anyhow::bail!("Xcoc Windows service stopped during startup")
            }
            _ => std::thread::sleep(Duration::from_millis(200)),
        }
    }
    anyhow::bail!("Xcoc Windows service did not become ready within 10 seconds")
}

pub fn preflight_setup() -> anyhow::Result<()> {
    let manager = ServiceManager::local_computer(None::<&str>, ServiceManagerAccess::CONNECT)
        .context("connect to Windows service manager before pairing")?;
    manager
        .open_service(
            NAME,
            ServiceAccess::CHANGE_CONFIG | ServiceAccess::QUERY_STATUS | ServiceAccess::START,
        )
        .context("install the MSI and run setup as administrator before using a pairing code")?;
    Ok(())
}
