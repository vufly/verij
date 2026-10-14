# Agent Monitoring Resource Investigation

Date: **2026-10-13**

Status: **Investigation, P0 baseline, and P1 passive-proof scheduling completed. P2 scheduler foundation is in progress; P3–P6 remain pending.**

Implementation follow-up: [Agent Monitoring Resource Fix Plan](AGENT_MONITORING_RESOURCE_FIX_PLAN.md).

## Conclusion

The strongest explanation for the reported hangs is monitoring-driven work amplification in stock Zellij's plugin pipeline. Verij's passive visit verification repeatedly opens CLI connections. Their teardown generates plugin state updates, including updates for status-bar plugins in background tabs. The earlier Verij exporter fix reduced one subscriber's work but did not remove these producers or bound other plugins' queues.

A second, independent amplifier exists in the OpenCode adapter: every relevant event can trigger a full conversation-family reconciliation, including fetching message bodies. The one-second fallback also runs while idle. The producer queue is bounded, but the API work before that queue is not rate-limited.

Neither this investigation nor retained kernel logs proves that the latest WSL restart was an OOM kill. The previously recorded multi-gigabyte Zellij incident and the newly reproduced CPU/backlog growth establish a credible resource-exhaustion mechanism. Short experiments do not establish a permanent memory leak.

## 1. Incident and current runtime evidence

The [2026-10-12 investigation](INCEPTION_SWITCH_TIMEOUT_INVESTIGATION.md) recorded:

- Zellij session `ss`: approximately **9.5 GB RSS**, **more than 215% CPU**, and saturated `plugin-exec-*` threads.
- Another session `o24`: approximately **5.5 GiB RSS** before restart, falling to approximately **105 MiB** afterward.
- Read-only CLI polling reproduced elevated plugin work.
- A failed plugin reload with no attached display could leave an unbounded pending-event cache in stock Zellij 0.45.1.
- Replacing Verij's pane/tab/session event subscriptions reduced measured CPU, but aggressive tests still saturated `zjstatus`.

On this investigation's initial inspection, WSL had approximately **15 GiB RAM**, **4 GiB swap**, **2.3 GiB used RAM**, and no swap usage. The problematic multi-gigabyte servers were no longer running.

A later five-second `/proc` sample measured:

| Process | RSS | CPU during sample |
|---|---:|---:|
| Current Zellij server | 107.44 MiB | 10.98% |
| Active OpenCode TUI | 1154.80 MiB | 86.45% |
| OpenCode's native Verij reporter | 5.03 MiB | 0% |

OpenCode was executing this investigation. Its CPU/RSS includes normal agent, rendering, and tool work; these figures do not isolate adapter overhead. Several `HeapHelper` threads were active, consistent with allocation/GC work, but this is not proof that monitoring caused those allocations.

Kernel journal searches for the current and three preceding boots returned no retained OOM, killed-process, hung-task, or segfault entries. The system journal records multiple recent boots; it does not identify why WSL stopped.

The installed exporter is already the bounded-polling implementation: startup logs explicitly say `bounded inventory polling (1s)`. Its SHA-256 was `196e133595a0c87e7ec3c6ea98748dace5d1f06bbe58675e00b3777adb0fde34`. It differs from the workspace build artifact, so experiments used the installed binary explicitly. Installed OpenCode `bridge.mjs` and `core.mjs` match repository assets exactly.

## 2. Primary amplifier: passive Zellij CLI polling

### Call chain

1. `verij-cli/src/tui/mod.rs`: requests a focus-monitor job every second whenever agent views exist. This runs even if the Done-candidate list is empty.
2. `verij-cli/src/tui/visits.rs`: a positive cached Workspace-focus hint calls `navigation::verified_visit`.
3. `verij-cli/src/navigation.rs`: `verified_visit` brackets an inner observation with two fresh outer observations.
4. Each outer observation performs `action list-clients` and a CLI `pipe` request. The inner observation performs another CLI `pipe` request.

A successful passive visit check therefore uses **five CLI calls**: two `list-clients` and three pipes. A Done acknowledgement invokes another `verified_visit` through `agent_cli::acknowledge_visit_if_current`, repeating that proof. The filesystem topology watcher additionally runs `list-sessions` every two seconds per sidebar.

This is a per-host cost. Multiple hosts observing the same session multiply it. Commands routed to different servers distribute the load; the five-call total is not five calls to each individual server.

The previously inspected stock source shows that teardown of transient CLI clients publishes pane/tab/session state even when those clients never attached to the display. Other plugins still subscribe to that stream. The user's default tab template loads one `zjstatus` per tab, and the restored `ss` layout contains four such status bars.

### New controlled reproduction

Extended `prototypes/resource-usage/verify.py` to support multiple tabs, an optional local status-bar WASM, peak RSS measurement, and a 512 MiB experiment abort threshold. Each experiment used a private socket directory, XDG roots, a tmux terminal, and no model execution. Pane create/close convergence and actual display detachment were checked. All private servers exited afterward.

Experiments used stock **Zellij 0.45.1**, the installed Verij exporter, four terminal tabs, and the locally cached `zjstatus` binary. The status-bar configuration was minimal; no git-command polling was enabled. Status-bar SHA-256: `282ceab219e56e1908c9fac33907241fe6c3e1ef7d85faab3e5f438cef87b8fe`.

| Configuration | Idle attached CPU | Read-only queries/sec | Loaded CPU | RSS change under load |
|---|---:|---:|---:|---:|
| Verij, four tabs, no status bars | 0.5% | 5.0 | 5.0% | +0.18 MiB / 10 seconds |
| Verij, four `zjstatus` bars | 2.2% | 5.0 | 35.27% | -0.38 MiB / 15 seconds |
| Verij, four `zjstatus` bars | 2.2% | 39.8 | **272.39%** | **+23.16 MiB / 20 seconds** |

At the higher rate, RSS rose from approximately 50.6 MiB to 73.8 MiB. During the subsequent detached/no-query phase, CPU averaged 28.85% while the backlog drained, and RSS returned to approximately 50.2 MiB. This is direct evidence of backlog/allocation growth and recovery, not proof of retained heap leakage.

The load generator uses `list-panes`, not the exact complete visit protocol. It reproduces the common CLI-connection teardown trigger and measures sensitivity to its rate. The 39.8-query/sec case is an elevated stress workload, not a measured live monitoring rate.

## 3. Independent amplifier: OpenCode reconciliation

`integrations/opencode/bridge.mjs` subscribes to message, session, permission, and question events. A relevant event schedules `collect` through `queueMicrotask`. The `collecting` guard prevents concurrent reconciliations, but there is no minimum refresh interval: if the previous reconciliation has finished, the next event starts another.

`integrations/opencode/core.mjs::reconcile` performs:

- Two session-status queries.
- One selected-session query, plus ancestor queries if needed.
- Two children queries per family member.
- One query for up to 100 messages, including parts/bodies.
- Full permission and question list queries.

For a selected root with no ancestors, this is **6 + 2 × family size** API calls per full reconciliation. Projecting small monitoring metadata happens after these responses are obtained; bounding the native report does not bound the amount of data loaded from the API. A message count limit is not a byte limit for tool outputs.

`prototypes/resource-usage/opencode_probe.mjs` exercises the unchanged bridge against a counted synthetic API. It uses no real conversation and an immediately exiting reporter stub, so it measures scheduling and API fan-out rather than real provider or store performance.

| Synthetic observation | Measured API work |
|---|---:|
| One family member | 8 calls/reconciliation |
| 16 family members | 38 calls/reconciliation |
| 64 family members | 134 calls/reconciliation |
| One-member family, no events | 7.93 requests/sec |
| One-member family, 20 owned events/sec | **163.94 requests/sec** |

Each three-second burst processed 59 synthetic events and caused 496 API calls. Real response latency can reduce this rate because the bridge permits only one reconciliation at a time. It does not introduce an explicit resource budget.

Even an unchanged snapshot can cause another native write after the previous queue item has been drained: `SnapshotQueue` deduplicates against the last queued item, not the last successfully delivered observation. The native reporter updates the record revision and atomically writes/fsyncs state for every accepted frame. A lease renewal may be needed, but it should have a separate, deliberately slower budget.

## 4. Additional growth and scaling paths

These findings come from source inspection; they were not reproduced as the cause of the latest crash.

- **Unbounded filesystem notification queues:** both `fs_watcher.rs` and `agent_watcher.rs` use `std::sync::mpsc::channel` in notification callbacks. Their bounded Tokio output channels do not bound incoming filesystem events. A slow full-directory read, subprocess query, or Magy reconciliation can allow incoming notifications to accumulate.
- **All-record rescans:** agent changes trigger full agent-directory reads rather than updates to changed instances. Size checks in `agent_watcher::read_records` happen after `std::fs::read` allocates the entire file. Dead records are ignored but remain until the explicit prune command runs.
- **Unrotated control journal:** successful passive RPCs append to `control-results.ndjson` and fsync on every reply. No rotation was found. At three replies per second, one host can append roughly 259,200 results per day while that polling path remains active. This is disk growth/I/O, not a demonstrated in-memory leak.
- **Exporter/client multiplicity:** each exporter queries the complete session list every second, including inventories that will be discarded when selecting the current session. More sessions, plugin instances, and clients increase repeated decode/allocation cost. Existing uncommitted topology-validation changes also introduce a live query per cached tab and terminal pane; they are not part of the installed binary tested here.
- **WSL metadata-error spam:** the current Zellij log contained approximately 24,090 `Failed to read created stamp of resurrection file` errors in 6.46 MB of log data. The platform reports unsupported creation time. This is noisy background work, but its measured volume alone does not explain multi-gigabyte RAM use.

## 5. Why the previous fix was incomplete

Commit `27c0295` removed Verij's expensive state-event subscriptions, bounded inventory polling, and prevented duplicate startup exporters. This mitigates Verij's own callback cost. It does not remove the sidebar's recurring CLI queries, other plugins' state subscriptions, pending reload caches, or OpenCode's full-state API polling.

The prior benchmark used one tab and short measurement windows. It established a useful CPU reduction, not multi-hour stability with several status bars, multiple hosts, long conversation histories, and agent activity.

## 6. Recommended implementation order

1. **Reduce passive CLI producers first.** Use existing focus heartbeats as invalidation hints; request expensive fresh whole-host proof when a completion actually needs acknowledgement or during explicit activation. Budget routine active-row observation separately. Reuse a verified proof within the same bounded acknowledgement operation instead of immediately repeating the entire visit protocol. Preserve client/process identity and Done correctness.
2. **Rate-limit OpenCode recovery work.** Coalesce owned events into a dirty flag with a bounded refresh rate. Keep direct permission/completion/error edges responsive. Cache stable family ancestry, slow idle recovery, and avoid repeated full message-body queries when existing structured metadata establishes the current outcome. Renew the lease independently from unchanged semantic snapshots.
3. **Bound watcher ingress and retained files.** Use nonblocking bounded/coalesced notification signals with periodic recovery, incremental instance reads, bounded file reads before allocation, controlled dead-record pruning, and journal rotation.
4. **Validate the actual deployment workload.** Test multiple tabs/status bars, multiple hosts, attached and detached sessions, long-running work, idle agents, and reporter failure. Capture CPU, peak RSS, request/event rates, and a multi-hour memory slope. Verify navigation and acknowledgement behavior alongside resource budgets.

Increasing WSL RAM or increasing navigation timeouts would not remove these amplifiers.

## Reproduction

```bash
python prototypes/resource-usage/verify.py \
  --plugin /home/vudinhn/.local/share/verij/verij_plugin.wasm \
  --status-plugin /home/vudinhn/.cache/zellij/11443609859275959894305566542538288992 \
  --tabs 4 --pollers 1 --query-interval 0.2 --seconds 15 \
  --output /tmp/opencode/verij-four-tabs-status.json

node prototypes/resource-usage/opencode_probe.mjs
```

Raw Zellij result files from this investigation are `/tmp/opencode/verij-four-tabs-no-status.json`, `/tmp/opencode/verij-four-tabs-status.json`, and `/tmp/opencode/verij-four-tabs-status-high-rate.json`. The five-second live process/log summary is `/tmp/opencode/verij-runtime-snapshot.json`. Temporary files may disappear after reboot; the principal measurements are retained above.

Verification: the three successful Zellij experiments passed topology/detachment checks and server cleanup; the synthetic OpenCode probe completed; an intentionally low RSS threshold aborted an experiment and still cleaned up its server; `git diff --check` passed. Production code and installed configuration were not changed by this investigation.

## P0 baseline follow-up (2026-10-14)

P0 now measures real Verij sidebars, production visit proofs, native OpenCode record
writes, and both hosts' independent completion acknowledgements. Reports are retained
in [`docs/resource-baselines/`](resource-baselines/README.md), rather than only `/tmp`.
Stock Zellij was **0.45.1**; actual OpenCode runtime was **1.19.0**. Each report records
tested and installed asset paths/hashes, topology, workload parameters, counters,
CPU/RSS, and cleanup. Experiments used the installed bounded-polling exporter, not
the workspace's uncommitted live-topology changes.

### Matched real-sidebar baseline with synthetic API/events

The foreground agent process, bridge, native reporter, sidebar, Zellij servers,
attachments, and visit protocol are real. Only the OpenCode API responses and owned
events are synthetic. Disabled controls retain exporters, status bars, layouts, and
attachments; they replace the sidebar with a sleeping terminal and disable the bridge.
Runs were sequential to avoid contention between benchmark cases.

| Hosts / status-bar tabs | Warm-up / phase duration | Idle total Zellij server CPU, disabled → enabled | Idle monitoring CLI calls/sec | Idle proof attempts/sec |
|---|---|---:|---:|---:|
| 1 / 4 | 5 s / 10 s | 16.30% → 114.38% | 0 → 5.50 | 1.00 |
| 1 / 8 | 5 s / 10 s | 35.90% → 225.75% | 0 → 5.50 | 1.00 |
| 2 / 4 | 5 s / 10 s | 51.59% → 433.04% | 0 → 11.00 | 2.00 |
| 2 / 8, short guarded run | 3 s / 3 s | 131.59% → 840.95% | 0 → 10.99 | 2.00 |

CPU percentages sum the shared inner server and outer host servers; more than 100%
means multiple logical cores. Short-window CPU values are machine-dependent
observations. The CLI counts include the real five-command visit protocol and
periodic `list-sessions` discovery, with operation/target breakdowns in the reports.
These idle agents have no unread completion, yet still generate recurring proofs.

The longer two-host/eight-tab enabled run exceeded the **512 MiB per-Zellij-process
RSS abort threshold** after retaining idle and 20-event/sec measurements. That
guarded abort is retained in `synthetic-2hosts-8tabs.json`; its disabled control
completed. The shorter matched run captured idle, 20/100-event streams, completion,
and actual display detachment without raising the guard. It passed independent
host-local acknowledgement checks. This is baseline evidence of severe amplification,
not a passing long-run stability result.

With one host/four tabs, the synthetic API workload measured:

| Phase | API calls/sec | Committed native record writes/sec |
|---|---:|---:|
| Idle | 8.00 | 1.00 |
| 20 owned events/sec requested, 19.40/sec observed | 163.17 | 20.40 |
| 100 owned events/sec requested, 96.09/sec observed | 776.72 | 97.09 |

The separate reporter-stub scheduling probe retained **165.35 requests/sec** at
20 synthetic events/sec and **789.32 requests/sec** at 100 events/sec. Family costs
remain 8, 38, and 134 endpoint calls for 1, 16, and 64 members. These are synthetic
invocation counts, not real HTTP measurements.

### Actual OpenCode runtime baseline

Additional matched pairs run installed OpenCode against a private local SSE provider,
with private HOME/XDG storage, database, verified server-owned listener, and synthetic
model text. The bridge calls the real OpenCode API and receives actual TUI events.
No remote provider or user conversation is used.

| Hosts / tabs | Warm-up / phase duration | Idle total Zellij server CPU, disabled → enabled | Idle CLI calls/sec, enabled | Idle real API calls/sec | Idle native writes/sec |
|---|---|---:|---:|---:|---:|
| 1 / 4 | 5 s / 10 s | 19.20% → 128.08% | 5.00 | 8.00 | 1.00 |
| 2 / 8 | 3 s / 3 s | 171.26% → 630.71% | 10.99 | 7.99 | 1.00 |

For one host/four tabs, streaming provider chunks at 20 and 100/sec produced **7.30**
and **8.80 real bridge API invocations/sec**, respectively. Provider chunks are not
equivalent to the bridge's subscribed event frequency: actual subscribed owned
events averaged approximately 1.0 and 1.7/sec in those phases. OpenCode buffering,
event types, latency, and generation rejection make this workload materially
different from the synthetic 20-event/sec scheduling probe. Reports retain both
the configured provider rate and observed event counts.

OpenCode's ordinary startup RSS exceeds 512 MiB, so these runtime experiments use
an explicitly recorded **1536 MiB OpenCode-only guard**. Zellij's guard remains
512 MiB. Total CPU/RSS includes ordinary OpenCode rendering, API, and conversation
growth; those totals must not be attributed entirely to monitoring.

### Measurement bounds and verification

`VERIJ_RESOURCE_METRICS_DIR` enables native counters only in sidebar and long-lived
OpenCode reporter processes. CLI counters retain at most 32 operation/target keys,
with overflow counted separately. Native writes count committed atomic monitoring
record replacements; proof attempts count actual `verified_visit` entry. Filesystem
counters distinguish incoming notifications from notifications discarded inside
existing coalescing windows. Counters are observational: scheduling and acknowledgement
semantics are unchanged.

Each process publishes one replaceable one-second snapshot outside watched roots;
native publication serializes its writers without holding the callback counter lock
during filesystem I/O. Phase summaries retain at most 60 intervals. No measurement
probe writes per-event payload logs. Counter boundaries have one-second resolution;
short phases can include an adjacent interval of work. Process totals cover retained
long-lived private processes, while transient CLI work is counted separately.

Successful pairs passed pane create/close convergence, expected tabs/status bars/client
counts, real passive completion acknowledgement for each host, zero attached clients
after host shutdown, and birth-qualified private-process cleanup. The guarded abort
also cleaned up. A deliberately lowered 32 MiB server guard additionally verified
cleanup before all private processes had published exporter/metric records; its
retained report is `rss-guard.json`. The harness shares Verij's startup lock when
starting hosts so background discovery cannot probe a half-initialized stock server.

Verification includes the retained-report checker, native bounded-counter tests,
the complete native CLI test suite (209 unit and 38 integration tests), OpenCode's
eight adapter tests, Python/JavaScript syntax checks, and `git diff --check`.
Installed assets have not been replaced. P1–P6 and their long-run performance
acceptance remain pending.

The existing H1 navigation harness was also exercised. Its installed-assets run
passed every gate, but subsequent instrumented-CLI and unmodified-HEAD control runs
failed at cross-session rebind with no fresh registration surface for destination
clients `{1, 2}`. The HEAD control was built from `dd0302f` in a temporary detached
worktree, without P0 instrumentation, and has the same binary hash as the installed
CLI. Both failures passed the earlier ownership, visit, host-local acknowledgement,
wrong-pane, sidebar-focus, and input gates, and cleaned up every private process.
The pre-existing cross-session registration failure remains
separate from P0 resource measurement. All three H1 reports are retained; P0 does
not claim that the complete navigation harness is consistently passing.

## Global build follow-up (2026-10-14)

After P0 measurements, the release CLI and WASM were built and installed globally
at the user's request. The native CLI at `~/.cargo/bin/verij` matches
`target/release/verij`, SHA-256
`816ffd65e8912fd46ea6875238bd12ebf757fdc333a9c7b0c4ec2d4e254fa187`.
The exporter at `~/.local/share/verij/verij_plugin.wasm` matches the release WASM
and `dist/verij_plugin.wasm`, SHA-256
`d5c3564ffa4004aacfc7187e0224fa29af493242f0bf3615405dde5dd20683e5`.
That exporter includes the pre-existing workspace live-topology validation edits;
those edits are separate from the P0 baseline commit. Retained P0 reports continue
to identify the older exporter actually used for their measurements.

OpenCode assets were refreshed through `verij agent setup opencode`; installed
bridge/core hashes match the repository. The private H1 harness passed every gate
with this globally installed CLI/exporter pair, including cross-session rebind,
plugin reload, rename, ownership, and host-local acknowledgement. All private
processes exited. The result is `/tmp/opencode/verij-global-build-navigation.json`.

## P1 follow-up (2026-10-14)

Passive monitoring now schedules only current, navigable, unread Done candidates in
the requesting host's bound and likely focused terminal. Cached SDK focus heartbeats
update presentation but never establish acknowledgement authority. One running job,
one replaceable pending job, and one replaceable update bound each host's work.
Minimum spacing is two seconds; failures back off to 30 seconds. Meaningful focus,
binding, or completion changes reset failure backoff without erasing minimum spacing.

The native visit helper returns an opaque, non-cloneable operation proof consumed
by acknowledgement. It brackets an inner observation with two fresh outer native
observations, retaining both actual single-display checks. Its two-second validity
starts at the earliest observation, not the final reply. Commit rechecks binding,
journal currency, outer ownership, negative focus invalidations, and terminal/agent
process lifetimes. Passive identity/current-completion checks run under persistence
locks; new turns and replacement identities reject old passive work. Explicit
navigation still acknowledges only its captured revision, not a newer completion.

| Scenario | P0 idle CLI calls/sec | P1 idle CLI calls/sec | P1 idle proof attempts |
|---|---:|---:|---:|
| One host, four status-bar tabs | 5.50 | 0.10 | 0 |
| Two hosts, eight status-bar tabs | 10.99 (short guarded P0 pair) | 0.20 | 0 |
| Actual OpenCode, one host/four tabs | 5.00 | 0.10 | 0 |

Remaining idle calls are `list-sessions` discovery at ten-second intervals. Exporter
filesystem updates remain prompt, including live publications that overturn cached
Exited data. No-exporter fallback discovery can now take up to ten seconds. This is
approximately a 98% reduction in passive CLI traffic.

Retained synthetic pairs check two completion revisions with real SDK/process
observations. Sidebar focus and another inner pane preserve Done; actual inner
keyboard navigation acknowledges through exactly one five-call proof. Hosts
acknowledge independently. Adding a second outer display rejects acknowledgement:
three failures in ten seconds each use only fresh `list-clients`. Removing that
display and changing focus resets backoff and obtains one new successful proof.
All private processes exited; Zellij's 512 MiB RSS guard remained unchanged.

Reports are `resource-baselines/p1-1host-4tabs.json`,
`resource-baselines/p1-2hosts-8tabs.json`, and
`resource-baselines/p1-opencode-1host-4tabs.json`. They include the P2 scheduler
foundation and installed topology-validation exporter; hashes distinguish them
from P0 assets. Total CPU/RSS includes those paths; CLI/proof counts isolate P1.
The larger pair completed every phase within the existing RSS guard.

Regression coverage includes empty/missing/conflicting candidates, bounded job
replacement, cancellation/drop, retry deadlines, binding/journal/focus/process
invalidation, stale/future proofs, superseding completion/new turn, and prompt
filesystem topology despite slower discovery. H1 again passed its earlier
ownership/visit gates and hit the pre-existing cross-session rebind failure;
that result is retained in `resource-baselines/p1-navigation.json`.
