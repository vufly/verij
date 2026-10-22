# Experimental OpenCode V2 live status

This CLI-local adapter follows a cache/event observation model, similar to [Herdr's V2 integration](https://herdr.dev/docs/integrations/#opencode). It does **not** implement Verij's full OpenCode recovery contract.

## Install explicitly

Build the current CLI, then select the intended OpenCode executable and configuration directory:

```sh
cargo build -p verij-cli
target/debug/verij agent setup opencode --experimental-live-status
target/debug/verij agent doctor opencode
```

`--opencode-bin /absolute/path/opencode` and `--config-dir /absolute/config/directory` are available. The adapter requires the current Verij native reporter and verified Zellij pane inventory/exporter, as described in the [OpenCode setup guide](../README.md). Setup alone does not prove runtime ownership.

Restart only the intended OpenCode CLI after setup. Run it normally inside an inner Zellij terminal. The plugin does nothing outside a pane locator environment or without its explicit `experimentalLiveStatus` option.

The experimental runtime contract selects **2.x >= 2.0.32** with shape validation. The actual retained loading smoke uses **2.1.2**; this is not a claim that every 2.x release or server mode is fully qualified. Mini, headless `run`, desktop/browser clients, and authenticated remote-server support are not qualified.

## Behavior and deliberate omissions

- The selected CLI route determines the root family. Foreign sessions cannot claim the pane. Only bounded, cached ancestry is traversed; missing or inconsistent ancestry produces Unknown.
- Positive cached activity and observed execution starts produce Working. Explicit cached/event permission and form IDs produce Needs input; forms use the existing Question request kind. Verij never answers, cancels, navigates, or marks anything viewed.
- Home is Idle without a conversation. **A selected, quiescent conversation is Unknown, not inferred Idle.** Success, failure, and interruption do not become Done or terminal Error. Child activity is only a positive hint, not proof of complete-family freshness.
- **No turn ID, successful completion, completion revision allocation, or acknowledgement target is emitted.** Native schema 2 independently rejects outcomes and selected Idle, even if the plugin sends them accidentally. V1 retains schema 1 and its existing outcome semantics.
- Route changes, reporter replacement, and observable connection invalidation clear event hints. Hints have a 15-second reconciliation lifetime; OpenCode's cache may still be stale. The public API does not supply a reliable connection-generation/freshness guarantee.
- Native process/TTY/pane ownership and producer fencing are unchanged. Full snapshots wait for native transaction receipts. One in-flight and one coalesced pending snapshot bound reporter work; unchanged observations renew their native lease every five seconds. The existing 15-second native lease expires to Unknown.

The plugin samples cache/route data once per second, retains at most 64 family members and 100 pending input identities, and drops all message bodies, input contents, errors, and titles. It performs **no SDK requests, cache synchronization/invalidation, history reads, database access, or credential discovery**. Event storms do not schedule extra reads or timers.

These are bounds on **Verij-owned traversal, retention, and reporting**, not OpenCode's cache allocation, buffering, or freshness. Missing-event recovery, durable successful completion, terminal-error recovery, complete-family/input freshness, upstream pre-parse bounds, native-pane/provider runtime acceptance, and resource qualification remain outstanding. Full-recovery P0 is still blocked on its upstream transport dependency.

## Configuration safety and rollback

V2 uses global `cli.json` under `OPENCODE_CONFIG_DIR`, XDG, or the explicit config directory. It ignores the V1-only `OPENCODE_TUI_CONFIG` override. Setup registers one object under `plugins`, pointing to the independently owned `verij-monitoring-v2-live/` directory with its `tui.js` entrypoint.

If `cli.json` is absent while V1 preferences exist, setup refuses to suppress OpenCode's first-start migration. Start V2 once in that environment and review migration first; legacy `tui.jsonc` may need manual review. A positively fingerprinted migrated V1 entry is removed from `cli.json` only; V1 TUI configuration and assets are preserved.

Setup preserves JSONC comments, other plugins/options, and unrelated settings. Recognized disable directives and ambiguous wildcard disables require review rather than being overridden. `OPENCODE_CLI_CONFIG_CONTENT` replacing `plugins` blocks setup and is reported by doctor. Project configuration or runtime plugin state can still override/disable the plugin; doctor does not infer runtime health from installation.

```sh
target/debug/verij agent uninstall opencode --experimental-live-status
```

Uninstall does not need a working OpenCode executable. It removes only V2's registration and unmodified owned assets, preserves user-modified files and V1 installation, and requires restarting the intended CLI. Without the flag, uninstall detects an installed V2 manifest first; a subsequent V1 uninstall remains separate. No shared service restart is needed for this CLI-only adapter.

## Verification

```sh
node --test integrations/opencode/v2/*.test.mjs
cargo test -p verij-cli opencode -- --test-threads=1
cargo test -p verij-cli --test opencode_live_setup --test opencode_setup
```

[Retained CLI-loading evidence](../../../prototypes/opencode-v2/live-status-loading-results.json) exercises exact production JavaScript assets on private real-TTY OpenCode 2.1.2 clients in standalone and authenticated private-server modes. It proves loading, Unknown-first reporting, Home Idle without authority, lease renewal, config disable cleanup, and re-enable fencing. An additional selected-session private-server case observes Unknown plus its own pending form, excludes a foreign session's form, and verifies both forms remain unanswered. Its reporter is an explicit **fixture protocol sink**, not native ownership verification. No provider prompts, end-to-end native-pane status acceptance, global installation, or resource benchmark were performed.

See the [approved reduced-scope track](../../../docs/OPENCODE_V2_UPGRADE_PLAN.md) for implementation status and the separate blocked full-recovery plan.
