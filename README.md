# Ember

Notification daemon for i3/X11. Implements the
[Freedesktop Desktop Notifications](https://specifications.freedesktop.org/notification-spec/) specification,
renders toast windows directly on X11 without GTK or Qt, persists history in SQLite,
and exposes a Unix socket IPC interface with a built-in CLI client.

> **GitHub description:** Notification daemon for i3/X11 — Freedesktop-compliant, pure-Rust, with per-app rules, history, keyboard shortcuts, and a built-in IPC client.

---

## Features

- Registers as `org.freedesktop.Notifications` on the session D-Bus
- Pure-Rust X11 rendering via `x11rb` + Cairo/Pango — no compositor required
- Persistent notification history in SQLite (WAL mode, async writes)
- Per-app rules: mute, urgency override, custom timeout
- Global keyboard shortcuts via `XGrabKey` (fully configurable)
- Notification grouping with badge counter
- Do-not-disturb mode, togglable via hotkey or `ember ctl`
- Config hot-reload — changes to `config.toml` are applied within 5 seconds
- Built-in `ember ctl` CLI for querying and controlling the running daemon
- `ember install` generates and registers a systemd user service
- Action hook: executes `~/.config/ember/action-hook.sh` when a non-default action is invoked

---

## Requirements

Runtime:
- `cairo` + `pangocairo`
- `dbus`
- `wmctrl`

Build:
- Rust 1.80+ (edition 2024)
- `cargo`

Optional:
- `noto-fonts` — default toast font
- `eww` — bar widget integration via IPC

---

## Installation

### From source

```sh
cargo build --release
./target/release/ember install
systemctl --user daemon-reload
systemctl --user enable --now ember.service
```

`ember install` writes `~/.config/systemd/user/ember.service` pointing at the current binary.

### Arch Linux (AUR)

```sh
cd pkg/
makepkg -si
```

The PKGBUILD is in `pkg/PKGBUILD`. Adjust the `url` and `sha256sums` fields before publishing.

---

## Configuration

The daemon loads config from `~/.config/ember/config.toml`.
A commented default is installed to `/usr/share/ember/default.toml` (or found at `config/default.toml` in the source tree).

```toml
[daemon]
socket_path = ""       # default: $XDG_RUNTIME_DIR/ember.sock
history_db  = ""       # default: $XDG_DATA_HOME/ember/history.db
max_history = 500

[dnd]
enabled = false

[toast]
position     = "top-right"    # top-right | top-left | bottom-right | bottom-left
animation    = "slide-right"  # slide-right | slide-left | slide-down | slide-up | fade | fade-slide
width        = 440
max_visible  = 5
timeout_normal   = 5000       # ms; 0 = never expire
timeout_critical = 0
timeout_low      = 3000

[toast.theme]
bg_normal  = "#282828"
fg         = "#ebdbb2"
font       = "Noto Sans 11"
# ... (see config/default.toml for full theme keys)

[shortcuts]
focus_next      = "ctrl+grave"
focus_prev      = "ctrl+shift+grave"
dismiss_focused = "ctrl+Delete"
invoke_default  = "ctrl+Return"
open_center     = "super+n"
toggle_dnd      = "super+shift+n"
clear_all       = "ctrl+shift+Delete"
```

### Per-app rules

```toml
[[app]]
name       = "spotify"
mute       = true

[[app]]
name       = "firefox"
timeout_ms = 8000
urgency    = "low"
```

---

## CLI — ember ctl

`ember ctl` connects to the running daemon's IPC socket and sends a single command.

```
ember ctl [--config PATH] COMMAND [ARGS]
```

| Command | Description |
|---|---|
| `state` | Active notifications, unread count, DND flag |
| `groups` | Active notifications grouped by application |
| `dismiss ID` | Dismiss a notification by ID |
| `clear` | Dismiss all active notifications |
| `delete ID` | Remove a single entry from history |
| `history [-n N] [--offset O]` | Query history (default: last 20) |
| `clear-history` | Delete all history records |
| `search QUERY` | Full-text search over history |
| `dnd on\|off\|toggle` | Control do-not-disturb |
| `mark-read` | Reset unread badge counter to zero |
| `reply ID TEXT` | Send an inline reply to a notification |
| `subscribe` | Stream live events from the daemon (newline-delimited JSON) |

All responses are JSON. Example:

```sh
$ ember ctl state
{"dnd":false,"focused_id":null,"notifications":[],"status":"state","unread":0}

$ ember ctl dnd on
{"enabled":true,"status":"dnd"}

$ ember ctl history -n 3
{"items":[...],"status":"history","total":42}
```

---

## IPC protocol

The socket is a Unix domain stream socket at `$XDG_RUNTIME_DIR/ember.sock`.
Commands are newline-terminated JSON objects:

```json
{"cmd": "get_state"}
{"cmd": "dismiss", "id": 12}
{"cmd": "history", "limit": 20, "offset": 0}
{"cmd": "search", "query": "firefox"}
{"cmd": "toggle_dnd"}
```

---

## Action hook

When a non-default action is invoked (e.g. via keyboard shortcut `ctrl+1`), ember executes:

```sh
~/.config/ember/action-hook.sh <app_name> <notification_id> <action_key>
```

The script is optional. If absent, ember silently continues.

---

## Keyboard shortcuts

All shortcuts are defined in `[shortcuts]` in `config.toml` and use `XGrabKey` (works without focus).

Default bindings:

| Shortcut | Action |
|---|---|
| `ctrl+grave` | Focus next notification |
| `ctrl+shift+grave` | Focus previous notification |
| `ctrl+Delete` | Dismiss focused notification |
| `ctrl+Return` | Invoke default action |
| `ctrl+1` / `ctrl+2` / `ctrl+3` | Invoke action 1 / 2 / 3 |
| `super+n` | Open notification center |
| `super+shift+n` | Toggle do-not-disturb |
| `ctrl+shift+Delete` | Clear all notifications |

---

## Architecture

```
D-Bus (zbus)          IPC socket (Unix)       XGrabKey listener
      |                      |                       |
      +-------------- Engine (tokio) ----------------+
                             |
                    +--------+--------+
                    |                 |
              SQLite store      Toast renderer
              (sqlx, WAL)     (x11rb + Cairo/Pango)
```

- `src/core/engine.rs` — central state machine, routes commands and events
- `src/dbus/` — zbus server implementing `org.freedesktop.Notifications`
- `src/ipc/` — Unix socket server, line-delimited JSON protocol
- `src/toast/` — X11 toast window lifecycle, animation, rendering
- `src/store/` — SQLite-backed history and in-memory active notification store
- `src/input/` — keyboard (`XGrabKey`) and mouse event handling
- `src/ctl/` — synchronous IPC CLI client (`ember ctl`)
- `src/config.rs` — TOML config loading and hot-reload

---

## Development

```sh
# Run tests
cargo test

# Check compilation
cargo check

# Run daemon in foreground (debug logging)
RUST_LOG=debug cargo run

# Smoke-test IPC (requires running daemon)
./scripts/smoke-ipc.sh
```

---

## License

MIT
