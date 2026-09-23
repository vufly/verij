# Verij Architecture

This document describes the current Verij implementation: Zellij host sessions, a Ratatui sidebar, distributed WASM agents in inner sessions, filesystem state synchronization, native in-place session switching, and durable host-local attachment recovery.

## Design

Verij separates host orchestration from inner-session state:

- Each host session owns navigation and one Workspace pane.
- Each inner Zellij session owns its own tabs and panes.
- A WASM agent runs inside every inner session and exports only that session's state.
- The sidebar aggregates state files instead of polling all Zellij sessions through one plugin.
- Session switching is executed by the agent inside the currently attached session.
- The last inner session attached to each host is persisted independently.

This avoids the former global-polling, cross-session pipe, process-killing, and PTY-manipulation design.

## Host Topology

```text
┌──────────────────────────── Host Session: "_vj_<name>" ────────────────────────┐
│ Tab: Verij Host                                                                │
│ ┌───────────────────────┐  ┌─────────────────────────────────────────────────┐ │
│ │ Verij pane            │  │ Workspace pane                                  │ │
│ │ verij ui              │  │ zellij attach <active_session>                  │ │
│ │                       │  │                                                 │ │
│ │ Sessions              │  │ ┌──────── Attached inner session: "backend" ──┐ │ │
│ │ ├─ backend [editor]   │  │ │ Tab 0: editor [active]  Tab 1: server       │ │ │
│ │ │  ├─ editor          │  │ │                                             │ │ │
│ │ │  └─ server          │  │ │ Active pane: shell                          │ │ │
│ │ └─ frontend           │  │ │ verij-plugin.wasm                           │ │ │
│ │                       │  │ └─────────────────────────────────────────────┘ │ │
│ └───────────────────────┘  └─────────────────────────────────────────────────┘ │
│                                                                                │
│ Hidden host plugin pane                                                        │
└────────────────────────────────────────────────────────────────────────────────┘
                         ▲                         ▲
                         │                         │
                         │                         └─ Inception Switch
                         │                            switch:<target>
                         │
                         └─ Filesystem watcher
                            /tmp/verij/states/*.json
```

The prefix defaults to `_vj_` and is configurable through `[workspace].prefix`. The generated layout uses:

- Sidebar name: `Verij`
- Workspace pane name: `Workspace`
- Host tab name: `Verij Host`
- Configurable sidebar width, default `25%`

The host plugin is not the source of inner-session state. It exists as part of the host layout, while each inner session has its own distributed agent.

## Inner Sessions

An inner session is an ordinary Zellij session such as `backend`, `frontend`, or `infra`. Its agent:

1. Subscribes to `SessionUpdate`, `TabUpdate`, and `PaneUpdate`.
2. Resolves its own session from `SessionUpdate`.
3. Captures ordered tabs, active tab state, active pane title, and optional client-count compatibility data.
4. Writes an atomic snapshot to `/tmp/verij/states/<session>.json`.
5. Listens for `verij_control` pipe commands.

The agent requests `ReadApplicationState`, `ChangeApplicationState`, and `ReadCliPipes` permissions.

## State Synchronization

The native CLI starts `fs_watcher` for `/tmp/verij/states/`.

1. The watcher reads existing JSON files at startup.
2. Create, modify, and remove events trigger an aggregate read.
3. Invalid, stale, and host-prefixed files are ignored or pruned.
4. Snapshots are sorted by session name and sent to the TUI event loop.
5. The TUI reconciles snapshots, preserves logical cursor position, and rebuilds the flattened session/tab tree.

Host sessions are filtered using the configured prefix, not a hard-coded historical prefix.

### Snapshot Schema

```json
{
  "name": "backend",
  "is_current": true,
  "active_pane": "shell",
  "connected_clients": 1,
  "tabs": [
    {
      "name": "editor",
      "position": 0,
      "is_active": true
    },
    {
      "name": "server",
      "position": 1,
      "is_active": false
    }
  ]
}
```

`active_pane` and `connected_clients` use serde defaults so snapshots from older agents remain readable. `connected_clients` is informational compatibility data; Workspace attachment is not inferred from global inner-session client counts.

## Workspace Attachment

The TUI tracks the session attached to each host Workspace pane separately from inner-session metadata. The durable record is keyed by canonical host session name, so multiple hosts can remember different inner sessions while sharing the global sidebar.

Durable records live at `$XDG_CONFIG_HOME/verij/hosts.toml`, or `~/.config/verij/hosts.toml` when `XDG_CONFIG_HOME` is unset:

```toml
[hosts."_vj_project"]
last_session = "backend"
```

The record stores attachment intent. `/tmp/verij/states/` and `/tmp/verij/workspace-<host>.session` remain runtime state and are reconciled against the durable record and current pane title.

### Attach

When no Workspace session is known, Verij focuses the right pane and writes a shell command equivalent to:

```bash
export VERIJ_WORKSPACE_SESSION=backend
stty sane
zellij attach backend
unset VERIJ_WORKSPACE_SESSION
rm -f /tmp/verij/workspace-<host>.session
```

The marker file is written by the CLI before attach and removed when the attach command returns. It lets the TUI recover attachment after its own process restarts. Native switches update the marker directly because the original attach process remains alive during an in-place switch.

### Recovery And Detach

On state updates, the TUI:

- Restores `active_session` from the host marker when the session exists.
- Falls back to the current Workspace pane title for hosts created before markers existed.
- Clears active state and restores the configured default pane name when the marker disappears.
- Never issues another `zellij attach` for a session already tracked as attached.
- Persists the selected inner session for the current host after a successful switch.
- Preserves durable attachment intent when the runtime Workspace marker disappears.

This distinction matters because an inner session may have other clients. A global client count cannot tell whether the host Workspace pane is the client currently attached.

## Session And Tab Switching

### Session Switch

For a target different from the current Workspace session:

1. TUI records the target as active.
2. TUI sends `switch:<target>` to the current inner agent:

   ```bash
   zellij -s <old> pipe --name verij_control -- switch:<target>
   ```

3. The inner agent calls `switch_session(Some(target))` through the Zellij plugin API.
4. Zellij switches the attached client in place.
5. TUI focuses the host Workspace pane and renames it using `pane_format`.

If there is no active Workspace session, Verij performs the initial attach fallback instead.

### Tab Switch

Selecting a tab in the active session runs:

```bash
zellij --session <session> action go-to-tab <position + 1>
```

The agent observes the resulting `TabUpdate` and `PaneUpdate` events. The Workspace pane title is then refreshed with the active tab and pane values.

## Inner Session Creation

Creating a session from the TUI uses a fake PTY because Zellij 0.45 can discard layouts created without an attached client.

1. Verij runs `script` around `zellij attach -c <name>` with `ZELLIJ` nesting variables removed.
2. The configured `~/.config/zellij/layouts/default.kdl` is passed as the default layout when present.
3. Verij waits for a visible layout plugin pane to appear, with a bounded timeout.
4. The fake client detaches while the session remains alive.
5. Inner pane frames are set to `full`.
6. If the agent state file does not appear, Verij launches the configured WASM plugin as a floating, unfocused fallback.
7. The Workspace pane switches to the new session.

The readiness poll replaces a blind delay so the first tab's layout plugin panes are not lost to startup timing.

## Session Resurrection

Zellij serializes sessions by default. `zellij list-sessions` marks exited sessions with `EXITED`; attaching with `--force-run-commands` restores their serialized layout and commands without waiting for the command banner.

Verij uses this in two stages:

1. An exited host is resurrected before its sidebar is attached.
2. The host's durable last-session record is loaded. A live inner session is attached normally; an exited inner session is resurrected with `--force-run-commands` before attachment.

Zellij resurrection restores runtime layout state. It does not replace Verij's host-to-inner attachment record, and it does not provide a global workspace model.

## Pane Naming

`WorkspaceConfig::format_pane_name` supports:

- `{session}`
- `{tab}` and `{active_tab}`
- `{pane}` and `{active_pane}`
- `{if variable}...{else}...{endif}` optional sections

Default:

```toml
pane_format = "{session}{if tab} | {tab}{endif}{if pane} | {pane}{endif}"
```

The formatter emits no dangling separator when an active tab or pane has no title. Conditional blocks are simple and non-nested.

## Crate Responsibilities

```text
verij/
|-- verij-cli/
|   `-- src/
|       |-- main.rs          CLI commands and host startup
|       |-- layout.rs        Generated host KDL and path resolution
|       |-- session.rs       Zellij session queries and exec helpers
|       |-- fs_watcher.rs    State directory watcher and aggregation
|       |-- actions.rs       Attach, marker, pane, tab, and switch actions
|       `-- tui/              Event loop, state, and rendering
|-- verij-plugin/
|   `-- src/main.rs          Distributed WASM agent
|-- verij-types/
|   `-- src/lib.rs           Shared serde wire types and constants
|-- layouts/verij.kdl       Static host layout reference
|-- Makefile                Build and verification targets
`-- docs/                    Project documentation
```

## Known Boundaries

- Workspace attachment tracking assumes Verij controls the host Workspace pane.
- Custom Zellij layouts can change pane geometry and may not provide a pane immediately to the right of the sidebar.
- Older plugins may omit optional snapshot fields until rebuilt.
- Zellij resurrection depends on session serialization being enabled in the user's Zellij configuration.
- Missing or externally deleted remembered inner sessions require user selection from the sidebar.
