# Pi 1.2.0 lifecycle qualification (P0)

This prototype qualifies the lifecycle contract needed by [Pi monitoring](../../docs/PI_MONITORING_PLAN.md). It is **not** the production adapter, a Pi launcher, a pane-ownership test, or the stock-Zellij end-to-end acceptance matrix.

## Qualification result

P0 passes for Pi **1.2.0** in a real interactive TUI over a controlling PTY:

| Distribution | Actual engine | Observer order | Scenario runs |
|---|---|---|---:|
| Compiled Pi | Bun 1.3.14 | Before fixture | 28 |
| Compiled Pi | Bun 1.3.14 | After fixture | 28 |
| npm CLI | Node 24.21.0 | Before fixture | 28 |
| npm CLI | Node 24.21.0 | After fixture | 28 |

The retained [results](results.json) contain 112 scenario runs and 1,608 whitelisted event records, executable and fixture hashes, per-case provider request metadata, and successful cleanup for all four processes. [Summary](summary.json) is the independent verifier's output. Event traces use one compact JSON object per line within the report for reviewability.

The provider returns synthetic model output; the test extension deliberately creates work, dialogs, queues, continuation, and failure conditions. Pi's scheduling, events, terminal input, model retries, compaction, and session replacement are real. No model subscription, API credential, existing Pi session, or existing Zellij session is used.

## Files and responsibilities

- `observer.ts`: passive event observer. It never mutates events, returns continuation, opens UI, registers tools, or writes monitoring entries into the session.
- `trace.mjs`: bounded, whitelist-only event projection and trace writer. No token/update handlers or transcript enumeration.
- `fixture.ts`: **active test fixture**, not production monitoring code. It registers harmless private tools and `/probe` commands to exercise real Pi APIs.
- `provider.py`: bounded loopback OpenAI-compatible SSE provider. Request bodies are processed in memory and discarded; only model, attempt, status, and summary-request flags are retained.
- `verify.py`: private HOME/agent/XDG/environment setup, controlling PTY, scenario driver, subprocess cleanup, and metadata-only evidence publication.
- `verify_evidence.py`: independent lifecycle/privacy/order/provenance checker; rejects partial runs and stale fixture hashes.
- `trace.test.mjs`, `test_evidence.py`: trace privacy, invalid data, evidence mutation, and constructor-failure cleanup tests.

The prototype trace writer makes small synchronous appends for event-order evidence and caps each private trace at 1 MiB. This is intentional instrumentation, **not** the planned nonblocking production reporter transport.

## Reproduce without touching global configuration

Requirements: Linux `/proc`, Python 3.10+, installed compiled Pi 1.2.0, and Node 22.19+ with npm. Run from the repository root.

Create a private, version-pinned npm CLI installation. Lifecycle scripts are disabled; no dependency installation occurs in this repository or the global Pi directory:

```bash
scratch=$(mktemp -d /tmp/vj-pi-node-XXXXXX)
chmod 700 "$scratch"
npm install --prefix "$scratch" --ignore-scripts --no-audit --no-fund \
  --save-exact @earendil-works/pi-coding-agent@1.2.0

python3 prototypes/pi-runtime/verify.py \
  --pi-bin pi \
  --node-cli "$scratch/node_modules/@earendil-works/pi-coding-agent/dist/cli.js"
python3 prototypes/pi-runtime/verify_evidence.py

# Remove only the private directory created above.
rm -rf -- "$scratch"
```

`verify.py` never installs dependencies itself. `--node-cli` is mandatory when Node is selected. Both executable versions must be exactly 1.2.0. `--runtime compiled|node|both`, `--order observer-first|observer-last|both`, `--cases ...`, and `--output PATH` support narrow investigations. The independent P0 gate requires the **complete** two-runtime × two-order matrix; a successful smoke run does not satisfy it.

Every child starts with an explicit environment allowlist, private HOME and agent/config/cache/data/state/tmp directories, offline mode, no inherited provider credentials, no project resources, no built-in tools, and only the two explicit probe extensions. Private storage, transcripts, and the in-memory bounded terminal tail are removed after the run. The loopback provider and Pi process group are stopped in cleanup. Constructor/setup failures also close the provider. Failures retain only exception type and available sanitized event metadata, never terminal text or error bodies.

The harness chooses and drives dialogs only in its isolated test process. Its Escape inputs are test stimuli, not a production permission bypass or automatic approval policy.

### Fast checks

```bash
node --test prototypes/pi-runtime/trace.test.mjs
python3 -m unittest discover -s prototypes/pi-runtime -p 'test_*.py' -v
python3 prototypes/pi-runtime/verify_evidence.py
git diff --check
```

Eight JavaScript tests and twelve Python tests passed. The TypeScript probe extensions also passed strict typechecking against the private npm Pi 1.2.0 declarations, without adding repository dependencies:

```bash
npm install --prefix "$scratch" --ignore-scripts --no-audit --no-fund --no-save \
  typescript@5.9.3 @types/node@24
mkdir -p "$scratch/typecheck"
cp prototypes/pi-runtime/{observer.ts,fixture.ts,trace.mjs} "$scratch/typecheck/"
printf '{"type":"module"}\n' > "$scratch/typecheck/package.json"
"$scratch/node_modules/.bin/tsc" --noEmit --strict --allowJs --checkJs false \
  --skipLibCheck --module NodeNext --moduleResolution NodeNext --target ES2022 \
  "$scratch/typecheck/observer.ts" "$scratch/typecheck/fixture.ts"
```

Run this before removing the private installation. Rust/WASM tests were not rerun for P0: no production Rust/WASM source changed.

## Contract established for P1/P2

### Aggregate identity and completion

1. A genuine aggregate run emits one `before_agent_start`. Keep its opaque run ID until the corresponding `agent_settled`, even if low-level `agent_start/end` occurs more than once.
2. Two parallel tools and nested `ctx.executeTool()` calls finish inside the same aggregate. Nested tool events carry `parentToolCallId`; child/tool completion never creates pane Done.
3. Steering plus follow-up work produces three assistant turns but one aggregate settlement. A follow-up submitted from `agent_end` creates another low-level run, not another aggregate start.
4. `agent_before_settle` is **not final**. The continuation fixture produces two boundaries and two low-level runs, with only one final settlement. An observer loaded first sees `continue_proposed: false` at the first boundary because a later extension has not proposed continuation yet; loaded last, it sees `true`. Neither value authorizes Done.
5. Automatic threshold compaction occurs during active work, before the final assistant turn and settlement. It does not complete the aggregate. Manual compaction success/failure produces paired compaction events with no `agent_start` or `agent_settled`.
6. `agent_settled` has `aborted`, but no outcome. A provider 400 failure and exhausted 503 retries settle with `aborted: false` and a final `agent_before_settle.outcome: error`. Retry-then-success starts with an error turn but finishes with `completed`. Do not latch the first error as terminal.
7. Escape during an open provider stream settles with `aborted: true` and a preceding aborted turn. It **does not** emit `agent_before_settle`; cancellation must not require that boundary.
8. A recoverable tool error is followed by a successful assistant turn and successful aggregate outcome. `tool_execution_end.isError` alone does not establish terminal Error.
9. **Post-run exception guard:** the fixture selects an invalid private compaction override from `agent_end`, after a completed assistant turn. Pi then settles with `aborted: false` and **no** `agent_before_settle`. Consequently, a successful last `turn_end` plus non-aborted settlement is insufficient success evidence. Require a current final pre-settlement success candidate, final quiescence, and no conflicting work/input; missing final-boundary evidence remains Unknown.

10. **Stale-candidate guard after continuation:** a second fixture first requests continuation from a successful `agent_before_settle`, then triggers the post-run exception in the resumed low-level run. Final settlement is non-aborted, but the only success candidate predates resumed work. This must remain Unknown, not Done.

The first `agent_before_settle` candidate can still be superseded by continuation. Invalidate it when newer `agent_start`/turn work resumes; require a fresh final candidate after that work and decide only at settlement. Beginning a new aggregate invalidates all previous outcome candidates. A failure before an observable run starts is not qualified as a terminal outcome by this probe.

### Blocking UI

- Real select, confirm, input, editor, and custom dialogs emit start/end pairs before tool and aggregate completion.
- A child confirmation inside an outer custom prompt is executed, but only the **outer** custom start/end pair is emitted. Do not maintain a guessed nested-dialog count.
- A blocking confirmation opened while Pi is idle still emits a prompt pair with `ctx.isIdle() === true`; closing it emits no agent settlement. Pending UI takes precedence over execution activity and restores the previous execution state when resolved.
- Prompt events contain no request ID or permission provenance. Generate a local opaque request ID for the outer pair. Do not describe every confirm dialog as a verified permission request.
- External browser approvals, direct terminal readers, and UI bypassing Pi's extension API remain unsupported. Assistant prose is not a Needs input signal.

### Session identity and runtime replacement

- Name changes are metadata only; retained traces store name presence, not text.
- Tree navigation emits before/after tree events, not an agent completion.
- New, resume, fork, and reload tear down and recreate the extension runtime **inside the same Pi PID**. They produce paired shutdown/start reasons and distinct extension runtime IDs.
- New/fork changes conversation ID; resume returns to the original conversation; reload preserves conversation ID.
- No replacement/reload event replays a historical `agent_settled`. Do not synthesize historical Done from a resumed transcript.
- Quit emits `session_shutdown(reason: quit)` and the process exits. Other shutdown reasons do not establish process exit.

Reload was exercised **while idle**, not mid-stream or with unresolved UI. P1/P2 must preserve native deduplication metadata and conservatively report Unknown when reload continuity cannot be proven. This evidence does not qualify reconnect or acknowledgement persistence.

## Explicit remaining scope

P0 is complete for the observed 1.2.0 contracts above. The following remain unsupported or belong to later gates:

- Other Pi releases, SDK embeddings, print/JSON/RPC observation, and detached or remote agents.
- Separately running background extension work without its own verified pane or explicit observer contract.
- Startup/preparation failures without observable outcome, provider protocols other than the local OpenAI-compatible fixture, overflow recovery and length-limit completion semantics, and auto-compaction failure/cancellation beyond the manual failure case.
- Mid-work/mid-dialog reload continuity and native reporter reconnect/health/deduplication.
- Foreground pane ownership, navigation/visibility, two-host acknowledgement, process/pane reuse, and resource acceptance with actual Verij/Zellij.

These exclusions prevent the P0 result from being mistaken for production readiness. P1 is the next task.
