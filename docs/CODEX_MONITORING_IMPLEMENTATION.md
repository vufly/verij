# Codex Monitoring Implementation & Qualification Summary

This document records the architecture, development gates, verification matrix, operational usage, and qualification status for the OpenAI Codex CLI monitoring subsystem (Steps 1–4).

---

## 1. Architecture Overview

Codex monitoring observes interactive Codex CLI instances running inside Zellij panes via lifecycle hooks and a dedicated internal reporter:

1. **Callback Adapter (`integrations/codex/callback.py`)**:
   - Python 3 hook handler invoked by Codex CLI lifecycle hooks.
   - Discovers foreground interactive ancestor process (`codex` or `codex.*`) via `/proc` traversal.
   - Validates controlling TTY and interactive terminal streams (pts/tty stdin/stdout).
   - Skips headless or background execution modes (`app-server`, `exec`, `review`, `serve`, `daemon`) that lack verified Zellij pane ownership. Note: hooks may fire in those modes, but the adapter intentionally ignores them to prevent foreign process contamination.
   - Validates command line arguments before reading stdin; reads hook stdin bounded to 256 KiB; discards sensitive payload content (prompts, assistant responses, model parameters, tool arguments).
   - Validates clean string identifiers (`session_id`, `turn_id`, `tool_name`, `tool_use_id`).
   - Dispatches normalized JSON payload to `verij agent codex` internal reporter with a 1.5-second deadline.
   - Non-gating guarantee: always outputs `{}` to stdout and exits `0` on all paths (acknowledging without falsifying permit success or input approval).

2. **Internal Reporter & Projection (`verij-cli/src/codex.rs`)**:
   - Validates process lifetime and foreground pane ownership via `verify_foreground`.
   - Projects observations into `PartialSourceRecord` for source `"codex"`.
   - Maintains journal correlation state: monotonically increasing samples, turn revisions, active session binding, retired sessions set, retired turns set.
   - State machine transitions:
     - `SessionStart`: `Idle` (retires previous turn and session; unretires on explicit `source=resume`; preserves working/terminal on `source=compact`).
     - `UserPromptSubmit`: `Working` (starts new turn or increments revision).
     - `PreToolUse`: `Working` (continues or reopens generation after intermediate Stop).
     - `PermissionRequest`: `NeedsInput` with unique ID `codex-perm-{sample}` or native `tool_use_id`.
     - `PostToolUse`: `Working` (clears matching request by ID; fallback by tool name matches only generated requests).
     - `Stop`: `Done` (marks completion boundary; error flags normalized if present; duplicate stops within turn deduplicated).
     - `Interrupt`: `Idle` (cancels active turn).
     - `SessionEnd`: `Idle` (retires turn).

3. **Lifecycle Management (`verij-cli/src/codex_setup.rs`)**:
   - `setup`:
     - Checks Codex binary version with 3s timeout against allowlist (`0.160.1`, `0.162.1`).
     - Resolves config directory (`--config-dir` > `$CODEX_HOME` > `~/.codex`).
     - Prechecks hook schema and refuses unmanifested command collisions.
     - Writes embedded `callback.py`, `adapter.json`, and ownership manifest `installed.json` under `<config>/verij-monitoring-codex/`.
     - Surgically installs handlers for all 8 lifecycle events in `<config>/hooks.json`.
     - Detects in-memory command updates (e.g. `--python-bin` change) and writes both `hooks.json` and `installed.json`.
     - Preserves all unrelated top-level keys, user groups, matchers, handlers, and `config.toml`. Enforces byte idempotence when unchanged.
   - `uninstall`:
     - Non-destructively removes unmodified owned command handlers and empty groups.
     - Preserves user handlers in shared groups and any user-edited handlers.
     - Preserves assets when retained handlers point to them across all events (including shell-quoted paths with spaces/quotes).
     - Preserves assets on invalid UTF-8 bytes or manual modifications.
     - Idempotent and safe for repeat executions.
   - `doctor`:
     - Inspects binary availability, device/inode match against installed adapter, and version compatibility.
     - Verifies manifest validity, 16-hex fingerprints, configured Python binary availability, callback integrity, adapter status, and exact hook group/handler consistency.
     - Probes effective runtime directory (`VERIJ_AGENT_STATE_DIR` > `adapter["runtime"]` > default) and reports `runtime_writable: false` cleanly without failing on blocked paths.
     - Probes effective inventory directory and counts active Codex instances matching pane inventory.

---

## 2. Review and Rework History (Stepwise Gates)

The implementation followed strict writer → review → rework → approval gates:

1. **Step 1: Types & Reducer Foundations**:
   - Implemented `verij-types/src/agent.rs` and `verij-cli/src/config.rs`.
   - Verified reducer contracts and configuration defaults.
   - Gate status: **Approved**.

2. **Step 2: Reporter, Projection & Python Callback**:
   - Initial implementation submitted. Static review and parent smoke tests identified edge-case defects:
     - Non-unique permission IDs on successive requests caused collisions.
     - `SessionStart` left lingering `turn_id`s, allowing late callbacks to revive stale turns.
     - Resuming older sessions was rejected by retired session set.
     - Unbounded cmdline reads risked memory bloat.
     - Blocked tests hung when invalid CLI args were given while stdin was open.
     - Native `PermissionRequest` omitted `tool_use_id`, breaking clearance without fallback matching.
   - Rework:
     - Implemented unique permission IDs (`codex-perm-{sample}`) and restricted fallback tool matching strictly to generated IDs.
     - Validated CLI arguments before reading stdin in `callback.py`.
     - `SessionStart` (`startup`, `resume`, `clear`) retires the previous turn and clears active turn state.
     - `SessionStart` with `source=compact` preserves active working, permission, and terminal states.
     - Resuming an older session (`source=resume`) unretires it without fabricating false terminal states.
     - Stale events from earlier turns or sessions rejected.
     - Duplicate `Stop` deduplicated; same-turn `PreToolUse` continuation reopens generation.
     - Bounded cmdline reads to 65,537 bytes (rejecting >65,536).
     - Normalized `ExecutionError` capability documentation.
   - Gate status: **Approved** (11 reporter tests, 12 Python callback tests, parent native 0.162.1 TUI smoke).

3. **Step 3: Lifecycle Management (Setup, Uninstall, Doctor & Tests)**:
   - Initial implementation submitted. Review rounds identified 5 ownership, readiness, and uninstall defects:
     - **Defect A**: Updating `--python-bin` mutated command in memory but did not persist changes to `hooks.json` when event set was unchanged. Fixed by comparing `hook_root != initial_hook_root`.
     - **Defect B**: Setup adopted existing commands without manifest. Entire handler comparison was missing (risked stripping fields like `additionalContext` or `statusMessage`). Doctor accepted lookalike commands. Fixed with strict collision refusal, whole-handler comparison, and single-owned-handler checks.
     - **Defect C**: Uninstall retained-reference detection missed shell-quoted paths containing single quotes and extra-event command references. Invalid UTF-8 bytes were treated as unmodified and deleted. Fixed by scanning raw, shell-quoted, and manifest command values across all events, and preserving invalid UTF-8 assets.
     - **Defect D**: Doctor probed default runtime instead of adapter-configured `effective_runtime`, and lacked Python binary executable validation. Fixed by probing effective runtime (reporting `runtime_writable: false` gracefully on blocked paths) and validating manifest `python_bin`.
     - **Defect E**: Shared inventory detail generalized from OpenCode-specific phrasing to generic foreground agent TUI.
   - Added 7 focused regression tests covering all 5 review defects (expanding integration suite to 20 tests).
   - Gate status: **Approved** (20/20 setup integration tests passed, full workspace 270/270 tests passed).

4. **Step 4: Operational Documentation & Qualification Matrix**:
   - Documented operational commands, testing commands, qualification matrix, and limitations.
   - Code approved and verification completed.
   - Final status: **Approved**; manual real Zellij sidebar visual check pending.

---

## 3. Important Fixes and Hardened Behaviors

| Subsystem | Issue / Edge Case | Hardened Behavior |
|---|---|---|
| Callback | CLI argument validation order | Validates command line arguments BEFORE reading stdin; prevents test suite hangs and blocked stdin reads on invalid arguments. |
| Callback | Non-gating response contract | Writes `{}` to stdout and exits 0 on all code paths; outputs `{}` acknowledgement rather than claiming false permit success or falsifying input approval. |
| Callback | Unbounded argument/stdin handling | Reads stdin bounded to 256 KiB; `_read_cmdline` bounded to 65,537 bytes; rejects missing or unknown `SessionStart` sources. |
| Reporter | Native PermissionRequest payload omission | Native Codex `PermissionRequest` omits `tool_use_id`; reporter synthesizes unique monotonic ID `codex-perm-{sample}` and requires fallback tool name matching. |
| Reporter | Fallback tool matching scope | Fallback `PostToolUse` tool name matching is strictly restricted to generated `codex-perm-` IDs; never clears unrelated explicit pending requests with the same tool name. |
| Reporter | Stale event rejection | Callbacks with mismatched or retired `turn_id`s (`PreToolUse`, `PostToolUse`, `PermissionRequest`, `Interrupt`) are strictly rejected; cannot revive previous turns. |
| Reporter | Repeated Stop deduplication & turn continuation | Duplicate `Stop` events within the same turn are deduplicated; `PreToolUse` arriving after `Stop` in the same turn clears `Done` and reopens generation with an incremented turn revision. |
| Reporter | Session resumption without false terminal | Explicit `SessionStart` with `source=resume` unretires an older session, clears terminal state, and retires the previous current session. |
| Reporter | Turn retirement on startup/resume/clear | `SessionStart` (`startup`, `resume`, `clear`) retires the prior turn and resets active turn state; `compact` source preserves active working/pending/terminal state. |
| Setup | Unmanifested collision adoption | Preflight strictly rejects existing matching commands if `installed.json` is missing or invalid. |
| Setup | Silent stripping of handler fields | Compares entire handler `*h == expected_handler(cmd)`; refuses setup if user added `additionalContext`, `statusMessage`, or `matcher`. |
| Setup | `--python-bin` update persistence | Compares `hook_root != initial_hook_root`; writes updated commands to `hooks.json` and updates `installed.json`. |
| Uninstall | Shell-quoted paths with spaces/quotes | Retained-reference scanner checks `quote(path)`, raw paths, and exact manifest commands across all events. |
| Uninstall | Corrupted / invalid UTF-8 assets | Non-destructive cleanup treats invalid UTF-8 bytes as customized assets (`callback_edited = true`) and preserves them. |
| Doctor | Effective runtime resolution | Probes `effective_runtime` from `VERIJ_AGENT_STATE_DIR` > `adapter["runtime"]` > default; handles file-blocked paths cleanly as `runtime_writable: false`. |
| Doctor | Configured Python verification | Validates `is_executable(&manifest.python_bin)`; reports `python_available: false` and `installation_ready: false` if missing. |

---

## 4. Test Verification Matrix

All tests execute cleanly without network access or host config alterations:

| Test Suite / Target | Location / Path | Tests Passed | Status |
|---|---|---|---|
| `verij-types` crate unit tests | `verij-types/src/` (`agent::tests` subset 19) | 25 / 25 passed | Verified |
| `verij-cli` crate unit tests | `verij-cli/src/` (`config::tests` 14, `codex::tests` 11 included) | 202 / 202 passed | Verified |
| `verij-cli` host integration tests | `verij-cli/tests/host_cli.rs` | 15 / 15 passed | Verified |
| `verij-cli` Codex integration tests | `verij-cli/tests/codex_setup.rs` | 20 / 20 passed | Verified |
| `verij-cli` Agy integration tests | `verij-cli/tests/agy_setup.rs` | 2 / 2 passed | Verified |
| `verij-cli` OpenCode integration tests | `verij-cli/tests/opencode_setup.rs` | 1 / 1 passed | Verified |
| `verij-plugin` crate unit tests (native) | `verij-plugin/src/` (native unit, not WASM integration) | 5 / 5 passed | Verified |
| **Cargo Workspace Total** | `cargo test --workspace` | **270 / 270 passed** | **Verified** |
| Callback Python contract tests | `integrations/codex/test_callback.py` | 12 / 12 passed | Verified |

> [!NOTE]
> The Cargo workspace total of 270 passed tests covers the 7 test suites compiled by Cargo (25 + 202 + 15 + 20 + 2 + 1 + 5 = 270). It does not double-count module subsets (such as the 19 agent reducer tests in `verij-types` or the 11 Codex reporter unit tests in `verij-cli`) and excludes the 12 Python unittest contract tests.

### Runnable Test Commands

```bash
# Run full workspace Rust tests (270 passed)
cargo test --workspace

# Run targeted Codex integration tests (20 passed)
cargo test -p verij-cli --test codex_setup

# Run targeted Codex reporter unit tests (11 passed)
cargo test -p verij-cli --bin verij codex::tests

# Run Python callback unit tests (12 passed)
python3 -m unittest discover -s integrations/codex

# Workspace compiler check (0 errors, 0 warnings)
cargo check --workspace
```

---

## 5. Operational Usage

### 1. Installation

Install Codex monitoring assets and register lifecycle hooks:

```bash
verij agent setup codex
```

Optional arguments:
- `--config-dir <DIR>`: Custom Codex configuration directory (overrides `$CODEX_HOME` and `~/.codex`).
- `--codex-bin <PATH>`: Path to Codex executable (defaults to `codex` on `PATH`).
- `--python-bin <PATH>`: Path to Python 3 executable (defaults to `python3` on `PATH`).

### 2. Interactive Hook Trust Approval

Codex CLI maintains an internal trust mechanism for hooks. Open an interactive Codex session and execute:

```text
/hooks
```

Review and approve the Verij lifecycle hooks.

> [!IMPORTANT]
> Verij does not modify or bypass Codex hook trust. Hook execution will be blocked by Codex CLI until approved via `/hooks`.

### 3. Launching Codex

Launch Codex in interactive mode inside a Zellij pane:

```bash
codex --no-daemon
```

> [!NOTE]
> Codex must run with `--no-daemon` in an interactive terminal. Hooks may fire in daemon or headless modes (`app-server`, `exec`, `review`, `serve`, `daemon`), but the Verij callback intentionally skips execution modes without verified Zellij pane ownership.

### 4. Diagnostics (Doctor)

Check installation integrity, asset fingerprints, runtime writability, and active instances:

```bash
verij agent doctor codex

# Or diagnose within a specific Zellij session:
verij agent doctor codex --session <SESSION_NAME>
```

A healthy installation reports: `installation_ready: true`, `python_available: true`, `hooks_installed: true`, and `runtime_writable: true` when prerequisites are satisfied. Inventory, active instances, and hook trust status reflect actual runtime state (e.g. `hook_trust_status: "unknown (manual /hooks needed)"` until approved).

### 5. Uninstallation

Remove Verij lifecycle hooks and clean up unmodified assets:

```bash
verij agent uninstall codex
```

Preserves user-edited handlers, user groups, and assets referenced by any remaining custom hooks.

---

## 6. Qualification Matrix and Truthful Scope

| Feature / Scenario | Codex 0.162.1 | Codex 0.160.1 | Evidence & Notes |
|---|---|---|---|
| CLI Version Qualification | **Qualified** | Supported | Verified via `codex --version` matching `codex-cli <version>`. Rust synthetic parser accepts both versions, but this is not native installation qualification. |
| Exec Hook Delivery | **Verified** | **Verified** | Local fake HTTP provider verified `SessionStart`, `UserPromptSubmit`, `Stop`, `SessionEnd`. |
| Tool Hook Delivery | **Verified** | Untested | `PreToolUse` and `PostToolUse(Bash, tool_use_id)` verified on 0.162.1. |
| Interactive TUI Execution | **Qualified** | **Not Qualified** | 0.162.1 PTY session with native `/proc` foreground ownership and fixture inventory drove reporter transitions: `Idle` → `Working` → `NeedsInput` → `Working` → `Done`. Legacy 0.160.1 app-server binary stalled before TUI rendering. |
| Setup-Generated Assets | **Verified** | Untested with native binary | `verij agent setup codex` generated assets verified on host in isolated `CODEX_HOME` for 0.162.1. Native 0.160.1 setup execution was not run by parent. |
| Uninstallation Cleanup | **Verified** | Untested with native binary | `verij agent uninstall codex` cleaned up hooks and assets cleanly on 0.162.1. Native 0.160.1 uninstall execution was not run by parent. |
| Live Zellij Sidebar Render | **Pending** | **Pending** | Host live visual validation of sidebar UI in a multi-pane layout is pending final manual review. |

### Temporary Verification Harness Note

Parent verification scripts `/tmp/opencode/codex_hook_smoke.py` and `/tmp/opencode/codex_tui_smoke.py` were used during review as an isolated, temporary test harness. They are host-local verification artifacts and are not claimed as durable repository fixtures. Host user configuration (`~/.codex`) was left completely untouched.

---

## 7. Known Limitations

1. **Native Provider Errors**:
   - Codex CLI Stop hooks do not include failure fields or HTTP error status.
   - Provider errors (such as HTTP 400/500) do **not** trigger a Stop hook.
   - As a result, previous `Working` or `NeedsInput` states can **remain stale** until another valid hook fires or the process terminates.
   - Provider failures do not automatically transition the agent to `Idle` or `Error`.
2. **Hook Trust Requirement**:
   - Codex CLI enforces interactive hook trust. Verij cannot programmatically bypass or pre-approve hooks; the user must run `/hooks` in Codex.
3. **Interactive Mode Requirement (`--no-daemon`)**:
   - Codex must run with `--no-daemon`. While hooks may fire in daemon or headless modes, the adapter intentionally skips modes lacking verified Zellij pane ownership.
4. **Execution Stop Boundary**:
   - A `Stop` event signals a generation stop boundary, which may be followed by tool execution continuation or additional turns within the same session.
5. **Version Qualification**:
   - Interactive TUI qualification applies strictly to `0.162.1`. `0.160.1` is supported for exec hooks, but its interactive TUI stalled during qualification.
