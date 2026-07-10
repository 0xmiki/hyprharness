use crate::{
    Harness,
    harness::{ClickResult, CursorObservation, DesktopMetadata, MoveResult, WindowsObservation},
    models::{MouseButton, Point},
};
use base64::{Engine as _, engine::general_purpose::STANDARD};
use rmcp::{
    ServerHandler, ServiceExt,
    handler::server::{router::tool::ToolRouter, wrapper::Parameters},
    model::{CallToolResult, ContentBlock, Implementation, ServerCapabilities, ServerInfo},
    tool, tool_handler, tool_router,
};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::json;

const INSTRUCTIONS: &str = "Hyprharness controls the local Hyprland desktop. Always call observe_desktop before pointer actions. Coordinates are Hyprland global logical coordinates, not screenshot pixels; use the returned monitor geometry and scale. Re-observe after clicks or other visible state changes. Stop when a tool returns SESSION_LOCKED, OUT_OF_BOUNDS, RATE_LIMITED, INPUT_DISABLED, or INPUT_UNAVAILABLE. Never infer coordinates from a stale observation.";

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ObserveParams {
    /// Hyprland monitor name. Omit to capture the focused monitor.
    pub monitor: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct MoveParams {
    /// Global logical X coordinate.
    pub x: i32,
    /// Global logical Y coordinate.
    pub y: i32,
    /// Smooth movement duration in milliseconds, from 0 through 2000.
    #[serde(default)]
    #[schemars(range(min = 0, max = 2000))]
    pub duration_ms: u32,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ClickParams {
    /// Mouse button to click.
    #[serde(default = "default_button")]
    pub button: MouseButton,
    /// Click count, from 1 through 3.
    #[serde(default = "default_count")]
    #[schemars(range(min = 1, max = 3))]
    pub count: u8,
    /// Delay between clicks in milliseconds, from 40 through 1000.
    #[serde(default = "default_interval")]
    #[schemars(range(min = 40, max = 1000))]
    pub interval_ms: u32,
}

fn default_button() -> MouseButton {
    MouseButton::Left
}

fn default_count() -> u8 {
    1
}

fn default_interval() -> u32 {
    120
}

#[derive(Clone, Debug)]
pub struct HyprHarnessMcp {
    harness: Harness,
    tool_router: ToolRouter<Self>,
}

impl HyprHarnessMcp {
    pub fn new(harness: Harness) -> Self {
        Self {
            harness,
            tool_router: Self::tool_router(),
        }
    }
}

#[tool_router]
impl HyprHarnessMcp {
    #[tool(
        description = "Capture the focused (or named) Hyprland monitor as PNG and return exact global logical coordinate metadata. Call this before pointer actions.",
        output_schema = rmcp::handler::server::tool::schema_for_type::<DesktopMetadata>(),
        annotations(
            title = "Observe desktop",
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn observe_desktop(
        &self,
        Parameters(params): Parameters<ObserveParams>,
    ) -> CallToolResult {
        match self.harness.observe_desktop(params.monitor).await {
            Ok(observation) => result_with_image(observation.metadata, observation.png),
            Err(error) => tool_error(error),
        }
    }

    #[tool(
        description = "Get the pointer position in Hyprland global logical coordinates and the containing monitor.",
        output_schema = rmcp::handler::server::tool::schema_for_type::<CursorObservation>(),
        annotations(
            title = "Get cursor",
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn get_cursor(&self) -> CallToolResult {
        structured(self.harness.get_cursor().await)
    }

    #[tool(
        description = "List mapped Hyprland windows with identifiers, titles, geometry, workspace, monitor, visibility, and focus state.",
        output_schema = rmcp::handler::server::tool::schema_for_type::<WindowsObservation>(),
        annotations(
            title = "List windows",
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn list_windows(&self) -> CallToolResult {
        structured(self.harness.list_windows().await)
    }

    #[tool(
        description = "Move the pointer to an enabled monitor position using Hyprland global logical coordinates. Optionally animate the movement.",
        output_schema = rmcp::handler::server::tool::schema_for_type::<MoveResult>(),
        annotations(
            title = "Move pointer",
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn move_pointer(&self, Parameters(params): Parameters<MoveParams>) -> CallToolResult {
        structured(
            self.harness
                .move_pointer(
                    Point {
                        x: params.x,
                        y: params.y,
                    },
                    params.duration_ms,
                )
                .await,
        )
    }

    #[tool(
        description = "Click at the current pointer position using a left, middle, or right button. Observe and move first.",
        output_schema = rmcp::handler::server::tool::schema_for_type::<ClickResult>(),
        annotations(
            title = "Click",
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = false,
            open_world_hint = false
        )
    )]
    async fn click(&self, Parameters(params): Parameters<ClickParams>) -> CallToolResult {
        structured(
            self.harness
                .click(params.button, params.count, params.interval_ms)
                .await,
        )
    }
}

#[tool_handler(router = self.tool_router)]
impl ServerHandler for HyprHarnessMcp {
    fn get_info(&self) -> ServerInfo {
        ServerInfo::new(ServerCapabilities::builder().enable_tools().build())
            .with_server_info(Implementation::new(
                "hyprharness",
                env!("CARGO_PKG_VERSION"),
            ))
            .with_instructions(INSTRUCTIONS)
    }
}

pub async fn serve(harness: Harness) -> anyhow::Result<()> {
    let service = HyprHarnessMcp::new(harness)
        .serve(rmcp::transport::stdio())
        .await
        .map_err(|e| anyhow::anyhow!("MCP initialization failed: {e}"))?;
    service
        .waiting()
        .await
        .map_err(|e| anyhow::anyhow!("MCP server stopped: {e}"))?;
    Ok(())
}

fn structured<T: Serialize>(result: crate::Result<T>) -> CallToolResult {
    match result {
        Ok(value) => match serde_json::to_value(value) {
            Ok(value) => CallToolResult::structured(value),
            Err(error) => CallToolResult::structured_error(json!({
                "ok": false,
                "error": {"code": "INTERNAL_ERROR", "message": error.to_string()}
            })),
        },
        Err(error) => tool_error(error),
    }
}

fn result_with_image(metadata: DesktopMetadata, png: Vec<u8>) -> CallToolResult {
    match serde_json::to_value(metadata) {
        Ok(value) => {
            let mut result = CallToolResult::structured(value);
            result
                .content
                .insert(0, ContentBlock::image(STANDARD.encode(png), "image/png"));
            result
        }
        Err(error) => CallToolResult::structured_error(json!({
            "ok": false,
            "error": {"code": "INTERNAL_ERROR", "message": error.to_string()}
        })),
    }
}

fn tool_error(error: crate::HarnessError) -> CallToolResult {
    CallToolResult::structured_error(error.as_json())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn exports_exact_tool_names_and_annotations() {
        let tools = [
            HyprHarnessMcp::observe_desktop_tool_attr(),
            HyprHarnessMcp::get_cursor_tool_attr(),
            HyprHarnessMcp::list_windows_tool_attr(),
            HyprHarnessMcp::move_pointer_tool_attr(),
            HyprHarnessMcp::click_tool_attr(),
        ];
        let names: Vec<_> = tools.iter().map(|tool| tool.name.as_ref()).collect();
        assert_eq!(
            names,
            [
                "observe_desktop",
                "get_cursor",
                "list_windows",
                "move_pointer",
                "click"
            ]
        );
        assert_eq!(
            tools[0].annotations.as_ref().unwrap().read_only_hint,
            Some(true)
        );
        assert_eq!(
            tools[4].annotations.as_ref().unwrap().destructive_hint,
            Some(true)
        );
        assert!(tools.iter().all(|tool| tool.output_schema.is_some()));
        assert_eq!(
            tools[3].input_schema["properties"]["duration_ms"]["maximum"],
            2000
        );
        assert_eq!(tools[4].input_schema["properties"]["count"]["maximum"], 3);
    }
}
