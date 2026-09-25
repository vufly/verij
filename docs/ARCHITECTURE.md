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
┌──────────────────────────── Host Session: "<name>" ────────────────────────────┐
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
└────────────────────────────────────────────────────────────────────────────────┘
                         ▲                         ▲
                         │                         │
                         │                         └─ Inception Switch
                         │                            switch:<target>
                         │
                         └─ Filesystem watcher
                            /tmp/verij/states/*.json
```

Verij hosts use exact requested names, tracked in `host-registry.toml`. Generated layout uses:

- Sidebar name: `Verij`
- Workspace pane name: `Workspace`
- Host tab name: `Verij Host`
- Configurable sidebar width, default `25%`

The host layout needs no state-export plugin. Each inner session runs its own distributed agent.

`layouts/verij.kdl` is embedded in CLI as the starter template. On first `verij start` / `verij attach --create` or `verij config init`, Verij creates the XDG-aware `verij.kdl` next to `config.toml` if missing. New host startup reads that user-owned template, escapes and substitutes `{{verij_bin}}` and `{{sidebar_width}}`, and passes a rendered cache file to Zellij. CLI width overrides TOML width; explicit `--layout` bypasses template rendering. Existing live or exited hosts retain their Zellij layout on attach/resurrection; changing the template affects only newly created hosts. `verij recover [HOST]` re-renders the current template and replaces a live host's complete layout, restoring missing Sidebar or Workspace panes while discarding extra host-only panes and tabs.

Verij separates editable config from durable and disposable data: `$XDG_CONFIG_HOME/verij` (fallback `~/.config/verij`) contains `config.toml` and user layout; `$XDG_STATE_HOME/verij` (fallback `~/.local/state/verij`) contains host registry and durable attachments; `$XDG_CACHE_HOME/verij` (fallback `~/.cache/verij`) contains generated layouts and generated host Zellij KDL; `$XDG_RUNTIME_DIR/verij` (fallback `/tmp/verij`) contains Workspace markers. Legacy state beside `config.toml` migrates on first access.

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
3. Invalid, stale, and registered host snapshots are ignored or pruned.
4. Snapshots are sorted by session name and sent to the TUI event loop.
5. The TUI reconciles snapshots, preserves logical cursor position, and rebuilds the flattened session/tab tree.

Host names are filtered through cached `host-registry.toml` membership rather than prefixes. Watcher reloads registry only after its file changes; attachment writes to `hosts.toml` do not trigger registry reloads.

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

The TUI tracks each host Workspace attachment separately from inner-session metadata. `host-registry.toml` registers hosts before any inner attachment and assigns a stable runtime marker key. `hosts.toml` stores attachment intent by current host name.

Durable records live at `$XDG_STATE_HOME/verij/hosts.toml`, or `~/.local/state/verij/hosts.toml` when `XDG_STATE_HOME` is unset:

```toml
[hosts."project"]
last_session = "backend"
```

The record stores attachment intent. `/tmp/verij/states/` and `/tmp/verij/workspace-<marker-key>.session` remain runtime state. Marker key stays stable through a Verij-controlled host rename, so the already-running nested attach shell can still clean it up.

Press `R` in sidebar to rename host: Verij invokes Zellij `rename-session`, updates host registry and attachment key, and retains runtime marker key. Direct host rename in Zellij session manager is unsupported; rename it back before using Verij rename. Existing local `🔷v` host was migrated to `v` outside the application; no migration logic is embedded in Verij.

CLI host management uses the same registry as the sidebar. `verij list` reads registered hosts even when they have no attachment record, checks one Zellij inventory, and reports `live`, `exited`, or `missing`; `--live`, `--exited`, and `--missing` filter results and can be combined. `verij show NAME` displays host attachment state; `--check` includes Zellij status. `verij prune [--dry-run]` removes only missing host registrations and missing orphan attachments, retaining exited sessions for resurrection. `verij delete NAME` removes state for one missing host; `--zellij` also deletes an existing verified host in Zellij (and requires `--force` for a live host). Both cleanup paths remove durable attachment state and the runtime marker keyed by the host's registry record. Zellij query failures abort cleanup; `verij rename OLD NEW` uses the same live-host rename logic as `R`.

### Attach

When no Workspace session is known, Verij focuses the right pane and writes a shell command equivalent to:

```bash
export VERIJ_WORKSPACE_SESSION=backend
stty sane
zellij attach backend
unset VERIJ_WORKSPACE_SESSION
rm -f /tmp/verij/workspace-<marker-key>.session
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

1. If target is `EXITED`, Verij resurrects it first with `zellij attach --force-run-commands`, then detaches the temporary client. A live target needs no preparation.
2. TUI records target as active and sends `switch:<target>` to current inner agent:

   ```bash
   zellij -s <old> pipe --name verij_control -- switch:<target>
   ```

3. The inner agent calls `switch_session(Some(target))` through the Zellij plugin API.
4. Zellij switches the attached client in place without encountering an exited session or suspended commands.
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
2. Inner session starts with the user's normal Zellij configuration and default layout. Verij reads that layout only to identify the first-tab plugin for readiness checks.
3. Verij waits for the default tab template's status plugin and a terminal pane to coexist in the first tab for a stabilization interval.
4. The fake client detaches while the session remains alive.
5. Inner sessions use normal Zellij config. Verij hosts use a generated complete config: scalar `[zellij]` entries replace matching root nodes in effective user config, preserving themes, keybinds, plugins, and other blocks. Host launch paths pass it through global `zellij --config`; inspect it with `verij config zellij --stdout`.
6. If the agent state file does not appear, Verij launches the configured WASM plugin as a floating, unfocused fallback.
7. The Workspace pane switches to the new session.

After detaching the fake client, Verij verifies the first-tab plugin still exists before switching the Workspace pane. A missing plugin becomes a visible creation error rather than a silent incomplete session.

## Session Resurrection

Zellij serializes sessions by default. `zellij list-sessions` marks exited sessions with `EXITED`; attaching with `--force-run-commands` restores their serialized layout and commands without waiting for the command banner.

Verij uses this in two stages:

1. An exited host is resurrected before its sidebar is attached.
2. The host's durable last-session record is loaded. A live inner session is attached normally; an exited inner session is resurrected with `--force-run-commands` before attachment.

Zellij resurrection restores runtime layout state. It does not replace Verij's host-to-inner attachment record, and it does not provide a global workspace model.

To preserve pane viewport and scrollback, Zellij must be configured with:

```kdl
session_serialization true
serialize_pane_viewport true
scrollback_lines_to_serialize 0
```

These serialization options require restarting the Zellij server. Existing resurrection snapshots cannot regain scrollback that was never serialized.

Verij removes stale serialized `start_suspended true` flags before automatic resurrection because Zellij 0.45.1 can retain them even when `--force-run-commands` is used. When serialized layout contains `zjstatus`, Verij waits until every terminal tab has a visible tiled plugin before detaching temporary resurrection client, preventing partial tab restoration from losing `zjstatus`.

Host resurrection rewrites the serialized Workspace-pane `zellij attach` command to the durable last-session target. This prevents an older command captured before an in-place session switch from resurrecting a different inner session first.

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
|       |-- registry.rs      Infrequently updated host-name registry
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
