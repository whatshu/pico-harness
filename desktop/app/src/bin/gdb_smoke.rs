//! Explicit hardware checks. Downloads SRAM images only; Flash fixtures must be preloaded.
use ph_app::gdb::{mi::quote, Config, Event, Request, Session};
use std::{
    collections::HashSet,
    process::{Command, Stdio},
    time::{Duration, Instant},
};

fn server_smoke(serial: String) -> Result<(), String> {
    let session = Session::start(Config {
        serial,
        server_only: true,
        ..Default::default()
    });
    let Event::ServerReady(endpoint) =
        until(&session, |event| matches!(event, Event::ServerReady(_)))?
    else {
        unreachable!()
    };
    let mut client = Command::new("gdb-multiarch")
        .args(["--batch", "-nx", "-ex", "set pagination off", "-ex"])
        .arg(format!("target extended-remote {endpoint}"))
        .args([
            "-ex",
            "monitor reset halt",
            "-ex",
            "x/4wx 0x20000000",
            "-ex",
            "monitor reset run",
            "-ex",
            "detach",
        ])
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .map_err(|error| error.to_string())?;
    let deadline = Instant::now() + Duration::from_secs(15);
    while client
        .try_wait()
        .map_err(|error| error.to_string())?
        .is_none()
    {
        if Instant::now() > deadline {
            let _ = client.kill();
            let _ = client.wait();
            return Err("External GDB client timed out".into());
        }
        std::thread::sleep(Duration::from_millis(20));
    }
    let output = client
        .wait_with_output()
        .map_err(|error| error.to_string())?;
    print!("{}", String::from_utf8_lossy(&output.stdout));
    if !output.status.success() || !String::from_utf8_lossy(&output.stdout).contains("0x20000000") {
        return Err(String::from_utf8_lossy(&output.stderr).into());
    }
    session.send(Request::Disconnect);
    until_closed(&session)?;
    assert!(
        std::net::TcpStream::connect(&endpoint).is_err(),
        "Managed server must close its port"
    );
    println!(
        "PASS: server-only without ELF, external GDB attach/read/detach, managed server shutdown"
    );
    Ok(())
}

fn until_closed(session: &Session) -> Result<(), String> {
    let deadline = Instant::now() + Duration::from_secs(12);
    loop {
        match session
            .rx
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .map_err(|error| error.to_string())?
        {
            Event::Closed => return Ok(()),
            Event::Error(error) => return Err(error),
            _ => {}
        }
    }
}
fn until(session: &Session, condition: impl Fn(&Event) -> bool) -> Result<Event, String> {
    let deadline = Instant::now() + Duration::from_secs(20);
    loop {
        let e = session
            .rx
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .map_err(|e| e.to_string())?;
        match &e {
            Event::Error(e) => return Err(e.clone()),
            Event::Closed => return Err("GDB closed unexpectedly".into()),
            Event::ServerReady(endpoint) => println!("server {endpoint}"),
            Event::State { state, generation } => println!("state {state} generation {generation}"),
            Event::Data { key, .. } => println!("data {key}"),
            Event::Log(line) => print!("{line}"),
            Event::OpenOcd(line) => println!("openocd {line}"),
        }
        if condition(&e) {
            return Ok(e);
        }
    }
}
fn execute(session: &Session, cmd: &str, key: &str) -> Result<(), String> {
    session.send(Request::Execute(cmd.into()));
    until(session, |e| matches!(e,Event::Data{key:k,..}if k==key))?;
    Ok(())
}

fn flash_fixture_smoke(elf: &str, serial: &str) -> Result<(), String> {
    let elf = std::fs::canonicalize(elf).map_err(|error| error.to_string())?;
    let session = Session::start(Config {
        elf: elf.display().to_string(),
        serial: serial.into(),
        ..Default::default()
    });
    until(
        &session,
        |event| matches!(event, Event::Data { key, .. } if key == "registers"),
    )?;
    execute(
        &session,
        "-interpreter-exec console \"monitor reset halt\"",
        "breakpoints",
    )?;
    // harness-signals calls this after UART init, which depends on a live timer.
    // Resetting two isolated cores and resuming just core0 freezes that timer.
    execute(&session, "-break-insert pico_harness_pwm", "breakpoints")?;
    session.send(Request::Execute("-exec-continue".into()));
    until(&session, |event| {
        matches!(event, Event::Data { key, value, .. }
            if key == "stop" && value.field("reason") == "breakpoint-hit")
    })?;
    let stack = until(
        &session,
        |event| matches!(event, Event::Data { key, .. } if key == "stack"),
    )?;
    if let Event::Data { value, .. } = stack {
        let frames = value.get("stack").ok_or("Missing stack")?;
        let frame = frames.items().first().ok_or("Empty stack")?;
        let frame = frame.get("frame").unwrap_or(frame);
        if frame.field("func") != "pico_harness_pwm" {
            return Err(format!("Stopped in unexpected frame: {frame:?}"));
        }
    }
    session.send(Request::Execute("-exec-next".into()));
    until(&session, |event| {
        matches!(event, Event::Data { key, value, .. }
            if key == "stop" && value.field("reason") == "end-stepping-range")
    })?;
    session.send(Request::Disconnect);
    until_closed(&session)?;
    println!("PASS: Flash fixture attach, reset, UART timer init, hardware breakpoint, source step, detach; no download");
    Ok(())
}

fn main() -> Result<(), String> {
    let args: Vec<_> = std::env::args().collect();
    if args.len() < 3 {
        return Err("usage: gdb_smoke <SRAM-ELF> <probe-serial> [existing-host:port], gdb_smoke --server-only <probe-serial>, or gdb_smoke --flash-fixture <harness-signals-ELF> <probe-serial>".into());
    }
    if args[1] == "--server-only" {
        return server_smoke(args[2].clone());
    }
    if args[1] == "--flash-fixture" {
        let serial = args.get(3).ok_or("Missing probe serial")?;
        return flash_fixture_smoke(&args[2], serial);
    }
    let elf = std::fs::canonicalize(&args[1]).map_err(|e| e.to_string())?;
    let bytes = std::fs::read(&elf).map_err(|e| e.to_string())?;
    if bytes.len() < 52 || &bytes[..4] != b"\x7fELF" || bytes[4] != 1 || bytes[5] != 1 {
        return Err("Expected little-endian ELF32".into());
    }
    let u16at = |i| u16::from_le_bytes(bytes[i..i + 2].try_into().unwrap());
    let u32at = |i| u32::from_le_bytes(bytes[i..i + 4].try_into().unwrap());
    let ph = u32at(28) as usize;
    let size = u16at(42) as usize;
    let count = u16at(44) as usize;
    for n in 0..count {
        let p = ph + n * size;
        if p + 32 > bytes.len() {
            return Err("Malformed ELF".into());
        }
        if u32at(p) == 1 && u32at(p + 16) > 0 {
            let address = u32at(p + 12);
            let end = address
                .checked_add(u32at(p + 20))
                .ok_or("ELF range overflow")?;
            if address < 0x20000000 || end > 0x20082000 {
                return Err(format!(
                    "Refusing non-SRAM segment: {address:08x}..{end:08x}"
                ));
            }
        }
    }
    let config = Config {
        elf: elf.display().to_string(),
        serial: args[2].clone(),
        managed: args.len() < 4,
        endpoint: args.get(3).cloned().unwrap_or_default(),
        ..Default::default()
    };
    let session = Session::start(config);
    until(
        &session,
        |e| matches!(e,Event::Data{key,..}if key=="registers"),
    )?;
    execute(
        &session,
        "-interpreter-exec console \"monitor reset halt\"",
        "breakpoints",
    )?;
    execute(&session, "-target-download", "registers")?;
    execute(&session, "-break-insert main", "breakpoints")?;
    // ELF entry startup initialises the RAM-only image.
    execute(
        &session,
        &format!(
            "-data-evaluate-expression {}",
            quote(&format!("$pc = 0x{:x}", u32at(24)))
        ),
        "registers",
    )?;
    session.send(Request::Execute("-exec-continue".into()));
    until(
        &session,
        |e| matches!(e,Event::Data{key,value,..}if key=="stop"&&value.field("reason")=="breakpoint-hit"),
    )?;
    until(
        &session,
        |e| matches!(e,Event::Data{key,..}if key=="registers"),
    )?;
    session.send(Request::Execute("-exec-next".into()));
    let event = until(
        &session,
        |e| matches!(e,Event::State{state,..}if state=="stopped"),
    )?;
    let generation = if let Event::State { generation, .. } = event {
        generation
    } else {
        0
    };
    let mut seen = HashSet::new();
    while seen.len() < 3 {
        if let Event::Data { key, value, .. } = until(
            &session,
            |e| matches!(e,Event::Data{key,..}if ["stack","locals","registers"].contains(&key.as_str())),
        )? {
            if key == "stack" {
                println!("STACK: {value:?}");
            }
            seen.insert(key);
        }
    }
    session.send(Request::Query {
        key: "memory-check".into(),
        command: "-data-read-memory-bytes 0x20000000 32".into(),
        generation,
    });
    until(
        &session,
        |e| matches!(e,Event::Data{key,value,..}if key=="memory-check"&&!value.get("memory").unwrap().items().is_empty()),
    )?;
    // work-selftest's main loop keeps calling this function. Let GDB select a
    // software breakpoint in SRAM and a hardware breakpoint in read-only Flash.
    session.send(Request::Execute("-exec-continue".into()));
    until(
        &session,
        |event| matches!(event, Event::State { state, .. } if state == "running"),
    )?;
    session.send(Request::Breakpoint("-break-insert work_tick".into()));
    until(
        &session,
        |event| matches!(event, Event::Data { key, value, .. } if key == "stop" && value.field("reason") == "breakpoint-hit"),
    )?;
    session.send(Request::Disconnect);
    until_closed(&session)?;
    println!("PASS: attach, RAM download, breakpoint, source step, stack, locals, registers, memory, running breakpoint insertion, detach");
    Ok(())
}
