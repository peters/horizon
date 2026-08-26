#[tokio::main]
async fn main() {
    tracing_subscriber::fmt()
        .with_env_filter(tracing_subscriber::EnvFilter::from_default_env())
        .with_ansi(false)
        .with_writer(std::io::stderr)
        .init();

    if let Err(error) = horizon_browser_mcp::serve_stdio().await {
        tracing::error!(%error, "Horizon browser MCP server stopped with an error");
        std::process::exit(1);
    }
}
