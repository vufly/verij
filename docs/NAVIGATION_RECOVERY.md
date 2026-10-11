# Navigation Recovery Guide

Status: **Phase 1 diagnostics implemented; remaining coordination/recovery phases proposed**. Engineering changes are tracked in the [navigation reliability plan](NAVIGATION_RELIABILITY_PLAN.md).

## What the Error Means

`outer native focus observation unavailable` means Verij confirmed the target terminal inside the inner session, then failed to confirm focus on the outer pane containing that session: Workspace.

The target terminal may already be selected internally. This error indicates unverified navigation, not an agent execution failure. It does not prove that a race occurred; missing panes, stale context, supersession, and observation failure can produce the same message.

## Reduce Interference

1. Select a target once and wait for navigation to finish before selecting another.
2. Avoid manual pane/tab changes while navigation is pending.
3. Keep exactly one attached display client on the outer host session when using this verified navigation path. Multiple hosts attached to an inner session are a different topology; do not detach them indiscriminately.
4. If pointer movement seems to interfere, keep the pointer still during navigation or consider disabling host focus-following-mouse behavior.

Optional host configuration in `config.toml`:

```toml
[zellij]
focus_follows_mouse = false
```

Verij defaults this setting to `true`. Changed host Zellij settings apply to new or resurrected hosts; they do not retroactively change an already-running Zellij server. See [configuration details](../README.md#configuration).

These habits reduce interference with today's asynchronous flow. The planned implementation must handle repeated navigation requests correctly without requiring perfectly paced input.

## Recover from a Failure

1. **Stop repeated clicking.** Let the current request finish.
2. **Manually select the outer Workspace pane.** Check whether the intended inner terminal is already visible and receives input.
3. **Confirm the target still exists.** A closed terminal or restarted agent may require selecting its new live row.
4. **Retry once explicitly** if the target remains live and focus is still wrong.
5. **If the failure repeats, collect diagnostic evidence** before changing the session or restarting components.

If diagnostics identify a stale plugin or host binding, refresh registration through Verij's verified registration flow. Capture evidence before restarting; do not manually edit ownership journals or reuse numeric pane/client IDs as proof of identity.

`verij recover [HOST]` restores the complete host layout and discards additional host-only panes and tabs. Use it for a damaged host layout, rather than as the first response to a focus-confirmation error. See [host recovery behavior](../README.md#host-management).

## Evidence to Capture

- Exact error text and approximate occurrence time.
- Host, selected target, and whether navigation crossed sessions or tabs.
- Whether Workspace was manually focused afterward and which terminal appeared.
- Whether another click, keyboard focus change, pointer movement, or additional outer display coincided with the failure.
- Whether the target closed, the plugin reloaded, or a session/attachment restarted.
- Detailed diagnostic records from `navigation-diagnostics.ndjson` under the active control directory (`VERIJ_CONTROL_DIR` or states parent `control`). Each record provides:
  - `intent_id`: navigation intent UUID correlating all stage-level requests
  - `stage`: `precondition`, `switch`, `rebind`, `inner_focus`, `outer_focus`, or `verification`
  - `failure_kind`: `precondition_failure`, `ownership_failure`, `dispatch_failure`, `timeout`, `control_failure`, `missing_observation`, `observation_mismatch`, `superseded`
  - `control_status`: underlying wire status from plugin when available (e.g. `pane_missing`, `stale_context`, `unavailable`, `superseded`)
  - `progress`: confirmed inner terminal and session vs unverified outer Workspace pane
  - `expected_terminal` and `observed_terminal`
  - `requested_session` and `observed_session`
  - `request_ids`: stage-level `ControlRequest` IDs correlated to the intent
  - `current_request_id`: stage-level expected request ID correlated to the intent
  - `elapsed_ms`: monotonic duration from intent start
  - `validated`: whether evidence was corroborated vs rejected as untrusted
- Relevant correlated wire results from `control-results.ndjson` under the control directory.

## Shipped and Planned User Experience

Phase 1 ships structured actionable messages that identify the exact stage and recovery recommendation, replacing generic errors:

- **Inner confirmed, outer unconfirmed**:
  `Inner terminal selected; Workspace focus not confirmed (pane missing). Expected outer pane 12, observed pane 7; select Workspace or retry.`
- **Inner focus unconfirmed**:
  `Inner focus not confirmed (pane missing). Expected terminal 3; retry.`
- **Session switch unconfirmed**:
  `Switch to session '<session>' not confirmed (timed out); retry.`
- **Rebind unconfirmed**:
  `Rebind to session '<session>' failed (timed out); retry registration.`
- **Precondition unconfirmed**:
  `Navigation precondition failed: <reason>; verify target and retry.`
- **Both focus confirmed, verification failed**:
  `Focus observed; final ownership/visit verification failed (<reason>). select Workspace or retry.`

Remaining phases (whole-operation coordination, bounded recovery, and combined state verification) remain proposed:

- Newer selections replace pending intent; obsolete results do not overwrite current UI state (Phase 2).
- Recoverable observation failures use bounded fresh-state checks (Phase 3).
- Explicit retries remain available when automatic recovery would risk overriding manual focus (Phase 3).
- Combined verification confirms bracketed state via independent visit checks before acknowledging completion revisions (Phase 4).

## Testing and Verification Note

Phase 1 diagnostics, structured error types, shared evaluator reducer, and journal append locking were verified via deterministic Rust unit and integration test suites (`cargo test -p verij-cli -p verij-types` and `make check`) covering synthetic and simulated transport failure modes, not through live multi-client Zellij reproductions. Live interactive Zellij validation is scheduled for Phase 5.
