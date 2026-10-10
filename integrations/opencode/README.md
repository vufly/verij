# OpenCode pane-local monitoring (H3)

This is a TUI plugin, configured in `tui.json` or `tui.jsonc`. It runs in the actual interactive OpenCode process, including `opencode attach`, rather than in the shared server. Install once, then launch OpenCode normally in an inner Zellij pane.

## Install and inspect

The sidebar binary and each inner session's **Verij WASM exporter** must both be from the monitoring build. A new CLI with an old exporter still renders sessions/tabs, but cannot establish agent ownership and therefore produces no agent children.

If normal Zellij configuration loads `~/.local/share/verij/verij_plugin.wasm`, install the current build at that path before starting a new inner session:

```sh
make install-plugin
```

This honors `XDG_DATA_HOME`; set `VERIJ_DATA_DIR=/absolute/plugin/directory` when the configured Verij plugin is elsewhere. `make dev` builds checkout artifacts but does not upgrade another installed path automatically. The installation target preserves Zellij configuration and uses an atomic artifact replacement.

For an already-running inner session, reload **the same URL configured in its `load_plugins`** after installing the new artifact. For the standard path:

```sh
zellij -s INNER_SESSION action start-or-reload-plugin \
  "file:$HOME/.local/share/verij/verij_plugin.wasm"
```

Use your configured URL/XDG path if different. This reloads the exporter without restarting the inner session or agent. An already-loaded OpenCode bridge retries registration and can recover once verified inventory appears. A fresh exporter context may require re-registering host navigation before verified visits.

```sh
target/release/verij agent setup opencode
target/release/verij agent doctor
target/release/verij agent doctor --session INNER_SESSION
```

**Quit and restart OpenCode after setup or uninstall.** Running TUIs retain their loaded configuration. The setup output includes the selected config file, absolute reporter path, shared runtime directory and restart requirement.

Config resolution honors `OPENCODE_TUI_CONFIG`, then `OPENCODE_CONFIG_DIR`, then the XDG/global OpenCode directory. An explicit `--config-dir /absolute/path` selects that directory's TUI config. `--opencode-bin /absolute/path/opencode` selects the executable used for version detection. If both `tui.json` and `tui.jsonc` exist in a selected directory, resolve that ambiguity first.

Setup embeds `bridge.mjs` and `core.mjs` under `verij-monitoring/`, adds one identifiable plugin tuple, and records asset fingerprints in `installed.json`. Repeating setup updates its own entry/assets and preserves unrelated plugin entries, options, JSONC comments, keybindings and themes. Modified assets are preserved for inspection. The plugin is not added to server-side `opencode.json`.

```sh
target/release/verij agent uninstall opencode
```

Uninstall removes the matching Verij entry and unmodified owned assets. It preserves other configuration and requires an OpenCode restart. Setup does not change a user's explicit `plugin_enabled` choice or OpenCode's persisted runtime enable/disable state.

Keep the reporter executable at its installed absolute path. Rerun setup after moving the binary. The installed shared runtime path survives profile-specific HOME changes; an explicit `VERIJ_AGENT_STATE_DIR` environment override takes precedence. The installed inventory path is likewise overridden by `VERIJ_STATES_DIR`. All sidebars and adapters must resolve the same directories.

## Contract and behavior

- Agent activation now recovers an absent/stale navigation binding automatically when this host has a live **owned Workspace attachment**. The user accepted a brief shared registration dialog for this recovery: attached peer clients may see it temporarily. No host is associated with an inner client until the real Workspace keyboard nonce receipt arrives. Registration itself never acknowledges Done.
- Recovery checks outer native pane/foreground ownership, Linux socket-peer metadata for the actual attachment-to-server connection, fresh native dialog focus, and exact receipt context. It serializes registration per server, respects superseded navigation, and cleans failed surfaces. The dialog uses stock public SDK functions and works without Normal-mode shortcuts, including Locked mode. No keymap edits or extra Zellij permissions are required.
- Active-tab highlighting uses the host's confirmed inner pane identity and updates before completion-acknowledgement work finishes. Focus wakeups are bounded; native receipts do not wait for broadcast CLI cleanup. Returning across sessions still needs fresh destination registration and may take several seconds. Unreferenced dedicated registration surfaces are retired after replacement, while surfaces referenced by another host are preserved. Reload the current exporter after an upgrade and restart the sidebar to load these changes; no host or inner-session deletion is required.
- Start an inner attachment from a session/tab row before activating an agent in a host with an empty Workspace. For an existing host carrying a raw legacy attachment, `verij recover HOST` refreshes its supported host layout; normal exited-host restoration now rewrites the serialized nested command to the owned wrapper. Running old sidebar processes need the current CLI to be restarted/recovered before they can use automatic registration.
- Bridge schema version: **1**. Selected TUI API versions: **1.18.32, 1.18.33 and 1.18.34**. The current production acceptance fixture runs **1.18.34**; earlier versions have the separately recorded H0 contract evidence. Other versions, including 2.x, are unsupported until their API is verified.
- Registration publishes presence before the first prompt. Native code verifies Linux boot/PID/start identity, server/pane ancestry, controlling terminal, foreground process group and actual terminal stdin/stdout. Environment values are locators, never ownership proof. Registration retries while inventory becomes available and resolves session rename through verified ownership.
- Home is idle with no conversation or completion. Other unowned routes are Unknown. Selecting a child resolves its root and descendants, producing one pane row for the family.
- Read-only SDK reconciliation obtains root metadata, current messages, descendant relationships, session statuses and pending permission/question requests. It repairs missing startup caches and reload/mid-turn state. Events accelerate updates; server broadcasts from foreign families cannot assign ownership. Activity and family membership are bracketed, and newer owned events/route changes invalidate in-flight samples.
- Busy/retry and active children produce Working. Explicit permission/question IDs produce Needs input; visiting does not reply to them. Individual tool failures do not become execution errors.
- Done requires a completed root assistant for the current user-message ID, `finish: stop` or `end_turn`, no assistant error, no outstanding family input and a quiescent family. Idle and `tool-calls` alone do not establish successful completion. Actual aborts with `MessageAbortedError` produce Error.
- A bounded, ordered persistent reporter stream commits full snapshots atomically with completion bookkeeping. Original upstream turn revisions are retained across polling, plugin reload and conversation reselection. A sidebar captures the displayed completion revision, including when revisiting an older conversation.
- Producer/SDK deadlines and a 128-frame queue bound adapter work. Overload invalidates authority and triggers fresh reconciliation. Disposal aborts collection, removes subscriptions/timers and bounds reporter shutdown. Missing reporting does not interrupt an OpenCode turn.
- Heartbeats refresh an explicit **15-second lease**. Lease expiry projects Unknown while the agent remains alive and prevents stale acknowledgement. Process/pane absence removes its row; source silence is not exit or Error.
- Persisted data contains identities, title, lifecycle facts and short error types. Prompts, message text, tool arguments/output, provider error bodies and credentials are excluded.

Bounds: at most 64 family sessions, 100 pending requests and 1,024 remembered successful turns per agent process. Ledger exhaustion stops allocating further Done revisions for new turns, retaining observed activity and a diagnostic. Existing remembered outcomes remain idempotent. Only the currently selected root's latest turn is presented; this is a pane navigator, not a conversation archive.

`agent doctor` inspects the selected configuration, versions, asset contents, reporter execution, runtime atomic-write availability, effective adapter/runtime paths, exporter inventory prerequisites and live source health. `--session` scopes inventory and records to one inner session. A legacy exporter is explicitly reported as `legacy_snapshot`; `monitoring_prerequisites_ready` remains false even when adapter installation is correct. Doctor also reports the bundled/default-installed WASM paths and whether their bytes match. `live_records` confirms actual registration, rather than inferring it from installed files. Merged project settings and OpenCode runtime KV can still disable a configured plugin.

## Verification and review

```sh
node --test integrations/opencode/core.test.mjs
```

The [H3 live fixture](../../prototypes/h3-opencode/README.md) exercises the installed runtime with a controlled local provider, real native panes and production records. The [progress log](../../docs/AGENT_MONITORING_PROGRESS.md) records current verification and human-review status.
