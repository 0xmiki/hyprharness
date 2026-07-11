# hyprharness

`hyprharness` is a local, safety-focused MCP server that lets Codex observe and operate a Hyprland desktop. It exposes typed desktop tools instead of an unrestricted shell and keeps a durable audit trail of every request.

The server exposes a complete observe/focus/input loop:

- `observe_desktop`
- `get_cursor`
- `list_windows`
- `move_pointer`
- `click`
- `point_and_click`
- `focus_window`
- `scroll`
- `press_key`
- `type_text`
- `wait`
- `switch_workspace`
- `run_sequence`

It also includes `doctor`, `observe`, `test-pointer`, and `permissions` commands for human diagnostics.

## Architecture

```text
Codex CLI
    │ MCP JSON-RPC over stdio
    ▼
hyprharness
    ├── Hyprland UNIX socket IPC
    ├── grim screenshot capture
    ├── wlr virtual-pointer input
    ├── wtype virtual-keyboard input
    ├── lock/bounds/rate safety policy
    └── rotating JSONL action audit
```

Hyprland IPC is the state/control plane for cursor queries, monitor/window discovery, focus, lock state, and workspace switching. Pointer movement, clicks, and scrolling share one persistent `zwlr_virtual_pointer_v1` Wayland device; complete 60 Hz trajectories are sent to its dedicated actor instead of opening a compositor socket for every frame. Screenshots use `grim` with a fixed argument list, never a shell, and keyboard input uses `wtype` with Wayland's virtual-keyboard protocol. No privileged `uinput` daemon is required.

## Build and verify

All project commands are intended to run through `shell.nix`:

```bash
nix-shell --run 'cargo build --release'
nix-shell --run 'cargo test --all-features'
nix-shell --run 'cargo clippy --all-targets --all-features -- -D warnings'
nix-shell --run 'cargo run -- doctor'
```

The runtime expects a live Hyprland session with `XDG_RUNTIME_DIR`, `HYPRLAND_INSTANCE_SIGNATURE`, and `WAYLAND_DISPLAY` inherited from Codex.

## Register with Codex

Build the release binary, then register its absolute path from inside `nix-shell` so the forwarded `PATH` contains both `grim` and `wtype`. The Hyprland/Wayland variables must also be forwarded to the stdio child:

```bash
codex mcp add hyprharness \
  --env PATH="$PATH" \
  -- /absolute/path/to/hyprharness/target/release/hyprharness \
  mcp --audit-log /absolute/path/to/hyprharness/.hyprharness/audit.jsonl
codex mcp list
```

Do not register `HYPRLAND_INSTANCE_SIGNATURE`, `WAYLAND_DISPLAY`, or `XDG_RUNTIME_DIR` with `--env`: Codex persists those literal values and the Hyprland signature changes after a reboot. Add `env_vars = ["XDG_RUNTIME_DIR", "HYPRLAND_INSTANCE_SIGNATURE", "WAYLAND_DISPLAY"]` to the parent `[mcp_servers.hyprharness]` section so they are forwarded from each current Codex session. Keep the Nix-shell `PATH` as the static `.env` value so `grim` and `wtype` remain discoverable.

Use `/mcp` inside a newly started Codex session to confirm that all thirteen tools are available. Codex launches the stdio server itself; do not run `hyprharness mcp` in a separate terminal. `Auth: Unsupported` is expected for a local stdio server.

For unattended local demos where Codex itself is launched from `nix-shell`, the equivalent fully dynamic `config.toml` entry is:

```toml
[mcp_servers.hyprharness]
command = "/absolute/path/to/hyprharness/target/release/hyprharness"
args = ["mcp", "--audit-log", "/absolute/path/to/hyprharness/.hyprharness/audit.jsonl"]
cwd = "/absolute/path/to/hyprharness"
env_vars = ["XDG_RUNTIME_DIR", "HYPRLAND_INSTANCE_SIGNATURE", "WAYLAND_DISPLAY", "PATH"]
required = true
startup_timeout_sec = 10
tool_timeout_sec = 60
default_tools_approval_mode = "approve"

[mcp_servers.hyprharness.tools.move_pointer]
approval_mode = "approve"

[mcp_servers.hyprharness.tools.click]
approval_mode = "approve"

[mcp_servers.hyprharness.tools.point_and_click]
approval_mode = "approve"

[mcp_servers.hyprharness.tools.focus_window]
approval_mode = "approve"

[mcp_servers.hyprharness.tools.scroll]
approval_mode = "approve"

[mcp_servers.hyprharness.tools.press_key]
approval_mode = "approve"

[mcp_servers.hyprharness.tools.type_text]
approval_mode = "approve"

[mcp_servers.hyprharness.tools.switch_workspace]
approval_mode = "approve"

[mcp_servers.hyprharness.tools.run_sequence]
approval_mode = "approve"
```

Put this in `~/.codex/config.toml`, or in a trusted project's `.codex/config.toml`. Auto-approval is powerful: only enable it on a machine and session you are comfortable allowing Codex to operate.

## MCP tools

| Tool | Purpose |
| --- | --- |
| `observe_desktop` | Return a focused/named monitor PNG plus coordinate metadata. |
| `get_cursor` | Read the global logical cursor position. |
| `list_windows` | List mapped windows and stable identifiers. |
| `move_pointer` | Move naturally, smoothly, or instantly to validated coordinates. |
| `click` | Emit bounded left, middle, or right clicks. |
| `point_and_click` | Decelerate to a target, settle visibly, then click atomically. |
| `focus_window` | Focus a mapped window by exact `stableId` or address. |
| `scroll` | Emit bounded discrete wheel steps at the pointer position. |
| `press_key` | Press a validated key/shortcut in the focused window. |
| `type_text` | Type bounded UTF-8 text with optional per-character delay. |
| `wait` | Pause for bounded UI navigation or asynchronous work. |
| `switch_workspace` | Focus a numeric Hyprland workspace through compositor IPC. |
| `run_sequence` | Execute a guarded, fail-fast demo choreography without inter-step latency. |

See [docs/tools.md](docs/tools.md) for complete inputs, outputs, key names, safety behavior, errors, and recommended agent workflows.

### Core observation and pointer tools

#### `observe_desktop`

Input: `{ "monitor"?: string }`. It captures the focused monitor by default and returns:

- PNG `ImageContent` with the cursor included.
- Structured metadata with monitor origin, logical dimensions, pixel dimensions, scale, transform, cursor, focused window, byte count, and SHA-256.

Pointer coordinates are always Hyprland global logical coordinates. Screenshot pixels can differ when display scaling is enabled.

#### `get_cursor` and `list_windows`

`get_cursor` returns the global logical cursor position and containing monitor. `list_windows` returns mapped clients with Hyprland stable/address identifiers, title/class, PID, geometry, workspace, monitor, visibility, fullscreen, and focus state.

#### `move_pointer`

Input: `{ "x": integer, "y": integer, "motion"?: "natural"|"smooth"|"instant", "duration_ms"?: 0..3000 }`. Destinations outside all active monitor rectangles are rejected.

`natural` is the default. It chooses a distance-aware duration (220–1200 ms), applies human-looking acceleration and deceleration, and follows a subtle bounded curve with at most 60 Wayland motion frames per second to align cleanly with 60 FPS recordings. `smooth` uses the same easing on a straight path. `instant`, or an explicit `duration_ms` of `0`, performs a single absolute Wayland move. Supply a nonzero duration when a demo needs exact pacing. Hyprland IPC verifies every final coordinate, and results identify the `wayland_virtual_pointer` backend.

#### `click`

Input: `{ "button"?: "left"|"middle"|"right", "count"?: 1..3, "interval_ms"?: 40..1000 }`. Defaults are a single left click and 120 ms interval.

#### `point_and_click`

Input: `{ "x": integer, "y": integer, "motion"?: "natural"|"smooth"|"instant", "duration_ms"?: 0..3000, "settle_ms"?: 0..2000, "button"?: "left"|"middle"|"right", "count"?: 1..3, "interval_ms"?: 40..1000, "guard"?: object }`.

This is the preferred recorded-demo action. It holds the global action lock while the pointer follows its eased path and decelerates to zero, pauses at the exact target for 300 ms by default, verifies the pointer stayed there, rechecks optional `focused_window_id`/`workspace_id` guards, and then clicks. Its result reports the movement, requested/measured settling time, and click.

### Window and input tools

- `focus_window`: `{ "window_id": string }`, using an exact `stableId` or address from `list_windows`.
- `scroll`: `{ "direction": "up"|"down"|"left"|"right", "amount"?: 1..20 }`. Scrolling occurs under the pointer.
- `press_key`: `{ "key": string, "modifiers"?: ["ctrl"|"alt"|"shift"|"super"], "repeat"?: 1..20, "expected_window_id"?: string }`.
- `type_text`: `{ "text": string, "interval_ms"?: 0..50, "expected_window_id"?: string }`. Supports UTF-8 through `wtype`.
- `wait`: `{ "duration_ms": 0..30000 }`.
- `switch_workspace`: `{ "workspace_id": integer >= 1 }`.

For keyboard safety, first call `list_windows`, focus the intended `stableId`, then pass the same ID as `expected_window_id`. This rejects stale focus immediately before injection; it cannot prevent an external focus change that happens during a long typing operation.

### Deterministic demo sequences

`run_sequence` executes 1–32 typed actions serially while holding the server's action lock. It is intended for rehearsed demo segments that do not require Codex to inspect an intermediate screen before choosing the next action. Plans are limited to 45 seconds, individual sequence waits to 10 seconds, and execution always stops on the first error.

```json
{
  "steps": [
    {
      "action": "point_and_click",
      "x": 1450,
      "y": 180,
      "duration_ms": 900,
      "settle_ms": 300,
      "button": "left",
      "guard": { "focused_window_id": "180000b1", "workspace_id": 2 }
    },
    { "action": "wait", "duration_ms": 800 },
    { "action": "switch_workspace", "workspace_id": 3 }
  ],
  "observe_at_end": true
}
```

The complete plan is statically checked before execution and each optional guard is checked again immediately before its step. A final observation can return both PNG content and structured metadata. Use separate MCP calls when the next action depends on interpreting an intermediate observation.

## Safety

Desktop input tools are armed by default for autonomous demos. Start a read-only server when you only want observation and waits:

```bash
hyprharness mcp --read-only
```

Regardless of Codex approval settings, the trusted server:

- Denies focus, pointer, and keyboard input while Hyprland reports the session locked.
- Rejects positions outside enabled, powered monitors.
- Limits movement, click, focus, scroll, and keyboard event rates independently.
- Validates button, click-count, scroll amount, key names, modifiers, repeats, text size, delays, and wait bounds.
- Serializes all input actions; `point_and_click` keeps movement, settling, guard recheck, and click indivisible.
- Preflights sequence size/duration and stops choreography on the first failed live guard or action.
- Optionally verifies the expected focused window immediately before keyboard injection.
- Exposes no command, executable, or shell-string arguments.
- Stops state-changing actions when the audit log is unavailable.

Terminate the `hyprharness` process or disable the MCP server in Codex as the immediate emergency stop.

## Audit log

The default path is `$XDG_STATE_HOME/hyprharness/audit.jsonl`, falling back to `~/.local/state/hyprharness/audit.jsonl`. Override it with `mcp --audit-log PATH`.

Records contain UTC timestamp, server/request IDs, tool, validated arguments, focused window address, before/after cursor positions, duration, success, and error code. Sequence parent/child records share a `sequence_id`, and child actions include `step_index`. Screenshot bytes are never logged—only monitor, dimensions, size, and hash. Typed text is also redacted in both standalone and sequence records: logs contain only character/byte counts, delay, and SHA-256. Files use mode `0600`, rotate at 10 MiB, and retain five archives.

## Diagnostics

```bash
hyprharness doctor [--json]
hyprharness observe [--monitor eDP-1] [--output /tmp/desktop.png]
hyprharness test-pointer [--distance 40]
hyprharness test-pointer --click --yes
hyprharness permissions [--json]
```

`doctor` and `permissions` now probe both the virtual pointer and virtual keyboard backends. `test-pointer` restores the original cursor position and will not emit a click unless both `--click` and `--yes` are supplied.

If Hyprland compositor permissions are enforced, permit the Nix-store `grim` binary or use Hyprland's interactive `ask` mode. `hyprharness permissions` reports the current compositor option and backend availability without changing configuration.

## Live tests

Live tests are ignored during the normal suite:

```bash
nix-shell --run 'cargo test --test live_hyprland observes_live_desktop -- --ignored --nocapture'
nix-shell --run 'cargo test --test live_hyprland moves_and_restores_live_pointer -- --ignored --nocapture'
nix-shell --run 'cargo test --test live_hyprland runs_and_audits_live_sequence -- --ignored --nocapture'
```

The live suite safely probes screenshots, focus-on-current-window, waits, keyboard availability, and reversible movement. Side-effectful input remains opt-in:

- `HYPRHARNESS_LIVE_CLICK=1` enables one click.
- `HYPRHARNESS_LIVE_SCROLL=1` enables a down/up scroll pair.
- `HYPRHARNESS_LIVE_KEYBOARD=1` enables a shortcut and text entry test in the focused window.

## Roadmap

The next capability layer is session recording and richer telemetry. The input interfaces are intentionally separate so future clipboard, drag, touch, and recording support can receive independent safety and audit policies.

## References

- [Codex MCP documentation](https://developers.openai.com/codex/mcp/)
- [Official Rust MCP SDK](https://github.com/modelcontextprotocol/rust-sdk)
- [Hyprland IPC](https://wiki.hypr.land/IPC/)
- [Using hyprctl](https://wiki.hypr.land/Configuring/Advanced-and-Cool/Using-hyprctl/)
- [Hyprland dispatchers](https://wiki.hypr.land/Configuring/Basics/Dispatchers/)
- [Hyprland permissions](https://wiki.hypr.land/Configuring/Permissions/)

## License

MIT
