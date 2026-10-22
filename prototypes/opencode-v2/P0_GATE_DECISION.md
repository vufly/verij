# P0 exit decision — blocked by the pinned upstream contract

**Full-recovery P0 has not passed. Full-recovery P1/P2 and production admission remain prohibited.**

**Scope exception approved 2026-10-21:** the user authorized a separate experimental live-status implementation using cached metadata/events only. This is not a P0 waiver or a full-recovery P1/P2 implementation. Native schema 2 rejects completion identities and outcomes; no upstream transport/freshness guarantee is claimed. See the [approved reduced-scope track](../../docs/OPENCODE_V2_UPGRADE_PLAN.md) and [adapter guide](../../integrations/opencode/v2/README.md). Global rollout remains unapproved.

This is the exit-gate decision for OpenCode **2.0.32**, commit
`843a6c514e606b436a7fe713422ace0a93ad3d61`. It is not a declaration that every
P0 experiment is complete. The unchecked qualification work in the upgrade
plan remains unchecked.

## Decisive blocker

The required bounded observation transport cannot be established through the
reviewed public CLI API on this target without violating the approved plan.
The [offline source audit](transport-source-results.json) pins seven complete
source files and checks eight structural facts. It complements the existing
[synthetic SDK characterization](sdk-results.json) and private runtime evidence;
it does not substitute for runtime transport qualification.

1. The public `Context` exposes `client: OpenCodeClient`, not the connection's
   authenticated transport constructor, observation factory, or generation.
   `createPluginContext` assigns `client: host.client.api`.
2. The SDK captures its constructor's `fetch`. Per-request options contain only
   `signal`, `headers`, and `onActivity`; none intercepts a response before JSON
   buffering/parsing. Both normal success and declared-error paths call the
   full-body JSON helper. Existing characterization demonstrates over-limit
   bodies, depth, and node counts reaching the caller.
3. Standalone generates a random child credential privately. The fixture's
   password for an explicitly launched `serve` is not that credential and is
   not a production provenance mechanism. Discovering another managed server
   would not establish the standalone client's connection.
4. The host can replace `api` during reconnect, whereas plugin construction
   takes its current value. The data cache separately receives a dynamic API
   getter and the host connection. This is a source-observed lifecycle concern,
   not a reproduced reconnect failure. Reading the shared cache does not give
   the plugin an independently bounded physical transport or freshness proof.

The source paths, upstream commit, and SHA-256 values are retained in the audit
report. The audit
never imports or executes upstream code, starts OpenCode, reads user service
state, or obtains credentials. The result describes the pinned sources only.

## Recheck against installed 2.1.2

The [2026-10-21 source recheck](TRANSPORT_RECHECK_2_1_2.md) pins upstream
commit `a517687fde98a7329ada3ae644a01a9fc6c4e99e` and confirms all eight
source-boundary facts still hold. The transport blocker remains. This does
not change the 2.0.32 qualification target or establish 2.1.2 runtime support.
Feature request [anomalyco/opencode#57885](https://github.com/anomalyco/opencode/issues/57885)
was filed with user approval on 2026-10-21. Its
[automated triage reply](https://github.com/anomalyco/opencode/issues/57885#issuecomment-6324541538)
confirms the gap and reports escalation to maintainers, but provides no design
approval, implementation commitment, or timeline. Await maintainer feedback
before an implementation PR. No patched installation or repinning has been
performed; a private patched-target experiment still requires explicit approval.

## Rejected shortcuts

- A small ordinary response, `limit=1`, or projection after SDK resolution does
  not bound the earlier allocation or HTTP error parser.
- Cancelling after the result resolves is too late. A request deadline alone
  does not enforce byte, depth, or structural-node limits.
- Cache emptiness, completed `sync()`, `session.outcome`, and an idle head do
  not establish fresh complete-family quiescence or absence of newer work.
- An event-only adapter cannot reconstruct missed terminals and pending family
  work after reload. It cannot silently claim the planned monitoring contract.
- Reading process/service credentials, guessing endpoint defaults, modifying
  shared client methods/global fetch, or proxying normal agent execution is
  explicitly forbidden by the plan.
- Patching the private harness or testing a hypothetical factory does not make
  that factory available in the installed hash-pinned executable.

## Required unblock

Upstream must supply a supported independently owned observation connection
with pre-parse transport decoration and connection-generation semantics, or
an equivalently bounded metadata API. The exact requirements and adversarial
acceptance matrix are in [the upstream transport requirement](UPSTREAM_TRANSPORT_REQUIREMENT.md).
A transport decorator alone does not establish fresh family/input recovery.

The plan prohibits upgrading/downgrading the installed target. Therefore,
shipping a patched build or switching to a release with the hook requires an
explicit scope/target decision; no such installation or repinning was done.

After the upstream capability is available and the target is explicitly pinned:

1. Demonstrate its actual CLI provenance in standalone, explicit authenticated
   server, and authenticated path-prefix modes. Exercise the existing body,
   cancellation, concurrency, deadline, replacement, and cleanup adversaries.
2. Finish child/inactive-tab/plugin-route and remaining interruption contracts.
3. Finish stable identity and recovery across successful execution, managed
   active shutdown/resume, missed events, reload/reselection, revert, and fork.
   Define the no-model-work revert-clear acknowledgement policy.
4. Prove fresh complete-family permission/form/inbox recovery under concurrent
   pagination and account for implicit cache requests and physical cancellation.
5. Re-run the evidence checkers and regressions. Only then reassess the P0 gate.

The retained loading, SDK shapes, markers, lifecycle, CLI reload, terminal,
reconciliation, and source-audit successes remain useful partial evidence.
None changes this decision to a gate pass. No production files, installer
activation, global configuration, or shared service were changed.
