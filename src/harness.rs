use crate::{
    audit::{AuditLogger, AuditRecord},
    capture::{Capture, GrimCapture, ScreenshotApi},
    error::{HarnessError, Result},
    input::{PointerApi, VirtualPointerActor},
    ipc::{HyprlandApi, HyprlandIpc},
    keyboard::{KeyboardApi, WtypeKeyboard, normalize_key},
    models::{KeyModifier, Monitor, MotionProfile, MouseButton, Point, ScrollDirection, Window},
    policy::SafetyPolicy,
};
use chrono::{DateTime, Utc};
use schemars::JsonSchema;
use serde::Serialize;
use serde_json::{Value, json};
use sha2::{Digest, Sha256};
use std::{path::PathBuf, sync::Arc, time::Duration};
use tokio::time::{Instant, sleep};
use uuid::Uuid;

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct CursorObservation {
    pub captured_at: DateTime<Utc>,
    pub position: Point,
    pub monitor: Option<String>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct WindowsObservation {
    pub captured_at: DateTime<Utc>,
    pub windows: Vec<Window>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct MonitorGeometry {
    pub id: i64,
    pub name: String,
    pub description: String,
    pub x: i32,
    pub y: i32,
    pub logical_width: i32,
    pub logical_height: i32,
    pub pixel_width: u32,
    pub pixel_height: u32,
    pub scale: f64,
    pub transform: i32,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct ImageMetadata {
    pub mime_type: String,
    pub bytes: usize,
    pub sha256: String,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct DesktopMetadata {
    pub captured_at: DateTime<Utc>,
    pub coordinate_system: String,
    pub monitor: MonitorGeometry,
    pub cursor: Point,
    pub active_window: Option<Window>,
    pub image: ImageMetadata,
}

#[derive(Debug, Clone)]
pub struct DesktopObservation {
    pub metadata: DesktopMetadata,
    pub png: Vec<u8>,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct MoveResult {
    pub started_at: Point,
    pub ended_at: Point,
    pub requested: Point,
    pub duration_ms: u32,
    pub motion: String,
    pub steps: u32,
    pub monitor: String,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct ClickResult {
    pub position: Point,
    pub button: String,
    pub count: u8,
    pub interval_ms: u32,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct FocusResult {
    pub previous_window: Option<Window>,
    pub focused_window: Window,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct ScrollResult {
    pub position: Point,
    pub direction: String,
    pub amount: u8,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct PressKeyResult {
    pub key: String,
    pub modifiers: Vec<String>,
    pub repeat: u8,
    pub focused_window: Window,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct TypeTextResult {
    pub characters: usize,
    pub bytes: usize,
    pub sha256: String,
    pub interval_ms: u32,
    pub focused_window: Window,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct WaitResult {
    pub requested_ms: u32,
    pub elapsed_ms: u128,
}

#[derive(Clone)]
pub struct Harness {
    ipc: Arc<dyn HyprlandApi>,
    capture: Arc<dyn ScreenshotApi>,
    pointer: Arc<dyn PointerApi>,
    keyboard: Arc<dyn KeyboardApi>,
    policy: Arc<SafetyPolicy>,
    audit: Arc<AuditLogger>,
    session_id: Uuid,
}

impl std::fmt::Debug for Harness {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("Harness")
            .field("read_only", &self.policy.read_only())
            .field("audit_path", &self.audit.path())
            .field("session_id", &self.session_id)
            .finish()
    }
}

impl Harness {
    pub fn from_environment(read_only: bool, audit_path: Option<PathBuf>) -> Result<Self> {
        Ok(Self::new(
            Arc::new(HyprlandIpc::from_env()?),
            Arc::new(GrimCapture::discover()?),
            Arc::new(VirtualPointerActor::spawn()?),
            Arc::new(WtypeKeyboard::discover()?),
            Arc::new(SafetyPolicy::new(read_only)),
            Arc::new(AuditLogger::new(audit_path)?),
        ))
    }

    pub fn new(
        ipc: Arc<dyn HyprlandApi>,
        capture: Arc<dyn ScreenshotApi>,
        pointer: Arc<dyn PointerApi>,
        keyboard: Arc<dyn KeyboardApi>,
        policy: Arc<SafetyPolicy>,
        audit: Arc<AuditLogger>,
    ) -> Self {
        Self {
            ipc,
            capture,
            pointer,
            keyboard,
            policy,
            audit,
            session_id: Uuid::new_v4(),
        }
    }

    pub fn read_only(&self) -> bool {
        self.policy.read_only()
    }

    pub fn audit_path(&self) -> &std::path::Path {
        self.audit.path()
    }

    pub fn capture_executable(&self) -> &std::path::Path {
        self.capture.executable()
    }

    pub fn keyboard_executable(&self) -> &std::path::Path {
        self.keyboard.executable()
    }

    pub async fn monitors(&self) -> Result<Vec<Monitor>> {
        self.ipc.monitors().await
    }

    pub async fn version(&self) -> Result<Value> {
        self.ipc.version().await
    }

    pub async fn permission_option(&self) -> Result<Value> {
        self.ipc.get_option("ecosystem:enforce_permissions").await
    }

    pub async fn input_probe(&self) -> Result<()> {
        self.pointer.probe().await
    }

    pub async fn keyboard_probe(&self) -> Result<()> {
        self.keyboard.probe().await
    }

    pub async fn lock_status(&self) -> Result<bool> {
        self.ipc.locked().await
    }

    pub async fn observe_desktop(
        &self,
        monitor_name: Option<String>,
    ) -> Result<DesktopObservation> {
        let started = Instant::now();
        let mut arguments = json!({"monitor": monitor_name});
        let result = async {
            let monitors = self.ipc.monitors().await?;
            let monitor = select_monitor(&monitors, monitor_name.as_deref())?.clone();
            let cursor = self.ipc.cursor().await?;
            let windows = self.ipc.windows().await?;
            let capture = self.capture.capture_monitor(&monitor.name).await?;
            let active_window = windows.into_iter().find(|window| window.focused);
            let sha256 = format!("{:x}", Sha256::digest(&capture.png));
            let metadata = desktop_metadata(&monitor, &capture, cursor, active_window, sha256);
            Ok(DesktopObservation {
                metadata,
                png: capture.png,
            })
        }
        .await;
        if let Ok(observation) = &result {
            arguments["capture"] = json!({
                "monitor": observation.metadata.monitor.name,
                "pixel_width": observation.metadata.monitor.pixel_width,
                "pixel_height": observation.metadata.monitor.pixel_height,
                "bytes": observation.metadata.image.bytes,
                "sha256": observation.metadata.image.sha256,
            });
        }
        self.finish_audit("observe_desktop", arguments, started, &result, None, None)
            .await?;
        result
    }

    pub async fn get_cursor(&self) -> Result<CursorObservation> {
        let started = Instant::now();
        let result = async {
            let position = self.ipc.cursor().await?;
            let monitors = self.ipc.monitors().await?;
            let monitor = monitors
                .iter()
                .find(|monitor| monitor.contains(&position))
                .map(|monitor| monitor.name.clone());
            Ok(CursorObservation {
                captured_at: Utc::now(),
                position,
                monitor,
            })
        }
        .await;
        let after = result.as_ref().ok().map(|value| value.position.clone());
        self.finish_audit("get_cursor", json!({}), started, &result, None, after)
            .await?;
        result
    }

    pub async fn list_windows(&self) -> Result<WindowsObservation> {
        let started = Instant::now();
        let result = self.ipc.windows().await.map(|windows| WindowsObservation {
            captured_at: Utc::now(),
            windows: windows.into_iter().filter(|window| window.mapped).collect(),
        });
        self.finish_audit("list_windows", json!({}), started, &result, None, None)
            .await?;
        result
    }

    pub async fn move_pointer(
        &self,
        target: Point,
        requested_duration_ms: Option<u32>,
        motion: MotionProfile,
    ) -> Result<MoveResult> {
        let started = Instant::now();
        let before = self.ipc.cursor().await.ok();
        self.audit.ensure_writable()?;
        let result = async {
            if requested_duration_ms.is_some_and(|duration| duration > 3000) {
                return Err(HarnessError::invalid(
                    "duration_ms must be between 0 and 3000",
                ));
            }
            self.deny_if_locked().await?;
            self.policy.allow_move()?;
            let monitors = self.ipc.monitors().await?;
            let monitor = self.policy.validate_target(&target, &monitors)?.clone();
            let origin = self.ipc.cursor().await?;
            let distance = point_distance(&origin, &target);
            let duration_ms = resolve_move_duration(requested_duration_ms, &motion, distance);
            let effective_motion = if duration_ms == 0 {
                MotionProfile::Instant
            } else {
                motion.clone()
            };
            let path = pointer_path(&origin, &target, &monitor, &effective_motion, duration_ms);
            if effective_motion == MotionProfile::Instant {
                self.ipc.move_cursor(target.clone()).await?;
            } else {
                let animation_started = Instant::now();
                let path_len = path.len();
                for (index, point) in path.iter().enumerate() {
                    self.ipc.move_cursor(point.clone()).await?;
                    if index + 1 < path_len {
                        let next_frame = (index + 1) as f64 / (path_len - 1) as f64;
                        let deadline = animation_started
                            + Duration::from_secs_f64(f64::from(duration_ms) * next_frame / 1000.0);
                        tokio::time::sleep_until(deadline).await;
                    }
                }
            }
            let ended_at = self.ipc.cursor().await?;
            Ok(MoveResult {
                started_at: origin,
                ended_at,
                requested: target.clone(),
                duration_ms,
                motion: effective_motion.as_str().into(),
                steps: path.len() as u32,
                monitor: monitor.name,
            })
        }
        .await;
        let after = result.as_ref().ok().map(|value| value.ended_at.clone());
        self.finish_audit(
            "move_pointer",
            json!({
                "x": target.x,
                "y": target.y,
                "duration_ms": requested_duration_ms,
                "motion": motion.as_str(),
            }),
            started,
            &result,
            before,
            after,
        )
        .await?;
        result
    }

    pub async fn click(
        &self,
        button: MouseButton,
        count: u8,
        interval_ms: u32,
    ) -> Result<ClickResult> {
        let started = Instant::now();
        let before = self.ipc.cursor().await.ok();
        self.audit.ensure_writable()?;
        let result = async {
            if !(1..=3).contains(&count) {
                return Err(HarnessError::invalid("count must be between 1 and 3"));
            }
            if !(40..=1000).contains(&interval_ms) {
                return Err(HarnessError::invalid(
                    "interval_ms must be between 40 and 1000",
                ));
            }
            self.deny_if_locked().await?;
            self.policy.allow_clicks(count as usize)?;
            let position = self.ipc.cursor().await?;
            let monitors = self.ipc.monitors().await?;
            self.policy.validate_target(&position, &monitors)?;
            self.pointer
                .click(
                    button.clone(),
                    count,
                    Duration::from_millis(u64::from(interval_ms)),
                )
                .await?;
            Ok(ClickResult {
                position,
                button: button.as_str().into(),
                count,
                interval_ms,
            })
        }
        .await;
        let after = self.ipc.cursor().await.ok();
        self.finish_audit(
            "click",
            json!({"button": button.as_str(), "count": count, "interval_ms": interval_ms}),
            started,
            &result,
            before,
            after,
        )
        .await?;
        result
    }

    pub async fn focus_window(&self, window_id: String) -> Result<FocusResult> {
        let started = Instant::now();
        self.audit.ensure_writable()?;
        let result = async {
            self.deny_if_locked().await?;
            self.policy.allow_focus()?;
            let windows = self.ipc.windows().await?;
            let previous_window = windows.iter().find(|window| window.focused).cloned();
            let target = resolve_window(&windows, &window_id)?.clone();
            self.ipc.focus_window(&target.address).await?;
            let focused_windows = self.ipc.windows().await?;
            let focused_window = focused_windows
                .into_iter()
                .find(|window| window.address == target.address && window.focused)
                .ok_or_else(|| {
                    HarnessError::new(
                        "FOCUS_FAILED",
                        format!("Hyprland did not focus window '{window_id}'"),
                    )
                })?;
            Ok(FocusResult {
                previous_window,
                focused_window,
            })
        }
        .await;
        self.finish_audit(
            "focus_window",
            json!({"window_id": window_id}),
            started,
            &result,
            None,
            None,
        )
        .await?;
        result
    }

    pub async fn scroll(&self, direction: ScrollDirection, amount: u8) -> Result<ScrollResult> {
        let started = Instant::now();
        let before = self.ipc.cursor().await.ok();
        self.audit.ensure_writable()?;
        let result = async {
            if !(1..=20).contains(&amount) {
                return Err(HarnessError::invalid("amount must be between 1 and 20"));
            }
            self.deny_if_locked().await?;
            self.policy.allow_scroll(amount as usize)?;
            let position = self.ipc.cursor().await?;
            let monitors = self.ipc.monitors().await?;
            self.policy.validate_target(&position, &monitors)?;
            self.pointer.scroll(direction.clone(), amount).await?;
            Ok(ScrollResult {
                position,
                direction: direction.as_str().into(),
                amount,
            })
        }
        .await;
        let after = self.ipc.cursor().await.ok();
        self.finish_audit(
            "scroll",
            json!({"direction": direction.as_str(), "amount": amount}),
            started,
            &result,
            before,
            after,
        )
        .await?;
        result
    }

    pub async fn press_key(
        &self,
        key: String,
        modifiers: Vec<KeyModifier>,
        repeat: u8,
        expected_window_id: Option<String>,
    ) -> Result<PressKeyResult> {
        let started = Instant::now();
        self.audit.ensure_writable()?;
        let modifier_names: Vec<String> = modifiers
            .iter()
            .map(|modifier| modifier.as_str().into())
            .collect();
        let audit_key = normalize_key(&key).ok();
        let result = async {
            let canonical_key = normalize_key(&key)?;
            if !(1..=20).contains(&repeat) {
                return Err(HarnessError::invalid("repeat must be between 1 and 20"));
            }
            if has_duplicate_modifiers(&modifiers) {
                return Err(HarnessError::invalid("modifiers cannot contain duplicates"));
            }
            self.deny_if_locked().await?;
            self.policy.allow_keyboard(repeat as usize, "press_key")?;
            let focused_window = self
                .validate_keyboard_target(expected_window_id.as_deref())
                .await?;
            self.keyboard
                .press_key(&canonical_key, &modifiers, repeat)
                .await?;
            Ok(PressKeyResult {
                key: canonical_key,
                modifiers: modifier_names.clone(),
                repeat,
                focused_window,
            })
        }
        .await;
        self.finish_audit(
            "press_key",
            json!({
                "key": audit_key,
                "requested_key": key,
                "modifiers": modifier_names,
                "repeat": repeat,
                "expected_window_id": expected_window_id,
            }),
            started,
            &result,
            None,
            None,
        )
        .await?;
        result
    }

    pub async fn type_text(
        &self,
        text: String,
        interval_ms: u32,
        expected_window_id: Option<String>,
    ) -> Result<TypeTextResult> {
        let started = Instant::now();
        self.audit.ensure_writable()?;
        let characters = text.chars().count();
        let bytes = text.len();
        let sha256 = format!("{:x}", Sha256::digest(text.as_bytes()));
        let audit_arguments = json!({
            "characters": characters,
            "bytes": bytes,
            "sha256": sha256,
            "interval_ms": interval_ms,
            "expected_window_id": expected_window_id,
        });
        let result = async {
            if characters == 0 || characters > 2_000 || bytes > 8_192 {
                return Err(HarnessError::invalid(
                    "text must contain 1-2000 characters and at most 8192 UTF-8 bytes",
                ));
            }
            if interval_ms > 50 {
                return Err(HarnessError::invalid(
                    "interval_ms must be between 0 and 50",
                ));
            }
            if characters.saturating_mul(interval_ms as usize) > 30_000 {
                return Err(HarnessError::invalid(
                    "text length multiplied by interval_ms cannot exceed 30000 ms",
                ));
            }
            self.deny_if_locked().await?;
            self.policy.allow_keyboard(characters, "type_text")?;
            let focused_window = self
                .validate_keyboard_target(expected_window_id.as_deref())
                .await?;
            self.keyboard.type_text(&text, interval_ms).await?;
            Ok(TypeTextResult {
                characters,
                bytes,
                sha256: sha256.clone(),
                interval_ms,
                focused_window,
            })
        }
        .await;
        self.finish_audit("type_text", audit_arguments, started, &result, None, None)
            .await?;
        result
    }

    pub async fn wait(&self, duration_ms: u32) -> Result<WaitResult> {
        let started = Instant::now();
        let result = async {
            if duration_ms > 30_000 {
                return Err(HarnessError::invalid(
                    "duration_ms must be between 0 and 30000",
                ));
            }
            sleep(Duration::from_millis(u64::from(duration_ms))).await;
            Ok(WaitResult {
                requested_ms: duration_ms,
                elapsed_ms: started.elapsed().as_millis(),
            })
        }
        .await;
        self.finish_audit(
            "wait",
            json!({"duration_ms": duration_ms}),
            started,
            &result,
            None,
            None,
        )
        .await?;
        result
    }

    async fn validate_keyboard_target(&self, expected_window_id: Option<&str>) -> Result<Window> {
        let windows = self.ipc.windows().await?;
        let focused = windows
            .iter()
            .find(|window| window.focused)
            .ok_or_else(|| HarnessError::new("NO_FOCUSED_WINDOW", "no focused window was found"))?;
        if let Some(window_id) = expected_window_id {
            let expected = resolve_window(&windows, window_id)?;
            if expected.address != focused.address {
                return Err(HarnessError::new(
                    "FOCUS_MISMATCH",
                    format!(
                        "expected window '{}' is not focused; '{}' is focused instead",
                        window_id, focused.stable_id
                    ),
                ));
            }
        }
        if !focused.accepts_input {
            return Err(HarnessError::new(
                "WINDOW_REJECTS_INPUT",
                format!(
                    "focused window '{}' does not accept input",
                    focused.stable_id
                ),
            ));
        }
        Ok(focused.clone())
    }

    async fn deny_if_locked(&self) -> Result<()> {
        if self.ipc.locked().await? {
            Err(HarnessError::new(
                "SESSION_LOCKED",
                "desktop input actions are disabled while the session is locked",
            ))
        } else {
            Ok(())
        }
    }

    async fn finish_audit<T>(
        &self,
        tool: &str,
        arguments: Value,
        started: Instant,
        result: &Result<T>,
        cursor_before: Option<Point>,
        cursor_after: Option<Point>,
    ) -> Result<()> {
        let active_window = self
            .ipc
            .windows()
            .await
            .ok()
            .and_then(|windows| windows.into_iter().find(|window| window.focused))
            .map(|window| window.address);
        self.audit.record(&AuditRecord {
            timestamp: Utc::now(),
            session_id: self.session_id,
            request_id: Uuid::new_v4(),
            tool: tool.into(),
            arguments,
            active_window,
            cursor_before,
            cursor_after,
            duration_ms: started.elapsed().as_millis(),
            success: result.is_ok(),
            error_code: result.as_ref().err().map(|error| error.code.into()),
        })
    }
}

fn select_monitor<'a>(monitors: &'a [Monitor], requested: Option<&str>) -> Result<&'a Monitor> {
    let selected = if let Some(name) = requested {
        monitors.iter().find(|monitor| monitor.name == name)
    } else {
        monitors.iter().find(|monitor| monitor.focused)
    };
    selected
        .filter(|monitor| !monitor.disabled && monitor.dpms_status)
        .ok_or_else(|| {
            HarnessError::invalid(match requested {
                Some(name) => format!("monitor '{name}' was not found or is inactive"),
                None => "no focused active monitor was found".into(),
            })
        })
}

fn desktop_metadata(
    monitor: &Monitor,
    capture: &Capture,
    cursor: Point,
    active_window: Option<Window>,
    sha256: String,
) -> DesktopMetadata {
    DesktopMetadata {
        captured_at: Utc::now(),
        coordinate_system:
            "Hyprland global logical coordinates; image pixels may differ by monitor scale".into(),
        monitor: MonitorGeometry {
            id: monitor.id,
            name: monitor.name.clone(),
            description: monitor.description.clone(),
            x: monitor.x,
            y: monitor.y,
            logical_width: monitor.logical_width(),
            logical_height: monitor.logical_height(),
            pixel_width: capture.pixel_width,
            pixel_height: capture.pixel_height,
            scale: monitor.scale,
            transform: monitor.transform,
        },
        cursor,
        active_window,
        image: ImageMetadata {
            mime_type: "image/png".into(),
            bytes: capture.png.len(),
            sha256,
        },
    }
}

fn resolve_window<'a>(windows: &'a [Window], window_id: &str) -> Result<&'a Window> {
    if window_id.is_empty() {
        return Err(HarnessError::invalid("window_id cannot be empty"));
    }
    windows
        .iter()
        .find(|window| {
            window.mapped && (window.address == window_id || window.stable_id == window_id)
        })
        .ok_or_else(|| {
            HarnessError::new(
                "WINDOW_NOT_FOUND",
                format!("no mapped window matches '{window_id}'"),
            )
        })
}

fn has_duplicate_modifiers(modifiers: &[KeyModifier]) -> bool {
    modifiers
        .iter()
        .enumerate()
        .any(|(index, modifier)| modifiers[index + 1..].contains(modifier))
}

fn point_distance(start: &Point, end: &Point) -> f64 {
    f64::from(end.x - start.x).hypot(f64::from(end.y - start.y))
}

fn resolve_move_duration(
    requested_duration_ms: Option<u32>,
    motion: &MotionProfile,
    distance: f64,
) -> u32 {
    if *motion == MotionProfile::Instant || distance < 1.0 {
        return 0;
    }
    requested_duration_ms.unwrap_or_else(|| {
        (180.0 + 18.0 * distance.sqrt())
            .round()
            .clamp(220.0, 1_200.0) as u32
    })
}

fn minimum_jerk(progress: f64) -> f64 {
    let progress = progress.clamp(0.0, 1.0);
    progress.powi(3) * (10.0 - 15.0 * progress + 6.0 * progress.powi(2))
}

fn pointer_path(
    origin: &Point,
    target: &Point,
    target_monitor: &Monitor,
    motion: &MotionProfile,
    duration_ms: u32,
) -> Vec<Point> {
    if *motion == MotionProfile::Instant || duration_ms == 0 || origin == target {
        return vec![target.clone()];
    }

    // 90 Hz is smooth in recordings without flooding Hyprland's command socket.
    let steps = ((f64::from(duration_ms) / (1_000.0 / 90.0)).ceil() as u32).max(1);
    let can_curve = *motion == MotionProfile::Natural && target_monitor.contains(origin);
    let distance = point_distance(origin, target);
    let sign = if (origin.x ^ origin.y ^ target.x ^ target.y) & 1 == 0 {
        1.0
    } else {
        -1.0
    };
    let bend = if can_curve && distance >= 40.0 {
        (distance * 0.075).min(64.0) * sign
    } else {
        0.0
    };
    let dx = f64::from(target.x - origin.x);
    let dy = f64::from(target.y - origin.y);
    let (normal_x, normal_y) = if distance > 0.0 {
        (-dy / distance, dx / distance)
    } else {
        (0.0, 0.0)
    };
    let min_x = f64::from(target_monitor.x);
    let min_y = f64::from(target_monitor.y);
    let max_x = f64::from(target_monitor.x + target_monitor.logical_width() - 1);
    let max_y = f64::from(target_monitor.y + target_monitor.logical_height() - 1);
    let bounded_control = |x: f64, y: f64| {
        if can_curve {
            (x.clamp(min_x, max_x), y.clamp(min_y, max_y))
        } else {
            (x, y)
        }
    };
    let control_one = bounded_control(
        f64::from(origin.x) + dx * 0.28 + normal_x * bend * 0.8,
        f64::from(origin.y) + dy * 0.28 + normal_y * bend * 0.8,
    );
    let control_two = bounded_control(
        f64::from(origin.x) + dx * 0.72 + normal_x * bend,
        f64::from(origin.y) + dy * 0.72 + normal_y * bend,
    );

    let mut path = Vec::with_capacity(steps as usize);
    for step in 1..=steps {
        let t = minimum_jerk(f64::from(step) / f64::from(steps));
        let inverse = 1.0 - t;
        let x = inverse.powi(3) * f64::from(origin.x)
            + 3.0 * inverse.powi(2) * t * control_one.0
            + 3.0 * inverse * t.powi(2) * control_two.0
            + t.powi(3) * f64::from(target.x);
        let y = inverse.powi(3) * f64::from(origin.y)
            + 3.0 * inverse.powi(2) * t * control_one.1
            + 3.0 * inverse * t.powi(2) * control_two.1
            + t.powi(3) * f64::from(target.y);
        let point = Point {
            x: x.round() as i32,
            y: y.round() as i32,
        };
        if path.last() != Some(&point) {
            path.push(point);
        }
    }
    if path.last() != Some(target) {
        path.push(target.clone());
    }
    path
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn minimum_jerk_has_stable_endpoints_and_is_monotonic() {
        assert_eq!(minimum_jerk(0.0), 0.0);
        assert_eq!(minimum_jerk(1.0), 1.0);
        let values: Vec<_> = (0..=100)
            .map(|step| minimum_jerk(f64::from(step) / 100.0))
            .collect();
        assert!(values.windows(2).all(|pair| pair[0] <= pair[1]));
    }

    #[test]
    fn automatic_duration_scales_with_distance_and_is_bounded() {
        assert_eq!(resolve_move_duration(None, &MotionProfile::Natural, 0.0), 0);
        let short = resolve_move_duration(None, &MotionProfile::Natural, 100.0);
        let long = resolve_move_duration(None, &MotionProfile::Natural, 1_000.0);
        assert!(short >= 220);
        assert!(long > short);
        assert_eq!(
            resolve_move_duration(None, &MotionProfile::Natural, 1_000_000.0),
            1_200
        );
        assert_eq!(
            resolve_move_duration(Some(750), &MotionProfile::Smooth, 100.0),
            750
        );
        assert_eq!(
            resolve_move_duration(Some(750), &MotionProfile::Instant, 100.0),
            0
        );
    }

    #[test]
    fn natural_path_curves_stays_bounded_and_finishes_exactly() {
        let monitor = Monitor {
            id: 0,
            name: "test".into(),
            description: String::new(),
            width: 1920,
            height: 1080,
            x: 0,
            y: 0,
            scale: 1.0,
            transform: 0,
            focused: true,
            disabled: false,
            dpms_status: true,
            active_workspace: Default::default(),
        };
        let origin = Point { x: 100, y: 100 };
        let target = Point { x: 1_400, y: 700 };
        let path = pointer_path(&origin, &target, &monitor, &MotionProfile::Natural, 700);
        assert_eq!(path.last(), Some(&target));
        assert!(path.iter().all(|point| monitor.contains(point)));
        assert!(path.iter().any(|point| {
            let cross = i64::from(point.x - origin.x) * i64::from(target.y - origin.y)
                - i64::from(point.y - origin.y) * i64::from(target.x - origin.x);
            cross != 0
        }));
    }

    #[test]
    fn smooth_path_is_straight_and_finishes_exactly() {
        let monitor = Monitor {
            id: 0,
            name: "test".into(),
            description: String::new(),
            width: 1920,
            height: 1080,
            x: 0,
            y: 0,
            scale: 1.0,
            transform: 0,
            focused: true,
            disabled: false,
            dpms_status: true,
            active_workspace: Default::default(),
        };
        let origin = Point { x: -500, y: -250 };
        let target = Point { x: 1_500, y: 750 };
        let path = pointer_path(&origin, &target, &monitor, &MotionProfile::Smooth, 700);
        assert_eq!(path.last(), Some(&target));
        assert!(path.iter().all(|point| {
            let cross = i64::from(point.x - origin.x) * i64::from(target.y - origin.y)
                - i64::from(point.y - origin.y) * i64::from(target.x - origin.x);
            cross.abs() <= 1_000
        }));
    }

    #[test]
    fn detects_duplicate_modifiers() {
        assert!(has_duplicate_modifiers(&[
            KeyModifier::Ctrl,
            KeyModifier::Ctrl
        ]));
        assert!(!has_duplicate_modifiers(&[
            KeyModifier::Ctrl,
            KeyModifier::Shift
        ]));
    }
}
