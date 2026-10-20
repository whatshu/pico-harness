use clap::Parser;
use std::{
    net::{SocketAddr, TcpListener},
    path::PathBuf,
    sync::atomic::Ordering,
};

#[derive(Parser)]
#[command(
    version,
    about = "Pico Harness daemon: authenticated socket access to USB probes"
)]
struct Args {
    #[arg(long,env="PHD_ADDR",default_value=ph_protocol::DEFAULT_ADDR)]
    listen: SocketAddr,
    #[arg(long, env = "PHD_TOKEN_FILE")]
    token_file: Option<PathBuf>,
    #[arg(long, help = "Create/check credentials without starting the daemon")]
    init_token: bool,
    #[cfg(windows)]
    #[arg(long, hide = true)]
    windows_service: bool,
}
fn run(args: Args) -> Result<(), Box<dyn std::error::Error>> {
    if args.init_token {
        phd::credentials::load_or_create(
            &args.token_file.unwrap_or(ph_client::rpc::token_path()?),
        )?;
        return Ok(());
    }
    // Bind before generating credentials: starting a second daemon must not
    // alter the state of the instance that already owns this socket.
    let listener = TcpListener::bind(args.listen)?;
    let token = match std::env::var("PHD_TOKEN") {
        Ok(token) if token.len() >= 32 => token,
        Ok(_) => return Err("PHD_TOKEN must contain at least 32 characters".into()),
        Err(_) => phd::credentials::load_or_create(
            &args.token_file.unwrap_or(ph_client::rpc::token_path()?),
        )?,
    };
    let runtime = phd::runtime::Runtime::new(token);
    #[cfg(windows)]
    if args.windows_service {
        return windows::serve(listener, runtime);
    }
    let shutdown = runtime.clone();
    ctrlc::set_handler(move || {
        shutdown.stop.store(true, Ordering::Relaxed);
    })?;
    eprintln!(
        "phd {} listening on {}",
        env!("CARGO_PKG_VERSION"),
        listener.local_addr()?
    );
    phd::server::serve(listener, runtime)?;
    Ok(())
}
#[cfg(windows)]
mod windows;
fn main() {
    let args = Args::parse();
    #[cfg(windows)]
    if args.windows_service {
        if let Err(error) = windows::dispatch() {
            eprintln!("phd service: {error}");
            std::process::exit(1);
        }
        return;
    }
    if let Err(error) = run(args) {
        eprintln!("phd: {error}");
        std::process::exit(1);
    }
}
