# Navigation Reliability Plan

Status: **Phase 1 implemented; remaining phases proposed**.

Related documents: [human recovery guide](NAVIGATION_RECOVERY.md), [agent monitoring design](AGENT_MONITORING.md), and [agent monitoring implementation guide](AGENT_MONITORING_IMPLEMENTATION.md).

## 1. Problem and Scope

The error `outer native focus observation unavailable` means that Verij confirmed inner terminal focus but could not confirm focus on the outer Workspace pane. It does not establish that a race occurred. The current message hides the returned status and expected/observed pane IDs.

Navigation can involve session switching, host rebinding, inner focus, outer focus, and final verification. These steps are asynchronous and are not one atomic Zellij operation. Competing requests, manual focus changes, stale bindings, target closure, and delayed observations require different handling.

This plan improves Verij on stock Zellij. Preserve native placement and multi-client limitations, verified process/client ownership, and existing completion-acknowledgement contracts. Focus confirmation alone must not acknowledge an agent's Done revision.

## 2. Current Implementation

| File | Existing behavior | Improvement area |
|---|---|---|
| `verij-cli/src/activation.rs` | A worker serializes local activations, drains pending requests to the latest intent, and suppresses obsolete results using tickets. | Extend existing coordination rather than adding a second independent queue; address other producers targeting the same host. |
| `verij-cli/src/navigation.rs` | Validates ownership, switches/rebinds, focuses inner then outer panes, checks current intent at several boundaries, and uses separate request journals/locks. | Coordinate the full operation, check cancellation around waits and dispatches, retain failure details, and verify the final combined state. |
| `verij-plugin/src/control.rs` | Filters recipient context, rejects old sequences, checks pane existence, and polls focus at 200 ms while a request is pending. | Preserve correlated observations and explicit failure status; verify late-result and supersession behavior. |
| `verij-types/src/control.rs` | Defines statuses, observations, process-qualified bindings, and request correlation. | Add compatible diagnostic/outcome types only where needed. |

The plugin already polls for approximately five seconds. Increasing timeouts or inserting fixed sleeps is not the primary solution.

## 3. Required Behavior

1. The newest local intent controls subsequent navigation work and visible results.
2. Each stage validates the current intent and live target/ownership before changing focus.
3. Superseded work stops without retries, obsolete UI updates, or completion acknowledgements.
4. Partial progress is reported accurately: inner focus may succeed while outer focus remains unverified.
5. Recovery is bounded and does not blindly replay an uncertain action.
6. Manual focus changes are treated as possible user intent, not automatically fought by retries.
7. Complete navigation success requires fresh observations of the expected inner target and outer Workspace pane.

Stock Zellij actions already dispatched cannot be revoked. Cancellation checks reduce obsolete dispatches but cannot eliminate the interval between a check and native execution. This plan must not advertise atomic navigation or guarantee that an old dispatched action never briefly changes focus.

## 4. Implementation Phases

### Phase 1 — Preserve Diagnostics and Partial Outcomes

- [x] Replace the generic outer-focus error with a structured navigation failure containing stage, failure kind, underlying control status, and partial progress.
- [x] Correlate stage-level request IDs with one navigation intent ID.
- [x] Include host, expected/observed terminal IDs, relevant session/context identity, observation timestamp, and elapsed time in diagnostic records.
- [x] Distinguish explicit control failure from missing observation, observation mismatch, dispatch failure, and timeout.
- [x] Retain valid correlated failure statuses even when an observation is absent; do not weaken freshness or identity validation for accepting success.
- [x] Present a short actionable UI message, with full detail in diagnostics.

Example UI message:

> Inner terminal selected; Workspace focus not confirmed. Expected outer pane 12, observed pane 7.

**Acceptance:** a reproduction identifies the failed stage and underlying status without inferring a race from the generic message. Missing evidence remains unverified rather than successful.

#### Phase 1 Verification Summary

Phase 1 implementation has been completed and verified using deterministic test suites (189 CLI unit tests, 2 AGY setup integration tests, 15 host CLI integration tests, and 22 verij-types tests all passing; total 228 tests). No live Zellij reproduction was performed during Phase 1 (deterministic unit/integration tests and fixture testing only; live reproductions remain part of Phase 5). Future phases (Phases 2–5) remain pending.

Verification commands:
- `cargo test -p verij-cli -p verij-types`
- `make check`
- `git diff --check`

### Phase 2 — Coordinate the Whole Operation

- [ ] Audit sidebar agent navigation, legacy session/tab navigation, and CLI entry points for producers targeting the same host.
- [ ] Extend the existing activation worker so the coordination boundary covers switch, rebind, inner focus, outer focus, and verification.
- [ ] Keep at most one active local operation and replace pending local work with the latest intent.
- [ ] Carry the intent generation through every stage, recovery attempt, UI update, and acknowledgement path.
- [ ] Check current intent before and after blocking waits, after acquiring relevant locks, immediately before mutating dispatches, and before publishing success.
- [ ] Make result waits cancellation-aware so obsolete work does not consume its entire timeout before yielding to the newest request.
- [ ] Define per-host cross-process coordination for CLI and sidebar producers using the existing stable lock/journal conventions. Local atomic tickets alone do not coordinate separate processes.

Keep passive observations possible where safe. Avoid nested lock ordering that deadlocks switch/rebind or request processing. Releasing a wait on cancellation does not revoke a dispatched action; retain correlation so late results cannot satisfy a newer intent.

**Acceptance:** deterministic A-then-B tests show that A performs no further work after observing supersession, emits no current UI result, and never retries or acknowledges completion. B proceeds without waiting for A's full observation deadline.

### Phase 3 — Add Bounded, Status-Specific Recovery

| Outcome | Recovery policy |
|---|---|
| Expected pane freshly observed despite earlier timeout | Accept that stage after validating request context, ownership, freshness, and current intent. |
| Transient observation failure | Query fresh state first; permit at most one focus retry only when ownership is valid, intent is current, and there is no evidence of competing/manual focus intent. |
| Superseded | Stop quietly; do not retry. |
| Pane missing or target process exited | Refresh inventory and report target unavailable; do not substitute another agent/pane. |
| Stale context or binding | Refresh/re-register through the verified ownership path; continue only if the same target lifetime remains valid. |
| Ambiguous ownership or multiple outer displays | Report the unsupported/ambiguous condition and require explicit recovery. |
| Persistent unavailable or unconfirmed focus | Report partial progress and offer manual Workspace selection or an explicit retry. |

- [ ] Use one monotonic end-to-end deadline so nested waits and recovery cannot multiply latency without bound.
- [ ] Reuse existing per-stage timing as a baseline and record elapsed time before choosing deadline values.
- [ ] Never retry an older intent, blindly replay an uncertain session switch, or treat a newer request's result as confirmation of an older one.
- [ ] When competing/manual intent cannot be distinguished, prefer an explicit user retry over automatic refocusing.

**Acceptance:** recovery has a bounded action count and duration; late confirmation can resolve an already-completed stage without dispatching another focus action.

### Phase 4 — Verify Combined State and Improve UI

- [ ] After outer focus confirmation, obtain a fresh inner target observation bracketed by outer observations.
- [ ] Adapt the approach in `verified_visit()` with checks for the exact expected `PaneKey`, current binding/context, live process identities, observation freshness, and current intent.
- [ ] Recheck attachment and binding lifetime before publishing a verified navigation outcome.
- [ ] Represent pending, verified, partially completed, superseded, and failed outcomes distinctly.
- [ ] Show the pending target and clear pending state when the current operation finishes or is cancelled.
- [ ] Keep navigation confirmation separate from agent completion acknowledgement; preserve the existing independent visit/revision checks.

These are bounded point-in-time observations, not a promise that focus remains unchanged afterward. Manual movement after verified completion is normal.

**Acceptance:** inner focus alone does not produce complete success; a final mismatch produces partial/unverified status; stale results cannot update the current selection or completion revision.

### Phase 5 — Validate and Document

- [ ] Add deterministic coordination tests using barriers or controlled event scheduling rather than timing-dependent sleeps.
- [ ] Extend stock-Zellij live reproductions for focus placement, lifecycle changes, and real keyboard/mouse interference.
- [ ] Update the human recovery guide with actual implemented UI messages and recovery commands.
- [ ] Record fixture versions, outcomes, and unresolved stock limitations with implementation progress.

## 5. Verification Matrix

| Scenario | Required result |
|---|---|
| A is superseded by B before outer dispatch | A stops its remaining work; B becomes the current navigation. |
| B arrives after A has dispatched a native action | A's late result is isolated; B proceeds; no claim of revoking A's action. |
| User changes outer focus during verification | No fabricated success and no automatic retry that knowingly overrides user intent. |
| Target closes or pane ID is reused | Reject the old target lifetime; do not focus a replacement by numeric ID alone. |
| Plugin reloads or context becomes stale | Preserve explicit stale failure and use verified rebinding before further mutations. |
| Focus succeeds but result is delayed/lost | Fresh query may confirm the stage; old result never completes a new request. |
| Session switch succeeds but later focus fails | Report partial progress; do not blindly replay the switch. |
| Two processes navigate the same host | Defined host-scoped ordering; no conflicting journals or obsolete completion updates. |
| A second outer display attaches | Clear single-display limitation; no arbitrary display routing. |
| Hidden floating, fullscreen, stacked, or cross-tab target | Verify effective focus using supported native observation; document stock limitations. |
| Manual focus changes after success | Normal focus change; no persistent refocus loop. |

For implementation changes, run the relevant Rust tests, `cargo test -p verij-cli -p verij-types`, and `make check`. Build the WASM plugin for live cases and retain reproduction evidence. These checks are implementation acceptance requirements, not claims that this documentation change ran them.

## 6. Delivery Order and Completion Criteria

Deliver diagnostics first, then whole-operation coordination, bounded recovery, combined verification/UI, and the live validation matrix. Each phase should produce independently reviewable evidence.

The work is complete when failures identify their cause and stage, superseded work cannot publish current outcomes or acknowledgements, recovery is bounded, final success has fresh combined-state evidence, and the recovery guide matches shipped behavior. Human pacing remains a useful workaround, not a correctness prerequisite.
