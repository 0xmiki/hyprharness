# hyprharness

Native, auditable computer use for Codex on Hyprland.

`hyprharness` is a local Rust MCP server that gives Codex eyes and a carefully
bounded set of desktop controls. Codex can inspect the desktop, move and click
the pointer, type, focus windows, switch workspaces, and choreograph complete
product-demo sequences—without being given an unrestricted shell tool.

The server combines Hyprland IPC for desktop state and window management with a
persistent Wayland virtual pointer for smooth, efficient input. Every action is
validated, serialized, checked against the live desktop, and written to an
audit log.

| | |
|---|---|
| Platform | Linux, Hyprland, Wayland |
| Protocol | MCP over local stdio |
| Observation | `grim` screenshots plus structured desktop metadata |
| Pointer input | Persistent `zwlr_virtual_pointer_v1` connection |
| Keyboard input | `wtype` |
| Demo motion | Natural or smooth 60 Hz trajectories, settling, and sequences |
| Safety | Typed tools, bounds checks, lock detection, guards, limits, audit |

The current server exposes 13 tools. It is intentionally local and
Hyprland-specific; it does not require root, a privileged daemon, or `/dev/uinput`.

## Quick start

### 1. Requirements

You need:

- a running Hyprland session;
- a compositor with the wlr virtual-pointer protocol (Hyprland provides it);
- `grim` for screenshots;
- `wtype` for keyboard input; and
- Codex CLI with MCP support.

The included [`shell.nix`](shell.nix) supplies the Rust toolchain and runtime
utilities used by the project.

### 2. Build and check the session

Run project commands through the Nix shell:

```bash
nix-shell --run 'cargo build --release'
nix-shell --run 'target/release/hyprharness doctor'
```

`doctor` checks the Hyprland and Wayland environment, IPC access, required
programs, screenshot capture, and the virtual-pointer protocol.

### 3. Register it with Codex

Enter the project shell so the registration captures the runtime `PATH` that
contains `grim` and `wtype`:

```bash
nix-shell
codex mcp add hyprharness \
  --env PATH="$PATH" \
  -- /absolute/path/to/hyprharness/target/release/hyprharness \
  mcp \
  --audit-log /absolute/path/to/hyprharness/.hyprharness/audit.jsonl
```

Then add the live session variables to the parent server table in Codex's
configuration:

```toml
[mcp_servers.hyprharness]
command = "/absolute/path/to/hyprharness/target/release/hyprharness"
args = [
  "mcp",
  "--audit-log",
  "/absolute/path/to/hyprharness/.hyprharness/audit.jsonl",
]
cwd = "/absolute/path/to/hyprharness"
env_vars = [
  "XDG_RUNTIME_DIR",
  "HYPRLAND_INSTANCE_SIGNATURE",
  "WAYLAND_DISPLAY",
]
required = true
startup_timeout_sec = 10
tool_timeout_sec = 60
default_tools_approval_mode = "approve"

[mcp_servers.hyprharness.env]
# Keep the PATH written by `codex mcp add` here.
PATH = "/nix/store/.../bin:..."
```

The distinction matters after a reboot:

- `PATH` can be stored because it comes from this project's Nix environment.
- `HYPRLAND_INSTANCE_SIGNATURE`, `WAYLAND_DISPLAY`, and `XDG_RUNTIME_DIR` must
  be inherited through `env_vars`; their values describe the current login
  session and can change after a reboot.

Do not copy a current Hyprland instance signature into
`[mcp_servers.hyprharness.env]`. A stale signature is the usual cause of
`connection closed: initialize response` after restarting the computer.

Restart Codex after changing its configuration or replacing the release binary.
Codex starts `hyprharness mcp` itself; do not run a second copy manually. `/mcp`
should list the 13 tools. `Auth: Unsupported` is normal for a local stdio server
and does not mean registration failed.

### 4. Smoke test

Start with observation, then a harmless pointer move:

```text
Use the hyprharness MCP server.
1. Call get_cursor.
2. Call list_windows and summarize the visible windows.
3. Call observe_desktop on the focused monitor and describe what you see.
4. Move the pointer 200 logical pixels to the right using natural motion.
5. Call get_cursor again and verify the final position.
Do not click or type.
```

For a first guarded click, ask Codex to observe immediately before acting and
name the exact visible target.

## What it can do

### Observe

- Capture the focused monitor as PNG.
- Return the active window, monitor geometry, cursor position, capture hash,
  and coordinate-system metadata with the image.
- Query mapped windows and workspaces as structured JSON.
- Read the current cursor position without taking a screenshot.

### Act

- Move through an entire natural, smooth, or instant pointer trajectory.
- Move, decelerate, settle at a target, verify the target is still valid, and
  click as one guarded action.
- Click, scroll, press key chords, and type text.
- Focus a window by stable Hyprland address and switch numeric workspaces.

### Choreograph

- Execute up to 32 typed actions in one deterministic `run_sequence` call.
- Insert brief waits between actions for animations and video pacing.
- Guard actions against the expected window and workspace.
- Fail fast, preserve per-step results, and optionally capture a final frame.

### Contain and audit

- Reject input while the session is locked or the server is read-only.
- Validate coordinates against the live monitor layout.
- Serialize actions so separate requests cannot interleave pointer or keyboard
  events.
- Apply rate limits and record every state-changing attempt in rotating JSONL.

## Architecture

```text
Codex CLI
    │ MCP JSON-RPC over stdio
    ▼
hyprharness
    │
    ├── Hyprland IPC — state and control plane
    │   ├── query cursor
    │   ├── query monitors/windows
    │   ├── query lock state
    │   ├── focus window
    │   ├── switch workspace
    │   └── verify final cursor position
    │
    ├── Persistent Wayland virtual pointer — input plane
    │   ├── move complete 60 Hz trajectory
    │   ├── click
    │   └── scroll
    │
    ├── grim — screenshot capture
    ├── wtype — keyboard and text input
    ├── safety policy + action lock
    └── rotating JSONL audit log
```

The split between state and input is deliberate. Hyprland IPC is authoritative
for compositor state, window operations, lock state, and final verification.
Pointer events travel through Wayland's native virtual-pointer protocol.

For a move, the harness calculates and validates the complete trajectory first,
then sends that path to one long-lived pointer actor. The actor emits absolute
Wayland motion frames at 60 updates per second, flushes each frame, performs one
final synchronization, and leaves the connection alive for the next move,
click, or scroll. This avoids spawning a process or opening a Hyprland IPC
round-trip for every animation frame. Hyprland IPC is queried afterward to
verify the actual final cursor position.

The common action lifecycle is:

```text
observe → plan → validate → act → verify → audit
```

Coordinates use Hyprland global logical space. The Wayland backend maps that
space across the complete active monitor layout, including negative monitor
origins and scaled outputs. Screenshot pixels can differ from logical
coordinates when monitor scaling is enabled; observation results include both
geometries so callers can transform them correctly.

## MCP tool catalog

| Tool | Purpose |
|---|---|
| `observe_desktop` | Capture one monitor and return PNG plus desktop metadata. |
| `get_cursor` | Return the current global logical cursor position. |
| `list_windows` | Return mapped Hyprland windows and workspace metadata. |
| `move_pointer` | Move with `natural`, `smooth`, or `instant` motion. |
| `click` | Click the current position one to three times. |
| `point_and_click` | Move, decelerate, settle, guard, verify, and click atomically. |
| `focus_window` | Focus an exact window address from `list_windows`. |
| `scroll` | Emit bounded vertical scrolling through the persistent pointer. |
| `press_key` | Press a key or modified chord through `wtype`. |
| `type_text` | Type bounded UTF-8 text with an optional per-character interval. |
| `wait` | Pause for a bounded duration. |
| `switch_workspace` | Switch to a numeric Hyprland workspace. |
| `run_sequence` | Preflight and execute a guarded multi-action demo plan. |

Full argument schemas, limits, examples, and result shapes are documented in
[`docs/tools.md`](docs/tools.md).

## Demo-quality pointer movement

### Natural and smooth trajectories

`move_pointer` supports three profiles:

- `natural` is the default. It uses minimum-jerk easing and a restrained curved
  path so acceleration and deceleration look intentional on video.
- `smooth` follows a direct eased path.
- `instant` sends the final coordinate immediately for diagnostics and precise
  non-demo work.

Animated movement runs at 60 updates per second to align cleanly with common
60 fps screen recording. The harness chooses a distance-aware duration when one
is not supplied; callers can also request a duration from 0 to 3000 ms.

### Settled point-and-click

`point_and_click` is the preferred tool for product demos. It keeps the move and
click under one action lock and performs this sequence:

1. Validate the target, expected window, and expected workspace.
2. Move with the requested trajectory and decelerate into the target.
3. Pause at the target for the settling interval (300 ms by default, up to
   2000 ms).
4. Recheck focus and workspace guards.
5. Confirm the cursor did not move during settling.
6. Click and return the observed final position.

That visible pause makes the target legible to viewers and prevents a click if
the desktop changed underneath the plan.

### Deterministic sequences

`run_sequence` removes tool-call latency between closely related demo actions.
The complete plan is validated before its first side effect, then every step is
executed in order under one action lock. A failed step stops the sequence.

Conceptually, a sequence can look like this:

```json
{
  "expected_window_id": "0x1234abcd",
  "expected_workspace": 3,
  "capture_final": true,
  "steps": [
    {
      "action": "point_and_click",
      "x": 420,
      "y": 310,
      "motion": "natural",
      "duration_ms": 700,
      "settle_ms": 350,
      "button": "left"
    },
    { "action": "wait", "duration_ms": 500 },
    { "action": "type_text", "text": "Hello from hyprharness", "interval_ms": 25 },
    { "action": "press_key", "key": "enter" }
  ]
}
```

Use the exact schema returned by MCP discovery; the example emphasizes the
workflow rather than replacing the tool schema. A sequence accepts 1–32 steps,
has a maximum planned duration of 45 seconds, and limits each wait step to 10
seconds. Each result includes step timing and correlation metadata for the
audit trail.

## Safety model

`hyprharness` is a narrow trusted process between the model and the desktop. It
does not expose arbitrary programs, shell arguments, or raw compositor commands.

Before an input action, the server checks:

- the session is not locked;
- input is allowed (the server was not started with `--read-only`);
- the target lies inside an enabled, powered monitor;
- the requested button, key, text, repeat, interval, and duration are bounded;
- any expected window or workspace guard still matches; and
- the relevant rate limit has capacity.

Current per-minute limits are:

| Action class | Limit |
|---|---:|
| Pointer moves | 300 |
| Clicks | 60 |
| Window focus/workspace operations | 120 |
| Scroll actions | 240 |
| Keyboard events | 2000 |

All input is serialized by a shared action lock. A sequence owns that lock for
its complete run, and `point_and_click` owns it across motion, settling, and the
click. State-changing actions fail closed if their audit record cannot be
written.

For observation-only use:

```bash
nix-shell --run 'target/release/hyprharness mcp --read-only'
```

The immediate emergency stop is to terminate the MCP process or disable the
server in Codex.

## Audit trail

The default log is:

```text
$XDG_STATE_HOME/hyprharness/audit.jsonl
```

When `XDG_STATE_HOME` is unset, it falls back to
`~/.local/state/hyprharness/audit.jsonl`. The path can be overridden with
`--audit-log`.

The log:

- is created with mode `0600`;
- rotates at 10 MiB and retains five archives;
- records timestamps, session and request IDs, tool name, validated arguments,
  active-window context, cursor positions, duration, and success or error;
- correlates sequence parent and child records with sequence and step IDs;
- stores screenshot metadata and hashes, never PNG bytes; and
- redacts typed text, retaining only its length and hash.

## Human CLI and diagnostics

The MCP server is the primary runtime, but the binary also includes a small
diagnostic CLI:

```bash
# Show commands
nix-shell --run 'target/release/hyprharness --help'

# Check dependencies, environment, IPC, and protocols
nix-shell --run 'target/release/hyprharness doctor'

# Capture the focused monitor and print metadata
nix-shell --run 'target/release/hyprharness observe'

# Explain runtime permissions and safety behavior
nix-shell --run 'target/release/hyprharness permissions'

# Move right and back; no click
nix-shell --run 'target/release/hyprharness test-pointer --distance 40'

# Explicit live click test
nix-shell --run 'target/release/hyprharness test-pointer --click --yes'
```

The click test requires both flags so it cannot be triggered accidentally.

## Development and verification

Keep development commands inside `shell.nix` so build-time and runtime
dependencies match:

```bash
nix-shell --run 'cargo fmt --check'
nix-shell --run 'cargo check --all-targets'
nix-shell --run 'cargo test'
nix-shell --run 'cargo clippy --all-targets -- -D warnings'
nix-shell --run 'cargo build --release'
```

Useful targeted tests include:

```bash
nix-shell --run 'cargo test pointer_trajectory'
nix-shell --run 'cargo test sequence'
nix-shell --run 'cargo test safety'
```

Live desktop tests should be deliberate: inspect the desktop first, use a
non-destructive target, and prefer `point_and_click` with window/workspace
guards over a bare click.

## Troubleshooting

### Codex shows `Tools: (none)` after a reboot

Remove hard-coded values for `HYPRLAND_INSTANCE_SIGNATURE`, `WAYLAND_DISPLAY`,
and `XDG_RUNTIME_DIR` from `[mcp_servers.hyprharness.env]`. Put their names in
the parent table's `env_vars` array, restart Codex from the live Hyprland
session, and check `/mcp` again.

### MCP initialization closes before listing tools

Run:

```bash
nix-shell --run 'target/release/hyprharness doctor'
```

Also confirm the configured binary exists and is current, the audit directory
is writable, and the server's `PATH` contains `grim` and `wtype`. Server logs
must go to stderr; stdout is reserved for MCP JSON-RPC.

### `/mcp` says `Auth: Unsupported`

That is expected. `hyprharness` is a local process connected over stdio, so it
does not perform an HTTP authentication flow.

### Pointer input is denied

Check whether the screen is locked, the server uses `--read-only`, the target is
outside the active monitor layout, or a live window/workspace guard no longer
matches.

### A screenshot target and logical coordinate do not line up

Image pixels and Hyprland logical coordinates differ on scaled monitors. Use the
monitor and coordinate-system metadata returned by `observe_desktop` rather
than treating screenshot pixels as global coordinates directly.

## Scope and direction

The project currently focuses on reliable, explainable primitives for local
product demos and desktop automation. The strong foundation is already in
place: observation, native pointer input, keyboard input, window/workspace
control, guarded actions, deterministic sequences, and a correlated audit log.

Natural extensions include drag and drop, recording lifecycle tools, richer
telemetry, reusable named demo scripts, and interactive permission prompts.
They should preserve the same design rule: small typed capabilities, live-state
validation, deterministic execution, and complete auditing.

## Further reading

- [`docs/tools.md`](docs/tools.md) — complete MCP tool reference
- [Codex MCP documentation](https://developers.openai.com/codex/mcp/)
- [Hyprland IPC documentation](https://wiki.hypr.land/IPC/)
- [Hyprland dispatchers](https://wiki.hypr.land/Configuring/Dispatchers/)
- [wlr virtual pointer protocol](https://wayland.app/protocols/wlr-virtual-pointer-unstable-v1)

## License

MIT
