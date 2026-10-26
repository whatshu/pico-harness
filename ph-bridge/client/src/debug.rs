//! A local GDB endpoint tunneled through the authenticated phd socket.
//! OpenOCD stays bound to loopback on the daemon host; the daemon socket is
//! the only port that needs to be reachable for remote debugging.
use crate::{Config, DebugConfig, Error, Result, RpcClient};
use serde_json::{json, Value};
use std::{
    io::{self, BufReader},
    net::{Shutdown, TcpListener, TcpStream},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
    thread,
    time::Duration,
};

pub struct DebugServer {
    rpc: RpcClient,
    pub endpoint: String,
    stop: Arc<AtomicBool>,
    sockets: Arc<Mutex<Vec<TcpStream>>>,
    proxy: Option<thread::JoinHandle<()>>,
}
impl DebugServer {
    pub fn start(config: DebugConfig) -> Result<Self> {
        let connection = Config::from_env()?;
        let mut rpc = RpcClient::connect(&connection)?;
        rpc.call::<Value>("device.open", json!({"serial":config.serial}))?;
        let opened: Value = rpc.call("debug.start", serde_json::to_value(config)?)?;
        let port = opened["port"]
            .as_u64()
            .and_then(|n| u16::try_from(n).ok())
            .ok_or_else(|| Error::Configuration("phd omitted debug port".into()))?;
        let listener = TcpListener::bind(("127.0.0.1", 0))?;
        listener.set_nonblocking(true)?;
        let endpoint = listener.local_addr()?.to_string();
        let stop = Arc::new(AtomicBool::new(false));
        let sockets = Arc::new(Mutex::new(Vec::new()));
        let shutdown = stop.clone();
        let connections = sockets.clone();
        let proxy = thread::spawn(move || {
            while !shutdown.load(Ordering::Relaxed) {
                match listener.accept() {
                    Ok((local, _)) => {
                        let result = (|| -> Result<()> {
                            let mut remote = RpcClient::connect(&connection)?;
                            remote.call::<Value>("debug.tunnel", json!({"port":port}))?;
                            let remote = remote.into_buffered_stream();
                            remote.get_ref().set_read_timeout(None)?;
                            remote.get_ref().set_write_timeout(None)?;
                            if let Ok(mut sockets) = connections.lock() {
                                sockets.push(local.try_clone()?);
                                sockets.push(remote.get_ref().try_clone()?);
                            }
                            relay(local, remote)?;
                            Ok(())
                        })();
                        if let Err(error) = result {
                            eprintln!("phd GDB tunnel: {error}");
                        }
                        break;
                    }
                    Err(error) if error.kind() == io::ErrorKind::WouldBlock => {
                        thread::sleep(Duration::from_millis(10))
                    }
                    Err(error) => {
                        eprintln!("phd GDB listener: {error}");
                        break;
                    }
                }
            }
        });
        Ok(Self {
            rpc,
            endpoint,
            stop,
            sockets,
            proxy: Some(proxy),
        })
    }
    pub fn poll(&mut self) -> Result<(bool, Vec<String>)> {
        let status: Value = self.rpc.call("debug.status", json!({}))?;
        let logs = serde_json::from_value(status["logs"].clone())?;
        Ok((status["exited"].as_bool().unwrap_or(true), logs))
    }
}
impl Drop for DebugServer {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Relaxed);
        if let Ok(sockets) = self.sockets.lock() {
            for socket in sockets.iter() {
                let _ = socket.shutdown(Shutdown::Both);
            }
        }
        let _ = self.rpc.call::<Value>("debug.stop", json!({}));
        if let Some(proxy) = self.proxy.take() {
            let _ = proxy.join();
        }
    }
}
fn relay(mut local: TcpStream, mut remote: BufReader<TcpStream>) -> io::Result<()> {
    let mut from_local = local.try_clone()?;
    let mut to_remote = remote.get_ref().try_clone()?;
    let upload = thread::spawn(move || {
        let _ = io::copy(&mut from_local, &mut to_remote);
        let _ = from_local.shutdown(Shutdown::Both);
        let _ = to_remote.shutdown(Shutdown::Both);
    });
    let result = io::copy(&mut remote, &mut local);
    let _ = local.shutdown(Shutdown::Both);
    let _ = remote.get_ref().shutdown(Shutdown::Both);
    let _ = upload.join();
    result.map(|_| ())
}
