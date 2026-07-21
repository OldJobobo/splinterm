use std::{
    collections::HashSet,
    sync::{
        Arc,
        atomic::{AtomicBool, AtomicU64, Ordering},
    },
};

use rmcp::{
    ErrorData, Json, RoleServer, ServerHandler,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{
        CallToolResult, Implementation, InitializeRequestParams, InitializeResult,
        ListResourceTemplatesResult, ListResourcesResult, PaginatedRequestParams, ProtocolVersion,
        ReadResourceRequestParams, ReadResourceResult, Resource, ResourceContents,
        ResourceTemplate, ResourceUpdatedNotificationParam, ServerCapabilities,
        SubscribeRequestParams, UnsubscribeRequestParams,
    },
    schemars,
    service::{NotificationContext, RequestContext},
    tool, tool_handler, tool_router,
};
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::sync::Mutex;

const TOPOLOGY_URI: &str = "splinterm://topology";
const TERMINAL_TEMPLATE: &str = "splinterm://splints/{splint_id}/terminal";
const CONTROL_TEMPLATE: &str = "splinterm://splints/{splint_id}/control";

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct EchoRequest {
    message: String,
}

#[derive(Debug, Deserialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct EmptyRequest {}

#[derive(Debug, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct EchoResponse {
    schema: &'static str,
    tool: &'static str,
    ok: bool,
    data: EchoData,
}

#[derive(Debug, Serialize, schemars::JsonSchema)]
#[serde(deny_unknown_fields)]
struct EchoData {
    message: String,
    revision: u64,
}

#[derive(Debug, Default)]
struct State {
    initialized: AtomicBool,
    revision: AtomicU64,
    cancellation_count: AtomicU64,
    subscriptions: Mutex<HashSet<String>>,
}

/// A deterministic, daemon-free server used only to validate the selected SDK.
#[derive(Debug, Clone)]
pub struct SpikeServer {
    tool_router: ToolRouter<Self>,
    state: Arc<State>,
}

impl SpikeServer {
    #[must_use]
    pub fn new() -> Self {
        Self {
            tool_router: Self::tool_router(),
            state: Arc::new(State::default()),
        }
    }

    fn require_initialized(&self) -> Result<(), ErrorData> {
        if self.state.initialized.load(Ordering::Acquire) {
            Ok(())
        } else {
            Err(ErrorData::invalid_request(
                "notifications/initialized is required",
                None,
            ))
        }
    }

    async fn publish_if_subscribed(&self, context: &RequestContext<RoleServer>, uri: &str) {
        if self.state.subscriptions.lock().await.contains(uri) {
            let _ = context
                .peer
                .notify_resource_updated(ResourceUpdatedNotificationParam::new(uri))
                .await;
        }
    }

    fn recognizes_resource(uri: &str) -> bool {
        if uri == TOPOLOGY_URI {
            return true;
        }
        let Some(remainder) = uri.strip_prefix("splinterm://splints/") else {
            return false;
        };
        let Some((splint_id, kind)) = remainder.split_once('/') else {
            return false;
        };
        !splint_id.is_empty() && !splint_id.contains('/') && matches!(kind, "terminal" | "control")
    }
}

impl Default for SpikeServer {
    fn default() -> Self {
        Self::new()
    }
}

#[tool_router]
impl SpikeServer {
    /// Echoes bounded metadata to prove closed inputs and structured output.
    #[tool(
        name = "splinterm.spike.echo",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        ),
        execution(task_support = "forbidden")
    )]
    async fn echo(
        &self,
        Parameters(request): Parameters<EchoRequest>,
        context: RequestContext<RoleServer>,
    ) -> Result<Json<EchoResponse>, ErrorData> {
        self.require_initialized()?;
        let revision = self.state.revision.fetch_add(1, Ordering::AcqRel) + 1;
        self.publish_if_subscribed(&context, TOPOLOGY_URI).await;
        Ok(Json(EchoResponse {
            schema: "splinterm.mcp.spike.v1",
            tool: "splinterm.spike.echo",
            ok: true,
            data: EchoData {
                message: request.message,
                revision,
            },
        }))
    }

    /// Returns a caller-visible structured tool error without a daemon.
    #[tool(
        name = "splinterm.spike.fail",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        ),
        execution(task_support = "forbidden")
    )]
    async fn fail(
        &self,
        Parameters(EmptyRequest {}): Parameters<EmptyRequest>,
    ) -> Result<CallToolResult, ErrorData> {
        self.require_initialized()?;
        Ok(CallToolResult::structured_error(json!({
            "schema": "splinterm.mcp.spike.v1",
            "tool": "splinterm.spike.fail",
            "ok": false,
            "error": {
                "code": "SPIKE_FAILURE",
                "message": "requested spike failure",
                "retryable": false
            }
        })))
    }

    /// Waits until the SDK exposes cancellation through the request context.
    #[tool(
        name = "splinterm.spike.wait_for_cancel",
        annotations(
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        ),
        execution(task_support = "forbidden")
    )]
    async fn wait_for_cancel(
        &self,
        Parameters(EmptyRequest {}): Parameters<EmptyRequest>,
        context: RequestContext<RoleServer>,
    ) -> Result<CallToolResult, ErrorData> {
        self.require_initialized()?;
        context.ct.cancelled().await;
        self.state.cancellation_count.fetch_add(1, Ordering::AcqRel);
        self.publish_if_subscribed(&context, TOPOLOGY_URI).await;
        Ok(CallToolResult::structured(json!({
            "schema": "splinterm.mcp.spike.v1",
            "tool": "splinterm.spike.wait_for_cancel",
            "ok": true
        })))
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for SpikeServer {
    async fn initialize(
        &self,
        request: InitializeRequestParams,
        context: RequestContext<RoleServer>,
    ) -> Result<InitializeResult, ErrorData> {
        if request.protocol_version != ProtocolVersion::V_2025_11_25 {
            return Err(ErrorData::invalid_request(
                "only MCP protocol version 2025-11-25 is supported",
                None,
            ));
        }
        context.peer.set_peer_info(request);
        Ok(self.get_info())
    }

    fn get_info(&self) -> InitializeResult {
        InitializeResult::new(
            ServerCapabilities::builder()
                .enable_resources()
                .enable_resources_subscribe()
                .enable_tools()
                .build(),
        )
        .with_protocol_version(ProtocolVersion::V_2025_11_25)
        .with_server_info(Implementation::new("splinterm-mcp-sdk-spike", "0.1.0"))
    }

    async fn on_initialized(&self, _context: NotificationContext<RoleServer>) {
        self.state.initialized.store(true, Ordering::Release);
    }

    async fn list_resources(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourcesResult, ErrorData> {
        self.require_initialized()?;
        Ok(ListResourcesResult::with_all_items(vec![
            Resource::new(TOPOLOGY_URI, "splinterm topology spike")
                .with_description("Deterministic SDK-spike state")
                .with_mime_type("application/json"),
        ]))
    }

    async fn list_resource_templates(
        &self,
        _request: Option<PaginatedRequestParams>,
        _context: RequestContext<RoleServer>,
    ) -> Result<ListResourceTemplatesResult, ErrorData> {
        self.require_initialized()?;
        Ok(ListResourceTemplatesResult::with_all_items(vec![
            ResourceTemplate::new(TERMINAL_TEMPLATE, "splinterm terminal spike")
                .with_mime_type("application/json"),
            ResourceTemplate::new(CONTROL_TEMPLATE, "splinterm control spike")
                .with_mime_type("application/json"),
        ]))
    }

    async fn read_resource(
        &self,
        request: ReadResourceRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<ReadResourceResult, ErrorData> {
        self.require_initialized()?;
        if !Self::recognizes_resource(&request.uri) {
            return Err(ErrorData::resource_not_found(
                "unknown spike resource",
                None,
            ));
        }
        let value = json!({
            "schema": "splinterm.mcp.spike.v1",
            "uri": request.uri,
            "revision": self.state.revision.load(Ordering::Acquire),
            "cancellationsObserved": self.state.cancellation_count.load(Ordering::Acquire)
        });
        Ok(ReadResourceResult::new(vec![
            ResourceContents::text(value.to_string(), request.uri)
                .with_mime_type("application/json"),
        ]))
    }

    async fn subscribe(
        &self,
        request: SubscribeRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<(), ErrorData> {
        self.require_initialized()?;
        if !Self::recognizes_resource(&request.uri) {
            return Err(ErrorData::resource_not_found(
                "unknown spike resource",
                None,
            ));
        }
        self.state.subscriptions.lock().await.insert(request.uri);
        Ok(())
    }

    async fn unsubscribe(
        &self,
        request: UnsubscribeRequestParams,
        _context: RequestContext<RoleServer>,
    ) -> Result<(), ErrorData> {
        self.require_initialized()?;
        if !self.state.subscriptions.lock().await.remove(&request.uri) {
            return Err(ErrorData::resource_not_found(
                "spike resource is not subscribed",
                None,
            ));
        }
        Ok(())
    }
}
