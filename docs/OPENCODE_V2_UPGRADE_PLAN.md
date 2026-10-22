# OpenCode V2 upgrade plan

Date: **2026-10-17 UTC**. Updated: **2026-10-21 UTC**. Status: **experimental live-status implementation approved; full recovery P0 remains blocked; global rollout is not approved**.

## Approved scope change — experimental live status (2026-10-21)

The user approved a separate, reduced-contract track after comparing Herdr's pane-local V2 implementation. This track may proceed without passing full-recovery P0. It does not mark P0 complete or authorize the full-recovery P1/P2 milestones below. Historical prototype evidence remains version-scoped and unchanged.

### Live-status contract

- Install only with explicit `--experimental-live-status` opt-in. Use independently owned V2 assets and `cli.json`; preserve V1 assets/configuration and unrelated JSONC content.
- Observe only the selected CLI route, bounded cached session ancestry/family/status, cached permission/form identities, and lifecycle events. Never call the SDK, cache `sync`/`invalidate`, history, storage, credentials, or mutation APIs.
- Reuse native pane/process ownership, producer fencing, ordered full-snapshot receipts, lease expiry, and existing sidebar projection. Introduce a separate schema/version pair whose native validator rejects all turn identities, successful completions, and terminal errors.
- Publish positive Working and explicit pending-input hints. Home may be Idle without a conversation. Selected quiescent, incomplete, malformed, oversized, or uncertain state is Unknown, never inferred Idle or Done. Success, failure, and interruption do not create a completion revision or acknowledgement target.
- Bound retained family data to 64 members, pending input to 100 identities, metadata to existing limits, and reporter work to one in-flight and one coalesced pending snapshot. Coalesce streaming wakeups; use one-second cache/route sampling and five-second lease renewal. Reset event hints on route changes, reporter reconnection, and observable connection invalidation; do not claim an upstream connection-generation guarantee the public API lacks.
- Bounds cover Verij-owned traversal, retention, and reporting only. OpenCode owns cache loading, buffering, and freshness; this track provides no pre-parse upstream allocation bounds, complete-family freshness, missed-terminal recovery, durable Done, terminal Error recovery, or authenticated-remote qualification. No extra requests are initiated by Verij.
- Runtime shape guards select the experimental `2.x >= 2.0.32` contract; deterministic tests and any live smoke evidence must state the actual tested binary. Historical 2.0.32 probes and the 2.1.2 source recheck are not production-adapter acceptance.

### Live-status implementation checklist

- [x] L1: Added schema-2 live-only native admission, strict version pairing, reduced capabilities, and adversarial no-Done/no-Error/no-old-Done-replay tests; V1 regressions pass.
- [x] L2: Implemented dependency-free V2 CLI plugin, bounded cache/event projector, coalescing native reporter, lifecycle cleanup, and deterministic attribution/privacy/overload/reconnect tests. Existing native ownership infrastructure is reused, not newly live-qualified.
- [x] L3: Added opt-in setup, V2 `cli.json` registration, migration refusal, independently fingerprinted assets, version-output normalization, executable-independent uninstall, and honest doctor diagnostics. Nine integration tests cover preservation, disable/override handling, and V1 coexistence.
- [x] L4: Ran focused/shared native, JavaScript, and Python regressions. Separate hash-pinned 2.1.2 real-CLI loading smoke passes in private standalone/server modes using a fixture protocol sink, proving loading/Home/renewal/cleanup and selected-session cached form isolation/read-only behavior, not end-to-end native ownership/status or provider execution. After explicit user approval, installed `wasm32-wasip1` for active Rust 1.99.1 and reran `make check`; native CLI and WASM plugin checks both pass. No installed executable, user shared service, or global setup was changed.
- [ ] L4 follow-up: complete native-pane/provider/selected-session live acceptance and matched resource/soak qualification before advertising a qualified runtime matrix. The local WASM toolchain prerequisite is resolved and `make check` passes; runtime/resource acceptance remains outstanding.
- [x] L5: Documented experimental installation and weaker guarantees in `integrations/opencode/v2/README.md`, setup guide, and progress log. Global installation and user-service changes remain a separate approval step.

The remaining plan describes **full-recovery support**. Its stronger transport, identity, complete-family freshness, acceptance, and rollout gates remain blocked unless explicitly satisfied.

## Goal and baseline

Restore pane-local OpenCode monitoring for the already-installed **2.0.32** release without weakening ownership, completion acknowledgement, privacy, or resource limits. Preserve the existing V1 adapter and installation contract.

The [refreshed investigation](OPENCODE_V2_UPGRADE_INVESTIGATION.md) records source references and verification. Baseline Verij commit: `5b6ea1ebd28472b54142d79f87f48854f64ff7c4`. Upstream target: `v2.0.32`, commit `843a6c514e606b436a7fe713422ace0a93ad3d61`.

Current setup rejects the real executable output, `opencode v2.0.32`; both JavaScript and native snapshot admission still reject major 2. Existing tests passing does not establish V2 support.

### Scope

- A separate V2 CLI plugin, bounded observation/recovery, and explicit native snapshot semantics.
- Version-aware setup, doctor, uninstall, asset ownership, and migration diagnostics.
- Private controlled-provider/TTY acceptance and matched resource measurements.
- Documentation and an explicit, reversible rollout procedure.

Do not upgrade/downgrade OpenCode, alter the user's shared service, rewrite unrelated server settings, or install global Verij assets as part of implementation. Mini, noninteractive `run`, desktop/browser clients, and general V1-to-V2 configuration conversion are outside initial support. Unsupported entrypoints must not be advertised as monitored.

## Non-negotiable contracts

1. **Pane-local ownership:** the interactive CLI owns its reporter. Keep native PID/start/TTY/foreground/pane/exporter verification. Shared-server event membership is never pane ownership.
2. **Selected family only:** Home has no conversation/completion; plugin routes have no conversation authority. Inactive OpenCode tabs and foreign families cannot replace the selected family. Child selection resolves the verified root.
3. **Read-only observation:** never reply to permissions/forms, cancel inbox items, mark sessions viewed, navigate OpenCode, or change rules. Sidebar acknowledgement stays a Verij host-local transaction.
4. **No false Done:** require a durable successful root outcome with stable identity, complete fresh family quiescence, and no pending permission/form/inbox work. Child success, terminal toast, cached idle, or session outcome alone is insufficient.
5. **Conservative failure:** partial pages, unknown schemas, missed/gapped events, disconnected transport, exhausted bounds, and inconsistent samples revoke terminal authority. Lease expiry remains Unknown, not exit or execution failure.
6. **Privacy:** retain bounded identities, title, lifecycle facts, and error type only. Never persist prompts, form contents/answers, tool output, provider bodies, credentials, or raw event traces.
7. **Bounded overhead:** retain the existing one-second active/dirty scheduler, ten-second quiet recovery, five-second lease renewal, 15-second lease, one in-flight collection/dirty bit, and independent API/reporter backoff. Keep the 32-invocation/second budget with eight-call burst, four physical requests/four pending, three-second collection deadline, and 128-frame reporter queue unless measured changes are reviewed.
8. **Bounded data:** retain 64 family members, 100 pending requests, 1,024 remembered completions, and bounded metadata/IDs. Carry forward the 64 KiB outcome-metadata ceiling, 1 MiB general response ceiling, JSON depth 32, and 16,384 structural-node ceiling through a V2-qualified pre-parse path. Message count and post-parse projection are not allocation bounds. No unbounded full-history synchronization or V1 database fallback.

## Architecture and decisions

### Separate upstream adapters; shared native infrastructure

Keep `integrations/opencode/bridge.mjs` and `core.mjs` operational for V1. Add proposed V2 files under `integrations/opencode/v2/` (`bridge.mjs`, `core.mjs`, and, if needed, `transport.mjs`) with their own tests. Reuse scheduling, budgeting, queueing, and reporter primitives only through a small tested shared module; do not force both upstream APIs through one recovery algorithm or copy the entire V1 adapter.

Retain agent kind/source `opencode`, native registration, persistence locks, lease handling, and completion ledger infrastructure. Do not add another watcher or sidebar polling loop. Extract shared JavaScript helpers only when both adapters need them, with unchanged V1 regression results.

### V2 identity and snapshot contract

Plan for **snapshot schema 2 for V2**, retaining schema 1 for V1. Validate the version/schema pair; reject accidental cross-major payloads. No shared agent-record schema change is presumed necessary.

The preferred completion key to prove is a bounded namespaced `v2/root-session-ID/idle-marker-ID`. V2 idle markers delimit execution turns including steered prompts; managed shutdown recovery is intended to resume the same turn without a shutdown marker. Private unregistered `serve` does not demonstrate automatic recovery. Revert-clear can wake execution and add a successful marker without new input or model work; define that transition's acknowledgement policy explicitly. Do not use wall-clock timestamps, plugin generations, or ephemeral notification receipt order as identity.

P0 must establish marker availability, ordering, behavior after revert/fork, and how live activity/error identity reconciles with recovered terminal identity. An event ID is not a fallback completion key unless it can be recovered identically after missed events/reload. If no stable bounded recovery path exists, block full V2 support rather than minting synthetic completions. Keep V1 keys untouched and namespace V2 keys to prevent ledger collisions.

Map genuine permission IDs to `Permission`. Map forms to an existing suitable `RequestKind` using lifecycle/ID metadata only; use `Question` only when supported by the contract, otherwise a bounded generic kind. Queued input is outstanding work, not automatically Needs input. Specify interruption reasons individually: user abort must not become Done; shutdown remains recoverable; superseded work must not overwrite newer execution; inactivity needs a verified terminal outcome. Preserve terminal failure separately from tool/retry errors.

### Supported bounded transport is a release blocker

The supplied V2 client exposes cancellation, headers, and streaming activity callbacks, not V1's `requestValidator`. The public plugin context does not expose the connected client's base URL/authenticated fetch constructor inputs. Its normal JSON and declared-error paths parse bodies before returning.

P0 must establish a supported observation path on the actual local/remote authenticated connection. A separately constructed read-only client is acceptable only if endpoint/auth/transport provenance is supported and demonstrated; do not infer it from defaults or expose secrets in manifests. No mutation of shared client methods or global fetch, no credential scraping, and no proxy that changes normal agent execution.

Existing cached metadata may provide hints, but cache emptiness and `sync()` completion are not automatically proof of complete/fresh topology or bounded allocation. If upstream needs a plugin transport hook or narrower endpoint, document that dependency and stop the support-admission milestone until available. Do not silently relax the existing response bounds.

## Implementation phases

Only explicitly checked work below is complete. Each phase must retain evidence and pass its exit gate before dependent work is admitted as supported.

### P0 — Isolated contract and transport spike

**Files:** `prototypes/opencode-v2/README.md`, `CONTRACT.md`, pinned CLI smoke, tagged-SDK characterization, bounded GET prototype, controlled-provider marker experiment, their tests/checkers, and separate sanitized evidence files. Reference existing `prototypes/opencode-runtime/` and `prototypes/h3-opencode/` controls.

- [x] Pin the executable by versioned absolute path and hash. Use disposable HOME, XDG config/data/state/cache, OpenCode config, and Verij state/inventory roots in a disposable cwd; exclude ambient project plugins/instructions and strip inherited server/config overrides.
- [x] Exercise both `--standalone` and an explicitly managed private `serve` plus `--server URL` client. Replace V1 `attach` usage. Record process/TTY layout and clean up only recorded private process births, never the user's service.
- [x] Probe interactive CLI plugin entrypoint/import resolution, options/version, Home and root selection, typed started/succeeded events versus `listen({ details })`, returned cleanup, source/options reload, and ordered config disable/re-enable. Retained CLI frame PIDs match verified foreground client births in both private modes; this is not native pane ownership or a transport qualification.
- [ ] Extend route/event contracts to child selection, inactive tabs, plugin pages, remaining interruption reasons, and missed-event recovery. Separate interactive evidence now proves terminal provider failure and user-abort payloads in both private modes; shutdown/superseded/inactivity remain unqualified. Quiescent reload does not establish continuity of native completion revisions.
- [x] Verify Promise-client shapes on a hash-pinned tagged SDK with an explicitly provisioned private headless connection: unwrapped `session.get`/`active`, parent-filtered paginated `session.list`, `message.list` cursor traversal, location-scoped pending permissions/forms, per-session pending arrays, and pending inbox. `permission.list({ sessionID })` returns pending requests, not rules. This establishes tiny-fixture response shapes, not actual CLI transport provenance, fresh concurrent topology, or cache-side-effect accounting.
- [x] Prove how idle markers are returned. Controlled headless 2.0.32 unfiltered pages return markers in both traversal orders. Follow `cursor.next` without `order`; combining them is rejected. The tagged `MessageListInput.type` excludes `idle`, although response types include it. No invented idle or multi-type filter is used. This proves endpoint availability on the private fixture, not full CLI recovery qualification.
- [ ] Establish stable terminal identity across shutdown/resume, queue/steer, failed execution, missed events, reload, conversation reselection, revert, and fork. Distinguish aggregate sequence cursors from global order. Separate explicit-server CLI evidence now proves failure/user-abort event-to-marker mapping, completed restart, and recovery of one terminal after observer disposal; successful-event mapping, standalone recovery, general missed-event reconciliation, and native revision continuity remain open.
- [ ] Prove fresh complete-family and input/inbox recovery, including a child created while paging. Identify which cache operations fetch implicitly and whether cancellation/budgets cover the physical requests.
- [ ] Demonstrate bounded reads before JSON/error parsing on both selected connection modes, including oversized user/tool/error bodies, stalled/chunked responses, and authentication/path-prefix preservation. Identify any required upstream dependency.

**Progress:** [`prototypes/opencode-v2/`](../prototypes/opencode-v2/README.md) implements the pinned private launch, observation-only CLI plugin, bounded evidence checker, and deterministic safety tests. [Retained loading evidence](../prototypes/opencode-v2/smoke-results.json) passes both local modes: load the plugin with options, observe Home without conversation authority, verify foreground CLI process births, invoke returned cleanup, and retire recorded private processes. The real ELF is pinned independently from its shell launcher. Tagged 2.0.32 skips direct plugin-file entries; the fixture uses a directory with a `tui.js` entrypoint. Private `serve` receives an explicit random password, never retained in evidence. API method presence is only a shape observation, not a recovery contract proof.

**Continuation:** separate [tagged-SDK evidence](../prototypes/opencode-v2/sdk-results.json) confirms that success/error bodies and structural JSON can exceed Verij bounds before caller projection. The prototype GET transport enforces pre-parse bounds, independent caller deadlines, retained physical cancellation slots, prefix/auth preservation, and existing request budgets. [Controlled-provider runtime evidence](../prototypes/opencode-v2/marker-results.json) checks repeated/new-turn identity, completed-turn restart, failure, abort, queue/steer, fork history, forms, and parent pagination. Both queued and steered prompts share one marker in this workload. Forks have reminted historical success markers but no root outcome. A child created during pagination escapes the older cursor; a fresh head detects the change. An independent bounded observer reads the same marker through an explicitly provisioned private connection; selected-CLI/standalone provenance remains unresolved.

**Further continuation:** separate [lifecycle evidence](../prototypes/opencode-v2/lifecycle-results.json) passes eight private-runtime checks for revert stage/clear/commit, replacement input, and shutdown during active work. Revert-clear adds a successful idle marker with zero provider requests; commit can resurface an unchanged older success marker. After active shutdown, restarted unregistered `serve` has empty active membership but preserves unfinished history and historical success. These findings constrain terminal authority and acknowledgement; they do not qualify managed automatic resume. Fifteen additional deterministic evidence/privacy/safety regressions pass. Earlier marker/loading evidence remains unchanged.

**Interactive continuation:** [CLI event/reload evidence](../prototypes/opencode-v2/cli-results.json) passes nine checks in each private mode. Typed callbacks and `listen({ details })` agree on durable started/succeeded identities; selected root survives options/source reload and disable/re-enable. Prompt execution continues while disabled without observer frames; resumed observation produces one callback per channel and event. Eleven JavaScript and fifteen Python regressions cover observer bounds/privacy/nonfatal failure, lifecycle fencing, evidence mutation, and cleanup on trace failure. The observer performs no SDK/cache reads; no bounded transport or native completion continuity is inferred.

**Tagged-client continuation:** [private SDK shape evidence](../prototypes/opencode-v2/sdk-shapes-results.json) passes 12 checks using a copied hash-pinned SDK and separately owned bounded constructor fetch. Genuine pending permissions and forms remain isolated between two directories; per-session arrays and pending inbox IDs agree with fixture control. Successful root outcome and latest idle success coexist with all three pending work kinds, and reads leave them and viewed state unchanged. Twenty-two SDK fetch invocations account for 21 physical GETs and one preaborted no-request invocation. Seven JavaScript and fifteen Python regressions protect transport decoration, source isolation, projection privacy, and evidence integrity. This corrects the earlier `permission.list` rules claim without relaxing the CLI transport blocker.

**Terminal-event continuation:** separate [terminal payload evidence](../prototypes/opencode-v2/terminal-results.json) passes six checks in each of four private real-TTY runs (provider failure/user abort × standalone/private authenticated server). Typed and envelope callbacks agree on durable identity and aggregate-local ordering; failures expose `provider.invalid-request`, and two user Esc presses expose interruption reason `user`, never success. The observer retains only fixed error-type/reason enums and hashed identities, without reading error messages or response bodies. Five added JavaScript and sixteen Python regressions protect projection privacy, malformed payloads, evidence integrity, and private cleanup. The earlier CLI reload experiment was rerun successfully against the updated observer core; its default projection is unchanged. These results do not qualify terminal-to-marker reconciliation, other interruption reasons, or native completion revisions.

**Identity reconciliation continuation:** separate [failure/abort reconciliation evidence](../prototypes/opencode-v2/reconciliation-results.json) passes 11 checks in each of two private explicit-server runs. Tagged `SessionMessage.ID.fromEvent` (`evt_` → `msg_`) exactly maps real CLI failed/interrupted events to recovered idle markers; direct event-ID hashes do not. Repeated reads and a completed-server restart preserve identity. A fixture-controlled second turn after observer disposal has a distinct recoverable marker and no new observer frames. Failure markers retain `provider.invalid-request`; interrupted markers omit the live reason. Seven JavaScript and fifteen Python regressions protect mapping, privacy, identity, fixed bounds, and cleanup; 143 prototype JavaScript, 55 V1 JavaScript, and 106 prototype Python tests pass. Recovery uses fixture-provisioned credentials and bounded fixture HTTP control, not a supported public CLI connection factory or qualified aggregate streaming deadline. Successful-event mapping, standalone recovery, managed active resume, general missed-event recovery, and native revisions remain unqualified. Earlier evidence is unchanged.

**Decisive source recheck:** the [seven-file pinned source audit](../prototypes/opencode-v2/transport-source-results.json) confirms eight boundary facts: public context exposes a constructed client, request options offer no pre-parse hook, standalone generates private auth outside the plugin, and the host can replace its client on reconnect while plugin construction captures the current API. The latter is a source-level concern, not a runtime reconnect failure claim. Eleven additional Python tests pass (117 total); all 198 combined prototype/V1 JavaScript tests remain green. See the [explicit exit decision](../prototypes/opencode-v2/P0_GATE_DECISION.md). The installed target was not changed. An upstream capability and explicit patched/new target decision are required before the actual transport matrix can pass; open P0 qualification checkboxes remain open.

**Installed-release recheck (2026-10-21):** a [seven-file source recheck of 2.1.2](../prototypes/opencode-v2/TRANSPORT_RECHECK_2_1_2.md), commit `a517687fde98a7329ada3ae644a01a9fc6c4e99e`, confirms the same eight transport-boundary facts. The public context still lacks an independent pre-parse observation transport and connection-generation contract. This is source-only evidence, not a repin or 2.1.2 runtime qualification; original 2.0.32 evidence and unchecked gates remain unchanged. Upstream feature request [anomalyco/opencode#57885](https://github.com/anomalyco/opencode/issues/57885) is filed; await maintainer design feedback before an implementation PR. A private patched-target experiment still requires explicit approval; otherwise pause V2 admission. Do not proceed to P1/P2 on the basis of the newer installation.

**Upstream dependency progress (2026-10-21):** [#57885](https://github.com/anomalyco/opencode/issues/57885) is open. Its [automated triage reply](https://github.com/anomalyco/opencode/issues/57885#issuecomment-6324541538) confirms the missing public capability and reports escalation to maintainers. This is not maintainer design approval or an implementation commitment; no direction or timeline is promised. Await maintainer feedback before an implementation PR. No additional P0 qualification checkbox is complete, and no patched-target experiment has been approved.

**Exit gate: BLOCKED.** The [contract table](../prototypes/opencode-v2/CONTRACT.md) distinguishes source observations from runtime proof. Bounded transport provenance, stable terminal identity, and complete-family freshness are unresolved. No production adapter, native admission, installer activation, global installation, or shared-service mutation is included. Do not broaden production admission merely because plugin loading works.

### P1 — Native V2 snapshot and completion semantics

**Files:** `verij-cli/src/opencode.rs`, `verij-cli/src/agent_version.rs`, and native tests; touch shared agent types/store/watcher only if the proven contract requires it.

- [ ] Add explicit schema-1/V1 and schema-2/V2 validation paths, canonical bare-version expectations, bounded V2 identity/input/error projection, and correct capabilities.
- [ ] Retain full-snapshot replacement, native producer fencing, process ownership, ordered receipts, atomic completion bookkeeping, lease expiry, and host-local acknowledgement.
- [ ] Namespace V2 completion keys. Replayed observations/reselection/reload must restore the original revision; a genuinely new execution gets one new revision. Ledger exhaustion stays bounded and conservative.
- [ ] Cover failures/interruption/current-turn replacement so old success cannot survive newer work or become visible from a partial family.
- [ ] Prepare consistent release-family guards: retain existing V1 admission; proposed V2 minimum is **2.0.32**, with later 2.x versions selected by the same contract and runtime shape validation. Reject earlier unqualified V2 releases, major 3, malformed strings, and mismatched schemas. Distinguish the tested binary from the accepted family in docs/doctor.

**Exit gate:** native tests prove V1 compatibility, V2 semantic validation, no duplicate revisions, and strict cross-major/schema rejection. Admission code is not rollout evidence; installer activation remains gated on subsequent phases.

### P2 — V2 CLI bridge and bounded recovery

**Files:** proposed `integrations/opencode/v2/*.mjs` and tests; minimal shared-helper extraction from the existing adapter if justified.

- [ ] Implement `Plugin.define({ id: "verij.monitoring.v2", setup(context) })`, `context.options`, current route tracking, and returned cleanup. Load in the interactive CLI only.
- [ ] Port native reporter startup/receipts/reconnect/lease renewal and disposal. Route, producer, and server replacement invalidate semantic authority and stale queued work.
- [ ] Adapt `event.data`, durable envelopes, permissions/forms, execution/inbox events, and retry metadata. Owned streaming events coalesce; foreign-family events cannot assign membership or trigger unbounded scans.
- [ ] Implement bounded resumable root/family discovery with pagination, cycle/identity checks, freshness brackets, and topology invalidation. Preserve verified input hints without letting them prove complete-family quiescence.
- [ ] Use the P0-qualified bounded transport for terminal/input recovery. Never call full-history `message.sync` as an unbounded shortcut; do not reuse the V1 prefix parser or SQLite schema without a separately proven V2 contract.
- [ ] Implement successful terminal identity, failure/interruption mapping, pending inbox handling, and zero retained message bodies. Missing proof yields Unknown rather than historical Done.
- [ ] Cover cancellation that retains occupied physical slots, event storms during collection, queue overflow, reporter outage, reload races, and complete cleanup.

**Exit gate:** deterministic V2 contract fixtures pass; existing V1 bridge/recovery tests remain green; all upstream reads are accounted for and bounded. No production config is installed yet.

### P3 — Version-aware setup, doctor, uninstall, and migration

**Files:** `verij-cli/src/agent_setup.rs`, proposed `verij-cli/src/opencode_setup_v2.rs` if separation improves clarity, module wiring in `verij-cli/src/main.rs`, `verij-cli/tests/opencode_setup.rs`, and new V2 preservation tests.

- [ ] Normalize executable output at detection only: accept canonical bare semver and the observed exact `opencode v<semver>` format after bounded whitespace trimming; reject ambiguous multi-line/banner output. Do not weaken native/JavaScript payload validation. Check callers before changing the shared version helper.
- [ ] Dispatch V1 to its existing TUI config/assets and V2 to global `cli.json` under the effective OpenCode config directory. Preserve `--config-dir` behavior, `OPENCODE_CONFIG_DIR`, and XDG/global fallback; do not create a project-local CLI config or invent `cli.jsonc`/a V2 `OPENCODE_TUI_CONFIG` override.
- [ ] Detect missing `cli.json` with legacy `tui.json`/preferences. Require OpenCode's first-run migration in that selected environment before setup, or provide an explicitly tested preservation path. Do not suppress migration by creating an empty CLI file first. Report legacy `tui.jsonc` as requiring review rather than assuming upstream migrates it.
- [ ] Install independently fingerprinted V2 assets and one `{ "package": absolutePath, "options": { ... } }` entry under `plugins`; preserve unrelated settings, JSONC syntax/comments, plugin ordering/options, and disable/wildcard directives.
- [ ] Recognize an upstream-migrated V1 Verij entry in `cli.json`. Replace only a positively identified owned reference there; leave V1 `tui.json(c)` and V1 assets intact. Preserve modified files and refuse ambiguous ownership rather than deleting by basename.
- [ ] Extend manifests with adapter/major/schema/config-path provenance. Uninstall must target installed owned entries even when the selected binary is missing or changes major; preserve assets still referenced by the other installation and user-modified files.
- [ ] Diagnose inline `OPENCODE_CLI_CONFIG_CONTENT` overrides that replace `plugins`, ordered disable directives, stale V1 entries, missing assets/reporter, native/exporter prerequisites, and actual live source health separately. Never report installation alone as runtime readiness or silently override a user's disable choice.
- [ ] Exercise no-config setup, pre/post-first-run migration, repeated updates, coexistence, missing/replaced binary, alternate config roots, invalid config, and modified assets. Report live-reload versus conservative restart requirements accurately after P4 verification.

**Exit gate:** real-output detection and V1/V2 setup/doctor/uninstall tests pass without losing unrelated configuration or disabling the other major's installation. Effective overrides have actionable diagnostics.

### P4 — Real-runtime acceptance

**Files:** proposed `prototypes/opencode-v2/` acceptance harness/evidence checker; update or version-dispatch existing H3/runtime harnesses without overwriting historical V1 evidence.

- [ ] Use a controlled local provider, private native panes, production embedded installation, current paired CLI/WASM artifacts, actual records, and actual terminal events. Keep synthetic unit results separate from runtime evidence.
- [ ] Test Home before prompt, root/child selection, inactive CLI tabs, plugin routes, same-cwd panes, two clients sharing one private server, and foreign-family traffic. Only the selected owned family changes its pane row.
- [ ] Test success, recoverable tool failure, retry, terminal model failure, all interruption reasons, native descendants, forms/permissions, multiple pending requests, queue/steer/cancel, and success followed immediately by new work. Visiting cannot answer or cancel anything.
- [ ] Test selection during recovery, paginated family growth, missed events, disconnect/reconnect, reporter outage, quiet lease expiry, hot reload/disable/re-enable, shutdown/resume, process replacement, exit-to-shell, and conversation reselection. No ghost rows or replayed completion revisions.
- [ ] Verify host A and host B acknowledge the same displayed Done independently. Sidebar highlight, OpenCode inactive-tab notification, and native session viewed state cannot acknowledge it.
- [ ] Qualify both standalone and `--server` modes. Add an authenticated remote-style private endpoint, including URL prefix handling, before claiming authenticated remote-server support.

**Exit gate:** independently checked, sanitized runtime evidence with source/artifact/binary hashes passes the supported-mode matrix. Any unsupported mode is explicitly excluded, not inferred from a shared API shape.

### P5 — Resource qualification and regression gate

**Files:** `prototypes/resource-usage/` V2 workload/probe integration, new retained V2 evidence directory, and monitoring documentation.

- [ ] Compare monitoring off/on using identical 2.0.32 binaries, workloads, and pane counts. Separately compare V1 only if an isolated V1 executable/data set is available; historical runs are not matched controls.
- [ ] Measure the complete private server/client/reporter/sidebar process set, request bytes/counts, native writes, queue occupancy, CPU/RSS, and cleanup. Separate shared-server runtime cost from monitoring overhead and adapter limits from upstream buffering.
- [ ] Cover one/two hosts and four/eight panes, quiet idle, streaming bursts, 1/16/64-member families, large user/tool/error bodies, repeated reload/reconnect, and long-lived idle/active cycles. Retain existing abort thresholds and bounded counter summaries.
- [ ] Verify request/concurrency/frame/data bounds, lease renewal without an API query, zero idle acknowledgement proofs, resource recovery after bursts, and no leaked processes/timers/subscriptions. Define soak duration and memory-growth thresholds before running it; report incomplete/aborted runs as such.
- [ ] Run full deterministic regressions and update `integrations/opencode/README.md`, `docs/AGENT_MONITORING_SETUP.md`, and `docs/AGENT_MONITORING_PROGRESS.md` only to the level actually qualified. The separate Zellij resource plan's pending soak/journal work is not automatically completed by this migration.

**Exit gate:** semantic acceptance plus matched short controls and an explicitly passing soak support rollout. No claim that V2 reduces total memory usage without measurements.

## Verification commands

P0 deterministic and isolated loading checks:

```sh
python3 -m unittest discover -s prototypes/opencode-v2 -p 'test_*.py'
node --test prototypes/opencode-v2/*.test.mjs
node --check prototypes/opencode-v2/contract-probe.mjs
# Set ELF to the absolute, canonical 2.0.32 executable path documented by P0.
# Do not use the shell shim or a mutable `latest` path.
python3 prototypes/opencode-v2/smoke.py --binary "$ELF" --output /tmp/verij-v2-p0-smoke.json
python3 prototypes/opencode-v2/verify_evidence.py /tmp/verij-v2-p0-smoke.json
# NODE is an explicit canonical Node executable path.
python3 prototypes/opencode-v2/marker_probe.py --binary "$ELF" --node "$NODE" --output /tmp/verij-v2-markers.json
python3 prototypes/opencode-v2/marker_evidence.py /tmp/verij-v2-markers.json
python3 prototypes/opencode-v2/lifecycle_probe.py --binary "$ELF" --output /tmp/verij-v2-lifecycle.json
python3 prototypes/opencode-v2/lifecycle_evidence.py /tmp/verij-v2-lifecycle.json
python3 prototypes/opencode-v2/cli_contract.py --binary "$ELF" --output /tmp/verij-v2-cli.json
python3 prototypes/opencode-v2/cli_evidence.py /tmp/verij-v2-cli.json
python3 prototypes/opencode-v2/terminal_contract.py --binary "$ELF" --output /tmp/verij-v2-terminal.json
python3 prototypes/opencode-v2/terminal_evidence.py /tmp/verij-v2-terminal.json
# BUN and SDK_SOURCE_DIR are explicit canonical paths; see P0 README.
python3 prototypes/opencode-v2/sdk_shapes.py --binary "$ELF" --bun "$BUN" --source-dir "$SDK_SOURCE_DIR" --output /tmp/verij-v2-sdk-shapes.json
python3 prototypes/opencode-v2/sdk_shapes_evidence.py /tmp/verij-v2-sdk-shapes.json
node prototypes/opencode-v2/sdk-probe.mjs --verify prototypes/opencode-v2/sdk-results.json
# Regenerating SDK characterization needs explicit Bun and pinned source paths;
# see prototypes/opencode-v2/README.md. No runtime downloads are performed.
```

Existing baseline/focused commands:

```sh
node --test integrations/opencode/core.test.mjs integrations/opencode/recovery.test.mjs
cargo test -p verij-cli opencode:: -- --test-threads=1
cargo test -p verij-cli agent_setup:: -- --test-threads=1
cargo test -p verij-cli --test opencode_setup
```

After adding V2 files, include `node --test integrations/opencode/v2/*.test.mjs`, new Rust V2 setup tests, the private runtime/evidence checker commands documented by P0/P4, and the full gate:

```sh
cargo test --workspace -- --test-threads=1
make check
node --test integrations/opencode/*.test.mjs
node --test integrations/pi/core.test.mjs
python3 -m unittest discover -s integrations/agy -p 'test_*.py'
python3 -m unittest discover -s integrations/codex -p 'test_*.py'
git diff --check
```

Retain existing other-agent acceptance gates when shared primitives change. Runtime/benchmark commands must explicitly use the pinned executable and private environment; do not run old V1 harnesses against mutable `opencode latest` and relabel their output as V2 qualification.

## P6 — Reviewed rollout and rollback

- [ ] Review the final support matrix, contract/transport decisions, config diff, completion semantics, and new evidence. Obtain human approval before global installation or shared-service changes.
- [ ] Back up the selected `cli.json`, relevant legacy config, owned assets/manifests, and reporter/exporter paths. Record exact restore locations; avoid copying credentials into repository evidence.
- [ ] Install matching Verij CLI/WASM assets, use qualified version-aware setup, and restart/reload only the intended OpenCode clients according to the tested lifecycle. Check doctor prerequisites and actual fresh live records, then exercise a successful turn and host-local acknowledgement.
- [ ] Roll back on ownership errors, false/replayed Done, lost pending input, config loss, unbounded reads, or resource growth. Disable/remove only the owned V2 plugin entry, stop only its owned reporters, and restore backed-up assets/config where safe. Let stale V2 leases expire conservatively; never forge state or delete unrelated agent records.
- [ ] Reverting Verij alone does not restore monitoring for an already-upgraded OpenCode. Running V1 again requires a separately preserved binary and compatible isolated configuration/data; do not point V1 at V2 storage or automatically stop/downgrade the shared service.

**Completion definition:** V2 support is implemented, bounded, live-qualified in every advertised mode, documented, and explicitly approved for rollout. Writing this plan, fixing version output, or loading the plugin is not completion.
