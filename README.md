# Verij

Verij adds a host-session layer above Zellij. A persistent Ratatui sidebar lists inner Zellij sessions and tabs; each host-session Workspace pane displays its selected inner session.

Project is functional but experimental. Core navigation, distributed state sync, pane naming, nested-session attachment, and per-host attachment persistence are implemented.

## Topology

```text
Host session: _vj_<name>
|
|-- Verij pane
|   `-- verij ui
|
|-- Workspace pane
|   `-- zellij attach <inner-session>
|
`-- hidden host plugin

Inner session: backend
|-- tabs
|-- panes
`-- verij-plugin.wasm
```

The host session is filtered from the sidebar. Inner sessions remain normal Zellij sessions and retain responsibility for their own tabs and panes.

## Requirements

- Zellij 0.45 or compatible plugin API
- Rust toolchain
- `wasm32-wasip1` target
- `script` for fake-PTY session initialization on Linux-like systems

Install the WASM target once:

```bash
rustup target add wasm32-wasip1
```

## Build And Run

Build CLI and plugin artifacts:

```bash
make dev
```

Start a host session:

```bash
target/release/verij start
```

Or build and start in one step:

```bash
make run
```

Useful Make targets:

```text
make build-plugin       Release WASM plugin
make build-plugin-dev   Debug WASM plugin
make build-cli          Release CLI
make build-cli-dev      Debug CLI
make check              Native and WASM checks
make clean              Remove build artifacts
```

The CLI also supports:

```bash
verij start [--session-name NAME] [--sidebar-width WIDTH]
verij attach [SESSION] [--create]
verij ui
verij config init
verij config path
```

Host sessions receive the configured workspace prefix. Default prefix is `_vj_`; `verij start --session-name work` therefore creates `_vj_work`.

## Configuration

Config path is `~/.config/verij/config.toml`, or `$XDG_CONFIG_HOME/verij/config.toml` when `XDG_CONFIG_HOME` is set. Per-host attachment state is stored in the sibling `hosts.toml` file.

```toml
[workspace]
prefix = "_vj_"
sidebar_width = "25%"
pane_format = "{session}{if tab} | {tab}{endif}{if pane} | {pane}{endif}"
pane_default = "Workspace"
```

`pane_format` supports:

- `{session}`: inner session name
- `{tab}` or `{active_tab}`: active inner tab name
- `{pane}` or `{active_pane}`: active inner pane title
- `{if variable}...{else}...{endif}`: optional conditional section

Default format renders cleanly as `session`, `session | tab`, `session | pane`, or `session | tab | pane` without dangling separators. Conditional sections are simple and non-nested.

Recent `tab_format` and `tab_default` keys remain accepted as compatibility aliases, but names now apply to the Workspace pane.

## TUI Controls

| Key | Action |
|---|---|
| `j` / `k`, arrows | Move selection |
| `PageUp` / `PageDown` | Page navigation |
| `g` / `Home` | Go to first item |
| `G` / `End` | Go to last item |
| `Space` / `Tab` | Fold or unfold session tabs |
| `h` / `Left` | Collapse or select parent |
| `l` / `Right` | Expand or enter tab list |
| `Enter` | Attach session or switch tab |
| Mouse click | Select item |
| Mouse double-click | Attach or switch |
| `n`, `c`, `+` | Create inner session |
| `?` | Toggle help |
| `q`, `Esc`, `Ctrl-c` | Exit sidebar |

## Runtime Model

Each inner session runs a headless `verij-plugin.wasm`. It exports atomic JSON snapshots to `/tmp/verij/states/`. The host TUI watches that directory and rebuilds its session/tab tree when snapshots change.

Session changes use the Inception Switch: the sidebar sends `switch:<target>` to the currently attached inner agent, which calls Zellij's native `switch_session` API from inside the inner session. Tab changes use Zellij's session-targeted `go-to-tab` action.

Workspace attachment state is host-local. Verij persists the last inner session for each host in `hosts.toml`. The attach shell also sets `VERIJ_WORKSPACE_SESSION`; a marker file lets the TUI recover after restart. Detaching clears runtime markers while preserving the durable last-session record for future restoration. This avoids using global inner-session client counts, which cannot identify the host Workspace client and can cause nested Zellij attaches.

When a host or remembered inner session is listed by Zellij as `EXITED`, `verij attach` uses Zellij session resurrection with `--force-run-commands`. Live sessions use the normal attach path. Multiple hosts keep independent remembered inner sessions while sharing the sidebar's global session tree.

To preserve pane viewport and scrollback across resurrection, enable these Zellij options:

```kdl
session_serialization true
serialize_pane_viewport true
scrollback_lines_to_serialize 0
```

These serialization options require restarting the Zellij server. Existing resurrection snapshots cannot regain scrollback that was never serialized.

Zellij 0.45.1 can retain `start_suspended` in serialized layouts despite `--force-run-commands`; Verij removes that stale flag before automatic resurrection.

Host resurrection also replaces stale serialized Workspace-pane attach commands with the host's durable last-session target.

New inner sessions receive a fake-PTY attach while their initial layout is created. Verij waits for a visible layout plugin pane when the default layout is available, then detaches the fake client before attaching the Workspace pane.

## Documentation

- [Architecture](docs/ARCHITECTURE.md)
- [Progress and roadmap](progress.md)

## License

See [LICENSE](LICENSE).
