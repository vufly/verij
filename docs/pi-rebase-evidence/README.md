# Pi monitoring rebase evidence

This directory contains new evidence from the `pi-monitoring` rebase onto
`d1b58fb29a1cf67474a9d2323b151958789b2ec8`. Historical P0/P4 reports under
`prototypes/pi-runtime/` and `prototypes/pi-acceptance/` are unchanged.

## Implementation and deterministic checks

The five Pi commits were replayed in their original order. Only
`verij-cli/src/agent_watcher.rs` conflicted. Its resolution retains master's bounded
incremental reader, identity cache, dirty ingress, scan/prune cursors, latest-pending
publication and 32-record lease/liveness slices. An idempotent Pi-only projection
helper runs after incremental reads and within that existing sweep. Expiry clears
stale presentation without touching source snapshots, adapter state, completion
ledgers, counters or disk. Existing OpenCode lease handling is unchanged.

Four additional watcher regressions cover all statuses including stale Unknown,
read-only expiry, heartbeat restoration, quiet expiry across 40 cached records,
idempotent publication, and latest Pi completion delivery through overflow plus a
saturated UI channel. Native reporter reconnect/deduplication tests remain green.

The automatically merged store differs from master only in reserved generic Pi
source rejection. Its registration/instance locks, bounded I/O and tombstone checks
remain intact. Pi native reporter/setup code and tests are byte-identical to the
pre-rebase branch. Shared `agent_version`, OpenCode integrations, inventory, process,
Magy, filesystem watcher and exporter code remain byte-identical to master. Pi's
qualified version remains exactly `1.2.0` in native, JavaScript, setup and doctor.

Checks performed:

- `cargo test --workspace -- --test-threads=1`: 367 tests passed, including 18
  watcher tests, eight native Pi reporter tests and 16 Pi setup tests.
- `make check`: native CLI and `wasm32-wasip1` plugin passed.
- Node: 35 Pi, 55 OpenCode, and eight P0 trace tests passed.
- Python: 12 Pi runtime, 15 Pi acceptance/comparison, five Agy and 12 Codex tests
  passed. The two new comparison tests include 12 negative-evidence cases.
- Release CLI and WASM built together; `git diff --check` passed. No unrelated
  workspace formatting was applied.

The comparison test initially exposed an import-order collision between the two
existing `test_evidence.py` modules: importing runtime at discovery time prepended
the P0 directory. The new comparison module now imports runtime lazily; the full
15-test acceptance discovery passes. This was a harness-only fix.

## Runtime results

The final frozen-harness matrix in `pi-acceptance.json` passed all 37 checks per
distribution, including quiet reporter outage in both sidebars, reconnect without
replayed Done, host-local acknowledgements, process exits and exporter targeting.
`pi-acceptance-summary.json` was produced by the independent P4 evidence checker.

| Distribution | Quiet sidebar expiry | Reconnect | Two-reporter idle / burst writes | Maximum process RSS |
|---|---|---|---|---|
| Compiled Pi 1.2.0 | 15.242s | 2.035s | 2 / 5 | 180.090 MiB |
| Node Pi 1.2.0 | 15.305s | 2.028s | 2 / 5 | 166.652 MiB |

Both runtimes retained one completion per successful aggregate, zero idle visit
proofs and clean private teardown without forced kills. All final acceptance
source/binary manifest entries match the checkout. The matched comparison was
run separately and sequentially, without other acceptance or build workloads.

### Matched Pi resource comparison

`pi-resource-comparison.json` passed three sequential, fresh private cells with one
compiled Pi, two sidebars, the same real 160-nested-tool aggregate and no status
bars. Idle and recovery phases lasted ten seconds; burst measurement lasted six
seconds. Master was built separately from an archive of exactly `d1b58fb`, using
the same Rust release commands. `provenance.json` records both CLI/WASM pairs.

| Cell | Pi CPU idle / burst / recovery (%) | Sidebar + reporter CPU idle / burst / recovery (%) | Pi RSS idle / recovery (MiB) |
|---|---|---|---|
| Master, Pi monitoring absent | 1.200 / 23.911 / 0.698 | 1.900 / 1.826 / 1.895 | 145.641 / 156.691 |
| Rebased, Pi monitoring absent | 1.100 / 23.604 / 0.998 | 1.900 / 1.994 / 1.897 | 146.426 / 156.664 |
| Rebased, Pi monitoring enabled | 0.999 / 23.410 / 0.999 | 2.496 / 2.822 / 2.298 | 150.848 / 161.555 |

All cells issued zero visit proofs in every measured phase. The enabled reporter
committed two writes in each idle, burst and recovery phase; its RSS stayed at
5.395 MiB. One-second metric publication is not an instantaneous write-rate trace;
deterministic transport tests establish the 100ms write bound and five-second
renewal interval. All cells completed exactly one aggregate and exited their
private processes/providers without forced kills.

CPU returned toward idle after the burst. Pi RSS did **not** return to pre-burst
levels in the short recovery: both unmonitored controls also retained roughly
10–11 MiB after genuine tool/history work. The monitored Pi retained a similar
increase; reporter RSS stayed flat and combined sidebar RSS rose 0.125 MiB during
recovery. These observations separate runtime growth from native reporter memory,
but cannot prove absence of in-process adapter retention or long-term leaks.
Zellij CPU remained approximately 13–15%; provider/harness CPU is excluded.

### Shared-infrastructure P3 smoke

`p3-smoke.json` passed the original one-host/four-tab, no-status-bar workload using
the rebased release CLI and WASM together. Topology refresh, real sidebar/native
reporter, synthetic OpenCode streams, one completion proof/host acknowledgement,
detachment and birth-qualified private-process cleanup passed. Idle phases issued
zero proofs. This is a short shared-infrastructure smoke, not real OpenCode runtime
qualification or a matched performance comparison.

`p3-smoke-attempt-1.json` retains an earlier failed startup in a longer scratch
root. Both enabled and disabled cells timed out before inventory and cleaned up.
The rerun used a short private scratch path and passed; the startup failure's cause
was not isolated. The generic P0 evidence checker requires a matched pair with
status bars and therefore does not describe this original P3 smoke topology.
`check.py` checks the smoke's actual one-host/four-tab/no-status-bar contract.

`pi-acceptance-attempt-1.json` retains the first attempt, not a passing matrix. Its
compiled distribution passed all 37 checks; Node failed host registration with a
rebind timeout before late-sidebar checks. Both distributions cleaned up without
forced kills. This attempt overlapped a separate master release build; its resource
measurements are not used as a matched performance comparison. Subsequent complete
matrices run without that build load. The timeout's cause is not established.

## Reproduction and review

```bash
# Build both rebased artifacts, without installing anything globally.
cargo build -p verij-cli --release
cargo build -p verij-plugin --target wasm32-wasip1 --release

# Run the two-distribution matrix with a private npm Pi 1.2.0 installation.
python3 prototypes/pi-acceptance/verify.py \
  --cli target/release/verij \
  --plugin target/wasm32-wasip1/release/verij-plugin.wasm \
  --runtime both --node-cli /private/npm/node_modules/@earendil-works/pi-coding-agent/dist/cli.js \
  --output docs/pi-rebase-evidence/pi-acceptance.json
python3 prototypes/pi-acceptance/verify_evidence.py \
  docs/pi-rebase-evidence/pi-acceptance.json \
  --output docs/pi-rebase-evidence/pi-acceptance-summary.json

# Build master separately from an archive of the pinned target.
master=$(mktemp -d /home/vudinhn/.cache/vj-pi-master-XXXXXX)
git archive d1b58fb | tar -x -C "$master"
cargo build --manifest-path "$master/Cargo.toml" --target-dir "$master/target" -p verij-cli --release
cargo build --manifest-path "$master/Cargo.toml" --target-dir "$master/target" -p verij-plugin --target wasm32-wasip1 --release
python3 prototypes/pi-acceptance/compare.py \
  --master-cli "$master/target/release/verij" \
  --master-plugin "$master/target/wasm32-wasip1/release/verij-plugin.wasm" \
  --cli target/release/verij --plugin target/wasm32-wasip1/release/verij-plugin.wasm \
  --seconds 10 --output docs/pi-rebase-evidence/pi-resource-comparison.json

# Repeat the original P3 topology with a short private scratch path.
scratch=$(mktemp -d /tmp/vpr-XXXXXX)
python3 prototypes/resource-usage/verify.py \
  --cli target/release/verij --plugin target/wasm32-wasip1/release/verij-plugin.wasm \
  --hosts 1 --tabs 4 --monitoring enabled --seconds 3 --warmup 2 \
  --scratch-dir "$scratch" --output docs/pi-rebase-evidence/p3-smoke.json --quiet

# Check retained reports and hashes against the current checkout/build.
python3 docs/pi-rebase-evidence/check.py
git merge-base --is-ancestor d1b58fb HEAD
git range-diff 4fa4b97..backup/pi-monitoring-pre-rebase-276d194 d1b58fb..HEAD
git diff --check

rm -rf -- "$master" "$scratch"
```

Private npm installation instructions remain in the acceptance README. All final
deterministic commands/results are retained in `deterministic-gates.json`;
`range-diff.txt` records the five original commits plus the lease-regression and
comparison follow-ups, before this evidence-only commit. Four original commits
are unchanged; the native commit differs only for the intended watcher adaptation.
The local backup branch is retained; remote history remains untouched.

## Scope

These checks do not establish four-hour soak results, complete journal retention
bounds, status-bar amplification behavior, all release/host combinations or global
rollout readiness. No global Pi/Verij assets are installed or reloaded. The published
branch is not pushed; its pre-rebase backup remains available for review.
