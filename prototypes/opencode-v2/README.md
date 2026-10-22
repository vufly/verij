# OpenCode V2 — P0 contract spike

**Status: full-recovery P0 GATE BLOCKED. No full-recovery production admission or P1/P2 work permitted.**

A separate [experimental live-status track](../../integrations/opencode/v2/README.md) was approved on 2026-10-21. `live_status_smoke.py` and `live-status-loading-results.json` cover exact production live-status JavaScript loading on hash-pinned **2.1.2**, with a fixture protocol sink in private standalone/server CLI runs. These checks prove loading, Unknown-first/Home state, lease renewal, disable cleanup, and re-enable fencing. A third selected-session private-server run observes its own cached form, excludes a foreign form, and leaves both pending. This is **not native ownership, end-to-end native-pane/provider acceptance, resource qualification, or full-recovery P0**. The original 2.0.32 pin/evidence below is unchanged.

Run the separate smoke with a canonical actual ELF and its independently checked hash:

```sh
python3 prototypes/opencode-v2/live_status_smoke.py \
  --binary "$ELF" --sha256 "$SHA256" --expected-version 2.1.2 \
  --output /tmp/verij-v2-live-loading.json
```

[Exit-gate decision and exact unblock requirements](P0_GATE_DECISION.md): the
pinned public CLI lacks the required observation transport. This is an external
contract blocker, not a passing milestone or a reason to mark open tests done.

Date: 2026-10-17 UTC. Upstream target: `v2.0.32`
(`843a6c514e606b436a7fe713422ace0a93ad3d61`). Verij baseline:
`5b6ea1ebd28472b54142d79f87f48854f64ff7c4`.

This directory is an investigation harness. Evidence files should be retained
per plan guidance; do not interpret any result here as production readiness.

---

## Pinned binary

| Item | Path / SHA-256 |
|------|----------------|
| Shell shim (`bin/opencode`) | `/home/vudinhn/.local/share/mise/installs/opencode/2.0.32/bin/opencode`<br>`cdf4729f26f340ce84eb0e1b3b96912e584eb1f53141d7da9ffe590280549ac5` |
| **ELF** (`@opencode/cli`) | `/home/vudinhn/.local/share/mise/installs/opencode/2.0.32/v11/398d3-18df6e3f729ee701-0/node_modules/.pnpm/@opencode+cli@2.0.32/node_modules/@opencode/cli/bin/opencode.exe`<br>`489b51816a77809e841310e80c88e834c28c19ad1639e12ba8a19f08b47ab607` |

`bin/opencode` is a POSIX **shell script** (not a JavaScript launcher) that
resolves symlinks and exec-replaces itself with the real ELF. The smoke
harness rejects it because `stream.read(4) != b"\x7fELF"`.

**Always pass the ELF path** (`opencode.exe` inside `@opencode/cli`) to
`--binary`. Resolve it once and verify:

```sh
ELF=/home/vudinhn/.local/share/mise/installs/opencode/2.0.32/v11/398d3-18df6e3f729ee701-0/node_modules/.pnpm/@opencode+cli@2.0.32/node_modules/@opencode/cli/bin/opencode.exe
sha256sum "$ELF"
# expected: 489b51816a77809e841310e80c88e834c28c19ad1639e12ba8a19f08b47ab607
```

---

## Harness files

| File | Purpose |
|------|---------|
| `pin.json` | Pinned version, upstream commit, ELF SHA, and shim SHA |
| `contract-probe.mjs` | Plugin loaded by OpenCode; records shape/route/cleanup frames |
| `smoke.py` | Disposable-HOME launch harness; writes a report JSON |
| `verify_evidence.py` | Strict allowlist checker for evidence files |
| `test_smoke.py` | Deterministic unit tests; **does not launch OpenCode** |
| `smoke-results.json` | Retained sanitized CLI-loading evidence |
| `sdk-probe.mjs`, `sdk-results.json` | Hash-pinned tagged client with synthetic fetch; demonstrates missing allocation guards |
| `bounded-transport.mjs`, `*.test.mjs` | Prototype GET-only transport and adversarial JavaScript tests |
| `marker_probe.py`, `provider.py` | Private headless runtime with a controlled loopback provider |
| `transport-check.mjs` | Read-only bounded observer; endpoint/auth arrive through private stdin |
| `marker_evidence.py`, `marker-results.json` | Bounded checker and sanitized marker/input/pagination evidence |
| `test_markers.py` | Deterministic marker-evidence and fixture safety tests |
| `lifecycle_probe.py`, `lifecycle_evidence.py` | Separate private revert/active-shutdown experiment and independent checker |
| `lifecycle-results.json`, `test_lifecycle.py` | Sanitized lifecycle evidence and mutation/privacy regressions |
| `cli_contract.py`, `cli-observer*.mjs` | Private interactive route/event/source-and-config-reload experiment |
| `cli_evidence.py`, `cli-results.json`, `test_cli_contract.py` | Independent CLI checker, sanitized evidence, and deterministic regressions |
| `sdk_shapes.py`, `sdk-runtime.mjs`, `sdk-observation.mjs` | Tagged Promise-client response shapes on an independently provisioned bounded private connection |
| `sdk_shapes_evidence.py`, `sdk-shapes-results.json`, `test_sdk_shapes.py` | SDK shape evidence checker, retained runtime result, and evidence/safety regressions |
| `terminal_contract.py`, `terminal-observer.mjs` | Separate private real-TTY provider-failure/user-abort event experiment |
| `terminal_evidence.py`, `terminal-results.json`, `test_terminal_contract.py` | Strict terminal-event checker, sanitized evidence, and privacy/safety regressions |
| `reconciliation_contract.py`, `reconciliation-*.mjs` | Separate private explicit-server event-to-marker identity experiment |
| `reconciliation_evidence.py`, `reconciliation-results.json`, `test_reconciliation.py` | Strict reconciliation checker, sanitized evidence, and deterministic regressions |
| `transport_source_audit.py`, `transport-source-results.json`, `test_transport_source_audit.py` | Seven-file pinned offline source audit, strict evidence validation, and safety regressions |
| `P0_GATE_DECISION.md` | Explicit blocked exit decision and upstream/requalification requirements |

---

## Running the deterministic tests

No binary or network access required:

```sh
cd /home/vudinhn/repos/verij/prototypes/opencode-v2
python3 -m unittest test_smoke -v
```

Tests cover synthetic failure/privacy cases and validate the retained smoke
against current artifact hashes. Passing tests do not qualify V2 monitoring.

---

## Running the smoke harness

> **Prerequisites:** Linux kernel ≥ 5.3 (`pidfd_open`, `pidfd_send_signal`).
> Python 3.10+ (tested on current system). Do not run as root.
> The temporary directory and all its parents must not contain `.opencode`,
> `.git`, `.hg`, or `AGENTS.md` — the harness refuses ambient project roots.

```sh
cd /home/vudinhn/repos/verij/prototypes/opencode-v2

ELF=/home/vudinhn/.local/share/mise/installs/opencode/2.0.32/v11/398d3-18df6e3f729ee701-0/node_modules/.pnpm/@opencode+cli@2.0.32/node_modules/@opencode/cli/bin/opencode.exe

python3 smoke.py --binary "$ELF" --output /tmp/verij-v2-p0-smoke.json
```

The harness:

1. Verifies the ELF SHA against `pin.json` and rejects the shim or a
   wrong-hash binary.
2. Walks ancestor directories of the temp root and refuses any directory that
   contains `.opencode`, `.git`, `.hg`, or `AGENTS.md` (tagged CLI plugin
   discovery has its own ancestor walk independent of `OPENCODE_DISABLE_PROJECT_CONFIG`).
3. Creates a fully isolated `TemporaryDirectory` with private HOME, XDG dirs,
   OpenCode config dir, and Verij state/inventory roots.
4. Writes a private `cli.json` with only the fixture plugin. Tagged CLI
   packages skip direct file entries; the fixture is placed at
   `fixture/tui.js` inside a directory and registered via its directory path.
   No legacy `tui.json`, no repo/project dependencies.
5. Exercises both `--standalone` mode and an explicitly managed private `serve`
   + `--server URL` client. The private server is started with a random
   `OPENCODE_PASSWORD`; the client authenticates against it. The password is
   never recorded in any output.
6. Records the private process layout (client, server, helper roles) via
   `/proc/{pid}/exe` comparison while known parents are still alive.
7. Signals cleanup via Ctrl-C (fixture lifecycle control; not a navigation,
   cancel, or reply call). Sends SIGTERM then SIGKILL only to recorded private
   process births via `pidfd`; never touches the user's shared service.
8. Runs `verify_evidence.validate` before writing any output.

Output is the requested report JSON; read its `status` field. `SMOKE_PASS` means plugin
loading, layout recording, and cleanup ran for both modes. It is **not** a P0
gate pass — see blockers in `CONTRACT.md`.

Validate a saved report separately:

```sh
python3 verify_evidence.py /tmp/verij-v2-p0-smoke.json
# Check the retained run independently:
python3 verify_evidence.py smoke-results.json
```

---

## Smoke run history

| Attempt | Status | Notes |
|---------|--------|-------|
| 1 (standalone) | INCOMPLETE | Timed out before route frame |
| 1 (server) | INCOMPLETE | Client exited before frame |
| Final retained run | **SMOKE_PASS** (both modes) | Current artifact hashes and recorded-process cleanup verified |

The passing run is retained as `smoke-results.json`. The first failure exposed
two tagged-runtime requirements: register a plugin directory, not a direct
file; supply an explicit password for private `serve`. Intermediate local
attempts also passed, but only the final current-artifact run is retained here.
`SMOKE_PASS` is **not** a P0 gate pass; see `CONTRACT.md` for its scope.

The continuation adds independent transport/SDK JavaScript regressions and
marker-evidence tests. Run all prototype tests with:

```sh
node --test prototypes/opencode-v2/*.test.mjs
python3 -m unittest discover -s prototypes/opencode-v2 -p 'test_*.py'
```

Existing V1 focused regressions remain separate from V2 runtime evidence.
The full workspace/resource gates were not run for this prototype-only change.

---

## Tagged SDK characterization

Use an explicit Bun executable and an absolute canonical directory containing
the tagged `packages/client/src/promise/generated/client.ts` and
`client-error.ts`. The probe verifies both source hashes before importing; it
never downloads code or connects to a real endpoint. A disposable source copy
can be prepared explicitly:

```sh
SDK_SOURCE_DIR="$(mktemp -d /tmp/verij-v2-sdk.XXXXXX)"
SOURCE=https://raw.githubusercontent.com/anomalyco/opencode/843a6c514e606b436a7fe713422ace0a93ad3d61/packages/client/src/promise/generated
curl --fail --silent --show-error "$SOURCE/client.ts" -o "$SDK_SOURCE_DIR/client.ts"
curl --fail --silent --show-error "$SOURCE/client-error.ts" -o "$SDK_SOURCE_DIR/client-error.ts"
# Set BUN to an explicit executable path; no default OpenCode client is used.
"$BUN" prototypes/opencode-v2/sdk-probe.mjs --source-dir "$SDK_SOURCE_DIR" > /tmp/verij-v2-sdk.json
node prototypes/opencode-v2/sdk-probe.mjs --verify /tmp/verij-v2-sdk.json
node prototypes/opencode-v2/sdk-probe.mjs --verify prototypes/opencode-v2/sdk-results.json
rm -rf -- "$SDK_SOURCE_DIR"  # only this newly created disposable source directory
```

Run these commands from the repository root. All oversized bodies are generated
locally. Output contains only fixed case names, source/artifact hashes, and
booleans; passing characterization documents the missing guards, not support.

## Controlled-provider marker experiment

From the repository root, use the same pinned ELF and an explicit canonical
Node executable path. No global assets/configuration are installed.

```sh
python3 prototypes/opencode-v2/marker_probe.py --binary "$ELF" --node "$NODE" --output /tmp/verij-v2-markers.json
python3 prototypes/opencode-v2/marker_evidence.py /tmp/verij-v2-markers.json
python3 prototypes/opencode-v2/marker_evidence.py prototypes/opencode-v2/marker-results.json
```

This **headless private-server experiment** creates real prompts, forms,
children, and a fork, interrupts one blocked execution, and restarts only its
own server. The provider emits synthetic responses and never stores request
bodies. Fixture control mutations are separate from the GET-only observer.
The observer receives explicitly provisioned endpoint/auth over stdin, not
argv, installed options, or a manifest. A system `/etc/opencode` directory is
refused to exclude ambient server configuration.

The retained run checks marker replay/new executions, failure, completed-turn
restart, forked history, queue/steer, abort, unmodified form/viewed state, and
parent pagination. Both queued and steered prompts share one marker in this
workload; a fork has historical successful markers but no root outcome. A child
created during pagination escapes the older cursor and forces a fresh scan.

Fixture HTTP controls have byte/depth/node bounds and socket timeouts; they
are not an independently qualified aggregate streaming-deadline transport.
The separate JavaScript observer demonstrates bounded reads on the explicitly
supplied private endpoint only. Neither experiment resolves the public CLI
context's missing connection factory. Full P0 admission stays blocked.

## Revert and active-shutdown continuation

Run the separate experiment without overwriting earlier marker evidence:

```sh
python3 prototypes/opencode-v2/lifecycle_probe.py --binary "$ELF" --output /tmp/verij-v2-lifecycle.json
python3 prototypes/opencode-v2/lifecycle_evidence.py /tmp/verij-v2-lifecycle.json
python3 prototypes/opencode-v2/lifecycle_evidence.py prototypes/opencode-v2/lifecycle-results.json
```

Eight checks pass against the pinned ELF, with recorded private-process and
provider cleanup. The fixture stages/clears/commits a revert with file restore
disabled, then stops its recorded server birth during a blocked execution.
It uses the same disposable environment and bounded fixture control as the
marker experiment; it does not qualify a production observation transport.

Important results:

- Staging a revert preserves history, but clearing it wakes execution and adds
  a **new successful idle marker with no new prompt or provider request**.
  Older message identities remain unchanged. Completion acknowledgement needs
  an explicit policy for this no-work transition, not an assumption that every
  new success marker represents new model work.
- Committing the second-turn revert removes that turn and its later markers.
  The first turn's marker reappears unchanged; a replacement prompt creates a
  new marker. Neither resurfaced history nor removed-marker replay establishes
  a new completion revision by itself.
- During an active second turn, session outcome and the historical marker can
  both remain `succeeded`. Graceful shutdown adds no terminal idle marker; an
  incomplete assistant entry may be projected during teardown.
- After restarting **unregistered private `serve`**, active membership is empty
  while the same historical success remains behind unfinished history. The
  fixture proves replay of this open turn, **not automatic resume**. Tagged
  managed-server recovery is a separate path and remains unqualified.

The first local attempt exposed the mistaken clear-preserves-latest-marker
assumption. The second exposed an assistant entry projected during shutdown.
Both cleaned up; the retained passing run encodes these observed contracts,
not the original assumptions. No failure text, provider bodies, or raw IDs are
retained. Fifteen deterministic evidence/privacy/safety tests protect the lifecycle
checker. `LIFECYCLE_PASS` is not a P0 gate pass.

## Interactive CLI events and reload continuation

```sh
python3 prototypes/opencode-v2/cli_contract.py --binary "$ELF" --output /tmp/verij-v2-cli.json
python3 prototypes/opencode-v2/cli_evidence.py /tmp/verij-v2-cli.json
python3 prototypes/opencode-v2/cli_evidence.py prototypes/opencode-v2/cli-results.json
```

The separate real-TTY experiment passes **nine checks per mode** in standalone
and authenticated private `serve` + `--server` mode. It sends fixed synthetic
prompts through the terminal; the observer makes zero SDK/cache reads and no
navigation, permission/form, inbox, or viewed-state mutations. OpenCode's own
UI behavior is not attributed to the observer.

- Home has no selected session. Prompt submission selects one root session;
  its identity survives options reload, local source-file reload, and ordered
  config disable/re-enable.
- `data.on` receives `event.data`; `data.listen` receives `event.details`.
  Started/succeeded notifications match on event ID, aggregate ID, aggregate
  sequence, and version. Callback receipt order is not treated as identity.
  Sequence gaps are expected because the observer projects only two event
  kinds in this workload, not the full aggregate log.
- Every recorded frame's PID matches the verified foreground CLI birth, not
  the server. This is CLI process evidence, not native exporter/pane binding.
- Returned cleanup runs before each replacement setup. A prompt while disabled
  reaches the controlled provider without producing observer frames. Re-enable
  observes another execution on the same selected root with no duplicate
  callbacks from retired generations. Final exit invokes cleanup again.
- Provider request count is measured, not equated with execution count: the
  first prompt produced two provider requests in the retained run. The first
  local attempt exposed that incorrect one-request-per-turn assumption and
  cleaned up both modes; only the final current-artifact run is retained.

The observer retains only bounded hashed identities, route metadata, and
lifecycle facts; no prompt/tool/provider bodies or raw envelopes. Malformed
metadata, frame saturation, subscription failure, and disk errors cannot throw
through event callbacks. A fault prevents a passing evidence report. Eleven
JavaScript and fifteen Python regressions cover these guarantees and the
independent checker. The 100 ms route timer and synchronous metadata appends
are **fixture instrumentation**, not production scheduling/resource evidence.

This does not qualify child/inactive-tab/plugin-page selection, terminal
shutdown/superseded/inactivity payloads, missed-event recovery, replayed native
completion revisions, connection provenance, or full P0 admission. Provider
failure and user-abort payloads now have separate evidence below. Source and
configuration reload are proven only for the successful, quiescent root fixture.

## Private tagged-SDK response-shape continuation

Prepare `SDK_SOURCE_DIR` with the pinned source files as described under tagged
SDK characterization. `BUN` must now be an explicit canonical executable path
(no mutable `latest` symlink); its SHA-256 is retained as harness provenance.

```sh
python3 prototypes/opencode-v2/sdk_shapes.py --binary "$ELF" --bun "$BUN" --source-dir "$SDK_SOURCE_DIR" --output /tmp/verij-v2-sdk-shapes.json
python3 prototypes/opencode-v2/sdk_shapes_evidence.py /tmp/verij-v2-sdk-shapes.json
python3 prototypes/opencode-v2/sdk_shapes_evidence.py prototypes/opencode-v2/sdk-shapes-results.json
```

Twelve checks pass using the real private server and a **new independently owned
SDK constructor**, not the public CLI's supplied client. Only hash-pinned
`client.ts` and `client-error.ts` are copied into the disposable root; ambient
package manifests, tsconfig, dependencies, and shadow `.js` files are excluded.
Endpoint/auth arrive through bounded private stdin, never argv or retained
configuration. Observer output is byte-bounded before parsing and validated
before retention.

The constructor receives an independent GET-only bounded fetch decorator. It
preserves endpoint/auth/query fields, rejects mutations and connection changes,
and enforces the existing transport budget and 64 KiB metadata/depth/node guards
**before** handing a successful response to SDK JSON parsing. Error bodies are
cancelled rather than passed to SDK declared-error parsing. No shared client or
global fetch is modified. These are private-constructor feasibility results,
not a supported way to clone the actual standalone/authenticated CLI connection.

Observed contracts:

- `session.get` and `session.active` return unwrapped values. Active membership
  identifies the blocked fixture execution, not the successful idle root.
- Parent-filtered `session.list` traverses three children in four single-item
  pages including exhaustion. Root history returns idle markers and consistent
  ascending/descending identities in four pages per direction. These small
  stable scans do not prove concurrent topology freshness.
- `permission.request.list` and `form.list` return `{ location, data }` and
  isolate genuine outstanding fixture requests between two directories.
- **Correction:** `permission.list({ sessionID })` returns an unwrapped array of
  per-session pending requests, not permission rules. Tagged handler/source and
  runtime IDs agree. `session.form.list` and `session.inbox.list` also unwrap
  their arrays.
- The successful root still has its successful latest idle marker while a
  permission, form, and queued inbox item remain outstanding. Reads neither
  resolve/cancel them nor change root viewed state. Permission requests are
  created through the real private permission API with session-local ask rules;
  model-driven tool permission provenance remains unqualified.
- Twenty-two SDK fetch invocations produce 21 physical GETs. A preaborted SDK
  request rejects without starting a physical request. There is no implicit
  SDK paging or retry in these particular calls. In-flight cancellation on the
  actual CLI connection is not established by this run.

Seven JavaScript and fifteen Python regressions cover the decorator, in-flight
physical-slot retention under synthetic cancellation, response bounds, source
copy isolation, output bounds, privacy, and independent evidence mutations.
`SHAPES_PASS` does not grant P0 or production admission. Fixture control may
seed permissions/forms/inbox items but the SDK observer performs only GETs;
cleanup stops only recorded private process births and the controlled provider.

## Interactive terminal-failure and user-abort continuation

```sh
python3 prototypes/opencode-v2/terminal_contract.py --binary "$ELF" --output /tmp/verij-v2-terminal.json
python3 prototypes/opencode-v2/terminal_evidence.py /tmp/verij-v2-terminal.json
python3 prototypes/opencode-v2/terminal_evidence.py prototypes/opencode-v2/terminal-results.json
```

The separate real-TTY experiment passes **six checks in each of four runs**:
provider failure and user abort, each in standalone and authenticated private
`serve` + `--server` mode. Case-specific private configuration selects the
controlled model; fixed prompt text enters through the terminal. For abort,
the provider remains blocked while two Esc presses arm/confirm OpenCode's own
interrupt action. The observer performs zero SDK/cache reads or mutations.

- Typed `data.on` and envelope `listen({ details })` callbacks agree on hashed
  event/session/aggregate IDs, aggregate-local sequence/version, and projected
  terminal metadata. Started precedes one terminal event per channel; neither
  failure nor user interruption produces a success event in these workloads.
- Controlled provider HTTP 400 yields `session.execution.failed` with error
  type **`provider.invalid-request`**. Only that fixed enum value is retained;
  error message, status, and `response.body` are not read by the observer.
- User Esc interruption yields `session.execution.interrupted` with reason
  **`user`**. The tagged [event schema](https://github.com/anomalyco/opencode/blob/843a6c514e606b436a7fe713422ace0a93ad3d61/packages/schema/src/session-event.ts)
  also declares `shutdown`, `superseded`, and `inactivity`; deterministic tests
  keep those reasons distinct, but do not qualify their runtime CLI delivery.
- Every frame belongs to the verified foreground CLI birth. Returned cleanup
  runs, and only recorded private processes and controlled-provider workers
  are retired. Each retained run made two provider requests; request count is
  not equated with execution count.

The shared observer core adds an opt-in fixed terminal projection; its default
reload-experiment output is unchanged. The earlier CLI reload experiment was
rerun successfully and `cli-results.json` refreshed against current hashes.
Other retained evidence is unchanged. An initial terminal attempt exited before
plugin setup when using a model command-line flag; private case-specific model
configuration produced the passing runs. All attempts cleaned up.

Five added JavaScript and sixteen Python regressions cover private-body getter
avoidance, unknown-error collapse to fixed `other`, malformed terminal schemas,
callback fencing, frame bounds, strict evidence mutations, case configuration,
and cleanup after trace failure. The complete prototype suite passes **136
JavaScript tests and 91 Python tests**. `TERMINAL_PASS` does not qualify
terminal-to-marker reconciliation, missed events, other interruption reasons,
managed resume, family freshness, native completion revisions, or P0 admission.

## Failure/abort event-to-marker reconciliation continuation

```sh
python3 prototypes/opencode-v2/reconciliation_contract.py --binary "$ELF" --output /tmp/verij-v2-reconciliation.json
python3 prototypes/opencode-v2/reconciliation_evidence.py /tmp/verij-v2-reconciliation.json
python3 prototypes/opencode-v2/reconciliation_evidence.py prototypes/opencode-v2/reconciliation-results.json
```

The separate experiment passes **11 checks in each of two private explicit-server
runs**: provider failure and user abort. A real foreground CLI emits the initial
terminal event, then exits and disposes the observer. Fixture-owned HTTP reads
recover its marker, repeat the observation, and restart only the recorded private
server after completed execution. A second fixture-controlled turn runs with no
observer; its new marker is recovered twice without changing the retired trace.

- Tagged [`SessionMessage.ID.fromEvent`](https://github.com/anomalyco/opencode/blob/843a6c514e606b436a7fe713422ace0a93ad3d61/packages/schema/src/session-message.ts)
  replaces `evt_` with `msg_`. For both initial terminal cases, that exact
  transformation of the live event ID matches the recovered idle-marker ID.
  Raw event and marker IDs are different; hashing the event ID directly is not
  a marker key. Only bounded hashes are retained.
- Repeated reads and a completed-server restart preserve marker identity and
  outcome. The unobserved second turn has a distinct marker with the same
  failure/interrupted outcome. This is targeted missed-terminal recovery, not
  a general missed-event, concurrent-family, or active-resume proof.
- The failed marker retains error type `provider.invalid-request`; interrupted
  markers contain no `reason` field. Recovered `interrupted` alone cannot
  reconstruct live reason `user` or distinguish other interruption policies.
- The CLI observer still makes zero SDK/cache reads or mutations. Endpoint/auth
  for recovery are explicitly owned by the fixture that launched private
  `serve`, never inferred from the CLI or scraped from a shared service.
  Reads use the existing 64 KiB/depth/node-bounded fixture control, capped at
  128 GET calls; its socket timeout is **not** an independently qualified
  aggregate streaming deadline. Message heads are tiny controlled idle records.
- The retained runs use 21 and 19 recovery GETs respectively, and four provider
  requests each. Request counts are not execution counts. Both private process
  sets and controlled providers clean up. Earlier evidence remains unchanged.

Seven JavaScript and fifteen Python tests protect mapping, private-body getter
avoidance, malformed IDs, event/marker mismatch, restart/replay identity,
missing-observer evidence, fixed enums, byte/schema fences, budgets, and cleanup
on trace failure. All **143 prototype JavaScript tests**, **55 existing V1
JavaScript tests**, and **106 prototype Python tests** pass.

`RECONCILIATION_PASS` does not qualify successful-event mapping, standalone
recovery, actual CLI transport provenance, active shutdown/managed resume,
reselection/reload revision continuity, general missed-event recovery,
no-work-marker acknowledgement, family freshness, or native completion
revisions. Full P0 and production admission remain blocked.

## Exit-gate source audit

The additional offline audit pins seven complete upstream files and checks eight
facts at the plugin/SDK/host/standalone boundaries. It confirms that public
request options have no pre-parse interception hook and that standalone auth
resolves outside the plugin. It also identifies a source-level reconnect
concern: the host can replace its API while plugin construction captures the
current API value. No runtime reconnect failure is claimed.

```sh
# SOURCE_TREE is an absolute, canonical local tree containing the seven exact
# tagged source files listed in transport_source_audit.py. No code is imported.
python3 prototypes/opencode-v2/transport_source_audit.py --source-dir "$SOURCE_TREE" > /tmp/verij-v2-transport-source.json
python3 prototypes/opencode-v2/transport_source_audit.py --verify /tmp/verij-v2-transport-source.json
python3 prototypes/opencode-v2/transport_source_audit.py --verify prototypes/opencode-v2/transport-source-results.json
```

The script does not fetch sources or inspect user service state. Changed source
hashes, symlinks, FIFOs, nonregular files, oversized input, invented facts, and
attempted admission promotion are rejected. Eleven added Python tests pass;
the complete prototype Python suite now passes **117 tests**. The 143 prototype
and 55 V1 JavaScript tests remain green. `SOURCE_AUDIT_PASS` means only that the
pinned source facts were characterized; its explicit `p0_status` is `BLOCKED`.

See [the exit decision](P0_GATE_DECISION.md). The target needs an upstream
observation hook or equivalently bounded metadata API, then actual CLI transport,
identity/lifecycle, route, and fresh-family qualification. Upgrading or patching
the installed target is outside the approved plan and was not performed.

## Isolation guarantees

- Inherits nothing from the ambient shell: `PATH` is `/usr/bin:/bin`; no
  `OPENCODE_SERVER`, `OPENCODE_PASSWORD`, `OPENCODE_CLI_CONFIG_CONTENT`,
  `NODE_OPTIONS`, `BUN_OPTIONS`, `HTTP_PROXY`, `VERIJ_STATES_DIR`, or
  `XDG_CONFIG_DIRS` values are forwarded.
- Private `OPENCODE_CONFIG_DIR`, all XDG dirs, `HOME`, and `TMPDIR` point
  inside the disposable temporary directory.
- `OPENCODE_DISABLE_AUTOUPDATE=1`, `OPENCODE_DISABLE_MODELS_FETCH=1`,
  `OPENCODE_DISABLE_PROJECT_CONFIG=1`, `OPENCODE_DISABLE_CLAUDE_CODE=1`.
- Ancestor walk refuses ambient project roots (`.opencode`, `.git`, `.hg`,
  `AGENTS.md`) before starting any subprocess.
- A fresh random `OPENCODE_PASSWORD` is generated per run; it is used by the
  private server and the corresponding client but is never written to evidence.
- The smoke/marker/lifecycle/SDK-shape fixture directory contains `contract-probe.mjs`
  (copied as `tui.js`) and a minimal `package.json`. The CLI-event experiment
  replaces that entrypoint with `cli-observer.mjs`; the terminal experiment uses
  `terminal-observer.mjs`. The reconciliation experiment uses
  `reconciliation-plugin.mjs` with its fixed identity projection. These fixtures
  copy only their required observer modules alongside the entrypoint. No
  production V1/V2 bridge or ambient external plugin.
- Cleanup signals only recorded private process births by `pidfd`; never
  `killpg`, `pkill`, or service stop.
- Evidence output is validated against a strict allowlist before writing.
  Session IDs are SHA-256 truncated to 20 hex chars; raw IDs never appear.

---

## Limitations and scope exclusions

- **No production support.** P0 gate is blocked; see `CONTRACT.md`.
- Linux only (`pidfd` signalling required).
- Local `serve`+`--server` mode authenticates against a private local server
  (OPENCODE_PASSWORD supplied). Authenticated remote endpoints (URL prefix,
  auth header forwarding, path prefix handling) are **source-verified only**
  and not runtime-proven.
- `mini` (`opencode mini`) and `run` (`opencode run`) are separate entrypoints
  with different interaction models; neither is tested or monitored here.
- Desktop/browser clients and V1-to-V2 config conversion are outside scope.
- `contract-probe.mjs` is observation-only: no SDK data reads, navigation,
  inbox cancel/reply, session view, or sync calls.
- Marker contracts have partial headless-runtime evidence, including revert
  transitions and shutdown of active work. Quiescent root CLI reload has
  separate interactive evidence. Managed automatic resume, conversation
  reselection, missed events, no-work-marker acknowledgement policy,
  successful/general identity reconciliation, and fresh-family quiescence remain unqualified;
  see `CONTRACT.md`.
- Unit-test results are not substitutes for the retained live smoke or future
  execution/resource acceptance.
- Do not point V1 monitoring at V2 storage.
