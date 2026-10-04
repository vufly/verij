# H1 production topology, storage and stock navigation review

This harness runs the **production Verij CLI and WASM plugin** against installed, unmodified Zellij 0.45.1. H0 and H1 are approved. Receiver processes and semantic reports are explicitly synthetic fixtures. They establish topology, process ownership, reducer/storage and navigation behavior, not working OpenCode/Agy adapters or the H2 agent tree.

On 2026-10-05 the user replied **“approve. commit H1 then continue H2”**. [approval.json](approval.json) records that decision separately from automated evidence. The reviewed instance `vj-h1-3luxrt2u` has been cleaned; its previous attach commands are no longer live.

## Build and reproduce

From the Verij worktree:

```bash
cargo check --workspace
cargo test --workspace
make check
make dev
python3 prototypes/h1-navigation/verify.py \
  --cli target/release/verij --plugin dist/verij_plugin.wasm \
  --scratch-dir ../artifacts/scratch --output prototypes/h1-navigation/results.json
```

`make dev` builds Verij, including its published-SDK WASM plugin. It does not build or modify Zellij. The harness uses private XDG directories, generated configs, pregranted permissions for this plugin only, and private tmux/Zellij sockets. Failed runs retain their original report and diagnostic frames; automated runs clean private sessions and check recorded process births for exit.

## What H1 implements

- Optional/defaulted full inventory preserves old session/tab JSON decoding. Terminal and plugin namespaces remain distinct. Stable tab IDs, full terminal membership, effective titles, floating/suppressed/fullscreen/layer-focus facts and SDK pane PID locators are exported. Stack IDs remain unknown; suppression is not stacking.
- Native readers qualify server and pane PIDs with boot/user/process birth. Session identity survives rename/plugin reload and changes with process incarnation. Old exports cannot inherit a reused PID's new identity. Plugin `TabUpdate` preserves cached panes; background title/topology changes publish atomic snapshots.
- Normalized records retain source-specific facts, producer/turn ordering, terminal outcome and reporter-owned completion revisions. Atomic, locked registration/report/pruning and host-local acknowledgement persistence are implemented. Agent updates use their own watcher, without launching topology queries for every report.
- The owned Workspace wrapper publishes its actual terminal and persistent attachment process before `exec`. Focused-keyboard registration supplies the native inner plugin/client tuple. A passive outer SDK locator verifies the wrapper against the exact pane's process/foreground tty; inherited environment and tty presence alone do not bind it.
- Versioned `verij_control` requests filter the exact server/plugin/client/application epoch, preserve passive query ordering and correlate replies. The accepted Verij application sequence resynchronizes a journal when returning to a still-live context; it is not a stock execution watermark. Same-session targeting is native; cross-session dispatch is followed by destination registration. Legacy `switch:<session>` remains available.
- Session/tab activation runs outside the TUI input/render path, with local queue coalescing and obsolete follow-up suppression. Initial attachment is serialized and an existing owned wrapper prevents reinjection. Tab zero remains a valid target.
- Done acknowledgement requires a separate passive, bracketed outer/inner visit observation, validated births, exact instance/pane identity and the caller's captured completion revision. Sidebar focus, stale/missing proof, wrong pane, failed navigation and an inner observation alone cannot clear it. Two hosts persist independent acknowledgements; an old captured revision does not consume a newer completion.

## Live checks

The harness checks:

1. Full terminal inventory, stable tab membership and native process births, including hidden floats and stack members with unknown stack IDs.
2. Two actual owned outer Workspace attachments and distinct inner client registrations.
3. Tiled, floating and stack targeting with real keyboard receiver tokens.
4. Another fullscreen target becoming visible and receiving input.
5. Passive queries leaving request sequence and acknowledgement unchanged.
6. Real foreground tty ownership and rejection of a redirected child with the same process family.
7. Clearly synthetic Working/Done records with actual native visit proof, host-local acknowledgement, a newer completion race and wrong-pane refusal.
8. Sidebar focus refusing acknowledgement and existing attachment remaining single after the legacy TUI activation path.
9. Cross-session rebind preserving the attachment process and the peer's source session.
10. Public plugin reload preserving native server/pane identity.
11. Session and background-pane title rename preserving server, terminal and stable tab identity, followed by fresh registration and actual keyboard targeting.

Raw receiver, inventory and control metadata remain in the printed private root. [results.json](results.json) records the latest cleaned automated run; workspace `artifacts/h1-navigation-attempt-*.json` retains earlier attempts. A PASS is scoped evidence, not human H1 approval.

## Human review

Add `--keep-live` to the verification command, with an output path under workspace artifacts. A passing run prints two attach commands and leaves that one private instance running. Use separate terminals for host-a and host-b. Run the helper in a third terminal:

```bash
python3 prototypes/h1-navigation/review.py inventory --root "$ROOT"
python3 prototypes/h1-navigation/review.py focus --root "$ROOT" --host a --pane 0
python3 prototypes/h1-navigation/review.py focus --root "$ROOT" --host b --pane 1
python3 prototypes/h1-navigation/review.py focus --root "$ROOT" --host a --pane 2
python3 prototypes/h1-navigation/review.py focus --root "$ROOT" --host a --pane 3
python3 prototypes/h1-navigation/review.py focus --root "$ROOT" --host a --pane 4
python3 prototypes/h1-navigation/review.py inspect --root "$ROOT"
```

Type a unique word and Enter in the chosen host after each focus. Click empty sidebar space to return. Observe the existing production Session → Tab tree; agent-row rendering belongs to H2. The inspection command explicitly identifies the synthetic record and its completion revision. Approve/rework H1 with any failing host, target, ownership or keyboard behavior.

Clean only that manifest's private instance after review:

```bash
python3 prototypes/h1-navigation/review.py clean --root "$ROOT"
```

## Limits

Verified host visits currently require **one attached display per outer host**, owned wrapper identity and available native observer. Multi-display outer hosts remain unavailable for verified acknowledgement. Each inner/destination session needs the registration binding; a client overlay alone did not add it to an already-running session in these tests. Missing registration refuses pane-control rather than injecting another attach. No global configuration or existing user session was changed to remove that limit.

Rename/reload can leave effective client focus unavailable even though the server/process identities remain stable. Re-register explicitly before further pane control in that case. The harness does so before its post-rename keyboard check; stale/no-focus context cannot acknowledge completion.

Stock queued operations are not cancellable. Verij coalesces local work and discards obsolete results/follow-up transfer, but cannot revoke upstream dispatch. Native shared-stack behavior remains accepted. Point-in-time stock queries are not private execution barriers. Live adapter setup, semantic capability detection/diagnostics and automatic agent-row presentation follow at H2–H4.
