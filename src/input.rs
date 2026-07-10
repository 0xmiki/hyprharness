use crate::{
    error::{HarnessError, Result},
    models::{MouseButton, ScrollDirection},
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
    async fn click(&self, button: MouseButton, count: u8, interval: Duration) -> Result<()>;
    async fn scroll(&self, direction: ScrollDirection, amount: u8) -> Result<()>;
    async fn probe(&self) -> Result<()>;
}

enum Command {
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
