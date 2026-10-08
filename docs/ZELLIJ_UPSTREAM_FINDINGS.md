# Zellij Upstream Findings for Verij

Research date: **2026-10-08**.

Status: **research for future integration; implementation pending**.

## 1. Review Baseline

- Latest stable release: [Zellij v0.45.1](https://github.com/zellij-org/zellij/releases/tag/v0.45.1), published on 2026-08-28.
- Installed version at review time: `zellij 0.45.1`.
- Reviewed upstream `main` through commit [`9dacbc1982`](https://github.com/zellij-org/zellij/commit/9dacbc1982f2b4048f8c2c66c6ed43afb65c9d81), dated 2026-10-08.
- Sources included the [release-to-main comparison](https://github.com/zellij-org/zellij/compare/v0.45.1...9dacbc1982f2b4048f8c2c66c6ed43afb65c9d81), pull requests, source code, and the pinned [unreleased changelog](https://github.com/zellij-org/zellij/blob/9dacbc1982f2b4048f8c2c66c6ed43afb65c9d81/CHANGELOG.md).

The comparison reports 52 commits on `main`, but the release and `main` histories have diverged. Some comparison entries describe changes already included in v0.45.1. There are 32 `main` commits with committer timestamps after the release publication time. The findings below use the unreleased changelog and post-release history to identify new work.

The prompt and related APIs described here are unreleased at this snapshot. Their names and behavior should be checked again when adopting a later release.

## 2. Features Most Relevant to Verij

| Feature | Upstream change | Potential use in Verij |
|---|---|---|
| Prompts, forms, and client-local popups | [#5685](https://github.com/zellij-org/zellij/pull/5685), merged October 5 | Create and rename session dialogs, session action menus, and forms with several options. |
| Notifications | [#5685](https://github.com/zellij-org/zellij/pull/5685) | Report session creation, operation failures, or agent attention without taking keyboard focus. Clicking a notification can focus its originating pane. |
| Nested-session mode and keybinding reporting | [#5589](https://github.com/zellij-org/zellij/pull/5589), merged September 24 | A host-side plugin can show the inner session's current mode and applicable shortcuts. |
| Collapsible plugin panes | [#5590](https://github.com/zellij-org/zellij/pull/5590), merged October 6 | Optional plugin bars can return reserved layout space while empty, then restore their original geometry. |
| Named swap layouts and configurable pane borders | [#5630](https://github.com/zellij-org/zellij/pull/5630), merged September 24 | Select a layout explicitly and customize sidebar/workspace boundaries. |
| Reduced memory usage and shared plugin instances | [#5622](https://github.com/zellij-org/zellij/pull/5622), [#5662](https://github.com/zellij-org/zellij/pull/5662) | Reduce the resource cost of keeping many inner sessions alive. |

### Other Notable Changes

- Plugins hidden with `hide_self` continue receiving mode, tab, and pane updates: [#5680](https://github.com/zellij-org/zellij/pull/5680).
- CLI socket checks have a receive timeout so session listing does not hang indefinitely on a wedged server: [#5481](https://github.com/zellij-org/zellij/pull/5481).
- Resurrection stops saving layouts that cannot reload and allows attachment to live sessions regardless of those saved-layout failures: [#5690](https://github.com/zellij-org/zellij/pull/5690).
- Pane and layout behavior received several stability fixes: [#5656](https://github.com/zellij-org/zellij/pull/5656), [#5663](https://github.com/zellij-org/zellij/pull/5663), [#5683](https://github.com/zellij-org/zellij/pull/5683), and [#5687](https://github.com/zellij-org/zellij/pull/5687).
- OSC 7 working-directory changes are forwarded to the parent terminal: [#4867](https://github.com/zellij-org/zellij/pull/4867).
- WebSocket heartbeat support reduces idle disconnections through proxies: [#4633](https://github.com/zellij-org/zellij/pull/4633).
- Horizontal mouse scrolling is supported for programs that request it: [#4860](https://github.com/zellij-org/zellij/pull/4860).

## 3. Prompt CLI

PR [#5685](https://github.com/zellij-org/zellij/pull/5685) introduces `zellij prompt` with these elements:

- `input`: a one-line text field.
- `confirm`: a yes/no question.
- `choose`: single or multiple selection from items, including input from stdin.
- `select`: a dropdown.
- `menu`: an action menu.
- `number`: an integer field with optional bounds and step size.
- `toggle`: an on/off field.
- `form`: multiple fields described in JSON.
- `notify`: a nonblocking notification.

Input supports prefilled defaults, placeholders, required values, regex validation, and undo/redo. Answer-producing commands support optional JSON output. Popups are visible only to the initiating client and are independent of the floating-pane layer. Center placement is useful for a dialog launched from Verij's narrow sidebar.

### Session Dialog Examples

These examples collect a name; Verij would validate the result and perform the corresponding session operation.

```bash
# Collect a name for a new inner session.
zellij prompt input "Session name" \
  --title "Create inner session" \
  --required --at-center --json

# Prefill the current name when renaming an inner session.
zellij prompt input "New session name" \
  --title "Rename inner session" \
  --default "$old_name" \
  --required --at-center --json
```

A successful input answer has this shape:

```json
{"result":"answered","value":"my-session"}
```

Answer-producing CLI prompts block until resolved. Exit codes are:

| Code | Meaning |
|---|---|
| `0` | Answered; for `confirm`, the affirmative answer. |
| `1` | Cancelled or closed; for `confirm`, also the negative answer. |
| `2` | Error. |
| `124` | Timed out without a default answer. |

`--default` supplies the initial value and the answer used when a configured timeout expires. JSON output identifies how the prompt ended, so an integration should inspect both the process status and the result field.

Forms could later collect a session name, layout selection, and additional options in one dialog. Notifications can report progress or completion without taking focus:

```bash
zellij prompt notify "Session created" --title Verij --timeout 5s
```

Source: pinned [CLI definitions](https://github.com/zellij-org/zellij/blob/9dacbc1982f2b4048f8c2c66c6ed43afb65c9d81/zellij-utils/src/cli.rs) and [prompt types and validation](https://github.com/zellij-org/zellij/blob/9dacbc1982f2b4048f8c2c66c6ed43afb65c9d81/zellij-utils/src/prompt.rs).

## 4. Plugin Prompt API

Plugins can request a built-in `zellij:prompt` popup asynchronously:

```rust
let request_id = prompt(
    PromptRequest::input("New session name")
        .title("Rename inner session")
        .default(old_name)
        .required()
        .placement(PromptPlacement::Center),
);
```

The plugin subscribes to `EventType::PromptResult` and receives `Event::PromptResult(request_id, result)`. The command requires `OpenTerminalsOrPlugins` permission. Results distinguish answered values, confirmation decisions, cancellation, timeout, and errors.

The result is sent only to the requesting plugin instance. If that plugin closes or reloads before an answer, its open prompts close and no result is delivered.

Source: pinned [`prompt()` API](https://github.com/zellij-org/zellij/blob/9dacbc1982f2b4048f8c2c66c6ed43afb65c9d81/zellij-tile/src/shim.rs#L3398-L3420).

## 5. Nested-Session and Layout APIs

At the reviewed snapshot, the nested-session reporting API exposes:

- `Event::NestedSessionModeUpdate`, including the pane ID, session path, current mode, optional base mode, and keybinding generation.
- `get_nested_session_keybinds(pane_id)`, returning the guest's mode and keybinding table or a typed error.
- `Event::NestedSessionEnded`, identifying a guest that exited or became unresponsive.

These APIs could support accurate inner-session shortcut hints in a host-side plugin. Verij's current WASM agents run inside inner sessions, so host-side consumption would require additional integration.

For optional layout bars, `set_self_collapsed(bool)` lets a tiled plugin return its space to neighboring panes while preserving its layout position and size constraint. It has no effect on floating panes. A shared plugin can use the corresponding slot-based API.

Sources: pinned [event and response definitions](https://github.com/zellij-org/zellij/blob/9dacbc1982f2b4048f8c2c66c6ed43afb65c9d81/zellij-utils/src/data.rs) and [plugin commands](https://github.com/zellij-org/zellij/blob/9dacbc1982f2b4048f8c2c66c6ed43afb65c9d81/zellij-tile/src/shim.rs).

## 6. Recommended Adoption Path

1. **Use CLI input prompts from the native host sidebar first.** Existing input handling is in `verij-cli/src/tui/mod.rs`. An accepted create answer can enter the current `creation_pending` flow and call `actions::create_inner_session()` in `verij-cli/src/actions.rs`.
2. **Implement inner-session rename handling alongside its dialog.** The existing `R` action renames the host. Inner-session rename needs backend handling and updates to name-dependent attachment and exported state records.
3. **Add notifications for useful lifecycle events.** Session creation and operation failures are initial candidates; agent-attention notifications can follow the monitoring work.
4. **Evaluate nested-session hints afterward.** This requires a host-side consumer and should be scoped separately from replacing the sidebar's name input.
5. **Consider forms and action menus once simple prompts are integrated.** A session creation form could add layout and other options without expanding the sidebar's custom input state machine.

When these features reach a release, detect prompt capability and preserve the existing inline input on older installations. Name validation and collision checks should remain in Verij's operation layer; required fields and regex validation improve the dialog experience but do not replace those checks.

Plugin API adoption requires updating the current `zellij-tile = "0.45"` dependency in `verij-plugin/Cargo.toml`. The unreleased SDK also includes breaking changes from [#5611](https://github.com/zellij-org/zellij/pull/5611): `Text::new` becomes `Text::from`, and `NestedListItem::new` takes `Text` instead of `&str`. Review the released SDK and running Zellij together before integration.

**Priority:** prompt input, then notifications, then nested-session hints.
