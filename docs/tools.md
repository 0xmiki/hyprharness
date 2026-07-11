# MCP tool reference

Hyprharness exposes thirteen tools over MCP stdio. Every tool has a JSON Schema, structured JSON output, a JSON text fallback, annotations for Codex approval decisions, and an audit record.

## Recommended workflow

For reliable computer use, agents should follow this loop:

1. `list_windows` to obtain exact stable window identifiers.
2. `focus_window` when the intended application is not already focused.
3. `observe_desktop` immediately before coordinate-based actions.
4. `move_pointer` before `click` or `scroll`.
5. Supply `expected_window_id` to keyboard tools whenever practical.
6. Use `wait` after navigation, submission, animation, or asynchronous work.
7. `observe_desktop` again to verify the result.

Never reuse coordinates from an old screenshot after focus, layout, workspace, scale, or window geometry changes.

## Coordinate system

Pointer inputs use Hyprland global logical coordinates. The monitor metadata returned by `observe_desktop` includes:

- Global logical origin and dimensions.
- Captured PNG pixel dimensions.
- Scale and transform.

At scale `1.0`, image pixels and logical coordinates normally match. At other scales or transforms, calculate actions using the returned logical geometry—not raw image dimensions.

## Observation

### `observe_desktop`

Input:

```json
{ "monitor": "eDP-1" }
```

`monitor` is optional and defaults to the focused active monitor. The result contains PNG `ImageContent` plus metadata: capture time, coordinate-system description, monitor geometry, cursor, active window, byte count, MIME type, and SHA-256. Image bytes are not stored in the audit log.

### `get_cursor`

Input: `{}`.

Returns the global logical point, containing monitor name, and capture time.

### `list_windows`

Input: `{}`.

Returns mapped Hyprland clients with `stableId`, address, class/title, PID, geometry, workspace, monitor, visibility, input acceptance, fullscreen, XWayland, and focus state. Prefer `stableId` for subsequent focus and keyboard calls.

## Pointer and focus

### `move_pointer`

```json
{ "x": 1390, "y": 582 }
```

- `motion` accepts `natural`, `smooth`, or `instant` and defaults to `natural`.
- Omit `duration_ms` for automatic distance-aware timing between 220 and 1200 ms.
- An explicit duration accepts `0..3000`; `0` preserves immediate/teleport behavior.
- `natural` combines minimum-jerk acceleration/deceleration with a subtle deterministic Bézier curve. Its curve is kept inside the destination monitor when the move starts there.
- `smooth` uses the same eased timing on a perfectly straight path.
- `instant` emits one immediate Hyprland cursor command and ignores automatic timing.
- Animated moves emit at most 60 Hyprland cursor IPC updates per second, deduplicate identical integer points, and finish at the exact coordinate. This cadence is designed for clean 60 FPS screen recordings.
- The destination must fall inside an enabled, powered monitor.
- Returns original, requested, and final positions, resolved duration, effective motion profile, emitted step count, and target monitor.

For a deliberately paced product-demo move:

```json
{ "x": 1390, "y": 582, "motion": "natural", "duration_ms": 900 }
```

For a straight UI test or an immediate diagnostic move:

```json
{ "x": 1390, "y": 582, "motion": "smooth", "duration_ms": 400 }
```

```json
{ "x": 1390, "y": 582, "motion": "instant" }
```

### `click`

```json
{ "button": "left", "count": 1, "interval_ms": 120 }
```

- Buttons: `left`, `middle`, `right`.
- Count: `1..3`, default `1`.
- Inter-click interval: `40..1000` ms, default `120`.
- Clicking occurs at the current pointer position. Observe and move first.

### `point_and_click`

```json
{
  "x": 1390,
  "y": 582,
  "motion": "natural",
  "duration_ms": 900,
  "settle_ms": 300,
  "button": "left",
  "count": 1,
  "guard": {
    "focused_window_id": "180000b1",
    "workspace_id": 2
  }
}
```

This composite tool is preferred for recorded demos:

1. Validate all movement, pause, click, and guard arguments before moving.
2. Follow the natural minimum-jerk path, which decelerates to zero at the exact target.
3. Hold the pointer still for `settle_ms` (`300` ms by default, `0..2000`).
4. Recheck optional focus/workspace guards immediately before clicking.
5. Click while retaining the same global action lock, so no other input can interleave.

Movement and click fields use the same limits/defaults as `move_pointer` and `click`. The result contains the complete movement result, requested and measured settling time, and click result. If the user moves the pointer, the session locks, or a guard changes during the settling pause, clicking is refused.

### `focus_window`

```json
{ "window_id": "180000b1" }
```

`window_id` must exactly match a mapped window's `stableId` or address from `list_windows`. Hyprharness resolves the identifier before dispatch, asks Hyprland to focus the exact address, then verifies the result. It returns the previous and newly focused window.

### `switch_workspace`

```json
{ "workspace_id": 3 }
```

Focuses a positive numeric Hyprland workspace through compositor IPC, without simulating a keyboard shortcut. The server reads the previous focused workspace, dispatches the workspace focus, verifies the resulting focused monitor, and returns both workspace references plus the monitor name. Named, relative, special, and arbitrary dispatcher expressions are deliberately not accepted.

### `scroll`

```json
{ "direction": "down", "amount": 3 }
```

- Directions: `up`, `down`, `left`, `right`.
- Amount: `1..20` discrete wheel steps, default `3`.
- The event targets the surface under the current pointer, not merely the focused window.
- The cursor must be on an active monitor.

## Keyboard

Keyboard events use `wtype`, which connects through Wayland's virtual-keyboard protocol. Hyprharness passes fixed argument vectors and never invokes a shell.

### `press_key`

```json
{
  "key": "l",
  "modifiers": ["ctrl"],
  "repeat": 1,
  "expected_window_id": "18000066"
}
```

Supported modifiers are `ctrl`, `alt`, `shift`, and `super`. Duplicate modifiers are rejected.

Supported keys:

- Any single ASCII letter or digit.
- `F1` through `F12`.
- `enter`, `escape`, `tab`, `space`, `backspace`, `delete`, `insert`.
- `left`, `right`, `up`, `down`, `home`, `end`, `page_up`, `page_down`.
- `minus`, `equal`, `comma`, `period`, `slash`, `semicolon`, `apostrophe`, `bracket_left`, `bracket_right`, `backslash`, `grave`.

`repeat` accepts `1..20` and defaults to `1`. When `expected_window_id` is supplied, the call fails with `FOCUS_MISMATCH` if focus is already different immediately before injection.
Single-letter key names are normalized to lowercase; add the `shift` modifier for an uppercase key event. Use `type_text` when entering literal text rather than shortcuts.

### `type_text`

```json
{
  "text": "Hello from Codex",
  "interval_ms": 5,
  "expected_window_id": "180000b1"
}
```

- Accepts UTF-8 text with `1..2000` characters and at most 8192 bytes.
- `interval_ms` accepts `0..50` and defaults to `5`.
- Total requested typing delay cannot exceed 30 seconds.
- The focused window must report that it accepts input.
- The returned result and audit entry contain length and SHA-256, not the text itself.

Text can still become visible in the target application and screenshot observations. Redaction applies only to hyprharness logs and tool results.

`expected_window_id` is a stale-focus guard, not an atomic focus lock. Keep delayed typing short and re-observe if another process or the user may change focus during injection.

## Synchronization

### `wait`

```json
{ "duration_ms": 1500 }
```

Waits `0..30000` ms and returns requested and measured elapsed time. It remains available in `--read-only` mode. Use it instead of shell `sleep` so the workflow stays within the MCP audit trail.

## Choreography

### `run_sequence`

`run_sequence` executes a deterministic list of typed actions under one server-side action lock. It eliminates MCP/model round trips between rehearsed demo actions; it does not allow Codex to inspect and reason about intermediate screens.

```json
{
  "steps": [
    {
      "action": "focus_window",
      "window_id": "180000b1"
    },
    {
      "action": "point_and_click",
      "x": 1450,
      "y": 180,
      "motion": "natural",
      "duration_ms": 900,
      "settle_ms": 300,
      "guard": {
        "focused_window_id": "180000b1",
        "workspace_id": 2
      }
    },
    {
      "action": "wait",
      "duration_ms": 800
    },
    {
      "action": "switch_workspace",
      "workspace_id": 3
    }
  ],
  "observe_at_end": true,
  "final_monitor": "eDP-1"
}
```

Supported step actions and their action-specific fields:

- `move_pointer`: `x`, `y`, optional `motion`, optional `duration_ms`.
- `click`: optional `button`, `count`, and `interval_ms`.
- `point_and_click`: `x`, `y`, optional movement/click fields, and optional `settle_ms` (default `300`).
- `focus_window`: `window_id`.
- `scroll`: `direction`, optional `amount`.
- `press_key`: `key`, optional `modifiers` and `repeat`.
- `type_text`: `text`, optional `interval_ms`.
- `wait`: `duration_ms`.
- `switch_workspace`: positive numeric `workspace_id`.

Every step may include a `guard` with `focused_window_id`, `workspace_id`, or both. Guards are evaluated immediately before the step. For sequence `press_key` and `type_text`, `focused_window_id` is also passed into the keyboard backend's final stale-focus check.

Sequence constraints:

- `1..32` steps.
- At most 45 seconds of conservatively planned movement, typing delay, click intervals, and waits.
- Each `wait` step is limited to 10 seconds.
- The full plan is validated before the first action.
- Existing per-tool bounds, lock checks, monitor bounds, focus checks, and rate limits still apply.
- The first guard or action failure stops the sequence; there is no continue-on-error mode.
- No other pointer, focus, workspace, or keyboard action can interleave while it runs.
- `final_monitor` is accepted only with `observe_at_end: true`; omitted means the focused monitor.

Successful output contains a sequence ID, status, UTC timestamps, actual total duration, completed-step count, and a result/timing record for every step. When final observation is requested, the result also includes desktop metadata and PNG `ImageContent`.

On failure, the MCP error code is `SEQUENCE_FAILED`. Its structured details contain the partial sequence report, including successful steps and the failed step's stable error code/message. GUI changes already completed cannot be rolled back.

Use separate tool calls instead when an intermediate result is uncertain, a dialog may appear, or the next coordinate depends on a fresh screenshot.

## Safety failures

Important stable error codes include:

- `SESSION_LOCKED`: Hyprland reports a locked session.
- `INPUT_DISABLED`: the server was started with `--read-only`.
- `OUT_OF_BOUNDS`: pointer destination/current pointer is outside active monitor bounds.
- `WINDOW_NOT_FOUND`: an exact stable ID/address did not resolve.
- `FOCUS_FAILED`: Hyprland did not focus the resolved target.
- `FOCUS_MISMATCH`: the expected keyboard target is not focused.
- `SEQUENCE_GUARD_FAILED`: a sequence step's expected focus/workspace no longer matches.
- `SEQUENCE_FAILED`: a fail-fast sequence stopped; partial results are in error details.
- `WORKSPACE_SWITCH_FAILED`: Hyprland did not report the requested workspace as focused.
- `POINTER_MOVED_DURING_SETTLE`: the pointer left a `point_and_click` target before clicking.
- `WINDOW_REJECTS_INPUT`: the focused window does not accept input.
- `KEYBOARD_UNAVAILABLE`: `wtype` or virtual-keyboard support failed.
- `RATE_LIMITED`: a per-action one-minute limit was exceeded.
- `INVALID_ARGUMENT`: an input violated its documented bounds or allowlist.

State-changing actions fail closed when the audit log cannot be opened or written.

## Example Codex prompts

Read-only inspection:

```text
Use only hyprharness MCP tools. List the windows, observe the focused monitor, and describe the active application. Do not perform input.
```

Browser address entry:

```text
Use hyprharness to list windows and focus the browser by stableId. Press Ctrl+L with that stableId as expected_window_id, type https://example.com with the same expected_window_id, press Enter, wait 1500 ms, and observe the result.
```

Scrolling:

```text
Observe the focused monitor, move the pointer over the center of the document content, scroll down three steps, wait 300 ms, and observe again. Do not click.
```

Recorded product demo:

```text
Use hyprharness for a recorded-style demo. Observe before every coordinate action. Use point_and_click with natural motion, an explicit 700-1000 ms movement, and a 250-400 ms settling pause when highlighting controls. Wait for each transition and re-observe to verify it. Never use instant movement.
```

Deterministic recorded segment:

```text
Use hyprharness to observe and plan a deterministic demo segment. Then call run_sequence once with guarded natural pointer moves, brief waits before clicks, and observe_at_end enabled. Do not batch any step whose result must be inspected before deciding what to do next.
```
