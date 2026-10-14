# Pi Monitoring Support Plan

Status: **approved by the user; P0 and P1 complete. P2–P4 implementation and acceptance remain pending**.

P0 evidence: [runtime qualification and established contract](../prototypes/pi-runtime/README.md), [sanitized traces](../prototypes/pi-runtime/results.json), and [independent verification summary](../prototypes/pi-runtime/summary.json). Compiled and Node-based Pi 1.2.0 passed the two-extension-order matrix. This is lifecycle qualification, not production monitoring readiness.

## 1. Objective and scope

Add Pi as a supported Verij agent without changing the existing Session → Tab → Agent pane experience. Install once, launch `pi` normally, and observe activity, blocking extension dialogs, successful aggregate completion, cancellation, and terminal failure.

The initial target is **Pi 1.2.0 (`@earendil-works/pi-coding-agent`) on Linux, in interactive TUI mode, inside a verified Zellij terminal pane**. Both the compiled Pi distribution and the Node-based CLI need qualification. Reading the installed documentation and tagged source establishes the integration contract, not runtime qualification.

Preserve the approved [monitoring design](AGENT_MONITORING.md) and [resource constraints](AGENT_MONITORING_RESOURCE_FIX_PLAN.md):

- Use stock Zellij and existing Verij navigation and host-local acknowledgements.
- Remove rows when the agent process or pane definitively exits.
- Do not infer completion from silence, permission requests from tool calls, or questions from assistant prose.
- Do not alter permissions, prompts, tools, keybindings, session transcripts, or other extensions.
- Monitoring failures must not block Pi's work.

Initial exclusions: print/JSON/RPC modes, SDK embeddings, detached jobs, remote agents, transcript history, older Pi API families, and separately executing subagents without their own verified pane. Extensions that launch untracked background work remain outside aggregate-completion guarantees.

## 2. Findings and existing extension points

### Verij

The shared machinery already exists:

| Area | Current implementation | Planned change |
|---|---|---|
| Agent identity and reducer | `verij-types/src/agent.rs` supports OpenCode, Agy, and Codex. | Add `AgentKind::Pi` and explicit `pi` source authority. |
| Registration and reporting | `verij-cli/src/agent_cli.rs`, `agent_store.rs` | Add Pi dispatch and reuse verified registration, ordered transactions, and completion deduplication. |
| Long-lived pane-local reporter | `verij-cli/src/opencode.rs` | Follow the bounded NDJSON/receipt pattern, not the OpenCode conversation API or its adapter-specific completion ledger. |
| Adapter lifecycle commands | `verij-cli/src/agent_setup.rs` | Add `setup pi`, `doctor pi`, and `uninstall pi`. |
| Titles and display labels | `verij-cli/src/config.rs`, `tui/tree_format.rs` | Add `[agents.pi]`; existing `agent_names.pi` is only a display placeholder. |
| Liveness and watcher | `verij-cli/src/process.rs`, `agent_watcher.rs` | Reuse foreground ownership; add Pi observer-health handling. |
| Navigation and visits | Existing activation and acknowledgement paths | Reuse unchanged; verify Pi through the generic ownership path. |

No new daemon, session inventory protocol, tree level, or agent launcher is needed. Older readers already decode unknown agent kinds conservatively; verify that adding Pi preserves wire compatibility rather than bumping the shared schema unnecessarily.

### Pi 1.2.0

Verified contracts from the installed documentation and the upstream `v1.2.0` source:

- Extensions load inside the actual Pi process. `process.pid` identifies that process; inherited `PI_SESSION_ID` and `ZELLIJ_*` variables do not prove ownership or identify the current conversation.
- The user agent directory defaults to `~/.pi/agent`, with `PI_CODING_AGENT_DIR` override. A directory under `extensions/` with `index.ts` or `index.js` autoloads.
- `session_start` includes startup/reload/new/resume/fork reasons. `session_shutdown` covers quit, reload, and session replacement; it is **not always process exit**.
- `agent_end` is a low-level boundary. Retries, compaction, queued work, and extension continuations can follow it.
- `agent_settled` is the final notification after automatic continuation stops. Its payload has `aborted`, but **no success/error outcome**.
- `turn_end` and `agent_before_settle` expose `outcome: completed | aborted | error`. They are actionable boundaries, so the observer must return nothing and never request continuation.
- `ui_prompt_start/end` expose blocking extension UI, including `select`, `confirm`, `input`, `editor`, and `custom`. The runner emits one outermost start/end pair, not separate pairs for nested dialogs, and supplies no request ID or permission provenance.
- `session_info_changed` supplies the normalized session name. Read conversation identity from `ctx.sessionManager.getSessionId()` and name from `getSessionName()`.
- `VERSION` is exported by the Pi package. Use the running process's version, not only the executable selected during setup.

References:

- [Extension documentation](https://github.com/earendil-works/pi/blob/v1.2.0/packages/coding-agent/docs/extensions.md).
- [Event types](https://github.com/earendil-works/pi/blob/v1.2.0/packages/coding-agent/src/core/extensions/types.ts).
- [Run settlement and continuation](https://github.com/earendil-works/pi/blob/v1.2.0/packages/coding-agent/src/core/agent-session.ts).
- [UI prompt observation](https://github.com/earendil-works/pi/blob/v1.2.0/packages/coding-agent/src/core/extensions/runner.ts).
- [Configuration and version exports](https://github.com/earendil-works/pi/blob/v1.2.0/packages/coding-agent/src/config.ts).

## 3. Proposed architecture

```text
Pi process / pane
  └─ Verij extension: lifecycle events → metadata-only snapshots
       └─ verij agent pi: verified ownership + ordered source transactions
            └─ existing agent store / watcher / reducer
                 └─ existing tree, summaries, navigation, host-local Done
```

New files:

- `integrations/pi/index.ts`: thin Pi event bindings and runtime/version guards.
- `integrations/pi/core.mjs`: pure event-state projection and bounded reporter transport.
- `integrations/pi/core.test.mjs`: deterministic state and transport tests.
- `integrations/pi/README.md`: installation, capabilities, exclusions, recovery, and qualification.
- `verij-cli/src/pi.rs`: native snapshot validation, ownership, ordering, health, and reconnect state.
- `verij-cli/src/pi_setup.rs`: owned installation, uninstall, and doctor.
- `verij-cli/tests/pi_setup.rs`: isolated lifecycle-command tests.
- `prototypes/pi-runtime/`: reproducible real-Pi qualification harness and retained sanitized evidence.

Keep the extension dependency-free apart from Pi's host-provided API and runtime built-ins. Do not install a second copy of Pi as a runtime dependency. Start subprocesses and timers in `session_start`, never in the extension factory; dispose them idempotently in `session_shutdown`.

### Ownership and transport

Pass the actual Pi PID and candidate pane ID to the native reporter. Resolve the owning session from current verified inventory and foreground terminal ancestry, rejecting ambiguous matches. A session name is a hint, not an immutable identity. Registration must reject redirected/background children even when they inherit the parent's Zellij variables.

Use one reporter subprocess per active extension runtime, with private stdin/stdout pipes. The agent terminal receives no reporter protocol or error output. Native registration keys remain server incarnation + terminal pane + process birth.

The reporter owns monotonic source epochs, single-producer fencing, atomic updates, and receipts. Full snapshots replace missing facts explicitly; omitted turn, error, or pending-request fields must not preserve stale state through partial-update semantics.

Define a versioned, strict snapshot whitelist: running Pi version, conversation ID/name, aggregate run ID, activity, pending request metadata, qualified outcome, and observed background work count. Exclude prompts, assistant text, tool arguments/results, dialog titles/content, provider bodies, and transcript paths. Validate bounded identifiers, frame size, request count, and logically inconsistent outcomes before publication.

### Completion and reconnect identity

Treat one aggregate run ending at `agent_settled` as the completion unit. Individual model/tool turns are not completion units. Create an opaque aggregate run ID and preserve it through retries and low-level `agent_start/end` cycles. Define its exact creation/reset points from the phase-zero traces, including queued follow-ups.

Persist only minimal adapter metadata in Verij's native runtime: aggregate identity, validated latest snapshot, source order, and a bounded completion deduplication record. Do not append monitoring entries to the Pi transcript. Heartbeats, transport reconnects, and `/reload` must not mint a new completion for the same run.

A reconnect handshake may restore same-process, same-conversation state. A reload that cannot prove pending UI or active-run continuity must report Unknown instead of guessing. A newly resumed historical conversation starts without an unread historical completion. Process restart and pane/server reuse invalidate the old association and acknowledgements.

On observer teardown or loss, invalidate semantic authority without claiming process exit or success. Keep deduplication metadata so a validated reconnect cannot replay Done. Retain a known live row as Unknown when the observer is unavailable; process/pane liveness removes it independently.

## 4. Event-to-state mapping

| Signal | Proposed behavior |
|---|---|
| `session_start` | Register only in `ctx.mode === "tui"` with verified ownership. Publish current conversation metadata and conservative current activity; never synthesize historical Done. |
| `before_agent_start` / `agent_start` | Enter Working. Clear prior run outcome on a genuinely new aggregate run, not on every low-level retry. |
| `turn_start`, tool execution start/end | Remain Working while the run is active. Tool failure alone is not a terminal execution error. Ignore token/update streams. |
| `turn_end`, `agent_before_settle` | Capture the latest current-run structured outcome only. Never publish Done at these boundaries. Return no mutations or continuation. |
| `agent_end` | Do not publish completion or idle; aggregate work may continue. |
| `ui_prompt_start` | Publish Needs input with one generated request ID and a generic UI/confirmation/question category derived from prompt kind. Do not label a generic confirm dialog as a verified permission request. |
| `ui_prompt_end` | Clear the matching outer prompt. Restore active work or the previous execution state; closing a dialog does not itself complete a run. |
| `agent_settled` | Publish successful completion only with current-run success evidence, no abort, no outstanding prompt, and no tracked work. Aborted runs become Idle without completion; validated terminal failure becomes Error. Missing/conflicting outcome evidence becomes Unknown. |
| `session_before_compact`, `session_compact`, `session_compact_failed` | Track compaction as a work overlay. Automatic compaction cannot create an intermediate Done. Manual compaction does not create an agent-run completion; failure/retry mapping follows qualified event fields. |
| `session_info_changed` | Update the conversation name without changing run or agent identity. |
| `session_tree` | Retire branch-sensitive execution facts conservatively; browsing old entries does not replay completion. |
| `session_shutdown` | Dispose observer resources. New/resume/fork retire conversation-specific facts; reload preserves only validated continuity metadata; quit does not manufacture Done. |

`agent_settled.aborted === false` alone is insufficient success evidence: settlement can follow an error or an early exception. Reset outcome candidates for each new aggregate run, and verify final outcome again after extension continuations. Do not publish a terminal Error during a provider retry that later succeeds.

Needs input covers blocking dialogs observed through Pi's extension UI API. External browser approval, direct terminal readers, third-party UI that bypasses that API, and ordinary assistant questions are not automatically observable. Document these limits rather than intercepting user UI methods or permissions.

## 5. Installation and diagnostics

Proposed public commands:

```bash
verij agent setup pi
verij agent doctor pi [--session SESSION]
verij agent uninstall pi
```

Add `--pi-bin` for version diagnostics and use existing `--config-dir` as the **Pi agent directory**. Resolve it as explicit directory → `PI_CODING_AGENT_DIR` → `~/.pi/agent`; match Pi's tilde/relative-path semantics and store canonical absolute paths.

Install an owned `extensions/verij-monitoring/` directory containing `index.ts` and `core.mjs`. Store `adapter.json` and `installed.json` in an owned sibling directory outside extension discovery. Include absolute reporter/runtime/inventory defaults and fingerprints. Explicit launch-time `VERIJ_AGENT_STATE_DIR` and `VERIJ_STATES_DIR` overrides win.

Prefer conventional autoloading without editing `settings.json`. Preserve other extensions, resource filters, trust settings, credentials, models, and keybindings. Refuse to overwrite an unrelated existing directory or modified owned assets. Setup/uninstall must be idempotent, use safe writes, and preserve user-modified files, including invalid UTF-8. Do not delete an owned directory while preserved code still references its configuration.

A fresh normal `pi` launch is the supported activation path; qualify `/reload` separately. `--no-extensions`, explicit resource exclusions, another agent directory, or an already-running Pi can prevent loading. Setup cannot override these decisions.

Doctor reports separately:

- Detected executable version and qualified version set; initially qualify 1.2.0 only.
- Asset/manifest integrity and reporter availability.
- Agent directory, runtime and inventory paths, writability, and path agreement.
- Verified pane inventory and exporter compatibility.
- Matching live Pi records, process ownership, observer lease, and last reported running version.
- Capabilities and exclusions, including generic blocking-UI support rather than universal permission/question coverage.
- Structural installation readiness versus evidence that a running Pi actually loaded the adapter.

Unsupported versions should disable observation conservatively and appear in diagnostics. Do not claim every Pi 1.x release is compatible based on its version number.

## 6. Resource and failure budgets

Use event-driven observation, not transcript scans, screen scraping, provider polling, or per-event Zellij CLI calls.

Initial targets, to be measured before release:

- One reporter, one in-flight snapshot, and a fixed-capacity semantic queue per Pi process.
- Coalesce identical refreshes, not completion, cancellation, input, or error transitions. Queue overflow degrades to Unknown and triggers a conservative reseed, never invented success.
- Unchanged observer lease renewal every five seconds; expire authority after fifteen seconds. Lease renewal requires no Pi history or topology query.
- No token-level publications. Target at most ten native snapshot writes per second under tool-event bursts, with prompt attention and terminal edges prioritized.
- Bound startup/receipt waits and shutdown flushes. Retry connection failures with exponential backoff capped at thirty seconds; never await transport from a Pi lifecycle handler.
- Handle child spawn errors, broken pipes, malformed receipts, reporter death, and duplicate runtimes without unhandled rejection or terminal spam.
- Reuse existing host proof scheduling; no extra passive visit queries when there is no unread completion.

Keep Pi lease handling narrow; do not make this feature depend on a broad rewrite of the unfinished resource roadmap. Existing OpenCode behavior must remain covered if a small shared transport/health helper is extracted.

## 7. Implementation sequence and gates

### P0 — Qualify the contract before finalizing state projection

Build a private runtime probe with isolated agent directory, runtime paths, PTY, and deterministic local provider. Record metadata-only event order for both compiled and Node-based Pi 1.2.0.

Exercise success, parallel/nested tools, tool error followed by success, provider failure, retry success/exhaustion, cancellation, queued steering/follow-ups, `agent_before_settle` continuation, compaction, outer/nested UI prompts, session switching, and `/reload`. Verify prompt timing and final outcome capture with the observer loaded before and after another extension.

**Gate:** retained traces establish aggregate run boundaries and success/error/abort discrimination. Uncovered paths remain explicitly unsupported or Unknown; do not implement optimistic Done.

**Completed:** 112 real-TUI scenario runs (28 × two distributions × two observer orders), with 1,608 sanitized event records and successful private cleanup. Eight JavaScript privacy/trace tests, twelve Python evidence/cleanup tests, strict TypeScript probe checks, and the independent evidence gate pass.

**Implementation consequences:** one aggregate starts at `before_agent_start` and can contain repeated `agent_start/end`; decide completion only at `agent_settled` using the latest current final `agent_before_settle` outcome. A post-run exception can settle non-aborted after a successful `turn_end` without any final pre-settlement boundary: that case remains Unknown, not Done. Cancellation skips the pre-settlement boundary. Continuation proposals are extension-order-sensitive; resumed work invalidates an earlier pre-settlement success candidate until a fresh final boundary arrives. Outer UI pairs cover nested dialogs, including blocking UI while execution is idle. Session replacement/reload recreates the extension runtime within the same Pi process. Mid-work reload continuity and other exclusions are recorded in the prototype README.

### P1 — Native Pi kind, protocol, and reducer support

Add Pi kind/source authority, CLI dispatch, title overrides, module wiring, strict snapshot validation, native source fencing, reconnect/deduplication state, and observer-health handling. Reuse generic foreground verification and existing store transactions.

**Gate:** unit tests cover source isolation, monotonic ordering, stale producer rejection, completion deduplication, unknown-kind compatibility, and unchanged existing adapters.

**Completed:**
- `AgentKind::Pi` and explicit `pi` source authority (priority 10) integrated in `verij-types/src/agent.rs` with wire compatibility preserved for unknown agent kinds.
- Title resolution and configuration override `[agents.pi]` added to `verij-cli/src/config.rs`.
- Hidden CLI dispatcher `verij agent pi` added in `verij-cli/src/agent_cli.rs`, guarded against duplicate nonblocking live producers with `.pi.lock` and global session resolution.
- Source reservation: generic `verij agent report` rejects `source_id: "pi"` to prevent bypassing the strict native Pi protocol and epoch fencing.
- Strict nested snapshot validation and privacy whitelist in `verij-cli/src/pi.rs`: closed enums (`PiActivity`, `PiRequestKind`, `PiErrorKind` with static terminal failure text), `deny_unknown_fields` on all frames/snapshots/outcomes, max 1 pending request, no dialog/detail content, no permission provenance, no arbitrary error message bodies.
- Terminal outcomes (`Completed`, `Error`, `Aborted`) require conversation ID, run ID, and explicit quiescent idle activity with zero pending requests and background tasks.
- Reconnect and handshake state: atomic persistence of `PiAdapterState` (bounded to 32KiB/128 entries ledger), monotonic ordering, turn receipts correlating epoch/revision, and deduplication of completed runs scoped by structured tuple key `(conversation_id, run_id)`. Saturated ledgers degrade conservatively to Unknown. Handshake uses `source_epoch >= 1, source_revision >= 1`.
- Native observer lease expiry in `verij-cli/src/agent_watcher.rs`: marks live row `Unknown` while clearing pending requests and background task count, preserving process-live row and deduplication metadata.
- Verification evidence: 29 `verij-types` tests pass, 17 unit tests in `pi::tests` pass, `agent_watcher` lease degradation test passes, `tree_format` custom Pi row test passes, and 8 isolated CLI integration tests with controlling PTY and private tempfile inventory pass in `pi_reporter.rs`.

### P2 — Pi extension and bounded transport

Implement thin event bindings, pure projection, full snapshots, nonblocking queue, receipts, backoff, lease renewal, and idempotent teardown. Avoid UI mutation, transcript mutation, and permission interception.

**Gate:** deterministic event/transport tests and the P0 real-Pi traces produce expected states, with no premature Done or monitoring-caused agent failure.

### P3 — Setup, uninstall, doctor, and documentation

Embed owned assets, add Pi configuration directory resolution and version qualification, implement ownership-safe lifecycle commands, and document capabilities and recovery. Update README/progress links and remove the claim that `pi` is only a future integration once qualification passes.

**Gate:** isolated command tests pass for repeated setup/uninstall, custom paths, user modifications, disabled discovery, unsupported versions, missing reporter, and missing/legacy inventory.

### P4 — End-to-end and resource acceptance

Run real Pi inside stock Zellij with real exporter inventory and sidebars. Use the prototype provider to avoid credential-dependent tests. Separate synthetic inventory/projection tests from actual pane and multi-host acceptance.

**Gate:** all acceptance cases below pass or are recorded as explicit capability limitations. Preserve versions, asset hashes, workload details, sanitized results, and private-process cleanup evidence. Do not change global Pi/Verij installation during planning or automated qualification.

## 8. Acceptance matrix

| Scenario | Required result |
|---|---|
| Normal startup; sidebar starts later | One Pi row with correct pane, title, and current state; no historical completion replay. |
| Two Pi processes in the same repository | Separate verified rows and conversation state; no cwd-based attribution. |
| Inherited Zellij variables in a detached/background child | No row assigned to the launching pane. |
| Successful multi-tool run | Working until final settlement; one completion revision, Done until visited. |
| Retry, compaction, queued work, extension continuation | No intermediate Done; final current-run outcome wins. |
| Provider error; cancellation; recoverable tool error | Error for qualified terminal failure, Idle without Done for abort, continued Working/success for recovered tool failure. |
| Blocking extension dialog; nested dialog | Needs input until outer prompt ends; visit does not clear the request; no fabricated permission provenance. |
| Heartbeat/reconnect/reload after acknowledged completion | No new completion revision or replayed unread badge. |
| New/resume/fork/tree navigation/name change | Correct metadata and execution reset without process-row duplication or historical Done. |
| Rename/move pane, floating/stacked/background tab | Existing native targeting behavior and stable selection remain intact. |
| Two Verij hosts | Visiting acknowledges only the requesting host and only the observed revision after verified focus. |
| Process exit, kill, pane close, pane/PID reuse, resurrection | Old row disappears; old acknowledgements never transfer to the new lifetime. |
| Reporter loss or adapter reload | Pi keeps working; row degrades conservatively and reconnect is bounded. |
| Print/JSON/RPC/export/help/version; unsupported API | No observer process or incorrect pane row; no stdout protocol contamination. |
| Idle and event bursts across multiple panes/hosts | Bounded processes, timers, queue, record writes, memory, and existing visit-query budgets. |

Planned checks: `cargo test --workspace`, native/WASM build checks, Pi setup/reporter tests, `node --test integrations/pi/core.test.mjs`, existing adapter regressions, then the private real-Pi/Zellij harness. Exact commands and measured budgets belong in the implementation evidence once those files exist.

## 9. Definition of done

Pi monitoring is ready when normal interactive launches work after owned setup; activity, blocking extension UI, completion, abort, error, exit, and recovery obey the shared contracts; generic titles and host-local visits work unchanged; installation is reversible; and reproducible runtime/resource evidence accompanies the documented supported-version and capability matrix.

P0 and P1 are complete. The next implementation task is P2 Pi extension and bounded transport (`integrations/pi/*`), not global installation or a UI redesign.
