use crate::{
    error::{HarnessError, Result},
    models::{Monitor, MouseButton, Point, ScrollDirection},
};
use async_trait::async_trait;
use std::{
    sync::mpsc::{self, Receiver, Sender},
    thread,
    time::{Duration, Instant},
};
use tokio::sync::oneshot;
use wayland_client::{
    Connection, Dispatch, QueueHandle, delegate_noop,
    globals::{GlobalListContents, registry_queue_init},
    protocol::{wl_pointer, wl_registry},
};
use wayland_protocols_wlr::virtual_pointer::v1::client::{
    zwlr_virtual_pointer_manager_v1::ZwlrVirtualPointerManagerV1,
    zwlr_virtual_pointer_v1::ZwlrVirtualPointerV1,
};

#[async_trait]
pub trait PointerApi: Send + Sync {
    async fn move_trajectory(
        &self,
        points: Vec<Point>,
        bounds: DesktopBounds,
        duration: Duration,
    ) -> Result<()>;
    async fn click(&self, button: MouseButton, count: u8, interval: Duration) -> Result<()>;
    async fn scroll(&self, direction: ScrollDirection, amount: u8) -> Result<()>;
    async fn probe(&self) -> Result<()>;
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DesktopBounds {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

impl DesktopBounds {
    pub fn from_monitors(monitors: &[Monitor]) -> Result<Self> {
        let active: Vec<_> = monitors
            .iter()
            .filter(|monitor| !monitor.disabled && monitor.dpms_status)
            .collect();
        let min_x = active
            .iter()
            .map(|monitor| i64::from(monitor.x))
            .min()
            .ok_or_else(|| HarnessError::new("OUT_OF_BOUNDS", "no active monitors found"))?;
        let min_y = active
            .iter()
            .map(|monitor| i64::from(monitor.y))
            .min()
            .ok_or_else(|| HarnessError::new("OUT_OF_BOUNDS", "no active monitors found"))?;
        let max_x = active
            .iter()
            .map(|monitor| i64::from(monitor.x) + i64::from(monitor.logical_width()))
            .max()
            .unwrap();
        let max_y = active
            .iter()
            .map(|monitor| i64::from(monitor.y) + i64::from(monitor.logical_height()))
            .max()
            .unwrap();
        let width = u32::try_from(max_x - min_x)
            .ok()
            .filter(|width| *width > 0)
            .ok_or_else(|| HarnessError::invalid("active monitor layout width is invalid"))?;
        let height = u32::try_from(max_y - min_y)
            .ok()
            .filter(|height| *height > 0)
            .ok_or_else(|| HarnessError::invalid("active monitor layout height is invalid"))?;
        Ok(Self {
            x: i32::try_from(min_x)
                .map_err(|_| HarnessError::invalid("monitor layout X origin is invalid"))?,
            y: i32::try_from(min_y)
                .map_err(|_| HarnessError::invalid("monitor layout Y origin is invalid"))?,
            width,
            height,
        })
    }

    fn map_point(self, point: &Point) -> Result<AbsolutePoint> {
        let x = i64::from(point.x) - i64::from(self.x);
        let y = i64::from(point.y) - i64::from(self.y);
        if x < 0 || y < 0 || x >= i64::from(self.width) || y >= i64::from(self.height) {
            return Err(HarnessError::new(
                "OUT_OF_BOUNDS",
                format!(
                    "point ({}, {}) is outside Wayland desktop bounds",
                    point.x, point.y
                ),
            ));
        }
        Ok(AbsolutePoint {
            x: x as u32,
            y: y as u32,
        })
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct AbsolutePoint {
    x: u32,
    y: u32,
}

enum Command {
    MoveTrajectory {
        points: Vec<Point>,
        bounds: DesktopBounds,
        duration: Duration,
        reply: oneshot::Sender<Result<()>>,
    },
    Click {
        button: MouseButton,
        count: u8,
        interval: Duration,
        reply: oneshot::Sender<Result<()>>,
    },
    Scroll {
        direction: ScrollDirection,
        amount: u8,
        reply: oneshot::Sender<Result<()>>,
    },
    Probe {
        reply: oneshot::Sender<Result<()>>,
    },
}

#[derive(Clone)]
pub struct VirtualPointerActor {
    sender: Sender<Command>,
}

impl std::fmt::Debug for VirtualPointerActor {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("VirtualPointerActor")
            .finish_non_exhaustive()
    }
}

impl VirtualPointerActor {
    pub fn spawn() -> Result<Self> {
        let (sender, receiver) = mpsc::channel();
        thread::Builder::new()
            .name("hyprharness-input".into())
            .spawn(move || run_actor(receiver))
            .map_err(|error| {
                HarnessError::io("INPUT_UNAVAILABLE", "spawn virtual pointer actor", error)
            })?;
        Ok(Self { sender })
    }

    async fn send(&self, command: Command, receiver: oneshot::Receiver<Result<()>>) -> Result<()> {
        self.sender
            .send(command)
            .map_err(|_| HarnessError::new("INPUT_UNAVAILABLE", "input actor stopped"))?;
        receiver
            .await
            .map_err(|_| HarnessError::new("INPUT_UNAVAILABLE", "input actor dropped reply"))?
    }
}

#[async_trait]
impl PointerApi for VirtualPointerActor {
    async fn move_trajectory(
        &self,
        points: Vec<Point>,
        bounds: DesktopBounds,
        duration: Duration,
    ) -> Result<()> {
        let (reply, receiver) = oneshot::channel();
        self.send(
            Command::MoveTrajectory {
                points,
                bounds,
                duration,
                reply,
            },
            receiver,
        )
        .await
    }

    async fn click(&self, button: MouseButton, count: u8, interval: Duration) -> Result<()> {
        let (reply, receiver) = oneshot::channel();
        self.send(
            Command::Click {
                button,
                count,
                interval,
                reply,
            },
            receiver,
        )
        .await
    }

    async fn scroll(&self, direction: ScrollDirection, amount: u8) -> Result<()> {
        let (reply, receiver) = oneshot::channel();
        self.send(
            Command::Scroll {
                direction,
                amount,
                reply,
            },
            receiver,
        )
        .await
    }

    async fn probe(&self) -> Result<()> {
        let (reply, receiver) = oneshot::channel();
        self.send(Command::Probe { reply }, receiver).await
    }
}

struct InputState;

impl Dispatch<wl_registry::WlRegistry, GlobalListContents> for InputState {
    fn event(
        _state: &mut Self,
        _proxy: &wl_registry::WlRegistry,
        _event: wl_registry::Event,
        _data: &GlobalListContents,
        _conn: &Connection,
        _qhandle: &QueueHandle<Self>,
    ) {
    }
}

delegate_noop!(InputState: ignore ZwlrVirtualPointerManagerV1);
delegate_noop!(InputState: ignore ZwlrVirtualPointerV1);

struct WaylandPointer {
    connection: Connection,
    pointer: ZwlrVirtualPointerV1,
    started: Instant,
}

impl WaylandPointer {
    fn connect() -> Result<Self> {
        let connection = Connection::connect_to_env()
            .map_err(|e| HarnessError::io("INPUT_UNAVAILABLE", "connect to Wayland display", e))?;
        let (globals, queue) = registry_queue_init::<InputState>(&connection)
            .map_err(|e| HarnessError::io("INPUT_UNAVAILABLE", "read Wayland globals", e))?;
        let qh = queue.handle();
        let manager: ZwlrVirtualPointerManagerV1 = globals
            .bind(&qh, 1..=2, ())
            .map_err(|e| HarnessError::io("INPUT_UNAVAILABLE", "bind virtual pointer", e))?;
        let pointer = manager.create_virtual_pointer(None, &qh, ());
        connection
            .flush()
            .map_err(|e| HarnessError::io("INPUT_UNAVAILABLE", "initialize virtual pointer", e))?;
        Ok(Self {
            connection,
            pointer,
            started: Instant::now(),
        })
    }

    fn click(&self, button: MouseButton, count: u8, interval: Duration) -> Result<()> {
        for index in 0..count {
            self.button(button.linux_code(), wl_pointer::ButtonState::Pressed)?;
            thread::sleep(Duration::from_millis(20));
            self.button(button.linux_code(), wl_pointer::ButtonState::Released)?;
            if index + 1 < count {
                thread::sleep(interval);
            }
        }
        Ok(())
    }

    fn move_trajectory(
        &self,
        points: &[Point],
        bounds: DesktopBounds,
        duration: Duration,
    ) -> Result<()> {
        if points.is_empty() {
            return Err(HarnessError::invalid(
                "pointer trajectory must contain at least one point",
            ));
        }
        let absolute_points: Vec<_> = points
            .iter()
            .map(|point| bounds.map_point(point))
            .collect::<Result<_>>()?;
        let movement_started = Instant::now();
        let last_index = absolute_points.len().saturating_sub(1);
        for (index, point) in absolute_points.iter().enumerate() {
            let time = self.started.elapsed().as_millis() as u32;
            self.pointer
                .motion_absolute(time, point.x, point.y, bounds.width, bounds.height);
            self.pointer.frame();
            self.connection
                .flush()
                .map_err(|error| HarnessError::io("INPUT_UNAVAILABLE", "move pointer", error))?;
            if index < last_index {
                let progress = (index + 1) as f64 / last_index as f64;
                let deadline = movement_started + duration.mul_f64(progress);
                if let Some(remaining) = deadline.checked_duration_since(Instant::now()) {
                    thread::sleep(remaining);
                }
            }
        }
        self.connection
            .roundtrip()
            .map(|_| ())
            .map_err(|error| HarnessError::io("INPUT_UNAVAILABLE", "sync pointer movement", error))
    }

    fn button(&self, code: u32, state: wl_pointer::ButtonState) -> Result<()> {
        let time = self.started.elapsed().as_millis() as u32;
        self.pointer.button(time, code, state);
        self.pointer.frame();
        self.connection
            .flush()
            .and_then(|_| self.connection.roundtrip().map(|_| ()))
            .map_err(|e| HarnessError::io("INPUT_UNAVAILABLE", "send pointer button", e))
    }

    fn scroll(&self, direction: ScrollDirection, amount: u8) -> Result<()> {
        let (axis, sign) = match direction {
            ScrollDirection::Up => (wl_pointer::Axis::VerticalScroll, -1.0),
            ScrollDirection::Down => (wl_pointer::Axis::VerticalScroll, 1.0),
            ScrollDirection::Left => (wl_pointer::Axis::HorizontalScroll, -1.0),
            ScrollDirection::Right => (wl_pointer::Axis::HorizontalScroll, 1.0),
        };
        let time = self.started.elapsed().as_millis() as u32;
        let discrete = sign as i32 * i32::from(amount);
        let value = sign * f64::from(amount) * 15.0;
        self.pointer.axis_source(wl_pointer::AxisSource::Wheel);
        self.pointer.axis(time, axis, value);
        self.pointer.axis_discrete(time, axis, value, discrete);
        self.pointer.frame();
        self.connection
            .flush()
            .and_then(|_| self.connection.roundtrip().map(|_| ()))
            .map_err(|e| HarnessError::io("INPUT_UNAVAILABLE", "send pointer scroll", e))
    }
}

fn run_actor(receiver: Receiver<Command>) {
    let mut pointer: Option<WaylandPointer> = None;
    for command in receiver {
        let result = match &command {
            Command::Probe { .. } => ensure_pointer(&mut pointer).map(|_| ()),
            Command::MoveTrajectory {
                points,
                bounds,
                duration,
                ..
            } => ensure_pointer(&mut pointer)
                .and_then(|pointer| pointer.move_trajectory(points, *bounds, *duration)),
            Command::Click {
                button,
                count,
                interval,
                ..
            } => ensure_pointer(&mut pointer)
                .and_then(|pointer| pointer.click(button.clone(), *count, *interval)),
            Command::Scroll {
                direction, amount, ..
            } => ensure_pointer(&mut pointer)
                .and_then(|pointer| pointer.scroll(direction.clone(), *amount)),
        };
        match command {
            Command::Probe { reply }
            | Command::MoveTrajectory { reply, .. }
            | Command::Click { reply, .. }
            | Command::Scroll { reply, .. } => {
                let _ = reply.send(result);
            }
        }
    }
}

fn ensure_pointer(pointer: &mut Option<WaylandPointer>) -> Result<&WaylandPointer> {
    if pointer.is_none() {
        *pointer = Some(WaylandPointer::connect()?);
    }
    pointer.as_ref().ok_or_else(|| {
        HarnessError::new(
            "INPUT_UNAVAILABLE",
            "virtual pointer could not be initialized",
        )
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::WorkspaceRef;

    fn monitor(x: i32, y: i32, width: i32, height: i32, scale: f64) -> Monitor {
        Monitor {
            id: 0,
            name: "test".into(),
            description: String::new(),
            width,
            height,
            x,
            y,
            scale,
            transform: 0,
            focused: true,
            disabled: false,
            dpms_status: true,
            active_workspace: WorkspaceRef::default(),
        }
    }

    #[test]
    fn maps_negative_and_scaled_monitor_layouts_to_unsigned_wayland_space() {
        let bounds = DesktopBounds::from_monitors(&[
            monitor(-1920, 0, 1920, 1080, 1.0),
            monitor(0, 0, 1920, 1080, 2.0),
        ])
        .unwrap();
        assert_eq!(
            bounds,
            DesktopBounds {
                x: -1920,
                y: 0,
                width: 2880,
                height: 1080,
            }
        );
        assert_eq!(
            bounds.map_point(&Point { x: -1920, y: 0 }).unwrap(),
            AbsolutePoint { x: 0, y: 0 }
        );
        assert_eq!(
            bounds.map_point(&Point { x: 959, y: 539 }).unwrap(),
            AbsolutePoint { x: 2879, y: 539 }
        );
    }

    #[test]
    fn rejects_points_outside_the_wayland_coordinate_frame() {
        let bounds = DesktopBounds {
            x: -100,
            y: -50,
            width: 200,
            height: 100,
        };
        assert!(bounds.map_point(&Point { x: -101, y: 0 }).is_err());
        assert!(bounds.map_point(&Point { x: 100, y: 0 }).is_err());
        assert!(bounds.map_point(&Point { x: 0, y: 50 }).is_err());
    }
}
