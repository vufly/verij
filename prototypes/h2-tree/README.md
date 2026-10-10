# H2 tree and local-completion fixture

This harness exercises the production Verij CLI and WASM plugin against installed, unmodified Zellij. Five real receiver terminals carry explicitly synthetic normalized OpenCode/Agy reports. It tests UI and navigation behavior, not live agent adapters.

## Current state

H2 was approved on 2026-10-05 with feedback **“approve H2. commit and push”**; approval is recorded separately in [approval.json](approval.json). [Automated results](results.json) pass all ten checks in cleaned root `vj-h2-sqlylf7p`; `verify_evidence.py` corroborates native identities, inner/outer control receipts, staged host acknowledgements, receiver tokens, UI frames and process-proven restart. Native/WASM builds and 170 Rust tests pass. The H1 live regression also passes all eleven checks. Original automated results retain their unapproved-at-run-time flag.

Earlier failures remain original workspace artifacts. Fixes include stoppable asynchronous watchers, bounded/coalesced discovery, nonblocking Workspace metadata updates, monotonic UI acknowledgement merging, guarded persistence after lock waits, and direct pane/tab events superseding stale session-list bootstrap data. No universal explanation is claimed for every historical bootstrap failure.

## Reviewed instance (cleaned)

Reviewed private root: `../artifacts/scratch/vj-h2-m3q16cxs`. Both host screens and passive native queries were checked after preparation. This instance was cleaned after approval; its private socket was removed and known receivers, sidebars, attachment clients and servers exited. Previous attach commands are no longer live. Reproduce with `--keep-live` to obtain a fresh root and attach commands; helper examples below refer to the historical reviewed root.

The sidebar starts with explicitly synthetic states: Fixture-0 **Done** in both hosts, Fixture-1 **Needs input**, Fixture-2 **Working**, Fixture-3 **Error**, and Fixture-unknown **Unknown** in a new real terminal. The automated closure test removed Fixture-4; review preparation creates the separate Unknown receiver rather than reusing its identity. Preparation advances synthetic turns without rewriting automated evidence.

Suggested review:

1. Move with `j`/`k` or arrows. Highlight and folds should leave Fixture-0 Done in both hosts.
2. Use Space on an agent to fold its parent tab; use Left/Right or `h`/`l` to navigate branches.
3. Enter or click Fixture-0 in host-a. Keyboard should reach receiver 0; Done clears only in host-a. Host-b should retain Done until its own confirmed visit.
4. Return to the sidebar by clicking its blank area. Enter Fixture-1; its permission marker remains because visiting does not answer a request.
5. Resize and inspect summaries. `q` restarts only the private sidebar; selection/folds and host acknowledgements persist.

The helper returns focus to the sidebar without requesting acknowledgement:

```sh
python3 prototypes/h2-tree/review.py sidebar \
  --root ../artifacts/scratch/vj-h2-m3q16cxs --host a
```

For another synthetic unread completion, first return both hosts to their sidebars, then:

```sh
python3 prototypes/h2-tree/review.py fixture \
  --root ../artifacts/scratch/vj-h2-m3q16cxs --pane 0 --status done
```

`fixture` also accepts `working`, `idle`, `needs_input`, `error` and `unknown` for surviving fixture panes 0–3. These are test reports, not adapter observations. `query` is passive; `focus` uses native navigation and may be acknowledged automatically after actual whole-host proof.

After review, clean only this manifest-qualified instance:

```sh
python3 prototypes/h2-tree/review.py clean \
  --root ../artifacts/scratch/vj-h2-m3q16cxs
```

The recorded approval covers H2 UI behavior within its documented limits. H3 OpenCode adapters remain the next checkpoint.

## Reproduce

From the Verij checkout:

```sh
make dev
python3 prototypes/h2-tree/verify.py \
  --cli target/release/verij \
  --plugin dist/verij_plugin.wasm \
  --scratch-dir ../artifacts/scratch \
  --output ../artifacts/h2-tree-next.json
python3 prototypes/h2-tree/verify_evidence.py ../artifacts/h2-tree-next.json
```

The harness generates private XDG directories, permissions, configuration, sockets and sessions. It cleans failed runs and successful runs by default, checking native births for receivers, sidebars, attachment clients and servers. `--keep-live` retains only a fully passing instance, then prints private host-a/host-b attach commands. No user session or global configuration is changed by the fixture.

Use `--bootstrap-registration` to reproduce the missing-binding human feedback. Both inner clients start Locked, with no fixture-created host bindings and no registration keybinding. The first Enter/click on an agent must invoke the production shared registration surface, receive the physical Workspace keyboard nonce, then complete exact navigation and host-local acknowledgement. The same ten checks and evidence validator apply; `initial_host_bindings_absent: true` records this stronger setup. A temporary native dialog may be visible to peers; the user accepted this on 2026-10-09. It does not itself acknowledge a completion.

Add `--legacy-source` with `--bootstrap-registration` to begin both hosts in a separate source session without a monitoring exporter. Activation must recover native inventory before switching to the shared agents. Six repeated agent activations then verify actual receiver keyboard delivery without any session-switch requests. Add `--startup-contention` to hold the real cross-process startup lock for over 2.5 seconds while receiver titles change, requiring automatic Workspace-title recovery and error-free retained sidebar frames. Both options together expand the fixture to thirteen independently validated checks. The 2026-10-10 passing root `vj-h2-v2yd2dpo` is cleaned.

Add `--latency` with `--legacy-source` to measure eight repeated cross-session agent hops. The fixture uses a production tree template with a visible `ACTIVE:session/tab` marker, corroborates exact native focus and real receiver input, and retains `cross-session-latency.json` plus per-hop frames. The final fourteen-check run `vj-h2-ql68vhqn` is cleaned and independently validated: focus-to-highlight follow-up is 243 ms median / 322 ms maximum; whole focus is 5.96 s median / 8.04 s maximum, including destination registration. Each sample also confirms bounded Verij plugin panes and a fresh peer-host query. The [latency ledger](latency-results.json) identifies this run; older thirteen-check roots do not imply the newer timing scenario passed.

Final builds used the private Rust 1.98.1 toolchain at `/tmp/opencode/verij-rustup` after a worker-reported Rust reinstall left the selected user toolchain without its WASM target. To reproduce with that private toolchain, prefix build/check commands with `RUSTUP_HOME=/tmp/opencode/verij-rustup RUSTUP_TOOLCHAIN=1.98.1`. This does not select a patched Zellij binary.

## Checks and evidence

- Session → Tab → Agent rows with fixture labels and six status icons.
- Highlight and sidebar focus leave Done unread in both hosts.
- Enter targets the exact receiver and acknowledges only the activating host.
- Space on an agent folds its parent tab, preserving other branches.
- Mouse activation acknowledges the second host independently.
- Collapsed summaries, selection, folds and host acknowledgements survive sidebar restart. The private `sidebar.py` wrapper records each production UI process birth; the check requires the old process to exit and a different live process to start.
- Native navigation acknowledges only after whole-host visit proof.
- Visiting Needs input has a fresh outer focus receipt and an actual receiver-1 keyboard token, while leaving its outstanding permission request intact.
- Narrow resizing preserves visible status and tree controls.
- Closing a receiver removes its row while retaining its parent and surviving sibling.

Each run retains its original `verification.json`, native topology, registrations, synthetic agent state, host acknowledgement records, control results, receiver input tokens and selected screen captures in its generated root. Failures include the failing stage and private process/thread diagnostics. Raw agent transcripts and credentials are not part of this fixture.

Stock limitations remain those accepted at H1: one attached display per outer host for verified visits, focused-keyboard registration, point-in-time native observations, and local supersession without cancellation of stock actions already dispatched. Missing proof preserves Done.
