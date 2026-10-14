# Pi pane-local monitoring

This extension provides event-driven, metadata-only observation for **Pi 1.2.0 (`@earendil-works/pi-coding-agent`)** running in interactive TUI mode inside a verified Zellij terminal pane.

Owned lifecycle management (`setup pi`, `doctor pi`, `uninstall pi`) is implemented. **P4 local automated acceptance passes for compiled and Node Pi 1.2.0 in stock Zellij 0.45.1**: 74 checks with actual exporter inventory, two sidebars, host-local visits, fault recovery and resource measurements. See [retained evidence and qualification limits](../../prototypes/pi-acceptance/README.md). Forced PID/server reuse, serialized resurrection, sidebar keyboard selection stability and long-duration resource stress are not qualified by this matrix; no global deployment approval is inferred.

## Architecture and lifecycle

```text
Pi process / pane
  └─ Verij extension (index.ts + core.mjs): lifecycle events → metadata-only snapshots
       └─ verij agent pi: verified ownership + ordered source transactions
            └─ existing agent store / watcher / reducer
                 └─ existing tree, summaries, navigation, host-local Done
```

- **Process boundary**: Subprocesses and timers start only during `session_start` when `ctx.mode === "tui"` and in verified Zellij panes (`ZELLIJ_PANE_ID` 0..4294967295), never in the extension factory. If `session_start` rejects (e.g. print mode or invalid ID), an explicit lifecycle flag ensures all subsequent events no-op.
- **Typed event bindings**: Event listeners in `index.ts` are explicitly registered with Pi's typed `ExtensionAPI` and `ExtensionContext`, delegating nonblocking event observation to `bridge.handleEvent` without raw error body retention.
- **Adapter configuration**: At `session_start` in TUI mode only, reads bounded configuration (<= 16 KiB, schema validated, host `getAgentDir()` path resolution) from `<agent_dir>/verij-monitoring/adapter.json` placed sibling to the `extensions/` root directory (outside extension discovery). Zero filesystem I/O occurs in the extension factory or non-TUI modes. Explicit launch-time overrides (`VERIJ_BIN`, `VERIJ_AGENT_STATE_DIR`, `VERIJ_STATES_DIR`) always take precedence.
- **Private transport**: Spawns one `verij agent pi` reporter per active extension runtime with private stdin/stdout pipes (`stdio: ["pipe", "pipe", "ignore"]`). No protocol frames or error logs appear in the user's terminal.
- **Strict handshake & greetings**: On startup, sends a single `hello` frame correlating `runtime_id`, `version`, and `conversation_id`. The native reporter validates ownership, single-producer lock (`.pi.lock`), and monotonic source epochs. Ready greetings and receipts enforce strict whitelists and closed status enums (`idle`, `needs_input`, `working`, `done`, `error`, `unknown`).
- **Receipt correlation**: Snapshots wait for native transaction receipts verifying exact source epoch, monotonic next revision, and positive record revisions. A recurring five-second heartbeat renews the 15-second observer lease.
- **Framing**: Uses UTF-8 `StringDecoder` and caps input frame buffers at 256 KiB.
- **Process lifecycle & bounded teardown flush**: Listeners are installed before any stdin write. Spontaneous child crashes clear timers, requeue in-flight frames, and wait for process closure before backoff retries. Teardown performs a bounded nonblocking flush of already-observed semantic frames within total fixed grace (500ms), still receipt-correlated and rate-limited; once delivered or expired, closes stdin (EOF) and escalates via 500ms `SIGTERM` and 500ms `SIGKILL`. Reconnects are strictly prevented during teardown.
- **Context conversation changes**: Unexpected conversation ID changes or invalid identifiers during an active run or after `Done` immediately retire run, outcome, and prompt state to `Unknown`, reseeding a fresh runtime ID to ensure no prior completions are published under new associations.

## Lifecycle commands (P3)

Verij manages the Pi monitoring adapter through dedicated CLI commands:

```bash
# Install or refresh owned adapter assets into the resolved agent directory
verij agent setup pi [--config-dir <DIR>] [--pi-bin <PATH>]

# Diagnose installation integrity, runtime writability, and live pane monitoring
verij agent doctor pi [--config-dir <DIR>] [--pi-bin <PATH>] [--session <SESSION>]

# Cleanly uninstall owned adapter assets with conservative user preservation
verij agent uninstall pi [--config-dir <DIR>] [--pi-bin <PATH>]
```

### Directory resolution precedence

The Pi configuration directory is resolved using the exact upstream precedence:

1. Explicit `--config-dir <PATH>` CLI option.
2. `PI_CODING_AGENT_DIR` environment variable (if non-empty; empty string falls back to default, non-empty whitespace is preserved).
3. Default `~/.pi/agent` expanded relative to the user's home directory.

All paths are normalized and canonicalized to absolute local filesystem paths. Nonexistent path components with `..` resolution are normalized component-by-component, resolving existing prefixes via canonicalization while forbidding symlinks in owned extension and configuration subtrees.

> [!IMPORTANT]
> **Matching Launch Environment**: `PI_CODING_AGENT_DIR` is used to configure Pi's agent directory; for Verij lifecycle commands, pass the same path via `PI_CODING_AGENT_DIR` or `--config-dir <DIR>`. Note that `--config-dir` is a Verij CLI flag, not a Pi CLI flag.
>
> Shell example with custom directory:
> ```bash
> verij agent setup pi --config-dir /path/to/agent
> PI_CODING_AGENT_DIR=/path/to/agent pi
> ```
>
> **Local Filesystem Paths Required**: `file:` URLs (e.g. `file:///path/to/agent`) are explicitly rejected before any filesystem writes or operations; local filesystem paths must be provided.

### Owned asset layout

To prevent Pi's package loader from treating configuration as an extension, Verij places code and configuration in separate subtrees:

- **Autoloaded extension**: `<agent_dir>/extensions/verij-monitoring/`
  - `index.ts` — thin event bridge
  - `core.mjs` — pure projection and bounded transport
- **Sibling configuration & manifest**: `<agent_dir>/verij-monitoring/`
  - `adapter.json` — resolved absolute reporter and states/runtime paths
  - `installed.json` — typed ownership manifest

### Ownership manifest & adoption guards

The manifest `<agent_dir>/verij-monitoring/installed.json` enforces strict ownership:
- `schema_version: 1`
- `adapter_id: "verij-monitoring-v1"`
- `version: "1.2.0"`
- 16-hex lowercase noncryptographic FNV-1a 64-bit ownership fingerprints for `index.ts`, `core.mjs`, and `adapter.json`
- Canonical absolute reporter executable path
- `installed_at_ms` preserved across idempotent reinstallations

Preflight adoption checks require a valid manifest before touching any existing files. Unowned directories, empty `{}` or `null` manifests, foreign files, or corrupted manifests refuse adoption.

### Symlink safety

To prevent symlink attacks:
- The ancestor `extensions` directory and `<agent_dir>/verij-monitoring` must not be symlinks.
- Owned directories, subdirectories, manifests, config files, and code assets must not be symlinks (broken or valid).
- Operations reject any non-regular files (FIFOs, sockets, device nodes).

### Conservative preservation on uninstall

`verij agent uninstall pi` enforces conservative retention:
- **Code or extra files retained**: If any code asset (`index.ts`, `core.mjs`) is modified, or if any extra user file or dotfile exists in `extensions/verij-monitoring/` or `verij-monitoring/`, the entire code and configuration group is preserved untouched, the manifest is not marked uninstalled, and the command reports:
  ```json
  {
    "uninstalled": false,
    "restart_required": false,
    "preserved": ["adapter.json", "core.mjs", "index.ts", "installed.json"],
    "note": "Extension code, configuration, or extra files retained; adapter remains active"
  }
  ```
  *(Any extra user files or dotfiles trigger full group retention and remain preserved on disk alongside the standard assets).*
- **Modified configuration only**: If extension code is unmodified and neither directory contains extra files, but `adapter.json` was modified, clean extension code is removed while the modified configuration and manifest (marked `uninstalled: true`) are retained (`"uninstalled": true, "preserved": ["adapter.json", "installed.json"]`).
- **Clean removal**: Only when all assets are unmodified and neither directory contains extra files are all files removed and empty owned directories pruned (`"uninstalled": true, "preserved": []`).

### Doctor diagnostics

`verij agent doctor pi` evaluates:
1. **Binary availability & version**: Probes `pi --version` with a bounded nonblocking pipe, 4 KiB cap, dedicated process group cleanup, and a 3-second deadline. Qualified strictly for `1.2.0`.
2. **Structural installation readiness**: Validates ownership manifest integrity, asset hashes, reporter binary availability (`verij --version`), and runtime directory writability.
3. **Discovery overrides**: Evaluates `settings.json` against Pi's supported subset of `isEnabledByOverrides`:
   - `!` glob exclusions evaluated first against directory name `verij-monitoring` and relative path `extensions/verij-monitoring`.
   - `+` exact inclusions evaluated second (re-enabling matching extensions).
   - `-` exact exclusions evaluated third (order-independent; always overrides `+`). Minus patterns are literal strings, not globs.
   - Complex minimatch syntax (`@(`, `+(`, `!(`, `{`, `[`) is reported as uncertainty rather than a false block.
   - Project-level `.pi/settings.json` overrides are reported as uncertainties rather than absolute blockers.
4. **Hydrated inventory & live monitoring**:
   - Matches exact session incarnation (`session_instance_id == pane_key.session`).
   - Verifies foreground terminal job ownership via `/proc` controlling terminal ancestry (`verify_foreground`).
   - Verifies active observer lease: source `"pi"`, `"lease"` capability, observed timestamp within 15 seconds, matching source epoch/revision, non-empty `runtime_id`, and supported version `1.2.0`.
   - Distinguishes structural readiness (`installation_ready: true`) from actual verified live reporting (`adapter_loaded: true`).

## Capabilities

- **Conservative aggregate projection**: Treats one aggregate run ending at `agent_settled` as the completion unit. Low-level `agent_start/end`, model retries, tool execution cycles, and compaction do not mint intermediate completions.
- **Preserved terminal outcome**: A completed run outcome is preserved across subsequent session name changes and temporary UI prompt opens/closes, maintaining stable completion timestamps and IDs until the next genuine aggregate or branch invalidation.
- **Final outcome qualification**: Outcome candidates are captured at `agent_before_settle`, invalidated if work resumes (such as through extension continuation or tool execution), and finalized only at `agent_settled` with explicit `event.aborted === false`. Conflicting aborted flags or missing final-boundary evidence degrade conservatively to `Unknown`, never false `Done`.
- **Quiescence enforcement**: Settlement requires complete quiescence: zero in-flight tools, no active compaction, no pending requests, zero background tasks, and no pending messages in context. Conflicting nonquiescent states degrade safely to `Unknown`.
- **Metadata-only blocking UI**: Observes blocking dialogs (`confirm`, `select`, `input`, `editor`, `custom`) emitted through Pi's extension UI API. Emits generic `confirmation`, `question`, or `ui` categories with locally generated request UUIDs. Nested dialogs respect Pi's outer prompt model and never exceed a single pending request. Closing a prompt restores the previous execution state, while mid-run prompt settlements degrade safely to `Unknown` rather than sticking in working activity.
- **Strict privacy whitelist**: Snapshots carry only version, conversation ID/title, aggregate run ID, activity, pending request category/ID, background task count, and qualified outcome. Prompts, assistant text, tool arguments/results, dialog titles/content, provider bodies, and transcript paths are excluded.
- **Bounded transport**: 128-item total capacity FIFO queue with coalescing of identical consecutive refreshes. Rate limiter enforces at most 10 writes per second (interval >= 100ms) across all writes without bypassing priority frames. Queue overflow invalidates qualified projector state, reseeds blocking UI metadata to `NeedsInput` when locally provable, and prevents old completions from reasserting on heartbeats.
- **Fault tolerance and backoff**: Process exits and pipe errors trigger exponential backoff (2s, 4s, 8s, 16s, capped at 30s). Reconnects with the same runtime restore active state without replaying unread completion badges. In-flight frames are requeued and retried across reporter crashes.

## Exclusions & Limitations

- **Non-interactive modes**: Print, JSON, and RPC modes do not run the TUI reporter.
- **Untracked background work**: Extensions launching asynchronous background work outside Pi's tracked turn lifecycle remain outside aggregate completion guarantees.
- **Unobservable UI**: External browser approvals, raw terminal readers, third-party UI bypassing Pi's extension API, and assistant conversational questions are not observable and do not trigger `Needs input`.
- **Historical transcripts**: Resumed sessions do not replay historical completions.
- **Versions**: Qualified strictly for Pi 1.2.0. Other releases are unsupported.
- **Autoloading prerequisite**: Launching with `--no-extensions` or disabling `verij-monitoring` via settings overrides disables monitoring.
- **Explicit local paths**: Explicit filesystem paths are required; `file:` URLs are not supported.

## Recovery

- **Observer loss**: If the reporter process terminates or the pipe breaks, native lease expiry marks the process row as `Unknown` after 15 seconds, clearing pending requests while preserving row liveness and deduplication history.
- **Same-runtime reconnect**: If the reporter restarts within the same Pi process and conversation, the handshake restores previous epoch state (`restored: true`) without re-minting completion revisions.
- **Reload / Session replacement**: `/reload`, `/new`, `/resume`, or `/fork` trigger clean `session_shutdown`, tearing down the reporter and releasing `.pi.lock`. A new runtime ID ensures fresh state without historical badge replay.

## P4 end-to-end evidence

The [private runtime harness](../../prototypes/pi-acceptance/README.md) installs only into isolated agent directories and runs real Pi in verified stock-Zellij panes. It covers completion/error/abort, nested tools/dialogs, retries/compaction/queued continuation, same-process reconnect, idle reload, conversation replacement, two-host acknowledgements, pane targeting and process exits. Print/JSON/RPC/help/version/export and disabled discovery produce no reporter or extra row. Source traces and native state are retained only as sanitized summaries.

Short two-pane/two-host resource phases observe positive native write counters, five-second lease renewal, no idle visit proofs and bounded burst writes. Queue, timer and per-frame interval guarantees remain deterministic-test evidence, not runtime introspection or a long-run soak result.

## Verification

1. Isolated lifecycle CLI tests (16 tests in `verij-cli/tests/pi_setup.rs`):
```bash
cargo test -p verij-cli --test pi_setup
```
Covers: repeated setup/uninstall idempotency, unowned directory adoption rejection, dotfile and custom config preservation, custom path/tilde resolution, explicit `file:` URL rejection, symlink safety across parent and child trees, unsupported version rejection, missing reporter and `VERIJ_BIN` override, legacy/missing inventory, doctor structural readiness vs loaded live adapter, settings discovery exclusion overrides, manifest integrity error handling, and process group timeout/large output handling.

2. Deterministic event projection and P0 trace replay (35 tests):
```bash
node --test integrations/pi/core.test.mjs
```

3. Native reporter unit and integration test suite:
```bash
cargo test -p verij-types
cargo test -p verij-cli --test pi_reporter
cargo test -p verij-cli -- pi
```

4. Full workspace verification:
```bash
cargo test --workspace
make check
```
