use super::*;

#[test]
fn independent_cursors_and_overflow_are_explicit() {
    let mut events = Events::default();
    for n in 0..1030 {
        events.push(json!({"value":n}));
    }
    let slow = events.read("instance", 0, 3);
    assert!(slow.gap);
    assert_eq!(slow.events[0]["value"], 6);
    let fast = events.read("instance", 1028, 10);
    assert!(!fast.gap);
    assert_eq!(fast.events.len(), 2);
    assert_eq!(events.read("instance", 1028, 10).events, fast.events);
    assert!(events.read("instance", 2000, 10).gap);
}
#[test]
fn image_upload_enforces_offsets_length_and_completion() {
    let mut upload = crate::tasks::Upload::new(&json!({"length":4,"extension":"bin"})).unwrap();
    let path = upload.file.path().to_owned();
    assert!(upload.write(1, b"a").is_err());
    upload.write(0, b"ab").unwrap();
    assert!(upload.finish().is_err());
    assert!(upload.write(2, b"cde").is_err());
    upload.write(2, b"cd").unwrap();
    upload.finish().unwrap();
    assert!(upload.write(4, b"").is_err());
    assert_eq!(std::fs::read(&path).unwrap(), b"abcd");
    drop(upload);
    assert!(!path.exists());
    assert!(crate::tasks::Upload::new(&json!({"length":1,"extension":"../bin"})).is_err());
}
#[test]
fn exclusive_owners_reject_competing_mutations() {
    let mut board = Board {
        serial: "fixture".into(),
        link: None,
        events: Events::default(),
        owner: None,
        suspended: false,
    };
    board.lease(1).unwrap();
    board.check_mutation(1).unwrap();
    assert_eq!(board.lease(2).unwrap_err().kind, "busy");
    assert_eq!(board.check_mutation(2).unwrap_err().kind, "busy");
    board.owner = None;
    board.lease(2).unwrap();
}
#[test]
fn credentials_are_private_and_reused() {
    let dir = tempfile::tempdir().unwrap();
    let path = dir.path().join("token");
    let token = crate::credentials::load_or_create(&path).unwrap();
    assert_eq!(token.len(), 64);
    assert_eq!(crate::credentials::load_or_create(&path).unwrap(), token);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            std::fs::metadata(&path).unwrap().permissions().mode() & 0o777,
            0o600
        );
        std::fs::set_permissions(&path, std::fs::Permissions::from_mode(0o644)).unwrap();
        assert!(crate::credentials::load_or_create(&path).is_err());
    }
}
#[test]
fn framing_rejects_truncated_and_oversized_input() {
    let mut truncated = std::io::Cursor::new(b"{\"id\":1}");
    assert!(ph_protocol::read_frame(&mut truncated).is_err());
    let mut oversized = std::io::Cursor::new(vec![b'x'; ph_protocol::MAX_FRAME + 1]);
    assert!(ph_protocol::read_frame(&mut oversized).is_err());
}

#[cfg(unix)]
mod wire {
    use super::*;
    use ph_bridge::{framing::FrameBuffer, msg::*};
    use ph_client::{Config, RpcClient};
    use prost::Message;
    use std::io::{Read, Write};
    use std::{
        fs::File,
        net::{TcpListener, TcpStream},
        os::fd::{AsFd, OwnedFd},
        sync::atomic::AtomicBool,
        time::Instant,
    };
    struct Fixture {
        runtime: Arc<Runtime>,
        config: Config,
        server: Option<thread::JoinHandle<std::io::Result<()>>>,
        wire: Option<thread::JoinHandle<()>>,
        stop: Arc<AtomicBool>,
        _slave: OwnedFd,
        sequences: Arc<Mutex<Vec<u32>>>,
    }
    impl Fixture {
        fn new() -> Self {
            let pty = nix::pty::openpty(None, None).unwrap();
            let path = nix::unistd::ttyname(&pty.slave)
                .unwrap()
                .to_string_lossy()
                .into_owned();
            let link =
                ph_device::PhBoardLink::open_port(path.clone(), Some("fixture".into())).unwrap();
            let runtime = Runtime::new("0123456789abcdef0123456789abcdef".into());
            *runtime.fixtures.lock().unwrap() = Some(vec![DeviceInfo {
                serial: Some("fixture".into()),
                bus: 1,
                address: 1,
                product: "PH fixture".into(),
                access_error: None,
                terminal: Some(path),
            }]);
            runtime.boards.lock().unwrap().insert(
                "fixture".into(),
                Arc::new(Mutex::new(Board {
                    serial: "fixture".into(),
                    link: Some(link),
                    events: Events::default(),
                    owner: None,
                    suspended: false,
                })),
            );
            let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
            let config = Config {
                address: listener.local_addr().unwrap().to_string(),
                token: runtime.token.clone(),
                timeout: Duration::from_secs(5),
            };
            let daemon = runtime.clone();
            let server = thread::spawn(move || crate::server::serve(listener, daemon));
            let stop = Arc::new(AtomicBool::new(false));
            let finished = stop.clone();
            let sequences = Arc::new(Mutex::new(Vec::new()));
            let seen = sequences.clone();
            let wire = thread::spawn(move || {
                let mut master = File::from(pty.master);
                let mut frames = FrameBuffer::new();
                let mut power = false;
                while !finished.load(Ordering::Relaxed) {
                    let mut polls = [nix::poll::PollFd::new(
                        master.as_fd(),
                        nix::poll::PollFlags::POLLIN,
                    )];
                    if nix::poll::poll(&mut polls, 50u16).unwrap() == 0 {
                        continue;
                    }
                    let mut buffer = [0u8; 512];
                    let Ok(count) = master.read(&mut buffer) else {
                        break;
                    };
                    if count == 0 {
                        break;
                    }
                    frames.extend(&buffer[..count]);
                    while let Some(request) = frames.try_next().unwrap() {
                        seen.lock().unwrap().push(request.seq);
                        let mut extra = None;
                        let body = match request.body.unwrap() {
                            ph_message::Body::GetInfo(_) => {
                                Some(ph_message::Body::GetInfo(GetInfo {
                                    info: "PicoHarness fixture".into(),
                                }))
                            }
                            ph_message::Body::GetStatus(_) => {
                                Some(ph_message::Body::GetStatus(GetStatus {
                                    power_enabled: power,
                                    power_good: power,
                                    ..Default::default()
                                }))
                            }
                            ph_message::Body::PowerSet(value) => {
                                power = value.enabled;
                                None
                            }
                            ph_message::Body::Measure(_) => {
                                Some(ph_message::Body::Measure(Measure {
                                    vbus_uv: 3300000,
                                    current_ua: 1200,
                                    ..Default::default()
                                }))
                            }
                            ph_message::Body::ConsoleTx(data) => {
                                extra = Some(PhMessage {
                                    seq: 0,
                                    status: 0,
                                    body: Some(ph_message::Body::ConsoleData(ConsoleData {
                                        data: data.data,
                                    })),
                                });
                                None
                            }
                            _ => None,
                        };
                        let response = PhMessage {
                            seq: request.seq,
                            status: 0,
                            body,
                        };
                        let mut bytes = response.encode_length_delimited_to_vec();
                        if let Some(extra) = extra {
                            bytes.extend(extra.encode_length_delimited_to_vec());
                        }
                        // Force the real serial parser to handle fragmented replies.
                        for part in bytes.chunks(3) {
                            if master.write_all(part).is_err() {
                                return;
                            }
                        }
                    }
                }
            });
            Self {
                runtime,
                config,
                server: Some(server),
                wire: Some(wire),
                stop,
                _slave: pty.slave,
                sequences,
            }
        }
        fn client(&self) -> RpcClient {
            let mut client = RpcClient::connect(&self.config).unwrap();
            client
                .call::<Value>("device.open", json!({"serial":"fixture"}))
                .unwrap();
            client
        }
    }
    impl Drop for Fixture {
        fn drop(&mut self) {
            self.runtime.stop.store(true, Ordering::Relaxed);
            if let Some(server) = self.server.take() {
                server.join().unwrap().unwrap();
            }
            self.stop.store(true, Ordering::Relaxed);
            if let Some(wire) = self.wire.take() {
                wire.join().unwrap();
            }
        }
    }
    #[test]
    fn socket_clients_share_one_firmware_sequence_and_independent_events() {
        let fixture = Fixture::new();
        let mut first = fixture.client();
        let mut second = fixture.client();
        first
            .call::<Value>("board.power", json!({"enabled":true}))
            .unwrap();
        assert_eq!(
            second.call::<Value>("board.status", json!({})).unwrap()["power_on"],
            true
        );
        first
            .call::<Value>("console.write", json!({"data":STANDARD.encode(b"hello")}))
            .unwrap();
        let a: Value = first
            .call("events.read", json!({"cursor":0,"wait_ms":500,"limit":10}))
            .unwrap();
        let b: Value = second
            .call("events.read", json!({"cursor":0,"wait_ms":500,"limit":10}))
            .unwrap();
        assert_eq!(a["events"], b["events"]);
        assert_eq!(a["events"][0]["console"], STANDARD.encode(b"hello"));
        let config = fixture.config.clone();
        let workers: Vec<_> = (0..4)
            .map(|_| {
                let config = config.clone();
                thread::spawn(move || {
                    let mut client = RpcClient::connect(&config).unwrap();
                    client
                        .call::<Value>("device.open", json!({"serial":"fixture"}))
                        .unwrap();
                    for _ in 0..12 {
                        assert_eq!(
                            client.call::<String>("board.info", json!({})).unwrap(),
                            "PicoHarness fixture"
                        );
                    }
                })
            })
            .collect();
        for worker in workers {
            worker.join().unwrap();
        }
        let sequences = fixture.sequences.lock().unwrap();
        assert!(sequences.windows(2).all(|pair| pair[1] == pair[0] + 1));
    }
    #[test]
    fn authentication_protocol_errors_and_device_selection_are_structured() {
        let fixture = Fixture::new();
        let mut bad = fixture.config.clone();
        bad.token = "incorrect token".into();
        let error = match RpcClient::connect(&bad) {
            Ok(_) => panic!("wrong token accepted"),
            Err(error) => error,
        };
        assert!(matches!(error,ph_client::Error::Remote(ref error) if error.kind=="unauthorized"));
        let mut socket = TcpStream::connect(&fixture.config.address).unwrap();
        // Write one byte at a time to exercise the socket framing layer too.
        let request=json!({"jsonrpc":"2.0","id":1,"method":"hello","params":{"version":999,"token":fixture.config.token}}).to_string()+"\n";
        for byte in request.bytes() {
            socket.write_all(&[byte]).unwrap();
        }
        let mut reader = std::io::BufReader::new(socket);
        let response: Value =
            serde_json::from_slice(&ph_protocol::read_frame(&mut reader).unwrap().unwrap())
                .unwrap();
        assert_eq!(response["error"]["kind"], "version_mismatch");
        let mut client = RpcClient::connect(&fixture.config).unwrap();
        assert!(client
            .call::<Value>("device.open", json!({"serial":"other"}))
            .is_err());
        client
            .call::<Value>("device.open", json!({"serial":"fixture"}))
            .unwrap();
        assert!(
            matches!(client.call::<Value>("board.reset",json!({"milliseconds":2000})),Err(ph_client::Error::Remote(ref error)) if error.kind=="invalid_argument")
        );
        assert_eq!(
            client.call::<Value>("daemon.health", json!({})).unwrap()["status"],
            "ready"
        );
    }
    #[test]
    fn a_competing_lease_blocks_mutations_and_releases_without_replay() {
        let fixture = Fixture::new();
        let mut client = fixture.client();
        let board = fixture.runtime.boards.lock().unwrap()["fixture"].clone();
        board.lock().unwrap().lease(u64::MAX).unwrap();
        assert!(
            matches!(client.call::<Value>("board.power",json!({"enabled":true})),Err(ph_client::Error::Remote(ref e)) if e.kind=="busy")
        );
        // Queries still progress while another owner holds SWD.
        assert_eq!(
            client.call::<String>("board.info", json!({})).unwrap(),
            "PicoHarness fixture"
        );
        board.lock().unwrap().owner = None;
        assert_eq!(
            client.call::<Value>("board.status", json!({})).unwrap()["power_on"],
            false
        );
    }
    #[test]
    fn incomplete_uploads_cannot_create_tasks() {
        let fixture = Fixture::new();
        let mut client = fixture.client();
        let upload: Value = client
            .call("upload.begin", json!({"length":4,"extension":"bin"}))
            .unwrap();
        assert!(client
            .call::<Value>("upload.finish", json!({"id":upload["id"]}))
            .is_err());
        assert!(client
            .call::<Value>(
                "task.start",
                json!({"upload":upload["id"],"kind":"work_flash","target":"rp2040"})
            )
            .is_err());
        assert!(fixture.runtime.boards.lock().unwrap()["fixture"]
            .lock()
            .unwrap()
            .owner
            .is_none());
    }
    #[test]
    fn shutdown_closes_idle_sessions_and_changes_instance_on_restart() {
        let fixture = Fixture::new();
        let mut client = fixture.client();
        let instance = client.instance.clone();
        client.call::<Value>("daemon.stop", json!({})).unwrap();
        let deadline = Instant::now() + Duration::from_secs(2);
        while !fixture.server.as_ref().unwrap().is_finished() && Instant::now() < deadline {
            thread::sleep(Duration::from_millis(10));
        }
        assert!(fixture.server.as_ref().unwrap().is_finished());
        assert!(client
            .call::<Value>("board.power", json!({"enabled":true}))
            .is_err());
        assert_ne!(
            Runtime::new(fixture.config.token.clone()).instance,
            instance
        );
    }
    #[test]
    fn authenticated_gdb_tunnel_relays_binary_data_and_rejects_arbitrary_ports() {
        let fixture = Fixture::new();
        let echo = TcpListener::bind(("127.0.0.1", 0)).unwrap();
        let port = echo.local_addr().unwrap().port();
        fixture
            .runtime
            .debug_ports
            .lock()
            .unwrap()
            .insert(port, "fixture".into());
        let echo = thread::spawn(move || {
            let (mut stream, _) = echo.accept().unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(3)))
                .unwrap();
            let mut payload = [0u8; 8];
            stream.read_exact(&mut payload).unwrap();
            stream.write_all(&payload).unwrap();
        });
        let mut rpc = RpcClient::connect(&fixture.config).unwrap();
        assert!(rpc
            .call::<Value>("debug.tunnel", json!({"port":1}))
            .is_err());
        rpc.call::<()>("debug.tunnel", json!({"port":port}))
            .unwrap();
        let mut stream = rpc.into_buffered_stream();
        let payload = [0, 255, b'$', b'#', b'\n', 128, 42, 0];
        stream.get_mut().write_all(&payload).unwrap();
        let mut received = [0u8; 8];
        stream.read_exact(&mut received).unwrap();
        assert_eq!(received, payload);
        drop(stream);
        echo.join().unwrap();
    }

    #[test]
    fn uploaded_task_survives_disconnect_and_can_be_cancelled_by_another_client() {
        use std::os::unix::fs::PermissionsExt;
        let fixture = Fixture::new();
        let tools = tempfile::tempdir().unwrap();
        let image = tools.path().join("work.bin");
        std::fs::write(&image, [1u8, 2, 3, 4]).unwrap();
        // Block in the actual tool validation process, before any hardware
        // power or programming operation. Cancellation must kill and reap it.
        let tool = tools.path().join("openocd");
        std::fs::write(&tool, "#!/bin/sh\nwhile :; do :; done\n").unwrap();
        std::fs::set_permissions(&tool, std::fs::Permissions::from_mode(0o700)).unwrap();
        let mut owner = fixture.client();
        let upload = owner.upload(&image).unwrap();
        let task: Value = owner
            .call(
                "task.start",
                json!({"upload":upload,"kind":"work_flash","target":"rp2040","openocd":tool}),
            )
            .unwrap();
        drop(owner);
        let mut observer = RpcClient::connect(&fixture.config).unwrap();
        assert_eq!(
            observer
                .call::<Value>("task.status", json!({"id":task["id"]}))
                .unwrap()["state"],
            "running"
        );
        let listed: Vec<Value> = observer.call("task.list", json!({})).unwrap();
        assert_eq!(listed.len(), 1);
        assert_eq!(listed[0]["id"], task["id"]);
        assert_eq!(listed[0]["serial"], "fixture");
        let board = fixture.runtime.boards.lock().unwrap()["fixture"].clone();
        assert!(board.lock().unwrap().owner.is_some());
        observer
            .call::<()>("task.cancel", json!({"id":task["id"]}))
            .unwrap();
        let deadline = Instant::now() + Duration::from_secs(3);
        loop {
            let status: Value = observer
                .call("task.status", json!({"id":task["id"]}))
                .unwrap();
            if status["state"] != "running" {
                assert_eq!(status["state"], "cancelled");
                break;
            }
            assert!(Instant::now() < deadline, "cancellation failed to finish");
            thread::sleep(Duration::from_millis(10));
        }
        assert!(board.lock().unwrap().owner.is_none());
        assert!(!board.lock().unwrap().suspended);
    }
}
