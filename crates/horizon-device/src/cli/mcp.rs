use super::dispatch::{Command, Dispatcher};
use crate::{ActRequest, CaptureOptions, ResizeRequest};
use rmcp::{
    ServerHandler, ServiceExt,
    handler::server::wrapper::Parameters,
    model::{CallToolResult, ContentBlock, Implementation, ServerCapabilities, ServerInfo},
    tool, tool_router,
};

#[derive(Clone)]
pub struct Server {
    dispatcher: Dispatcher,
}
impl Server {
    pub fn new(dispatcher: Dispatcher) -> Self {
        Self { dispatcher }
    }
    async fn execute(&self, command: Command) -> CallToolResult {
        let dispatcher = self.dispatcher.clone();
        let Ok(mut delivered) = tokio::task::spawn_blocking(move || dispatcher.call(command)).await else {
            return CallToolResult::error(vec![ContentBlock::text(
                "device worker failed; observe before retrying",
            )]);
        };
        let response = &mut delivered.value;
        let success = response["ok"] == true;
        let mime = response["result"]["mime_type"]
            .as_str()
            .unwrap_or("image/png")
            .to_owned();
        let image = response["result"]
            .as_object_mut()
            .and_then(|r| r.remove("image_base64"));
        let mut content = vec![ContentBlock::text(response.to_string())];
        if let Some(serde_json::Value::String(image)) = image {
            content.push(ContentBlock::image(image, mime));
        }
        if let Err(error) = delivered.complete_observation() {
            return CallToolResult::error(vec![ContentBlock::text(error.to_string())]);
        }
        if success {
            CallToolResult::success(content)
        } else {
            CallToolResult::error(content)
        }
    }
}
#[tool_router]
impl Server {
    #[tool(
        name = "device_doctor",
        description = "Check the configured device, input/capture capabilities, desktop resize support, owner permission and limits."
    )]
    async fn doctor(&self) -> CallToolResult {
        self.execute(Command::Doctor).await
    }
    #[tool(
        name = "device_screenshot",
        description = "Observe the configured device. Optional crop, output dimensions and PNG/JPEG quality. Returns the original geometry, source_region and image_dimensions for mapping image pixels to surface coordinates."
    )]
    async fn screenshot(&self, Parameters(options): Parameters<CaptureOptions>) -> CallToolResult {
        self.execute(Command::Screenshot(options)).await
    }
    #[tool(
        name = "device_act",
        description = "Send one bounded input action to the configured device using fresh screenshot geometry. Success means dispatched, not verified app outcome. Do not retry an indeterminate action without observing. Touch and accessibility are unsupported."
    )]
    async fn act(&self, Parameters(request): Parameters<ActRequest>) -> CallToolResult {
        self.execute(Command::Act(request)).await
    }
    #[tool(
        name = "device_resize",
        description = "Explicitly resize the actual configured desktop within owner-enabled limits. Check device_doctor first. Returns requested and confirmed dimensions; capture a fresh screenshot before input. Never retry an uncertain resize without owner reconciliation. Screenshot output dimensions and viewer Fit do not resize the desktop. A bounded resize already in progress completes even if this request is cancelled."
    )]
    async fn resize(&self, Parameters(request): Parameters<ResizeRequest>) -> CallToolResult {
        self.execute(Command::Resize(request)).await
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

    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("horizon-device", env!("CARGO_PKG_VERSION")))
            .with_instructions("Only control the configured authorized device. Observe before and after input. Live viewing is read-only; use device tools for application input. Bounded input already in progress finishes and releases held input even if a tool request is cancelled.")
    }
}

pub async fn serve(dispatcher: Dispatcher) -> u8 {
    let result = async {
        let service = Server::new(dispatcher)
            .serve(rmcp::transport::stdio())
            .await
            .map_err(|error| error.to_string())?;
        service.waiting().await.map_err(|error| error.to_string())?;
        Ok::<_, String>(())
    }
    .await;
    match result {
        Ok(()) => 0,
        Err(error) => {
            eprintln!("device MCP transport failed: {error}");
            1
        }
    }
}
