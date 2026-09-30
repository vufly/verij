# Agent Monitoring Implementation Guide

Status: **implementation specification; no feature code is implied to exist**.

Read [the approved product design](AGENT_MONITORING.md) first. This guide supplies an implementation sequence and concrete contracts. Track actual work, evidence, and human sign-off in the [feature progress log](AGENT_MONITORING_PROGRESS.md). Type/module names and command spellings below are proposed engineering defaults; they may be refined while retaining the approved behavior. Verification gates are engineering work, not unresolved product preferences.

## 1. Current Code and Required Changes

| File | Current responsibility | Monitoring change |
|---|---|---|
| `verij-types/src/lib.rs` | Session/tab snapshots and control constants | Add compatible pane inventory and shared monitoring/control wire types. |
| `verij-plugin/src/main.rs` | Observe Zellij state, export snapshots, handle `switch:<session>` | Preserve full terminal-pane inventory, export navigation capabilities, and accept exact pane-focus requests. |
| `verij-cli/src/fs_watcher.rs` | Watch session snapshots and periodically reconcile Zellij inventory | Keep topology watching; add a separate agent-record update path without querying all sessions per agent event. |
| `verij-cli/src/actions.rs` | Initial attach, Inception Switch, tab navigation, Workspace focus | Add asynchronous, identity-targeted pane navigation with completion/failure reporting. |
| `verij-cli/src/tui/state.rs` | Two-level tree, selection, session collapse | Add agent nodes, tab collapse, stable node keys, agent store, and host-local acknowledgement. |
| `verij-cli/src/tui/tree_format.rs` | Compiled row templates, styles, variables, hitboxes | Add agent formats, summaries, depth/sibling context, and status variables. |
| `verij-cli/src/tui/render.rs` | Flattened tree rendering and status bar | Render three levels and compute branches from topology rather than adjacent flat rows. |
| `verij-cli/src/tui/mod.rs` | Input and snapshot event loop | Handle topology, agent, navigation, focus, and tick events independently. |
| `verij-cli/src/config.rs` | Config, colors, tree formatting, XDG path helpers | Add title policies, agent row styles, and monitoring paths. |
| `verij-cli/src/main.rs` | CLI commands | Add adapter setup/report/diagnostic entry points. |

Suggested native modules under `verij-cli/src/agents/`:

```text
mod.rs          public API and monitoring types
identity.rs     session/pane/process binding and liveness
store.rs        source records, atomic writes, locking, watcher
reducer.rs      source merging and normalized execution state
ack.rs          host-local completion acknowledgement
setup.rs        adapter installation/removal/diagnostics
opencode.rs     version-specific observation normalization
agy.rs          callback/hook normalization
```

Adapter assets can live in a repository-owned `integrations/` directory and be embedded/distributed with the native CLI. Follow existing configuration and packaging conventions when implementing setup.

## 2. Phase 0: Resolve Integration Gates

Complete these short investigations before committing to wire details that depend on unverified APIs. Record versions, payload fixtures, exact native calls, and results alongside tests.

### G1. Zellij identity and per-client focus

Baseline: Zellij **0.45.1**, with `zellij-tile = "0.45"` in this repository.

Verify:

1. Which supported API provides the server/session lifetime identity and pane process identity.
2. How an inner plugin instance and a CLI pipe invocation map to an attached client.
3. Whether an exact client can be addressed when two hosts attach the same inner session.
4. How to observe the host Workspace pane's focus plus the corresponding inner client's effective focus.
5. Native focus behavior for tiled, floating, ordinary stacks, hidden stack-list members, and fullscreen targets.
6. Behavior while a session has no clients, and during its first nested attachment.
7. Session rename, plugin reload, and session resurrection behavior for the chosen identity.

Zellij can remember focused panes in both tiled and floating layers. `pane.is_focused` alone is insufficient. Effective focus must account for the active tab, floating-layer visibility, stack visibility, and the correct client.

**H0 source finding (live routing still to verify):** Zellij 0.45.1 instantiates plugins per client; `get_plugin_ids()` supplies the plugin's server PID and bound client ID, and `get_focused_pane_info()` queries that client. Plugin `focus_terminal_pane`/`switch_session_with_focus` route using the instance's client ID. A CLI action may select the last active display client, and a named CLI pipe is broadcast across plugin/client instances: the pipe's CLI source ID does not identify the host Workspace client. A host-scoped request therefore needs a verified host-client binding and recipient-side filtering before a client-bound instance issues focus. `SessionInfo.creation_time` is elapsed socket-file age, not a stable incarnation token; pair server PID with a verified process start token. Exact host-to-client binding, headless instances, mirroring, and pane placement still need live reproduction.

**Rejected H0 mapping shortcut:** setting `VERIJ_HOST_PANE_ID` on the inner `zellij attach` process does not give each inner plugin instance that client's marker. Tagged `zellij-server/src/plugins/plugin_loader.rs:420-448` creates WASI contexts with `inherit_env()` from the **shared inner server process**. The first attach may seed the server's environment; a second client attached to that server cannot independently supply its marker to its plugin instance through this mechanism. Likewise the supported `host_terminal_env` excludes host Zellij pane identifiers. Use an explicit transport with demonstrated client-scoped binding and validate it live before issuing or acknowledging host-targeted focus; otherwise report navigation unavailable for ambiguous multi-client sessions.

**ClientId reuse caveat:** `zellij-server/src/lib.rs:655-672` chooses the lowest unused display client ID. A disposable live probe detached display client 1 through its own keybinding, observed zero display clients, then attached another client to the same session: it also received ID 1. Each CLI connection separately gets a transient server-assigned ID (`lib.rs:946-977`). Adding a sender ID to a CLI pipe would identify that transient CLI connection, not necessarily the host's attached inner display client. A registration bridge must communicate the display client's assigned ID to its own host wrapper with an attachment-generation token, invalidate on disconnect, and still confirm effective focus. Do not persist a bare ClientId as stable host identity or accept `PipeSource::Cli` as an attachment marker.

**Approved isolated prototype:** [`prototypes/zellij-attach-bridge/`](../prototypes/zellij-attach-bridge/README.md) preserves a patch against exact upstream `v0.45.1`, runnable disposable harnesses and reviewed outputs. The patch adds opt-in identity handoff on the actual attached display socket, per-connection generation records and a typed focus request revalidated on the screen thread. Direct and real nested-host keyboard tests passed for tiled, hidden floating, fullscreen and ordinary stack targets, including stale-token rejection after ClientId reuse. Nested tests used explicit Descend handling in private outer host config; both outer-display PTY and private tmux-backed terminal keys reached the correct inner pane. Earlier injected-input-only checks are retained as diagnostic history. This is a prototype-only dependency, not stock Zellij support or H0 approval; review whole-host/sidebar visits, remaining placements/lifecycle races, user nested policies and production completion semantics before choosing an integration contract.

Native APIs to inspect are `switch_session_with_focus`, `focus_terminal_pane`, and `show_pane_with_id`. The installed CLI exposes **`focus-pane-id`**, not `focus-pane` or `focus-pane-with-id`. Plugin signatures differ between Zellij releases; use the pinned crate's signature.

Respect Zellij's own mirrored-session behavior: a mirrored session can intentionally share focus across clients. Verij must still maintain host-local acknowledgement and must not route an unmirrored focus request to an arbitrary other host. Document inherent Zellij limitations if they remain after investigation.

**Deliverable:** a client-routing/focus strategy and a stable session-incarnation strategy, with a minimal live reproduction for each tricky placement. Do not substitute fixed sleeps or global client counts for this gate.

### G2. OpenCode version and local ownership

Baseline inspected during design: **1.18.32**. Verify that version's plugin context, event shapes, current-state queries, process placement, and lifecycle cleanup. Collect fixtures for:

- Launching an idle TUI before the first prompt.
- Starting and completing a turn.
- Permission asked/replied and question asked/replied/rejected.
- A terminal execution error and an individual recoverable tool error.
- A child/subagent run while its parent remains active.
- Continuing or changing conversation in one TUI.
- Two TUIs in the same working directory.
- TUI attachment to an already-running server.

Determine whether the plugin runs in the actual pane process. If it does not, add a pane-local binding using an available TUI integration/registration mechanism. A server-wide event stream with an inherited pane ID is not sufficient.

**H0 finding (idle foreground binding and local event broadcast verified; family isolation unverified):** upstream `v1.18.32` has a TUI-local plugin API distinct from server plugins (`packages/opencode/specs/tui-plugins.md`). TUI plugins configured via `tui.json` receive `api.route.current`, `api.state.session.{status,permission,question}`, `api.event.on`, and `api.lifecycle.onDispose`. Earlier isolated probes injected `ZELLIJ_PANE_ID` and did not prove pane ownership. Later installed-1.18.32 probes launched two idle TUIs in separate terminal panes of the same disposable Zellij session/cwd, first standalone and then both attached to one isolated headless OpenCode server: local plugin PIDs differed from one another and from the server, actual inherited pane IDs matched terminal inventory, and each process owned a different foreground PTY group; disposal ran on shutdown. A further shared-server probe registered `api.event.on("session.created")` in both TUI-local plugins before creating an empty session via REST; **both callbacks received the same event ID** while their routes were Home. Event delivery alone cannot establish pane ownership; filter to a proven foreground conversation family before interpreting any state change. Validate real work/input/outcome ordering, two-TUI semantic isolation, and startup recovery before claiming monitored status. Do not assume server plugin events and TUI events have identical payloads or lifetime. See dated G2 reproductions in progress log.

A further empty-session test created one session from each TUI-local plugin with `api.client.session.create()` and navigated each local route to its own `sessionID`. Routes stayed distinct while the first TUI received the second TUI's `session.created` event. Equality between local foreground route ID and verified `event.properties.sessionID` classified that event as foreign in this **controlled no-model case**; an early event emitted before the other plugin subscribed was not replayed. `api.state.session.status(ownedID)` returned `null` for these empty sessions, so `null` must not be interpreted as established Idle. Actual navigation, parent/child family filtering, work, input, errors and aggregate Done remain unverified.

OpenCode 2.x uses different event/plugin contracts in the reference bridge. Version detection must select a verified bridge or report an unsupported integration. Do not silently use 2.x event names against 1.x or claim support for shared-server attachment without a proven local binding.

**Deliverable:** a version-specific bridge contract, initial-state recovery mechanism, and tests proving that one TUI cannot overwrite another pane's status.

### G3. Agy callbacks, questions, and neutral hooks

Verify the installed Agy version's:

- Status-line callback on startup and on activity/permission transitions.
- Whether callbacks execute serially and what identity reaches child commands.
- Existing custom status-line composition, exit behavior, and callback disablement.
- Lifecycle payloads in interactive and `--print --output-format stream-json` modes.
- `Stop.fullyIdle` transitions while background tasks remain and after they finish.
- Structured signals for `ask_question` and cancellation.
- Required hook stdout so reporting leaves execution and permissions unchanged.

Do not register a blanket `PreToolUse` hook returning `allow` merely to observe tool use. That changes permissions. Prefer the status callback and non-gating lifecycle signals; any extra observation hook must demonstrate neutral behavior.

**H0 installed-runtime finding:** a workspace-scoped `.agents/hooks.json` in a disposable project loaded under the authenticated Magy profile without editing profile settings. One installed Agy **1.2.13** headless `--print --output-format stream-json` no-tool success emitted `PreInvocation`, `PostInvocation`, then `Stop` with `executionNum=0`, `fullyIdle=true`, empty error, and `terminationReason="NO_TOOL_CALL"`; neutral Stop stdout allowed completion. This confirms only that simple headless path. A binary located beneath a `/1.2.12/` path also self-reported `1.2.13`; detect version from executable output, not install directory. Validate permission/dialog neutrality, `fullyIdle:false`, failures, questions, cancellation, callback composition and interactive ordering separately before advertising those capabilities. Earlier hand-fed synthetic hook samples do not count as installed payload evidence; see the progress log.

In another disposable 1.2.13 headless turn, the model ran a shell command exiting 1, handled it, then completed successfully. Live hook order included `PreInvocation(0)`, `PostToolUse` with empty `error`, `PostInvocation(0)`, `PreInvocation(1)`, `PostInvocation(1)`, `Stop` with `fullyIdle=true` and no error. Do not interpret a shell nonzero exit or `PostToolUse` as terminal agent failure; the aggregate Stop and final result determine outcome. This observation does not establish permission neutrality (the test child used auto-approval), real hook execution errors, background work or question coverage.

**Deliverable:** supported-state/capability matrix and real payload fixtures. Explicit question detection must be verified before advertising complete Needs input coverage; if no neutral signal exists, document the limitation and conservative Unknown handling rather than invent a terminal-text heuristic.

### G4. Magy pane-backed watch lifecycle

Verify both `pane_start` interactive launch and `watch_start` execution in Magy:

- Resolve identity inside the new pane, not from the launching MCP server.
- Validate Magy's `watch_id` (`watch_<hex>`), `state.json` session/pane ID, `runner_pid`/`runner_create_time`, and `MAGY_WATCH_ID`/`MAGY_RUN_ID` bindings against the actual Zellij pane. Do not treat a custom `mux_cmd` PID as a Zellij terminal ID.
- Verify watch `pty.log` streaming NDJSON (`init`, `step_update`, `result`) and final `state.json` status (`completed`, `failed`, `cancelled`) as sources for activity and outcome. Tool-level `ERROR` is not necessarily a failed run; final `result` and watch state decide. A stream `result` describes Agy execution; Magy's watch may still fail during final Git snapshot/diff processing.
- Distinguish Agy execution completion from the watch runner's final process exit and Magy's detached monitor closing its pane.
- Check whether Agy lifecycle hooks add any needed signals in headless mode; watch monitoring must work without an interactive status-line callback.
- Verify synthetic homes use the same reporter executable and runtime directory.
- Ensure pane-less detached runs cannot register against an inherited parent pane.

Magy's `watches.py` persists watch state under its `watches/<watch_id>/` directory; `watch_runner.py` writes NDJSON to `pty.log`, records runner identity and conversation ID, and `watch_monitor.py` finalizes state and closes the pane. Prefer reading these existing records or binding a small observer in Verij. If a new Magy runner hook is required for reliable live pane ownership or outcome, track that change explicitly as a dependency in the Magy repository.

An observer should discover the effective Magy state directory without assuming it shares Verij's runtime path, watch new `state.json` records, and tail only the bound watch's NDJSON from a saved byte offset. Reconcile startup and log rotation/truncation without replaying a completion twice. Do not read `request.json` (it contains prompts) for identity or display. Track a watch only while its live Zellij pane matches the recorded session and pane ID; on terminal watch state, runner exit, or pane auto-close, remove the row when pane/process absence is confirmed. `pane_start` without a watch ID continues through the normal interactive Agy adapter.

**Deliverable:** a working Zellij-backed watch observation path, including cancellation and auto-close cleanup.

## 3. Identity Model

Use typed identities internally rather than passing interchangeable session names, conversation IDs, and pane IDs.

```text
SessionInstanceId   identity of one live Zellij server/session lifetime
PaneKey             (SessionInstanceId, TerminalPaneId)
AgentInstanceId     identity of one monitored agent process/run lifetime
ConversationId      upstream conversation identity, optional before first prompt
TurnId              upstream execution/turn identity, scoped to an agent instance
HostKey             stable marker key from Verij host registry
```

### 3.1 Session and pane identity

`SessionInstanceId` must survive session rename and plugin reload but change when the Zellij session/server is replaced or resurrected. Prefer a verified server process identity consisting of PID plus process start token, qualified by the local boot/user context as needed. A bare PID, mutable session name, or fresh UUID minted on every plugin reload is insufficient.

The exact acquisition mechanism is G1's deliverable. Keep it opaque in wire types so platform-specific process identity does not leak throughout the TUI.

`PaneKey` uses the terminal namespace explicitly. Zellij terminal and plugin panes can have the same numeric ID. All agent navigation must target a terminal ID.

Pane title, tab title, tab position, session name, and cwd are metadata. Resolve them from the newest inventory at activation time. If Zellij exposes a stable tab ID, use it for tab fold/selection identity; otherwise reconcile tab position changes using verified membership information and document the fallback.

### 3.2 Agent identity

Allocate a new `AgentInstanceId` for each process/run lifetime and bind it to:

- `PaneKey`.
- Agent kind (`opencode` or `agy`).
- Process PID and start token, or an equivalent verified runner-owned identity.
- Optional runner identity such as a Magy `watch_id`.

Changing conversations within one live TUI need not replace the pane row or process instance. Starting another process in the same terminal must create a new instance. Native child conversations do not become new pane instances.

Environment variables `ZELLIJ_SESSION_NAME` and `ZELLIJ_PANE_ID` are candidate locators. Validate them against the pane's real process tree or a verified runner binding before accepting observations. This prevents inherited environment in detached descendants or shared servers from creating false rows.

Process ancestry alone is also insufficient: an agent tool can launch another agent with inherited environment while redirecting its input/output away from the pane. Establish that the monitored TUI belongs to the pane's terminal/foreground job, or that a trusted pane-backed runner explicitly owns and renders the run. Use the latter path for Magy watches. The persistent process or runner is the liveness owner, never a short-lived report subprocess.

Session rename can leave old environment values in a process. Use the established runtime binding and current inventory to resolve the new name instead of rejecting a valid instance or writing under the old name forever.

## 4. Inventory and Runtime Storage

### 4.1 Plugin snapshot extension

Preserve existing `SessionSnapshot` fields. Add optional/defaulted monitoring fields, conceptually:

```text
SessionSnapshot
  session_instance_id: optional opaque identity / verified identity inputs
  inventory_revision: optional monotonic exporter revision
  capabilities: optional set of supported inventory/control features
  tabs: existing tabs plus optional stable tab identity and layer metadata
  panes: default-empty list of PaneSnapshot

PaneSnapshot
  terminal_pane_id
  tab identity and current position
  effective title
  is_floating
  is_suppressed
  stack identity/visibility when the API provides it
  exited
  process identity inputs when available
  per-client focus information or data needed for targeted focus queries
```

Do not fabricate `is_stacked` from `is_suppressed`. Export facts the installed API can establish. If a field is unavailable, represent that as unknown/optional and use a targeted query for navigation confirmation when needed.

Cache full pane state on `SessionUpdate` and `PaneUpdate`. `TabUpdate` must not accidentally erase agent-pane inventory until another pane event arrives. Export changes to background panes as well as the active pane title. Keep legacy active-pane naming behavior compatible.

Retain atomic session snapshot writes. Agent records are owned by the native path; do not add agent-config parsing or native process scanning to WASM.

### 4.2 Paths

Suggested new layout:

```text
<agent-runtime>/agents/v1/<agent-instance-id>/
    identity.json
    state.json

<verij-state>/agent-monitoring/hosts/<host-marker-key>.json
```

- `<verij-state>` uses the existing XDG state helper.
- `<agent-runtime>` uses `$XDG_RUNTIME_DIR/verij` when suitable, with a user-qualified temporary fallback when unavailable.
- An absolute `VERIJ_AGENT_STATE_DIR` override can keep all adapters and Magy profile homes on one runtime location. The TUI, setup, and reporter must use the same resolver.
- Do not place native agent records beneath synthetic profile `$HOME` by default.
- Existing WASI-mapped `/tmp/verij/states` resolution remains the session inventory path. Native agent files do not need to be visible inside WASM.
- Use opaque IDs in filenames. Keep display names inside JSON, avoiding session-name sanitization collisions.

`identity.json` contains the immutable instance binding. `state.json` contains a map of source-specific snapshots, the reduced execution facts, the current completion identity/revision, and a monotonic record revision. Keeping all mutable facts in one file avoids a partially committed update across source and aggregate files.

For each report, acquire a per-instance lock, validate and merge that source's observation, run the shared reducer, and atomically replace the complete `state.json` using a unique temporary file on the same filesystem. Release the lock before returning. Keep source snapshots logically separate inside the file so the Agy UI callback and lifecycle hooks cannot overwrite each other's authority. Registration must also serialize concurrent first reports for the same verified process lifetime, yielding one instance rather than duplicate rows.

The reporter owns aggregate completion revisions; each TUI reads them and applies its own acknowledgement. It must not allocate a new completion revision independently in every sidebar. A failed write leaves the previous whole state readable. Readers ignore an incomplete registration until both identity and a valid state exist.

Bound record size and normalize the fields retained. Monitoring needs IDs, lifecycle facts, titles, and short reasons, not transcripts, prompt bodies, command outputs, account email, or credentials.

### 4.3 Native event loop

Extend the TUI input channel to carry distinct events, for example:

```text
TopologyUpdated(snapshots)
AgentRecordsChanged(instance_ids)
LivenessUpdated(results)
HostFocusUpdated(focus)
NavigationFinished(request_id, result)
```

Agent events update the agent store and affected tree metadata. They must not call `read_all_states` or run `zellij list-sessions` on every report. Session discovery and periodic topology reconciliation remain on their existing slower path.

The native reporter persists observations even when no TUI runs. The watcher reads initial records on startup, then watches changes with a periodic reconciliation fallback. Coalesce redundant activity/title refreshes, but preserve completion, input-request, error, and exit edges.

## 5. Reporter and Observation Contract

Proposed CLI surface:

```text
verij agent setup opencode
verij agent setup agy
verij agent doctor
verij agent uninstall opencode
verij agent uninstall agy

verij agent report opencode
verij agent report agy --source ui
verij agent report agy --source lifecycle --event pre-invocation
verij agent report agy --source lifecycle --event stop
```

Reporting reads JSON from stdin; titles and other agent-provided strings do not become shell command fragments. Adapters are bounded and nonblocking from the agent's perspective. A missing/unavailable reporter must not break an agent turn.

### 5.1 Normalized source records

The report subcommands accept their verified version-specific payloads and store a common source record under the source map in `state.json`. A representative normalized source record is:

```json
{
  "schema_version": 1,
  "agent_instance_id": "instance-opaque-id",
  "source_id": "agy-ui",
  "source_revision": 42,
  "observed_at_ms": 1780000000000,
  "conversation_id": "conversation-opaque-id",
  "conversation_title": null,
  "turn_id": "execution-7",
  "activity": "working",
  "pending_requests": [
    {"id": "confirmation-opaque-id", "kind": "permission"}
  ],
  "background_task_count": 0,
  "successful_completion": null,
  "execution_error": null,
  "capabilities": ["activity", "permission_requests"]
}
```

This is a normalized storage example, not the raw upstream Agy JSON contract. `identity.json` binds the instance to a verified `PaneKey` and process lifetime. It contains the agent kind and process/runner identity, so a report cannot silently rebind an existing instance to another pane.

Define capabilities explicitly, including where applicable activity, permissions, questions, conversation titles, successful completion, terminal failure, and lifecycle/liveness. An absent capability must not be confused with an observed negative value.

Normalize partial source input before writing a complete source snapshot. A title-only event must not clear pending input requests or completion data. Unknown optional fields are forward-compatible; unsupported schema versions or invalid required identity fields are rejected without deleting the previous valid record.

### 5.2 Ordering and duplicates

- Use a single ordered producer queue where an adapter has a persistent process, notably OpenCode.
- Give state-changing source observations monotonic source revisions. Reject a lower or duplicate revision from the same source lifetime.
- Coalesce only redundant refreshes, never an input/completion/error edge across a later state transition.
- Wall-clock timestamps support diagnostics, not authoritative event ordering.
- One-shot Agy hooks need upstream execution/step identity plus serialized updates in the reporter. A lock prevents lost updates but alone does not repair semantically out-of-order hook delivery.
- Do not let a delayed stop from execution N clear Working or Needs input for execution N+1.
- Keep UI and lifecycle source authority separate. The newest arrival is not automatically the most authoritative observation of every field.
- Validate upstream ordering in G3. If concurrent callbacks cannot be ordered reliably, treat their partial facts as hints and reconcile from an authoritative current-state observation before clearing stronger state.

The reporter/reducer retains the completion identity/revision in `state.json` rather than relying on the TUI receiving every filesystem notification. A coalesced watcher update or sidebar restart must still recover an unread completion. Record the completed turn identity alongside the revision so replaying its terminal event is idempotent. Periodic cleanup must acquire the same instance lock and recheck process lifetime before removing records, preventing a cleaner from racing with registration/reporting.

## 6. Reducer Rules

Use a pure, testable reducer for normalized execution facts. Keep host-local presentation as a small second step over that output.

### 6.1 Source authority

| Fact | Preferred authority |
|---|---|
| Pane location/title/existence | Current Zellij inventory. |
| Agent-process existence | Verified process/runner liveness. |
| Interactive Agy current activity and permission dialog | Current UI callback snapshot. |
| Interactive Agy execution outcome | Verified lifecycle stop, qualified by turn and `fullyIdle`. |
| Magy watched-run activity/outcome | Bound watch runner's `pty.log` NDJSON and watch `state.json`, validated against the live Zellij pane. |
| OpenCode current work/input/outcome | Verified events plus current-state reconciliation for the locally bound conversation family. |
| Completion acknowledgement | Host-local acknowledgement store only. |

Do not merge unrelated conversation families simply because they share an OpenCode server. Within a pane's owned family, track child work and blocking requests without independently publishing child Done states.

### 6.2 Display derivation

Apply these rules in order, using only observations valid for the current instance/execution:

1. If process or pane is definitively gone, remove the row.
2. If a valid explicit blocking request remains, display Needs input.
3. If current work, retry, relevant child activity, or tracked background work remains, display Working.
4. If the current execution ended in a terminal failure and no newer execution supersedes it, display Error.
5. If the latest successful aggregate completion is still current and unacknowledged by this host, display Done.
6. If a valid observation establishes no work or input request, display Idle.
7. Otherwise display Unknown.

Failure/stop normalization should resolve or invalidate old requests and work belonging to the terminated execution. Precedence is not permission to keep abandoned requests forever. A late activity refresh cannot override a terminal result for the same execution.

Suggested transition expectations:

| Input | Result |
|---|---|
| Initial idle snapshot | Idle; no fabricated completion. |
| New prompt/execution | Working; previous completion no longer displays as current. |
| Permission/question opened | Needs input; retain current turn identity. |
| One of several requests resolved | Needs input until all relevant requests resolve. |
| Requests resolved, work continues | Working. |
| Recoverable tool error | Working while agent handles it. |
| Provider retry | Working with optional `retrying` detail. |
| Child completes while parent works | Working. |
| Successful root completion, child/background work remains | Working; defer aggregate Done. |
| Successful completion with no remaining work | Idle execution plus one new completion revision. |
| User visits completed pane | Done becomes Idle only for that host. |
| Terminal execution failure | Error; no successful-completion increment. |
| User cancellation | Normalize from actual upstream outcome; never label an aborted execution as a successful completion merely because it became idle. |
| Repeated idle/stop event | No additional completion revision. |
| Agent closes, shell survives | Remove row. |

### 6.3 Liveness and source health

Track source health separately from semantic state. Source silence and process exit are different facts.

- A live event-only source can legitimately be silent through a long turn or idle prompt.
- Use process identity checks, an explicit adapter lease/heartbeat where supported, or current-state queries to detect reporter failure.
- Do not expire an Agy idle callback solely by a short TTL when callbacks only occur on changes.
- When previously authoritative state can no longer be trusted, retain the known pane row as Unknown while its agent process remains verified alive.
- Do not infer Error solely from disappearance; normal exits also disappear.
- If topology querying fails, preserve last valid inventory as uncertain instead of declaring every agent dead.

Use a short, bounded absence grace for transient pane movements. Prefer a second independent inventory observation or explicit live-pane query, rather than waiting forever for an event that may never arrive. Explicit process exit/pane-close evidence can bypass the grace.

### 6.4 Acknowledgement race handling

At navigation start, capture the target instance and currently observed completion revision. On confirmed focus, acknowledge only revisions actually observed while a valid visit exists. Never blindly write an arbitrarily newer revision from a shared record that changed during the request.

If the pane completes again during navigation, process the new completion with a fresh effective-focus observation. If another turn begins before confirmation, do not resurrect the old Done display.

Persist acknowledgements per stable `HostKey` with atomic updates. A host rename preserves the key. A host deletion can remove its acknowledgement file. Two sidebars for one host must not corrupt or decrease acknowledgement revisions.

## 7. Adapter Implementation Notes

### 7.1 OpenCode bridge

The bridge should:

1. Establish verified local pane/process binding and publish presence on load, even before the first prompt.
2. Determine relevant root conversations and child relationships from the supported API.
3. Seed current activity, pending requests, current title, and any recoverable active execution identity.
4. Subscribe to version-specific lifecycle and metadata events.
5. Send ordered updates asynchronously; bound child reporter time and queue growth.
6. Coalesce repetitive activity/title refreshes without dropping terminal/input edges.
7. Dispose listeners and unregister or expire the binding on exit/reload.

For 1.x, verify `session.status`, `session.idle`, `session.error`, permission events, and question events against actual payloads. A generic idle event does not by itself distinguish a successful completion from cancellation/error. Track execution context and terminal outcomes.

For 2.x, the reference TUI bridge uses execution/form events and TUI ownership filtering. Treat it as a separate versioned adapter, not a drop-in implementation for the installed baseline.

A shared server may expose events for all TUIs. Never use “any server conversation is busy” as a pane status without filtering the locally bound family. Conversation switching must update title/ownership without duplicate pane rows.

### 7.2 Agy interactive callback

Normalize documented `agent_state` values conservatively:

| Upstream observation | Normalized fact |
|---|---|
| `thinking`, `working`, `tool_use` | Activity is working. |
| `initializing` | Starting/unknown until verified ready; must not fabricate Done. |
| `idle` | No foreground activity; combine with requests, tasks, and outcome before deriving display. |
| `tool_confirmation_pending = true` | Outstanding permission request. |
| `tool_confirmation_pending = false` | Clear the callback-owned permission request only. |
| `task_count > 0` | Tracked background work remains. |
| `conversation_id` changes | Update conversation metadata without changing process/pane identity. |

An idle callback is not enough to increment completion. Use the lifecycle outcome and a validated transition/turn identity. If the UI callback cannot name a turn, the native adapter must correlate it with lifecycle records without letting an old callback clear a newer execution.

To compose with a user's status-line command, tee the same stdin payload to the reporter and the original command, preserving the original command's visible stdout. Respect the original execution environment and invocation semantics. If there is no custom command, use the supported `stack_with_default` behavior when verified, preserving the built-in status line. Validate whether empty reporter output introduces unwanted blank lines.

Only persist monitoring fields from the callback; discard quota/account/transcript-related fields that the monitor does not need. Do not use the terminal title callback as a transport requirement, since title behavior is independently configurable.

### 7.3 Agy lifecycle hooks

Recommended base events are PreInvocation and Stop. PostInvocation can refine activity but must never mean Done. Additional hooks need the neutral-output verification described in G3.

For Stop:

- Qualify it by the execution identity/counter and process instance.
- Check `terminationReason` and `error` before classifying success.
- Honor `fullyIdle = false` and outstanding task state.
- Emit completion only when successful aggregate execution is known to have ended.
- Return the upstream-neutral response; do not force the agent to continue.

Short-lived hook subprocesses are reporters, not the monitored agent PID. Bind their observations to the persistent Agy process or explicit runner lifetime.

### 7.4 Setup and diagnostics

Setup should be idempotent and own identifiable adapter files/config entries. Preserve unrelated plugins, hooks, and user formatting. Repeated setup must not wrap a status-line command recursively or install duplicate event producers.

Use absolute paths or a verified executable resolver that still works inside Magy profile homes and after shell startup differences. Uninstall removes only Verij-owned integration entries and restores the composed status-line command when its stored predecessor is still applicable.

`verij agent doctor` should report:

- Detected agent and Zellij versions.
- Installed bridge version and supported capabilities.
- Reporter executable and shared runtime resolution.
- Pane/process binding availability.
- Whether an Agy interactive callback and required headless hooks are installed.
- Magy profile integration readiness where it can be checked reliably.

Do not claim a successful integration merely because setup wrote a file. Include a reporting self-check that does not start an agent turn or alter user pane focus.

## 8. Navigation Implementation

Introduce an identity-bearing target and a request ID, conceptually:

```text
AgentTarget { pane_key, agent_instance_id }
NavigationRequest { request_id, host_key, target, observed_completion_revision }
```

Run navigation outside the blocking input/render path. Feed its completion back into the TUI. A new request supersedes the old request; stale completions cannot change active-session state, acknowledge completion, or refocus a previous target.

### 8.1 Same inner session

Resolve the latest pane inventory and use an exact terminal-pane native focus operation routed to the intended inner client. Let Zellij select the target's current tab. Reveal the correct layer/stack member without blind toggles.

### 8.2 Different inner session

Extend `verij_control` with a versioned structured command carrying the target session and terminal pane identity. Prefer a native `switch_session_with_focus` route when G1 confirms it handles the required target/client semantics.

After connection, reconcile effective visibility/focus and issue an idempotent reveal/focus correction if necessary. Do not trust a tab index captured before the pane moved. Do not preserve the current `position > 0` shortcut: tab zero is a valid explicit target.

### 8.3 First attachment

Reuse the existing initial-attach machinery, but observe completion of the correct Workspace attachment before pane targeting. Initial attach must occur only once; a transient lack of focus confirmation must not write another `zellij attach` command into an already-attached pane.

### 8.4 Host focus and confirmation

Resolve the host Workspace pane using its actual terminal ID, leveraging the existing pane-resolution helper. Prefer exact-ID focus to `move-focus right` after switching, since the already-focused pane and custom host geometry can change.

Confirm the whole effective-focus condition described in the product design. If no fresh passive per-client observation is available, use a bounded targeted query. Timeouts produce a visible navigation error and no acknowledgement. A command accepted by the inner plugin is an acknowledgement of receipt, not proof of visible focus.

Preserve existing session/tab activation and resurrection flows while making the new agent path explicit. Maintain compatibility for `switch:<session>` while introducing structured commands, and advertise new control capabilities so a new CLI can diagnose an old plugin rather than silently fail.

## 9. TUI and Configuration Implementation

### 9.1 State

Add `TreeNode::AgentPane` and explicit stable `NodeKey` values. Store collapsed sessions and collapsed tabs separately or in one typed-key set. Avoid restoring selection using only `(session_name, tab_position)` once pane nodes exist.

Join current inventory with live agent instances using `PaneKey`. Do not put raw adapter parsing in `rebuild_nodes`. The reducer produces a pane-level view model containing agent kind, title inputs, state, completion revision, and short detail.

When several native conversations belong to one process/pane, aggregate them into one row. If multiple independent agents compete for the same pane, use verified foreground/runner ownership; do not choose whichever report arrived last. Treat unresolved ownership as Unknown rather than duplicate misleading rows.

### 9.2 Rendering

Current `render.rs` infers the last tab by inspecting the next flattened node. That breaks once the next node can be an agent child. Compute parent, depth, sibling index, and last-sibling flags while building the tree.

Compile `agent_format` once, like the current session/tab templates. Extend the rendering context with the variables in the product design and host-derived summaries. Apply selection/active row bases consistently, then status/text span styles.

Keep fold hitboxes attached to actual rendered marker cell ranges, including wide glyphs, indentation, clipping, and scroll offset. Rebuild row-indexed hitboxes after structural changes. Animation redraws must not rebuild identities or reset selection.

### 9.3 Configuration

Add an `AgentsConfig` with a global title-source policy and per-kind overrides. Suggested typed representation:

```text
TitleSource = Pane | Conversation | Agent
AgentsConfig { title_sources, opencode: AgentOverride, agy: AgentOverride }
AgentOverride { title_sources: optional list }
```

Validate each override independently and retain current behavior that malformed tree templates fall back to defaults with a startup warning. Add `agent_styles` with normal/selected/active/both states and color tokens for status styling, reusing existing muted/error/spinner colors where appropriate.

Existing configurations remain valid. `config init` should write the complete new defaults for new configurations without overwriting existing user files.

## 10. Implementation Sequence

1. **Integration gates:** resolve G1–G4 and capture representative fixtures. Finalize process/client identity before designing around mutable names or inherited environment.
2. **Shared types and pane inventory:** export compatible full pane snapshots and capabilities; verify hidden/background panes and rename/reload behavior.
3. **Reporter/store/reducer:** add native runtime records, identity validation, source ordering, liveness, completion revision, and acknowledgement persistence.
4. **Exact pane navigation:** implement client-scoped, cancellable navigation and verify tiled/floating/stacked/cross-session targets before wiring click actions.
5. **Three-level TUI:** add nodes, folds, stable selection, formatting, status animation, and parent summaries using fixture records first.
6. **OpenCode adapter:** support the verified installed API, recovery mid-turn, family filtering, and local pane binding.
7. **Agy and Magy integration:** compose interactive callback, install neutral hooks, and bind Zellij-backed Magy watches to their NDJSON/state and synthetic homes.
8. **Setup/diagnostics/documentation:** make installation repeatable, advertise supported versions/capabilities, and update user-facing controls/configuration docs.
9. **Acceptance verification:** run focused automated checks and the live matrix below. Record any upstream limitations accurately.

The feature is complete only when both MVP agents and Zellij-backed Magy watches pass the applicable scenarios. A tree fed solely by synthetic records or process-name detection is not a completed monitoring feature.

### 10.1 Human verification checkpoints

**Stop after each checkpoint below.** An engineer records implementation evidence, versions, test commands/results, live observations, limitations, and open questions in [the progress log](AGENT_MONITORING_PROGRESS.md), then requests human verification. The reviewer runs or observes the listed checks in a disposable inner session where possible, records **approve** or **rework** with concrete feedback, and explicitly authorizes the next stage. Do not treat automated checks, a successful agent run, or silence as approval. On rework, fix the issue, rerun affected checks, and request verification again. Keep the log current after each work session and each review.

| Stop | After sequence step(s) | Human verification guideline and expected result | Feedback to capture |
|---|---|---|---|
| **H0 — integration contract** | 1 (G1–G4 feasibility) | Review recorded versions, real payload fixtures, ownership/client-routing evidence and focused reproductions. With two attached clients and tiled, floating, stacked, and fullscreen targets, confirm the proposed focus route addresses the intended client; verify a Magy watch's pane/runner binding, not its launcher's. G4's *implemented* observer and cleanup are verified at H4. If a probe cannot be run, leave H0 blocked and state exactly what is missing. | Are any client-focus, question-detection, shared-server, or runner-binding limits unacceptable? Which verified API/identity strategy is approved? |
| **H1 — topology and navigation** | 2–4 | Build CLI/plugin, launch a disposable inner session with a tiled pane, hidden float, and stack member. Inspect exported terminal IDs, titles, tab membership and stable instance identity across rename/reload. Invoke exact-pane navigation through a temporary test harness or documented development command across tabs/sessions and on first attachment; each target becomes visible, focused and accepts input, with no placement change or focus theft by an older request. Confirm old snapshots still render sessions/tabs and a failed focus does not acknowledge completion. | Which pane or host combination fails? Did an action affect another attached client or accidentally attach twice? |
| **H2 — tree and local completion** | 5 | Using fixture-backed records (clearly marked synthetic), verify three-level folds, keyboard/mouse hitboxes, narrow widths and collapsed parent summaries. Complete a fixture turn in a background pane: selecting its row leaves Done; confirmed activation clears Done only in this host. Repeat with second host, restart sidebar, move/remove row, and check selection and acknowledgement persist correctly. | Are statuses, summaries, title precedence, fold/selection behavior, and visit semantics understandable and correct? |
| **H3 — OpenCode live integration** | 6 | Install the OpenCode bridge via its preliminary setup path, then start two OpenCode TUIs in different panes of the same cwd. Exercise initial idle, working, explicit input, child work, success, error, shared-server attachment, and process exit. Verify only their own pane rows update; a child completing does not mark its parent Done; no agent row remains after exit to shell. Capture actual version and any unsupported cases. | Any missing event, wrong-pane status/title, or surprising Done/input behavior? Approve OpenCode coverage before integrating Agy. |
| **H4 — Agy/Magy live integration** | 7–8 | Preserve an existing Agy status-line command and run interactive Agy, Magy `pane_start` in a profile home, and Zellij-backed `watch_start` through completion and cancellation. Verify permissions remain unchanged, background tasks delay Done, bound watch NDJSON/state drives status, auto-close removes its row, and pane-less detached runs make no launcher row. Check `agent doctor` and repeated setup/uninstall. | Which states cannot be observed in installed Agy? Any lost status line, permission change, wrong binding, or orphan row? |
| **H5 — release acceptance** | 9 | Run the repository checks in §11.3 and applicable live scenarios in §11.2, including two-host independent acknowledgements, pane movement/closure races, sidebar restart, and rename/resurrection. Inspect diagnostics and documentation of any upstream limitation. Mark feature complete only after reviewer signs off the full matrix. | Approve release or list failed scenarios and reproducible steps. |

For each stop, note **pass / fail / not run** per scenario, environment and exact commands, plus an observation (screenshot, sanitized fixture, or concise written result). Never paste prompts, credentials, or raw private transcripts into the log. Keep subsequent steps blocked until the reviewer provides feedback and approval; an unresolved failure remains a blocker rather than being silently deferred.

## 11. Verification

### 11.1 Automated checks

Use meaningful tests around reducer transitions, identities, navigation decisions, and rendered tree behavior:

- Old snapshots decode with no agent children; new snapshot serde roundtrips preserve pane identity.
- Renaming session/pane and moving panes preserves identity; process restart and pane reuse invalidate it.
- Detached descendants and unrelated shared-server sessions cannot hijack a pane association.
- Duplicate/out-of-order events, old-turn completion, multiple pending requests, child completion, retries, and background tasks derive the right state.
- A sidebar starting mid-turn or after completion reconstructs state from records.
- Two hosts independently acknowledge the same completion; acknowledgement survives host rename and sidebar restart.
- An acknowledgement racing with a newer completion does not consume the newer revision.
- Agent exit removes its row while its shell pane remains; transient inventory omission does not remove it prematurely.
- Title source inheritance, overrides, empty values, invalid entries, and control-character normalization work without changing identity.
- Nested folding, row deletion/movement, scroll offsets, wide glyph hitboxes, and sibling branches remain correct.
- A stale navigation result cannot focus or acknowledge a superseded target.
- Agent updates do not launch session inventory subprocesses per report.
- Setup is idempotent, composes existing callbacks, and uninstall preserves unrelated settings.

Adapter fixtures should come from the installed versions and include documented schema version/capability expectations. Test neutral hook stdout and existing status-line passthrough explicitly.

### 11.2 Live acceptance matrix

| Scenario | Expected observation |
|---|---|
| Start interactive OpenCode or Agy in an existing pane | One agent child appears under its actual tab, including before first prompt when supported registration is active. |
| Two agents in the same cwd and different panes | Distinct rows and statuses, with no attribution crossover. |
| Long tool/model call with little terminal output | Working remains valid; no timeout-generated Done. |
| Permission or supported question prompt | Needs input persists through visiting and clears only on resolution. |
| Parent turn with native subagents | One pane row; child completion does not prematurely mark Done. |
| Agy background task survives model stop | Working until verified aggregate completion. |
| Complete a turn in a background pane | Done appears and contributes to parent summary. |
| Highlight Done row with sidebar cursor | Done remains. |
| Activate Done row successfully | Correct pane is visible and accepts keyboard input; only this host acknowledges it. |
| Complete a turn while already effectively focused | This host can immediately acknowledge; another host still sees unread completion. |
| Focus by native Zellij navigation | Acknowledgement follows verified effective focus, not stale remembered focus. |
| Hidden floating pane in another tab/session | One activation reveals and raises the correct pane. |
| Collapsed stack member, including hidden stack-list representation | One activation expands that member without unstacking it. |
| Another pane is fullscreen | Target is revealed and focused. |
| Two hosts attached to one inner session | Request reaches intended client subject to Zellij mirroring semantics; acknowledgements remain independent. |
| Click targets rapidly | Last request wins; delayed earlier actions cannot steal focus. |
| Target moves or closes during navigation | Follow verified move, or report unavailable target without false acknowledgement. |
| Quit agent back to its shell | Agent child disappears; session/tab remain. |
| Magy interactive profile | Status/title updates work with synthetic home. |
| Magy `watch_start` in Zellij | Watched pane appears and reports working/outcome from bound NDJSON/state without requiring TUI callback. |
| Watched run completes or is cancelled and pane auto-closes | Node disappears; no retained completion tombstone. |
| Pane-less detached Magy run | No false child on launcher pane. |
| Restart sidebar while agent runs | Correct current state and host acknowledgement recover. |
| Rename inner session or reload plugin | No duplicate/ghost agent rows and navigation uses current session name. |
| Resurrect session or reuse pane ID | Old agent state/acknowledgement does not attach to new process. |
| Disable/break reporting while agent stays alive | Ordinary navigation survives; unreliable semantic status is diagnosed rather than fabricated. |
| Resize narrow sidebar and scroll through folded tree | Correct branches, status glyphs, hitboxes, and stable cursor. |

### 11.3 Repository checks

After feature implementation, run:

```bash
cargo check --workspace
cargo test --workspace
make check
```

`make check` includes the WASM-target check that native-only workspace checks do not replace. Build runnable CLI/plugin artifacts with `make dev` before live verification. Document version-specific adapter test commands once the bridge language and packaging are fixed.

For documentation-only changes to these specifications, validate Markdown links and the diff; building Rust does not verify prose.
