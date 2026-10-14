# Agent Monitoring Resource Fix Plan

Date: **2026-10-13**

Status: **P0 and P1 complete (2026-10-14). P2 scheduler foundation started; P2 remains in progress. P3–P6 remain pending.**

## 1. Objective

Make agent monitoring resource usage bounded during idle periods, streaming output, multi-host observation, and long-lived sessions. Preserve responsive navigation, explicit input states, process ownership, and host-local Done acknowledgement.

The [resource investigation](AGENT_MONITORING_RESOURCE_INVESTIGATION.md) establishes two priorities:

1. Passive Zellij CLI polling amplifies state updates across plugins. Four `zjstatus` bars raised server CPU to 35.3% at five read-only queries per second; elevated load reached 272.4% with growing queued memory.
2. OpenCode events trigger unthrottled full reconciliation. The unchanged bridge issued approximately 164 API requests per second in a synthetic 20-event-per-second workload.

The latest WSL termination has no retained OOM evidence. Acceptance therefore requires measured resource improvement and long-run stability rather than claiming an unverified crash cause has been eliminated.

## 2. Required invariants

- Use stock Zellij and Verij-owned changes.
- Cached focus observations may trigger or suppress verification, but cannot establish a Done acknowledgement.
- A visit proof must still establish the requesting host's Workspace focus, current inner client, effectively focused target, process lifetimes, and observed completion revision.
- Reusing a proof is restricted to the same bounded operation. It must not become a durable positive-focus cache.
- Preserve distinct permission, question, terminal-error, completion, cancellation, and exit semantics. Coalescing may discard redundant refresh work, not fabricate or erase semantic transitions.
- Retain the existing bounded-polling exporter design; do not restore persistent pane/tab/session event subscriptions.
- Monitoring failures must not block agent execution or alter permission behavior.
- Periodic reconciliation must recover missed notifications without unbounded queues or permanent busy loops.

## 3. Proposed operating budgets

These are initial implementation targets. Record final values and measured results before rollout.

| Path | Initial target |
|---|---|
| Passive whole-host visit proof | Zero CLI calls when no unread completion needs acknowledgement |
| Pending acknowledgement | At most one proof attempt per host every two seconds; retry only while an eligible completion remains |
| Failed acknowledgement proof | Exponential backoff, capped at 30 seconds; reset on meaningful focus, binding, or completion change |
| Routine topology discovery | One `list-sessions` query per ten seconds, shared within a host process; filesystem updates remain immediate |
| OpenCode active recovery | At most one scheduled reconciliation per second, with one in-flight operation and one pending dirty flag |
| OpenCode idle recovery | At most one recovery reconciliation per ten seconds |
| OpenCode family topology recovery | Cache verified ancestry; rescan on topology invalidation and a slower fallback, initially ten seconds |
| OpenCode unchanged lease renewal | Every five seconds, below the existing 15-second lease expiry; no API query solely to renew the lease |
| OpenCode total API traffic | Initial per-bridge sustained ceiling of 32 requests/second, including recovery scans; bounded bursts and concurrency |
| Filesystem notification ingress | Fixed-capacity signal/path queue; notification callbacks never block |
| Control result journal | Rotate at 8 MiB; retain at most two generations plus the active file |

Explicit user navigation remains prompt and independently bounded by existing deadlines. Background backoff must not delay a new activation. Request ceilings must cover actual API calls, not merely the number of reconciliation functions invoked.

## 4. Implementation phases

### P0 — Establish the actual monitoring baseline

**Files:** `prototypes/resource-usage/verify.py`, `prototypes/resource-usage/opencode_probe.mjs`, and the existing navigation/agent verification harnesses.

Implemented instrumentation also lives in `verij-cli/src/resource_metrics.rs`, with
opt-in counter hooks at existing CLI, visit, record-write, and watcher ingress paths.
Supporting fixtures and the retained-evidence checker live under
`prototypes/resource-usage/`; H1 verification now records asset hashes and elapsed time.

- [x] Extend measurements to include the real Verij sidebar and visit protocol, not only synthetic `list-panes` load.
- [x] Count CLI calls by operation and target server, OpenCode API calls by endpoint, native record writes, proof attempts, and incoming/coalesced filesystem events.
- [x] Collect fixed-size counters and interval summaries. Avoid per-event logs that create another growing workload.
- [x] Record stock binary version, installed asset hashes, pane/tab/plugin/client counts, warm-up duration, and workload duration.
- [x] Compare monitoring enabled and disabled under otherwise identical workloads. Use private sockets, configurations, and runtime directories.
- [x] Preserve topology/detachment checks, private-process cleanup, and the experiment RSS abort threshold.

**Exit criteria:** retained baseline covers one and two hosts, four and eight tabs with status bars, idle agents, and active event streams. Separate synthetic scheduling counts from real runtime measurements.

**Evidence:** [retained P0 reports and reproduction](resource-baselines/README.md) and
[investigation follow-up](AGENT_MONITORING_RESOURCE_INVESTIGATION.md#p0-baseline-follow-up-2026-10-14).
The complete synthetic host/tab matrix includes real sidebars, native reporters,
fresh visit proofs, and host-local completion acknowledgements. Actual OpenCode
runtime pairs cover one host/four tabs and two hosts/eight tabs. The longer
two-host/eight-tab synthetic run hit the unchanged 512 MiB Zellij RSS guard and
cleaned up successfully; a shorter guarded run retained every workload phase.

### P1 — Remove recurring expensive visit proofs

**Files:** `verij-cli/src/tui/mod.rs`, `verij-cli/src/tui/visits.rs`, `verij-cli/src/navigation.rs`, `verij-cli/src/agent_cli.rs`, `verij-cli/src/fs_watcher.rs`.

- [x] Schedule acknowledgement verification only for current, navigable, unread completions. Do not invoke `verified_visit` for an empty candidate set.
- [x] Use existing focus heartbeats to update routine presentation conservatively and to invalidate pending work. Keep presentation hints separate from verified visits.
- [x] Filter acknowledgement candidates against the current bound session and likely focused terminal before starting expensive proof work.
- [x] Give each host one coalesced acknowledgement job, a minimum retry interval, and failure backoff. Invalidate it on navigation, binding replacement, process exit, or candidate revision change.
- [x] Produce one fresh, internally validated proof per acknowledgement operation. Pass that proof to the native acknowledgement path instead of invoking the five-command visit sequence twice.
- [x] Recheck target instance, host/binding generation, process lifetimes, completion revision, and local request currency before committing acknowledgement. Reject superseded proof results.
- [x] Slow periodic `list-sessions` discovery to the proposed ten-second interval. Keep filesystem-driven topology reads and explicit user actions responsive. Document the longer fallback discovery window for sessions with no exporter.
- [x] Do not replace fresh single-display checks with indefinitely cached client counts.

**Exit criteria:** no idle visit RPCs; one proof sequence per successful passive acknowledgement; bounded retries; manual inner navigation still acknowledges eligible Done; another host cannot consume the badge.

**Evidence:** `resource-baselines/p1-1host-4tabs.json`,
`resource-baselines/p1-2hosts-8tabs.json`, and
`resource-baselines/p1-opencode-1host-4tabs.json`. Idle/working agents issue no
passive visit RPCs. Idle CLI traffic is 0.1 calls/second per host, entirely ten-second
discovery: approximately 98% below P0's 5.5 calls/second per host. Actual inner
keyboard navigation uses exactly one five-call proof. Hosts acknowledge independently;
sidebar/other-pane focus preserves Done. A real second outer display rejects
acknowledgement, with three one-command attempts in ten seconds, and a meaningful
focus change restores one successful proof after the extra display leaves.

Each monitor has one running job, one replaceable pending job, one-slot notification
ingress, and one replaceable result. Minimum two-second spacing survives cancellation
and candidate/focus churn; failed work backs off through 2/4/8/16/30 seconds. Cached
hints contain no proof. The non-cloneable operation proof is consumed once and checks
binding/journal/ownership/focus, process lifetimes, and freshness again at commit.
Passive identity/current-completion checks run under persistence locks; explicit
navigation retains older-observed-revision semantics. Validity starts at the earliest
native observation, so the final outer reply cannot refresh an old inner proof.

No-exporter fallback discovery can take up to ten seconds. Exporter filesystem
events remain prompt; cached discovery authority still expires after 2.5 seconds,
and live producer lifetime evidence can overturn cached Exited data. The existing
H1 cross-session registration failure is retained in `resource-baselines/p1-navigation.json`;
all preceding ownership, focus, and acknowledgement gates passed, and the failure
was already reproduced without this implementation.

### P2 — Budget OpenCode observation work

**Files:** `integrations/opencode/bridge.mjs`, `integrations/opencode/core.mjs`, `integrations/opencode/core.test.mjs`, `verij-cli/src/opencode.rs`, and `verij-cli/tests/opencode_setup.rs`.

- [x] Replace event-triggered full collection with a dirty-state scheduler. Track the next eligible refresh, one in-flight collection, and one pending invalidation.
- [x] Ensure an event arriving during collection invalidates the sampled generation and schedules one later retry. Do not lose the dirty flag on a rejected snapshot.
- [ ] Preserve immediate bounded event projection for validated permission/question and activity edges. Reconcile terminal outcomes conservatively when current evidence is incomplete.
- [x] Avoid allocating a microtask or request graph for every streaming update. Repeated activity and part updates should only set an already-pending flag.
- [ ] Cache validated root/family ancestry for the current route and producer generation. Invalidate it on relevant create/delete/reparent events, route changes, reconnect, or inconsistent responses.
- [ ] Separate cheap current-status recovery from full ancestry/history recovery. Charge every endpoint invocation to the per-bridge request budget.
- [ ] Inspect the installed OpenCode API/cache contract for metadata-only or incremental outcome recovery. Use local structured message metadata where verified. Do not repeatedly fetch full tool-output bodies to rediscover unchanged completion metadata.
- [ ] If startup recovery requires message retrieval, use the narrowest supported query and explicit allocation limits where the API permits them. A message-count limit alone must not be presented as a byte bound.
- [ ] Keep family-size, response-time, and concurrency bounds. When a complete family cannot be verified within the budget, retain conservative state and retry later rather than increasing request rate or manufacturing Done.
- [x] Back off failed API recovery and reporter reconnects independently. Reset backoff on meaningful ownership changes or explicit new work.
- [x] Deduplicate against the last successfully delivered semantic observation, not only the current queue tail. Distinct state edges must remain ordered.
- [x] Renew unchanged source health every five seconds without a full API reconciliation. Initially reuse the existing snapshot protocol; add a dedicated renewal frame only if compatibility and measured write cost justify it.
- [x] On reconnect, publish conservative presence and recover current state before restoring semantic authority. Keep the native source epoch and completion ledger rules.
- [x] Dispose timers, listeners, queued work, and in-flight requests on reload/exit.

**Exit criteria:** streaming frequency no longer determines full-query frequency; idle recovery and lease renewals have independent rates; supported input/completion/error transitions remain correct; native writes for unchanged state are limited to lease renewal.

**Initial P2 milestone (in progress):** active/dirty collection starts are separated
by at least one second; quiet idle recovery uses ten seconds; unchanged lease renewal
uses five seconds after a native receipt. A shared endpoint token bucket permits
32 invocations/second with an eight-call burst, four physical requests in flight,
and four pending requests. Cancellation does not prematurely free a physical slot.
Tests cover timer/dirty coalescing, generation rejection, backoff, endpoint accounting,
lease renewal without queries, ordered input edges, reconnect, and disposal.

Retained initial results: `resource-baselines/p2-synthetic-start.json` and
`resource-baselines/p2-opencode-start.json`. One-member synthetic streaming at 20 and
100 owned events/second both measured 8.0 API calls/second. Quiet synthetic idle
measured 0.8 calls/second and 0.2 committed native writes/second. Existing ancestry
and bulk-message reconciliation is still used behind the scheduler; verified family
caching, resumable large-family recovery, and metadata-only outcome recovery remain
required before P2 can be marked complete. Budget exhaustion yields conservative
state; the current three-second collection deadline cannot finish every 64-member
full scan at the request ceiling.

### P3 — Bound watcher ingress and directory work

**Files:** `verij-cli/src/fs_watcher.rs`, `verij-cli/src/agent_watcher.rs`, `verij-cli/src/inventory.rs`, `verij-cli/src/magy.rs`, and `verij-cli/src/agent_store.rs`.

- [ ] Replace unbounded notification channels with nonblocking bounded signals. A one-slot dirty signal is sufficient where the worker already performs a complete reconciliation.
- [ ] For incremental reads, keep a bounded set of dirty instance IDs. Overflow must set a full-rescan flag, not grow the set or block the callback.
- [ ] Filter irrelevant temporary/access events before enqueueing them. Include atomic rename, creation, removal, and identity/state replacement paths.
- [ ] Perform incremental reads of changed agent records and cache immutable identity data qualified by process lifetime. Keep a slower full-directory reconciliation for missed events and liveness changes.
- [ ] Read at most the existing record-size limit plus one byte before allocation/JSON parsing. Apply this to both identity and state files and other monitoring reads with post-allocation checks.
- [ ] Give periodic full scans and Magy history discovery a bounded work slice and cancellation points. Resume from a cursor when necessary instead of blocking a watcher indefinitely.
- [ ] Add controlled periodic pruning of definitively dead agent records using existing registration/instance locks. Recheck liveness under lock; never delete an uncertain live record.
- [ ] Preserve the latest pending UI snapshot on output-channel saturation, and retain prompt shutdown during continuous notifications or failed subprocess queries.

**Exit criteria:** a notification flood cannot grow ingress memory; overflow recovers the latest committed completion; directory size does not turn every active-agent event into an unrestricted rescan.

### P4 — Bound retained control data and exporter multiplication

**Files:** `verij-cli/src/navigation.rs`, `verij-cli/src/navigation_diagnostics.rs`, `verij-plugin/src/main.rs`, `verij-plugin/src/control.rs`, `verij-plugin/src/queries.rs`, and `verij-cli/src/actions.rs`.

- [ ] Rotate `control-results.ndjson` using a shared lock and atomic replacement so concurrent hosts cannot race rotation. Keep diagnostic compatibility with retained generations.
- [ ] Review whether routine passive success records require individual fsync. Preserve durable navigation reservation/acknowledgement semantics while batching optional diagnostic I/O.
- [ ] Clean expired result and dead-exporter focus files under bounded, ownership-aware rules. Do not remove records belonging to a live client or pending operation.
- [ ] Measure SDK query counts and decode cost across increasing session/client counts before changing inventory behavior.
- [ ] Reconcile the existing uncommitted live-topology validation changes with the resource budget. Retain stale-pane rejection while avoiding unrestricted per-pane queries from every exporter on every tick.
- [ ] Preserve one inventory timer chain and one recurring control timer chain. Fast focus confirmation must not multiply background timers.
- [ ] Maintain persistent exporter duplicate prevention. Temporary registration surfaces and legitimate per-client instances are separate lifetimes.
- [ ] Never repeatedly reload a plugin in a session with no attached display. Confirm plugin readiness from actual state rather than command exit alone.

**Exit criteria:** retained diagnostic size is bounded; client/session scaling is measured; correctness fixes do not silently restore the original event amplification or introduce multiplied query chains.

### P5 — Verify performance and semantics together

Run focused regressions as each phase lands, then one combined workload matrix. Do not claim leak freedom from a short CPU benchmark.

| Scenario | Required checks |
|---|---|
| Idle agent, no unread completion | No passive visit CLI traffic; slow API recovery; valid lease |
| Streaming at 20 and 100 owned events/sec | Bounded API rate, queued work, native writes, and RSS |
| Families of 1, 16, and 64 sessions | Bounded traversal/request work; no child completion promoted to root Done |
| Large tool output / long conversation | No recurring bulk-history fetch; bounded adapter retention |
| Permission/question open and resolve | Prompt explicit state changes; no dropped blocking request |
| Completion while already focused | One fresh proof; captured revision acknowledged for this host |
| Completion during navigation / new turn | No acknowledgement of an unobserved later revision |
| Two hosts on the same inner session | Independent acknowledgement; coalesced work per host |
| Sidebar focus / hidden float / stack / fullscreen | Presentation hints do not consume Done; effective-focus rules preserved |
| API failure / reporter timeout / reload | Backoff, Unknown where needed, no duplicate producer or timer chain |
| Notification flood / channel overflow | Fixed ingress bounds; latest state recovered; prompt shutdown |
| Pane/process exit and ID reuse | Old identity removed without affecting replacement instance |
| No attached displays | Bounded inventory work; no failed-reload cache accumulation |
| Eight tabs with `zjstatus`, two hosts | Measured total server CPU/RSS and CLI counts |

**Performance acceptance:**

- Enforce deterministic call/write/queue limits in scheduler tests; do not rely only on machine-dependent CPU assertions.
- Target at least an 80% reduction in passive CLI calls for the real idle-agent workload against P0.
- Target at least a 90% reduction in API calls for the one-member, 20-event/sec synthetic OpenCode workload against the approximately 164 requests/sec baseline.
- Run a four-hour soak after warm-up with alternating idle, working, input, and completion periods. Record per-process RSS, total memory, CPU, API/CLI rates, record/journal counts, and recovery after bursts.
- For the fixed-size synthetic workload, require no sustained positive monitoring-attributable RSS trend in the final two hours. Investigate endpoint growth exceeding 32 MiB per monitored process relative to the matched monitoring-disabled control.
- Confirm resource usage returns toward baseline after bursts and no monitoring queue continues growing once producers stop.
- Retain all workload parameters and measurements; distinguish real conversation-history growth from observer retention.

**Correctness acceptance:** existing navigation, ownership, reducer, adapter, and acknowledgement tests pass on stock Zellij. Newly introduced throttling must have regression coverage for cancellation, generation invalidation, retry coalescing, and lease expiry.

### P6 — Roll out the validated assets

**Files:** `Makefile`, relevant setup commands, and monitoring setup/recovery documentation.

- [ ] Build the native CLI and release WASM; verify installed paths and hashes against the tested artifacts.
- [ ] Install OpenCode assets through the existing setup command and confirm installed versions match. Reopen affected TUIs so old bridge schedulers are disposed.
- [ ] Reload each healthy existing exporter once at its configured URL while a real display is attached. Observe the new epoch before treating rollout as complete.
- [ ] For already backlogged or trapped servers, back up resurrection state and restart through the established recovery flow. Replacing the WASM file on disk does not clear an existing queue.
- [ ] Measure the normal WSL workload after rollout, including multiple hosts and background tabs. Retain a 24-hour follow-up summary.
- [ ] Record installed versions, results, and remaining limitations in the investigation document. Mark phases complete only when their exit criteria have evidence.

## 5. Suggested delivery sequence

1. **Baseline and passive proof scheduling:** P0 and P1. This removes the strongest Zellij event producer first.
2. **OpenCode scheduling and lease separation:** P2.
3. **Bounded watchers and runtime retention:** P3 and the journal/cleanup portion of P4.
4. **Exporter scaling review:** the remaining P4 work, informed by measurements.
5. **Combined soak and deployment:** P5 and P6.

Keep each change independently verifiable. Do not increase WSL memory or navigation deadlines as a substitute for removing amplified work.

## 6. Completion checklist

- [x] Idle monitoring has no recurring whole-host visit proofs.
- [x] Successful acknowledgement does not repeat the same proof sequence.
- [ ] OpenCode query rate is independent of streaming event rate.
- [ ] Unchanged semantic snapshots only renew their lease at the bounded interval.
- [ ] Watcher ingress, record reads, background scans, and retained journals are bounded.
- [ ] Multi-host Done, input, error, lifecycle, and navigation behavior remain correct.
- [ ] Four-hour soak and real WSL follow-up have retained measurements.
- [ ] Installed native/WASM/adapter assets match the verified versions.
