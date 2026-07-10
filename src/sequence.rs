use crate::models::{KeyModifier, MotionProfile, MouseButton, ScrollDirection};
use schemars::JsonSchema;
use serde::{Deserialize, Serialize};
use serde_json::Value;

pub const MAX_SEQUENCE_STEPS: usize = 32;
pub const MAX_SEQUENCE_DURATION_MS: u64 = 45_000;
pub const MAX_SEQUENCE_WAIT_MS: u32 = 10_000;

#[derive(Debug, Clone, Default, Serialize, Deserialize, JsonSchema)]
pub struct SequenceGuard {
    /// Stable ID/address that must be focused immediately before this step.
    pub focused_window_id: Option<String>,
    /// Numeric workspace ID that must be active on the focused monitor.
    #[schemars(range(min = 1))]
    pub workspace_id: Option<i32>,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct SequenceStep {
    #[serde(flatten)]
    pub action: SequenceAction,
    /// Optional live preconditions checked immediately before this step.
    #[serde(default)]
    pub guard: SequenceGuard,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(tag = "action", rename_all = "snake_case")]
pub enum SequenceAction {
    MovePointer {
        x: i32,
        y: i32,
        #[schemars(range(min = 0, max = 3000))]
        duration_ms: Option<u32>,
        #[serde(default)]
        motion: MotionProfile,
    },
    Click {
        #[serde(default = "default_button")]
        button: MouseButton,
        #[serde(default = "default_count")]
        #[schemars(range(min = 1, max = 3))]
        count: u8,
        #[serde(default = "default_click_interval")]
        #[schemars(range(min = 40, max = 1000))]
        interval_ms: u32,
    },
    FocusWindow {
        window_id: String,
    },
    Scroll {
        direction: ScrollDirection,
        #[serde(default = "default_scroll_amount")]
        #[schemars(range(min = 1, max = 20))]
        amount: u8,
    },
    PressKey {
        key: String,
        #[serde(default)]
        modifiers: Vec<KeyModifier>,
        #[serde(default = "default_count")]
        #[schemars(range(min = 1, max = 20))]
        repeat: u8,
    },
    TypeText {
        text: String,
        #[serde(default = "default_text_interval")]
        #[schemars(range(min = 0, max = 50))]
        interval_ms: u32,
    },
    Wait {
        #[schemars(range(min = 0, max = 10000))]
        duration_ms: u32,
    },
    SwitchWorkspace {
        #[schemars(range(min = 1))]
        workspace_id: i32,
    },
}

impl SequenceAction {
    pub fn name(&self) -> &'static str {
        match self {
            Self::MovePointer { .. } => "move_pointer",
            Self::Click { .. } => "click",
            Self::FocusWindow { .. } => "focus_window",
            Self::Scroll { .. } => "scroll",
            Self::PressKey { .. } => "press_key",
            Self::TypeText { .. } => "type_text",
            Self::Wait { .. } => "wait",
            Self::SwitchWorkspace { .. } => "switch_workspace",
        }
    }

    pub fn is_input(&self) -> bool {
        !matches!(self, Self::Wait { .. })
    }

    pub fn planned_duration_ms(&self) -> u64 {
        match self {
            Self::MovePointer {
                duration_ms,
                motion,
                ..
            } => {
                if *motion == MotionProfile::Instant {
                    0
                } else {
                    u64::from(duration_ms.unwrap_or(1_200))
                }
            }
            Self::Click {
                count, interval_ms, ..
            } => u64::from(count.saturating_sub(1)) * u64::from(*interval_ms),
            Self::TypeText {
                text, interval_ms, ..
            } => (text.chars().count() as u64).saturating_mul(u64::from(*interval_ms)),
            Self::Wait { duration_ms } => u64::from(*duration_ms),
            _ => 0,
        }
    }
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct SequenceStepError {
    pub code: String,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub details: Option<Value>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct SequenceStepResult {
    pub index: usize,
    pub action: String,
    pub started_offset_ms: u128,
    pub elapsed_ms: u128,
    pub ok: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error: Option<SequenceStepError>,
}

fn default_button() -> MouseButton {
    MouseButton::Left
}

fn default_count() -> u8 {
    1
}

fn default_click_interval() -> u32 {
    120
}

fn default_scroll_amount() -> u8 {
    3
}

fn default_text_interval() -> u32 {
    5
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn deserializes_a_typed_sequence_step_with_defaults() {
        let step: SequenceStep = serde_json::from_value(serde_json::json!({
            "action": "move_pointer",
            "x": 400,
            "y": 300
        }))
        .unwrap();
        assert!(matches!(
            step.action,
            SequenceAction::MovePointer {
                motion: MotionProfile::Natural,
                duration_ms: None,
                ..
            }
        ));
    }

    #[test]
    fn automatic_motion_uses_conservative_planned_duration() {
        let action = SequenceAction::MovePointer {
            x: 1,
            y: 2,
            duration_ms: None,
            motion: MotionProfile::Natural,
        };
        assert_eq!(action.planned_duration_ms(), 1_200);
    }
}
