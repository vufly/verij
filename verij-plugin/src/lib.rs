/// verij-plugin/src/main.rs
///
/// Headless Zellij WASM plugin for Verij.
///
/// Responsibilities:
///   1. Subscribe to `SessionUpdate` events from Zellij Core.
///   2. Listen for the CLI pipe named "verij_events" connecting via `pipe()`.
///   3. On every `SessionUpdate`, serialize the full session snapshot to JSON
///      and stream it to the connected CLI pipe via `cli_pipe_output`.
///
/// This plugin has NO visual output (render() is a no-op). It acts purely as
/// a bridge between Zellij's internal state and the external Ratatui TUI process.
use serde::Serialize;
use std::collections::BTreeMap;
use zellij_tile::prelude::*;

// ---------------------------------------------------------------------------
// Wire-format types (must stay in sync with verij-cli/src/pipe_reader.rs)
// ---------------------------------------------------------------------------

/// A compact, serializable snapshot of a single Zellij session.
#[derive(Serialize)]
struct SessionSnapshot {
    /// Session name (globally unique within a Zellij server).
    name: String,
    /// True if this is the session the plugin is currently running inside.
    is_current: bool,
    /// Ordered list of tabs in this session.
    tabs: Vec<TabSnapshot>,
}

/// A compact, serializable snapshot of a single tab.
#[derive(Serialize)]
struct TabSnapshot {
    /// Display name of the tab.
    name: String,
    /// Zero-based position index (used for `go-to-tab` actions).
    position: usize,
    /// Whether this tab is currently focused in its session.
    is_active: bool,
}

// ---------------------------------------------------------------------------
// Plugin state
// ---------------------------------------------------------------------------

/// The plugin's persistent state across event invocations.
#[derive(Default)]
struct State {
    /// The name of the active CLI pipe, if one has connected.
    /// Set by `pipe()`, consumed by `update()` to fan out snapshots.
    active_pipe_name: Option<String>,

    /// Cached snapshot of all known sessions. Replaced atomically on each
    /// `SessionUpdate` — no delta tracking required.
    sessions: Vec<SessionSnapshot>,
}

// ---------------------------------------------------------------------------
// ZellijPlugin implementation
// ---------------------------------------------------------------------------

register_plugin!(State);

impl ZellijPlugin for State {
    /// Called once when the plugin is loaded into Zellij.
    ///
    /// Subscribes to `SessionUpdate` so we receive the full session list
    /// whenever any session or tab changes. Also requests the permissions
    /// needed to read application state and write to CLI pipes.
    fn load(&mut self, _configuration: BTreeMap<String, String>) {
        // Subscribe to the events we care about.
        subscribe(&[EventType::SessionUpdate]);

        // Request required permissions:
        //   - ReadApplicationState: to receive SessionUpdate with full session data.
        //   - ReadCliPipes:         to call cli_pipe_output().
        request_permission(&[
            PermissionType::ReadApplicationState,
            PermissionType::ReadCliPipes,
        ]);
    }

    /// Called when a Zellij pipe message arrives for this plugin.
    ///
    /// When the CLI-side `verij ui` process spawns `zellij pipe --name verij_events`,
    /// Zellij delivers the pipe open-event here. We record the pipe name so that
    /// `update()` knows where to write future snapshots.
    ///
    /// If a payload is present, the pipe is sending us a command. We ignore
    /// commands in this initial implementation — the plugin is write-only.
    ///
    /// Returns `false` because this plugin never renders anything.
    fn pipe(&mut self, pipe_message: PipeMessage) -> bool {
        if pipe_message.name == "verij_events" {
            match &pipe_message.source {
                PipeSource::Cli(_pipe_id) => {
                    // A CLI pipe named "verij_events" has opened. Store its name
                    // so update() can write to it.
                    self.active_pipe_name = Some(pipe_message.name.clone());

                    // If we already have a cached snapshot (e.g. the TUI reconnected),
                    // immediately push the current state so the TUI doesn't wait for
                    // the next SessionUpdate.
                    self.broadcast_snapshot();
                }
                PipeSource::Plugin(_) => {
                    // Inter-plugin pipes are not used in this design; ignore.
                }
            }
        }
        false // headless: never request a render
    }

    /// Called when a subscribed event arrives from Zellij Core.
    ///
    /// On `SessionUpdate`: convert `Vec<SessionInfo>` → `Vec<SessionSnapshot>`,
    /// cache the result, then stream it to the active CLI pipe.
    ///
    /// Returns `false` because this plugin never renders anything.
    fn update(&mut self, event: Event) -> bool {
        match event {
            // SessionUpdate carries the full list of live sessions plus their tabs.
            // The second argument is resurrectable (dead) sessions — we skip those.
            Event::SessionUpdate(session_infos, _resurrectable) => {
                self.sessions = session_infos
                    .into_iter()
                    .map(|s| SessionSnapshot {
                        name: s.name.clone(),
                        is_current: s.is_current_session,
                        tabs: s
                            .tabs
                            .into_iter()
                            .map(|t| TabSnapshot {
                                name: if t.name.is_empty() {
                                    format!("Tab {}", t.position + 1)
                                } else {
                                    t.name
                                },
                                position: t.position,
                                is_active: t.active,
                            })
                            .collect(),
                    })
                    .collect();

                self.broadcast_snapshot();
            }

            // PermissionRequestResult fires after load() requests permissions.
            // We don't need to act on it here; Zellij will begin sending
            // SessionUpdate events as soon as ReadApplicationState is granted.
            Event::PermissionRequestResult(_) => {}

            _ => {}
        }

        false // headless: never request a render
    }

    /// No-op: this plugin has no visual output.
    fn render(&mut self, _rows: usize, _cols: usize) {}
}

// ---------------------------------------------------------------------------
// Internal helpers
// ---------------------------------------------------------------------------

impl State {
    /// Serialize the current `sessions` cache to a single-line JSON string and
    /// write it to the active CLI pipe, if one is connected.
    ///
    /// Single-line JSON is used so that the CLI-side `BufReader::read_line()`
    /// can treat each newline as a complete, self-contained snapshot message.
    fn broadcast_snapshot(&self) {
        let Some(pipe_name) = &self.active_pipe_name else {
            // No CLI pipe connected yet; nothing to do.
            return;
        };

        match serde_json::to_string(&self.sessions) {
            Ok(json) => {
                // Append a newline so the CLI BufReader sees a complete line.
                let line = format!("{}\n", json);
                cli_pipe_output(pipe_name, &line);
            }
            Err(e) => {
                // Serialization should never fail for these simple types,
                // but we log to stderr defensively (visible in Zellij logs).
                eprintln!("[verij-plugin] Failed to serialize snapshot: {e}");
            }
        }
    }
}
