# Coding Agent Monitoring Design

Status: **approved product design; implementation pending**.

This document records the agreed MVP behavior for coding agent monitoring in Verij. The companion [implementation guide](AGENT_MONITORING_IMPLEMENTATION.md) specifies the engineering work, integration contracts, validation gates, and acceptance matrix. The existing [architecture document](ARCHITECTURE.md) describes the current implementation, not this proposed feature.

## 1. Decisions

| Area | Agreed behavior |
|---|---|
| Hierarchy | Extend the sidebar to **Session → Tab → Agent pane**. |
| Row ownership | Each agent row represents an actual Zellij terminal pane containing a supported agent. |
| MVP agents | OpenCode and Antigravity CLI (`agy`), including Magy interactive sessions and pane-backed review runs. |
| Installation | Install adapters once, then launch agents normally. A Verij-specific agent launcher is not required. |
| Activation | Click or Enter attaches the correct inner session, reveals and focuses the target pane, and transfers keyboard focus to the host Workspace pane. |
| Hidden panes | Floating panes and collapsed stack members remain discoverable and must be revealed when activated. |
| Completion | A completed turn displays **Done until visited**, while the agent remains open. |
| Acknowledgement scope | Each Verij host independently acknowledges completed turns. |
| Exit | Remove the agent row when the agent exits or its pane closes. Do not retain closed-pane completion rows. |
| Titles | Use the effective Zellij pane title by default. Allow per-agent title-source and fallback-order overrides. |
| Presentation | Extend the existing tree-format system with agent rows, status indicators, and parent summaries. |

These decisions do not require another product-design round. Engineering uncertainties are listed in the implementation guide and should be resolved through targeted investigation and tests.

## 2. Scope

The MVP monitors live, local, pane-backed agents across Verij's inner Zellij sessions. It supports ordinary tiled panes, floating panes, stacked panes, background tabs, and sessions not currently attached to a particular host.

The MVP includes:

- Normal interactive `opencode` and `agy` invocations after adapter installation.
- Magy interactive Agy panes, including their profile-specific environments.
- Magy review runs that execute in user-facing panes, even though Agy runs in print/headless mode inside those panes.
- Initial state recovery when the sidebar starts after an agent.
- Live status, title, pane-movement, and process-exit updates.
- Host-local completion acknowledgement and exact pane navigation.

The tree is a pane navigator, not a conversation archive. Detached jobs without an independently navigable pane, historical conversations, agent launch orchestration, transcript browsing, remote-agent transport, and closed-pane history are outside this MVP. An adapter must not attach a detached job to its launcher's pane merely because it inherited Zellij environment variables.

Native subagents that execute within the same pane contribute to that pane's status. A child agent that runs in its own pane gets its own row.

## 3. User Experience

### 3.1 Tree

Illustrative appearance:

```text
▾ backend                         !1 ✓1
  ├─ ▾ implementation
  │    ├─ ⠋ opencode  API changes
  │    └─ ! agy       Review tests
  ├─ ▾ investigation
  │    └─ ✓ opencode  Trace timeout
  └─   server

▸ frontend                        ⠋2
```

The actual fold and branch glyphs follow the user's existing tree configuration.

- Sessions contain tabs as before.
- Tabs with monitored agent panes gain agent children and can be folded independently.
- Tabs without agent panes remain navigable leaves.
- Ordinary shell, editor, and plugin panes do not create agent rows.
- New agent children are visible by default unless their session or tab was explicitly collapsed.
- New events must not expand a subtree that the user collapsed.
- Status and title changes update row contents without reordering rows or moving the cursor.
- A pane moving to another tab carries its logical selection with it.
- A disappearing selected agent falls back to its surviving parent tab, then session, then the nearest surviving row.

Order sessions and tabs using the current inventory ordering. Within a tab, use a deterministic pane order, initially terminal pane ID order. Do not sort agent rows by status or title; attention should be conveyed by indicators and summaries rather than cursor-moving reorderings.

### 3.2 Controls

Preserve existing navigation keys and `single_click_action` behavior:

| Input | Behavior |
|---|---|
| `j` / `k`, arrows, page keys | Navigate the flattened visible tree. |
| `Enter` | Activate the selected session, tab, or agent pane. |
| Click row | Activate immediately when `single_click_action = true`; otherwise select and activate on double-click. |
| Click fold marker | Fold or unfold the selected session or tab without activating it. |
| `Right` / `l` | Expand a collapsed branch; otherwise select its first child. On an agent leaf, no structural action. |
| `Left` / `h` | Collapse an expanded branch; otherwise select its parent. On an agent leaf, select its tab. |
| `Space` / `Tab` | Toggle the selected branch. On an agent leaf, collapse its parent tab and select that tab. A tab with no agent children retains the existing parent-session folding behavior. |

The new meaning of `Space` on a tab with agent children is intentional: it now toggles that tab's subtree rather than always folding the entire session. Update the help text and control documentation accordingly.

### 3.3 Status Indicators

| Display state | Default indicator | Meaning |
|---|---|---|
| Working | Animated spinner | The agent is thinking, executing tools, retrying, or performing tracked work. |
| Needs input | Amber `!` | An explicit permission request, question, or equivalent blocking input request is outstanding. |
| Done | Green `✓` | A turn completed successfully and this host has not yet visited the pane. |
| Idle | Muted `○` | The agent is open with no active work, blocking request, or unread completion. |
| Error | Red `×` | The agent reports a terminal execution failure. |
| Unknown | Muted `?` | An agent instance is known, but current semantic status cannot be established reliably. |

Indicators must remain distinguishable without color. Animation uses the TUI tick and only requires redraws while a visible working indicator or summary is animated.

The following are not equivalent:

- A live process is not necessarily Working.
- An idle agent is not necessarily asking a question.
- A tool error is not necessarily an agent execution failure.
- A quiet terminal is not proof of completion.
- A child/subagent finishing is not proof that the parent turn finished.
- A stale report is not proof that the agent exited.

Use structured adapter signals for status. Do not infer Needs input because the last assistant message ends in a question mark. Do not infer Done from a lack of terminal output.

### 3.4 Parent Summaries

Session and tab rows expose descendant counts for Working, Needs input, unread Done, Error, Unknown, and total agent panes. Counts refer to pane rows, not the number of internal conversations or subagents.

The default summary displays nonzero attention, completion, and activity counts, in this order:

1. Error.
2. Needs input.
3. Unread Done for this host.
4. Working.
5. Unknown.

Idle is available as a count but need not consume space in the default summary. Summaries remain visible when branches are collapsed and do not require expanding the branch to update. They use the same reducer output as agent rows.

For narrow sidebars, truncate titles and summaries without losing the fold target or status indicator. Do not introduce horizontal scrolling as a prerequisite for the MVP.

## 4. Completion and Visits

### 4.1 Separate execution from acknowledgement

Done is a presentation state derived from a completion record and host-local acknowledgement. It is not a mutation of the agent's global execution state.

Keep at least:

- The current execution state and active turn/execution identity.
- A monotonic completion revision for successful aggregate turn completion.
- The turn identity associated with that revision.
- The highest completion revision acknowledged by each host for that agent instance.

Increment the completion revision exactly once for a successful turn completion, not once for every duplicate `idle` or `stop` notification. Repeating an unchanged idle snapshot must not recreate an unread badge.

A new turn replaces the previous completed turn as the current activity. Acknowledging an earlier completion must not acknowledge a later completion that arrives during navigation.

### 4.2 What counts as a visit

A visit requires the target pane to be effectively visible and focused through this host's Workspace client:

- The host Workspace pane has keyboard focus.
- It is attached to the target inner session.
- The target inner tab and pane are active for that client.
- The correct tiled/floating layer is visible, and the target stack member is expanded.

Successful activation through Verij is one way to visit. Navigation directly inside Zellij should also count when Verij can observe the same effective-focus conditions. If a turn completes while those conditions already hold, the completion can be acknowledged immediately.

The following do not count:

- Moving the sidebar cursor onto an agent row.
- Merely rendering its row.
- Attaching its session while viewing another pane.
- Focusing a different pane in its tab.
- An inner pane remaining marked focused while host keyboard focus is in the sidebar.
- Another Verij host visiting the same agent.
- A focus command being dispatched without confirmed success.

Permission requests and questions stay Needs input until the agent reports resolution. Visiting does not clear them. Likewise, visiting alone does not clear an execution error.

### 4.3 Host-local persistence

Store acknowledgement separately from shared agent observations. Key it by the host registry's stable marker key so renaming a host preserves its acknowledgement state.

Acknowledgement should survive restarting `verij ui` while the agent instance remains alive. Old acknowledgement must not leak into a restarted agent, a resurrected session, or a reused pane ID. Prune records after the corresponding live instance is definitively gone.

## 5. Titles and Configuration

### 5.1 Effective pane title

Zellij 0.45.1 resolves its effective terminal pane title in this order:

1. Explicit pane name, including a name from a layout or manual rename.
2. Application-provided terminal title.
3. Zellij's fallback title.

Thus an agent's terminal-title updates naturally reach Verij through pane inventory updates unless an explicit pane name takes precedence. Verij should consume that effective title, not overwrite the pane name to implement sidebar labels.

### 5.2 Title-source policy

The agreed configuration shape is:

```toml
[agents]
title_sources = ["pane", "conversation", "agent"]

# Optional example override; this is not a built-in OpenCode default.
[agents.opencode]
title_sources = ["conversation", "pane", "agent"]

# Optional explicit equivalent of the global default.
[agents.agy]
title_sources = ["pane", "conversation", "agent"]
```

| Source | Value |
|---|---|
| `pane` | Effective Zellij pane title, including explicit-name precedence. |
| `conversation` | The adapter-reported conversation title, when available. |
| `agent` | The agent's display name, such as `opencode` or `agy`. |

Rules:

- The global default is `["pane", "conversation", "agent"]` for both MVP agents.
- An agent override replaces the entire inherited list; it does not append to it.
- Resolve the first nonempty source after trimming and normalizing display text.
- Do not guess whether a nonempty title is generic. Users control precedence explicitly.
- Missing conversation metadata falls through to the next source.
- If every configured source is empty, fall back to the agent display name so the row remains usable.
- Invalid source names or an empty list produce a configuration warning and fall back to the inherited valid policy.
- Titles and labels never serve as identity keys or navigation targets.

`agy` is the configuration key for Antigravity CLI, including Agy launched through Magy. Magy is a launcher/provider of metadata, not a third agent kind. A pane title such as `Magy work` remains available through the `pane` source.

### 5.3 Row templates

Extend `[tui.tree]` rather than introducing a second formatting language:

```toml
[tui.tree]
# Illustrative unstyled template. Defaults can include status-specific styles.
agent_format = '#{tree_prefix}#{branch} #{status_icon} #{agent_name} #{agent_title}'

[tui.tree.agent_styles]
normal = ""
selected = "fg=selected_fg,bg=selected_bg"
active = "fg=active_fg,bg=active_bg"
both = "fg=selected_fg,bg=selected_bg"
```

The renderer supplies ancestor indentation through `tree_prefix`; `branch` remains the first/middle/last glyph among the row's siblings. The implementation must specify sibling metadata explicitly rather than infer it from adjacent flattened rows.

Add these variables:

| Variable | Availability / meaning |
|---|---|
| `tree_prefix` | Ancestor continuation/indentation before the branch at this depth. |
| `agent_name` | Agent display name on agent rows. |
| `agent_title` | Title resolved by the per-agent policy. |
| `pane_title` | Raw effective Zellij title on agent rows. |
| `conversation_title` | Adapter-reported title or empty text. |
| `pane_id` | Terminal pane ID, for display/debugging. |
| `agent_status` | `working`, `needs_input`, `done`, `idle`, `error`, or `unknown`. |
| `status_icon` | The corresponding glyph or current spinner frame. |
| `status_detail` | A short normalized reason, such as `permission`, `retrying`, or `background tasks`. |
| `agent_summary` | Compact descendant status counts on session/tab rows. |
| `agent_count` | Total monitored descendant pane count. |
| `working_count`, `needs_input_count`, `done_count`, `error_count`, `unknown_count`, `idle_count` | Host-derived descendant counts on session/tab rows. |

Add conditions for `has_agents`, `working`, `needs_input`, `done`, `idle`, `error`, `unknown`, `floating`, `stacked`, `first_agent`, and `last_agent`. Extend `collapsed`, `expanded`, and `fold_marker` to tabs with children. Define status/placement conditions as false on row kinds where they do not apply.

Keep `selected` distinct from `active`: selected is the sidebar cursor, while an agent's active style follows its effective focus in this host. Existing custom session/tab templates remain valid; users must opt into new variables when they have overridden those templates. New default templates include tab folding and summaries.

Normalize line breaks and control characters in agent-provided text. Preserve existing width-aware truncation, selection styles, and fold-marker hitboxes. Optional metadata such as full title and placement can be shown in the help/detail area without making it another tree level.

## 6. Navigation Contract

Activation is one operation with a target identity, not a loose sequence of session, tab, and pane commands:

```text
Resolve live target
    → attach/switch this host's Workspace client if needed
    → reveal and focus exact terminal pane
    → focus host Workspace pane
    → confirm effective target focus
    → acknowledge the observed completion revision
```

| Target condition | Required result |
|---|---|
| Tiled pane in current tab | Focus the pane. |
| Pane in another tab | Select its current tab and focus it. |
| Hidden floating pane | Reveal the floating layer and raise/focus the target. |
| Collapsed stacked pane | Expand the target within its existing stack. |
| Target obscured by another fullscreen pane | Reveal the target and focus it. |
| Different inner session | Switch this host's nested client, then satisfy the same pane-focus contract. |
| No current Workspace attachment | Attach once, wait for the correct client to become ready, then focus the target. |
| Pane moved after the click | Resolve its new tab using pane identity. |
| Agent/pane exited after the click | Refresh inventory and report an unavailable target without acknowledging completion. |

Do not unstack or convert a valid floating pane into a tiled pane to achieve focus. Do not blindly toggle floating visibility: a retry must be idempotent. An unavailable agent row does not resurrect its former agent process. Session-level resurrection remains available through existing session rows.

Use bounded, state-confirmed navigation. A successful CLI exit code or elapsed sleep alone does not prove that the correct nested client is displaying the target. Older requests must not steal focus after the user activates another target.

## 7. Architecture and Ownership

```text
Inner verij-plugin ── session/tab/pane snapshots ──┐
                                                  │
OpenCode bridge ──┐                                ▼
                 ├─ native reporter ── agent records ── watcher/reducer ── tree
Agy callbacks ───┤                                ▲
Magy run data ───┘                                │
                                      host-local acknowledgements
```

### Zellij inventory

The existing inner WASM plugin owns session/tab/pane topology and pane-focus operations. Extend its snapshots to include terminal pane IDs, tab membership, titles, placement, liveness metadata, and the focus information necessary for host-aware navigation.

Export hidden floating and stacked panes. `is_suppressed` is not a dead-pane flag. Session/tab names and tab positions are mutable metadata; avoid making them the lifetime identity of an agent instance.

### Agent observation

Adapters own semantic observations such as busy, blocked, complete, and failed. Native reporting owns validation, atomic runtime storage, and source ordering. It must function while no sidebar is running.

Use runtime files rather than requiring a persistent Verij agent-monitoring daemon for this MVP. Keep agent files separate from existing session JSON snapshots so that a tool event does not cause a full Zellij session inventory query.

### Derived state

A single native reducer combines observations, current pane/process liveness, and host-local acknowledgement. Rows and parent summaries consume the same derived state. Adapter mappings, UI event handlers, and rendering code must not each implement independent interpretations of Done or Needs input.

### Identity and liveness

Use distinct identities for:

- The Zellij server/session incarnation.
- The terminal pane within that incarnation.
- The agent process instance within that pane.
- The agent's conversation and current execution/turn.

Validate pane association against actual local process ownership or an explicit runner binding. Environment variables are useful hints, not sufficient proof. Session/pane renames do not create new agent instances. Process restart, pane reuse, and session resurrection do.

## 8. Agent Integrations

### 8.1 OpenCode

Use structured plugin events and a pane-local binding. Relevant signals include execution status, explicit permission/question requests and replies, successful completion, terminal failure, and session/title updates.

The version inspected during design was **OpenCode 1.18.32**. Reference repositories also contain OpenCode 2.x integrations with different plugin APIs and a shared background server. Implement and validate the installed 1.x path first; detect unsupported versions explicitly. The adapter boundary should allow a separate 2.x implementation without changing Verij's normalized protocol.

Do not blindly attribute a server plugin's environment to the active TUI. With a shared or attached server, that environment can belong to another pane. An adapter must bind observations to the local TUI and the conversation family it owns/displays. Working directory equality is not sufficient, since two agents can work in the same repository.

Seed current status and outstanding input requests when connecting or reloading mid-turn. Aggregate relevant child activity and input requests without turning a child's completion into the pane's Done state. Do not count unrelated conversations elsewhere on a shared server.

### 8.2 Agy

Use the current documented structured interfaces:

- The interactive status-line callback exposes `agent_state`, `tool_confirmation_pending`, `conversation_id`, and `task_count`.
- Lifecycle hooks expose invocation and execution-stop information, including `terminationReason`, `error`, and `fullyIdle`.

The interactive callback supplies current activity and permission-wait state. Lifecycle hooks supplement successful completion and failures and cover print/headless execution in a pane. Agy's callback command must compose with an existing user status-line command rather than silently replace it.

Mapping constraints:

- `PostInvocation` means one model invocation finished, not that the entire turn finished.
- `Stop` with `fullyIdle = false` must not produce Done while tracked work remains.
- A tool-confirmation dialog takes precedence over otherwise busy/idle activity.
- Explicit question detection must be verified against the installed version. Do not silently label an unobservable question as confirmed Idle; report a capability limitation and conservative state where necessary.
- Monitoring hooks must not auto-approve tools, inject conversation messages, force continuation, or change permission behavior.

### 8.3 Magy

Magy interactive panes use the Agy adapter, with pane association resolved in the newly created pane rather than in the MCP server that launched it.

Magy review panes run Agy headlessly inside a user-facing runner pane. Their monitoring must not depend on an interactive status-line callback. Use verified lifecycle hooks and, where needed, a small explicit runner bridge for pane identity, process start/exit, and run outcome.

Magy changes `$HOME` for account isolation. Adapter installation and runtime discovery must work across those profile homes. Prefer an absolute reporter executable path and a shared per-user runtime location; do not create separate agent registries under each synthetic home.

A completed review pane normally closes automatically. Its row is removed immediately once closure is confirmed. The Done badge can be transient or unobservable in that case; this is intentional. Review history and results remain with Magy.

## 9. Failure and Compatibility Behavior

- Missing adapters do not break ordinary session/tab navigation.
- Known live agents with incomplete observations display Unknown; do not create Unknown rows for arbitrary ordinary panes.
- Adapter reporting failure does not block or fail the agent's work.
- Agent process exit removes its row even if the shell and terminal pane remain.
- Confirmed pane/session disappearance removes the corresponding live associations.
- Temporary inventory omission during pane movement gets a short reconciliation grace period.
- A stale source loses authority; a quiet but live event-only source does not become stale merely because it emitted no new event.
- A restart/resurrection invalidates old process and pane associations before accepting new reports.
- Older plugin snapshots without pane fields remain readable; they provide the existing two-level tree and cannot claim agent-navigation support.
- New fields use versioned/compatible decoding. Malformed agent files are isolated from valid records and existing session inventory.

## 10. Reference Findings

Reference code is a source of techniques, not a dependency or an API guarantee. Verify event names and Zellij behavior against the installed versions before using them.

| Reference | Relevant lesson |
|---|---|
| [zellaude](https://github.com/ishefi/zellaude) | Hook-driven status, ordering, and pane-manifest cleanup. |
| [zellij-ai-session](https://github.com/snail-vs/zellij-ai-session) | Conversation indexing and resume metadata; cwd matching is insufficient for exact live-pane attribution. |
| [zj-agent-mob](https://github.com/mohseenrm/zj-agent-mob) | Cross-session state files, attention categories, and subagent counters. |
| [zj-radar OpenCode bridge](https://github.com/marktoda/zj-radar/blob/main/crates/cli/src/setup/opencode_tui_plugin.js) | TUI-local OpenCode attribution, serialized status edges, coalesced activity updates, and conversation-family filtering. |
| [Herdr](https://github.com/herdrdev/herdr) | Per-agent integration boundaries and authoritative lifecycle signals before screen-detection fallback. |
| [agent-deck OpenCode SSE watcher](https://github.com/asheshgoplani/agent-deck/blob/main/internal/session/opencode_sse.go) | Reconnect snapshots and relevant child-session activity aggregation. |
| [agent-deck status reducer](https://github.com/asheshgoplani/agent-deck/blob/main/internal/sessionstatus/sessionstatus.go) | Centralized status derivation and separating acknowledgement from execution. |

Direct contracts examined:

- [OpenCode plugin documentation](https://opencode.ai/docs/plugins/).
- [Agy status-line callback](https://antigravity.google/docs/cli/statusline/).
- [Agy lifecycle hooks](https://antigravity.google/docs/hooks/).
- [Agy terminal-title customization](https://antigravity.google/docs/cli/title/).
- [Zellij 0.45.1 plugin shims](https://github.com/zellij-org/zellij/blob/v0.45.1/zellij-tile/src/shim.rs): `switch_session_with_focus`, `focus_terminal_pane`, and `show_pane_with_id`.
- [Zellij 0.45.1 tab focus implementation](https://github.com/zellij-org/zellij/blob/v0.45.1/zellij-server/src/tab/mod.rs): floating and hidden stack-member handling.
- [Zellij 0.45.1 tiled focus implementation](https://github.com/zellij-org/zellij/blob/v0.45.1/zellij-server/src/panes/tiled_panes/mod.rs): stack expansion and fullscreen handling.
- [Zellij 0.45.1 effective terminal title](https://github.com/zellij-org/zellij/blob/v0.45.1/zellij-server/src/panes/terminal_pane.rs): `current_title` precedence.

Do not carry over these reference-project assumptions:

- “OpenCode has no integration hooks, so read SQLite timestamps for live status.” OpenCode has structured plugin/event interfaces.
- “Agy requires screen scraping for every lifecycle state.” Its current callback and hook contracts provide structured signals.
- “`should_float_if_hidden` is a general floating-layer toggle.” It controls suppressed-pane handling; verify the complete native focus behavior instead of converting pane placement indiscriminately.
- “Focus clears the shared status.” In Verij it only advances this host's completion acknowledgement.
