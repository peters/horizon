#![forbid(unsafe_code)]

//! MCP adapter for audited control of live Horizon browser panels.
//!
//! MCP is the only public agent-facing contract. The Horizon manifest queue
//! and one-shot result files used by this crate are private coordination
//! details and are deliberately absent from every tool schema.

mod controller;
mod model;
mod network_watch;
mod server;

pub use server::HorizonBrowserMcp;

/// Failure while serving MCP over the process standard streams.
#[derive(Debug, thiserror::Error)]
pub enum StdioServerError {
    #[error("could not initialize MCP stdio transport: {0}")]
    Initialize(String),
    #[error("MCP stdio service task failed: {0}")]
    Join(#[from] tokio::task::JoinError),
}

/// Serve the sole Horizon browser agent contract over newline-delimited
/// JSON-RPC on stdin/stdout until the client disconnects.
///
/// # Errors
/// Returns when transport initialization or the service task fails.
pub async fn serve_stdio() -> Result<(), StdioServerError> {
    serve(HorizonBrowserMcp::from_environment()).await
}

/// Serve the contract over stdio as `actor`, the one agent of the host
/// `host_instance` that runs in this process.
///
/// # Errors
/// Returns when transport initialization or the service task fails.
pub async fn serve_stdio_for_host(actor: &str, host_instance: &str) -> Result<(), StdioServerError> {
    serve(HorizonBrowserMcp::for_host(actor, host_instance)).await
}

async fn serve(server: HorizonBrowserMcp) -> Result<(), StdioServerError> {
    use rmcp::ServiceExt as _;

    let service = server
        .serve(rmcp::transport::stdio())
        .await
        .map_err(|error| StdioServerError::Initialize(error.to_string()))?;
    service.waiting().await?;
    Ok(())
}
