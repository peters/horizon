#![forbid(unsafe_code)]

//! MCP adapter for audited control of live Horizon browser panels.
//!
//! MCP is the only public agent-facing contract. The Horizon manifest queue
//! and one-shot result files used by this crate are private coordination
//! details and are deliberately absent from every tool schema.

mod controller;
mod model;
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
    use rmcp::ServiceExt as _;

    let server = HorizonBrowserMcp::from_environment();
    let service = server
        .serve(rmcp::transport::stdio())
        .await
        .map_err(|error| StdioServerError::Initialize(error.to_string()))?;
    service.waiting().await?;
    Ok(())
}
