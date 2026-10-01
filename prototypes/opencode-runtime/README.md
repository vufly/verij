# OpenCode runtime evidence qualification

This is H0 evidence tooling, not a production adapter or H0 approval. The 2026-10-02 Magy worker observed installed OpenCode **1.18.33**; the workspace's read-only reference remains pinned to 1.18.32.

The retained worker lives at workspace `artifacts/worker-g2/`. Its `probes/g2/g2-observations-1026081.ndjson` contains 102 redacted callbacks from two genuine Zellij TUI panes attached to one server. `INDEPENDENT_QUALIFICATION.md` overrides overbroad PASS claims in the original worker report. The watched run itself ended failed/exit 1 after retaining useful artifacts; it is not recorded as a successful worker execution.

Run from the Verij worktree:

```bash
python3 prototypes/opencode-runtime/qualify.py \
  ../artifacts/worker-g2/probes/g2/g2-observations-1026081.ndjson \
  --output ../artifacts/g2-independent-qualification.json
```

The qualifier verifies ready-before-callback ordering, same-envelope delivery to both TUIs, owner/foreign direct-route equality, matching permission/question request IDs, pending-state clearance, programmatic navigation and plugin disposal. It never infers Done from idle or null status. The [compact fixture](semantic-fixture.json) is a manually projected subset with original line provenance, not another independent runtime.

Still required: assistant terminal finish/cancel/retry semantics, descendant families, mid-turn recovery, user navigation, foreground tpgid/process-birth validation and fully isolated server storage. The worker inherited HOME/XDG, used private TUI configuration and private Zellij sockets, and requested port zero but obtained port 4096. Future reproducers must verify listener ownership and isolate storage before model turns. Raw server logs and payload snapshots stay local; only reviewed status/identity/schema projections should be promoted.
