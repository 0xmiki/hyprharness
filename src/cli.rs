use crate::{Harness, capture::write_png, error::HarnessError, mcp, models::Point};
use clap::{Parser, Subcommand};
use serde_json::{Value, json};
use std::path::PathBuf;

#[derive(Debug, Parser)]
#[command(
    name = "hyprharness",
    version,
    about = "Codex computer use for Hyprland"
)]
pub struct App {
    #[command(subcommand)]
    command: Command,
}

#[derive(Debug, Subcommand)]
enum Command {
    /// Run the MCP server over stdio.
    Mcp {
        /// Disable all focus, pointer, and keyboard actions.
        #[arg(long)]
        read_only: bool,
        /// Override the JSONL audit log path.
        #[arg(long)]
        audit_log: Option<PathBuf>,
    },
    /// Diagnose the Hyprland, capture, input, and audit environment.
    Doctor {
        #[arg(long)]
        json: bool,
    },
    /// Capture a desktop observation to a PNG file.
    Observe {
        #[arg(long)]
        monitor: Option<String>,
        #[arg(long)]
        output: Option<PathBuf>,
    },
    /// Move the pointer through a small bounded pattern and restore it.
    TestPointer {
        #[arg(long, default_value_t = 40)]
        distance: i32,
        /// Also perform a left click after the movement test.
        #[arg(long, requires = "yes")]
        click: bool,
        /// Confirm the intentionally side-effectful --click test.
        #[arg(long)]
        yes: bool,
    },
    /// Report Hyprland and hyprharness permission state.
    Permissions {
        #[arg(long)]
        json: bool,
    },
}

pub async fn run() -> crate::Result<()> {
    match App::parse().command {
        Command::Mcp {
            read_only,
            audit_log,
        } => {
            let harness = Harness::from_environment(read_only, audit_log)?;
            tracing::info!(read_only, audit = %harness.audit_path().display(), "starting MCP server");
            mcp::serve(harness)
                .await
                .map_err(|e| HarnessError::new("MCP_ERROR", e.to_string()))
        }
        Command::Doctor { json } => doctor(json).await,
        Command::Observe { monitor, output } => observe(monitor, output).await,
        Command::TestPointer {
            distance,
            click,
            yes: _,
        } => test_pointer(distance, click).await,
        Command::Permissions { json } => permissions(json).await,
    }
}

async fn doctor(as_json: bool) -> crate::Result<()> {
    let harness = Harness::from_environment(false, None)?;
    let version = check_value(harness.version().await);
    let monitors = check_value(harness.monitors().await);
    let screenshot = match harness.observe_desktop(None).await {
        Ok(observation) => json!({
            "ok": true,
            "monitor": observation.metadata.monitor.name,
            "size": [observation.metadata.monitor.pixel_width, observation.metadata.monitor.pixel_height],
            "bytes": observation.metadata.image.bytes,
        }),
        Err(error) => error.as_json(),
    };
    let input = match harness.input_probe().await {
        Ok(()) => json!({"ok": true}),
        Err(error) => error.as_json(),
    };
    let keyboard = match harness.keyboard_probe().await {
        Ok(()) => json!({"ok": true}),
        Err(error) => error.as_json(),
    };
    let healthy = [&version, &monitors, &screenshot, &input, &keyboard]
        .into_iter()
        .all(|value| value["ok"] == true);
    let report = json!({
        "ok": healthy,
        "hyprland": version,
        "monitors": monitors,
        "screenshot": screenshot,
        "virtual_pointer": input,
        "virtual_keyboard": keyboard,
        "grim": harness.capture_executable(),
        "wtype": harness.keyboard_executable(),
        "audit_log": harness.audit_path(),
        "read_only": harness.read_only(),
    });
    print_report(&report, as_json);
    if report["ok"] == true {
        Ok(())
    } else {
        Err(HarnessError::new(
            "DOCTOR_FAILED",
            "one or more diagnostic checks failed",
        ))
    }
}

async fn observe(monitor: Option<String>, output: Option<PathBuf>) -> crate::Result<()> {
    let harness = Harness::from_environment(true, None)?;
    let observation = harness.observe_desktop(monitor).await?;
    let path = output.unwrap_or_else(default_observation_path);
    write_png(&path, &observation.png).await?;
    println!(
        "{}",
        serde_json::to_string_pretty(&json!({
            "ok": true,
            "output": path,
            "metadata": observation.metadata,
        }))
        .expect("observation JSON serialization")
    );
    Ok(())
}

async fn test_pointer(distance: i32, click: bool) -> crate::Result<()> {
    if !(1..=500).contains(&distance) {
        return Err(HarnessError::invalid("distance must be between 1 and 500"));
    }
    let harness = Harness::from_environment(false, None)?;
    let origin = harness.get_cursor().await?.position;
    let monitors = harness.monitors().await?;
    let monitor = monitors
        .iter()
        .find(|monitor| monitor.contains(&origin))
        .ok_or_else(|| HarnessError::new("OUT_OF_BOUNDS", "cursor is not on an active monitor"))?;
    let max_x = monitor.x + monitor.logical_width() - 1;
    let max_y = monitor.y + monitor.logical_height() - 1;
    let x = (origin.x + distance).clamp(monitor.x, max_x);
    let y = (origin.y + distance).clamp(monitor.y, max_y);
    let points = [
        Point { x, y: origin.y },
        Point { x, y },
        Point { x: origin.x, y },
    ];
    for point in points {
        harness
            .move_pointer(point, Some(150), crate::models::MotionProfile::Smooth)
            .await?;
    }
    harness
        .move_pointer(
            origin.clone(),
            Some(150),
            crate::models::MotionProfile::Smooth,
        )
        .await?;
    if click {
        harness
            .click(crate::models::MouseButton::Left, 1, 120)
            .await?;
    }
    println!(
        "{}",
        json!({"ok": true, "restored": origin, "clicked": click})
    );
    Ok(())
}

async fn permissions(as_json: bool) -> crate::Result<()> {
    let harness = Harness::from_environment(false, None)?;
    let option = check_value(harness.permission_option().await);
    let locked = check_value(harness.lock_status().await);
    let screencopy = match harness.observe_desktop(None).await {
        Ok(observation) => json!({
            "ok": true,
            "backend": "grim",
            "monitor": observation.metadata.monitor.name,
            "bytes": observation.metadata.image.bytes,
        }),
        Err(error) => error.as_json(),
    };
    let input = match harness.input_probe().await {
        Ok(()) => json!({"ok": true, "available": true}),
        Err(error) => error.as_json(),
    };
    let keyboard = match harness.keyboard_probe().await {
        Ok(()) => json!({"ok": true, "available": true, "backend": "wtype"}),
        Err(error) => error.as_json(),
    };
    let report = json!({
        "hyprland_permission_enforcement": option,
        "session_locked": locked,
        "screencopy": screencopy,
        "virtual_pointer": input,
        "virtual_keyboard": keyboard,
        "server_input_default": "enabled",
        "read_only_escape_hatch": "hyprharness mcp --read-only",
        "audit_log": harness.audit_path(),
    });
    print_report(&report, as_json);
    Ok(())
}

fn check_value<T: serde::Serialize>(result: crate::Result<T>) -> Value {
    match result {
        Ok(value) => json!({"ok": true, "value": value}),
        Err(error) => error.as_json(),
    }
}

fn print_report(report: &Value, as_json: bool) {
    if as_json {
        println!(
            "{}",
            serde_json::to_string(report).expect("report JSON serialization")
        );
    } else {
        println!(
            "{}",
            serde_json::to_string_pretty(report).expect("report JSON serialization")
        );
    }
}

fn default_observation_path() -> PathBuf {
    std::env::var_os("XDG_RUNTIME_DIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir)
        .join("hyprharness-observe.png")
}
