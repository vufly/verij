# OpenCode V2 upgrade investigation

## Recommendation

Do not replace the installed OpenCode 1.19.0 in the production Verij workflow yet. OpenCode V2 requires a new pane-local monitoring adapter and installer support, not just a broader version check. Keep V1 operational while qualifying V2 in an isolated environment.

The official changelog currently lists **2.0.29**. This investigation compares the current working tree, including its uncommitted monitoring/resource changes, with official migration documentation and upstream source pinned to **v2.0.29**. Development documentation is not necessarily identical to that release; use the tagged client implementation when designing the adapter.

No OpenCode upgrade or production configuration change was performed. Existing working-tree changes were left intact.

## What breaks immediately

| Area | Current Verij / OpenCode V1 | OpenCode V2 / required change |
|---|---|---|
| Version admission | JavaScript and Rust accept only `1.x.x`. Setup rejects other majors; snapshot validation also rejects them. | A V2 adapter must be qualified before admitting `2.x`. Changing only one gate does not fix loading or reporting. |
| Plugin configuration | `tui.json(c)`, `plugin`, and `[path, options]` tuples. | Global `cli.json`, `plugins`, and `{ "package": path, "options": options }`. Update setup, doctor, uninstall, asset ownership, and config-preservation tests. |
| Plugin entrypoint | Default object with `async tui(api, options)`. | CLI plugin definition with `setup(context)`, usually through `Plugin.define` from `@opencode/plugin/tui`. Options come from `context.options`; setup returns cleanup. |
| Selected conversation | `api.route.current.name` and `params.sessionID`. | `context.ui.router.current()` returns a discriminated route, including `{ type: "session", sessionID }`. Home and plugin routes must still clear conversation authority. |
| Event subscriptions | `api.event.on(...)`, with payload in `event.properties`. | `context.data.on(...)` / `listen(...)`, with payload in `event.data`. Durable events also expose an event ID and sequence envelope. |
| Session recovery | `session.get`, `session.children`, `session.status`, and `session.messages`; the adapter expects `{ data }` responses. | The tagged Promise client uses `session.get`, `session.list({ parentID, ... })`, `session.active`, and **`message.list`**. Some methods return unwrapped domain values; lists carry their own data/cursors. Rewrite response handling and pagination. |
| Cached data | `api.state.ready`, `api.state.session.messages(id)`. | `context.data.session` supplies `get`, `root`, `family`, `status`, `sync`, plus message/input collections. There is no equivalent global readiness property in the public CLI context. Cache emptiness still cannot prove freshness or family completeness. |
| Input requests | Permissions plus `question.asked/replied/rejected` and `question.list`. | Permissions remain, but interactive input uses forms: `form.created/replied/cancelled` and form collections. Preserve Verij's read-only behavior; navigating must never reply or cancel. |
| Completion/error recovery | Latest user and matching assistant `parentID`, `finish`, completion timestamp, and `error.name`. | Messages use `type`, not V1 `info.role`, and include content directly. V2 exposes explicit execution success/failure/interruption and session outcome metadata. Rework completion identity and error-type projection. |
| Disposal | `api.lifecycle.onDispose(...)`. | Cleanup returned by `setup`. Abort requests, unsubscribe, stop timers, and bound reporter shutdown as today. |
| Response-size protection | Per-request SDK `requestValidator` wraps the resolved transport before parsing; the history reader skips V1 message parts after bounded metadata. | The tagged V2 client's request options expose `signal`, `headers`, and streaming activity callbacks, **not `requestValidator`**. Its constructor accepts `fetch`, but the CLI supplies an existing client. Current wrapper/parser cannot be reused unchanged. A supported bounded observation transport must be established without mutating shared client/global fetch. |

Relevant local files:

- `integrations/opencode/bridge.mjs`: plugin entrypoint, route tracking, event dispatch, native reporter lifecycle.
- `integrations/opencode/core.mjs`: version gate, recovery, history projection, request budgeting, response bounds.
- `verij-cli/src/opencode.rs`: independent version/snapshot validation and completion ledger.
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

However, a success event's data contains a session ID, not a V1 user-message/assistant-parent pair. V2 also supports queued and steered input. The existing immutable `root/user-message` completion contract therefore needs an explicit correlation design, or a deliberate switch to execution-based identity. Durable event IDs/sequences are useful candidates, but their correctness across replay and history recovery must be demonstrated.

Do not treat every terminal notification as Done: V2's built-in notification plugin announces interruptions through its terminal path too. Verij must keep success separate from failed/interrupted execution, require a quiescent verified family, and preserve completion revision idempotence across reconnect, reload, and conversation reselection.

### Richer pane-local data API

The CLI context provides family/root helpers, session synchronization, per-session permissions/forms, and pending inbox data. These may remove redundant adapter-owned queries and scans. They are not automatically a proof of complete, fresh topology: upstream's 2.0.29 changelog itself mentions a paginated-child family fix.

The V2 client can request type-filtered message pages. Recovering user/idle markers may avoid downloading assistant/tool content, but user text is still present and filtering is not a byte bound. Keep privacy projection and allocation bounds independent of endpoint selection.

### Shared service by default

V2 normally discovers or starts one background server per user. `--standalone` selects a private server; `--server URL` connects to an explicit server. Session execution, permissions, and configuration belong to that server, not to a terminal client.

This reinforces the existing Verij architecture: run the monitor in the local interactive CLI process, never infer pane ownership from a server plugin or a global session list. A terminal-local plugin configured in `cli.json` also works against remote servers. Continue proving local process/TTY/pane ownership through the native reporter and exporter inventory.

Existing fixture and restored-session commands use `opencode serve` / `opencode attach`. Their exact V2 compatibility and process layout were not exercised; verify commands against the selected binary rather than assuming they were removed or preserved. Isolate any V2 qualification from the user's shared service.

### Additional user-facing changes

- Most supported V1 server config, agents, commands, skills, credentials, and MCP configuration are intended to remain compatible; conversion to native V2 syntax is optional.
- The first V2 terminal startup migrates supported global TUI settings/preferences when `cli.json` is absent, leaving V1 files unchanged. It does not port Verij's plugin implementation. Project-local client configuration is not migrated.
- CLI config is global and supports live reload. `OPENCODE_CLI_CONFIG_CONTENT` can override it, including replacing the plugin array. Existing `OPENCODE_TUI_CONFIG`-based isolation must be replaced; do not assume V1 environment resolution remains valid.
- V2 supports terminal session tabs, a minimal `opencode mini` interface, shared-service/remote workflows, and explicit queued/steered input. This investigation does not establish that mini or noninteractive run loads the CLI monitoring plugin.
- V2 accepts `lsp` configuration but currently does not run language servers, LSP tools, or diagnostics. Workflows relying on those features need lint/typecheck/compiler alternatives.
- V2 instruction discovery currently uses `AGENTS.md`, not the V1 `CLAUDE.md` fallback.
- Both versions use the `opencode` command and shared configuration locations. The official V2 installer can replace the V1 binary; preserve a separately addressed V1 binary and configuration before testing.

## Resource implications

V2 provides an opportunity to reduce completion-recovery overhead through explicit lifecycle events, outcome metadata, and cached family/input APIs. There is no measured V1-versus-V2 CPU/RSS result in this investigation.

A shared background server does not prove lower total memory usage. Measure the server, all CLI clients, and native reporters together, with multiple panes and large histories. The current scheduler, request/concurrency budgets, response bounds, lease renewal, queue bounds, and shutdown behavior remain necessary unless equivalent constraints are demonstrated in the V2 implementation.

V2 does **not** remove the separate Zellij CLI teardown/state-broadcast amplification documented in `docs/AGENT_MONITORING_RESOURCE_INVESTIGATION.md`. Continue that resource work independently.

## Proposed migration sequence

1. Pin a specific V2 release and isolate its binary, configuration/data roots, and server. Keep V1 available for rollback; do not convert shared configuration to V2-only shapes.
2. Introduce a separate V2 CLI adapter and version-aware installer. Keep the native Verij snapshot/reducer contract where possible; avoid forcing V1 and V2 upstream APIs through the same recovery implementation.
3. Define execution/current-turn correlation, failure/interruption mapping, forms, pending inbox semantics, and complete-family freshness. Decide whether execution-based turn identity requires a bridge schema revision.
4. Establish supported bounded reads on the actual connected/authenticated transport. Preserve request budgets, cancellation, response limits, and metadata-only persistence.
5. Update setup/doctor/uninstall and migration tests for global `cli.json`, package/object plugin entries, runtime disable directives, inline overrides, modified-asset preservation, and V1 coexistence.
6. Port the live provider/TTY fixture and test Home, root/child selection, two clients on one server, foreign-family events, forms/permissions, retries, aborts, background descendants, queued/steered input, reconnect, disable/reload, and completion acknowledgement.
7. Repeat multi-pane, large-output, idle/working, and long-duration resource benchmarks before replacing production V1.

This is an integration migration spanning the JavaScript bridge/recovery, Rust installer/admission, and live qualification harness. The Zellij navigation/exporter and native persistence/reducer infrastructure appear reusable from source inspection, subject to live ownership and acknowledgement tests.

## Verification performed

- Installed binary: `opencode --version` returned **1.19.0**.
- Reviewed current local implementation and V2 official docs.
- Retrieved upstream source at tag **v2.0.29**, commit `042d0832f5f3a73eaa039a58a7d565776f6d4290`, into temporary files outside the repository.
- Ran `node --test integrations/opencode/core.test.mjs integrations/opencode/recovery.test.mjs`: **52 passed, 0 failed**. These are current V1 adapter tests, not V2 qualification.
- Did not run a V2 binary, change production config, or perform a V2 resource benchmark.

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

- [CLI plugin context](https://github.com/anomalyco/opencode/blob/v2.0.29/packages/plugin/src/tui/context.ts)
- [Promise client and transport](https://github.com/anomalyco/opencode/blob/v2.0.29/packages/client/src/promise/generated/client.ts)
- [Execution/session events](https://github.com/anomalyco/opencode/blob/v2.0.29/packages/schema/src/session-event.ts)
- [Session outcome metadata](https://github.com/anomalyco/opencode/blob/v2.0.29/packages/schema/src/session.ts)
- [Message schema](https://github.com/anomalyco/opencode/blob/v2.0.29/packages/schema/src/session-message.ts)
- [Event envelope](https://github.com/anomalyco/opencode/blob/v2.0.29/packages/schema/src/event.ts)
- [Built-in notifications/forms/lifecycle usage](https://github.com/anomalyco/opencode/blob/v2.0.29/packages/tui/src/feature-plugins/system/notifications.ts)
