# OpenCode V2 P0 contract table

**Gate: BLOCKED.** Retained evidence separates [CLI loading](smoke-results.json),
[synthetic tagged-SDK characterization](sdk-results.json),
[private headless execution contracts](marker-results.json),
[revert/active-shutdown contracts](lifecycle-results.json), and
[interactive CLI event/reload contracts](cli-results.json), and
[private tagged-SDK response shapes](sdk-shapes-results.json), and
[interactive failure/user-abort payloads](terminal-results.json), and
[private failure/abort event-to-marker reconciliation](reconciliation-results.json). None authorizes
native V2 admission, production adapter activation, installation, or rollout.

The additional [pinned source audit](transport-source-results.json) confirms the
transport boundary. [The exit decision](P0_GATE_DECISION.md) records why P0
cannot pass on the current target and which qualifications remain outstanding.

Qualification target: **2.0.32**, upstream commit
`843a6c514e606b436a7fe713422ace0a93ad3d61`. The real executable and its shell
launcher have separate SHA-256 pins in [pin.json](pin.json). Source observations
below are release-pinned, not claims about other releases.

## Source observations versus runtime evidence

| Contract | Release-pinned source observation | Retained runtime proof / remaining gap |
|---|---|---|
| CLI plugin | [`Plugin.define`](https://github.com/anomalyco/opencode/blob/843a6c514e606b436a7fe713422ace0a93ad3d61/packages/plugin/src/tui/plugin.ts) accepts `{ id, setup }`; setup may return cleanup. | Import, options/version, typed/listen subscriptions, and returned cleanup pass in both local modes. Interactive options reload, source-save reload, ordered disable/re-enable, and final cleanup pass on a quiescent root. Disabled observer does not affect the next prompt's execution. |
| Import target | [CLI loader](https://github.com/anomalyco/opencode/blob/843a6c514e606b436a7fe713422ace0a93ad3d61/packages/tui/src/plugin/context.tsx) skips direct file entries. [`Host.resolve`](https://github.com/anomalyco/opencode/blob/843a6c514e606b436a7fe713422ace0a93ad3d61/packages/plugin/src/host.ts) resolves a directory/package's `tui` subpath. | A private directory containing `tui.js` and a minimal `package.json` loads. Do not reuse a V1 direct-file installer assumption. |
| Selected route | [`Context.ui.router.current()`](https://github.com/anomalyco/opencode/blob/843a6c514e606b436a7fe713422ace0a93ad3d61/packages/plugin/src/tui/context.ts) returns Home, session, or plugin route. | Home has no selected session; terminal prompt submission selects one root. That selected identity survives options/source reload and disable/re-enable in both modes. Child/reselection, inactive tabs, plugin routes, and native pane/exporter binding remain untested. |
| Events | `data.on(type, handler)` receives typed events; `data.listen(handler)` receives `{ details }`. Both return unsubscribe functions. | Real CLI started/succeeded callbacks have matching typed `data` and `listen({ details })` event IDs, aggregate IDs, aggregate-local sequence and version. Both channels remain ordered within the root execution and agree across reload. Separate real-TTY runs in both modes prove matching failed/interrupted payloads: provider HTTP 400 yields `provider.invalid-request`; user Esc interruption yields reason `user`. No success event occurs in either workload. Private explicit-server failure/abort reconciliation now matches derived live event identities to recovered markers, including completed restart and one terminal missed after observer disposal. Global ordering, gap-free delivery, shutdown/superseded/inactivity delivery, and general missed-event recovery remain unqualified. |
| Client shapes | [Promise client](https://github.com/anomalyco/opencode/blob/843a6c514e606b436a7fe713422ace0a93ad3d61/packages/client/src/promise/generated/client.ts) unwraps `session.get` and `session.active`; `session.list` accepts `parentID`, pagination, and ordering parameters. | A hash-pinned tagged SDK on an explicitly provisioned private connection returns unwrapped get/active values and parent-filtered pages. Exhaustive tiny message pages agree across traversal orders. The bounded constructor decorator accounts for 22 SDK fetch invocations and 21 physical GETs; a preaborted invocation performs no physical read. CLI cache side effects, concurrent pagination consistency, and in-flight cancellation on the actual CLI connection remain unqualified. |
| Messages | `message.list` calls `GET /api/session/{sessionID}/message` and returns a data/cursor page. [`MessageListInput.type`](https://github.com/anomalyco/opencode/blob/843a6c514e606b436a7fe713422ace0a93ad3d61/packages/client/src/promise/generated/types.ts) accepts one type, excluding `idle`. | Private unfiltered pages return idle markers; ascending and descending cursor traversals agree. Follow `cursor.next` without `order`: tagged handler rejects that combination. Do not invent an idle or multi-type filter. |
| Pending input | `permission.request.list` and `form.list` return location-scoped `{ location, data }` values. **Correction:** tagged `permission.list({ sessionID })` unwraps per-session pending requests, not rules. `session.form.list` and `session.inbox.list` also unwrap per-session arrays. | Private tagged-SDK evidence proves genuine pending permissions and forms in two separate directories, per-session identity agreement, and pending inbox recovery. Reads preserve all work and root viewed state. Successful root outcome plus latest idle success coexists with all three pending input kinds. Complete fresh-family recovery remains unproven. |
| Cache | Public data API exposes `session.root`, `family`, `status`, and `sync`, with separate message/input collections. | No cache synchronization performed. Emptiness, getter side effects, implicit fetching, pagination, and freshness are not qualified. |
| Connection mode | [`--server`](https://github.com/anomalyco/opencode/blob/843a6c514e606b436a7fe713422ace0a93ad3d61/packages/cli/src/services/server-connection.ts) uses explicit endpoint/password. [`--standalone`](https://github.com/anomalyco/opencode/blob/843a6c514e606b436a7fe713422ace0a93ad3d61/packages/cli/src/services/standalone.ts) launches a private child with its own credential and stdin lease. | Both local modes load and dispose the plugin. Private `serve` listener ownership and foreground client process births are checked. Authenticated remote/path-prefix support remains unqualified. |

## Blocker 1: supported bounded transport

The tagged Promise client's public constructor accepts `baseUrl`, `fetch`, and
`headers`. Its per-request options expose only `signal`, `headers`, and
`onActivity`; there is no V1-style `requestValidator`.

The generated implementation uses:

- Success: `response.text()` followed by `JSON.parse`.
- Declared errors: the same parser, followed by conversion of the parsed body
  into an error retaining body fields.
- SSE: a decoded-buffer length guard named `maxSseEventBytes` with value
  `16 * 1024 * 1024`, not Verij's JSON allocation limits.

The normal JSON/error paths contain no Verij-equivalent pre-parse byte, depth,
or structural-node limits. A result/page limit or projection after resolution
cannot bound allocations that already occurred.

The seven-file `transport_source_audit.py` pins the complete reviewed sources
and checks eight boundary facts without importing upstream code. The host's
reconnect callback can assign a new API; plugin construction assigns the
current `host.client.api` value. The data cache receives a dynamic getter and
host connection separately. This is an additional source-level generation
concern, not demonstrated runtime reconnect behavior or a replacement for
actual connection-generation qualification.

The public CLI context supplies an already-constructed client, but does not
expose the connected endpoint, authenticated fetch factory, or constructor
inputs. The [context implementation](https://github.com/anomalyco/opencode/blob/843a6c514e606b436a7fe713422ace0a93ad3d61/packages/tui/src/plugin/api.tsx)
projects `host.client.api`, not the host's connection provenance.

**Required dependency/decision:** the [upstream transport requirement](UPSTREAM_TRANSPORT_REQUIREMENT.md)
records a proposed hook and acceptance matrix, not an existing API. Establish
a supported independently bounded observation transport for the actual selected connection. An upstream plugin
transport hook, supported connection factory, or appropriately bounded metadata
endpoint may be necessary. No suitable production CLI path is established here. Explicit
endpoint/auth provisioning is acceptable only after supported provenance and
both connection modes are demonstrated; a harness-owned server password does
not establish production access to standalone or remote credentials.

Do not infer defaults, scrape credentials, mutate the supplied client/global
fetch, or proxy normal agent execution. Preserve the 64 KiB outcome and 1 MiB
general byte ceilings, depth 32, and 16,384-node ceiling before parsing,
including error responses.

`bounded-transport.mjs` implements a prototype GET-only path with explicit
in-memory endpoint/auth, prefix-preserving query construction, error-body
cancellation, pre-parse success bounds, fatal UTF-8 decoding, three-second
aggregate deadlines, and the unchanged V1 request-budget primitive. Review
regressions cover late/uncooperative fetches and cancellation promises: callers
may time out, but physical slots stay occupied until actual teardown settles.
Adversarial tests exercise oversized/chunked/stalled responses, prefixes/auth,
and redirect rejection. `transport-check.mjs` recovers the same marker through
this transport on an explicitly provisioned private OpenCode server.

The separate `sdk-observation.mjs` decorator supplies a GET-only owned `fetch`
to the hash-pinned SDK constructor. It preserves the explicitly provisioned
endpoint/auth, rejects method/header/authority changes, and returns a success
`Response` only after the same 64 KiB pre-parse guards finish. Upstream HTTP
errors reject without SDK error-body parsing. The real private SDK shape run
accounts for all 21 physical requests and a preaborted no-request invocation;
deterministic tests additionally cover in-flight abort, retained physical slots,
byte/depth/node failures, deadlines, and disposal. This demonstrates feasibility
for an **independently constructed private client**, not access to the public
CLI context's actual endpoint/authenticated transport.

These results do **not** provide standalone plugin connection provenance,
qualify authenticated remote servers, or demonstrate large-body behavior on
both actual CLI connection modes. The tagged-SDK characterization separately
confirms its normal parser accepts over-limit success/error bodies and
structural JSON before returning to the caller; it uses a synthetic owned
fetch, not a real CLI connection.

## Blocker 2: stable terminal identity

The [idle-marker schema](https://github.com/anomalyco/opencode/blob/843a6c514e606b436a7fe713422ace0a93ad3d61/packages/schema/src/session-message.ts)
defines a turn boundary containing all work since the previous marker,
including steered prompts. Shutdown does not write a marker because resume
continues the same turn. Outcomes are `succeeded`, `failed`, and `interrupted`.

The proposed identity remains `v2/root-session-ID/idle-marker-ID`. The private
controlled-provider experiment now proves:

- Repeated reads and a restart **after completed execution** preserve marker IDs.
- A new execution after idle produces a different marker.
- Terminal provider failure produces `failed`; user abort produces `interrupted`.
- Two queued prompts, and separately two steered prompts, share **one** idle
  marker in the observed workload. Do not assume one prompt or queue item per
  completion identity.
- Forking copies/remints historical successful markers, but the fork's root
  outcome is absent. Historical success must not manufacture Done in the fork.

The separate lifecycle experiment adds these private-runtime observations:

- Staging a revert preserves history. Clearing it creates a new successful idle
  marker **without new input or a provider request**, preserving older message
  identities. Tagged [`Session.revert.clear`](https://github.com/anomalyco/opencode/blob/843a6c514e606b436a7fe713422ace0a93ad3d61/packages/core/src/session/session.ts)
  calls `execution.wake`. A marker-only ledger needs an explicit no-work policy:
  stable identity does not by itself establish new model work to acknowledge.
- Committing the second-turn revert removes its messages and later markers;
  the first turn's historical marker becomes newest again, unchanged. The
  session outcome remains `succeeded`. Replacement input creates a fresh marker.
  Do not mint a new revision for resurfaced historical success.
- During active work, session outcome and the latest historical marker remain
  `succeeded` in the fixture. Graceful shutdown creates no new idle marker;
  an incomplete assistant entry may appear during teardown. Restarted private
  unregistered `serve` exposes no active membership but preserves the unfinished
  history and historical success. Empty `active` plus session outcome is not a
  complete current-terminal proof.

The [tagged restart service](https://github.com/anomalyco/opencode/blob/843a6c514e606b436a7fe713422ace0a93ad3d61/packages/core/src/session/execution/restart.ts)
explicitly distinguishes managed-server startup recovery from unregistered
servers. This fixture does not invoke or qualify managed automatic resume.

The separate interactive [terminal experiment](terminal-results.json) proves
provider failure and user-abort event delivery in both private connection modes.
Typed and envelope callbacks agree on durable identities and aggregate-local
started-before-terminal sequence. Fixed enum projection retains error type
`provider.invalid-request` or interruption reason `user`, never error text or
provider bodies. These CLI runs make no SDK/cache reads and do not link their
events to recovered idle markers. Shutdown/superseded/inactivity remain
source-observed reasons, not runtime-qualified CLI payloads.

The separate [reconciliation experiment](reconciliation-results.json) adds two
private explicit-server runs with 11 checks each. Tagged
[`SessionMessage.ID.fromEvent`](https://github.com/anomalyco/opencode/blob/843a6c514e606b436a7fe713422ace0a93ad3d61/packages/schema/src/session-message.ts)
replaces `evt_` with `msg_`. Runtime failure and user-abort events produce exactly
the recovered marker ID under that transformation. Direct event-ID hashes are
not marker hashes. Repeated reads and a completed-server restart retain the
same marker; a second fixture-controlled turn while the observer is absent
produces a distinct, repeatedly recoverable marker without new observer frames.
Failed markers retain `provider.invalid-request`; interrupted markers omit the
reason, so history alone cannot reconstruct `user` versus other interruption
policies. No success event or active resume is tested by this experiment.

Recovery uses explicitly provisioned fixture credentials and the existing
bounded fixture control, not the public CLI client's transport. Tiny idle
message heads and a 128-GET cap do not qualify general history reads, aggregate
streaming deadlines, family freshness, or standalone provenance. Seven
JavaScript and fifteen Python regressions enforce mapping, privacy, evidence
integrity, and cleanup. Earlier retained evidence is unchanged.

Both headless experiments cap every response at 64 KiB and history at 16 two-item pages;
these are small controlled histories, not proof of scalable recovery. Managed
shutdown/resume, general missed events, standalone recovery, CLI reselection,
no-work-marker policy, successful-event mapping, and general reconciliation
with live activity/error identity remain unqualified. An
event ID, timestamp, cached idle status, or session outcome is not a substitute.
Never perform unbounded full-history synchronization to find a marker.

## Blocker 3: complete-family freshness

Neither `family()` nor completed cache `sync()` is proof of fresh complete
membership. Parent-filtered pagination needs identity/cycle checks, bounded
resumable discovery, freshness brackets, and topology invalidation. The private
experiment creates a new newest child after reading the first descending page:
the older cursor misses it, while a fresh head sees it. Cursor exhaustion is
therefore not proof of complete-family freshness. Missed-event recovery and a
stable bounded freshness bracket remain unqualified.

Successful root outcome alone cannot establish Done. Fresh quiescence must
cover the complete verified family and all permission/form/inbox work. Missing
proof yields Unknown. The headless fixture creates real execution turns, but
no native completion records or acknowledgement revisions. The CLI-loading
smoke still creates no execution turns.

## Scope of the passing smoke

The probe makes **zero SDK data reads** and performs no navigation, replies,
inbox cancellation, session viewing, or cache synchronization. Terminal bytes
are discarded; evidence contains only allowlisted shapes, route metadata,
process facts, and lifecycle frames. Only recorded private process births are
signalled during teardown.

This is not P4 pane ownership/host-local acknowledgement acceptance or P5
resource qualification. Mini, noninteractive `run`, desktop/browser clients,
V1 configuration conversion, shared-server multi-client isolation, remote
endpoints, and production monitoring remain outside this result.
