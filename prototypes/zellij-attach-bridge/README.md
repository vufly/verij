# Experimental Zellij attachment bridge

This is an H0 feasibility prototype, approved for an isolated Zellij checkout. It is **not installed Zellij support, a Verij feature implementation, or H0 approval**.

## Reproduce

Base: upstream Zellij tag `v0.45.1`, commit `efd8fd5a89a20c07a111d248ad7fce53848d2c18`. The patch includes Rust code, protobuf schema/bindings, and focused tests. No change to the installed binary is required.

```bash
# Run from ~/repos/workspaces/verij-agent-monitoring/verij.
# The sibling ../zellij checkout is already restored and patched; do not reapply.
cargo build --manifest-path ../zellij/Cargo.toml -p zellij --no-default-features

python3 prototypes/zellij-attach-bridge/verify.py \
  --binary ../zellij/target/debug/zellij --placements \
  --scratch-dir ../artifacts/scratch --output ../artifacts/direct-results.json

python3 prototypes/zellij-attach-bridge/verify.py \
  --binary ../zellij/target/debug/zellij --nested --placements \
  --scratch-dir ../artifacts/scratch --output ../artifacts/nested-keyboard-results.json

python3 prototypes/zellij-attach-bridge/verify_tmux.py \
  --binary ../zellij/target/debug/zellij \
  --scratch-dir ../artifacts/scratch --output ../artifacts/nested-tmux-results.json

# Extended Linux lifecycle observations; inspect limitations even on PASS.
python3 prototypes/zellij-attach-bridge/verify.py \
  --binary ../zellij/target/debug/zellij --placements --lifecycle \
  --scratch-dir ../artifacts/scratch --output ../artifacts/lifecycle-direct-results.json

python3 prototypes/zellij-attach-bridge/verify.py \
  --binary ../zellij/target/debug/zellij --nested --lifecycle \
  --scratch-dir ../artifacts/scratch --output ../artifacts/lifecycle-nested-results.json
```

Run from the workspace's Verij worktree. The Zellij worktree is on its own `agent-monitoring` branch, based on the exact upstream commit above. Source, patch, builds and saved evidence are durable under `~/repos/workspaces/verij-agent-monitoring`; no `/tmp/opencode` checkout is required. Use a clean exact-tag checkout when applying the patch elsewhere; do not reapply to an already-patched checkout. Each probe retains its isolated config/XDG/identity/input files and `results.json` beneath `--scratch-dir` (or the prototype's ignored `.scratch/` default), so interrupted or failed experiments remain inspectable. Only short-lived sockets use a private `/tmp/vj-sock-*` directory because Unix socket paths have a length limit. Probes target only those private sockets and clean their own clients and sessions. Some earlier starts timed out; failures are reported rather than interpreted as successful focus. Historical JSON results retain original `/tmp` binary paths as provenance, not current instructions.

The nested probes create two actual disposable host Zellij sessions, replace each host's terminal pane with an inner attachment wrapper, and record the wrapper's real inherited host pane identity before `exec` of the inner client. Each wrapper supplies its own identity-record directory. Their private host configuration explicitly uses `nested_session_handling "descend"` and Locked outer mode. `verify.py --nested` sends keyboard bytes through each outer display PTY; `verify_tmux.py` creates a separate private tmux server and sends normal terminal keys through its outer display panes. Neither passing keyboard mode injects input directly into inner panes. The optional `--host-injected-input` remains a diagnostic transport.

## Bridge contract

1. A host wrapper creates a private, unique absolute directory and launches the patched inner display client with `VERIJ_ZELLIJ_IDENTITY_DIR` set to it.
2. The display client requests identity **on its own attached IPC connection**. The server returns its actual assigned `ClientId`, a random connection-generation UUID, and server PID. CLI/unattached connections and web clients do not qualify as display attachments.
3. That client atomically publishes a mode-0600 JSON record containing client PID, server PID, ClientId, connection token, and `attached: true`. Each connection owns a unique filename, so old cleanup cannot overwrite a replacement attachment's record.
4. Normal teardown writes `attached: false` to that connection's record. Abrupt death can leave a stale record. A real wrapper must independently validate client/server process birth tokens and pane ownership; a JSON flag alone is insufficient.
5. The prototype control helper sends a typed `VerijFocusPane` request with the target display ClientId and generation. The server validates the pair, and the screen thread validates the generation again when executing the queued focus. Numeric ClientId reuse therefore does not authorize a stale connection token.
6. `verij:accepted` means dispatch accepted, **not effective focus confirmed**. The harness checks per-client `list-clients` and actual receiver input after dispatch. Missing panes, queued invalidation, and other execution failures require a production completion/query contract.

The prototype bypasses last-active-client CLI routing for this dedicated request. It does not change existing CLI action routing or the Verij plugin. The small raw-protobuf helper in `verify.py` is a test client, not a public integration API.

## Reviewed results

Observed on 2026-09-30 with Linux, Rust 1.98.1, and a local debug Zellij build without default features:

| Check | Result |
|---|---|
| Client/server protobuf identity and focus wire roundtrip | Pass |
| CLI/unattached and web identities excluded; reused ClientId gets a new generation | Pass, focused unit test |
| Build check with `web_server_capability` enabled | Pass; web listener ignores local-only identity message |
| Two direct display clients get distinct IDs and tokens for one server | Pass |
| Tiled focus can be assigned and swapped independently; receiver gets input from the correct display PTY | Pass |
| Hidden floating target is revealed and receives input | Pass |
| A different target receives input when another pane was fullscreen | Pass |
| Ordinary stack members can be targeted and receive input | Pass |
| Clean detach invalidates the record; reattachment reuses ClientId but changes token | Pass |
| Old token is rejected after ID reuse without moving the replacement client | Pass |
| Two real nested host panes bind to their respective inner display identities | Pass |
| Nested placement/input checks using explicit host-pane CLI injection | Pass; diagnostic transport only |
| Nested outer-display PTY keyboard input with explicit Descend handling | Pass |
| Nested terminal keyboard through a private real tmux server | Pass; tiled, float, fullscreen, stack, reconnect and stale-token checks |

Saved harness outputs: [direct-results.json](direct-results.json), [nested-keyboard-results.json](nested-keyboard-results.json), [nested-tmux-results.json](nested-tmux-results.json), and diagnostic [nested-injected-results.json](nested-injected-results.json). [nested-keyboard-summary.json](nested-keyboard-summary.json) records the earlier failure and its resolution: the host had not selected the nested guest's Descend policy. Unit checks:

```bash
cargo test --manifest-path ../zellij/Cargo.toml \
  -p zellij-utils verij_bridge_wire_roundtrip --no-default-features
cargo test --manifest-path ../zellij/Cargo.toml \
  -p zellij-server verij_generation_invalidates_reused_client_id_and_excludes_cli --no-default-features
```

## Remaining H0 work

### Lifecycle follow-up — 2026-10-01

The `--lifecycle` probe adds actual keyboard checks for client-specific cross-tab focus and return to tab zero, focus after session rename, missing targets, abrupt client death and generation reuse, and first reattachment after zero display clients. It independently samples Linux process identity as `(boot_id, PID, /proc start_jiffies)` and excludes zombies; attachment directories and records are checked for modes 0700 and 0600. These are harness observations, not production identity-directory ownership or wrapper validation.

[Direct lifecycle results](lifecycle-direct-results.json) pass. A [nested run without the placement matrix](lifecycle-nested-success-results.json) also passed all lifecycle observations; its original evidence is `../artifacts/lifecycle-nested-results-2.json` from the Verij worktree, under scratch run `vj-bridge-bkl7gcq1`. Later nested runs timed out during host startup or reconnect, and the first nested placement+lifecycle run timed out waiting for detach-record invalidation. [The retained nested failure](lifecycle-nested-results.json) and [attempt summary](lifecycle-summary.json) keep these outcomes explicit. The full nested placement+lifecycle combination is **not verified**. Stage, attachment liveness and private outer terminal output are retained for future diagnosis; no startup cause is established.

Two concrete limitations were independently reproduced:

- **Unmirrored stacks still share expansion.** Returning client A from another tab to stack member 0 changed client B from member 1 to member 0; B's subsequent keyboard token reached member 0. Tagged native `tiled_panes/mod.rs::focus_pane` calls `focus_pane_for_all_clients_in_stack` when expanding a stack, separately from its `session_is_mirrored` branch. Earlier ordinary-stack passes prove that the addressed pane accepts input, not independent focus for two clients viewing different members of one stack.
- **Acceptance is not completion, including a missing target.** A valid generation targeting nonexistent terminal pane `4294967295` received `verij:accepted`; per-client focus stayed unchanged and keyboard input reached the previous pane. `SIGKILL` left `attached:true` in the old identity file, while independent process liveness showed death and the server rejected its token before and after numeric ClientId reuse. Production navigation therefore needs execution completion/failure and independent liveness checks.

- Have a reviewer observe keyboard forwarding through two host Workspace panes and verify sidebar-versus-Workspace visits. Automated outer keyboard checks now pass under the explicit Descend test configuration; this does not exercise every user nested-session policy or host geometry.
- Stabilize nested host startup/reconnect and rerun the full placement+lifecycle combination. Verify hidden stack-list members, cross-session transitions, mirroring, plugin reload, resurrection, and superseded-navigation races. Cross-tab, rename and first reattachment after zero clients have the limited observations above.
- Add a production request/completion contract and demonstrate execution-time generation validation under disconnect/reconnect races, rather than relying on acceptance replies.
- Decide how the bridge would be maintained/distributed. The new protobuf tags are experimental; this is not an upstream-approved protocol extension or a supported mixed-version deployment.
- Validate private identity-directory ownership, boot-qualified process birth identity, abrupt exits, and wrapper lifecycle in the eventual integration.
- Finish OpenCode semantic event-family/status checks and Agy input/background/interactive checks described in `docs/AGENT_MONITORING_IMPLEMENTATION.md`.

Keep the installed Zellij unchanged until the prototype and dependency decision receive explicit review.
