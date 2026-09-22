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
#[derive(Serialize, PartialEq, Clone, Debug)]
struct SessionSnapshot {
    /// Session name (globally unique within a Zellij server).
    name: String,
    /// True if this is the session the plugin is currently running inside.
    is_current: bool,
    /// Ordered list of tabs in this session.
    tabs: Vec<TabSnapshot>,
}

/// A compact, serializable snapshot of a single tab.
#[derive(Serialize, PartialEq, Clone, Debug)]
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
    /// The ID of the active CLI pipe, if one has connected.
    /// Set by `pipe()`, consumed by `update()` to fan out snapshots.
    active_pipe_id: Option<String>,

    /// Cached snapshot of all known sessions. Replaced atomically on each
    /// `SessionUpdate` — no delta tracking required.
    sessions: Vec<SessionSnapshot>,
}

// ---------------------------------------------------------------------------
// ZellijPlugin implementation
// ---------------------------------------------------------------------------

thread_local! {
    static STATE: std::cell::RefCell<State> = std::cell::RefCell::new(Default::default());
}

fn main() {}

#[no_mangle]
fn load() {
    let mut plugin_configuration = BTreeMap::new();
    STATE.with(|state| {
        use std::convert::TryFrom;
        use zellij_tile::shim::plugin_api::action::ProtobufPluginConfiguration;
        use zellij_tile::shim::prost::Message;
        if let Ok(protobuf_bytes) = zellij_tile::shim::object_from_stdin::<Vec<u8>>() {
            if let Ok(protobuf_configuration) = ProtobufPluginConfiguration::decode(protobuf_bytes.as_slice()) {
                if let Ok(config) = BTreeMap::try_from(&protobuf_configuration) {
                    plugin_configuration = config;
                }
            }
        }

        if let Ok(mut s) = state.try_borrow_mut() {
            s.load(plugin_configuration);
        }
    });

    // Perform subscriptions and permission requests with NO active borrow on STATE
    eprintln!("[verij-plugin] subscribing and requesting permissions");
    subscribe(&[
        EventType::SessionUpdate,
        EventType::TabUpdate,
        EventType::Timer,
        EventType::PermissionRequestResult,
    ]);

    request_permission(&[
        PermissionType::ReadApplicationState,
        PermissionType::ReadCliPipes,
    ]);

    set_timeout(1.0);
}

#[no_mangle]
pub fn update() -> bool {
    use std::convert::TryInto;
    use zellij_tile::shim::plugin_api::event::ProtobufEvent;
    use zellij_tile::shim::prost::Message;
    STATE.with(|state| {
        match zellij_tile::shim::object_from_stdin::<Vec<u8>>() {
            Ok(protobuf_bytes) => {
                match ProtobufEvent::decode(protobuf_bytes.as_slice()) {
                    Ok(protobuf_event) => {
                        match protobuf_event.try_into() {
                            Ok(event) => {
                                if let Ok(mut s) = state.try_borrow_mut() {
                                    s.update(event)
                                } else {
                                    eprintln!("[verij-plugin] State busy during update, skipping");
                                    false
                                }
                            }
                            Err(e) => {
                                eprintln!("[verij-plugin] ProtobufEvent try_into error: {:?}", e);
                                false
                            }
                        }
                    }
                    Err(e) => {
                        eprintln!("[verij-plugin] ProtobufEvent decode error: {:?}", e);
                        false
                    }
                }
            }
            Err(e) => {
                eprintln!("[verij-plugin] update object_from_stdin error: {:?}", e);
                false
            }
        }
    })
}

#[no_mangle]
pub fn pipe() -> bool {
    use std::convert::TryInto;
    use zellij_tile::shim::plugin_api::pipe_message::ProtobufPipeMessage;
    use zellij_tile::shim::prost::Message;
    STATE.with(|state| {
        match zellij_tile::shim::object_from_stdin::<Vec<u8>>() {
            Ok(protobuf_bytes) => {
                match ProtobufPipeMessage::decode(protobuf_bytes.as_slice()) {
                    Ok(protobuf_pipe_message) => {
                        match protobuf_pipe_message.try_into() {
                            Ok(pipe_message) => {
                                if let Ok(mut s) = state.try_borrow_mut() {
                                    s.pipe(pipe_message)
                                } else {
                                    eprintln!("[verij-plugin] State busy during pipe, skipping");
                                    false
                                }
                            }
                            Err(e) => {
                                eprintln!("[verij-plugin] ProtobufPipeMessage try_into error: {:?}", e);
                                false
                            }
                        }
                    }
                    Err(e) => {
                        eprintln!("[verij-plugin] ProtobufPipeMessage decode error: {:?}", e);
                        false
                    }
                }
            }
            Err(e) => {
                eprintln!("[verij-plugin] pipe object_from_stdin error: {:?}", e);
                false
            }
        }
    })
}

#[no_mangle]
pub fn render(rows: i32, cols: i32) {
    STATE.with(|state| {
        if let Ok(mut s) = state.try_borrow_mut() {
            s.render(rows as usize, cols as usize);
        }
    });
}

#[no_mangle]
pub fn plugin_version() {
    println!("{}", zellij_tile::prelude::VERSION);
}

impl ZellijPlugin for State {
    fn load(&mut self, _configuration: BTreeMap<String, String>) {
        eprintln!("[verij-plugin] State::load called");
    }

    /// Called when a Zellij pipe message arrives for this plugin.
    fn pipe(&mut self, pipe_message: PipeMessage) -> bool {
        eprintln!("[verij-plugin] pipe() called: name={}, source={:?}", pipe_message.name, pipe_message.source);
        if pipe_message.name == "verij_events" {
            match &pipe_message.source {
                PipeSource::Cli(pipe_id) => {
                    eprintln!("[verij-plugin] CLI pipe connected, pipe_id={}", pipe_id);
                    self.active_pipe_id = Some(pipe_id.clone());
                    block_cli_pipe_input(pipe_id);

                    // If we already have a cached snapshot (e.g. the TUI reconnected),
                    // immediately push the current state so the TUI doesn't wait for
                    // the next SessionUpdate.
                    self.broadcast_snapshot();
                }
                _ => {
                    // Other pipe sources (Plugin, Keybind, etc.) ignored.
                }
            }
        }
        false // headless: never request a render
    }

    /// Called when a subscribed event arrives from Zellij Core.
    fn update(&mut self, event: Event) -> bool {
        match event {
            // SessionUpdate carries the full list of live sessions plus their tabs.
            Event::SessionUpdate(session_infos, _resurrectable) => {
                self.update_sessions(session_infos);
            }

            // Periodic timer: polls all sessions and tabs across the machine via get_session_list()
            Event::Timer(_) => {
                set_timeout(1.0);
                if let Ok(snapshot) = get_session_list() {
                    self.update_sessions(snapshot.live_sessions);
                }
            }

            Event::TabUpdate(tabs) => {
                // When tabs are updated within the active session, update our cache and broadcast
                if let Some(current) = self.sessions.iter_mut().find(|s| s.is_current) {
                    let new_tabs: Vec<TabSnapshot> = tabs
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
                        .collect();
                    if current.tabs != new_tabs {
                        current.tabs = new_tabs;
                        self.broadcast_snapshot();
                    }
                }
            }

            Event::PermissionRequestResult(_) => {
                set_timeout(1.0);
            }

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
    /// Update internal session cache and broadcast if anything changed.
    fn update_sessions(&mut self, session_infos: Vec<SessionInfo>) {
        let new_sessions: Vec<SessionSnapshot> = session_infos
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

        if new_sessions != self.sessions {
            self.sessions = new_sessions;
            self.broadcast_snapshot();
        }
    }

    /// Serialize the current `sessions` cache to a single-line JSON string and
    /// write it to the active CLI pipe, if one is connected.
    ///
    /// Single-line JSON is used so that the CLI-side `BufReader::read_line()`
    /// can treat each newline as a complete, self-contained snapshot message.
    fn broadcast_snapshot(&self) {
        let Some(pipe_id) = &self.active_pipe_id else {
            // No CLI pipe connected yet; nothing to do.
            return;
        };

        match serde_json::to_string(&self.sessions) {
            Ok(json) => {
                // Append a newline so the CLI BufReader sees a complete line.
                let line = format!("{}\n", json);
                cli_pipe_output(pipe_id, &line);
            }
            Err(e) => {
                // Serialization should never fail for these simple types,
                // but we log to stderr defensively (visible in Zellij logs).
                eprintln!("[verij-plugin] Failed to serialize snapshot: {e}");
            }
        }
    }
}
