use super::{CATALOG, inspect, list, now};
use rmcp::{
    ServerHandler, ServiceExt,
    handler::server::wrapper::Parameters,
    model::{CallToolResult, ContentBlock, Implementation, ServerCapabilities, ServerConfig},
    tool, tool_router,
};
use std::{io, path::Path};

#[derive(Clone)]
struct Server;

#[derive(serde::Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct Inspect {
    alias: String,
}

fn response(result: io::Result<impl serde::Serialize>) -> CallToolResult {
    match result.and_then(|value| serde_json::to_value(value).map_err(io::Error::other)) {
        Ok(value) => CallToolResult::structured(value),
        Err(error) => CallToolResult::error(vec![ContentBlock::text(error.to_string())]),
    }
}

#[tool_router]
impl Server {
    #[tool(
        name = "cloud_companions_list",
        description = "List declared companion repositories, explicit selection, pinned cloud identity, last controller-observed status and SSH details. Read-only; never starts or provisions a cloud. Snapshots older than 60 seconds cannot report ready. Use inspect for a live direct-SSH probe; the controller may be offline while existing SSH remains usable."
    )]
    async fn list(&self) -> CallToolResult {
        response(list(Path::new(CATALOG), now()))
    }

    #[tool(
        name = "cloud_companion_inspect",
        description = "Inspect one declared companion alias, probing existing selected SSH access and its worktree. Returns repository/profile, pinned cloud identity, status, SSH alias and isolated worktree. Never grants access, starts or provisions a cloud. Use ordinary SSH, Git and rsync for commands and transfers. Stopped or unavailable targets need owner action in M0."
    )]
    async fn inspect(&self, Parameters(request): Parameters<Inspect>) -> CallToolResult {
        match tokio::task::spawn_blocking(move || {
            inspect(
                Path::new(CATALOG),
                &request.alias,
                now(),
                crate::companions::probe_access,
            )
        })
        .await
        {
            Ok(result) => response(result),
            Err(_) => CallToolResult::error(vec![ContentBlock::text("Companion inspection failed")]),
        }
    }

    #[tool(
        name = "cloud_offers",
        description = "Rank cloud compute offers the Horizon that owns this cloud can rent for given requirements, cheapest estimated total first, without renting anything. Pass minimum vCPU and memory for CPU workers, or gpu=true with an optional minimum GPU memory or GPU type, plus an optional maximum hourly price, expected hours, workspace storage in GB and region. Each offer has its hourly price (for a CPU size, the highest among the flavors Horizon requests for it, since the provider picks one), an estimated total for the hours including workspace storage, availability (CPU sizes are confirmed when a cloud is created), regions with GPU stock, and host trust. Prices are the ones that Horizon last sent this worker, every 15 minutes while it runs; the result says when they were observed. Without prices from the last 20 minutes the tool returns an error rather than old prices. Offers are informational, never a reservation."
    )]
    async fn cloud_offers(
        &self,
        Parameters(requirements): Parameters<horizon_cloud::offers::Requirements>,
    ) -> CallToolResult {
        match crate::offers::rank_here(&requirements) {
            Ok(offers) => CallToolResult::structured(offers),
            Err(error) => CallToolResult::error(vec![ContentBlock::text(error)]),
        }
    }
}

impl ServerHandler for Server {
    async fn call_tool(
        &self,
        request: rmcp::model::CallToolRequestParams,
        context: rmcp::service::RequestContext<rmcp::RoleServer>,
    ) -> Result<rmcp::model::CallToolResponse, rmcp::ErrorData> {
        Self::tool_router()
            .call(rmcp::handler::server::tool::ToolCallContext::new(
                self, request, context,
            ))
            .await
    }

    fn list_tools(
        &self,
        _request: Option<rmcp::model::PaginatedRequestParams>,
        _context: rmcp::service::RequestContext<rmcp::RoleServer>,
    ) -> impl Future<Output = Result<rmcp::model::ListToolsResult, rmcp::ErrorData>> {
        std::future::ready(Ok(rmcp::model::ListToolsResult {
            tools: Self::tool_router().list_all(),
            ttl_ms: Some(0),
            cache_scope: Some(rmcp::model::CacheScope::Private),
            ..Default::default()
        }))
    }

    fn get_tool(&self, name: &str) -> Option<rmcp::model::Tool> {
        Self::tool_router().get(name).cloned()
    }

    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("horizon-cloud-companions", env!("CARGO_PKG_VERSION")))
            .with_instructions("Companion selection grants trusted shell access. Discovery is read-only and never makes stopped clouds ready. Use returned SSH aliases and separate worktrees; preserve others' files.")
    }
}

pub(super) fn run() -> io::Result<()> {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()?
        .block_on(async {
            let server = Server;
            let service = server.serve(rmcp::transport::stdio()).await.map_err(io::Error::other)?;
            service.waiting().await.map_err(io::Error::other)?;
            Ok(())
        })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn discovery_catalog_has_required_cache_metadata() {
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
        let (client, transport) = tokio::io::duplex(64 * 1024);
        let task = tokio::spawn(async move {
            Server
                .serve(transport)
                .await
                .expect("start server")
                .waiting()
                .await
                .expect("server exit");
        });
        let (input, mut output) = tokio::io::split(client);
        let mut input = BufReader::new(input);
        for (id, method) in [(1, "server/discover"), (2, "tools/list")] {
            let request = serde_json::json!({
                "jsonrpc": "2.0", "id": id, "method": method,
                "params": {"_meta": {
                    "io.modelcontextprotocol/protocolVersion": "2026-07-28",
                    "io.modelcontextprotocol/clientCapabilities": {}
                }}
            });
            output
                .write_all(format!("{request}\n").as_bytes())
                .await
                .expect("write request");
            let mut line = String::new();
            tokio::time::timeout(std::time::Duration::from_secs(5), input.read_line(&mut line))
                .await
                .expect("response deadline")
                .expect("read response");
            let response: serde_json::Value = serde_json::from_str(&line).expect("response JSON");
            assert!(response.get("error").is_none(), "{response}");
            if method == "tools/list" {
                assert_eq!(response["result"]["resultType"], "complete");
                assert_eq!(response["result"]["ttlMs"], 0);
                assert_eq!(response["result"]["cacheScope"], "private");
                let tools = response["result"]["tools"].as_array().expect("tools");
                assert_eq!(tools.len(), 3);
                // Agents without browser tools still rank cloud offers here.
                let offers = tools
                    .iter()
                    .find(|tool| tool["name"] == "cloud_offers")
                    .expect("cloud_offers tool");
                // Every requirement is described for the agent.
                let properties = offers["inputSchema"]["properties"].as_object().expect("properties");
                assert_eq!(properties.len(), 11);
                for (field, schema) in properties {
                    assert!(
                        schema["description"].as_str().is_some_and(|text| !text.is_empty()),
                        "{field}"
                    );
                }
            }
        }
        drop(output);
        drop(input);
        tokio::time::timeout(std::time::Duration::from_secs(5), task)
            .await
            .expect("shutdown deadline")
            .expect("server task");
    }
}
