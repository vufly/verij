# Verij Progress

Verij currently provides a working nested-session host for Zellij. Each host session has a persistent sidebar and Workspace pane; the Workspace pane remembers its last inner-session attachment independently from other hosts.

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
- Durable host metadata records the last inner session attached to each host.
- Live and exited Zellij hosts and inner sessions are distinguished during attach.
- Exited hosts and inner sessions can be resumed through Zellij session resurrection.

### Inner Session Initialization

- Fake-PTY initialization avoids Zellij 0.45 no-viewport layout failures.
- Creation waits for a visible layout plugin instead of relying only on a fixed delay.
- Plugin fallback launches the agent when automatic layout loading does not produce state.

## Verification

Current workspace checks:

```bash
cargo check --workspace
cargo test --workspace
```

The repository currently passes both checks. Focused tests cover host-keyed metadata and live/exited session classification. Release artifacts are built with:

```bash
make dev
```

## Limitations

- Verij assumes its sidebar and Workspace pane use the generated two-pane host layout.
- Custom layouts with different geometry may require compatible pane placement.
- The Workspace marker represents Verij-controlled attachment; arbitrary external manipulation of the right pane is outside the supported flow.
- State files and host markers are runtime data under `/tmp/verij/`, not durable attachment metadata.
- Durable attachment metadata is stored under the XDG config directory in `hosts.toml`.

## Roadmap

- Add end-to-end coverage for host resurrection and inner-session resurrection.
- Add coverage for manual Workspace detach, multiple inner clients, multiple hosts, and custom layouts.
- Improve status reporting for missing or externally deleted remembered inner sessions.

The previous named-workspace lifecycle proposal was superseded. A Verij workspace is the host Zellij session plus its current Workspace-pane attachment; `verij start` and `verij attach` remain the lifecycle commands.
