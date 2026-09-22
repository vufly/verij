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

### Milestone 2 — `verij start` & `verij attach`
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

---

## Active Milestone: Milestone 3 — `verij-types` Shared Crate

**Goal:** Eliminate dual wire-type definitions between `verij-plugin` (WASM) and `verij-cli` (Native).

### Tasks
- [ ] **3.1** Create new library crate `verij-types` in workspace:
  - Pure Rust library (`[lib]`) with zero OS or WASM dependencies.
  - Lightweight serde dependencies (`serde`, `serde_json`).
- [ ] **3.2** Extract shared snapshots & models into `verij-types`:
  - `SessionSnapshot`, `TabSnapshot`, and any protocol messages.
- [ ] **3.3** Update `verij-plugin` and `verij-cli`:
  - Reference `verij-types` as a workspace dependency.
  - Remove duplicate struct definitions in `verij-plugin/src/main.rs` and `verij-cli/src/pipe_reader.rs`.
- [ ] **3.4** Verify cross-compilation:
  - Ensure cargo resolver cleanly builds for both `wasm32-wasip1` and native targets with zero drift.
