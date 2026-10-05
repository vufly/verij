# Agent monitoring: remaining gates and ownership

Assessment updated: **2026-10-05**. **H0–H2 are approved; H2 commit and push are requested.** The [approved product design](AGENT_MONITORING.md) and [implementation checkpoints](AGENT_MONITORING_IMPLEMENTATION.md#101-human-verification-checkpoints) remain authoritative.

## Recorded reviewer decisions

The user answered the three dependency/capability decisions:

1. **No Zellij patches, fork maintenance, build or distribution effort.** Use stock Zellij and tolerate documented Verij limitations.
2. **Native shared-stack behavior is accepted.** Host-local acknowledgement still stays independent.
3. **Documented Agy limits are accepted.** Do not resume vendor/interface work just to remove these accepted gaps; use verified sources and conservative outcome/capability handling.

These choices supersede the earlier patched-dependency proposal and resolve the corresponding requests for input below. The existing fork/patch remains historical evidence, not an active dependency. They do not imply production implementation or completion of H1 checks.

After the stock-only two-host demo handoff, the user replied **“approve”** and then requested **“You should commit H0 first”**. This approves the selected H0 contract within its documented scope and limits. No additional per-scenario human observations are inferred from that feedback. The reviewed private instance has been cleaned; see the [approval record](AGENT_MONITORING_PROGRESS.md#h0-approval-and-review-cleanup--2026-10-02).

## Where the work stands

**H0–H2 are approved.** Shared identities/full pane inventory, normalized reporter/store/reducer, host-local acknowledgement and client-bound stock control exist in production source. H2 adds the three-level tree and local-completion UI; 170 Rust tests, native/WASM checks, ten live H2 checks and eleven live H1 regression checks pass. The user requested the H2 commit and push; H3/H4 live adapters remain pending.

The stock-only [two-host review fixture](../prototypes/stock-navigation/README.md) has [15 scoped live checks](../prototypes/stock-navigation/results.json) and successful qualified Magy review. It exposes binding/observation limits and keeps ack=false. H0 approval does not finish production H1 phases.

**H1 — topology and navigation** covers production phases 1–3. The [H1 harness](../prototypes/h1-navigation/README.md) runs built production CLI/plugin artifacts and explicitly synthetic semantic records. The user approved this scope and its documented limits. Original automated evidence does not substitute for the separately recorded approval.

**H2 — tree and local completion** was approved on 2026-10-05 through the [private two-host fixture](../prototypes/h2-tree/README.md#reviewed-instance-cleaned), covering nested folding and summaries, title/status display, highlight versus actual activation, host-local Done clearing, and persistent sidebar state. Semantic reports are visibly synthetic over real receiver processes and stock navigation. Approval is recorded separately from automated evidence; the reviewed instance is cleaned.

## Technical follow-ups after H0

These are engineering follow-ups, not new H0 approval blockers. Accepted capability limits do not require further vendor fixes or repeated policy decisions.

| Item | Coding-agent work | Limit or boundary |
|---|---|---|
| Stock-only navigation contract | Current fixture verifies focused keyboard registration, filtered public plugin control, local journal order and stock focus/switch/rebind. Prepare production registration/lifecycle and first-attachment behavior after review. | Explicit private registration configuration is still required; point-in-time observations and single-display outer fixture do not prove broad host guarantees. Queued stock actions stay uncancellable, ack=false. |
| Remaining Agy outcomes | Run bounded fatal-outcome, cancellation, callback nonzero-exit/disablement/concurrency and broader permission-policy probes; keep unsupported modes explicit. | Idle, wrapper SUCCESS and repeated execution counters are already known to be insufficient by themselves. |
| Accepted Agy capabilities | Implement verified activity, permissions and aggregate completion; expose accepted question/cancellation/deadline limitations. Keep the rejected PreToolUse experiment excluded. | Full question-pending detection is not claimed; the user accepted that limit. No terminal-text or permission-changing workaround is required. |
| OpenCode coverage | Extend real 1.18.33 probes for parallel/background descendants, user-driven route changes and remote-provider variations. | Current controlled-provider tests verify installed runtime behavior with synthetic provider input; earlier authenticated permission/question fixtures are separate. |
| Magy binding and runner edge cases | Automate native Zellij-backed runner/birth/pane correlation, rename, failure/cancel/deadline and pane-closure checks when an authenticated profile is available. | Custom tmux launcher IDs are not native Zellij pane IDs. Profile availability/authentication may require account access from the user. |
| Preparation for human review | Consolidate contracts/capabilities, provide a disposable visible harness and concise reviewer commands, retain exact pass/fail/not-run evidence. | Human approval is recorded only after explicit reviewer feedback. |

These investigations are technical work; the user does not need to reproduce every failure path manually. A missing upstream interface remains a dependency constraint, not an instruction for the user to debug vendor internals.

## H0: input or real usage required from the user

### 1. Production dependency choice

**Resolved: stock Zellij only.** The earlier experimental bridge lives in the user's fork but is not selected for production. Do not ask for fork adoption again or continue dependency patching/upstream work. Build/package/runtime checks must target stock interfaces and Verij-owned integration code.

### 2. Acceptance of native shared-layout behavior

Native stack expansion can move the other client's visible member even with `mirror_session=false`. Native mirrored focus is intentionally shared. Inner-client targeting and host-local acknowledgement must still be kept separate.

**Resolved: accepted.** Preserve native layout behavior and document its multi-host effects. Do not spend dependency effort trying to provide independent stack-member visibility; keep host-local acknowledgement separate.

### 3. Acceptance or rework of agent capability limits

**Resolved: documented limits accepted.** Conservative Unknown handling and no fabricated Done are already part of the approved design. Implement the verified capability-limited Agy path without treating accepted question/cancellation/deadline gaps as a reason for endless feasibility work or new permission hooks.

The coding agent can encode version/capability detection and reliable fallback behavior. It cannot certify a real pending question from terminal text, an invalid permission hook or missing state.

### 4. Real terminal and workflow verification

**Resolved for H0: approved.** The coding agent prepared the disposable two-host test and the user supplied approve feedback. Later H1/H2 checks must confirm Workspace versus sidebar focus and host-local visits with the built feature.

H1 exposes normalized storage and verified acknowledgement through development CLI commands. H2 now renders agent rows and host-local Done; live OpenCode/Agy adapters belong to later checkpoints.

## H1: implementation that can be done by the coding agent

After H0 approval and authorization to proceed, these are normal engineering tasks:

| H1 workstream | Concrete work | User input needed during implementation? |
|---|---|---|
| Shared identities/protocol | Add server-incarnation, terminal-pane, agent-process, host-binding and navigation identities using stock observations and Verij-owned epochs; version capabilities and preserve old snapshots. | No invented stock attachment nonce/watermark; selected stock-only boundary is settled. |
| Full pane topology | Export terminal IDs, effective titles, stable tab IDs/membership, layers, suppression/stack/fullscreen state and liveness; reconcile moves, rename, reload and resurrection. | The shared-layout acceptance decision is inherited from H0. |
| Reporter/store/reducer | Implement atomic records, locking, sequence/generation validation, initial-state hydration, process birth/liveness, family/task aggregation, terminal outcomes and monotonic completion revisions. | Technical implementation and testing are autonomous. Missing upstream capabilities stay explicit. |
| Host-local acknowledgement | Persist by stable host/agent identity, prevent old-revision races and require fresh whole-host effective-focus proof. | Existing visit semantics are approved. Human keyboard/UX confirmation remains necessary. |
| Stock-capability navigation | Implement asynchronous same-/cross-session targeting through public interfaces, local coalescing/supersession, first-attachment guards and explicit verified/best-effort/unavailable outcomes. | No Zellij dependency changes; already-queued upstream operations cannot be promised cancellable. |
| Compatibility/regression verification | Build CLI/plugin, run meaningful identity/reducer/navigation tests, prove legacy session/tab snapshots remain usable and normal navigation survives unsupported reporting. | Coding agent can run checks and produce a visible development harness. |

H1 production code will require the normal workspace native and WASM checks, not merely the existing prototype validators.

## H1: human verification remains mandatory

The implementation guide explicitly requires a human approve/rework decision at H1. With the built CLI/plugin and a disposable setup, the reviewer should:

1. Inspect exported pane IDs/titles/tab membership and stable identity through rename/reload.
2. Activate tiled, hidden floating, fullscreen and stack targets across tabs/sessions and on first attachment; verify visibility and actual keyboard input.
3. Exercise two hosts and rapid requests; check for wrong-client focus, duplicate attachment or older requests stealing focus.
4. Confirm old snapshots still render sessions/tabs and failed focus never acknowledges completion.

The coding agent executed and presented the scoped scenarios, and the user supplied explicit H1 approval on 2026-10-05. That approval is recorded separately from automated PASS results. Later H2–H5 checks remain mandatory for their own stages.

## Minimum input to unlock implementation

The dependency, shared-stack and Agy capability choices are now supplied; do not repeat those questions. The coding agent owns re-scoping the contract to stock APIs and documenting honest navigation/confirmation limits.

H0 and H1 approvals are committed. H2 verification and explicit approval are recorded; the user requested committing and pushing H2. H3 remains the next checkpoint. No further Zellij patch effort is part of the backlog.
