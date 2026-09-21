# Verij Architecture

> **Context anchor** — This document is the authoritative reference for the Verij system design. All implementation decisions should trace back to the principles and data-flow described here.

---

## 1. Overview

**Verij** is a workspace and session manager for the [Zellij](https://zellij.dev) terminal multiplexer. It adds an IDE-like navigation layer on top of Zellij's native session → tab → pane hierarchy using a *Nested Session* model.

The user experience goal: a persistent sidebar that lists all Zellij sessions and their tabs, allowing instant context-switching without leaving the terminal.

---

## 2. Nested Session Architecture

```
┌─────────────────────────────────────── Host Session (Zellij) ─────┐
│  Tab 0 (only tab)                                                  │
│  ┌──────────────────────┐  ┌─────────────────────────────────────┐ │
│  │  Left Pane           │  │  Right Pane                         │ │
│  │  verij ui            │  │  <Active Inner Session>             │ │
│  │  (Ratatui TUI)       │  │  (embedded via zellij attach)       │ │
│  │                      │  │                                     │ │
│  │  Sessions            │  │  ┌── Inner Session: "project-a" ──┐ │ │
│  │  ├─ project-a   ←──────── │  Tab 0: editor                 │ │ │
│  │  │  ├─ editor        │  │  │  Tab 1: git                   │ │ │
│  │  │  └─ git           │  │  └────────────────────────────────┘ │ │
│  │  └─ project-b        │  │                                     │ │
│  └──────────────────────┘  └─────────────────────────────────────┘ │
└────────────────────────────────────────────────────────────────────┘
```

### 2.1 Host Session

- Created and owned by Verij.
- Contains **exactly 1 tab** and **2 panes**:
  - **Left pane**: runs `verij ui` — the Ratatui TUI sidebar.
  - **Right pane**: displays the currently active Inner Session by running `zellij attach --create <session-name>` (or equivalent action).
- The Host Session is the outer frame that the user always sees.

### 2.2 Inner Sessions

- Standard Zellij sessions — the user's actual workspaces.
- Users create/manage tabs and panes inside Inner Sessions as usual.
- They are *attached* into the Host Session's right pane for display; they continue to exist independently.

---

## 3. Crate Structure

```
verij/                          (Cargo workspace root)
├── Cargo.toml                  (workspace manifest)
├── docs/
│   └── ARCHITECTURE.md         (this file)
├── verij-cli/                  (crate 1: native binary)
│   ├── Cargo.toml
│   └── src/
│       ├── main.rs             (clap CLI entry point)
│       ├── tui/
│       │   ├── mod.rs          (Ratatui event loop)
│       │   ├── state.rs        (AppState: sessions + cursor)
│       │   └── render.rs       (2-level tree widget rendering)
│       ├── pipe_reader.rs      (spawns `zellij pipe` subprocess, reads stdout)
│       └── actions.rs          (dispatches `zellij action` shell commands)
└── verij-plugin/               (crate 2: WASM plugin)
    ├── Cargo.toml
    └── src/
        └── main.rs             (ZellijPlugin impl)
```

### 3.1 `verij-cli`

| Attribute      | Value                                                                                    |
| -------------- | ---------------------------------------------------------------------------------------- |
| Crate type     | `[[bin]]`                                                                                |
| Target         | native host platform (x86\_64-linux, etc.)                                               |
| Key deps       | `ratatui`, `crossterm`, `clap`, `tokio`, `serde`, `serde_json`                          |
| Responsibilities | CLI parsing, TUI rendering, pipe subprocess, state reconciliation, action dispatch     |

### 3.2 `verij-plugin`

| Attribute      | Value                                                                                    |
| -------------- | ---------------------------------------------------------------------------------------- |
| Crate type     | `[lib]` with `crate-type = ["cdylib"]`                                                  |
| Target         | `wasm32-wasip1` (Zellij WASM runtime)                                                   |
| Key deps       | `zellij-tile`, `serde`, `serde_json`                                                     |
| Responsibilities | Subscribe to Zellij events, serialize state snapshots, stream to CLI via named pipe   |

---

## 4. Communication Architecture: Zellij Native Pipes

Verij uses Zellij's built-in **Pipe system** as the IPC mechanism between the WASM plugin and the native TUI process. This avoids external sockets or files and stays entirely within Zellij's security model.

### 4.1 Data Flow (Full State Snapshot)

```
                   Zellij Core
                       │
         SessionUpdate / TabUpdate events
                       │
                       ▼
             ┌─────────────────┐
             │  verij-plugin   │  (WASM, headless)
             │  (ZellijPlugin) │
             │                 │
             │  pipe() ──────► saves pipe_name
             │  update() ────► serializes full snapshot
             │                 │
             └────────┬────────┘
                      │ cli_pipe_output(pipe_name, json)
                      │ (Zellij routes to stdout of…)
                      ▼
             ┌─────────────────┐
             │  zellij pipe    │  (subprocess in Host Session)
             │  --name         │
             │  verij_events   │
             └────────┬────────┘
                      │ stdout (line-delimited JSON)
                      ▼
             ┌─────────────────┐
             │  pipe_reader    │  (tokio task in verij-cli)
             │  (BufReader)    │
             └────────┬────────┘
                      │ mpsc::Sender<AppState>
                      ▼
             ┌─────────────────┐
             │  Ratatui Loop   │  (verij-cli main thread)
             │  (event_loop)   │
             │  replaces state │
             │  triggers render│
             └─────────────────┘
```

### 4.2 Plugin Side: `verij-plugin`

1. **`load()`**: Calls `subscribe(&[EventType::SessionUpdate])` and `request_permission(&[PermissionType::ReadApplicationState, PermissionType::ReadCliPipes])`.
2. **`pipe(pipe_message: PipeMessage)`**: When `pipe_message.name == "verij_events"`, stores the name in state as `active_pipe_name: Option<String>`. Returns `false` (headless, no render).
3. **`update(Event::SessionUpdate(sessions, _))`**: If `active_pipe_name` is set, maps `Vec<SessionInfo>` into a compact `Vec<SessionSnapshot>`, serializes to single-line JSON, calls `cli_pipe_output(&pipe_name, &json)`. Returns `false`.

**Why Full Snapshot (not deltas)?**

- Eliminates ordering bugs and missed-event drift.
- Zellij's `SessionUpdate` already provides a complete list; delta tracking would duplicate that work.
- The TUI treats each message as a complete replacement of its `AppState.sessions` vec — a "dumb view" that is trivially correct.

### 4.3 CLI Side: `verij-cli`

1. **`verij ui`** subcommand launches the TUI.
2. A `tokio::task` spawns `zellij pipe --name verij_events` as a subprocess and holds a `BufReader` over its stdout.
3. Each complete line is parsed as `Vec<SessionSnapshot>`. On success, the snapshot is sent through an `mpsc::channel`.
4. The Ratatui event loop selects on both terminal input events and the mpsc receiver:
   - **Input event**: navigate cursor, trigger actions.
   - **Channel message**: replace `app_state.sessions`, trigger re-render.

---

## 5. Data Schemas (JSON Wire Format)

The plugin serializes into a JSON array of `SessionSnapshot` objects, one per active session:

```json
[
  {
    "name": "project-a",
    "is_current": false,
    "tabs": [
      { "name": "editor", "position": 0, "is_active": true },
      { "name": "git",    "position": 1, "is_active": false }
    ]
  },
  {
    "name": "project-b",
    "is_current": false,
    "tabs": [
      { "name": "main", "position": 0, "is_active": true }
    ]
  }
]
```

These types are defined in both crates independently (no shared crate needed at bootstrap stage) and kept in sync manually. A future `verij-types` crate can unify them.

---

## 6. TUI Layout & Interaction

### 6.1 Visual Layout

```
╭─ Verij ──────────────────╮
│ Sessions                 │
│                          │
│ ▶ project-a              │  ← Level 1: Session (selected)
│     editor               │  ← Level 2: Tab (active tab)
│     git                  │
│   project-b              │
│     main                 │
│                          │
│ j/k ↕  Enter: attach     │
╰──────────────────────────╯
```

### 6.2 Navigation Model

The cursor is a flat index over a linearized list of **tree nodes**. A `TreeNode` enum has two variants:

```rust
enum TreeNode {
    Session { name: String, index: usize },
    Tab { session_index: usize, tab_index: usize, name: String },
}
```

The flat list is rebuilt on every state reconciliation. Cursor movement wraps within the list.

### 6.3 Key Bindings

| Key          | Action                                         |
| ------------ | ---------------------------------------------- |
| `j` / `↓`   | Move cursor down                               |
| `k` / `↑`   | Move cursor up                                 |
| `Enter`      | Attach selected session to Host right pane     |
| `q` / `Esc` | Quit `verij ui`                                |

---

## 7. Action Dispatch

On `Enter`, the TUI reads the selected `TreeNode`:

- If `Session`: runs `zellij action switch-session <name>` via `std::process::Command`.
- If `Tab`: runs `zellij action switch-session <name>` then `zellij action go-to-tab <position+1>`.

> **Note**: The exact zellij CLI commands for attaching a session into a specific pane of the Host Session may require additional investigation of `zellij action focus-pane` and layout manipulation APIs. The initial implementation uses `switch-session` as a baseline that is confirmed to work.

---

## 8. Permissions & Security

The WASM plugin requires the following Zellij permissions (declared in the plugin's KDL configuration):

| Permission              | Reason                                            |
| ----------------------- | ------------------------------------------------- |
| `ReadApplicationState`  | Receive `SessionUpdate` events                    |
| `ReadCliPipes`          | Call `cli_pipe_output` to write to the CLI pipe   |

---

## 9. Build & Deployment

### 9.1 Plugin Build

```bash
# Build the WASM plugin
cargo build -p verij-plugin --target wasm32-wasip1 --release
# Output: target/wasm32-wasip1/release/verij_plugin.wasm
```

### 9.2 CLI Build

```bash
cargo build -p verij-cli --release
# Output: target/release/verij
```

### 9.3 Plugin Registration (KDL Layout)

The plugin must be loaded in the Host Session's Zellij layout. Example KDL snippet:

```kdl
layout {
    tab {
        pane split_direction="vertical" {
            pane size=30 {
                plugin location="file:target/wasm32-wasip1/release/verij_plugin.wasm" {
                    // Plugin configuration (headless, no UI)
                }
            }
            pane command="verij" {
                args "ui"
            }
        }
    }
}
```

> **Note**: The plugin is loaded in a minimal pane or as a background plugin. The TUI runs in a separate pane. The right pane for the Inner Session is opened dynamically by the TUI's action dispatcher.

---

## 10. Development Roadmap

| Phase | Deliverable                                    | Status     |
| ----- | ---------------------------------------------- | ---------- |
| 0     | `docs/ARCHITECTURE.md`                         | ✅ Done    |
| 1     | Cargo workspace + `Cargo.toml` files            | ✅ Done    |
| 2     | `verij-plugin`: ZellijPlugin + pipe + snapshot  | ✅ Done    |
| 3     | `verij-cli`: pipe reader + Ratatui event loop   | ✅ Done    |
| 4     | Action dispatcher (`Enter` → `zellij action`)   | ✅ Done    |
| 5     | Host Session launcher (`verij start`)           | 🔲 Future  |
| 6     | KDL layout generation                           | 🔲 Future  |
| 7     | `verij-types` shared crate                      | 🔲 Future  |
| 8     | Workspace persistence (TOML config)             | 🔲 Future  |
| 9     | Agent activity monitoring pane                  | 🔲 Future  |
