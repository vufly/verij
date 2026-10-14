# Verij Progress

Verij currently provides a working nested-session host for Zellij. Each host session has a persistent sidebar and Workspace pane; the Workspace pane remembers its last inner-session attachment independently from other hosts.

## Implemented

### Host And CLI

- `verij start` creates or attaches to an exact-name registered host session.
- `verij attach` attaches to an existing host session or creates one with `--create`.
- `verij ui` runs the Ratatui sidebar.
- Host KDL comes from editable XDG config `verij.kdl`; cache rendering inserts native CLI path and sidebar width, with no host state-export plugin.
- Plugin path resolution supports CLI argument, environment variable, executable-relative, development, user, and system paths.
- `verij config init` and first `verij start` create missing layout template without overwriting user edits; `verij config path` prints config path; `verij config zellij` writes or inspects generated host config.
- Hosts use complete generated Zellij config with scalar `[zellij]` root-node overrides, defaulting pane frames to `titles` and focus-following-mouse to `true`; inner sessions inherit normal Zellij config unchanged.

### Sidebar And Navigation

- Live session and tab tree with filesystem-backed updates.
- Host filtering by cached registration, independent of session-name prefix.
- Sidebar `R` renames the current host while retaining Workspace attachment identity.
- Keyboard navigation, paging, folding, mouse selection, double-click actions, help view, and resize handling.
- Resize redraw avoids cursor-position queries, keeping sidebar alive when Zellij temporarily delays terminal replies.
- Active session and active tab styling.
- New inner session creation from the sidebar.

### Distributed State

- One headless WASM agent runs in each inner session.
- Agents export atomic JSON snapshots to `/tmp/verij/states/`.
- Snapshots include tabs, active pane title, and optional connected-client compatibility data.
- CLI watcher aggregates snapshots and prunes stale state files.
- Old snapshots without optional fields remain readable.

### Workspace Switching

- Inception Switch uses `verij_control` and native Zellij `switch_session`.
- Tab navigation uses targeted `go-to-tab` actions.
- Workspace pane title follows configured session, tab, and pane values.
- Conditional pane formats omit empty tab/pane separators.
- Workspace attachment uses `VERIJ_WORKSPACE_SESSION` plus a host marker file.
- Detach cleanup, restart recovery, and legacy pane-title recovery avoid nested duplicate attaches.
- Durable host metadata records the last inner session attached to each host.
- Separate `host-registry.toml` registers hosts even when they have no inner attachment.
- Live and exited Zellij hosts and inner sessions are distinguished during attach.
- Exited hosts and inner sessions can be resumed through Zellij session resurrection.
- Switching to an exited inner session force-resurrects it before the native switch so saved commands start without an Enter prompt.
- Resurrection sanitizes stale suspended-command flags and waits for tiled `zjstatus` on every restored terminal tab before detaching.
- Host resurrection rewrites stale serialized Workspace-pane attach commands to the durable target.

### Inner Session Initialization

- Fake-PTY initialization avoids Zellij 0.45 no-viewport layout failures.
- Creation waits for the first tab's configured status plugin and a terminal pane to stabilize, then verifies the plugin survived fake-client detach.
- Plugin fallback launches the agent when automatic layout loading does not produce state.

### Pi Monitoring — P0–P3 Complete; P4 Acceptance Pending

- The [Pi monitoring plan](docs/PI_MONITORING_PLAN.md) is approved; P0, P1, P2, and P3 are complete, while P4 acceptance remains pending.
- P0 private runtime qualification: 112 real-TUI scenario runs, 1,608 sanitized lifecycle records across compiled/Bun and npm/Node Pi 1.2.0.
- P1 native implementation:
  - Added `AgentKind::Pi` and explicit `pi` source authority (priority 10) in `verij-types/src/agent.rs`, preserving wire compatibility.
  - Added `[agents.pi]` title override in `verij-cli/src/config.rs`.
  - Added hidden CLI dispatcher `verij agent pi` in `verij-cli/src/agent_cli.rs`, guarded by `.pi.lock` nonblocking single-producer fencing and global session resolution.
  - Reserved source `"pi"` in `agent_store::report` against generic report bypass.
  - Strict metadata-only whitelist and closed enums in `verij-cli/src/pi.rs`, enforcing zero unknown fields, max 1 pending request, no permission provenance or dialog content, and quiescent terminal idle outcomes.
  - Atomic handshake state claim, monotonic ordering, turn receipts correlating epoch/revision, and deduplication of completed runs scoped by structured tuple key `(conversation_id, run_id)`. Bounded completions ledger with conservative degradation to Unknown on saturation.
  - Native observer lease expiry in `agent_watcher.rs` clears background task count and pending requests, retaining live process row as Unknown.
  - Full test verification: 29 `verij-types` tests, 17 `pi::tests` unit tests, `agent_watcher` lease expiry unit test, `tree_format` actual Pi row test, 8 isolated CLI integration tests with controlling PTY in `pi_reporter.rs`.
- P2 extension & bounded transport:
  - Pure event-state projection (`integrations/pi/core.mjs`) and thin typed event bindings (`integrations/pi/index.ts`).
  - Strict whitelist, bounded nonblocking FIFO transport (128-item queue), rate-limiting (<= 10 writes/s), exponential backoff (2s–30s), 5-second lease renewal heartbeat, and bounded nonblocking teardown flush (500ms grace) with SIGTERM/SIGKILL escalation.
  - Outcome qualification finalized strictly at `agent_settled` with `aborted === false` and complete quiescence.
  - 35 deterministic unit tests and full trace replay across all 112 P0 scenario runs in `core.test.mjs`; strict TypeScript check passes against private Pi 1.2.0 declarations.
- P3 lifecycle management, CLI commands, and doctor diagnostics:
  - Embedded owned assets: `index.ts` and `core.mjs` embedded in CLI binary; written atomically into `<agent_dir>/extensions/verij-monitoring/`.
  - Sibling configuration & manifest: `<agent_dir>/verij-monitoring/{adapter.json, installed.json}` outside `extensions/` discovery.
  - Typed manifest `installed.json` (`schema_version: 1`, `adapter_id: "verij-monitoring-v1"`, version `"1.2.0"`, 16-hex lowercase noncryptographic FNV-1a 64-bit ownership fingerprints, bounded absolute reporter path, `deny_unknown_fields`). Byte-idempotent reinstallation preserves initial timestamp.
  - Preflight adoption guards: unowned directories, missing/invalid manifests, foreign files, or corrupt manifests refuse adoption.
  - Symlink safety: rejects symlinks, broken symlinks, non-regular files, and FIFOs across ancestor and child trees.
  - Conservative preservation: code modification or extra user files/dotfiles retain entire code and configuration group intact (`uninstalled: false`); modified configuration only removes clean code while retaining custom config and manifest (`uninstalled: true`); clean state removes all files and prunes empty owned directories.
  - Path resolution: `--config-dir` > `PI_CODING_AGENT_DIR` > `~/.pi/agent` with canonical absolute paths, resolving existing prefixes while forbidding symlinks in owned extension and configuration subtrees; Pi launch uses `PI_CODING_AGENT_DIR`, while Verij CLI supports `--config-dir` and `PI_CODING_AGENT_DIR`; explicit local filesystem paths required; `file:` URLs rejected before writes.
  - Doctor diagnostics: distinguishes structural installation readiness from live loaded adapter; verifies live foreground terminal job ownership against hydrated inventory using exact session incarnation; checks active observer lease; mirrors supported subset of Pi 1.2.0 discovery overrides contract (`!`, `+`, `-`), reporting complex minimatch syntax as uncertainty.
  - Bounded version probe: nonblocking pipe with validated `fcntl` flags, 4 KiB cap, dedicated process group cleanup (`setpgid`), and 3-second deadline.
  - Verification: 16 isolated tests in `verij-cli/tests/pi_setup.rs`, 3 unit tests in `pi_setup::tests`, 234 CLI tests, workspace tests, and `make check` all pass.

## Verification

Current workspace checks:

```bash
cargo check --workspace
cargo test --workspace
```

The repository currently passes both checks. Focused tests cover host registration/rename, first-tab readiness, host-keyed metadata, and live/exited session classification. Release artifacts are built with:

```bash
make dev
```

## Limitations

- Verij assumes its sidebar and Workspace pane use the generated two-pane host layout.
- Custom layouts with different geometry may require compatible pane placement.
- The Workspace marker represents Verij-controlled attachment; arbitrary external manipulation of the right pane is outside the supported flow.
- Generated layouts and host configs are cache data under `$XDG_CACHE_HOME/verij` or `~/.cache/verij`; Workspace markers and plugin snapshots remain runtime data.
- Durable attachment metadata and host registry are stored under `$XDG_STATE_HOME/verij` or `~/.local/state/verij`, migrating legacy config-adjacent state automatically.
- Zellij `serialize_pane_viewport` and scrollback serialization must be enabled to preserve terminal history.

## Roadmap

- Agent monitoring H0–H2 is approved. Current H3 [OpenCode monitoring](integrations/opencode/README.md) and H4 [Agy/Magy monitoring](integrations/agy/README.md) are accepted for master integration and global installation. H4 passes 19 live checks and the ten-check H2 regression; 216 Rust tests and native/WASM checks pass. The [feature progress log](docs/AGENT_MONITORING_PROGRESS.md) records evidence and acceptance; the full H5 release matrix remains separate work.
- Validate live host rename after restarting existing sidebar with new binary; local `🔷v` → `v` migration was done once outside the product.
- Add end-to-end coverage for host resurrection and inner-session resurrection.
- Add coverage for manual Workspace detach, multiple inner clients, multiple hosts, and custom layouts.
- Improve status reporting for missing or externally deleted remembered inner sessions.

The previous named-workspace lifecycle proposal was superseded. A Verij workspace is the host Zellij session plus its current Workspace-pane attachment; `verij start` and `verij attach` remain the lifecycle commands.
