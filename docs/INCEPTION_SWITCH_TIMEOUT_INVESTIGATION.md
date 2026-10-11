# Inception Switch Timeout Investigation

Date: **2026-10-12**
Status: **Stale runtime cleaned; Verij-only resource mitigation implemented and verified on stock Zellij 0.45.1.**

---

## 1. Incident Summary

During interactive navigation in the Verij sidebar, switching the host's active inner session from `ss` to `o24` failed with the following error rendered in the TUI status bar:

```text
✕ Failed to dispatch Inception Switch pipe to session 'ss': Zellij command timed out after 3s: "zellij" "-s" "ss" "pipe" "--name" "verij_control" "--" "switch:o24"
```

---

## 2. Architecture & Trigger Flow

1. **Inception Switch Dispatch (`verij-cli/src/actions.rs`):**
   - When a user selects a different inner session in the sidebar, Verij uses the *Inception Switch* pattern rather than killing and re-spawning PTYs.
   - The CLI invokes `inception_switch(old_session, new_session)`:
     ```rust
     pub fn inception_switch(old_active_session: &str, new_selected_session: &str) -> Result<()> {
         let payload = format!("switch:{}", new_selected_session);
         crate::session::zellij_action(&[
             "-s",
             old_active_session,
             "pipe",
             "--name",
             VERIJ_CONTROL_PIPE,
             "--",
             &payload,
         ])
         .with_context(|| {
             format!(
                 "Failed to dispatch Inception Switch pipe to session '{}'",
                 old_active_session
             )
         })
     }
     ```
   - This dispatches `zellij -s <old_session> pipe --name verij_control -- switch:<new_session>` targeting the inner session.
   - The headless `verij-plugin.wasm` inside that session listens for `verij_control` pipe messages and invokes Zellij's native `switch_session(Some(target_session))` shim to migrate the attached client.

2. **Query Execution & Deadline Enforcement (`verij-cli/src/session.rs`):**
   - All synchronous Zellij CLI queries routed through `crate::session::zellij_action` use `query_zellij`.
   - `query_zellij` enforces a hard deadline governed by `QUERY_TIMEOUT = Duration::from_secs(3)`.
   - If the Zellij process fails to exit within 3 seconds, `query_zellij` issues `SIGKILL` to the child and returns:
     ```text
     Zellij command timed out after 3s: <description>
     ```

---

## 3. Findings & Evidence

### 3.1 Degradation of Target Zellij Session (`ss`)
Inspection of running processes revealed that the Zellij server managing session `ss` had severely degraded:

- **Process Identity:** PID `1026` (`zellij --server /run/user/1000/zellij/contract_version_1/ss`).
- **Resource Consumption:**
  - Memory: **9.5 GB RSS** (virtual size > 9.5 GB).
  - CPU Utilization: **>215% CPU** consistently.
  - Cumulative Runtime: Over 1700 CPU minutes.
- **Thread Analysis (`ps -T -p 1026`):**
  - Worker threads `plugin-exec-0`, `plugin-exec-1`, `plugin-exec-2`, and `plugin-exec-3` were burning significant CPU time (accumulating 4 to 6 hours of CPU execution each).

### 3.2 Upstream Router Failure in Zellij Server
Inspection of `/tmp/zellij-1000/zellij-log/zellij.log` revealed recurring router timeout errors inside the Zellij server for session `ss`:

```text
ERROR | zellij_server::route | zellij-server/src/route.rs:77: Action CliPipe did not complete within 1s timeout
```

- When the CLI runs `zellij -s ss pipe`, the CLI client establishes a Unix domain socket connection to `ss` and sends an `Action::CliPipe`.
- The Zellij server router enforces an internal 1-second timeout for dispatching `CliPipe` actions to plugins.
- Because the WASM plugin executor threads in `ss` were backlogged and saturated, the server router timed out internally, stalling the CLI pipe caller indefinitely.
- After 3 seconds, `verij-cli`'s `query_zellij` deadline expired, terminating the command.

### 3.3 Duplicate Plugin Instances in Session `ss`
Log entries showed multiple instances of `verij-plugin` running concurrently inside session `ss`:

```text
DEBUG | [id: 1 ] [verij-plugin] State::load initialized, epoch=1791741986435538492
DEBUG | [id: 10] [verij-plugin] State::load initialized, epoch=1791741986450578478
```

- Because `zellij pipe --name verij_control` broadcasts without targeting a specific plugin ID, both Verij instances attempted to process the event. Other running plugins can receive the broadcast too and ignore the name.
- Multiple instances amplify work, but duplication alone does not establish a deadlock. Follow-up inspection found heavy executor CPU in sessions with only one persistent Verij exporter.

### 3.4 Workspace Marker vs. Physical Attachment Desync
Further inspection of host session `v` (`PID 2195`) showed a state mismatch:

- The host runtime marker (`/run/user/1000/verij/workspace-*.session`) still recorded `ss`.
- However, the actual client process attached inside the Workspace terminal pane (`PID 2652966`) had already migrated or reattached to `codex-monitoring`:
  ```text
  zellij attach --force-run-commands codex-monitoring
  ```
- Because the host's recorded state pointed to `ss`, the switch action targeted `ss` (an idle, wedged session with no active workspace client) instead of the session actually hosting the workspace client.

---

## 4. Immediate Recovery Steps

To recover the environment from this state:

1. **Terminate the wedged `ss` session:**
   ```bash
   zellij kill-session ss
   ```
2. **Clear stale host workspace markers and attachment locks:**
   ```bash
   rm -f /run/user/1000/verij/workspace-*.session
   rm -f /run/user/1000/verij/workspace-*.attach.lock
   ```
3. **Trigger clean workspace attachment:**
   In the Verij TUI sidebar, select the desired target session (e.g. `o24` or `codex-monitoring`). With the marker cleared, Verij will execute a clean attach in the Workspace pane rather than dispatching a pipe to a non-existent/stuck session.

---

## 5. Architectural Recommendations & Hardening

1. **Verify Attachment Prior to Inception Switch:**
   - Before dispatching an Inception Switch pipe to `old_active_session`, inspect whether `old_active_session` actually has connected clients via `list-clients` or verify the PID running inside the workspace pane.
   - If `old_active_session` has 0 clients or the workspace pane process has diverged, fall back directly to `attach_in_right_pane(new_selected_session)` instead of waiting for a pipe on an unattached session.

2. **Deduplicate Plugin Launch:**
    - Keep one persistent exporter per configured URL. Temporary keyboard-registration surfaces and per-client instances are intentional; do not blindly delete them.

3. **Fallback on Inception Switch Timeout:**
   - If `inception_switch` fails with a timeout or socket error, catch the failure in `switch_session_if_current` and fall back to detaching/reattaching the Workspace pane (`attach_in_right_pane`) rather than leaving the user stuck on a failed dispatch.

4. **Track Upstream Issue #5676:**
    - [#5676](https://github.com/zellij-org/zellij/issues/5676) included the same log message, but was closed by [#5683](https://github.com/zellij-org/zellij/pull/5683) for a tab-focus crash. It is not evidence of a fix for this resource problem.

---

## 6. Follow-up: Cleanup and Resource Investigation

### Cleanup performed

- Confirmed original `ss` server PID `1026` had exited.
- Removed the stale `workspace-4076985-1791667161069625250.attach.lock`. No workspace session marker remained at the first inspection.
- Removed nine inventory JSON files and eight focus observations belonging to the dead `1026` exporter.
- `ss` was subsequently resurrected under a different server PID. Its new exports and newly recreated host markers belong to the live session and were preserved.
- Removed the temporary event sampler and its permission-cache entry after collecting evidence. Private benchmark sessions were terminated.

### Reproduced trigger: read-only CLI polling amplifies plugin events

Fresh isolated sessions with Verij, the user configuration, and `zjstatus` stayed below approximately 4% CPU. Reattaching ten times and creating several sessions/tabs did not reproduce the live saturation by themselves.

A temporary sampler in the recreated `ss` observed 824 `PaneUpdate`, 824 `TabUpdate`, and 832 `SessionUpdate` events, plus nine two-second timer callbacks. WASM event decoding consumed 21.2 CPU-seconds. This establishes a substantial event stream, rather than normal one-second heartbeat cost.

A private stock-Zellij test reproduced high CPU using only two loops of read-only `action list-panes --json` commands. Source inspection explains the amplification: CLI/socket-probe teardown reaches `Screen::remove_client`, whose end publishes pane, tab, and session state even when the transient client never attached to the screen. Zellij queues subscribed events before invoking WASM. Comparing unchanged facts inside Verij's callback therefore saves JSON publication, but not queue allocation or event decoding.

The precise mix of live callers and terminal activity driving the original session's event rate was not isolated. The controlled reproduction demonstrates a sufficient trigger; it does not attribute every live update to Verij's sidebar.

### Additional failure modes

1. **Reloading with no attached client can leave unbounded cached events.** In stock 0.45.1, `reload_plugin_with_id` marks the plugin pending and creates its event cache before checking for a connected client. Returning with `No connected clients, cannot reload plugin` leaves that cache active. A successful CLI exit does not establish that a reload occurred. Avoid repeating reload commands on an unattached session.
2. **Some SDK queries trap instead of returning an error.** `get_focused_pane_info`, `get_pane_info`, and `get_session_list` unwrap the reply bytes/protobuf internally. A permission-denied reply can be empty. Integration verification captured that panic followed by repeated `RefCell already borrowed` failures: aborting a WASM call leaves its state borrowed for subsequent callbacks.
3. **Startup publication is not plugin-presence evidence.** The old startup fallback launched another plugin when its JSON file had not appeared after 150 ms. Loading, permissions, and polling can all delay publication.

## 7. Implemented Verij-only Mitigation

No custom Zellij binary or upstream patch is required.

- `verij-plugin/src/main.rs`: replaced persistent pane/tab/session event subscriptions with a one-second stock SDK inventory poll. It reuses the control timer, so fast focus polls and obsolete timer callbacks cannot start additional inventory chains. Full terminal topology is still exported, including background tabs, hidden floating panes, and detached sessions. A native focus query projects the active tab for the exporter client. Unchanged inventory does not rewrite JSON.
- `verij-plugin/src/queries.rs`: added fallible wrappers over the same stock SDK protocol for the three reply-unwrapping queries. Empty, malformed, or denied replies return an error instead of trapping the WASM instance.
- `verij-cli/src/actions.rs`: checks actual plugin topology and URL before launching a startup fallback. Missing inventory publication alone no longer creates another persistent exporter.
- `prototypes/resource-usage/verify.py`: provides a private stock-Zellij benchmark with controlled CLI query rate, pane-create/close convergence checks, detached-client checks, and bounded cleanup.

**Tradeoff:** inventory is eventually consistent on the one-second poll plus Zellij's metadata publication cadence. Permission processing, keyboard registration, control pipes, and fast pending-focus confirmation retain their existing control path. No navigation deadline was increased.

### Measured result

Both runs used the same stock binary, one Verij exporter, one terminal, two pollers, exactly 10 completed queries/second, and ten-second measurement windows. The figures cover the entire Zellij server, not only Verij.

| Measurement | Previous exporter | Bounded exporter |
|---|---:|---:|
| Idle attached CPU | 0.5% | 0.6% |
| Read-only query-load CPU | 18.6% | 7.7% |
| RSS under query load | 26.20 MiB | 26.26 MiB |
| Idle detached CPU | 0.3% | 0.4% |
| Pane create/close convergence | PASS | PASS |
| Actual detached client inventory | PASS | PASS |
| Private server cleanup | PASS | PASS |

This is approximately **59% less server CPU under the controlled query workload**. The short test does not establish multi-hour leak freedom. An aggressive reproduction with `zjstatus` still saturated its executor: reducing Verij's subscriptions cannot bound another plugin's queue.

### Verification

- Eight plugin unit tests passed, including timer-rate limiting, client-focus projection, closed-pane removal, and malformed SDK reply handling.
- Four CLI readiness tests passed, including duplicate-prevention topology checks.
- `cargo check --workspace` and `git diff --check` passed.
- The existing H1 navigation harness passed all eleven checks with the new distribution WASM on stock Zellij: two-client keyboard registration, tiled/floating/stacked/fullscreen focus, cross-session rebind, preserved peer attachment, public plugin reload, and session/background-title rename. All private processes exited.

Reproduce the load check:

```bash
cargo build -p verij-plugin --target wasm32-wasip1 --release
python prototypes/resource-usage/verify.py \
  --plugin target/wasm32-wasip1/release/verij-plugin.wasm \
  --query-interval 0.2 --seconds 10 \
  --output /tmp/opencode/verij-resource-result.json
```

### Rollout and recovery

Install the new exporter with `make install-plugin`. New sessions load that file normally. For an existing healthy session, attach a real client first and reload the **same configured URL** once:

```bash
zellij -s SESSION action start-or-reload-plugin \
  file:/home/vudinhn/.local/share/verij/verij_plugin.wasm
```

A server already carrying cached reload events or trapped plugin instances may need session restart to release accumulated state. The new WASM on disk does not replace already loaded instances or clear their queues. Startup duplicate prevention also requires the rebuilt native CLI.

During diagnosis, a no-client reload attempt in `o24` exercised the pending-cache failure above; it did not repair the exporter. The `o24` background `link` plugin was removed during isolation. The user subsequently approved restarting and restoring this session.

**Recovery completed:** backed up the durable session directory to `/tmp/opencode/o24-recovery-iseptlfb/session_info`, terminated server PID `2341983`, and resurrected the saved layout with commands enabled. New server PID `1612522` restored three tabs, seven terminal panes, all three status bars, the background link plugin, and the new Verij exporter. `node RUN.js`, its `watch.js --watch` child, and `bun ui-proxy.bun.js` are running; proxy ports 8091–8094 listen again. Inventory confirms zero attached clients after the temporary recovery terminal closed. Old-server exporter and focus files were removed.

`o24` RSS dropped from approximately 5.5 GiB to 105 MiB. CPU still varied around 40–70% after startup, with remaining executor work in other plugins; this recovery does not claim that all plugin resource use is solved.

The new exporter was installed at the normal configured URL. Attached `ss` and `v` were reloaded once, and their newly published epochs confirmed that the reloads completed. Native CLI release was rebuilt; at the final check the installed CLI matched that release binary.
