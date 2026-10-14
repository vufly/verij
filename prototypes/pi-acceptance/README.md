# Pi monitoring P4: local end-to-end and resource acceptance

This harness exercises **real Pi 1.2.0 in stock Zellij 0.45.1**, with the production Verij exporter, owned monitoring installation, native reporters, and two real Verij sidebars. It is separate from the [P0 lifecycle probe](../pi-runtime/README.md) and synthetic ownership/reporter tests.

The local acceptance matrix comprises **37 checks per distribution**, for compiled Pi and the private npm/Node CLI. [Results](results.json) retain sanitized native-state transitions, independent lifecycle event counts, workload details, binary/source SHA-256 hashes, resource counters, and cleanup evidence. [Summary](summary.json) is produced by the separate evidence checker. Completion is qualified only within the limits below; this does not authorize global installation or claim the complete Verij release matrix.

## What runs

- `verij agent setup pi` installs embedded production assets into a private agent directory. Monitoring is conventionally autoloaded, not supplied through `--extension`. Explicit extensions are only the active test fixtures and P0 trace observer.
- A shell launches real Pi inside a stock Zellij terminal pane. Registration and `agent verify` use actual exporter inventory and foreground process ancestry; there is no injected inventory or synthetic agent record.
- Both Pi processes use the same private working directory. Distinct pane/process/conversation identities, rather than working directory, establish attribution.
- Two outer Verij host sessions attach to the shared inner session. Each runs the production sidebar and maintains its own durable acknowledgement store.
- The P0 loopback provider supplies success, tools, retries, failures, compaction and continuation. A second loopback provider supplies the burst: one parent tool plus **160 real nested tool calls in each Pi pane**. Provider bodies and generated content are not retained.
- Private HOME, agent, XDG, socket, runtime and temporary directories isolate configuration and transcripts. Launch environments are allowlisted; provider credentials, inherited Pi session IDs, and existing Zellij sessions are not used.
- Zellij sessions are seeded through a private tmux socket. Explicit stock pane/tab actions create the workload and host layouts, avoiding attach/create layout-option differences. Session serialization is disabled.
- Reporter outage injection renames only a private executable copy. Existing global and built executables are never modified.

## Accepted local cases

| Area | Evidence |
|---|---|
| Startup and late sidebars | One nonsynthetic Pi row, foreground verification, autoloaded adapter, live doctor result and two sidebar displays. |
| Aggregate outcomes | Success, parallel/nested tools, recovered tool error, provider error, retry success/exhaustion, cancellation, queued/late work, continuation and automatic compaction. One completion per successful aggregate; no intermediate completion. |
| Missing outcome evidence | Post-run exception and stale pre-continuation success become Unknown, not Done. |
| Blocking UI | All five extension dialog kinds become Needs input before dismissal; nested UI produces one outer request. Visiting preserves the request; no permission provenance or dialog details are invented. |
| Host-local Done | Visiting host A preserves host B's unread completion. Host B subsequently acknowledges the same observed revision independently. |
| Recovery | A killed reporter reconnects without changing the completion revision or timestamp. Heartbeats and idle reload do not replay Done. A longer outage expires the 15-second lease in both actual sidebars while Pi continues working; recovery delivers the genuinely new run once. |
| Conversation lifecycle | Name, tree, new, resume and fork update/reset execution metadata without duplicating the process row or replaying historical completion. Manual compaction does not create completion. |
| Targeting | Native agent activation reaches renamed, moved, floating and stacked panes from a background tab, retaining immutable identity. |
| Detached child | A real Pi TUI with inherited pane hints but an unrelated controlling PTY is rejected by foreground verification and creates no additional row. |
| Non-TUI and opt-out | Print, JSON, RPC, help, version, export and `--no-extensions` create no reporter metric files or extra rows; stdout contains no reporter protocol. |
| Lifetimes | Kill, normal quit and pane close remove displayed rows. Relaunch in the same pane creates a fresh process/agent identity, revision zero and no transferred host acknowledgement. |
| Resources | Two live reporters, idle lease writes, zero idle visit proofs, nested-tool burst coalescing, CPU/RSS measurements and clean private-process shutdown. |

The watcher projects lease expiry and process liveness on its read side. The harness checks the actual sidebar for outage/exit presentation instead of expecting fabricated source transactions on disk.

## Resource interpretation

Measurements use four six-second phases: acknowledged heartbeat, idle, two-pane/two-host idle, and simultaneous two-pane tool burst. Counters are the existing opt-in `VERIJ_RESOURCE_METRICS_DIR` snapshots. P4 adds Pi dispatch to the instrumentation guard; previously Pi reporter writes were unmeasured. The evidence gate rejects zero/missing reporter write counters rather than treating them as successful throttling.

- Idle reporters renew leases every five seconds; no passive visit proof is issued without an eligible unread completion.
- Burst calls are real Pi nested executions, not synthetic calls into the projector. Both aggregates settle once; redundant Working refreshes coalesce.
- Counter snapshots have one-second publication resolution. Runtime phase totals test the sustained write budget with a bounded boundary allowance; deterministic transport tests separately enforce the 100ms write interval, 128-item queue, one in-flight snapshot, bounded timers, overflow degradation, teardown and reconnect.
- CPU/RSS includes identified private pane processes, sidebars, reporters and Zellij clients/servers. Provider and harness CPU are not included. Pi RSS includes the complete agent runtime, not just monitoring. Instrumentation adds its own opt-in publishing thread.
- A 768 MiB per-process RSS guard stops the short private experiment. Results are machine-specific observations, not long-run memory-stability guarantees.

Retained run measurements (two reporters, approximately six seconds per phase):

| Distribution | Two-pane idle record writes | Burst record writes | Maximum process RSS across all phases |
|---|---:|---:|---:|
| Compiled | 2 | 6 | 181.152 MiB |
| Node | 2 | 5 | 172.918 MiB |

Every idle phase has zero visit-proof attempts. A burst phase may acknowledge the newly observed successful revision when its host is actually focused; that is not an idle proof. Native counter publication is not a per-event log or an instantaneous-rate measurement.

## Explicit limits / remaining coverage

These are not silently counted as passed:

- Forced numerical PID/server reuse and serialized session resurrection were not exercised. Same-pane process replacement was exercised; process-birth/server-incarnation rejection also remains covered by existing native identity tests.
- Sidebar keyboard selection stability was not exercised. Targeting uses the production native activation path; existing tree/selection unit tests remain separate evidence.
- Other Pi releases are unsupported and tested through version guards, not another-release runtime matrix. P0's observer-order matrix remains separate qualification.
- Idle reload was exercised, not active-run or unresolved-dialog reload continuity. Such continuity remains conservative/Unknown.
- Remote agents/hosts, background work outside Pi's tracked lifecycle, UI bypassing Pi's extension API, and real-provider credential workflows remain outside this qualification.
- No status-bar amplification, many-tab soak or long-duration stress acceptance was performed. The broader monitoring resource roadmap remains unfinished.
- Queue/timer bounds are deterministic-test evidence, not runtime introspection.

## Reproduce without global changes

Requirements: Linux `/proc`, stock Zellij 0.45.1, tmux, Python 3.10+, compiled Pi 1.2.0, Node 22.19+, Rust with `wasm32-wasip1`, and a private npm Pi 1.2.0 installation. This run used Python 3.14.8, Node 24.21.0 and Rust 1.99.0; compiled Pi's Bun engine was qualified in P0.

```bash
cargo build -p verij-cli --release
cargo build -p verij-plugin --target wasm32-wasip1 --release

scratch=$(mktemp -d /tmp/vj-pi-node-XXXXXX)
chmod 700 "$scratch"
npm install --prefix "$scratch" --ignore-scripts --no-audit --no-fund \
  --save-exact @earendil-works/pi-coding-agent@1.2.0

python3 prototypes/pi-acceptance/verify.py \
  --cli target/release/verij \
  --plugin target/wasm32-wasip1/release/verij-plugin.wasm \
  --runtime both \
  --node-cli "$scratch/node_modules/@earendil-works/pi-coding-agent/dist/cli.js"
python3 prototypes/pi-acceptance/verify_evidence.py \
  prototypes/pi-acceptance/results.json \
  --output prototypes/pi-acceptance/summary.json
python3 -m unittest discover -s prototypes/pi-acceptance -p 'test_*.py' -v

rm -rf -- "$scratch"
```

`--runtime compiled` is a smoke test, not the two-distribution gate. `--seconds` permits 5–60-second resource phases. `--debug` prints private diagnostics to the caller; never publish those logs. Ordinary retained failures contain only exception class, sanitized completed checks and cleanup metadata. Every successful gate requires both providers stopped, all discovered birth-qualified private processes exited, temporary directories removed, and zero forced cleanup kills.

## Regression checks

```bash
cargo test --workspace
make check
node --test integrations/pi/core.test.mjs
node --test integrations/opencode/*.test.mjs
node --test prototypes/pi-runtime/trace.test.mjs
python3 -m unittest discover -s prototypes/pi-runtime -p 'test_*.py'
git diff --check
```

The new evidence suite has 13 positive/negative tests, including incomplete matrices, missing scenarios, premature native completion, abort-as-Done, invented permission provenance, content leakage, unclean shutdown, missing instrumentation, idle proofs, write-budget violations and insufficient actual burst tools. The existing suites cover 333 Rust tests, 35 Pi tests, 22 OpenCode tests, eight P0 trace tests and twelve P0 Python tests. Strict TypeScript checking of the active P4 fixture also passes. Workspace-wide `cargo fmt --all --check` reports pre-existing formatting differences; P4's changed Rust files pass targeted formatting checks and unrelated formatting was left untouched.
