//! Exercise initialization and tool calls through the official MCP client and
//! a real stdio subprocess, including daemon failure after initialization.
use rmcp::{model::CallToolRequestParams, transport::TokioChildProcess, ServiceExt};
use std::{
    net::TcpListener,
    sync::{atomic::Ordering, Arc},
    thread,
};

struct Daemon {
    runtime: Arc<phd::runtime::Runtime>,
    worker: Option<thread::JoinHandle<std::io::Result<()>>>,
}
impl Drop for Daemon {
    fn drop(&mut self) {
        self.runtime.stop.store(true, Ordering::Relaxed);
        self.worker.take().unwrap().join().unwrap().unwrap();
    }
}

#[tokio::test]
async fn initialization_tools_and_daemon_failure_are_protocol_safe() {
    let listener = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let address = listener.local_addr().unwrap().to_string();
    let runtime = phd::runtime::Runtime::new("0123456789abcdef0123456789abcdef".into());
    let server = runtime.clone();
    let daemon = Daemon {
        runtime: runtime.clone(),
        worker: Some(thread::spawn(move || phd::server::serve(listener, server))),
    };
    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_ph-mcp"));
    command
        .env("PHD_ADDR", &address)
        .env("PHD_TOKEN", &runtime.token);
    let client = ().serve(TokioChildProcess::new(command).unwrap()).await.unwrap();
    let tools = client.list_all_tools().await.unwrap();
    assert_eq!(tools.len(), 18);
    let status = tools.iter().find(|t| t.name == "phd_status").unwrap();
    assert_eq!(
        status.input_schema["required"],
        serde_json::json!(["serial"])
    );
    let health = client
        .call_tool(CallToolRequestParams::new("phd_health"))
        .await
        .unwrap();
    assert_ne!(health.is_error, Some(true));
    assert_eq!(
        health.structured_content.unwrap()["result"]["instance"],
        runtime.instance
    );
    let caps = client
        .call_tool(CallToolRequestParams::new("phd_capabilities"))
        .await
        .unwrap();
    assert_eq!(
        caps.structured_content.unwrap()["result"]["logic_capture"],
        false
    );
    let invalid = client
        .call_tool(
            CallToolRequestParams::new("phd_memory_read").with_arguments(
                serde_json::json!({"serial":"fixture","address":3,"words":0})
                    .as_object()
                    .unwrap()
                    .clone(),
            ),
        )
        .await
        .unwrap();
    assert_eq!(invalid.is_error, Some(true));
    let invalid = invalid.structured_content.unwrap();
    assert_eq!(invalid["error"]["kind"], "invalid_argument");
    assert_eq!(invalid["error"]["retryable"], false);
    drop(daemon);
    let failed = client
        .call_tool(CallToolRequestParams::new("phd_health"))
        .await
        .unwrap();
    assert_eq!(failed.is_error, Some(true));
    assert_eq!(
        failed.structured_content.unwrap()["error"]["retryable"],
        true
    );
    client.cancel().await.unwrap();

    // An unavailable daemon does not prevent an agent from discovering tools.
    let mut command = tokio::process::Command::new(env!("CARGO_BIN_EXE_ph-mcp"));
    command
        .env("PHD_ADDR", address)
        .env("PHD_TOKEN", &runtime.token);
    let client = ().serve(TokioChildProcess::new(command).unwrap()).await.unwrap();
    assert_eq!(client.list_all_tools().await.unwrap().len(), 18);
    assert_eq!(
        client
            .call_tool(CallToolRequestParams::new("phd_devices"))
            .await
            .unwrap()
            .is_error,
        Some(true)
    );
    client.cancel().await.unwrap();
}
