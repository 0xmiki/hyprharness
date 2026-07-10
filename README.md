# hyprharness

`hyprharness` is a local, safety-focused MCP server that lets Codex observe and operate a Hyprland desktop. It exposes typed desktop tools instead of an unrestricted shell and keeps a durable audit trail of every request.

The first release deliberately focuses on a trustworthy perception-and-pointer loop:

- `observe_desktop`
- `get_cursor`
- `list_windows`
- `move_pointer`
- `click`

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
    ├── lock/bounds/rate safety policy
    └── rotating JSONL action audit
```

Hyprland queries and cursor movement use its compositor socket directly. Screenshots use `grim` with a fixed argument list, never a shell. Clicks use `zwlr_virtual_pointer_v1`, so no privileged `uinput` daemon is required.

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

Build the release binary, then register its absolute path:

```bash
codex mcp add hyprharness -- /absolute/path/to/hyprharness/target/release/hyprharness mcp
codex mcp list
```

Use `/mcp` inside Codex to confirm that the five tools are available. Codex CLI, the IDE extension, and the desktop app share the same host MCP configuration.

For unattended local demos, the equivalent `config.toml` entry is:

```toml
[mcp_servers.hyprharness]
command = "/absolute/path/to/hyprharness/target/release/hyprharness"
args = ["mcp"]
required = true
startup_timeout_sec = 10
tool_timeout_sec = 60
default_tools_approval_mode = "approve"

[mcp_servers.hyprharness.tools.move_pointer]
approval_mode = "approve"

[mcp_servers.hyprharness.tools.click]
approval_mode = "approve"
```

Put this in `~/.codex/config.toml`, or in a trusted project's `.codex/config.toml`. Auto-approval is powerful: only enable it on a machine and session you are comfortable allowing Codex to operate.

## MCP tool contracts

### `observe_desktop`

Input: `{ "monitor"?: string }`. It captures the focused monitor by default and returns:

- PNG `ImageContent` with the cursor included.
- Structured metadata with monitor origin, logical dimensions, pixel dimensions, scale, transform, cursor, focused window, byte count, and SHA-256.

Pointer coordinates are always Hyprland global logical coordinates. Screenshot pixels can differ when display scaling is enabled.

### `get_cursor` and `list_windows`

`get_cursor` returns the global logical cursor position and containing monitor. `list_windows` returns mapped clients with Hyprland stable/address identifiers, title/class, PID, geometry, workspace, monitor, visibility, fullscreen, and focus state.

### `move_pointer`

Input: `{ "x": integer, "y": integer, "duration_ms"?: 0..2000 }`. Destinations outside all active monitor rectangles are rejected. Nonzero duration interpolates at approximately 60 Hz.

### `click`

Input: `{ "button"?: "left"|"middle"|"right", "count"?: 1..3, "interval_ms"?: 40..1000 }`. Defaults are a single left click and 120 ms interval.

## Safety

Pointer tools are armed by default for autonomous demos. Start a read-only server when you only want observation:

```bash
hyprharness mcp --read-only
```

Regardless of Codex approval settings, the trusted server:

- Denies pointer input while Hyprland reports the session locked.
- Rejects positions outside enabled, powered monitors.
- Limits movement requests to 300/minute and click events to 60/minute.
- Validates button, click-count, interval, and animation bounds.
- Exposes no command, executable, or shell-string arguments.
- Stops state-changing actions when the audit log is unavailable.

Terminate the `hyprharness` process or disable the MCP server in Codex as the immediate emergency stop.

## Audit log

The default path is `$XDG_STATE_HOME/hyprharness/audit.jsonl`, falling back to `~/.local/state/hyprharness/audit.jsonl`. Override it with `mcp --audit-log PATH`.

Records contain UTC timestamp, server/request IDs, tool, validated arguments, focused window address, before/after cursor positions, duration, success, and error code. Screenshot bytes are never logged—only monitor, dimensions, size, and hash. Files use mode `0600`, rotate at 10 MiB, and retain five archives.

## Diagnostics

```bash
hyprharness doctor [--json]
hyprharness observe [--monitor eDP-1] [--output /tmp/desktop.png]
hyprharness test-pointer [--distance 40]
hyprharness test-pointer --click --yes
hyprharness permissions [--json]
```

`test-pointer` restores the original cursor position. It will not emit a click unless both `--click` and `--yes` are supplied.

If Hyprland compositor permissions are enforced, permit the Nix-store `grim` binary or use Hyprland's interactive `ask` mode. `hyprharness permissions` reports the current compositor option and backend availability without changing configuration.

## Live tests

Live tests are ignored during the normal suite:

```bash
nix-shell --run 'cargo test --test live_hyprland observes_live_desktop -- --ignored --nocapture'
nix-shell --run 'cargo test --test live_hyprland moves_and_restores_live_pointer -- --ignored --nocapture'
```

The movement test clicks only when `HYPRHARNESS_LIVE_CLICK=1` is explicitly set.

## Roadmap

The service boundaries are ready for `focus_window`, scrolling, keyboard/text input, waits, recording, and telemetry. Those capabilities are intentionally not exposed in v1 so each can receive its own safety policy and audit treatment.

## References

- [Codex MCP documentation](https://developers.openai.com/codex/mcp/)
- [Official Rust MCP SDK](https://github.com/modelcontextprotocol/rust-sdk)
- [Hyprland IPC](https://wiki.hypr.land/IPC/)
- [Using hyprctl](https://wiki.hypr.land/Configuring/Using-hyprctl/)
- [Hyprland permissions](https://wiki.hypr.land/Configuring/Permissions/)

## License

MIT
