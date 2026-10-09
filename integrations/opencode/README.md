# OpenCode pane-local monitoring (H3)

This is a TUI plugin, configured in `tui.json` or `tui.jsonc`. It runs in the actual interactive OpenCode process, including `opencode attach`, rather than in the shared server. Install once, then launch OpenCode normally in an inner Zellij pane.

## Install and inspect

```sh
target/release/verij agent setup opencode
target/release/verij agent doctor
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

`agent doctor` inspects the selected configuration, versions, asset contents, reporter execution, runtime atomic-write availability and live source health. A matching file is installation evidence; actual TUI registration is the live reporting check. Merged project settings and OpenCode runtime KV can still disable a configured plugin.

## Verification and review

```sh
node --test integrations/opencode/core.test.mjs
```

The [H3 live fixture](../../prototypes/h3-opencode/README.md) exercises the installed runtime with a controlled local provider, real native panes and production records. The [progress log](../../docs/AGENT_MONITORING_PROGRESS.md) records current verification and human-review status.
