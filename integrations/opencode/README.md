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
- Bridge schema version: **1**. Supported OpenCode versions: **1.x.x**, including prerelease and build-suffixed versions. Minor and patch upgrades are accepted without an exact-version allowlist. Current resource/adapter acceptance fixtures run **1.19.0**; **1.18.32–1.18.34** have earlier contract/runtime evidence. Snapshot schema validation remains independent of the OpenCode version. Other major versions, including 2.x, are unsupported. Unsupported transport/storage shapes degrade conservatively within 1.x.
- Registration publishes presence before the first prompt. Native code verifies Linux boot/PID/start identity, server/pane ancestry, controlling terminal, foreground process group and actual terminal stdin/stdout. Environment values are locators, never ownership proof. Registration retries while inventory becomes available and resolves session rename through verified ownership.
- Home is idle with no conversation or completion. Other unowned routes are Unknown. Selecting a child resolves its root and descendants, producing one pane row for the family.
- Read-only SDK recovery caches verified root/family metadata for the current route and producer epoch. Topology discovery and verification resume in bounded slices, with a ten-second fallback. Frequent recovery brackets current status and reads pending permission/question IDs separately from slower topology/history work. Events accelerate updates; server broadcasts from foreign families cannot assign ownership. Newer owned events invalidate semantic samples without discarding independent structural progress.
- Busy/retry and active children produce Working. Explicit permission/question IDs produce Needs input; visiting does not reply to them. Individual tool failures do not become execution errors.
- Done requires a completed root assistant for the current user-message ID, `finish: stop` or `end_turn`, no assistant error, no outstanding family input and a quiescent family. Idle and `tool-calls` alone do not establish successful completion. Actual aborts with `MessageAbortedError` produce Error.
- A bounded, ordered persistent reporter stream commits full snapshots atomically with completion bookkeeping. Original upstream turn revisions are retained across polling, plugin reload and conversation reselection. A sidebar captures the displayed completion revision, including when revisiting an older conversation.
- Producer/SDK deadlines and a 128-frame queue bound adapter work. Overload invalidates authority and triggers fresh reconciliation. Disposal aborts collection, removes subscriptions/timers and bounds reporter shutdown. Missing reporting does not interrupt an OpenCode turn.
- Heartbeats refresh an explicit **15-second lease**. Lease expiry projects Unknown while the agent remains alive and prevents stale acknowledgement. Process/pane absence removes its row; source silence is not exit or Error.
- P2 implemented: owned events set one dirty flag. Active/dirty refresh starts are at least one second apart; quiet idle recovery uses ten seconds. Endpoint and local metadata invocations share a 32/sec token budget with an eight-call burst, four physical requests and four pending requests. Topology slices invoke at most 16 endpoints within the existing three-second collection deadline. Cached status/input recovery invokes four endpoints. Busy families skip message retrieval; quiescent outcome repair has a separate ten-second fallback and message/activity invalidation, with at most six one-message metadata pages per collection. API and reporter failures back off independently to 30 seconds. Unchanged observations dedupe against native transaction receipts and renew their lease every five seconds without an API query. Reconnect begins with Unknown and fresh recovery; disposal cancels every monitoring timer/subscription and request signal and closes its read-only database handle.
- Validated permission/question and activity edges project immediately for the root and verified descendants. IDs are bounded, and replies match request kind and session. Child creation does not itself establish membership. Previously verified, unaffected members can retain explicit input through a topology rescan, but those hints cannot establish complete family quiescence or Done. Child retry produces Working; only root retry produces Retrying.
- Persisted data contains identities, title, lifecycle facts and short error types. Prompts, message text, tool arguments/output, provider error bodies and credentials are excluded.

Bounds: at most 64 family sessions, 100 pending requests and 1,024 remembered successful turns per agent process. Ledger exhaustion stops allocating further Done revisions for new turns, retaining observed activity and a diagnostic. Existing remembered outcomes remain idempotent. Only the currently selected root's latest turn is presented; this is a pane navigator, not a conversation archive.

Outcome recovery finds the newest root user and matching latest assistant through
one-message metadata pages, then rechecks the head. Ordering and parent/session IDs
must agree. It resumes long suffixes using one cursor and searches at most 100
messages for the current user. Missing predecessors or changed heads remain Unknown;
an unchanged head alone does not authorize cached Done. Only two projected message
records are retained. Synthetic 1/16/64-member families finish topology recovery in
1/3/9 slices without increasing endpoint rate or concurrency. Partial family or
outcome scans cannot establish Done.

For **attach**, the existing SDK transport/authentication reads only the stock `info`
prefix, capped at 64 KiB, and cancels before parts. Other responses have a 1 MiB
adapter buffer limit, JSON depth 32, and 16,384 structural-node limit before SDK
parsing. Error bodies are never parsed. Transport-owned read-ahead chunks and server
serialization are outside these adapter allocation limits.

For **standalone**, stock worker RPC would buffer whole response bodies. Message
observation instead reads the existing producer database **read-only**, using fresh,
indexed SQLite metadata pages. Producer paths, file ownership/inode, schema and
indexes are verified; the connection uses a 512 KiB cache, no mmap and zero lock
wait. Stored metadata above 64 KiB is rejected before JSON extraction. Parts are
never selected, and no message-body RPC occurs. `OPENCODE_DB` overrides are honored
within the installed storage contract. Missing/in-memory databases, uncertain
channel paths, replaced files, changed schemas or missing indexes produce conservative
state rather than falling back to unbounded message retrieval. Other stock worker
SDK endpoints still own their upstream buffers; adapter decoding/parsing stays capped.

The installed OpenCode 1.19.0 contract exposes `api.state.session.messages(id)` as
structured `Message` metadata, with parts held separately. Its global `ready` flag
does not prove a particular session's history finished hydrating. Local cache metadata
can label Working, but current-outcome authority comes from the independently verified
suffix. Empty/stale cache contents never establish Idle or Done. Source references:
[plugin state API](https://github.com/anomalyco/opencode/blob/v1.19.0/packages/plugin/src/tui.ts)
and [TUI synchronization](https://github.com/anomalyco/opencode/blob/v1.19.0/packages/tui/src/context/sync.tsx).
The standalone storage contract is established by the stock
[worker transport](https://github.com/anomalyco/opencode/blob/v1.19.0/packages/opencode/src/cli/tui/worker.ts),
[database path](https://github.com/anomalyco/opencode/blob/v1.19.0/packages/core/src/database/database.ts),
and [message schema/index](https://github.com/anomalyco/opencode/blob/v1.19.0/packages/core/src/session/sql.ts).

`agent doctor` inspects the selected configuration, versions, asset contents, reporter execution, runtime atomic-write availability, effective adapter/runtime paths, exporter inventory prerequisites and live source health. `--session` scopes inventory and records to one inner session. A legacy exporter is explicitly reported as `legacy_snapshot`; `monitoring_prerequisites_ready` remains false even when adapter installation is correct. Doctor also reports the bundled/default-installed WASM paths and whether their bytes match. `live_records` confirms actual registration, rather than inferring it from installed files. Merged project settings and OpenCode runtime KV can still disable a configured plugin.

## Verification and review

```sh
node --test integrations/opencode/core.test.mjs integrations/opencode/recovery.test.mjs
```

The [H3 live fixture](../../prototypes/h3-opencode/README.md) exercises the installed runtime with a controlled local provider, real native panes and production records. The [progress log](../../docs/AGENT_MONITORING_PROGRESS.md) records current verification and human-review status.
