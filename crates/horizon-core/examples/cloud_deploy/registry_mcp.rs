//! Optional stdio adapter bound to one explicitly configured machine settings file.
use horizon_core::cloud_runtime::{self, Cancellation, Error, registry, settings::Settings};
use rmcp::{
    RoleServer, ServerHandler, ServiceExt,
    handler::server::wrapper::{Json, Parameters},
    model::{Implementation, ServerCapabilities, ServerConfig},
    service::RequestContext,
    tool, tool_router,
};
use std::path::PathBuf;

#[derive(Clone)]
struct Server {
    settings: PathBuf,
}

struct CancelOnDrop(Cancellation);
impl Drop for CancelOnDrop {
    fn drop(&mut self) {
        self.0.cancel();
    }
}

#[tool_router]
impl Server {
    #[tool(
        name = "cloud_registry",
        description = "Manage explicitly saved machine-local private image bindings. Arguments: operation=verify with image (repository@sha256:digest), or operation=status|reconcile|revoke with repository and generation. Verify checks the read-only pull grant and immutable image, then creates or reconciles its provider pull binding. Revoke removes only that provider binding; issuer-token revocation and running workers are separate. No compute allocation or image publication. Secrets, provider endpoints and settings paths cannot be supplied through this tool. Configure credentials through Cloud settings or the registry-bind CLI first. Cancellation leaves uncertain mutations fenced for reconciliation."
    )]
    async fn registry(
        &self,
        Parameters(value): Parameters<serde_json::Map<String, serde_json::Value>>,
        context: RequestContext<RoleServer>,
    ) -> Result<Json<serde_json::Value>, String> {
        let action: registry::Action = serde_json::from_value(serde_json::Value::Object(value))
            .map_err(|_| "Invalid registry operation".to_owned())?;
        let settings = self.settings.clone();
        let cancellation = Cancellation::default();
        let guard = CancelOnDrop(cancellation.clone());
        let work = tokio::task::spawn_blocking(move || {
            let settings = Settings::load(&settings)?;
            registry::manage(&settings, &action, &cancellation)
        });
        tokio::select! {
            result = work => {
                let status = result.map_err(|_| "Registry operation failed".to_owned())?.map_err(|error| error.to_string())?;
                serde_json::to_value(status).map(Json).map_err(|_| "Invalid registry status".to_owned())
            }
            () = context.ct.cancelled() => {
                guard.0.cancel();
                Err("Registry operation cancelled; reconcile any pending provider mutation".into())
            }
        }
    }
}

impl ServerHandler for Server {
    async fn call_tool(
        &self,
        request: rmcp::model::CallToolRequestParams,
        context: RequestContext<RoleServer>,
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
        _context: RequestContext<RoleServer>,
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
            .with_server_info(Implementation::new("horizon-cloud-registry", env!("CARGO_PKG_VERSION")))
    }
}

pub(super) fn serve(settings: PathBuf) -> cloud_runtime::Result<()> {
    if !settings.is_absolute() {
        return Err(Error::Invalid(
            "Registry MCP requires an absolute machine settings path",
        ));
    }
    Settings::load(&settings)?;
    let runtime = tokio::runtime::Builder::new_multi_thread().enable_all().build()?;
    runtime.block_on(async move {
        let server = Server { settings };
        let service = server
            .serve((tokio::io::stdin(), tokio::io::stdout()))
            .await
            .map_err(|_| Error::Invalid("Registry MCP initialization failed"))?;
        service
            .waiting()
            .await
            .map_err(|_| Error::Invalid("Registry MCP transport failed"))?;
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn registry_tool_discovery_has_an_object_input_schema() {
        let tools = Server::tool_router().list_all();
        assert_eq!(tools.len(), 1);
        assert_eq!(tools[0].input_schema.get("type"), Some(&serde_json::json!("object")));
    }
}
