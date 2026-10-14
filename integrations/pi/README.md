# Pi pane-local monitoring (P2)

This extension provides event-driven, metadata-only observation for **Pi 1.2.0 (`@earendil-works/pi-coding-agent`)** running in interactive TUI mode inside a verified Zellij terminal pane.

## P2 Source Scope and Testing

At Phase 2 (P2), this extension is **source-only**. It is tested via explicit path execution:

```bash
pi -e ./integrations/pi/index.ts
```

There is **no global installation**, repository modification, or automatic loading into global Pi directories at this phase. Automatic installation, profile management, and lifecycle CLI commands (`setup pi`, `doctor pi`, `uninstall pi`) belong strictly to **Phase 3 (P3)**.

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
- **P3 adapter configuration (wiring design)**: At `session_start` in TUI mode only, reads bounded configuration (<= 16 KiB, schema validated, no tilde expansion) from `~/.pi/agent/verij-monitoring/adapter.json` if present. Zero filesystem I/O occurs in the extension factory or non-TUI modes. Explicit launch-time overrides (`VERIJ_BIN`, `VERIJ_AGENT_STATE_DIR`, `VERIJ_STATES_DIR`) always take precedence.
- **Private transport**: Spawns one `verij agent pi` reporter per active extension runtime with private stdin/stdout pipes (`stdio: ["pipe", "pipe", "ignore"]`). No protocol frames or error logs appear in the user's terminal.
- **Strict handshake & greetings**: On startup, sends a single `hello` frame correlating `runtime_id`, `version`, and `conversation_id`. The native reporter validates ownership, single-producer lock (`.pi.lock`), and monotonic source epochs. Ready greetings and receipts enforce strict whitelists and closed status enums (`idle`, `needs_input`, `working`, `done`, `error`, `unknown`).
- **Receipt correlation**: Snapshots wait for native transaction receipts verifying exact source epoch, monotonic next revision, and positive record revisions. A recurring five-second heartbeat renews the 15-second observer lease.
- **Framing**: Uses UTF-8 `StringDecoder` and caps input frame buffers at 256 KiB.
- **Process lifecycle & bounded teardown flush**: Listeners are installed before any stdin write. Spontaneous child crashes clear timers, requeue in-flight frames, and wait for process closure before backoff retries. Teardown performs a bounded nonblocking flush of already-observed semantic frames within total fixed grace (500ms), still receipt-correlated and rate-limited; once delivered or expired, closes stdin (EOF) and escalates via 500ms `SIGTERM` and 500ms `SIGKILL`. Reconnects are strictly prevented during teardown.
- **Context conversation changes**: Unexpected conversation ID changes or invalid identifiers during an active run or after `Done` immediately retire run, outcome, and prompt state to `Unknown`, reseeding a fresh runtime ID to ensure no prior completions are published under new associations.

## Capabilities

- **Conservative aggregate projection**: Treats one aggregate run ending at `agent_settled` as the completion unit. Low-level `agent_start/end`, model retries, tool execution cycles, and compaction do not mint intermediate completions.
- **Preserved terminal outcome**: A completed run outcome is preserved across subsequent session name changes and temporary UI prompt opens/closes, maintaining stable completion timestamps and IDs until the next genuine aggregate or branch invalidation.
- **Final outcome qualification**: Outcome candidates are captured at `agent_before_settle`, invalidated if work resumes (such as through extension continuation or tool execution), and finalized only at `agent_settled` with explicit `event.aborted === false`. Conflicting aborted flags or missing final-boundary evidence degrade conservatively to `Unknown`, never false `Done`.
- **Quiescence enforcement**: Settlement requires complete quiescence: zero in-flight tools, no active compaction, no pending requests, zero background tasks, and no pending messages in context. Conflicting nonquiescent states degrade safely to `Unknown`.
- **Metadata-only blocking UI**: Observes blocking dialogs (`confirm`, `select`, `input`, `editor`, `custom`) emitted through Pi's extension UI API. Emits generic `confirmation`, `question`, or `ui` categories with locally generated request UUIDs. Nested dialogs respect Pi's outer prompt model and never exceed a single pending request. Closing a prompt restores the previous execution state, while mid-run prompt settlements degrade safely to `Unknown` rather than sticking in working activity.
- **Strict privacy whitelist**: Snapshots carry only version, conversation ID/title, aggregate run ID, activity, pending request category/ID, background task count, and qualified outcome. Prompts, assistant text, tool arguments/results, dialog titles/content, provider bodies, and transcript paths are excluded.
- **Bounded transport**: 128-item total capacity FIFO queue with coalescing of identical consecutive refreshes. Rate limiter enforces at most 10 writes per second (interval >= 100ms) across all writes without bypassing priority frames. Queue overflow invalidates qualified projector state, reseeds blocking UI metadata to `NeedsInput` when locally provable, and prevents old completions from reasserting on heartbeats.
- **Fault tolerance and backoff**: Process exits and pipe errors trigger exponential backoff (2s, 4s, 8s, 16s, capped at 30s). Reconnects with the same runtime restore active state without replaying unread completion badges. In-flight frames are requeued and retried across reporter crashes.

## Exclusions

- **Non-interactive modes**: Print, JSON, and RPC modes do not run the TUI reporter.
- **Untracked background work**: Extensions launching asynchronous background work outside Pi's tracked turn lifecycle remain outside aggregate completion guarantees.
- **Unobservable UI**: External browser approvals, raw terminal readers, third-party UI bypassing Pi's extension API, and assistant conversational questions are not observable and do not trigger `Needs input`.
- **Historical transcripts**: Resumed sessions do not replay historical completions.
- **Versions**: Qualified strictly for Pi 1.2.0. Unsupported versions disable monitoring cleanly without spawning processes or setting timers.

## Recovery

- **Observer loss**: If the reporter process terminates or the pipe breaks, native lease expiry marks the process row as `Unknown` after 15 seconds, clearing pending requests while preserving row liveness and deduplication history.
- **Same-runtime reconnect**: If the reporter restarts within the same Pi process and conversation, the handshake restores previous epoch state (`restored: true`) without re-minting completion revisions.
- **Reload / Session replacement**: `/reload`, `/new`, `/resume`, or `/fork` trigger clean `session_shutdown`, tearing down the reporter and releasing `.pi.lock`. A new runtime ID ensures fresh state without historical badge replay.

## Verification

Deterministic event-state projection, transport, rate limiting, and P0 trace replay tests:

```bash
node --test integrations/pi/core.test.mjs
```

Strict TypeScript check against private Pi 1.2.0 declarations:

```bash
scratch=$(mktemp -d /tmp/vj-pi-p2-types-XXXXXX)
chmod 700 "$scratch"
npm install --prefix "$scratch" --ignore-scripts --no-audit --no-fund --save-exact \
  @earendil-works/pi-coding-agent@1.2.0 typescript@5.9.3 @types/node@24
cat << 'EOF' > "$scratch/tsconfig.json"
{
  "compilerOptions": {
    "target": "ES2022",
    "module": "NodeNext",
    "moduleResolution": "NodeNext",
    "strict": true,
    "esModuleInterop": true,
    "skipLibCheck": true,
    "forceConsistentCasingInFileNames": true,
    "noEmit": true
  },
  "include": ["index.ts"]
}
EOF
printf '{"type":"module"}\n' > "$scratch/package.json"
cp integrations/pi/{index.ts,core.mjs,core.d.mts} "$scratch/"
"$scratch/node_modules/.bin/tsc" --project "$scratch/tsconfig.json"
rm -rf -- "$scratch"
```

Native Rust reporter unit and integration test suite:

```bash
cargo test -p verij-types
cargo test -p verij-cli --test pi_reporter
cargo test -p verij-cli -- pi
```

Reviewer smoke verification evidence (synthetic ownership inventory via private controlling PTY, not P4):
- **Smoke 1**: Status transition sequence `idle -> working -> needs_input -> working -> done -> unknown`, completion revision 1, child clean exit.
- **Smoke 2**: Killed native reporter process after `Done`, reconnect source epochs 1 -> 2, completion revisions 0 -> 1 only, one stable `completed_at` timestamp preserved across reconnection, child clean exit.
