//! stdout is reserved for MCP frames; diagnostics go to stderr.
use rmcp::{transport::stdio, ServiceExt};
#[tokio::main]
async fn main() {
    let result = async {
        let server = ph_mcp::Harness::default().serve(stdio()).await?;
        server.waiting().await?;
        Ok::<(), Box<dyn std::error::Error>>(())
    }
    .await;
    if let Err(error) = result {
        eprintln!("ph-mcp: {error}");
        std::process::exit(1);
    }
}
