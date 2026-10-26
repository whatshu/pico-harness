//! Socket transport. A failed call is never replayed: a lost reply cannot tell
//! us whether a power change, reset, console write or flash operation ran.

use ph_protocol::{Request, RpcError, DEFAULT_ADDR, VERSION};
use serde::de::DeserializeOwned;
use serde_json::{json, Value};
use std::{
    io::{BufReader, Write},
    net::{TcpStream, ToSocketAddrs},
    path::PathBuf,
    time::Duration,
};

#[derive(Debug, thiserror::Error)]
pub enum Error {
    #[error("phd transport failed ({0}); a submitted operation may have executed")]
    Transport(#[from] std::io::Error),
    #[error("invalid phd reply: {0}")]
    Json(#[from] serde_json::Error),
    #[error(transparent)]
    Remote(#[from] RpcError),
    #[error("{0}")]
    Configuration(String),
    #[error("invalid argument: {0}")]
    InvalidArgument(String),
}
pub type Result<T> = std::result::Result<T, Error>;

#[derive(Clone)]
pub struct Config {
    pub address: String,
    pub token: String,
    pub timeout: Duration,
}

/// Platform-native per-user state path; PHD_STATE_DIR is useful for isolated
/// tests and multiple explicitly configured daemon instances.
pub fn state_dir() -> Result<PathBuf> {
    if let Some(path) = std::env::var_os("PHD_STATE_DIR") {
        return Ok(path.into());
    }
    directories::ProjectDirs::from("org", "PicoHarness", "phd")
        .map(|dirs| dirs.data_local_dir().to_path_buf())
        .ok_or_else(|| {
            Error::Configuration("cannot locate the user state directory; set PHD_STATE_DIR".into())
        })
}
pub fn token_path() -> Result<PathBuf> {
    std::env::var_os("PHD_TOKEN_FILE")
        .map(PathBuf::from)
        .map(Ok)
        .unwrap_or_else(|| state_dir().map(|path| path.join("token")))
}
impl Config {
    pub fn from_env() -> Result<Self> {
        let token = match std::env::var("PHD_TOKEN") {
            Ok(token) => token,
            Err(_) => {
                let path = token_path()?;
                std::fs::read_to_string(&path).map_err(|e| Error::Configuration(format!(
                    "cannot read phd token at {}: {e}; start phd or set PHD_TOKEN/PHD_TOKEN_FILE", path.display()
                )))?
            }
        };
        if token.trim().is_empty() {
            return Err(Error::Configuration("phd token must not be empty".into()));
        }
        Ok(Self {
            address: std::env::var("PHD_ADDR").unwrap_or_else(|_| DEFAULT_ADDR.into()),
            token: token.trim().into(),
            timeout: Duration::from_secs(30),
        })
    }
}

pub struct RpcClient {
    stream: BufReader<TcpStream>,
    next_id: u64,
    usable: bool,
    pub instance: String,
}
impl RpcClient {
    pub fn from_env() -> Result<Self> {
        Self::connect(&Config::from_env()?)
    }
    pub fn connect(config: &Config) -> Result<Self> {
        let mut last_error = None;
        let mut connected = None;
        for address in config.address.to_socket_addrs()? {
            match TcpStream::connect_timeout(&address, Duration::from_secs(3)) {
                Ok(stream) => {
                    connected = Some(stream);
                    break;
                }
                Err(e) => last_error = Some(e),
            }
        }
        let stream = connected.ok_or_else(|| {
            last_error.unwrap_or_else(|| {
                std::io::Error::new(
                    std::io::ErrorKind::AddrNotAvailable,
                    "phd address resolved to no socket addresses",
                )
            })
        })?;
        stream.set_nodelay(true)?;
        stream.set_read_timeout(Some(config.timeout))?;
        stream.set_write_timeout(Some(config.timeout))?;
        let mut client = Self {
            stream: BufReader::new(stream),
            next_id: 0,
            usable: true,
            instance: String::new(),
        };
        let hello: Value =
            client.call("hello", json!({"version": VERSION, "token": config.token}))?;
        if hello["version"] != VERSION {
            return Err(Error::Configuration(
                "incompatible phd protocol version".into(),
            ));
        }
        client.instance = hello["instance"]
            .as_str()
            .ok_or_else(|| Error::Configuration("phd omitted its instance ID".into()))?
            .into();
        Ok(client)
    }
    pub fn call<T: DeserializeOwned>(&mut self, method: &str, params: Value) -> Result<T> {
        if !self.usable {
            return Err(Error::Configuration(
                "phd session is closed; reconnect before another call".into(),
            ));
        }
        let result = self.call_inner(method, params);
        if matches!(
            result,
            Err(Error::Transport(_) | Error::Json(_) | Error::Configuration(_))
        ) {
            self.usable = false;
            let _ = self.stream.get_mut().shutdown(std::net::Shutdown::Both);
        }
        result
    }
    fn call_inner<T: DeserializeOwned>(&mut self, method: &str, params: Value) -> Result<T> {
        self.next_id = self
            .next_id
            .checked_add(1)
            .ok_or_else(|| Error::Configuration("RPC request ID exhausted".into()))?;
        let request = Request {
            jsonrpc: "2.0".into(),
            id: self.next_id,
            method: method.into(),
            params,
        };
        let mut frame = serde_json::to_vec(&request)?;
        frame.push(b'\n');
        if frame.len() > ph_protocol::MAX_FRAME {
            return Err(Error::InvalidArgument(
                "request exceeds socket frame limit".into(),
            ));
        }
        self.stream.get_mut().write_all(&frame)?;
        let frame = ph_protocol::read_frame(&mut self.stream)?.ok_or_else(|| {
            std::io::Error::new(std::io::ErrorKind::UnexpectedEof, "phd closed the session")
        })?;
        // `null` is a valid result (including Rust's unit type). Check field
        // presence before decoding; Option<Value> would lose this distinction.
        let reply: Value = serde_json::from_slice(&frame)?;
        let fields = reply
            .as_object()
            .ok_or_else(|| Error::Configuration("phd reply must be an object".into()))?;
        if reply["jsonrpc"] != "2.0"
            || reply["id"].as_u64() != Some(self.next_id)
            || fields.contains_key("result") == fields.contains_key("error")
        {
            return Err(Error::Configuration(
                "phd reply has an invalid envelope or request ID".into(),
            ));
        }
        if let Some(error) = fields.get("error") {
            return Err(serde_json::from_value::<RpcError>(error.clone())?.into());
        }
        Ok(serde_json::from_value(reply["result"].clone())?)
    }
    pub fn address(&self) -> Result<std::net::SocketAddr> {
        Ok(self.stream.get_ref().peer_addr()?)
    }
    /// Retain read-ahead bytes when an RPC upgrades the connection to a binary
    /// tunnel: the peer may send its first payload alongside the JSON reply.
    pub fn into_buffered_stream(self) -> BufReader<TcpStream> {
        self.stream
    }
    pub fn upload(&mut self, path: &std::path::Path) -> Result<String> {
        use base64::Engine;
        use std::io::Read;
        let mut file = std::fs::File::open(path)?;
        let length = file.metadata()?.len();
        let extension = path.extension().and_then(|s| s.to_str()).unwrap_or("");
        let upload: Value = self.call(
            "upload.begin",
            json!({"length":length,"extension":extension}),
        )?;
        let id = upload["id"]
            .as_str()
            .ok_or_else(|| Error::Configuration("phd omitted upload ID".into()))?
            .to_owned();
        let mut offset = 0u64;
        let mut buffer = vec![0u8; 512 * 1024];
        loop {
            let count = file.read(&mut buffer)?;
            if count == 0 {
                break;
            }
            self.call::<Value>("upload.write",json!({"id":id,"offset":offset,"data":base64::engine::general_purpose::STANDARD.encode(&buffer[..count])}))?;
            offset += count as u64;
        }
        self.call::<Value>("upload.finish", json!({"id":id}))?;
        Ok(id)
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::io::{BufRead, Read};
    use std::net::TcpListener;

    #[test]
    fn tunnel_upgrade_preserves_read_ahead_bytes() {
        let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let socket = TcpStream::connect(listener.local_addr().unwrap()).unwrap();
        socket
            .set_read_timeout(Some(Duration::from_secs(3)))
            .unwrap();
        let (mut peer, _) = listener.accept().unwrap();
        let payload = [0, 255, b'$', b'#', b'\n', 128, 42, 0];
        peer.write_all(&payload).unwrap();
        let mut reader = BufReader::new(socket);
        // Explicitly prime the buffer so this test does not depend on how TCP
        // splits the JSON upgrade reply and the first binary payload.
        assert!(!reader.fill_buf().unwrap().is_empty());
        let client = RpcClient {
            stream: reader,
            next_id: 0,
            usable: true,
            instance: String::new(),
        };
        let mut tunnel = client.into_buffered_stream();
        let mut received = [0u8; 8];
        tunnel.read_exact(&mut received).unwrap();
        assert_eq!(received, payload);
    }
}
