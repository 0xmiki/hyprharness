use schemars::JsonSchema;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, PartialEq)]
pub struct Point {
    pub x: i32,
    pub y: i32,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
#[serde(rename_all = "camelCase")]
pub struct WorkspaceRef {
    #[serde(default)]
    pub id: i64,
    #[serde(default)]
    pub name: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Monitor {
    pub id: i64,
    pub name: String,
    #[serde(default)]
    pub description: String,
    pub width: i32,
    pub height: i32,
    pub x: i32,
    pub y: i32,
    #[serde(default = "one_f64")]
    pub scale: f64,
    #[serde(default)]
    pub transform: i32,
    #[serde(default)]
    pub focused: bool,
    #[serde(default)]
    pub disabled: bool,
    #[serde(default = "true_value")]
    pub dpms_status: bool,
    #[serde(default)]
    pub active_workspace: WorkspaceRef,
}

fn one_f64() -> f64 {
    1.0
}

fn true_value() -> bool {
    true
}

impl Monitor {
    pub fn logical_width(&self) -> i32 {
        (f64::from(self.width) / self.scale.max(0.01)).round() as i32
    }

    pub fn logical_height(&self) -> i32 {
        (f64::from(self.height) / self.scale.max(0.01)).round() as i32
    }

    pub fn contains(&self, point: &Point) -> bool {
        !self.disabled
            && self.dpms_status
            && point.x >= self.x
            && point.y >= self.y
            && point.x < self.x + self.logical_width()
            && point.y < self.y + self.logical_height()
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "camelCase")]
pub struct Window {
    pub address: String,
    #[serde(default)]
    pub stable_id: String,
    #[serde(default)]
    pub mapped: bool,
    #[serde(default)]
    pub hidden: bool,
    #[serde(default)]
    pub visible: bool,
    #[serde(default)]
    pub accepts_input: bool,
    #[serde(default)]
    pub at: [i32; 2],
    #[serde(default)]
    pub size: [i32; 2],
    #[serde(default)]
    pub workspace: WorkspaceRef,
    #[serde(default)]
    pub monitor: i64,
    #[serde(default)]
    pub class: String,
    #[serde(default)]
    pub title: String,
    #[serde(default)]
    pub pid: i64,
    #[serde(default)]
    pub xwayland: bool,
    #[serde(default)]
    pub fullscreen: i32,
    #[serde(default, rename = "focusHistoryID")]
    pub focus_history_id: i64,
    #[serde(default, skip_deserializing)]
    pub focused: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
pub struct CursorPosition {
    pub x: i32,
    pub y: i32,
}

impl From<CursorPosition> for Point {
    fn from(value: CursorPosition) -> Self {
        Self {
            x: value.x,
            y: value.y,
        }
    }
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema, Default)]
pub struct LockStatus {
    #[serde(default)]
    pub locked: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, JsonSchema)]
#[serde(rename_all = "snake_case")]
pub enum MouseButton {
    Left,
    Middle,
    Right,
}

impl MouseButton {
    pub fn linux_code(&self) -> u32 {
        match self {
            Self::Left => 0x110,
            Self::Right => 0x111,
            Self::Middle => 0x112,
        }
    }

    pub fn as_str(&self) -> &'static str {
        match self {
            Self::Left => "left",
            Self::Middle => "middle",
            Self::Right => "right",
        }
    }
}
