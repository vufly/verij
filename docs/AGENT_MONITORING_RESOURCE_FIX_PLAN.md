# Agent Monitoring Resource Fix Plan

Date: **2026-10-15**

Status: **P0–P2 complete (2026-10-14); P3 implemented and regression-verified (2026-10-15). P4–P6 remain pending, including combined soak and rollout.**

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
| OpenCode current-outcome recovery | One-message metadata pages; at most six history invocations per collection and 100 messages searched for the current user |
| OpenCode adapter parsing | 64 KiB metadata-prefix scan; 1 MiB other response buffer; JSON depth 32 and 16,384 structural nodes |
| Standalone worker metadata | Read-only indexed SQLite pages; 512 KiB cache, no mmap, zero lock-wait timeout; no message-body RPC |
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

**Files:** `integrations/opencode/bridge.mjs`, `integrations/opencode/core.mjs`, `integrations/opencode/core.test.mjs`, `integrations/opencode/recovery.test.mjs`, `verij-cli/src/opencode.rs`, and `verij-cli/tests/opencode_setup.rs`.

- [x] Replace event-triggered full collection with a dirty-state scheduler. Track the next eligible refresh, one in-flight collection, and one pending invalidation.
- [x] Ensure an event arriving during collection invalidates the sampled generation and schedules one later retry. Do not lose the dirty flag on a rejected snapshot.
- [x] Preserve immediate bounded event projection for validated permission/question and activity edges. Reconcile terminal outcomes conservatively when current evidence is incomplete.
- [x] Avoid allocating a microtask or request graph for every streaming update. Repeated activity and part updates should only set an already-pending flag.
- [x] Cache validated root/family ancestry for the current route and producer generation. Invalidate it on relevant create/delete/reparent events, route changes, reconnect, or inconsistent responses.
- [x] Separate cheap current-status recovery from full ancestry/history recovery. Charge every endpoint invocation to the per-bridge request budget.
- [x] Inspect the installed OpenCode API/cache contract for metadata-only or incremental outcome recovery. Use local structured message metadata where verified. Do not repeatedly fetch full tool-output bodies to rediscover unchanged completion metadata.
- [x] If startup recovery requires message retrieval, use the narrowest supported query and explicit allocation limits where the API permits them. A message-count limit alone must not be presented as a byte bound.
- [x] Keep family-size, response-time, and concurrency bounds. When a complete family cannot be verified within the budget, retain conservative state and retry later rather than increasing request rate or manufacturing Done.
- [x] Back off failed API recovery and reporter reconnects independently. Reset backoff on meaningful ownership changes or explicit new work.
- [x] Deduplicate against the last successfully delivered semantic observation, not only the current queue tail. Distinct state edges must remain ordered.
- [x] Renew unchanged source health every five seconds without a full API reconciliation. Initially reuse the existing snapshot protocol; add a dedicated renewal frame only if compatibility and measured write cost justify it.
- [x] On reconnect, publish conservative presence and recover current state before restoring semantic authority. Keep the native source epoch and completion ledger rules.
- [x] Dispose timers, listeners, queued work, and in-flight requests on reload/exit.

**Exit criteria:** streaming frequency no longer determines full-query frequency; idle recovery and lease renewals have independent rates; supported input/completion/error transitions remain correct; native writes for unchanged state are limited to lease renewal.

**Initial P2 milestone (retained foundation evidence):** active/dirty collection starts are separated
by at least one second; quiet idle recovery uses ten seconds; unchanged lease renewal
uses five seconds after a native receipt. A shared endpoint token bucket permits
32 invocations/second with an eight-call burst, four physical requests in flight,
and four pending requests. Cancellation does not prematurely free a physical slot.
Tests cover timer/dirty coalescing, generation rejection, backoff, endpoint accounting,
lease renewal without queries, ordered input edges, reconnect, and disposal.

Retained initial results: `resource-baselines/p2-synthetic-start.json` and
`resource-baselines/p2-opencode-start.json`. One-member synthetic streaming at 20 and
100 owned events/second both measured 8.0 API calls/second. Quiet synthetic idle
measured 0.8 calls/second and 0.2 committed native writes/second. At that milestone,
existing ancestry and bulk-message reconciliation remained behind the scheduler;
family caching, resumable large-family recovery, and metadata-only outcome recovery
were still required. Budget exhaustion yielded conservative state because the
three-second collection deadline could not finish every 64-member full scan at
the request ceiling. The following milestone addresses the topology portion.

**Family recovery milestone (2026-10-14):** `ObservationRecovery` now caches
verified root/family metadata for the route and producer generation, with a
ten-second topology fallback. Discovery, child-set verification, and selected
ancestry rechecks resume across slices of at most 16 topology calls. Each collection
retains the three-second deadline and shared endpoint budget; completed cursor work
survives a timed-out read. Streaming semantic invalidations do not discard structural
progress. Partial or inconsistent families cannot establish Done.

At the family milestone, cached status/input recovery cost four endpoint calls.
Busy families skipped message retrieval; quiescent history recovery had an independent
ten-second fallback and message/activity invalidation. Only bounded structured
metadata was retained after retrieval; SDK response allocation and idle bulk-body
retrieval were still open. The completion milestone below addresses those paths.

Immediate input projection validates bounded IDs and request kind/session ownership.
Topology changes retain input for unaffected, previously verified members; new
descendants await verification. Old membership hints cannot establish family
quiescence or completion. Root reparent, route replacement, and reporter/server
reconnect retire authority. Child retry projects Working rather than root Retrying.
Verified ancestry enables subsequent event projection even when continuous streaming
supersedes every semantic recovery sample.

**Evidence:** `resource-baselines/p2-family-scheduler.json` verifies families of
1/16/64 members in 1/3/9 slices, with four-call cached recovery. The matched real
sidebar/native-reporter synthetic run in `p2-family-synthetic.json` measured
4.5/4.8 API calls/sec at requested 20/100 owned events/sec and 0.3/0.2 native
writes/sec. The actual OpenCode 1.19.0 pair in `p2-family-opencode.json` passed
completion acknowledgement, detachment, and cleanup. Its provider chunk rates
are distinct from observed subscribed event rates. `p2-family-h3.json` passed
26 runtime checks with 27 corroborated snapshots across four instances. All
38 Node tests and the four focused native OpenCode/setup tests passed.

Those short, one-host/four-tab measurements established the family milestone,
before metadata-only outcome recovery and pre-parse bounds were complete.

**P2 completion milestone (2026-10-14):** outcome recovery now reads one-message
metadata pages, independently of topology and streaming frequency. It finds the
newest root user and corresponding newest assistant, verifies strict page ordering,
and rechecks the head before returning semantic authority. An unchanged head cannot
skip predecessor verification: a backdated inserted user or deleted predecessor
must not authorize an old Done. Long current-turn suffixes resume with one cursor,
at most six metadata invocations per collection, and a 100-message search bound.
Only current-user/latest-assistant metadata is retained. Local TUI metadata can
label Working; empty, stale, or unhydrated caches never establish Idle or Done.

Attach uses the existing SDK transport/authentication, extracting the stock `info`
prefix and cancelling before parts. The prefix scan is capped at 64 KiB, including
envelope whitespace. Other SDK responses are capped at 1 MiB before adapter copying,
decoding, or SDK JSON parsing, with depth/node guards. Error bodies are cancelled
without parsing. These are adapter allocation limits: transport-owned read-ahead
chunks and server-side response construction are explicitly outside that bound.

Stock standalone worker RPC buffers whole bodies upstream of the SDK. Its message
path instead uses fresh, read-only SQLite metadata from the producer's verified
storage scope. It checks path/UID/device/inode, schema and stock indexes; uses a
512 KiB cache, no mmap, and zero busy wait; and rejects oversized stored metadata
with `octet_length` before JSON extraction. It never selects parts or invokes
message-body RPC. These local observation invocations are charged to the same
budget. Missing/in-memory/unverified storage or unsupported schemas remain
conservative, with bounded retries. Handles close on disposal; replacement and
schema changes invalidate them. Other worker SDK endpoints retain their upstream
transport behavior and the adapter's pre-parse guards.

Recovery eligibility now starts at collection start, so API latency cannot silently
turn a ten-second fallback into twenty seconds. New tests also prevent stale local
Working metadata from overwriting an explicitly observed newer root turn.

**Completion evidence:** `resource-baselines/p2-metadata-large-output.json` passes
seven checks against OpenCode 1.19.0 with 128 historical messages and an 8 MiB
synthetic tool part inserted through the actual REST API. Six attach recovery queries
delivered 312,094 transport bytes (including read-ahead), while standalone recovery
delivered **zero message-body RPC bytes**. Plugin reload changed source epoch and
preserved completion revision; the controlled provider was invoked once. Existing
512 MiB Zellij / 1536 MiB OpenCode guards and private-process cleanup passed.
`p2-metadata-h3.json` passed 26 live checks with 27 corroborated snapshots across
four instances; `p2-metadata-opencode.json` passed the matched runtime workload,
acknowledgement, and cleanup. `p2-metadata-scheduler.json` retains family and stream
budgets. All 55 Node tests and four focused native OpenCode/setup tests passed.

P2's phase-specific exit criteria now have evidence. P5's combined multi-host matrix,
four-hour soak, and P6 deployment/follow-up remain pending; these short runs do not
establish leak freedom or a byte bound on stock OpenCode's own caches/serialization.

### P3 — Bound watcher ingress and directory work

**Files:** `verij-cli/src/fs_watcher.rs`, `verij-cli/src/agent_watcher.rs`, `verij-cli/src/inventory.rs`, `verij-cli/src/magy.rs`, `verij-cli/src/process.rs`, `verij-cli/src/resource_metrics.rs`, and `verij-cli/src/agent_store.rs`.

- [x] Replace unbounded notification channels with nonblocking bounded signals. A one-slot dirty signal is sufficient where the worker already performs a complete reconciliation.
- [x] For incremental reads, keep a bounded set of dirty instance IDs. Overflow must set a full-rescan flag, not grow the set or block the callback.
- [x] Filter irrelevant temporary/access events before enqueueing them. Include atomic rename, creation, removal, and identity/state replacement paths.
- [x] Perform incremental reads of changed agent records and cache immutable identity data qualified by process lifetime. Keep a slower full-directory reconciliation for missed events and liveness changes.
- [x] Read at most the existing record-size limit plus one byte before allocation/JSON parsing. Apply this to both identity and state files and other monitoring reads with post-allocation checks.
- [x] Give periodic full scans and Magy history discovery a bounded work slice and cancellation points. Resume from a cursor when necessary instead of blocking a watcher indefinitely.
- [x] Add controlled periodic pruning of definitively dead agent records using existing registration/instance locks. Recheck liveness under lock; never delete an uncertain live record.
- [x] Preserve the latest pending UI snapshot on output-channel saturation, and retain prompt shutdown during continuous notifications or failed subprocess queries.

**P3 completion milestone (2026-10-15):**
- **Nonblocking measurement hooks:** Filesystem ingress/coalescing counters use four fixed atomic counters, so opt-in measurement cannot block notification callbacks behind metrics serialization.
- **Nonblocking bounded ingress:** Replaced notify callbacks' unbounded channels and mutex locking with nonblocking primitives: `FsNotificationSignal` uses `sync_channel(1)` try-send with zero callback locks; `AgentNotificationSignal` uses `try_lock()` on a 128-capacity dirty ID set with atomic `full_rescan` flag and `sync_channel(1)` wakeups. Contention or overflow conservatively sets `full_rescan` without blocking callback threads. Enqueues both source and destination IDs for rename events. Filters `EventKind::Access`, temporary files (`.*.tmp.*`), and unrelated extensions pre-enqueue. Debounce sleeps in small bounded intervals with prompt stop checks instead of spinning on undrained dirty signals.
- **Pre-allocation bounded reads & non-blocking I/O:** `read_bounded_bytes` and `read_bounded_bytes_with_meta` open files with `libc::O_NONBLOCK | libc::O_CLOEXEC` on Unix and verify `metadata.is_file()` before reading at most `limit + 1` bytes via `.take(limit + 1).read_to_end(&mut buf)`. Special files (FIFOs / named pipes) and directories fail immediately without hanging or blocking. File stamps are derived from the open file handle (`fstat`) to prevent metadata-read races. Applied across `agent_store` (`IDENTITY_MAX_BYTES` = 64 KiB, `STATE_MAX_BYTES` = 256 KiB, `HOST_ACK_MAX_BYTES` = 256 KiB), `inventory::read_states` (4 MiB + 1), `magy::read_state` / `Cursor::tail` (1 MiB tail), and `process::verify_agy_foreground` (64 KiB + 1 cmdline read).
- **Qualified identity cache & slice-bounded watcher liveness:** Qualified by `FileStamp` (`dev`, `ino`, `mtime`, `ctime`, `len`) and `ProcessIdentity`. Unchanged stamps reuse parsed identity in memory; atomic replacements or file modifications invalidate cache immediately. Process liveness is cached cheaply for 1 second. Expired OpenCode leases are projected in-memory across all statuses (`Working`, `Done`, `NeedsInput` -> `Unknown`) without re-reading state files from disk. Quiet lease and liveness checks in `agent_watcher` slice across at most 32 cached records per iteration, checking cancellation at each record. Full scan sweep EOF cleans orphan `identity_cache` entries alongside `current_records`, avoiding memory leaks across deletions.
- **Resumable directory work & nonblocking background registration:** `prune_slice_in_root` and `fs_watcher::read_states_slice_cancelable` maintain persistent `Option<fs::ReadDir>` cursors across slices (16/32 entries), counting all visited entries, checking `stop` between entries, and yielding. Background agent registration uses owned, resumable `RegistrationCursor`: try-acquires `RegistrationLock`, scans <=32 entries per slice, validates `.tombstone` rejection, schema version, and directory key match, and detects concurrent directory modifications via root directory `FileStamp` to restart conservatively before atomic EOF allocation. Slices interleave with dirty ID updates.
- **Owned Magy Reconciler, bounded active cache & double revalidation:** Replaced static binding cache with owned `Reconciler` held by `agent_watcher`. Operates in bounded slices: discovers <=32 watch entries per slice; rejects dead runner history prior to inventory; holds at most ONE pending operation (`SearchingInventory` scanning 16 entries/slice via `inventory::read_states_slice`, deduplicating identical owner snapshots and capping candidate owners at 2 lifetime pairs to prevent memory growth or false ambiguity; transitions to `RegistrationCursor` scanning 32 entries/slice). Double revalidation: process lifetimes and `runner_binding` are re-verified immediately before `cursor.step` and again immediately before `update_instance`. On every pending step, nonblocking `read_state` verifies unchanged watch ID, pane ID, runner PID, birth, and log binding; any mismatch retires pending. Refreshes stored state on each step so a status change from completed to cancelled never authorizes Done. Top-level cancellation check halts immediately without extra owner/process checks. CLI `reconcile()` enforces a 100-step budget and 5-second deadline with 10ms backoff sleep on contention.
- **Active cache latency & starvation boundaries:** Active watch cache is bounded to <=128 entries. Cache hits bypass inventory search and registration locks completely, verifying `same_runtime`, `same_runner`, `same_pane_id`, and `verify_owner(&ident, &server, &pane)`. Unchanged observations retain cache binding without eviction. Live cached active watches are evaluated as the watch directory is scanned (<=32 entries per slice). When a pending watch operation is active, pending processing takes priority; once the pending watch finishes or retires, watch directory scanning resumes. Revisit latency for active cached entries is bounded by `ceil(N_watches / 32)` slices plus slices spent resolving pending operations.
- **Bounded subprocess shutdown & pipe handling:** `run_command_bounded` creates isolated child process groups (`cmd.process_group(0)`) and polls pipes nonblockingly with a monotonic deadline. Any pipe retention past deadline by child or background descendants is treated as failure and triggers full process group termination (`kill(-pid, SIGKILL)`). Reading beyond `max_bytes + 1` sets an explicit overflow flag, immediately killing the child process group without returning truncated output prefixes. Closed pipe file descriptors set `fd = -1` in `libc::pollfd` to eliminate busy polling on EOF.
- **UI snapshot saturation resilience:** When the output channel is full (`TrySendError::Full`), `pending_send` stores the newest snapshot, replacing older pending snapshots. Saturated channel recovery verifies that revision 2 and `AgentStatus::Done` are delivered once the channel drains.

**Completion evidence:**
- Deterministic regression test suites pass serially (`--test-threads=1`):
  - `agent_watcher::tests` (13/13 pass): `test_dirty_ingress_bounds_and_overflow_coalescing` (200 IDs bounded to 128 + rescan flag), `test_ingest_event_filters_access_and_temp_and_handles_renames` (access/temp filtered, source/dest rename preserved, need_rescan handled), `test_lease_expiration_projects_done_and_needs_input` (lease expiry projects Done and NeedsInput to Unknown), `test_incremental_reads_and_immutable_identity_caching` (state update reuses identity without re-reading identity.json), `test_id_reuse_and_atomic_replacement_invalidates_cache` (atomic replacement detected via FileStamp, invalidates cache), `test_full_scan_eof_removes_missed_deletion` (missed deletions pruned only on full scan EOF), `test_full_scan_clears_orphan_identity_cache_entries` (orphan identity cache entries cleared on full scan), `test_overflow_recovers_latest_done_end_to_end` (flood >128 IDs during saturated output triggers full rescan with fixed bound 128, recovering revision 2 Done status), `test_saturated_channel_delivers_latest_snapshot_with_done`, `channel_full_eventually_sends_latest_when_quiet`, prompt shutdown on continuous notifications, quiet stream shutdown, full channel watchdog termination.
  - `agent_store::tests` (14/14 pass): `test_registration_cursor_validations` (validates .tombstone rejection, schema version mismatch, and instance ID directory mismatch before returning existing ID or allocating at EOF), `test_read_bounded_bytes_rejects_oversized_and_fifo` (pre-parse size limit and nonblocking FIFO rejection), `test_prune_skips_contended_instance_lock` (contended lock skipped safely without blocking), `test_prune_preserves_schema_mismatch` (unsupported schema preserved), `test_prune_safe_removal_preserves_lock_and_tombstone_on_unexpected_files` (state files cleaned, unexpected file + lock + tombstone preserved to prevent lock splitting), `test_prune_slice_resumable_and_preserves_live` (cursor slicing across iterations, dead pruned, live preserved), registration idempotence, immutable conflict rejection, atomic update bounds, and host-local ack proof racing.
  - `fs_watcher::tests` (15/15 pass): `test_subprocess_shutdown_bounded_with_pipe_retention` (background sleeper descendant holding open pipe terminates within bounded deadline without blocking), `test_subprocess_oversized_response_rejected` (reading past buffer limit rejects oversized command without returning truncated prefix and kills process group), `test_subprocess_stop_while_failed_command` (prompt shutdown on stop flag while command fails), `test_subprocess_timeout_terminates_process_group` (killpg terminates child group), quiet stream shutdown, continuous notification shutdown, channel full coalescing, and live filesystem publication.
  - `magy::tests` (7/7 pass): `test_candidate_owners_dedup_and_capped_at_two` (deduplicates duplicate owner snapshots, caps candidate owners at 2 lifetime pairs, never allocates beyond 2), `test_pane_replacement_invalidates_pending` (pane replacement invalidates pending search/registration immediately), `test_pending_completed_to_cancelled_cannot_authorize_done` (status changed during pending to cancelled never authorizes Done or successful_completion), `test_reconciler_dead_history_rejected_before_inventory` (dead runner history rejected before inventory scan, cancellation halts immediately), `test_fifo_rejection_nonblocking` (FIFO state and log files rejected nonblockingly), `saved_offset_partial_lines_truncation_rotation_and_large_frames_are_bounded`, and `tool_error_is_not_run_failure_and_stream_success_waits_for_watch_finalization`.
  - `inventory::tests` (3/3 pass): `test_read_states_slice_with_cursor_limit` (cursor slicing with limit, counts all entries, EOF resets cursor), bounded reads, and identity hydration.
  - `process::tests` (5/5 pass): terminal foreground ownership, stat parsing, socket peer verification.
- Suite results:
  - Final independent verification: `cargo test -p verij-cli -- --test-threads=1` passed all **237 unit tests and 38 integration tests**, including `stale_workspace_attachment_recovers_only_exact_foreground_connected_client`.
  - P3 added 25 unit regressions relative to the 212-test baseline. `cargo test -p verij-types -p verij-plugin` passed all 34 tests; the OpenCode core/recovery Node suites passed all 55 tests.
  - The [P3 private-runtime smoke summary](resource-baselines/p3-smoke.md) retains a short stock-Zellij run with one host, four tabs, real sidebar/native reporter, synthetic streams, completion acknowledgement, detachment, and successful private-process cleanup. This is a smoke check, not a matched performance comparison or soak.
  - Directory and cache contents still scale with monitored records; ingress and individual background directory slices have fixed bounds. Magy discovery/revisit latency grows with history size and pending ownership work. P5 must measure the combined workload and long-run behavior.
- P3 exit criteria verified: notification floods cannot grow memory beyond 128 dirty IDs; overflow recovers full state including latest completions; directory scans slice work incrementally; subprocess shutdowns are strictly bounded; background Magy reconciliation operates under owned, bounded context with double-revalidated ownership. P4 (journal rotation/retained control data) and P5 (combined soak matrix) remain pending.

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
- [x] OpenCode query rate is independent of streaming event rate.
- [x] Unchanged semantic snapshots only renew their lease at the bounded interval.
- [ ] Watcher ingress, record reads, background scans, and retained journals are bounded.
- [ ] Multi-host Done, input, error, lifecycle, and navigation behavior remain correct.
- [ ] Four-hour soak and real WSL follow-up have retained measurements.
- [ ] Installed native/WASM/adapter assets match the verified versions.
