# Agent monitoring: remaining gates and ownership

Assessment date: **2026-10-02**. This assessment does not approve H0 or authorize later implementation phases. The [approved product design](AGENT_MONITORING.md) and [implementation checkpoints](AGENT_MONITORING_IMPLEMENTATION.md#101-human-verification-checkpoints) remain authoritative.

## Where the work stands

The current stop is **H0 — integration contract**. The accumulated changes are isolated dependency prototypes, reproducible runtime harnesses and qualified evidence. Production monitoring types, agent store/reducer, host-local acknowledgement and agent-pane navigation are not implemented.

**H1 — topology and navigation** follows production phases 1–3: shared types/full pane inventory, reporter/store/reducer/acknowledgement, and exact asynchronous navigation. The prototypes prove useful native behavior, but they do not complete those phases or substitute for the H1 CLI/plugin build and human verification.

## H0: work the coding agent can continue

| Item | Coding-agent work | Limit or boundary |
|---|---|---|
| Final cross-session control contract | Design and implement a generation-qualified, correlated switch/binding prototype; validate races, first attachment and effective focus. | Existing switching evidence uses a display-owned native keybinding. The current request/result endpoint focuses within one verified server attachment. |
| Remaining Agy outcomes | Run bounded fatal-outcome, cancellation, callback nonzero-exit/disablement/concurrency and broader permission-policy probes; keep unsupported modes explicit. | Idle, wrapper SUCCESS and repeated execution counters are already known to be insufficient by themselves. |
| Neutral question observation | Investigate documented structured interfaces and compare behavior. Keep the rejected empty-output PreToolUse experiment excluded from production. | A real question is visible while callbacks remain Working. A user looking at the terminal cannot manufacture a missing structured API signal. If no neutral interface exists, an upstream change or accepted capability restriction is needed. |
| OpenCode coverage | Extend real 1.18.33 probes for parallel/background descendants, user-driven route changes and remote-provider variations. | Current controlled-provider tests verify installed runtime behavior with synthetic provider input; earlier authenticated permission/question fixtures are separate. |
| Magy binding and runner edge cases | Automate native Zellij-backed runner/birth/pane correlation, rename, failure/cancel/deadline and pane-closure checks when an authenticated profile is available. | Custom tmux launcher IDs are not native Zellij pane IDs. Profile availability/authentication may require account access from the user. |
| Preparation for human review | Consolidate contracts/capabilities, provide a disposable visible harness and concise reviewer commands, retain exact pass/fail/not-run evidence. | Human approval is recorded only after explicit reviewer feedback. |

These investigations are technical work; the user does not need to reproduce every failure path manually. A missing upstream interface remains a dependency constraint, not an instruction for the user to debug vendor internals.

## H0: input or real usage required from the user

### 1. Production dependency choice

The experimental Zellij 0.45.1 bridge lives in the user's `vufly/zellij` fork. The user approved the isolated prototype and fork, but production pinning/maintenance/distribution is not yet approved. Confirm whether the production feature should depend on that patched fork while upstream support is pursued. The coding agent can implement the chosen build/package/version-detection path; upstream acceptance is outside its control.

### 2. Acceptance of native shared-layout behavior

Native stack expansion can move the other client's visible member even with `mirror_session=false`. Native mirrored focus is intentionally shared. Inner-client targeting and host-local acknowledgement must still be kept separate.

The user must decide whether the observed shared-stack behavior is acceptable for the intended multi-host workflow. If it is unacceptable, the coding agent can investigate a different native/control strategy or dependency change, but must not silently promise independent stack visibility from the current implementation.

### 3. Acceptance or rework of agent capability limits

Conservative Unknown handling and no fabricated Done are **already part of the approved design**; they do not require a new product-design round. The outstanding choice is whether the **observed incomplete Agy question/cancellation/deadline coverage** is acceptable for the release scope, or requires an upstream signal before release. This is an explicit H0 capability decision, not permission to weaken the agreed semantics.

The coding agent can encode version/capability detection and reliable fallback behavior. It cannot certify a real pending question from terminal text, an invalid permission hook or missing state.

### 4. Real terminal and workflow verification

The coding agent can prepare and automate a disposable two-host test. The user should observe actual keyboard transfer in the terminal/layout normally used, judge shared-stack/mirroring behavior, and provide approve/rework feedback for the proposed contract. Later H1/H2 checks must confirm Workspace versus sidebar focus and host-local visits with the built feature.

Do not ask the user to test agent-row Done visits against today's code: those rows and acknowledgement logic do not exist yet. The prescribed H1/H2 human checks occur after their respective implementation artifacts are available.

## H1: implementation that can be done by the coding agent

After H0 approval and authorization to proceed, these are normal engineering tasks:

| H1 workstream | Concrete work | User input needed during implementation? |
|---|---|---|
| Shared identities/protocol | Add server-incarnation, terminal-pane, agent-process, host-binding and navigation identities; version capabilities and preserve old snapshot decoding. | No new product decision beyond the selected H0 integration contract. |
| Full pane topology | Export terminal IDs, effective titles, stable tab IDs/membership, layers, suppression/stack/fullscreen state and liveness; reconcile moves, rename, reload and resurrection. | The shared-layout acceptance decision is inherited from H0. |
| Reporter/store/reducer | Implement atomic records, locking, sequence/generation validation, initial-state hydration, process birth/liveness, family/task aggregation, terminal outcomes and monotonic completion revisions. | Technical implementation and testing are autonomous. Missing upstream capabilities stay explicit. |
| Host-local acknowledgement | Persist by stable host/agent identity, prevent old-revision races and require fresh whole-host effective-focus proof. | Existing visit semantics are approved. Human keyboard/UX confirmation remains necessary. |
| Exact navigation | Implement asynchronous same-/cross-session targeting, first-attachment guard, visibility correction, supersession, timeouts and correlated failure with no false acknowledgement. | No per-edit approval needed once H0 selects the contract; unresolved dependency/API changes require a decision. |
| Compatibility/regression verification | Build CLI/plugin, run meaningful identity/reducer/navigation tests, prove legacy session/tab snapshots remain usable and normal navigation survives unsupported reporting. | Coding agent can run checks and produce a visible development harness. |

H1 production code will require the normal workspace native and WASM checks, not merely the existing prototype validators.

## H1: human verification remains mandatory

The implementation guide explicitly requires a human approve/rework decision at H1. With the built CLI/plugin and a disposable setup, the reviewer should:

1. Inspect exported pane IDs/titles/tab membership and stable identity through rename/reload.
2. Activate tiled, hidden floating, fullscreen and stack targets across tabs/sessions and on first attachment; verify visibility and actual keyboard input.
3. Exercise two hosts and rapid requests; check for wrong-client focus, duplicate attachment or older requests stealing focus.
4. Confirm old snapshots still render sessions/tabs and failed focus never acknowledges completion.

The coding agent can execute and present those scenarios. The user's real workflow/keyboard judgement and explicit sign-off cannot be replaced by an automated PASS. H1 is not approved by this assessment, and the remaining human checks are not evidence that implementation has already been completed.

## Minimum input to unlock implementation

The immediate input is **H0 feedback**, not another coding request for H1:

- Accept or rework the patched-Zellij production dependency and shared-stack limitation.
- Accept an explicitly capability-limited Agy path with the approved conservative semantics, or require further upstream work.
- Review the visible client-routing evidence and explicitly approve the selected H0 contract when the remaining agreed probes are sufficient.

The coding agent owns the technical backlog and can prepare these decisions for review. Formal H1 implementation and final human acceptance follow that authorization.
