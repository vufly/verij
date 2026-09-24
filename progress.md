# Verij Progress

Verij currently provides a working nested-session host for Zellij. Each host session has a persistent sidebar and Workspace pane; the Workspace pane remembers its last inner-session attachment independently from other hosts.

## Implemented

### Host And CLI

- `verij start` creates or attaches to an exact-name registered host session.
- `verij attach` attaches to an existing host session or creates one with `--create`.
- `verij ui` runs the Ratatui sidebar.
- Host KDL comes from editable `~/.config/verij/verij.kdl`; runtime rendering inserts native CLI path and sidebar width, with no host state-export plugin.
- Plugin path resolution supports CLI argument, environment variable, executable-relative, development, user, and system paths.
- `verij config init` and first `verij start` create missing layout template without overwriting user edits; `verij config path` prints config path.
- Host sessions override pane-frame style via `[host]` (default `titles`); inner sessions inherit Zellij's default config unchanged.

### Sidebar And Navigation

- Live session and tab tree with filesystem-backed updates.
- Host filtering by cached registration, independent of session-name prefix.
- Sidebar `R` renames the current host while retaining Workspace attachment identity.
- Keyboard navigation, paging, folding, mouse selection, double-click actions, help view, and resize handling.
- Resize redraw avoids cursor-position queries, keeping sidebar alive when Zellij temporarily delays terminal replies.
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
- Separate `host-registry.toml` registers hosts even when they have no inner attachment.
- Live and exited Zellij hosts and inner sessions are distinguished during attach.
- Exited hosts and inner sessions can be resumed through Zellij session resurrection.
- Resurrection sanitizes stale suspended-command flags and waits for visible layout plugins before detaching.
- Host resurrection rewrites stale serialized Workspace-pane attach commands to the durable target.

### Inner Session Initialization

- Fake-PTY initialization avoids Zellij 0.45 no-viewport layout failures.
- Creation waits for the first tab's configured status plugin and a terminal pane to stabilize, then verifies the plugin survived fake-client detach.
- Plugin fallback launches the agent when automatic layout loading does not produce state.

## Verification

Current workspace checks:

```bash
cargo check --workspace
cargo test --workspace
```

The repository currently passes both checks. Focused tests cover host registration/rename, first-tab readiness, host-keyed metadata, and live/exited session classification. Release artifacts are built with:

```bash
make dev
```

## Limitations

- Verij assumes its sidebar and Workspace pane use the generated two-pane host layout.
- Custom layouts with different geometry may require compatible pane placement.
- The Workspace marker represents Verij-controlled attachment; arbitrary external manipulation of the right pane is outside the supported flow.
- State files and host markers are runtime data under `/tmp/verij/`, not durable attachment metadata.
- Durable attachment metadata is stored under the XDG config directory in `hosts.toml`.
- Zellij `serialize_pane_viewport` and scrollback serialization must be enabled to preserve terminal history.

## Roadmap

- Validate live host rename after restarting existing sidebar with new binary; local `🔷v` → `v` migration was done once outside the product.
- Add end-to-end coverage for host resurrection and inner-session resurrection.
- Add coverage for manual Workspace detach, multiple inner clients, multiple hosts, and custom layouts.
- Improve status reporting for missing or externally deleted remembered inner sessions.

The previous named-workspace lifecycle proposal was superseded. A Verij workspace is the host Zellij session plus its current Workspace-pane attachment; `verij start` and `verij attach` remain the lifecycle commands.
