//! Human and machine control of phd; no USB dependencies are linked here.
mod service;
use base64::{engine::general_purpose::STANDARD, Engine};
use clap::{Parser, Subcommand};
use ph_client::{Config, Error, RpcClient};
use serde_json::{json, Value};
use std::{
    path::PathBuf,
    process::{Command, Stdio},
    thread,
    time::{Duration, Instant},
};

type Result<T> = std::result::Result<T, Box<dyn std::error::Error>>;
#[derive(Parser)]
#[command(version, about = "Control the Pico Harness daemon locally or over TCP")]
struct Args {
    #[arg(long,global=true,env="PHD_ADDR",default_value=ph_protocol::DEFAULT_ADDR)]
    address: String,
    #[arg(long, global = true, env = "PHD_TOKEN_FILE")]
    token_file: Option<PathBuf>,
    #[arg(long, global = true)]
    serial: Option<String>,
    #[arg(long, global = true)]
    json: bool,
    #[command(subcommand)]
    command: Action,
}
#[derive(Subcommand)]
enum Action {
    Daemon {
        #[command(subcommand)]
        command: Daemon,
    },
    Service {
        #[command(subcommand)]
        command: Service,
    },
    Doctor,
    Device {
        #[command(subcommand)]
        command: Device,
    },
    Info,
    Status,
    Measure,
    Power {
        #[arg(value_parser=["on","off"])]
        state: String,
    },
    Reset {
        #[arg(default_value_t=20,value_parser=clap::value_parser!(u16).range(1..=1000))]
        milliseconds: u16,
    },
    Bootsel,
    /// Keep a daemon-managed OpenOCD session and local authenticated GDB tunnel alive.
    Debug {
        #[arg(long,value_parser=["rp2040","rp2350"])]
        target: String,
        #[arg(long, env = "PDBG_OPENOCD", default_value = "openocd")]
        openocd: String,
        #[arg(long, env = "PDBG_OPENOCD_SCRIPTS", default_value = "")]
        scripts: String,
    },
    Console {
        #[command(subcommand)]
        command: Console,
    },
    Term {
        baud: u32,
    },
    Bulk {
        file: PathBuf,
        #[arg(long,default_value="work_spi",value_parser=["work_spi","debug"])]
        route: String,
    },
    Flash {
        file: PathBuf,
        #[arg(long,value_parser=["rp2040","rp2350"])]
        target: Option<String>,
        #[arg(long)]
        debug: bool,
        #[arg(long)]
        wait: bool,
    },
    Task {
        #[command(subcommand)]
        command: Task,
    },
    /// Advanced versioned RPC. Select a board with --serial when required.
    Call {
        method: String,
        #[arg(default_value = "{}")]
        params: String,
    },
}
#[derive(Subcommand)]
enum Daemon {
    Start {
        #[arg(long)]
        phd: Option<PathBuf>,
    },
    Stop,
    Status,
}
#[derive(Subcommand)]
enum Service {
    Install,
    Uninstall,
    Start,
    Stop,
    Status,
}
#[derive(Subcommand)]
enum Device {
    List,
    Capabilities,
}
#[derive(Subcommand)]
enum Console {
    Send {
        text: String,
    },
    Read {
        #[arg(long, default_value_t = 0)]
        cursor: u64,
        #[arg(long,default_value_t=500,value_parser=clap::value_parser!(u64).range(0..=1000))]
        wait_ms: u64,
    },
}
#[derive(Subcommand)]
enum Task {
    List,
    Status { id: String },
    Cancel { id: String },
}
fn config(args: &Args) -> Result<Config> {
    let mut config = Config::from_env().or_else(|error| {
        if let Some(path) = &args.token_file {
            Ok(Config {
                address: args.address.clone(),
                token: std::fs::read_to_string(path)?.trim().into(),
                timeout: Duration::from_secs(30),
            })
        } else {
            Err(error)
        }
    })?;
    config.address = args.address.clone();
    if let Some(path) = &args.token_file {
        config.token = std::fs::read_to_string(path)?.trim().into();
    }
    Ok(config)
}
fn executable(name: &str) -> Result<PathBuf> {
    let sibling =
        std::env::current_exe()?.with_file_name(format!("{name}{}", std::env::consts::EXE_SUFFIX));
    Ok(if sibling.is_file() {
        sibling
    } else {
        PathBuf::from(name)
    })
}
fn daemon_start(args: &Args, phd: Option<&PathBuf>) -> Result<Value> {
    if let Ok(config) = config(args) {
        if let Ok(mut rpc) = RpcClient::connect(&config) {
            return Ok(rpc.call("daemon.health", json!({}))?);
        }
    }
    let state = ph_client::rpc::state_dir()?;
    if !state.exists() {
        std::fs::create_dir_all(&state)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&state, std::fs::Permissions::from_mode(0o700))?;
        }
    }
    let log = std::fs::OpenOptions::new()
        .create(true)
        .append(true)
        .open(state.join("phd.log"))?;
    let executable = phd.cloned().unwrap_or(executable("phd")?);
    let mut command = Command::new(executable);
    command.args(["--listen", &args.address]);
    if let Some(path) = &args.token_file {
        command.arg("--token-file").arg(path);
    }
    command
        .stdin(Stdio::null())
        .stdout(log.try_clone()?)
        .stderr(log);
    #[cfg(unix)]
    {
        use std::os::unix::process::CommandExt;
        command.process_group(0);
    }
    #[cfg(windows)]
    {
        use std::os::windows::process::CommandExt;
        command.creation_flags(0x08000000);
    }
    let mut child = command.spawn()?;
    let deadline = Instant::now() + Duration::from_secs(10);
    loop {
        if let Ok(config) = config(args) {
            if let Ok(mut rpc) = RpcClient::connect(&config) {
                return Ok(rpc.call("daemon.health", json!({}))?);
            }
        }
        if let Some(status) = child.try_wait()? {
            return Err(format!(
                "phd exited ({status}); inspect {}",
                state.join("phd.log").display()
            )
            .into());
        }
        if Instant::now() >= deadline {
            let _ = child.kill();
            let _ = child.wait();
            return Err(format!(
                "phd did not become ready; inspect {}",
                state.join("phd.log").display()
            )
            .into());
        }
        thread::sleep(Duration::from_millis(50));
    }
}
fn run(args: &Args) -> Result<Value> {
    if let Action::Daemon {
        command: Daemon::Start { phd },
    } = &args.command
    {
        return daemon_start(args, phd.as_ref());
    }
    if let Action::Service { command } = &args.command {
        return service::run(args, command, &executable("phd")?);
    }
    let mut rpc = RpcClient::connect(&config(args)?)?;
    match &args.command {
        Action::Daemon {
            command: Daemon::Stop,
        } => return Ok(rpc.call("daemon.stop", json!({}))?),
        Action::Daemon { .. } | Action::Doctor => return Ok(rpc.call("daemon.health", json!({}))?),
        Action::Device {
            command: Device::List,
        } => return Ok(rpc.call("device.list", json!({}))?),
        Action::Device {
            command: Device::Capabilities,
        } => return Ok(rpc.call("device.capabilities", json!({}))?),
        Action::Task { command } => {
            return Ok(match command {
                Task::List => rpc.call("task.list", json!({}))?,
                Task::Status { id } => rpc.call("task.status", json!({"id":id}))?,
                Task::Cancel { id } => rpc.call("task.cancel", json!({"id":id}))?,
            })
        }
        Action::Call { method, params } => {
            if let Some(serial) = &args.serial {
                rpc.call::<Value>("device.open", json!({"serial":serial}))?;
            }
            return Ok(rpc.call(method, serde_json::from_str(params)?)?);
        }
        _ => {}
    }
    let selected: Value = rpc.call("device.open", json!({"serial":args.serial}))?;
    if let Action::Debug {
        target,
        openocd,
        scripts,
    } = &args.command
    {
        let mut server = ph_client::debug::DebugServer::start(ph_protocol::DebugConfig {
            serial: selected["serial"]
                .as_str()
                .ok_or("daemon omitted selected serial")?
                .into(),
            target: target.clone(),
            openocd: openocd.clone(),
            scripts: scripts.clone(),
        })?;
        println!("{}", json!({"endpoint":server.endpoint}));
        let stop = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let signal = stop.clone();
        ctrlc::set_handler(move || signal.store(true, std::sync::atomic::Ordering::Relaxed))?;
        while !stop.load(std::sync::atomic::Ordering::Relaxed) {
            let (exited, logs) = server.poll()?;
            for log in logs {
                eprintln!("{log}");
            }
            if exited {
                return Err("daemon OpenOCD session exited".into());
            }
            thread::sleep(Duration::from_millis(200));
        }
        return Ok(json!({"stopped":true}));
    }
    let (method, params) = match &args.command {
        Action::Info => ("board.info", json!({})),
        Action::Status => ("board.status", json!({})),
        Action::Measure => ("board.measure", json!({})),
        Action::Power { state } => ("board.power", json!({"enabled":state=="on"})),
        Action::Reset { milliseconds } => ("board.reset", json!({"milliseconds":milliseconds})),
        Action::Bootsel => ("board.bootsel", json!({})),
        Action::Term { baud } => ("console.configure", json!({"baud":baud})),
        Action::Console {
            command: Console::Send { text },
        } => (
            "console.write",
            json!({"data":STANDARD.encode(text.as_bytes())}),
        ),
        Action::Console {
            command: Console::Read { cursor, wait_ms },
        } => (
            "events.read",
            json!({"cursor":cursor,"wait_ms":wait_ms,"limit":128}),
        ),
        Action::Bulk { file, route } => {
            if std::fs::metadata(file)?.len() > ph_protocol::MAX_DATA as u64 {
                return Err("Bulk payload exceeds 1 MiB".into());
            }
            (
                "bulk.write",
                json!({"data":STANDARD.encode(std::fs::read(file)?),"route":route}),
            )
        }
        Action::Flash {
            file,
            target,
            debug,
            wait,
        } => {
            if *debug && target.is_some() {
                return Err("--debug and --target cannot be combined".into());
            }
            if !debug && target.is_none() {
                return Err("Work flashing requires --target rp2040|rp2350".into());
            }
            let upload = rpc.upload(file)?;
            let task:Value=rpc.call("task.start",json!({"upload":upload,"kind":if *debug { "debug_flash" } else { "work_flash" },"target":target}))?;
            if !wait {
                return Ok(task);
            }
            loop {
                let status: Value = rpc.call("task.status", json!({"id":task["id"]}))?;
                if status["state"] != "running" {
                    if status["state"] != "succeeded" {
                        return Err(status["error"]
                            .as_str()
                            .unwrap_or("flash task failed")
                            .to_owned()
                            .into());
                    }
                    return Ok(status);
                }
                thread::sleep(Duration::from_millis(100));
            }
        }
        _ => return Err("unsupported command".into()),
    };
    Ok(rpc.call(method, params)?)
}
fn main() {
    let args = Args::parse();
    match run(&args) {
        Ok(value) => {
            if args.json {
                println!("{value}");
            } else {
                println!(
                    "{}",
                    serde_json::to_string_pretty(&value).unwrap_or_else(|_| value.to_string())
                );
            }
        }
        Err(error) => {
            if args.json {
                let details = match error.downcast_ref::<Error>() {
                    Some(Error::Remote(remote)) => serde_json::to_value(remote).unwrap(),
                    Some(Error::Transport(_)) => {
                        json!({"kind":"transport","message":error.to_string(),"retryable":false,"outcome":"unknown_if_submitted"})
                    }
                    _ => {
                        json!({"kind":"client_error","message":error.to_string(),"retryable":false})
                    }
                };
                eprintln!("{}", json!({"error":details}));
            } else {
                eprintln!("phdcli: {error}");
            }
            let code = match error.downcast_ref::<Error>() {
                Some(Error::Remote(error)) if error.kind == "busy" => 3,
                Some(Error::Transport(_)) => 4,
                _ => 1,
            };
            std::process::exit(code);
        }
    }
}
