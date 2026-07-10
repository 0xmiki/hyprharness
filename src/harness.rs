use crate::{
    audit::{AuditLogger, AuditRecord},
    capture::{Capture, GrimCapture, ScreenshotApi},
    error::{HarnessError, Result},
    input::{PointerApi, VirtualPointerActor},
    ipc::{HyprlandApi, HyprlandIpc},
    models::{Monitor, MouseButton, Point, Window},
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
    pub monitor: String,
}

#[derive(Debug, Clone, Serialize, JsonSchema)]
pub struct ClickResult {
    pub position: Point,
    pub button: String,
    pub count: u8,
    pub interval_ms: u32,
}

#[derive(Clone)]
pub struct Harness {
    ipc: Arc<dyn HyprlandApi>,
    capture: Arc<dyn ScreenshotApi>,
    pointer: Arc<dyn PointerApi>,
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
            Arc::new(SafetyPolicy::new(read_only)),
            Arc::new(AuditLogger::new(audit_path)?),
        ))
    }

    pub fn new(
        ipc: Arc<dyn HyprlandApi>,
        capture: Arc<dyn ScreenshotApi>,
        pointer: Arc<dyn PointerApi>,
        policy: Arc<SafetyPolicy>,
        audit: Arc<AuditLogger>,
    ) -> Self {
        Self {
            ipc,
            capture,
            pointer,
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

    pub async fn move_pointer(&self, target: Point, duration_ms: u32) -> Result<MoveResult> {
        let started = Instant::now();
        let before = self.ipc.cursor().await.ok();
        self.audit.ensure_writable()?;
        let result = async {
            if duration_ms > 2000 {
                return Err(HarnessError::invalid(
                    "duration_ms must be between 0 and 2000",
                ));
            }
            self.deny_if_locked().await?;
            self.policy.allow_move()?;
            let monitors = self.ipc.monitors().await?;
            let monitor = self
                .policy
                .validate_target(&target, &monitors)?
                .name
                .clone();
            let origin = self.ipc.cursor().await?;
            if duration_ms == 0 {
                self.ipc.move_cursor(target.clone()).await?;
            } else {
                let steps = ((duration_ms as f64 / (1000.0 / 60.0)).ceil() as u32).max(1);
                let step_delay = Duration::from_millis(u64::from(duration_ms) / u64::from(steps));
                for step in 1..=steps {
                    let progress = f64::from(step) / f64::from(steps);
                    let point = Point {
                        x: interpolate(origin.x, target.x, progress),
                        y: interpolate(origin.y, target.y, progress),
                    };
                    self.ipc.move_cursor(point).await?;
                    if step < steps && !step_delay.is_zero() {
                        sleep(step_delay).await;
                    }
                }
            }
            let ended_at = self.ipc.cursor().await?;
            Ok(MoveResult {
                started_at: origin,
                ended_at,
                requested: target.clone(),
                duration_ms,
                monitor,
            })
        }
        .await;
        let after = result.as_ref().ok().map(|value| value.ended_at.clone());
        self.finish_audit(
            "move_pointer",
            json!({"x": target.x, "y": target.y, "duration_ms": duration_ms}),
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

    async fn deny_if_locked(&self) -> Result<()> {
        if self.ipc.locked().await? {
            Err(HarnessError::new(
                "SESSION_LOCKED",
                "pointer actions are disabled while the session is locked",
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

fn interpolate(start: i32, end: i32, progress: f64) -> i32 {
    (f64::from(start) + f64::from(end - start) * progress).round() as i32
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interpolation_finishes_exactly() {
        assert_eq!(interpolate(-100, 101, 1.0), 101);
        assert_eq!(interpolate(0, 10, 0.5), 5);
    }
}
