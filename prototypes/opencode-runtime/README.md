# OpenCode runtime evidence qualification

This is H0 evidence tooling, not a production adapter or H0 approval. The 2026-10-02 Magy worker observed installed OpenCode **1.18.33**; the workspace's read-only reference remains pinned to 1.18.32.

**Current dependency direction:** future reproductions use installed stock Zellij. Earlier recorded runs used the experimental patched binary and retain that provenance; they do not make it a production requirement. This OpenCode harness uses public Zellij CLI operations, not the archived private attachment/completion protocol.

The retained worker lives at workspace `artifacts/worker-g2/`. Its `probes/g2/g2-observations-1026081.ndjson` contains 102 redacted callbacks from two genuine Zellij TUI panes attached to one server. `INDEPENDENT_QUALIFICATION.md` overrides overbroad PASS claims in the original worker report. The watched run itself ended failed/exit 1 after retaining useful artifacts; it is not recorded as a successful worker execution.

Run from the Verij worktree:

```bash
python3 prototypes/opencode-runtime/qualify.py \
  ../artifacts/worker-g2/probes/g2/g2-observations-1026081.ndjson \
  --output ../artifacts/g2-independent-qualification.json
```

The qualifier verifies ready-before-callback ordering, same-envelope delivery to both TUIs, owner/foreign direct-route equality, matching permission/question request IDs, pending-state clearance, programmatic navigation and plugin disposal. It never infers Done from idle or null status. The [compact fixture](semantic-fixture.json) is a manually projected subset with original line provenance, not another independent runtime.

Still required: assistant terminal finish/cancel/retry semantics, descendant families, mid-turn recovery, user navigation, foreground tpgid/process-birth validation and fully isolated server storage. The worker inherited HOME/XDG, used private TUI configuration and private Zellij sockets, and requested port zero but obtained port 4096. Future reproducers must verify listener ownership and isolate storage before model turns. Raw server logs and payload snapshots stay local; only reviewed status/identity/schema projections should be promoted.

## Isolated controlled-provider continuation — 2026-10-02

[`verify_runtime.py`](verify_runtime.py) closes several of those gaps in installed **OpenCode 1.18.33**. It starts a loopback OpenAI-compatible provider fixture, an actual OpenCode server, and two actual Zellij pane-local TUIs with [`runtime-tui.mjs`](runtime-tui.mjs). The provider replies are **synthetic protocol fixtures**; the session engine, SDK events, message finish/error metadata, native task child and TUI caches are real installed runtime. No user credentials or remote model calls are used. This complements, rather than replaces, the earlier authenticated-provider permission/question observations.

```bash
python3 prototypes/opencode-runtime/verify_runtime.py \
  --binary "$(command -v zellij)" --scratch-dir ../artifacts/scratch \
  --output ../artifacts/opencode-runtime-results.json
python3 prototypes/opencode-runtime/verify_evidence.py \
  prototypes/opencode-runtime/runtime-results.json
```

Every run uses a private HOME, config/data/cache/state, SQLite database and temporary directory. It chooses an explicit ephemeral OpenCode port, checks that its new server PID actually holds that listener, and confirms runtime paths via HTTP. Configuration is loaded by fresh child processes. Native inventory, controlling tty/tpgid and boot/PID/start tokens corroborate the two bindings; replacement TUI identity is checked separately. Metadata records hash identifiers and omit message/tool text, request arguments, error bodies and credentials. Original local callback NDJSON remains in scratch; the evidence validator checks reported snapshots against it.

[Final results](runtime-results.json), scratch `vj-runtime-apm7344n`, pass nine checks:

- Success requires completed assistant metadata with `finish:stop` and no error, not idle alone.
- HTTP 400 yields completed `APIError` without terminal success; cancellation yields `MessageAbortedError` without success.
- HTTP 429 yields retry callback/attempt and an incomplete assistant, followed by success after a controlled response. Five requests are recorded for this retry case.
- SIGKILL of one TUI during a blocked provider request and native replacement-pane attachment recover Busy/history without replaying the provider call. The server birth remains unchanged and request count stays one.
- An actual synchronous native `task` child is in the selected parent's family, while both parent/child are foreign to the other TUI. The blocked child prevents terminal parent completion; both later finish. Parent `tool-calls` completion is distinct from its final `stop`.
- A later turn after the owner navigates Home still broadcasts events, all classified without a local owner.

**Hydration caveat:** `api.state.ready` can be true while selected status is absent and message history empty. The new probe initially failed by treating that flag as sufficient; a subsequent snapshot, about 0.3 seconds later in the final run, hydrated Busy/history. Keep Unknown until required family/status/history snapshots are present. This is a real initial-state observation, not permission to infer Idle from missing state.

Original failures remain workspace `artifacts/opencode-runtime-attempt-1.json` (probe incorrectly expected a `data` field in `/path`) and `attempt-2.json` (premature hydration assertion). `attempt-3.json` passed before final metadata/cleanup checks. Remote-provider variants, background/parallel descendant behavior, user-driven conversation switches, server crash recovery and production reducer/store isolation remain outside this scoped result. H0 is unapproved.
