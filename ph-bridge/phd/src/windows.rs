//! Native Windows Service Control Manager integration.
use std::{
    ffi::OsString,
    net::TcpListener,
    sync::{atomic::Ordering, Arc},
    time::Duration,
};
use windows_service::{
    define_windows_service,
    service::{
        ServiceControl, ServiceControlAccept, ServiceExitCode, ServiceState, ServiceStatus,
        ServiceType,
    },
    service_control_handler::{self, ServiceControlHandlerResult},
    service_dispatcher,
};
const NAME: &str = "PicoHarness";
define_windows_service!(ffi_main, service_main);
pub fn dispatch() -> windows_service::Result<()> {
    service_dispatcher::start(NAME, ffi_main)
}
fn service_main(_arguments: Vec<OsString>) {
    use clap::Parser;
    if let Err(error) = super::run(super::Args::parse()) {
        eprintln!("phd Windows service: {error}");
    }
}
pub fn serve(
    listener: TcpListener,
    runtime: Arc<phd::runtime::Runtime>,
) -> Result<(), Box<dyn std::error::Error>> {
    let stopped = runtime.clone();
    let handler = service_control_handler::register(NAME, move |control| match control {
        ServiceControl::Stop | ServiceControl::Shutdown => {
            stopped.stop.store(true, Ordering::Relaxed);
            ServiceControlHandlerResult::NoError
        }
        ServiceControl::Interrogate => ServiceControlHandlerResult::NoError,
        _ => ServiceControlHandlerResult::NotImplemented,
    })?;
    let status = |state, controls, code| ServiceStatus {
        service_type: ServiceType::OWN_PROCESS,
        current_state: state,
        controls_accepted: controls,
        exit_code: ServiceExitCode::Win32(code),
        checkpoint: 0,
        wait_hint: Duration::default(),
        process_id: None,
    };
    handler.set_service_status(status(
        ServiceState::Running,
        ServiceControlAccept::STOP | ServiceControlAccept::SHUTDOWN,
        0,
    ))?;
    let result = phd::server::serve(listener, runtime);
    handler.set_service_status(status(
        ServiceState::Stopped,
        ServiceControlAccept::empty(),
        if result.is_ok() { 0 } else { 1 },
    ))?;
    Ok(result?)
}
