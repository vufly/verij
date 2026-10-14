# Pi monitoring rebase plan

Status: executed and verified locally. Rebase, deterministic gates, two-distribution Pi acceptance, P3 smoke and matched short resource controls passed. Publication, combined soak, journal-retention completion and global rollout remain out of scope. See the execution record below; original comparison and planned gates are retained for review.

## Compared revisions (pre-execution snapshot)

- Branch: `pi-monitoring`, `276d194` (`origin/pi-monitoring` matches).
- Target: `master` and freshly fetched `origin/master`, both `d1b58fb`.
- Common base: `4fa4b97`, “Bounded passive visit proofs and OpenCode refreshes”.
- Working tree was clean before this plan was added.
- Branch has five unique commits; master has four unique commits.

The branch already contains the common base's passive visit-proof and initial OpenCode refresh improvements. Those are not new changes to import during this rebase.

## Changes to retain from master

| Commit | Change | Rebase implication |
|---|---|---|
| `9822bcd` | Bounded OpenCode recovery, metadata-only outcome recovery, and live exporter topology refresh | Keep the updated OpenCode bridge, tests, and exporter behavior. Rebuild both CLI and WASM for Pi runtime verification. |
| `2a639fe` | Bounded watcher ingress, incremental agent reads, qualified identity caching, sliced scans/pruning/Magy work, bounded file reads, and subprocess cleanup | Treat this infrastructure as authoritative. Adapt Pi lease handling to the new cached watcher instead of restoring the old full-directory reader. |
| `03725d0` | Preserve sidebar sessions across transient inventory refresh failures | Keep failure preservation and its regressions; do not replace failed refreshes with empty session inventories. |
| `d1b58fb` | Release-family compatibility for existing agent integrations | Retain shared `agent_version` support and existing integrations' changes. Pi's separately qualified version contract stays unchanged in this rebase. |

Master's resource plan reports P0–P2 complete and P3 regression-verified. Journal retention, the combined soak, and rollout remain pending. Importing these commits does not establish complete performance qualification.

## Conflict and semantic integration

A `git merge-tree --write-tree --name-only master HEAD` preview reports one conflicting file: `verij-cli/src/agent_watcher.rs`. A preview through the native Pi commit, `865275b`, reports the same file. This is a merge preview, not proof that every sequential replay will have no other conflict.

Only three paths are changed on both sides since the common base:

- `verij-cli/src/agent_watcher.rs`: conflict in the old read loop and appended tests.
- `verij-cli/src/agent_store.rs`: automatic merge in the preview; preserve Pi's reserved-source rejection alongside master's updated locking and bounded I/O.
- `verij-cli/src/main.rs`: automatic merge in the preview; retain `agent_version`, `pi`, `pi_setup`, the recursion limit, and Pi's resource-metrics dispatch.

### Required watcher adaptation

The branch expires Pi leases inside `read_records`. Master moves record loading into `read_single_agent` and retains records in memory, with a separate periodic lease/liveness sweep. Merely transplanting the old Pi check into the incremental reader would miss timely expiry when the reporter stops writing.

1. Keep master's `read_single_agent`, identity cache, bounded reads, dirty-ID ingress, scan cursors, pruning, and latest-pending-snapshot behavior.
2. Apply Pi lease degradation after reading an individual record and during the existing periodic in-memory lease sweep.
3. Prefer one idempotent Pi degradation helper shared by these paths. Its change result should prevent repeated identical UI publications after expiry.
4. On expired Pi health, set `Unknown`, clear pending requests, background task count, latest completion, and latest error, and retain the Pi-specific unavailable detail.
5. Preserve identity, source records, adapter snapshots, completion deduplication, and completion revision. Expiry is a read-side projection, not a fabricated source transaction or disk write.
6. Use the existing sweep time and 32-record slice/cancellation boundaries. Do not add another timer, subprocess, state-file polling loop, or unrestricted scan.
7. Retain both master watcher regressions and the branch's expired-Pi test. Resolve appended-test conflicts manually rather than choosing a whole file with `ours` or `theirs`.
8. Preserve OpenCode behavior while adding Pi handling; avoid an unrelated rewrite of existing lease semantics.

### Automatically merged paths still need review

- `agent_store::report` must continue rejecting generic `source_id == "pi"` reports.
- Pi's native `update` transactions must retain fencing, full-snapshot replacement, reconnect restoration, and completion deduplication under master's registration/pruning locks and tombstone checks.
- Doctor and foreground ownership must continue working against updated inventory, process checks, and exporter topology.
- Pi setup's custom file reader has separate symlink/preservation requirements. Do not mechanically replace it with the shared reader or discard those requirements during cleanup. Broader reader hardening is separate from this rebase.

### Version policy

Keep Pi restricted to qualified `1.2.0` in native validation, JavaScript validation, setup, doctor, and documentation. Master broadens other agents' release families, but no newer-Pi runtime qualification is present here. Reusing `agent_version::supports_series` for Pi would be a behavioral expansion requiring separate approval, consistent guards/tests, and runtime evidence.

## Execution order

Preserve the existing five-commit dependency order:

1. `805a6dc` — lifecycle qualification and P0 evidence.
2. `865275b` — native types/reporter; resolve the watcher conflict here and include the cache-aware Pi lease adaptation.
3. `c7a5abb` — extension projection and transport.
4. `d251213` — owned setup, doctor, and uninstall.
5. `276d194` — local acceptance harness and metrics dispatch.

Before starting, commit or otherwise safely preserve this plan so the working tree is clean. Refresh the target and repeat comparison if `origin/master` moved. The commands below assume it remains `d1b58fb`:

```bash
git status --short
git branch backup/pi-monitoring-pre-rebase-276d194 276d194
git rebase --onto d1b58fb 4fa4b97 pi-monitoring
```

At the watcher conflict, manually integrate the changes above, stage the resolved file, and continue. Additional lease regressions can be a dedicated follow-up commit if they do not belong in the original native commit. Preserve old retained runtime evidence; do not relabel it as a result from the rebased binaries.

## Verification gates

### New integration regressions

- Incremental reads degrade an expired Pi lease while retaining its live process row and adapter ledger.
- The periodic cached sweep expires Pi health without any new filesystem event; cover Working, Done, NeedsInput, and stale fields on an already-Unknown row.
- Expiry clears background work and terminal/input presentation but does not change completion counters or write state to disk.
- A fresh heartbeat/reconnect restores current state without a new historical Done revision.
- More than 32 cached records do not starve Pi lease expiry across slices.
- Overflow and a saturated UI channel still deliver the newest Pi completion/state; shutdown remains prompt.
- Existing OpenCode expiry, identity replacement, pruning, and inventory-failure preservation tests remain green.

### Deterministic suites

```bash
cargo test --workspace -- --test-threads=1
make check
node --test integrations/pi/core.test.mjs
node --test integrations/opencode/*.test.mjs
node --test prototypes/pi-runtime/trace.test.mjs
python3 -m unittest discover -s prototypes/pi-runtime -p 'test_*.py'
python3 -m unittest discover -s prototypes/pi-acceptance -p 'test_*.py'
python3 -m unittest discover -s integrations/agy -p 'test_*.py'
python3 -m unittest discover -s integrations/codex -p 'test_*.py'
git diff --check
```

Use focused watcher/store/fs_watcher/Magy/inventory/process and Pi reporter/setup tests while resolving conflicts, then the full gate. The branch documents pre-existing workspace formatting differences; do not mix unrelated formatting into the rebase.

### Runtime and performance

1. Build the rebased release CLI and WASM exporter together.
2. Repeat `prototypes/pi-acceptance/README.md`'s private, two-distribution Pi acceptance matrix. Specifically verify reporter outage/quiet lease expiry in both sidebars, reconnect without duplicate Done, host-local acknowledgements, process exits, and exporter targeting.
3. Repeat the master P3 private-runtime smoke with rebased artifacts to catch shared-infrastructure regressions.
4. Compare matched private workloads on master and the rebased branch, separating Pi runtime cost from monitoring cost. Retain binary hashes and workload parameters. Verify zero idle visit proofs, five-second Pi lease renewal, bounded burst writes/ingress, and CPU/RSS recovery after bursts.
5. Write new evidence with current provenance. Short acceptance phases do not establish soak results, complete journal bounds, or global deployment readiness. Do not install global assets as part of validation.

## Final review and publication

```bash
git merge-base --is-ancestor d1b58fb HEAD
git range-diff 4fa4b97..backup/pi-monitoring-pre-rebase-276d194 d1b58fb..HEAD
git diff --stat d1b58fb..HEAD
git status --short
```

Review range-diff changes for the intended watcher adaptation and new tests; confirm master-only performance paths were not reverted. Because `pi-monitoring` is published, pushing the rebased history requires explicit approval and a force-with-lease pinned to the previously observed remote commit. Do not push during planning. Use `git rebase --abort` if integration fails; retain the backup branch after success until review is complete.

## Execution record — 2026-10-14 UTC

- Refreshed `origin/master`; it remained exactly `d1b58fb`. Observed published
  `origin/pi-monitoring` remained `276d1943f86edfaa9311ddc2f4087ba4438a6729`.
- Safely copied this initially untracked plan outside the worktree, created
  `backup/pi-monitoring-pre-rebase-276d194`, rebased the five commits in order, and
  restored the plan. No user changes were discarded.
- Replayed commits are `8723d09`, `46add8a`, `501b721`, `bef4c9a`, and `27a746f`.
  Only the native commit conflicted, only in `verij-cli/src/agent_watcher.rs`.
- Resolved both conflict regions manually: retained master's bounded reader and
  all its tests, retained the original Pi expiry test, and added one idempotent
  helper shared by incremental reads and the existing cached lease sweep. No new
  timer, polling loop, subprocess, unrestricted scan or disk transaction was added.
- Follow-up `f662fe7` adds four integration regressions. Follow-up `fa6f513` adds a
  reproducible matched private Pi resource comparison and evidence tests.
- Reviewed automatic merges: the store's only delta from master is generic Pi
  source rejection; native Pi transactions retain master's locks/tombstones and
  bounded I/O. `agent_version`, OpenCode, exporter, filesystem watcher, Magy,
  inventory and process implementations remain unchanged from master. Pi's native
  reporter/setup and their tests remain unchanged from the old branch.
- Full deterministic gates pass: 367 Rust, 98 Node and 44 Python tests, `make check`,
  paired release CLI/WASM builds, and `git diff --check`. Formatting changes were
  limited to new test code; historical formatting differences were not mixed in.
- The final frozen-harness two-distribution acceptance matrix passed all 37 checks
  per runtime. Quiet expiry appeared in both sidebars after 15.242s compiled and
  15.305s Node; reconnect took about two seconds without duplicate Done. Matched
  master/rebased Pi resource controls and the original P3 private-runtime smoke
  passed. All source/artifact hashes were rechecked by the retained evidence
  checker. See [new evidence and limitations](pi-rebase-evidence/README.md).
- Historical P0/P4 reports are unchanged. New failed attempts are retained and are
  not counted as passing gates. CPU recovered after the matched burst, but short
  recovery did not return Pi RSS to pre-burst levels in either control or enabled
  cell; this is reported explicitly, not treated as proof of memory stability.
- Ancestry and range-diff review confirm master's performance paths were retained.
  No global installation, global plugin reload, push or force-push was performed.
  The backup branch remains available. Journal bounds, combined soak and rollout
  remain pending and are not acceptance claims of this rebase.
