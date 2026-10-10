# Agy and Magy monitoring

Verij monitors interactive Agy from its status-line callback and non-gating
`PreInvocation`, `PostInvocation`, and `Stop` hooks. Magy watched panes use their
existing runner state and streaming log, independently of interactive callbacks.

## Install and diagnose

Build Verij and upgrade the inner-session exporter first:

```bash
make dev
make install-plugin
# Reload the same Verij plugin URL configured in each existing inner session.
zellij -s SESSION action start-or-reload-plugin file:/absolute/path/verij_plugin.wasm

target/release/verij agent setup agy
target/release/verij agent doctor agy --session SESSION
```

Restart Agy after setup. If a host sidebar was already running when Verij was
rebuilt, restart that sidebar too: replacing the executable file does not update
the code loaded by an existing process. An older sidebar can display a registered
Agy row while refusing its renderer ownership on activation.

`--agy-bin /absolute/path/agy` selects the executable
whose version is inspected. Supported contracts: **1.2.14, 1.3.2, and 1.3.3**.
The H4 production fixture verifies 1.3.3; 1.3.2 has earlier scoped live evidence,
and 1.2.14 supplies the retained H0 payload contract. Setup records the binary's
device/inode as well as its detected version. After an Agy binary upgrade, run
setup again; stale installation metadata does not silently enable a new version.

Requirements: Linux `/proc`, Python 3, stock Zellij 0.45.1-compatible exporter,
and the same runtime/inventory paths in the agent and sidebar. Python and Verij
paths are absolute in installed commands. Setup defaults to `$HOME/.gemini`;
`--config-dir /path/to/.gemini` selects another home. It preserves unrelated
settings/hooks and existing status-line command, padding, enablement and stacking.
With no custom command, the empty callback stacks with the native default line.
Setup does not install `PreToolUse`, change permissions, or answer questions.

The previous command receives its original stdin, cwd and environment; its stdout,
stderr and exit code are preserved. Reporting has a 1.5-second deadline and no
visible output. Missing reporting cannot gate an agent turn. Neutral hook output
is `{}` for invocation hooks and `{"decision":""}` for Stop.

```bash
target/release/verij agent uninstall agy
```

Uninstall restores the predecessor only while the installed entry still matches.
Edited user entries are preserved. Small disabled passthrough assets remain so
Magy homes with copied absolute commands keep their predecessor/neutral output.
An untouched reinstall is idempotent; edited Verij entries require preserving or
restoring those edits before setup can proceed. Adapter assets are not recursively
wrapped.

## Magy profiles and watches

Magy copies settings and hooks into profile homes. Absolute commands can continue
using the original shared Verij assets/runtime. For an existing profile, refresh
its settings through Magy's normal settings-copy workflow, or install directly:

```bash
target/release/verij agent setup agy --config-dir /path/to/profile-home/.gemini
target/release/verij agent doctor agy --config-dir /path/to/profile-home/.gemini
```

Doctor resolves copied commands to their actual shared assets. It checks version,
binary identity, installation, enablement, reporter, writable runtime and native
inventory separately. Installation alone is not proof of live reporting.

Every running sidebar reconciles watched panes once per second. For reporting
without a sidebar:

```bash
target/release/verij agent magy
target/release/verij agent magy --once  # bounded diagnostic summary
```

Magy state resolution: `VERIJ_MAGY_STATE_DIR`, then `MAGY_STATE_DIR`, then the
normal XDG state directory (`$XDG_STATE_HOME/magy` or `~/.local/state/magy`). Set an
absolute override when the sidebar and Magy use different homes. Verij reads only
`watches/watch_<16 hex digits>/state.json` and that watch's `pty.log`, never
`request.json` or its prompt. A custom multiplexer PID is not accepted as a Zellij
terminal ID.

Binding requires native server/pane identity, actual foreground renderer tty,
runner PID and birth, the exact `magy.watch_runner` directory, and its direct
child's `MAGY_WATCH_ID`/`MAGY_RUN_ID`. Saved byte offsets and log inode/device
identity survive observer/sidebar restarts. Partial, oversized, rotated and
truncated log frames are handled conservatively. Only short IDs and outcome facts
are retained; no prompts, command arguments/output, account details or transcripts
enter agent records. Terminal watch state supersedes the Agy stream result;
post-run Git processing can fail after stream success. A completed or cancelled
watch disappears when its pane/renderer exits, without retaining a visible Done
tombstone. Detached pane-less runs never register on their launcher.

## Capabilities and limits

| Observation | Behavior |
|---|---|
| Startup/authentication | Presence with Unknown; no fabricated completion. |
| Thinking/working/tool use | Working. |
| Explicit native confirmation pending | Needs input; native permission semantics are preserved. |
| Background tasks or Stop `fullyIdle=false` | Working; aggregate Done is deferred. |
| Correlated invocation pair and successful Stop `fullyIdle=true` | One completion revision. |
| Error-bearing correlated terminal Stop | Error with a fixed reason; provider error bodies are discarded. |
| UI Idle without qualified Stop | Idle only with sufficient facts, otherwise Unknown; never Done. |
| Process exit/pane close | Remove row. |

Ready-state payloads omit false permission flags in tested versions; a current
Working/Thinking/Idle callback resolves its callback-owned permission. Startup
omissions do not establish input/task quiescence. Execution counters can repeat,
so adapter-owned generations and source-specific monotonic samples qualify
outcomes. UI refreshes cannot discard a correlated Stop from the other source.
Old invocation pairs, foreign conversations and older-generation callbacks are
rejected. Arbitrary concurrent upstream executions without enough correlation
remain unsupported; a lock alone is not semantic ordering proof.

Accepted limits remain explicit:

- No verified neutral structured question source. A native question can still
  appear Working; no terminal-text heuristic or permission-changing hook is used.
- Ctrl+C may produce Idle without Stop. Idle cannot establish successful completion.
- Unsupported stop reasons and uncorrelated startup/mid-turn outcomes do not create Done.
- Agy conversation titles are unavailable from the retained metadata; native pane
  titles remain usable. Silence in an event-only callback is not a short-TTL failure.
- Watch SUCCESS is qualified by final Magy state. The live watch API has no caller
  `print-timeout` in its runner; general headless deadline SUCCESS remains an
  accepted Agy limitation, not a universal successful-turn signal.

## Verification

```bash
python3 integrations/agy/test_callback.py
cargo test --workspace
make check
```

The [H4 fixture](../../prototypes/h4-agy/README.md) records production reports from
real Agy/Magy runs in disposable stock Zellij sessions. Human H4 approval remains
a separate checkpoint.
