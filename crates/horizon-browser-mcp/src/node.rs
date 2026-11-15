//! The card contract of one session of one machine, over one stream.
//!
//! Horizon listens on one Unix socket for each session of a machine, and the
//! `ssh` child of that machine carries the socket there. Each accepted stream is
//! one MCP connection of the agent in that session, and it serves one identity
//! that this computer minted: the actor `horizon:machine/<machine>/<session>` and
//! the host instance of this process. The server lists `block_panel` alone and
//! refuses every operation of it outside the card ones, so a machine agent draws
//! cards and reaches no machine, key, setting or panel of the person.

use rmcp::ServiceExt as _;
use tokio::io::{AsyncRead, AsyncWrite};

use crate::{HorizonBrowserMcp, StdioServerError};

/// Serve the card contract for `actor` of this host over `stream`.
///
/// # Errors
/// Returns when the transport of the stream cannot start, or when the service
/// task fails.
pub async fn serve_stream(
    actor: &str,
    host_instance: &str,
    stream: impl AsyncRead + AsyncWrite + Send + 'static,
) -> Result<(), StdioServerError> {
    let server = HorizonBrowserMcp::for_cards(actor, host_instance);
    let service = server
        .serve(stream)
        .await
        .map_err(|error| StdioServerError::Initialize(error.to_string()))?;
    service.waiting().await?;
    Ok(())
}

#[cfg(test)]
mod tests;
