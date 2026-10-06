//! Public native MCP adapter over the shared owned host actor; no provider secrets enter tool inputs.
mod evidence;
mod media;
mod model;
mod run;
use crate::{Error, Result, actor::Actor};
use base64::Engine as _;
use evidence::Evidence;
use rmcp::{
    ErrorData, RoleServer, ServerHandler,
    handler::server::{
        router::tool::ToolRouter,
        tool::ToolCallContext,
        wrapper::{Json, Parameters},
    },
    model::{
        CallToolRequestParams, CallToolResponse, CallToolResult, ContentBlock, Implementation, ListToolsResult,
        PaginatedRequestParams, ServerCapabilities, ServerConfig, Tool,
    },
    service::RequestContext,
    tool, tool_router,
};
use serde::Serialize;
use serde_json::{Value, json};
use std::{path::Path, sync::Arc, time::Duration};
use uuid::Uuid;

#[derive(Clone)]
pub struct NativeMcp {
    actor: Arc<Actor>,
    evidence: Arc<Evidence>,
    reports: Arc<crate::archive::Store>,
    media: Arc<std::sync::Mutex<Option<Arc<crate::archive::Archive>>>>,
    views: Arc<crate::view::Registry>,
    router: ToolRouter<Self>,
}
impl NativeMcp {
    /// # Errors
    /// Construct only with the host's selected actor and an existing owned private evidence directory.
    pub fn new(actor: Arc<Actor>, evidence: &Path, reports: &Path) -> Result<Self> {
        actor.arm()?;
        let views = Arc::new(crate::view::Registry::new(&actor));
        Ok(Self {
            actor,
            evidence: Arc::new(Evidence::new(evidence)?),
            reports: Arc::new(crate::archive::Store::new(reports)?),
            media: Arc::new(std::sync::Mutex::new(None)),
            views,
            router: Self::tool_router(),
        })
    }
    async fn execute<T: Serialize + Send + 'static>(
        &self,
        operation: impl FnOnce(&Actor) -> Result<T> + Send + 'static,
    ) -> std::result::Result<Json<Value>, String> {
        let actor = Arc::clone(&self.actor);
        tokio::task::spawn_blocking(move || {
            let result = operation(&actor)?;
            serde_json::to_value(result).map_err(|_| Error::Unavailable)
        })
        .await
        .map_err(|_| Error::Unavailable.to_string())?
        .map(Json)
        .map_err(|error| error.to_string())
    }
}
fn handle(value: &str) -> Result<Uuid> {
    let id = Uuid::parse_str(value).map_err(|_| Error::SessionUnknown)?;
    if id.is_nil() || id.to_string() != value {
        return Err(Error::SessionUnknown);
    }
    Ok(id)
}
fn lifetime(seconds: u64) -> Result<Duration> {
    if !(1..=1800).contains(&seconds) {
        return Err(horizon_app_runtime::Error::OperationInvalid.into());
    }
    Ok(Duration::from_secs(seconds))
}
#[tool_router]
impl NativeMcp {
    #[tool(
        name = "app_view",
        description = "Open or reuse a read-only live RFB screenshot stream for this exact owned native session. Returns a numeric loopback endpoint; attach it with the public device_panel create tool in the caller's Horizon workspace. Inspect image_displayed and changing frames automatically. The fixed 768x1536 presentation preserves aspect ratio and does not change native geometry. Streams capture at most once per second, stop with native closure/expiry/host exit and never forward keyboard, pointer or clipboard input."
    )]
    async fn app_view(
        &self,
        Parameters(input): Parameters<model::Session>,
    ) -> std::result::Result<Json<Value>, String> {
        let views = Arc::clone(&self.views);
        tokio::task::spawn_blocking(move || {
            views
                .open(handle(&input.session)?)
                .and_then(|view| serde_json::to_value(view).map_err(|_| Error::Unavailable))
        })
        .await
        .map_err(|_| Error::Unavailable.to_string())?
        .map(Json)
        .map_err(|error| error.to_string())
    }
    #[tool(
        name = "app_logs",
        description = "Export bounded redacted device, crash, Appium or network logs for the exact owned native session to private retained evidence. Returns an opaque local evidence handle/path, never raw logs or provider URLs. Network logs require provider capture support and an enabled capture; disabled, pending and unavailable evidence returns a typed error. Provider-sensitive log lines are removed before saving; other app content may remain private."
    )]
    async fn app_logs(&self, Parameters(input): Parameters<model::Logs>) -> std::result::Result<Json<Value>, String> {
        media::logs(self, input).await
    }
    #[tool(
        name = "app_video",
        description = "Manage the owned session's provider recording. Recording starts automatically at allocation when the project enables video; start/status report that configured state. get downloads finalized private MP4 evidence. stop closes this native session and its services before requesting its finalized recording. BrowserStack cannot pause or start a recording disabled at allocation. Downloads never return signed provider URLs; pending/disabled/unavailable evidence is a typed error."
    )]
    async fn app_video(&self, Parameters(input): Parameters<model::Video>) -> std::result::Result<Json<Value>, String> {
        media::video(self, input).await
    }
    #[tool(
        name = "app_audit",
        description = "Read this actor's native upload/create/snapshot/action/wait/close/reset/media/link receipts after a sequence cursor, 1..256 entries. Screenshot and tunnel-status polling are excluded. Compare stream UUID before resuming a saved cursor; actor restart creates a new stream. Returns opaque operation/session handles, accepted/completed/failed outcomes, durations and typed error codes. The latest 256 entries are retained for this host lifetime; truncated reports a cursor older than retention. Never records target labels, typed content, URL values, credentials or provider references. Completion is driver acknowledgement, not an app-level assertion."
    )]
    async fn app_audit(&self, Parameters(input): Parameters<model::Audit>) -> std::result::Result<Json<Value>, String> {
        self.execute(move |actor| actor.audit(input.after_sequence, input.limit))
            .await
    }
    #[tool(
        name = "device_test_run",
        description = "Build the selected project's declared native apps, upload each platform once, and run every resolved device and executable recipe concurrently within the current App Automate quota and a two-lane host bound. Streams progress when requested. Returns per-device/step outcomes, retained screenshots/provider video/failure-log evidence, authenticated token-free provider links and verified cleanup state. Uses only the host-selected project and credentials."
    )]
    async fn device_test_run(
        &self,
        Parameters(input): Parameters<model::Run>,
        context: RequestContext<RoleServer>,
    ) -> std::result::Result<Json<Value>, String> {
        run::execute(
            self,
            lifetime(input.lifetime_seconds).map_err(|error| error.to_string())?,
            context,
        )
        .await
    }
    #[tool(
        name = "app_upload",
        description = "Upload the selected project's declared iOS or Android artifact with Horizon-held credentials. Returns an opaque owned handle and content hash; never a provider token. Lifetime is 1..1800 seconds and never renews a cached upload's original deadline."
    )]
    async fn app_upload(
        &self,
        Parameters(input): Parameters<model::Upload>,
    ) -> std::result::Result<Json<Value>, String> {
        self.execute(move |actor| actor.upload(input.platform, lifetime(input.lifetime_seconds)?))
            .await
    }
    #[tool(
        name = "app_session_create",
        description = "Create a real native app session for a declared matrix index and owned artifact handle. The host starts distinct managed backends and an exact restricted tunnel for this device before allocation. No endpoint, credential, command, root path, app token or forwarding port is accepted. Lost allocation outcomes are held for reconciliation rather than replayed."
    )]
    async fn app_session_create(
        &self,
        Parameters(input): Parameters<model::Create>,
    ) -> std::result::Result<Json<Value>, String> {
        self.execute(move |actor| {
            actor.create(
                input.matrix_index,
                handle(&input.artifact)?,
                lifetime(input.lifetime_seconds)?,
            )
        })
        .await
    }
    #[tool(
        name = "app_snapshot",
        description = "Read a fresh normalized native accessibility tree for an owned app session. Every snapshot invalidates older refs; refs expire after 30 seconds and cannot cross sessions. Returns identifier, role, label, value, bounds, visibility and enabled state."
    )]
    async fn app_snapshot(
        &self,
        Parameters(input): Parameters<model::Session>,
    ) -> std::result::Result<Json<Value>, String> {
        self.execute(move |actor| actor.snapshot(handle(&input.session)?)).await
    }
    #[tool(
        name = "app_act",
        description = "Perform one bounded native action on an owned session: tap, long_press, type, clear, swipe, scroll, back, home, rotate, launch, terminate or deep_link. Targets use fresh refs, accessibility identifiers, labels or coordinates. Ambiguous targets fail. Reset closes the original native session and its services before reallocation, preserving the upload and original deadline. Reset returns a replacement opaque session handle; use it for subsequent actions and cleanup."
    )]
    async fn app_act(&self, Parameters(input): Parameters<model::Act>) -> std::result::Result<Json<Value>, String> {
        self.execute(move |actor| {
            let session = handle(&input.session)?;
            if matches!(input.action, horizon_app_testing::recipe::Action::Reset {}) {
                return Ok(json!({"completed":true,"replacement":actor.reset(session)?}));
            }
            actor.act(session, &input.action)?;
            Ok(json!({"completed":true}))
        })
        .await
    }
    #[tool(
        name = "app_wait",
        description = "Wait for a native target to become visible, hidden, enabled or disabled in an owned session. timeout_millis is 1..60000 and is clamped to original native and service deadlines. Waiting does not renew a lease or block another device lane."
    )]
    async fn app_wait(&self, Parameters(input): Parameters<model::Wait>) -> std::result::Result<Json<Value>, String> {
        self.execute(move |actor| {
            if !(1..=60_000).contains(&input.timeout_millis) {
                return Err(horizon_app_testing::Error::RecipeInvalid.into());
            }
            actor.wait(
                handle(&input.session)?,
                &input.target,
                input.state,
                Duration::from_millis(input.timeout_millis),
            )?;
            Ok(json!({"completed":true}))
        })
        .await
    }
    #[tool(
        name = "app_screenshot",
        description = "Capture an owned native session as a bounded PNG and private local evidence reference. The host retains the latest 32 exports across this actor, removing them on normal actor shutdown. A capture proves one observed image; use a live Device panel for stream/display proof."
    )]
    async fn app_screenshot(
        &self,
        Parameters(input): Parameters<model::Session>,
    ) -> std::result::Result<CallToolResult, String> {
        let actor = Arc::clone(&self.actor);
        let evidence = Arc::clone(&self.evidence);
        tokio::task::spawn_blocking(move || {
            let session = handle(&input.session)?;
            let png = actor.screenshot(session)?;
            let capture = evidence.screenshot(session, &png)?;
            let metadata = serde_json::to_string(&capture).map_err(|_| Error::Unavailable)?;
            Ok(CallToolResult::success(vec![
                ContentBlock::text(metadata),
                ContentBlock::image(base64::engine::general_purpose::STANDARD.encode(png), "image/png"),
            ]))
        })
        .await
        .map_err(|_| Error::Unavailable.to_string())?
        .map_err(|error: Error| error.to_string())
    }
    #[tool(
        name = "app_tunnel_status",
        description = "Observe only the exact owned service lane's tunnel readiness, declared loopback ports and original time remaining. Never returns tunnel credentials, provider session references or arbitrary forwarding configuration."
    )]
    async fn app_tunnel_status(
        &self,
        Parameters(input): Parameters<model::Session>,
    ) -> std::result::Result<Json<Value>, String> {
        self.execute(move |actor| actor.tunnel_status(handle(&input.session)?))
            .await
    }
    #[tool(
        name = "app_session_close",
        description = "Close an owned native session and wait for acknowledgement before stopping its exact tunnel and synthetic backends. Cleanup uncertainty retains that lane until reconciliation or original expiry. Unknown or foreign handles cannot delete a provider resource."
    )]
    async fn app_session_close(
        &self,
        Parameters(input): Parameters<model::Session>,
    ) -> std::result::Result<Json<Value>, String> {
        self.execute(move |actor| {
            actor.close(handle(&input.session)?)?;
            Ok(json!({"closed":true}))
        })
        .await
    }
}
impl ServerHandler for NativeMcp {
    fn get_info(&self) -> ServerConfig {
        ServerConfig::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new("horizon-native", env!("CARGO_PKG_VERSION")))
    }
    async fn call_tool(
        &self,
        request: CallToolRequestParams,
        context: RequestContext<RoleServer>,
    ) -> std::result::Result<CallToolResponse, ErrorData> {
        self.router.call(ToolCallContext::new(self, request, context)).await
    }
    fn list_tools(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> impl Future<Output = std::result::Result<ListToolsResult, ErrorData>> {
        std::future::ready(Ok(ListToolsResult {
            tools: self.router.list_all(),
            ttl_ms: Some(0),
            cache_scope: Some(rmcp::model::CacheScope::Private),
            ..Default::default()
        }))
    }
    fn get_tool(&self, name: &str) -> Option<Tool> {
        self.router.get(name).cloned()
    }
}
/// # Errors
/// Serve an already-authorized host actor; transport errors expose no provider response or credentials.
pub async fn serve_stdio(actor: Arc<Actor>, evidence: &Path, reports: &Path) -> Result<()> {
    use rmcp::ServiceExt as _;
    let result = async {
        NativeMcp::new(Arc::clone(&actor), evidence, reports)?
            .serve(rmcp::transport::stdio())
            .await
            .map_err(|_| Error::Unavailable)?
            .waiting()
            .await
            .map_err(|_| Error::Unavailable)?;
        Ok(())
    }
    .await;
    tokio::task::spawn_blocking(move || actor.shutdown())
        .await
        .map_err(|_| Error::CleanupUncertain)??;
    result
}

#[cfg(all(test, unix))]
mod tests;
