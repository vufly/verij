# Experimental focus completion and query contract

This extends the isolated Zellij v0.45.1 bridge for H0 investigation. It is not a released protocol, stock Zellij capability, Verij monitoring feature, or H0 approval.

## Wire and lifecycle

The existing opt-in attachment identity exchange and legacy `VerijFocusPane` tag 29 remain available for historical reproductions. New requests use client protobuf tag **30**, `VerijPaneRequestMsg`; responses use server tag **21**, `VerijPaneResultMsg`. Source schemas, checked-in Prost bindings, native IPC conversion, local client and web-listener exhaustiveness are included in the saved patch.

Each one-shot control connection sends one request and stays open for its asynchronous response. The response echoes the entire request and server PID, alongside status and optional observed pane/tab identity. The helper validates correlation and enforces a five-second result deadline, then closes its socket. Route EOF owns control-connection removal. Returning from the route immediately after queueing would remove the connection before the screen result; an initial failed probe caught and corrected that lifecycle error.

Requests contain:

| Field | Meaning |
|---|---|
| `request_id` | Nonempty caller correlation ID, at most 128 bytes. |
| `target_client_id`, `connection_id` | Verified attached display connection and its generation. |
| `pane_id` | Exact terminal namespace target; zero is valid. |
| `sequence` | Positive, strictly increasing navigation sequence within one display generation. |
| `query_only` | Read-only effective-focus observation; queries require sequence zero and do not supersede navigation. |

The eventual host controller must coordinate/persist its sequence across one attachment lifetime; separate producers must not independently restart at one. A new display generation resets ordering. This prototype does not yet define controller restart recovery or a public CLI API.

## Execution and delivery

1. The server validates the generation and reserves a newer navigation sequence. Older or duplicate sequences fail without queueing focus.
2. The screen thread validates the requester connection generation, target display generation, and latest reserved sequence against the live shared server registry at execution start. It releases the registry lock before native focus: cross-tab focus may send to the bounded server queue, whose consumer can need a registry write lock. Native focus and its immediate observation are serialized on the screen thread. A newer reservation arriving after execution starts cannot undo an in-flight mutation; delivery revalidation prevents that older result from reporting current success.
3. Focus locates the target's current tab and uses native layer/stack handling. A query never executes focus. The observed pane comes from the active tab's current visible layer, not an inventory `is_focused` flag in an inactive tab or hidden layer. Results include stable server-local `tab_id`, not tab position; absent pane/tab IDs and zero remain distinct.
4. The server revalidates display generation and navigation sequence before delivery. An older completion is downgraded if already superseded. Requester generations prevent executing or delivering a pending result to a recycled numeric control-client ID. Disconnected requesters fail execution validation; helpers receive no later response after their own socket has closed.

Screen results are point-in-time inner-client observations. Keyboard input, shared-layout changes, or later navigation may supersede them afterward. A production host must validate the current request, target instance, and fresh whole-host effective focus before acknowledging a completion revision. This endpoint does not prove host Workspace keyboard focus, render consumption, or an agent visit.

## Statuses

| Status | Interpretation |
|---|---|
| `focused` | Screen observed requested terminal as that client's active visible-layer pane. |
| `not_focused` | Target exists, but another pane is effective; expected for a passive query of another target. |
| `pane_missing` | Requested terminal does not exist at execution; observation can still report prior focus. |
| `stale_attachment` | Display generation is no longer current. |
| `superseded` | Navigation sequence is older/duplicate or a newer reservation replaced it before execution/delivery. |
| `invalid_request` | Invalid correlation ID or query/navigation sequence shape. |
| `unavailable` | Screen dispatch or connected display state unavailable. |
| `focus_failed` | Native focus returned an error; never interpreted as success. |
| `cancelled` | Requester connection generation no longer exists at execution; normally no receiver remains. |

Unknown statuses, malformed correlation, closed IPC, or deadline expiration are failures, not acknowledgements. Results can preserve observed pane metadata on an execution failure; clients must inspect status rather than pane equality alone.

## Verified on 2026-10-01

- Two real protobuf roundtrip tests: legacy identity/focus and new request/result, including u64 sequence and optional zero IDs.
- Three server policy tests: display ClientId reuse and web/CLI exclusion; superseded queued-policy requests and ordering reset; requester cancellation/reuse exclusion.
- `cargo build -p zellij --no-default-features` and web-capability compile check passed.
- Direct `--completion --placements --lifecycle` passed with per-client inventory and actual keyboard tokens. Typed missing-target failure, passive queries, older/duplicate rejection, cross-tab, rename, abrupt-death stale rejection and zero-client reattachment are recorded in [direct results](completion-direct-results.json).
- Real nested tmux `--completion` passed tiled, hidden floating, fullscreen, ordinary stack, passive queries, missing targets and reconnect/stale-token behavior, with normal keys traversing the outer host terminal. See [nested tmux results](completion-tmux-results.json).
- User-requested Magy default-model review `watch_e1a29f93062b41de` completed with exit 0 and found no must-fix defects within its inspected request/route/execution/delivery scope. Report remains at workspace `artifacts/magy-default-completion-review.md`. This read-only review is not human H0 approval or runtime race evidence.

The deterministic policy tests prove validation against queued generations/sequences; they do not substitute for an actual paused-screen disconnect/supersession race reproduction. That reproduction, hidden stack-list targets, cross-session switching, controller restart recovery, plugin reload/resurrection, mirroring and human whole-host/sidebar visits remain H0 work. Native stack expansion remains shared between clients even with mirroring disabled. The flaky synthetic-PTY nested startup path remains under investigation; successful real tmux completion probes do not erase its failures.
