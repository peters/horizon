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
