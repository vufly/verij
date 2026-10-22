# OpenCode V2 upgrade investigation

Date: **2026-10-17 UTC**. Updated: **2026-10-21 UTC**. Status: **full-recovery P0 blocked; separate experimental live-status adapter implemented; global rollout not approved**.

## Approved reduced-scope implementation — 2026-10-21

The user approved an independent Herdr-style cache/event live-status track. See the [scope change and checklist](OPENCODE_V2_UPGRADE_PLAN.md) and [experimental adapter guide](../integrations/opencode/v2/README.md). Schema 2 rejects all turn identities and outcomes, so this track cannot produce Done or replay completion acknowledgements. It makes no SDK/history/sync requests and makes no upstream cache allocation/freshness guarantee. V1 behavior remains separate.

Deterministic native/JavaScript/setup regressions and private real-CLI **2.1.2** loading checks are recorded in the progress log. This does not repin or pass the original full-recovery P0. No user global configuration, installed executable, or shared service was changed. The dated baseline investigation below describes the original full-recovery target and historical findings.

## Recommendation

The installed binary is now **OpenCode v2.0.32**, but Verij still implements the V1 integration. Treat V2 monitoring as unsupported until the adapter, installer, native snapshot admission, and live qualification are updated together. Do not fix this by widening the version gate alone.

Implement a separate V2 pane-local adapter while retaining V1 support and independently owned assets/configuration. Qualify the installed **2.0.32** release first; do not automatically downgrade OpenCode or edit the user's global configuration during development. The phased [V2 upgrade plan](OPENCODE_V2_UPGRADE_PLAN.md) defines dependencies, implementation files, acceptance gates, and rollback.

This refresh compares Verij commit `5b6ea1ebd28472b54142d79f87f48854f64ff7c4` (initially clean working tree), official development documentation, and upstream source pinned to **v2.0.32**, commit `843a6c514e606b436a7fe713422ace0a93ad3d61`. This is the installed qualification target, not a claim that it is the latest upstream release. Tagged contracts take precedence where development docs differ.

The baseline investigation below ran only version/help commands, an isolated setup rejection check, source inspection, and existing deterministic tests. Subsequent isolated [P0 prototypes](../prototypes/opencode-v2/README.md) add private CLI-loading and controlled-provider contract evidence. They expose revert-clear success markers without model work and historical success surviving active shutdown, qualify quiescent root CLI reload/events, verify tagged SDK response shapes with pending permissions/forms across two directories, observe terminal provider-failure/user-abort payloads in both private CLI modes, and reconcile those events with recovered markers on an explicitly provisioned private server (including completed restart and an unobserved second turn). Managed resume, bounded CLI transport provenance, and complete-family freshness remain blocked. A subsequent [seven-file pinned source audit](../prototypes/opencode-v2/transport-source-results.json) confirms the missing public transport boundary and identifies a source-level client-replacement concern; the [P0 exit decision](../prototypes/opencode-v2/P0_GATE_DECISION.md) records the upstream prerequisite without marking unfinished qualification complete. No shared-service mutation, production setup/uninstall, or resource benchmark was performed.

## What breaks immediately

| Area | Current Verij / OpenCode V1 | OpenCode V2 / required change |
|---|---|---|
| Version detection and admission | Setup trims `--version` stdout but does not normalize a product prefix. JavaScript and native snapshot guards accept only bare `1.x.x`; shared release-family tests explicitly reject V2. | Installed stdout is **`opencode v2.0.32`**. Add strict executable-output normalization separately from canonical snapshot validation. A V2 adapter must be qualified before admitting its release family. |
| Plugin configuration | `tui.json(c)`, `plugin`, and `[path, options]` tuples. | Global **`cli.json`**, `plugins`, and `{ "package": path, "options": options }`. The file accepts JSONC syntax, but a separate `cli.jsonc` is not the tagged target. Update setup, doctor, uninstall, asset ownership, and preservation tests. |
| Plugin entrypoint | Default object with `async tui(api, options)`. | CLI plugin definition with `setup(context)`, usually through `Plugin.define` from `@opencode/plugin/tui`. Options come from `context.options`; setup returns cleanup. |
| Selected conversation | `api.route.current.name` and `params.sessionID`. | `context.ui.router.current()` returns a discriminated route, including `{ type: "session", sessionID }`. Home and plugin routes must still clear conversation authority. |
| Event subscriptions | `api.event.on(...)`, with payload in `event.properties`. | `context.data.on(...)` / `listen(...)`, with payload in `event.data`. Durable events also expose an event ID and sequence envelope. |
| Session recovery | `session.get`, `session.children`, `session.status`, and `session.messages`; the adapter expects `{ data }` responses. | Tagged Promise client uses `session.get`, paginated `session.list({ parentID, ... })`, `session.active`, and **`message.list`**. `get`/`active` return unwrapped values; message pages return `{ data, cursor: { previous, next } }`. `active` maps running sessions, not V1 idle/busy/retry status objects. Rewrite response handling, ordering, and pagination. |
| Cached data | `api.state.ready`, `api.state.session.messages(id)`. | `context.data.session` supplies `get`, `root`, `family`, `status`, `sync`, plus message/input collections. There is no equivalent global readiness property in the public CLI context. Cache emptiness still cannot prove freshness or family completeness. |
| Input requests | `permission.list` plus `question.asked/replied/rejected` and `question.list`. | `permission.request.list` returns location-scoped `{ location, data }`; tagged `permission.list({ sessionID })` returns unwrapped per-session pending requests, **not rules** (corrected by the private tagged-SDK runtime probe). Interactive input uses `form.created/replied/cancelled`, `session.form.list`, and form collections. Global pending lists are location-scoped. Navigating must never reply, cancel, or change rules. |
| Completion/error recovery | Latest user and matching assistant `parentID`, `finish`, completion timestamp, and `error.name`. | Messages use `type`, not V1 `info.role`, and include content directly. V2 exposes execution outcomes and idle markers. Structured errors use **`type`**, with private `message`/`response.body` fields that must not be persisted. Rework completion identity and interruption policy. |
| Disposal | `api.lifecycle.onDispose(...)`. | Cleanup returned by `setup`. Abort requests, unsubscribe, stop timers, and bound reporter shutdown as today. |
| Response-size protection | Per-request SDK `requestValidator` wraps the resolved transport before parsing; the history reader skips V1 message parts after bounded metadata. | The tagged V2 client's request options expose `signal`, `headers`, and streaming activity callbacks, **not `requestValidator`**. Its constructor accepts `fetch`, but the CLI supplies an existing client. Current wrapper/parser cannot be reused unchanged. A supported bounded observation transport must be established without mutating shared client/global fetch. |

Relevant local files:

- `integrations/opencode/bridge.mjs`: plugin entrypoint, route tracking, event dispatch, native reporter lifecycle.
- `integrations/opencode/core.mjs`: version gate, recovery, history projection, request budgeting, response bounds.
- `verij-cli/src/opencode.rs`: independent version/snapshot validation and completion ledger.
- `verij-cli/src/agent_version.rs`: cross-adapter release-family regression expectations.
- `verij-cli/src/agent_setup.rs`: embedded assets, V1 config resolution/surgery, setup/doctor/uninstall.
- `verij-cli/tests/opencode_setup.rs`: installation/config-preservation expectations.
- `prototypes/h3-opencode/`, `prototypes/opencode-runtime/`, `prototypes/resource-usage/`: V1 fixtures, event probes, lifecycle controls, API assumptions, and launch/config isolation.

## Useful V2 differences

### Explicit execution outcomes

Upstream defines durable events:

- `session.execution.started`
- `session.execution.succeeded`
- `session.execution.failed`
- `session.execution.interrupted`

`Session.Info` also exposes `outcome: succeeded | failed | interrupted` and `time.idle`. Projected history includes `type: "idle"` messages carrying those outcomes. This is a better recovery foundation than inferring successful completion solely from assistant finish metadata.

However, a success event's data contains a session ID, not a V1 user-message/assistant-parent pair. V2 explicitly defines an idle marker as the boundary for all work since the preceding marker, **including steered prompts**. Shutdown does not write a marker because resumed execution continues the same turn. A one-user-message/one-execution assumption is therefore incorrect.

The preferred design to validate is an immutable, namespaced `root/idle-marker-ID` completion identity, with events used for invalidation and fast activity/input projection. This requires a V2 snapshot contract rather than silently changing V1's `root/user-message` meaning. Event IDs/aggregate sequences alone are not an established history-recovery identity. Sequence values are aggregate-local, not a global freshness clock.

**Tagged-client caveat:** the 2.0.32 message response union includes idle markers, but `MessageListInput.type` does **not** include `idle`; it accepts a single supported type, not a user/idle filter set. Do not implement `message.list({ type: "idle" })` from the schema alone. First prove which markers the real endpoint returns, their ordering/cursors, and bounded recovery after reload or missed events. Session `outcome`/`time.idle` alone do not prove identity or absence of newer queued work.

Do not treat every terminal notification as Done: V2's built-in notification plugin announces interruptions through its terminal path too. Require successful root execution, fresh complete-family quiescence, no pending input/inbox work, and a stable recovered identity. Define `user`, `shutdown`, `superseded`, and `inactivity` interruption behavior explicitly; a resumable shutdown must not become a fabricated successful completion or an irreversible execution error. Separate [private CLI terminal evidence](../prototypes/opencode-v2/terminal-results.json) proves that controlled HTTP 400 provider failure projects error type `provider.invalid-request`, while two Esc presses during blocked execution project interruption reason `user`. Typed and envelope event identities agree in both modes. Separate [private explicit-server reconciliation evidence](../prototypes/opencode-v2/reconciliation-results.json) now proves that tagged `SessionMessage.ID.fromEvent` (`evt_` → `msg_`) maps these two terminal event kinds to recovered idle markers; repeated reads and a completed-server restart preserve identity, and a second turn after observer disposal yields a distinct recoverable marker. Failure type survives in history, but interruption reason does not. This uses fixture-owned credentials, not a supported public CLI transport hook. Successful-event mapping, standalone recovery, other reasons, active resume, general missed-event recovery, and native revision continuity remain unqualified; neither tested terminal event is successful completion.

### Richer pane-local data API

The CLI context provides family/root helpers, session synchronization, per-session permissions/forms, and pending inbox data. These may remove redundant adapter-owned queries and scans. They are not automatically a proof of complete, fresh topology: upstream's 2.0.29 changelog itself mentions a paginated-child family fix.

Type-filtered message pages may reduce payloads, but the tagged query does not offer an idle filter. User records and pending inbox items contain prompt text; assistant records contain tool output. Even cache `sync` calls can allocate complete upstream responses before Verij projects them. Keep privacy projection, adapter allocation bounds, and freshness proofs independent of endpoint selection. Do not sync full history merely to establish an idle label.

### Shared service by default

V2 normally discovers or starts one background server per user. `--standalone` selects a private server; `--server URL` connects to an explicit server. Session execution, permissions, and configuration belong to that server, not to a terminal client.

This reinforces the existing Verij architecture: run the monitor in the local interactive CLI process, never infer pane ownership from a server plugin or a global session list. A terminal-local plugin configured in `cli.json` also works against remote servers. Continue proving local process/TTY/pane ownership through the native reporter and exporter inventory.

Installed `--help` confirms `serve`, `--standalone`, and `--server URL`. There is **no registered `attach` subcommand** in the tagged command registry; `opencode attach --help` merely prints top-level help. Replace fixture attachment with `opencode --server URL` and verify session selection/process ownership live. Help output is not a successful connection test. Isolate qualification from the user's shared service; never run global `service stop/restart` as fixture cleanup.

### Additional user-facing changes

- Most supported V1 server config, agents, commands, skills, credentials, and MCP configuration are intended to remain compatible; conversion to native V2 syntax is optional.
- The first V2 terminal startup migrates supported global TUI settings/preferences when `cli.json` is absent, leaving V1 files unchanged. Tagged migration reads **`tui.json`** and `kv.json`, and converts legacy plugin entries and enable/disable directives; it does not port plugin code. A migrated Verij entry can still point at the incompatible V1 bridge. Project-local client configuration is not migrated.
- Creating `cli.json` in setup before OpenCode's first-run migration can suppress migration of unrelated settings. Require that migration to complete first when legacy state exists, or design and test an explicit preservation path; never silently create an almost-empty replacement.
- Tagged CLI uses `OPENCODE_CONFIG_DIR` before the XDG/global directory, then loads `cli.json`. `OPENCODE_CLI_CONFIG_CONTENT` merges over it and **replaces arrays**, including `plugins`. Replace `OPENCODE_TUI_CONFIG` isolation. Doctor must distinguish a configured entry from one overridden or disabled by ordered plugin directives.
- V2 supports terminal session tabs, a minimal `opencode mini` interface, shared-service/remote workflows, and explicit queued/steered input. This investigation does not establish that mini or noninteractive run loads the CLI monitoring plugin.
- V2 accepts `lsp` configuration but currently does not run language servers, LSP tools, or diagnostics. Workflows relying on those features need lint/typecheck/compiler alternatives.
- V2 instruction discovery currently uses `AGENTS.md`, not the V1 `CLAUDE.md` fallback.
- Both versions use the `opencode` command and shared configuration locations. V2 is already installed here. Retain any available V1 binary/configuration for explicit rollback, but do not assume V1 can safely open V2 data or rewrite V2-only configuration.

## Resource implications

V2 provides an opportunity to reduce completion-recovery overhead through explicit lifecycle events, outcome metadata, and cached family/input APIs. There is no measured V1-versus-V2 CPU/RSS result in this investigation.

A shared background server does not prove lower total memory usage. Measure the server, all CLI clients, and native reporters together, with multiple panes and large histories. The current scheduler, request/concurrency budgets, response bounds, lease renewal, queue bounds, and shutdown behavior remain necessary unless equivalent constraints are demonstrated in the V2 implementation.

V2 does **not** remove the separate Zellij CLI teardown/state-broadcast amplification documented in `docs/AGENT_MONITORING_RESOURCE_INVESTIGATION.md`. Continue that resource work independently.

## Update plan

See [OpenCode V2 upgrade plan](OPENCODE_V2_UPGRADE_PLAN.md) for the executable work breakdown. Its critical path is:

1. Prove 2.0.32 transport bounds, history identity, and family freshness in an isolated runtime before committing to an adapter design.
2. Implement a separate V2 bridge/recovery and explicit native V2 snapshot contract, preserving V1 behavior and shared ownership/ledger infrastructure.
3. Add strict executable-version normalization and version-aware setup/doctor/uninstall with first-run migration and V1 coexistence safeguards.
4. Port controlled-provider/real-TTY acceptance, including queued/steered work, shared-server clients, reload/reconnect, and host-local acknowledgement.
5. Complete resource controls, soak, and manual rollout/rollback review. Keep the separate Zellij resource program independent.

The Zellij navigation/exporter and native persistence/reducer infrastructure appear reusable from source inspection, subject to live ownership and acknowledgement tests. No phase is marked complete merely because this investigation or existing V1 tests pass.

## Verification performed — 2026-10-17 UTC

- `opencode --version`: **`opencode v2.0.32`**. Resolved binary: `~/.local/share/mise/installs/opencode/2.0.32/bin/opencode`; SHA-256: `cdf4729f26f340ce84eb0e1b3b96912e584eb1f53141d7da9ffe590280549ac5`.
- Inspected installed help and the tagged command registry; confirmed the server flags and absence of an `attach` registration. Did not establish a live server connection.
- Reproduced current setup rejection against the real V2 binary with disposable HOME/XDG/Verij directories: **`unsupported OpenCode opencode v2.0.32; supported: 1.x.x`**, exit 1.
- Reviewed current implementation and official V2 docs. Retrieved upstream **v2.0.32**, commit `843a6c514e606b436a7fe713422ace0a93ad3d61`, into temporary files outside the repository.
- `node --test integrations/opencode/core.test.mjs integrations/opencode/recovery.test.mjs`: **55 passed**.
- `cargo test -p verij-cli --test opencode_setup`: **1 passed**; `cargo test -p verij-cli opencode:: -- --test-threads=1`: **3 passed**; `cargo test -p verij-cli agent_setup:: -- --test-threads=1`: **5 passed**.
- These are existing V1 adapter/installer regressions, not V2 monitoring qualification. No interactive V2/provider test, production configuration edit, shared-service change, or V2 resource benchmark was performed.

## Sources

Official documentation:

- [Changelog](https://opencode.ai/changelog)
- [Migrate from V1](https://dev.opencode.ai/v2/docs/migrate-v1/)
- [CLI plugin API](https://dev.opencode.ai/v2/docs/build/plugins/cli/)
- [CLI plugin loading](https://dev.opencode.ai/v2/docs/cli/plugins/)
- [CLI settings](https://dev.opencode.ai/v2/docs/cli/config/)
- [CLI/shared service](https://dev.opencode.ai/v2/docs/cli/)
- [API reference](https://dev.opencode.ai/v2/docs/api/)

Release-pinned implementation:

- [CLI plugin context](https://github.com/anomalyco/opencode/blob/v2.0.32/packages/plugin/src/tui/context.ts) and [definition/cleanup](https://github.com/anomalyco/opencode/blob/v2.0.32/packages/plugin/src/tui/plugin.ts)
- [Promise client and transport](https://github.com/anomalyco/opencode/blob/v2.0.32/packages/client/src/promise/generated/client.ts) and [generated query/response types](https://github.com/anomalyco/opencode/blob/v2.0.32/packages/client/src/promise/generated/types.ts)
- [Execution/session events](https://github.com/anomalyco/opencode/blob/v2.0.32/packages/schema/src/session-event.ts)
- [Session outcome metadata](https://github.com/anomalyco/opencode/blob/v2.0.32/packages/schema/src/session.ts)
- [Message/idle-marker schema](https://github.com/anomalyco/opencode/blob/v2.0.32/packages/schema/src/session-message.ts), [inbox schema](https://github.com/anomalyco/opencode/blob/v2.0.32/packages/schema/src/session-inbox.ts), and [structured errors](https://github.com/anomalyco/opencode/blob/v2.0.32/packages/schema/src/session-error.ts)
- [Event envelope](https://github.com/anomalyco/opencode/blob/v2.0.32/packages/schema/src/event.ts)
- [CLI config parsing/merge](https://github.com/anomalyco/opencode/blob/v2.0.32/packages/cli/src/config/config.ts), [first-run migration](https://github.com/anomalyco/opencode/blob/v2.0.32/packages/cli/src/config/migrate.ts), and [global path resolution](https://github.com/anomalyco/opencode/blob/v2.0.32/packages/util/src/global.ts)
- [CLI commands/config wiring](https://github.com/anomalyco/opencode/blob/v2.0.32/packages/cli/src/index.ts)
- [Built-in notifications/forms/lifecycle usage](https://github.com/anomalyco/opencode/blob/v2.0.32/packages/tui/src/feature-plugins/system/notifications.ts)
