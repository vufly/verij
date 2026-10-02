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

The host controller must coordinate/persist its sequence across one attachment lifetime; separate producers must not independently restart at one. A new display generation resets ordering. The experimental [`controller.py`](controller.py) now demonstrates restart recovery; it is not a released Verij CLI API.

Results include optional uint64 **`latest_sequence`** (result field 7), sampled from the live server reservation registry at delivery. A current display generation with no reservation reports **Some(0)**; a stale generation reports absence. This is a reservation watermark, not proof that a navigation executed, rendered or was acknowledged. A passive query returns it without changing focus or ordering. Helpers requiring recovery refuse older peers that omit the field.

## Controller restart recovery — 2026-10-02

All cooperating producers use one private journal directory. The journal key hashes server `(boot_id, PID, start_jiffies)`, display ClientId and connection generation, so session rename retains ordering while display replacement gets a distinct journal. The supplied binding also contains captured client process birth identity; both process identities must still be live before use.

1. Acquire the generation's advisory lock on a stable lock inode. Keep it through query, reservation, send and result; process death releases it. Acquisition has a five-second deadline.
2. Issue a passive query and require a current generation plus a present, valid uint64 watermark. Select `max(server watermark, durable journal reservation) + 1`.
3. Atomically replace the mode-0600 journal, fsync its contents and containing directory **before sending**. Failed sends, timeouts, cancelled requests and lost results consume their reservation; no automatic replay occurs.
4. A missing journal recovers from the live server watermark. Malformed or mismatched journals and uint64 exhaustion fail without focus. Producers using separate journal directories are not coordinated by this helper; intervening external reservations can yield `superseded`, which remains failure.

Live probes use separate controller subprocesses, not a reset Python counter. They kill controllers after durable reservation but before send, while actually queued on the paused screen, and after server completion but before printing the result. Restart selects a new sequence; queued abandoned requests are cancelled, and completed requests are not replayed. Two concurrent producers sharing the lock receive distinct consecutive sequences. The same directory is reused after display-generation replacement without inheriting the former generation's ordering. See [race/recovery results](races-recovery-results.json).

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

## Live paused-screen verification — 2026-10-02

The debug binary accepts an explicit disposable `VERIJ_ZELLIJ_PROBE_DIR`. Request-specific hold files pause screen processing before execution validation or after focus mutation but before completion delivery. Atomic event files establish reservation, entry, execution, requester removal and delivery ordering; neither sleeps alone nor successful enqueue replies are used as race evidence. Registry locks are not held across barriers. Barriers have a 15-second deadline, and the verifier refuses a PASS if any barrier expired. Release builds ignore these barriers.

Five live scenarios passed with actual display-PTY keyboard tokens:

- A newer queued reservation supersedes an older request before execution; the older request does not focus.
- A request already mutated focus when a newer reservation arrives; delivery downgrades its old success to `superseded`, and the newer target receives input. This does not promise undo of an in-flight mutation.
- A requester disconnects and its numeric control ID is reused before execution; the abandoned request is cancelled, and the replacement receives only its own correlated query result.
- A display disconnects and its numeric display ID is reused before old execution; validation uses the live registry despite screen RemoveClient/AddClient still being queued.
- A display disconnects and its numeric ID is reused after old focus mutation but before delivery; delivery returns `stale_attachment` with no replacement-generation watermark.

Final target run: scratch `vj-bridge-oaz1pqhk`, [retained results](races-recovery-results.json). Updated-wire real nested tmux regression passed (`vj-mux-pqk_9ljf`), and the separate direct placement+lifecycle regression passed (`vj-bridge-a_h9liaa`). An expanded races+recovery+placements+lifecycle attempt (`vj-bridge-4fybe18i`) passed races and same-generation recovery, then timed out during a legacy stale request after reconnect; its FAIL report remains in workspace `artifacts/races-recovery-attempt-2.json`. The complete combined matrix is not claimed reliable, and no cause is established by that timeout.

### Later cleanup and visibility continuation — 2026-10-02

Rapid legacy/query socket turnover reproduced delayed route cleanup removing a newly allocated numeric control ID before its query dispatched. Legacy control now stays open until helper EOF, and route EOF/ClientExited removal carries its originating generation; stale removals are skipped at processing. A sixth live race explicitly pauses route end after ClientExited removes a control connection, reuses the ID, then releases old cleanup and verifies the new query/keyboard path survives. The [full combined run](continuation-combined-results.json) passes this race, 32 turnover iterations, recovery, placements and lifecycle. Old timeouts remain retained; the concrete cleanup fix does not claim a universal cause for all historical failures.

Direct and full nested title-list placement+lifecycle checks now pass, as does a mirrored completion/placement/reconnect run. Hidden stack-list members are suppressed but can retain nonzero remembered geometry; effective query and reveal/input establish visibility, not inventory size. Probe config now selects classic stacks explicitly unless `--stack-list` is supplied. Native mirrored focus and native stack expansion remain shared; this does not acknowledge another host's completion. See [continuation summary](continuation-summary.json) for exact evidence.

### Native lifetime continuation — 2026-10-02

[The lifetime probe](verify_lifetimes.py) now verifies display-owned native cross-session switching and return, per-client WASM reload and native cached-layout resurrection. See [results](lifetimes-results.json). Switching invalidates the old connection token despite stable client process birth; returning to the same live server creates another generation. Plugin reload preserves server birth, attachment generations and reservation watermark, with actual new per-client load markers. Resurrection reuses a session name/layout but has a new server birth, fresh generation and zero watermark; old tokens fail.

The request/result endpoint still focuses within one verified server/display attachment. Native switching here uses a private keybinding, not a correlated control-switch API. Production cross-session binding/completion, Verij plugin re-registration/store integration and human whole-host/sidebar visits remain H0 work. These scoped passes are not human H0 approval.
