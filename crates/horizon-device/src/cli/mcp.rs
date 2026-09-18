use super::dispatch::{Command, Dispatcher, Response};
use super::permission::ResizePermission;
use crate::{ActRequest, CaptureOptions, ResizeRequest};
use rmcp::{
    RoleServer, ServerHandler, ServiceExt,
    handler::server::wrapper::Parameters,
    model::{
        CallToolResult, ClientNotification, ContentBlock, Implementation, JsonRpcMessage, RequestId,
        ServerCapabilities, ServerInfo,
    },
    service::{RequestContext, RxJsonRpcMessage, TxJsonRpcMessage},
    tool, tool_router,
    transport::{Transport, async_rw::AsyncRwTransport},
};

use std::{
    collections::HashMap,
    io,
    sync::{Arc, Mutex},
};

#[derive(Default)]
struct DeliveryState {
    response: Option<Response>,
    cancelled: bool,
}
type Delivery = Arc<Mutex<DeliveryState>>;
#[derive(Clone, Default)]
struct Deliveries(Arc<Mutex<HashMap<RequestId, Delivery>>>);
impl Deliveries {
    fn cancel(&self, id: &RequestId) {
        if let Some(delivery) = self
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .remove(id)
        {
            let mut delivery = delivery.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            delivery.cancelled = true;
            delivery.response.take();
        }
    }
    fn clear(&self) {
        for (_, delivery) in self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner).drain() {
            let mut delivery = delivery.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
            delivery.cancelled = true;
            delivery.response.take();
        }
    }
}
struct Sending {
    id: RequestId,
    delivery: Delivery,
    deliveries: Deliveries,
    response: Option<Response>,
}
impl Drop for Sending {
    fn drop(&mut self) {
        let mut pending = self
            .deliveries
            .0
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner);
        if pending
            .get(&self.id)
            .is_some_and(|entry| Arc::ptr_eq(entry, &self.delivery))
        {
            pending.remove(&self.id);
        }
    }
}
struct DeliveredTransport<T> {
    inner: T,
    deliveries: Deliveries,
}
impl<T> Drop for DeliveredTransport<T> {
    fn drop(&mut self) {
        self.deliveries.clear();
    }
}
impl<T: Transport<RoleServer, Error = io::Error>> Transport<RoleServer> for DeliveredTransport<T> {
    type Error = io::Error;
    fn send(&mut self, message: TxJsonRpcMessage<RoleServer>) -> impl Future<Output = io::Result<()>> + Send + 'static {
        let sending = if let JsonRpcMessage::Response(response) = &message {
            self.deliveries
                .0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .get(&response.id)
                .cloned()
                .map(|delivery| {
                    let response_guard = delivery
                        .lock()
                        .unwrap_or_else(std::sync::PoisonError::into_inner)
                        .response
                        .take();
                    Sending {
                        id: response.id.clone(),
                        delivery,
                        deliveries: self.deliveries.clone(),
                        response: response_guard,
                    }
                })
        } else {
            None
        };
        let send = self.inner.send(message);
        async move {
            send.await?;
            if let Some(mut sending) = sending {
                // AsyncRwTransport completes serialization and flush before success.
                let state = sending
                    .delivery
                    .lock()
                    .unwrap_or_else(std::sync::PoisonError::into_inner);
                if !state.cancelled
                    && let Some(response) = &mut sending.response
                {
                    response.complete_observation().map_err(io::Error::other)?;
                }
            }
            Ok(())
        }
    }
    async fn receive(&mut self) -> Option<RxJsonRpcMessage<RoleServer>> {
        let message = self.inner.receive().await;
        if let Some(JsonRpcMessage::Notification(notification)) = &message
            && let ClientNotification::CancelledNotification(cancelled) = &notification.notification
            && let Some(id) = &cancelled.params.request_id
        {
            self.deliveries.cancel(id);
        }
        message
    }
    async fn close(&mut self) -> io::Result<()> {
        self.deliveries.clear();
        self.inner.close().await
    }
}

#[derive(Clone)]
pub struct Server {
    dispatcher: Dispatcher,
    deliveries: Deliveries,
}
impl Server {
    pub fn new(dispatcher: Dispatcher) -> Self {
        Self {
            dispatcher,
            deliveries: Deliveries::default(),
        }
    }
    async fn execute(&self, command: Command, observation: Option<RequestContext<RoleServer>>) -> CallToolResult {
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
        if success && let Some(context) = observation {
            let mut pending = self
                .deliveries
                .0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner);
            if !context.ct.is_cancelled() {
                pending.insert(
                    context.id,
                    Arc::new(Mutex::new(DeliveryState {
                        response: Some(delivered),
                        cancelled: false,
                    })),
                );
            }
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
        self.execute(Command::Doctor, None).await
    }
    #[tool(
        name = "device_screenshot",
        description = "Observe the configured device. Optional crop, output dimensions and PNG/JPEG quality. Returns the original geometry, source_region and image_dimensions for mapping image pixels to surface coordinates."
    )]
    async fn screenshot(
        &self,
        Parameters(options): Parameters<CaptureOptions>,
        context: RequestContext<RoleServer>,
    ) -> CallToolResult {
        self.execute(Command::Screenshot(options), Some(context)).await
    }
    #[tool(
        name = "device_act",
        description = "Send one bounded input action to the configured device using fresh screenshot geometry. Success means dispatched, not verified app outcome. Do not retry an indeterminate action without observing. Touch and accessibility are unsupported."
    )]
    async fn act(&self, Parameters(request): Parameters<ActRequest>) -> CallToolResult {
        self.execute(Command::Act(request), None).await
    }
    #[tool(
        name = "device_set_resize_enabled",
        description = "Enable or disable desktop resizing for this configured target at runtime. Persists permission for CLI and MCP without restarting. Preserves size limits, endpoint and uncertainty journals; does not resize or cancel a dispatched operation. Requires a writable target configuration."
    )]
    async fn set_resize_enabled(&self, Parameters(permission): Parameters<ResizePermission>) -> CallToolResult {
        self.execute(Command::SetResizeEnabled(permission), None).await
    }
    #[tool(
        name = "device_resize",
        description = "Explicitly resize the actual configured desktop within owner-enabled limits. Check device_doctor first. Returns requested and confirmed dimensions; capture a fresh screenshot before input. Never retry an uncertain resize without owner reconciliation. Screenshot output dimensions and viewer Fit do not resize the desktop. A bounded resize already in progress completes even if this request is cancelled."
    )]
    async fn resize(&self, Parameters(request): Parameters<ResizeRequest>) -> CallToolResult {
        self.execute(Command::Resize(request), None).await
    }
}
impl ServerHandler for Server {
    fn on_cancelled(
        &self,
        notification: rmcp::model::CancelledNotificationParam,
        _: rmcp::service::NotificationContext<RoleServer>,
    ) -> impl Future<Output = ()> {
        if let Some(id) = notification.request_id {
            self.deliveries.cancel(&id);
        }
        std::future::ready(())
    }
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
        let server = Server::new(dispatcher);
        let transport = DeliveredTransport {
            inner: AsyncRwTransport::new_server(tokio::io::stdin(), tokio::io::stdout()),
            deliveries: server.deliveries.clone(),
        };
        let service = server.serve(transport).await.map_err(|error| error.to_string())?;
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

#[cfg(test)]
mod tests {
    use super::*;
    use rmcp::model::ServerResult;
    use std::{
        fs::File,
        pin::Pin,
        task::{Context, Poll},
    };
    use tokio::io::{AsyncWrite, Empty};

    #[derive(Clone, Copy)]
    enum Flush {
        Success,
        Failed,
        Pending,
    }
    struct Writer(Arc<Mutex<Flush>>);
    impl AsyncWrite for Writer {
        fn poll_write(self: Pin<&mut Self>, _: &mut Context<'_>, bytes: &[u8]) -> Poll<io::Result<usize>> {
            Poll::Ready(Ok(bytes.len()))
        }
        fn poll_flush(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
            match *self.0.lock().unwrap_or_else(std::sync::PoisonError::into_inner) {
                Flush::Success => Poll::Ready(Ok(())),
                Flush::Failed => Poll::Ready(Err(io::Error::other("closed output"))),
                Flush::Pending => Poll::Pending,
            }
        }
        fn poll_shutdown(self: Pin<&mut Self>, _: &mut Context<'_>) -> Poll<io::Result<()>> {
            Poll::Ready(Ok(()))
        }
    }
    struct Fixture {
        directory: tempfile::TempDir,
        flush: Arc<Mutex<Flush>>,
        transport: DeliveredTransport<AsyncRwTransport<RoleServer, Empty, Writer>>,
    }
    impl Fixture {
        fn new(flush: Flush) -> io::Result<Self> {
            let directory = tempfile::tempdir()?;
            let marker = directory.path().join("target.resize-observe");
            std::fs::write(&marker, b"")?;
            let lock = File::create(directory.path().join("target.lock"))?;
            lock.lock()?;
            let deliveries = Deliveries::default();
            deliveries
                .0
                .lock()
                .unwrap_or_else(std::sync::PoisonError::into_inner)
                .insert(
                    RequestId::Number(1),
                    Arc::new(Mutex::new(DeliveryState {
                        response: Some(Response::new(serde_json::json!({}), Some(lock), Some(marker))),
                        cancelled: false,
                    })),
                );
            let flush = Arc::new(Mutex::new(flush));
            Ok(Self {
                directory,
                flush: Arc::clone(&flush),
                transport: DeliveredTransport {
                    inner: AsyncRwTransport::new_server(tokio::io::empty(), Writer(flush)),
                    deliveries,
                },
            })
        }
        fn gated(&self) -> bool {
            self.directory.path().join("target.resize-observe").exists()
        }
        fn unlocked(&self) -> Result<(), Box<dyn std::error::Error>> {
            File::open(self.directory.path().join("target.lock"))?.try_lock()?;
            Ok(())
        }
    }
    fn message() -> TxJsonRpcMessage<RoleServer> {
        JsonRpcMessage::response(
            ServerResult::CallToolResult(CallToolResult::success(vec![])),
            RequestId::Number(1),
        )
    }

    #[tokio::test]
    async fn only_successful_transport_flush_releases_observation() -> Result<(), Box<dyn std::error::Error>> {
        for (flush, success) in [(Flush::Success, true), (Flush::Failed, false)] {
            let mut fixture = Fixture::new(flush)?;
            assert_eq!(fixture.transport.send(message()).await.is_ok(), success);
            assert_eq!(fixture.gated(), !success);
            fixture.unlocked()?;
        }
        Ok(())
    }
    #[test]
    fn dropped_send_keeps_observation_and_releases_lock() -> Result<(), Box<dyn std::error::Error>> {
        let mut fixture = Fixture::new(Flush::Pending)?;
        let mut send = Box::pin(fixture.transport.send(message()));
        assert!(
            send.as_mut()
                .poll(&mut Context::from_waker(std::task::Waker::noop()))
                .is_pending()
        );
        assert!(fixture.gated());
        assert!(matches!(
            File::open(fixture.directory.path().join("target.lock"))?.try_lock(),
            Err(std::fs::TryLockError::WouldBlock)
        ));
        drop(send);
        fixture.unlocked()?;
        assert!(fixture.gated());
        Ok(())
    }
    #[tokio::test]
    async fn cancelled_queued_or_sending_response_keeps_observation() -> Result<(), Box<dyn std::error::Error>> {
        for queued in [true, false] {
            let mut fixture = Fixture::new(Flush::Pending)?;
            if queued {
                fixture.transport.deliveries.cancel(&RequestId::Number(1));
            }
            let mut send = Box::pin(fixture.transport.send(message()));
            assert!(
                send.as_mut()
                    .poll(&mut Context::from_waker(std::task::Waker::noop()))
                    .is_pending()
            );
            fixture.transport.deliveries.cancel(&RequestId::Number(1));
            *fixture.flush.lock().unwrap_or_else(std::sync::PoisonError::into_inner) = Flush::Success;
            send.await?;
            fixture.unlocked()?;
            assert!(fixture.gated());
        }
        Ok(())
    }
    #[test]
    fn closing_transport_preserves_undelivered_observation() -> Result<(), Box<dyn std::error::Error>> {
        let fixture = Fixture::new(Flush::Success)?;
        drop(fixture.transport);
        assert!(fixture.directory.path().join("target.resize-observe").exists());
        File::open(fixture.directory.path().join("target.lock"))?.try_lock()?;
        Ok(())
    }
}
