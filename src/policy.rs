use crate::{
    error::{HarnessError, Result},
    models::{Monitor, Point},
};
use std::{
    collections::VecDeque,
    sync::Mutex,
    time::{Duration, Instant},
};

const WINDOW: Duration = Duration::from_secs(60);
const MOVE_LIMIT: usize = 300;
const CLICK_LIMIT: usize = 60;
const FOCUS_LIMIT: usize = 120;
const SCROLL_LIMIT: usize = 240;
const KEYBOARD_LIMIT: usize = 2_000;

#[derive(Debug)]
pub struct SafetyPolicy {
    read_only: bool,
    moves: Mutex<VecDeque<Instant>>,
    clicks: Mutex<VecDeque<Instant>>,
    focuses: Mutex<VecDeque<Instant>>,
    scrolls: Mutex<VecDeque<Instant>>,
    keyboard: Mutex<VecDeque<Instant>>,
}

impl SafetyPolicy {
    pub fn new(read_only: bool) -> Self {
        Self {
            read_only,
            moves: Mutex::new(VecDeque::new()),
            clicks: Mutex::new(VecDeque::new()),
            focuses: Mutex::new(VecDeque::new()),
            scrolls: Mutex::new(VecDeque::new()),
            keyboard: Mutex::new(VecDeque::new()),
        }
    }

    pub fn read_only(&self) -> bool {
        self.read_only
    }

    pub fn validate_target<'a>(
        &self,
        point: &Point,
        monitors: &'a [Monitor],
    ) -> Result<&'a Monitor> {
        monitors
            .iter()
            .find(|monitor| monitor.contains(point))
            .ok_or_else(|| {
                HarnessError::new(
                    "OUT_OF_BOUNDS",
                    format!(
                        "point ({}, {}) is outside enabled monitor bounds",
                        point.x, point.y
                    ),
                )
            })
    }

    pub fn allow_move(&self) -> Result<()> {
        self.ensure_input_enabled()?;
        register(&self.moves, 1, MOVE_LIMIT, "move_pointer")
    }

    pub fn allow_clicks(&self, count: usize) -> Result<()> {
        self.ensure_input_enabled()?;
        register(&self.clicks, count, CLICK_LIMIT, "click")
    }

    pub fn allow_focus(&self) -> Result<()> {
        self.ensure_input_enabled()?;
        register(&self.focuses, 1, FOCUS_LIMIT, "focus_window")
    }

    pub fn allow_workspace(&self) -> Result<()> {
        self.ensure_input_enabled()?;
        register(&self.focuses, 1, FOCUS_LIMIT, "switch_workspace")
    }

    pub fn allow_scroll(&self, amount: usize) -> Result<()> {
        self.ensure_input_enabled()?;
        register(&self.scrolls, amount, SCROLL_LIMIT, "scroll")
    }

    pub fn allow_keyboard(&self, events: usize, tool: &str) -> Result<()> {
        self.ensure_input_enabled()?;
        register(&self.keyboard, events, KEYBOARD_LIMIT, tool)
    }

    fn ensure_input_enabled(&self) -> Result<()> {
        if self.read_only {
            Err(HarnessError::new(
                "INPUT_DISABLED",
                "desktop input is disabled by --read-only",
            ))
        } else {
            Ok(())
        }
    }
}

fn register(
    queue: &Mutex<VecDeque<Instant>>,
    amount: usize,
    limit: usize,
    tool: &str,
) -> Result<()> {
    let now = Instant::now();
    let mut queue = queue
        .lock()
        .map_err(|_| HarnessError::new("INTERNAL_ERROR", "rate limiter state is unavailable"))?;
    while queue
        .front()
        .is_some_and(|instant| now.duration_since(*instant) >= WINDOW)
    {
        queue.pop_front();
    }
    if queue.len() + amount > limit {
        return Err(HarnessError::new(
            "RATE_LIMITED",
            format!("{tool} exceeded the limit of {limit} events per minute"),
        ));
    }
    queue.extend(std::iter::repeat_n(now, amount));
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::models::WorkspaceRef;

    fn monitor(x: i32, scale: f64) -> Monitor {
        Monitor {
            id: 1,
            name: "test".into(),
            description: String::new(),
            width: 1920,
            height: 1080,
            x,
            y: 0,
            scale,
            transform: 0,
            focused: true,
            disabled: false,
            dpms_status: true,
            active_workspace: WorkspaceRef::default(),
        }
    }

    #[test]
    fn validates_negative_and_scaled_layouts() {
        let policy = SafetyPolicy::new(false);
        let monitors = vec![monitor(-1920, 1.0), monitor(0, 2.0)];
        assert!(
            policy
                .validate_target(&Point { x: -1, y: 100 }, &monitors)
                .is_ok()
        );
        assert!(
            policy
                .validate_target(&Point { x: 959, y: 539 }, &monitors)
                .is_ok()
        );
        assert!(
            policy
                .validate_target(&Point { x: 960, y: 100 }, &monitors)
                .is_err()
        );
    }

    #[test]
    fn read_only_denies_input() {
        assert_eq!(
            SafetyPolicy::new(true).allow_move().unwrap_err().code,
            "INPUT_DISABLED"
        );
    }

    #[test]
    fn rate_limits_excessive_moves() {
        let policy = SafetyPolicy::new(false);
        for _ in 0..MOVE_LIMIT {
            policy.allow_move().unwrap();
        }
        assert_eq!(policy.allow_move().unwrap_err().code, "RATE_LIMITED");
    }
}
