# Verij Progress

## Overview
Verij is a nested workspace and session manager for the Zellij terminal multiplexer. It provides a real-time sidebar displaying all active Zellij sessions and tabs, and enables switching between sessions within an isolated Workspace pane.

---

## Completed Milestones

### Milestone 1 — End-to-End Smoke Test (`da0c493`)
- [x] **1.1** Added `wasm32-wasip1` build configuration in `.cargo/config.toml`.
- [x] **1.2** Created standardized `Makefile` with targets: `build`, `build-plugin`, `build-cli`, `check`, `clean`, `run`.
- [x] **1.3** Created base KDL layout `layouts/verij.kdl` with sidebar and borderless workspace panes.
- [x] **1.4** Converted `verij-plugin` to a binary WASM crate with periodic 1s polling (`get_session_list()`) and duplicate filtering to stream real-time updates over Zellij native pipes.
- [x] **1.5** Fixed CLI pipe reader handshake deadlock and added automatic reconnection on pipe break.
- [x] **1.6** Implemented remote session tab switching via `zellij --session <target> action go-to-tab <N>`.
- [x] **1.7** Implemented workspace pane detection and seamless session switching (sending `SIGTERM` to existing `zellij attach` process, pre-focusing workspace pane to avoid modal hangs, and attaching new target).
- [x] **1.8** Implemented responsive TUI resizing handling (`CEvent::Resize`).

### Milestone 2 — `verij start` & `verij attach` (`9102666`)
- [x] **2.1** `verij start` subcommand (`verij-cli`):
  - Validates session state against `zellij list-sessions`. Auto-attaches if already running (unless `--no-attach` passed).
  - Resolves plugin WASM binary and `verij` binary paths automatically.
  - Dynamically builds KDL layout and launches Zellij (`-n` when nested inside Zellij, `-l` when outside).
- [x] **2.2** Dynamic KDL layout generator (`verij-cli/src/layout.rs`):
  - Programmatic KDL builder embedding sidebar width, binary path, and plugin location.
  - Resolves `verij_plugin.wasm` via CLI flag, `VERIJ_PLUGIN_PATH`, executable sibling paths, dev build targets, user data, or system share paths.
  - Writes layout to runtime cache directory (`/run/user/<uid>/verij` or `~/.cache/verij`).
- [x] **2.3** Session naming configuration:
  - Added `-s` / `--session-name` (default `verij`).
  - Collision check against existing active sessions with diagnostic errors.
- [x] **2.4** `verij attach` subcommand (`verij-cli/src/session.rs`):
  - Direct attach to existing session by name (`zellij attach <session_name>`).
  - Optional `-c` / `--create` flag to create session if not running.
  - Informative error displaying active sessions when target not found.
- [x] **2.5** Updated `Makefile`:
  - `make run` executes `target/release/verij start`.

### Milestone 3 — `verij-types` Shared Crate (`bc09457`)
- [x] **3.1** Created library crate `verij-types` in workspace (`[lib]`, zero OS/WASM dependencies, serde-only).
- [x] **3.2** Extracted shared wire & model types:
  - `SessionSnapshot`, `TabSnapshot`, `active_tab()` helper.
  - `VERIJ_EVENTS_PIPE` constant ("verij_events").
  - Serialization roundtrip unit tests.
- [x] **3.3** Updated `verij-plugin` and `verij-cli`:
  - Added `verij-types` dependency to both crates.
  - Removed duplicate struct and constant definitions in `verij-plugin/src/main.rs` and `verij-cli/src/pipe_reader.rs`.
- [x] **3.4** Verified cross-compilation:
  - Clean compilation for both `wasm32-wasip1` and native target with 0 warnings.
  - Workspace test suite passing.

### Milestone 4 — TUI Polish
- [x] **4.1** Scrolling: Persistent `ListState` in `AppState` providing virtual viewport scrolling. Extended navigation (`PageUp`, `PageDown`, `Home`/`g`, `End`/`G`, `Ctrl+u`, `Ctrl+d`).
- [x] **4.2** Session count badge in title: `Verij (N sessions)`.
- [x] **4.3** Tab activity indicator: Inlined active tab (`[tab-name]`) and tab count on collapsed session rows; active/inactive dots on tab rows.
- [x] **4.4** Animated empty state: Braille spinner (`⠋ ⠙ ⠹...`) with tick-based loop animation while connecting to plugin.
- [x] **4.5** Mouse support: Click to select item, double-click (<400ms) to attach, mouse wheel up/down to scroll list.
- [x] **4.6** Dynamic resizing: `CEvent::Resize` with terminal clearing and redrawing (from M1).
- [x] **4.7** Tree collapse/expand: `Space` / `Tab` toggles folding; `h`/`Left` collapses or jumps to parent; `l`/`Right` expands; cursor preserves logical session.

---

## Active Milestone: Milestone 5 — Workspace Persistence

**Goal:** Named workspaces with saved session metadata and automatic resurrection.

### Tasks
- [ ] **5.1** Config format: `~/.config/verij/workspaces.toml` schema for workspace definitions (`[[workspace]]`).
- [ ] **5.2** `verij workspace new <name>`: Create workspace entry and initialize host session.
- [ ] **5.3** `verij workspace list`: List all saved workspaces and their live/offline status.
- [ ] **5.4** `verij workspace delete <name>`: Kill host session and prune configuration entry.
- [ ] **5.5** Auto-restore: Re-create offline sessions on `verij start`.
