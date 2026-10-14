# Codex CLI Monitoring

Verij monitors interactive OpenAI Codex CLI (`codex`, package `@openai/codex`) through a non-gating Python lifecycle callback adapter and an internal Rust reporter (`verij agent codex`).

## Setup and Diagnostics

### 1. Installation

Run setup to install adapter assets and configure Codex lifecycle hooks:

```bash
verij agent setup codex
```

Options:
- `--config-dir <PATH>`: Explicit Codex configuration directory (overrides `$CODEX_HOME` and `~/.codex`).
- `--codex-bin <PATH>`: Codex executable used for version qualification (defaults to `codex` on `PATH`).
- `--python-bin <PATH>`: Python 3 executable used in hook commands (defaults to `python3` on `PATH`).

Configuration directory resolution precedence:
1. `--config-dir <DIR>` (CLI argument)
2. `$CODEX_HOME` (Environment variable)
3. `$HOME/.codex` (Default global directory)

Setup installs:
- Assets under `<config_dir>/verij-monitoring-codex/`:
  - `callback.py`: Non-gating hook handler.
  - `adapter.json`: Runtime configuration and hook contract version; recorded binary device/inode is diagnostic metadata.
  - `installed.json`: Manifest of owned commands and asset fingerprints.
- Surgical entries in `<config_dir>/hooks.json` for 8 lifecycle events:
  `SessionStart`, `UserPromptSubmit`, `PreToolUse`, `PermissionRequest`, `PostToolUse`, `Stop`, `Interrupt`, `SessionEnd`.
- Preserves all existing top-level values, user groups, matchers, handlers, and `config.toml`.

### 2. Hook Trust Approval

Codex CLI requires interactive trust approval for configured hooks. In an interactive Codex session, run:

```text
/hooks
```

Review and approve the Verij lifecycle hook commands.

> [!IMPORTANT]
> Verij does not modify or bypass Codex hook trust. Hooks will not execute until approved by the user inside Codex.

### 3. Launching Codex

Launch Codex in interactive mode inside a Zellij pane:

```bash
codex --no-daemon
```

> [!IMPORTANT]
> The `--no-daemon` flag is required. Hooks may fire in daemon or headless subcommands (`app-server`, `exec`, `review`, `serve`, `daemon`), but the adapter skips modes without verified pane ownership to avoid unassociated process tracking.

### 4. Health Check (Doctor)

Verify installation integrity and runtime prerequisites:

```bash
verij agent doctor codex

# Or diagnose within a specific Zellij session:
verij agent doctor codex --session <SESSION_NAME>
```

Doctor checks:
- Codex binary availability and compatible release family (`>=0.160.1, <1.0.0`).
- Integrity of `callback.py`, `adapter.json`, `installed.json`, and `hooks.json`.
- Configured Python binary availability.
- Reporter binary availability and runtime directory writability.
- Verified Zellij pane inventory and active Codex instances.

Minor and patch upgrades within this release family do not require rebuilding
Verij or rerunning setup. Binary inode/version differences from installation
are not readiness failures. Executable ancestry, process birth and live foreground
pane ownership still establish identity, and hook payloads still undergo schema,
identifier and lifecycle validation. The installed version selects the callback
contract; it is not asserted to be the current binary version. Upgrading from an
older inode-pinned Verij callback requires one setup refresh of its assets.

### 5. Uninstallation

```bash
verij agent uninstall codex
```

Uninstall surgically removes Verij-owned hook handlers while preserving user hooks and custom groups. Unmodified assets are cleaned up. Any edited handlers or assets (including invalid UTF-8 bytes) are preserved, preventing dangling references. Repeat uninstall is safe and idempotent.

---

## Wire Schema and Lifecycle Events

The callback intercepts 8 lifecycle events and projects them to Verij agent states:

| Codex Hook Event | Verij Activity / Status | Transition Details |
|---|---|---|
| `SessionStart` | `Idle` (or preserved on `compact`) | Resets active turn; retires previous session unless `source=resume`. |
| `UserPromptSubmit` | `Working` | Begins generation; increments turn revision. |
| `PreToolUse` | `Working` | Continues generation or reopens generation after intermediate Stop. |
| `PermissionRequest` | `NeedsInput` | Generates unique request ID `codex-perm-{sample}` or preserves `tool_use_id`. |
| `PostToolUse` | `Working` | Clears matching pending request by ID or fallback tool name for generated requests. |
| `Stop` | `Done` (Idle + terminal completion) | Signals stop boundary. Error flags are normalized if present. |
| `Interrupt` | `Idle` | Cancels current turn. |
| `SessionEnd` | `Idle` | Retires active turn and clears session state. |

### Title Resolution

Codex does not emit distinct title hooks. In `config.toml`, title resolution defaults to `["pane", "conversation", "agent"]`:
- Active Zellij pane title takes precedence when set.
- Otherwise falls back to conversation session ID, and finally `"codex"`.

---

## Non-gating Guarantees and Privacy

- **Strict Non-gating**: The callback reads hook JSON up to 256 KiB, outputs empty JSON `{}` on stdout, and always exits `0`. Failures or timeouts in reporting never gate or interrupt agent execution.
- **Privacy Filtering**: Prompts, assistant message content, transcripts, model arguments, and tool payloads are discarded by the callback whitelist. Only clean identifier metadata (`session_id`, `turn_id`, `tool_name`, `tool_use_id`) is forwarded to the internal reporter.
- **Subprocess Isolation**: Reporter execution is bounded by a 1.5-second timeout and runs with stdout/stderr piped to `/dev/null`.

---

## Running Verification Tests

```bash
# Run full workspace test suite (270 passed)
cargo test --workspace

# Run targeted Codex lifecycle & doctor integration tests (20 passed)
cargo test -p verij-cli --test codex_setup

# Run targeted Codex reporter unit tests (11 passed)
cargo test -p verij-cli --bin verij codex::tests

# Run Python callback unit tests (12 passed)
python3 -m unittest discover -s integrations/codex
```

---

## Limitations and Qualification Matrix

- **Native Provider Errors Unsupported**:
  - Codex CLI Stop hooks do not contain failure status or provider error payloads. A provider 400 or 500 error does not emit a Stop hook.
  - Native terminal error detection is therefore unsupported (`native_terminal_errors_supported: false`).
  - As a result, previous `Working` or `NeedsInput` states can **remain stale** until another valid hook fires or the process terminates; provider failures do **not** automatically transition to `Idle` or `Error`.
- **Stop as Execution Boundary**: A Stop event marks a generation stop boundary that may be followed by third-party tool continuation or subsequent turns.
- **Hook Trust vs Installation Readiness**:
  - Verij doctor verifies that hook configurations and adapter assets are structurally intact (`installation_ready: true`).
  - However, Codex CLI enforces an independent internal trust check for hooks (`hook_trust_required: true`, `hook_trust_status: "unknown"`).
  - Users must interactively approve hooks with `/hooks`. Verij does not modify or bypass this trust mechanism.
- **Supported vs Qualified Versions**:
  - `0.162.1`: Fully qualified. Verified in isolated TUI PTY sessions with real `/proc` foreground verification and synthetic inventory (`Idle` → `Working` → `NeedsInput` → `Working` → `Done`, harmless `true` command approved).
  - `0.160.1`: Exec hooks qualified via local fake HTTP provider. TUI initialization on legacy `0.160.1` app-server binary stalled before interactive rendering; do not rely on `0.160.1` for interactive TUI.
- **Temporary Verification Harness**:
  - Isolated verification scripts `/tmp/opencode/codex_hook_smoke.py` and `/tmp/opencode/codex_tui_smoke.py` were used as temporary test harnesses during review, not durable repo fixtures.
  - Live Zellij sidebar visual verification in a running multi-pane layout remains pending manual parent review.
