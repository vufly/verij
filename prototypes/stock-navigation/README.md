# Stock-Zellij navigation and human review fixture

This is a **Verij-owned H0 control fixture**, not the production agent-monitoring tree. It uses installed, unmodified **Zellij 0.45.1** and the public `zellij-tile = 0.45.1` API. No sibling dependency edits/build/install or private IPC extension is involved. Every result keeps `acknowledge_done=false`; no real agent records or completion store exist.

## Reproduce

From the Verij worktree:

```bash
cargo build --locked --manifest-path prototypes/stock-navigation/plugin/Cargo.toml \
  --target wasm32-wasip1 --target-dir ../artifacts/stock-plugin-target
python3 prototypes/stock-navigation/demo.py verify \
  --plugin ../artifacts/stock-plugin-target/wasm32-wasip1/debug/verij-stock-navigation.wasm \
  --scratch-dir ../artifacts/scratch --output ../artifacts/stock-navigation-results.json
```

The WASM build is a Verij probe against the published stock SDK, not a Zellij build. Generated plugin permissions apply only in the private XDG cache. Config, native sessions, tmux socket and receiver logs are isolated; diagnostics remain in scratch after cleanup. The binary path/version/hash are recorded and the archived sibling binary is explicitly rejected.

## Stock contract

1. Each owned Workspace wrapper records its actual host session/pane and persistent inner-client PID. Controller validates Linux boot/PID/start identity; it does not guess inner ClientId from environment/cwd.
2. Ctrl+b focuses the public probe plugin for that display. The controller enters its own random host nonce on the focused plugin. Stock Event::Key plus native `get_plugin_ids()` supplies the actual plugin/client/server context. Two distinct clients are validated by live receiver input. Legacy CLI/keybind bind payloads cannot register hosts.
3. Plugin load timestamps are **Verij-owned epochs**, not stock socket-generation tokens. Reload/client replacement needs fresh registration; server birth and fresh producer snapshots are checked separately. Session names are mutable routing metadata. SessionUpdate is preferred, with server-owned session environment as a startup hint—not host binding proof.
4. Named CLI control pipes can broadcast; receivers filter exact server/plugin/client/epoch before acting. Request IDs, sequence journal, result files and polling are Verij-owned application protocol. Query sequence0 is passive and preserves the journal. Older/duplicate requests are rejected at the plugin boundary; already-queued stock actions cannot be cancelled.
5. Focus uses public `get_pane_info`, `focus_terminal_pane` and fresh `get_focused_pane_info`. It reports `inner_focus_observed`, a point-in-time observation—not upstream execution completion or whole-host visit. Switch uses public `switch_session_with_focus`, returns `switch_dispatched_unverified`, waits for actual receiver rendering, then re-registers in the destination.
6. Outer focus confirmation is scoped to the demo's single-display outer servers. Production multi-client host guarantees are not claimed. The review sidebar coalesces queued requests, ignores stale local results and keeps input/render responsive; this is not full production navigation/reducer implementation.

Public CLI commands may return an “already focused” nonzero result; the controller confirms current focus before treating it as success. Early bootstrap control/keyboard probes caused failures; setup now waits for an actual guest receiver frame before registration/control. This avoids that fixture race without claiming to fix stock internals. A direct per-host keybind payload attempt did not register both hosts and was replaced by focused-plugin keyboard input.

## Visible two-host review

```bash
python3 prototypes/stock-navigation/demo.py start \
  --plugin ../artifacts/stock-plugin-target/wasm32-wasip1/debug/verij-stock-navigation.wasm \
  --scratch-dir ../artifacts/scratch --output ../artifacts/stock-review-live.json
```

The command returns ROOT, two private tmux attach commands and exact cleanup. Open one terminal for host-a and another for host-b. Left sidebar is a clearly labelled review fixture; right Workspace contains receiver targets:

- 0/1: ordinary tiled panes.
- 2: hidden floating pane.
- 3/4: members of a separate stack tab.

Use j/k or arrows to select (no navigation), Enter or a row click to activate, then type a unique test word and Enter into Workspace. Click an empty part of the sidebar to return without activation; click a row to activate it. q is a passive query, r refreshes explicit registration. F11 was intercepted under Descend in earlier tests, so use mouse or the supplied helper instead of relying on it.

**Reviewer feedback:** approve/rework the stock routing demonstration, naming the host/target/input that failed. Check Enter/click keyboard transfer, hidden float and stack reveal, two-host tiled isolation subject to accepted shared-stack behavior, and selection/query separation. Do not test actual Done acknowledgement against this fixture: it is deliberately absent. H0 and H1 approval are not automatic.

Cleanup, with the actual ROOT printed by start:

```bash
python3 prototypes/stock-navigation/demo.py clean --root "$ROOT"
```

Only this manifest's private sessions/socket are selected. Recorded server/client births are checked for exit. Persistent review demos intentionally stay alive until the reviewer cleans them; automated verify runs clean themselves.

## Evidence and workers

[results.json](results.json) is the latest automated stock verification; failed/intermediate roots retain their original setup/verification JSON and receiver traces. [summary.json](summary.json) distinguishes attempts and limits. Original Magy worker reports remain in workspace `artifacts/worker-stock-review` and `worker-stock-ux`, with independent qualifications correcting overstatements and recording applied fixes.

Both default-model watches completed/exit0: `watch_d1a73f133168421b` (contract/controller review) and `watch_226257675d254063` (human review guide). Their custom tmux launch metadata is not native pane binding evidence. Broad stock-version/lifecycle coverage and production H1 code remain pending.

## Recorded H0 approval

On **2026-10-02**, the user replied **“approve”** to the two-host stock-demo handoff and then requested committing H0 first. This approves the selected integration contract and documented limits, not production agent rows, Done handling or H1 completion. [summary.json](summary.json) records that decision separately from the original automated [results.json](results.json); no additional per-scenario human observations are inferred.

The reviewed instance `vj-stock-o_d7evzw` has been cleaned. Its old attach commands are no longer live; use `demo.py start` to create another private fixture when needed.
