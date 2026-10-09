# H3 OpenCode production adapter verification

The fixture runs the embedded production adapter, preliminary setup/doctor/uninstall, release Verij CLI and WASM plugin in real stock Zellij panes. Model output comes from a **controlled loopback HTTP provider**. OpenCode's TUI, session engine, tool permission/question requests, native task child, SDK observations, foreground bindings and persisted records are actual installed runtime. The test driver only records metadata and controls route/plugin enablement; it does not synthesize semantic reports.

## Reproduce

From the Verij checkout, after preparing the build environment described in the setup guide:

```sh
make dev
node --test integrations/opencode/core.test.mjs
python3 prototypes/h3-opencode/verify.py \
  --zellij "$(command -v zellij)" \
  --verij target/release/verij \
  --plugin dist/verij_plugin.wasm \
  --scratch-dir ../artifacts/scratch \
  --output ../artifacts/h3-next.json
python3 prototypes/h3-opencode/verify_evidence.py ../artifacts/h3-next.json
```

The scratch parent must already exist. The harness creates private HOME/XDG/config/data/cache/state/temp directories, its own OpenCode server/database/listener, private Zellij sockets and configuration. It installs the adapter only into its private TUI config. Default runs clean their known OpenCode TUIs, server and Zellij client/server; cleanup is verified in results. Original captures, callback metadata and failed attempts remain local workspace artifacts. No authenticated remote model is needed.

The compact [results ledger](results.json) identifies the final run. Its absolute artifact paths identify original evidence on this computer, rather than reusable runtime bindings on another machine. Rerun the fixture to obtain locally valid evidence. Automated success does not confer human H3 approval.

After committing H3 as `445519f`, master `854ecb1` was integrated. The separate [merge ledger](merge-results.json) records passing 197 Rust tests, seven Node tests, all ten H2 checks and all 25 H3 checks against the merged release artifacts. Original checkpoint results remain unchanged.

## Current checks

The final 2026-10-09 run uses **OpenCode 1.18.34**, **stock Zellij 0.45.1**, and the release-built CLI/WASM. It passes 25 scoped checks, including inherited listener/private-storage and native-binding checks:

- Idempotent setup and diagnostics preserve another TUI plugin and theme.
- Two empty selected conversations register non-synthetic idle rows before a prompt, in different foreground terminals of the same cwd/shared server.
- One held turn updates only its owner; terminal success allocates one revision across repeated reconciliation.
- Nonretryable API failure produces Error without Done.
- Actual permission request/reply and question request/reply produce Needs input, continuation and terminal success. The fixture replies once only to its generated harmless `printf VJ_H3_PERMISSION` command.
- Actual question rejection clears the request and becomes Idle without terminal success. The installed runtime ended this path at `tool-calls`; the test does not relabel it Done.
- Retry remains Working; cancellation with `MessageAbortedError` does not allocate a completion.
- Actual native task-child work aggregates into one pane row. Child completion leaves the held parent Working.
- Disabling/re-enabling the TUI plugin retains the same process/instance and completion revision.
- Pausing the TUI long enough to stop heartbeats makes doctor/watcher presentation Unknown while process birth remains alive. Resuming restores the same completion without replay.
- Home and conversation reselection clear ownership then restore the original displayed revision without allocating a new one.
- Session rename plus reporter rebind works despite stale session environment.
- Replacing a TUI mid-turn creates a new process/instance and recovers current work without another provider request.
- Normal standalone `opencode` registers Home/Idle before its first prompt. Exiting to a surviving shell removes its record from live discovery; pruning removes the dead instance.
- Uninstall preserves unrelated TUI configuration.

`verify_evidence.py` compares the report with original captures and corroborates 27 snapshots across four instances against native pane IDs, foreground process births, actual terminal assistant finish/error metadata and actual permission/question event IDs. The H2 regression separately verifies sidebar rendering/navigation and host-local acknowledgements with explicitly synthetic semantic records.

## Human H3 verification

H3 is **ready for review, not approved**. Use the [adapter installation guide](../../integrations/opencode/README.md) in a disposable inner session:

1. Run `target/release/verij agent setup opencode`, then quit and restart OpenCode. Inspect `agent doctor` for the installed binary/config/runtime and live records.
2. Start two normal OpenCode TUIs in separate panes of the same cwd. Verify two agent children appear under their actual tab, including before the first prompt.
3. Exercise a held/model/tool turn, explicit permission/question, successful completion and terminal error. Only the owning pane's status/title should update. Needs input remains until a real reply/rejection; Idle alone never creates Done.
4. Exercise native child work and attach one TUI to a shared server. Child completion must not produce parent Done while the parent still works; foreign conversations must not affect the other pane.
5. With the sidebar focused, highlight a Done row, then activate it. Highlight leaves Done; a verified visit clears only this host's displayed revision. Repeat with another host. Missing registration/proof preserves Done under the accepted stock-navigation limits.
6. Return Home, switch conversation, disable/re-enable the plugin, restart a TUI mid-turn and exit back to shell. Expect no duplicate/ghost rows, replayed completion or retained closed-agent child.

Record **approve H3** or **rework H3** with the pane/scenario and observed behavior. Human whole-host observation and authenticated remote-provider coverage have not been performed by this fixture. H4 requires its own authorization after this checkpoint.
