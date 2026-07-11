use crate::{
    Harness,
    harness::{
        ClickResult, CursorObservation, DesktopMetadata, FocusResult, MoveResult,
        PointAndClickResult, PressKeyResult, ScrollResult, SequenceExecution, SequenceRun,
        TypeTextResult, WaitResult, WindowsObservation, WorkspaceResult,
    },
    models::{KeyModifier, MotionProfile, MouseButton, Point, ScrollDirection},
    sequence::{DEFAULT_SETTLE_MS, SequenceGuard, SequenceStep},
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

const INSTRUCTIONS: &str = "Hyprharness controls the local Hyprland desktop. Observe before acting and re-observe after visible changes. Coordinates are Hyprland global logical coordinates, not image pixels. Use stableId from list_windows for focus_window and input guards. Pointer movement is natural and distance-timed by default; use point_and_click for demo-friendly deceleration, a visible settling pause, and an atomic click. Use run_sequence only for deterministic choreography that does not require reasoning between steps; use individual calls whenever an intermediate screen must be inspected. Move the pointer over a scroll target before scrolling. Stop on safety errors and never infer coordinates from a stale observation.";

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
    /// Movement duration in milliseconds, from 0 through 3000. Omit for distance-aware timing; 0 moves instantly.
    #[schemars(range(min = 0, max = 3000))]
    pub duration_ms: Option<u32>,
    /// Path style. Natural is subtly curved, smooth is straight and eased, and instant teleports.
    #[serde(default)]
    pub motion: MotionProfile,
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

#[derive(Debug, Deserialize, JsonSchema)]
pub struct PointAndClickParams {
    /// Global logical X coordinate to approach and click.
    pub x: i32,
    /// Global logical Y coordinate to approach and click.
    pub y: i32,
    /// Movement duration in milliseconds, from 0 through 3000. Omit for distance-aware timing.
    #[schemars(range(min = 0, max = 3000))]
    pub duration_ms: Option<u32>,
    /// Natural is subtly curved and decelerates at the target; smooth is straight and eased.
    #[serde(default)]
    pub motion: MotionProfile,
    /// Visible pause after movement finishes and before the click, from 0 through 2000 ms.
    #[serde(default = "default_settle_ms")]
    #[schemars(range(min = 0, max = 2000))]
    pub settle_ms: u32,
    /// Mouse button to click after settling.
    #[serde(default = "default_button")]
    pub button: MouseButton,
    /// Click count, from 1 through 3.
    #[serde(default = "default_count")]
    #[schemars(range(min = 1, max = 3))]
    pub count: u8,
    /// Delay between multiple clicks in milliseconds, from 40 through 1000.
    #[serde(default = "default_interval")]
    #[schemars(range(min = 40, max = 1000))]
    pub interval_ms: u32,
    /// Optional focus/workspace conditions checked before moving and again before clicking.
    #[serde(default)]
    pub guard: SequenceGuard,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct FocusWindowParams {
    /// Exact stableId or address returned by list_windows.
    pub window_id: String,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct ScrollParams {
    /// Direction to scroll at the current pointer position.
    pub direction: ScrollDirection,
    /// Number of discrete wheel steps, from 1 through 20.
    #[serde(default = "default_scroll_amount")]
    #[schemars(range(min = 1, max = 20))]
    pub amount: u8,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct PressKeyParams {
    /// Letter, digit, F1-F12, or documented named key such as enter, tab, escape, left, or page_down.
    pub key: String,
    /// Modifiers held while pressing the key.
    #[serde(default)]
    pub modifiers: Vec<KeyModifier>,
    /// Number of key presses, from 1 through 20.
    #[serde(default = "default_count")]
    #[schemars(range(min = 1, max = 20))]
    pub repeat: u8,
    /// Optional stableId/address that must currently be focused, guarding against stale focus.
    pub expected_window_id: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct TypeTextParams {
    /// UTF-8 text to type. The text itself is never written to the audit log.
    pub text: String,
    /// Delay between characters in milliseconds, from 0 through 50.
    #[serde(default = "default_text_interval")]
    #[schemars(range(min = 0, max = 50))]
    pub interval_ms: u32,
    /// Optional stableId/address that must currently be focused, guarding against stale focus.
    pub expected_window_id: Option<String>,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct WaitParams {
    /// Time to wait in milliseconds, from 0 through 30000.
    #[schemars(range(min = 0, max = 30000))]
    pub duration_ms: u32,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct SwitchWorkspaceParams {
    /// Positive numeric Hyprland workspace ID to focus.
    #[schemars(range(min = 1))]
    pub workspace_id: i32,
}

#[derive(Debug, Deserialize, JsonSchema)]
pub struct RunSequenceParams {
    /// Ordered, typed actions. The server validates the complete plan before starting.
    #[schemars(length(min = 1, max = 32))]
    pub steps: Vec<SequenceStep>,
    /// Capture and return the final desktop after all steps succeed.
    #[serde(default)]
    pub observe_at_end: bool,
    /// Monitor to capture at the end. Requires observe_at_end; omit for the focused monitor.
    pub final_monitor: Option<String>,
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

fn default_settle_ms() -> u32 {
    DEFAULT_SETTLE_MS
}

fn default_scroll_amount() -> u8 {
    3
}

fn default_text_interval() -> u32 {
    5
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
        description = "Move the pointer to an enabled monitor position in global logical coordinates. Defaults to a natural, distance-timed, subtly curved motion suitable for demos; use smooth for a straight eased path or instant for a teleport.",
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
                    params.motion,
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

    #[tool(
        description = "Move naturally to a validated coordinate, decelerate to a full stop, hold visibly at the target (300 ms by default), verify the pointer stayed there, recheck optional focus/workspace guards, then click without allowing other input to interleave. Preferred for recorded demos.",
        output_schema = rmcp::handler::server::tool::schema_for_type::<PointAndClickResult>(),
        annotations(
            title = "Point and click",
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = false,
            open_world_hint = false
        )
    )]
    async fn point_and_click(
        &self,
        Parameters(params): Parameters<PointAndClickParams>,
    ) -> CallToolResult {
        structured(
            self.harness
                .point_and_click(
                    Point {
                        x: params.x,
                        y: params.y,
                    },
                    params.duration_ms,
                    params.motion,
                    params.settle_ms,
                    params.button,
                    params.count,
                    params.interval_ms,
                    params.guard,
                )
                .await,
        )
    }

    #[tool(
        description = "Focus a mapped Hyprland window by the exact stableId or address returned from list_windows.",
        output_schema = rmcp::handler::server::tool::schema_for_type::<FocusResult>(),
        annotations(
            title = "Focus window",
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn focus_window(
        &self,
        Parameters(params): Parameters<FocusWindowParams>,
    ) -> CallToolResult {
        structured(self.harness.focus_window(params.window_id).await)
    }

    #[tool(
        description = "Scroll at the current pointer position using discrete wheel steps. Move the pointer over the intended scrollable surface first.",
        output_schema = rmcp::handler::server::tool::schema_for_type::<ScrollResult>(),
        annotations(
            title = "Scroll",
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = false,
            open_world_hint = false
        )
    )]
    async fn scroll(&self, Parameters(params): Parameters<ScrollParams>) -> CallToolResult {
        structured(self.harness.scroll(params.direction, params.amount).await)
    }

    #[tool(
        description = "Press a validated key or shortcut in the focused window. Supply expected_window_id for race-safe input.",
        output_schema = rmcp::handler::server::tool::schema_for_type::<PressKeyResult>(),
        annotations(
            title = "Press key",
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = false,
            open_world_hint = false
        )
    )]
    async fn press_key(&self, Parameters(params): Parameters<PressKeyParams>) -> CallToolResult {
        structured(
            self.harness
                .press_key(
                    params.key,
                    params.modifiers,
                    params.repeat,
                    params.expected_window_id,
                )
                .await,
        )
    }

    #[tool(
        description = "Type validated UTF-8 text into the focused window through Wayland. Supply expected_window_id for race-safe input; text content is redacted from audit logs.",
        output_schema = rmcp::handler::server::tool::schema_for_type::<TypeTextResult>(),
        annotations(
            title = "Type text",
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = false,
            open_world_hint = false
        )
    )]
    async fn type_text(&self, Parameters(params): Parameters<TypeTextParams>) -> CallToolResult {
        structured(
            self.harness
                .type_text(params.text, params.interval_ms, params.expected_window_id)
                .await,
        )
    }

    #[tool(
        description = "Wait for a bounded duration so an application can finish navigation, animation, or asynchronous work before observing again.",
        output_schema = rmcp::handler::server::tool::schema_for_type::<WaitResult>(),
        annotations(
            title = "Wait",
            read_only_hint = true,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn wait(&self, Parameters(params): Parameters<WaitParams>) -> CallToolResult {
        structured(self.harness.wait(params.duration_ms).await)
    }

    #[tool(
        description = "Focus a positive numeric Hyprland workspace through compositor IPC and verify the resulting focused monitor/workspace.",
        output_schema = rmcp::handler::server::tool::schema_for_type::<WorkspaceResult>(),
        annotations(
            title = "Switch workspace",
            read_only_hint = false,
            destructive_hint = false,
            idempotent_hint = true,
            open_world_hint = false
        )
    )]
    async fn switch_workspace(
        &self,
        Parameters(params): Parameters<SwitchWorkspaceParams>,
    ) -> CallToolResult {
        structured(self.harness.switch_workspace(params.workspace_id).await)
    }

    #[tool(
        description = "Execute 1-32 typed desktop actions serially as one fail-fast choreography. Plans are limited to 45 seconds, waits to 10 seconds each, input cannot interleave, per-step guards are checked live, and an optional final screenshot can be returned. Use only when no intermediate visual reasoning is required.",
        output_schema = rmcp::handler::server::tool::schema_for_type::<SequenceExecution>(),
        annotations(
            title = "Run action sequence",
            read_only_hint = false,
            destructive_hint = true,
            idempotent_hint = false,
            open_world_hint = false
        )
    )]
    async fn run_sequence(
        &self,
        Parameters(params): Parameters<RunSequenceParams>,
    ) -> CallToolResult {
        match self
            .harness
            .run_sequence(params.steps, params.observe_at_end, params.final_monitor)
            .await
        {
            Ok(run) => sequence_result(run),
            Err(error) => tool_error(error),
        }
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

fn sequence_result(run: SequenceRun) -> CallToolResult {
    match serde_json::to_value(run.execution) {
        Ok(value) => {
            let mut result = CallToolResult::structured(value);
            if let Some(png) = run.png {
                result
                    .content
                    .insert(0, ContentBlock::image(STANDARD.encode(png), "image/png"));
            }
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
            HyprHarnessMcp::point_and_click_tool_attr(),
            HyprHarnessMcp::focus_window_tool_attr(),
            HyprHarnessMcp::scroll_tool_attr(),
            HyprHarnessMcp::press_key_tool_attr(),
            HyprHarnessMcp::type_text_tool_attr(),
            HyprHarnessMcp::wait_tool_attr(),
            HyprHarnessMcp::switch_workspace_tool_attr(),
            HyprHarnessMcp::run_sequence_tool_attr(),
        ];
        let names: Vec<_> = tools.iter().map(|tool| tool.name.as_ref()).collect();
        assert_eq!(
            names,
            [
                "observe_desktop",
                "get_cursor",
                "list_windows",
                "move_pointer",
                "click",
                "point_and_click",
                "focus_window",
                "scroll",
                "press_key",
                "type_text",
                "wait",
                "switch_workspace",
                "run_sequence"
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
            3000
        );
        assert_eq!(
            tools[3].input_schema["properties"]["motion"]["default"],
            "natural"
        );
        assert_eq!(tools[4].input_schema["properties"]["count"]["maximum"], 3);
        assert_eq!(
            tools[5].input_schema["properties"]["settle_ms"]["default"],
            DEFAULT_SETTLE_MS
        );
        assert_eq!(
            tools[5].input_schema["properties"]["settle_ms"]["maximum"],
            2_000
        );
        assert_eq!(tools[7].input_schema["properties"]["amount"]["maximum"], 20);
        assert_eq!(
            tools[10].input_schema["properties"]["duration_ms"]["maximum"],
            30_000
        );
        assert_eq!(
            tools[12].input_schema["properties"]["steps"]["maxItems"],
            32
        );
        assert_eq!(
            tools[12].annotations.as_ref().unwrap().destructive_hint,
            Some(true)
        );
    }
}
