# Verij Progress

Verij currently provides a working nested-workspace prototype for Zellij. The host session, sidebar, distributed state agents, Workspace attachment lifecycle, and session switching are implemented and tested.

## Implemented

### Host And CLI

- `verij start` creates or attaches to a prefixed host session.
- `verij attach` attaches to an existing host session or creates one with `--create`.
- `verij ui` runs the Ratatui sidebar.
- Generated KDL embeds the native CLI path, plugin path, sidebar width, Verij pane, and Workspace pane.
- Plugin path resolution supports CLI argument, environment variable, executable-relative, development, user, and system paths.
- Config helpers provide `verij config init` and `verij config path`.

### Sidebar And Navigation

- Live session and tab tree with filesystem-backed updates.
- Session filtering by configurable host prefix.
- Keyboard navigation, paging, folding, mouse selection, double-click actions, help view, and resize handling.
- Active session and active tab styling.
- New inner session creation from the sidebar.

### Distributed State

- One headless WASM agent runs in each inner session.
- Agents export atomic JSON snapshots to `/tmp/verij/states/`.
- Snapshots include tabs, active pane title, and optional connected-client compatibility data.
- CLI watcher aggregates snapshots and prunes stale state files.
- Old snapshots without optional fields remain readable.

### Workspace Switching

- Inception Switch uses `verij_control` and native Zellij `switch_session`.
- Tab navigation uses targeted `go-to-tab` actions.
- Workspace pane title follows configured session, tab, and pane values.
- Conditional pane formats omit empty tab/pane separators.
- Workspace attachment uses `VERIJ_WORKSPACE_SESSION` plus a host marker file.
- Detach cleanup, restart recovery, and legacy pane-title recovery avoid nested duplicate attaches.

### Inner Session Initialization

- Fake-PTY initialization avoids Zellij 0.45 no-viewport layout failures.
- Creation waits for `zjstatus` readiness instead of relying only on a fixed delay.
- Plugin fallback launches the agent when automatic layout loading does not produce state.

## Verification

Current workspace checks:

```bash
cargo check --workspace
cargo test --workspace
```

The repository currently passes both checks. Release artifacts are built with:

```bash
make dev
```

## Limitations

- Verij assumes its sidebar and Workspace pane use the generated two-pane host layout.
- Custom layouts with different geometry may require compatible pane placement.
- The Workspace marker represents Verij-controlled attachment; arbitrary external manipulation of the right pane is outside the supported flow.
- State files are runtime data under `/tmp/verij/`, not durable workspace metadata.

## Roadmap

- Define durable workspace configuration in `~/.config/verij/workspaces.toml`.
- Add `verij workspace new <name>`.
- Add `verij workspace list` with live/offline state.
- Add `verij workspace delete <name>` and cleanup behavior.
- Restore configured inner sessions automatically when starting a workspace.
- Add more end-to-end tests for host restart, manual detach, multiple inner clients, and custom layouts.
