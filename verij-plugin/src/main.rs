/// verij-plugin/src/main.rs
///
/// Distributed WASM Agent for Verij ("The Inside Man").
///
/// Runs headless inside EVERY inner Zellij session.
///
/// Responsibilities:
///   1. State Export: On `SessionUpdate` or `TabUpdate`, serializes the session's
///      own tabs into a `SessionSnapshot` and writes it to
///      `/tmp/verij/states/<session_name>.json`.
///   2. Action Listener: Listens for the `verij_control` pipe. When a payload
///      `switch:<target>` arrives, executes Zellij's native `switch_session(Some(target))`
///      from within the inner session to achieve in-place client switching
///      ("The Inception Switch").
use std::collections::BTreeMap;
use std::path::Path;
use verij_types::{SessionSnapshot, TabSnapshot, VERIJ_CONTROL_PIPE, VERIJ_STATES_DIR};
use zellij_tile::prelude::*;

// ---------------------------------------------------------------------------
// Plugin state
// ---------------------------------------------------------------------------

#[derive(Default)]
struct State {
    /// The name of this session (discovered from `SessionUpdate` where `is_current_session` is true).
    session_name: Option<String>,

    /// Cached list of tabs for this session.
    tabs: Vec<TabSnapshot>,

    /// Number of clients attached to this session in the latest snapshot.
    connected_clients: Option<usize>,

    /// Title of the focused pane in the active tab.
    active_pane: Option<String>,
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
            if let Ok(protobuf_configuration) =
                ProtobufPluginConfiguration::decode(protobuf_bytes.as_slice())
            {
                if let Ok(config) = BTreeMap::try_from(&protobuf_configuration) {
                    plugin_configuration = config;
                }
            }
        }

        if let Ok(mut s) = state.try_borrow_mut() {
            s.load(plugin_configuration);
        }
    });

    eprintln!("[verij-plugin] load: subscribing to SessionUpdate, TabUpdate, PaneUpdate");
    subscribe(&[
        EventType::SessionUpdate,
        EventType::TabUpdate,
        EventType::PaneUpdate,
        EventType::PermissionRequestResult,
    ]);

    request_permission(&[
        PermissionType::ReadApplicationState,
        PermissionType::ChangeApplicationState,
        PermissionType::ReadCliPipes,
    ]);
}

#[no_mangle]
pub fn update() -> bool {
    use std::convert::TryInto;
    use zellij_tile::shim::plugin_api::event::ProtobufEvent;
    use zellij_tile::shim::prost::Message;
    STATE.with(
        |state| match zellij_tile::shim::object_from_stdin::<Vec<u8>>() {
            Ok(protobuf_bytes) => match ProtobufEvent::decode(protobuf_bytes.as_slice()) {
                Ok(protobuf_event) => match protobuf_event.try_into() {
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
                },
                Err(e) => {
                    eprintln!("[verij-plugin] ProtobufEvent decode error: {:?}", e);
                    false
                }
            },
            Err(e) => {
                eprintln!("[verij-plugin] update object_from_stdin error: {:?}", e);
                false
            }
        },
    )
}

#[no_mangle]
pub fn pipe() -> bool {
    use std::convert::TryInto;
    use zellij_tile::shim::plugin_api::pipe_message::ProtobufPipeMessage;
    use zellij_tile::shim::prost::Message;
    STATE.with(
        |state| match zellij_tile::shim::object_from_stdin::<Vec<u8>>() {
            Ok(protobuf_bytes) => match ProtobufPipeMessage::decode(protobuf_bytes.as_slice()) {
                Ok(protobuf_pipe_message) => match protobuf_pipe_message.try_into() {
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
                },
                Err(e) => {
                    eprintln!("[verij-plugin] ProtobufPipeMessage decode error: {:?}", e);
                    false
                }
            },
            Err(e) => {
                eprintln!("[verij-plugin] pipe object_from_stdin error: {:?}", e);
                false
            }
        },
    )
}

#[no_mangle]
pub fn render(_rows: i32, _cols: i32) {
    // Headless agent: no UI render
}

#[no_mangle]
pub fn plugin_version() {
    println!("{}", zellij_tile::prelude::VERSION);
}

impl ZellijPlugin for State {
    fn load(&mut self, _configuration: BTreeMap<String, String>) {
        eprintln!("[verij-plugin] State::load initialized");
    }

    /// Action Listener: receives injected pipe messages.
    ///
    /// Listens for `verij_control`. If the payload is `switch:<target>`,
    /// executes native `switch_session(Some(target))` to switch the attached client.
    fn pipe(&mut self, pipe_message: PipeMessage) -> bool {
        eprintln!(
            "[verij-plugin] pipe received: name='{}', payload={:?}, source={:?}",
            pipe_message.name, pipe_message.payload, pipe_message.source
        );

        if pipe_message.name == VERIJ_CONTROL_PIPE {
            let payload = pipe_message
                .payload
                .as_deref()
                .or_else(|| pipe_message.args.get("payload").map(|s| s.as_str()));

            if let Some(payload_str) = payload {
                let trimmed = payload_str.trim();
                if let Some(target) = trimmed.strip_prefix("switch:") {
                    let target_session = target.trim();
                    eprintln!(
                        "[verij-plugin] executing Inception Switch to session: '{}'",
                        target_session
                    );
                    switch_session(Some(target_session));
                } else {
                    eprintln!("[verij-plugin] unknown verij_control command: '{}'", trimmed);
                }
            } else {
                eprintln!("[verij-plugin] verij_control pipe received with no payload");
            }

            // If invocation originated from CLI pipe, unblock it so calling process exits cleanly.
            if let PipeSource::Cli(ref pipe_id) = pipe_message.source {
                unblock_cli_pipe_input(pipe_id);
            }
        }

        false // headless: never request a render
    }

    /// State Export: triggered on `SessionUpdate` or `TabUpdate`.
    fn update(&mut self, event: Event) -> bool {
        match event {
            Event::SessionUpdate(session_infos, _resurrectable) => {
                // Find our own session from the list of all sessions
                if let Some(current) = session_infos.into_iter().find(|s| s.is_current_session) {
                    let name_changed = self.session_name.as_deref() != Some(&current.name);
                    self.session_name = Some(current.name.clone());
                    let connected_clients = Some(current.connected_clients);
                    let clients_changed = self.connected_clients != connected_clients;
                    self.connected_clients = connected_clients;
                    let active_pane = Self::active_pane_name(&current.tabs, &current.panes);
                    let pane_changed = self.active_pane != active_pane;

                    let new_tabs: Vec<TabSnapshot> = current
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
                        .collect();

                    if name_changed || self.tabs != new_tabs || clients_changed || pane_changed {
                        self.tabs = new_tabs;
                        self.active_pane = active_pane;
                        self.export_state();
                    }
                }
            }

            Event::TabUpdate(tabs) => {
                if self.session_name.is_none() {
                    self.resolve_session_from_snapshot();
                }

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

                if self.tabs != new_tabs {
                    self.tabs = new_tabs;
                    self.active_pane = None;
                    self.export_state();
                }
            }

            Event::PaneUpdate(panes) => {
                if self.session_name.is_none() {
                    self.resolve_session_from_snapshot();
                }

                let active_pane = Self::active_pane_name_from_snapshots(&self.tabs, &panes);
                if self.active_pane != active_pane {
                    self.active_pane = active_pane;
                    self.export_state();
                }
            }

            Event::PermissionRequestResult(_) => {
                self.resolve_session_from_snapshot();
            }

            _ => {}
        }

        false // headless: never request a render
    }

    fn render(&mut self, _rows: usize, _cols: usize) {}
}

// ---------------------------------------------------------------------------
// State Export helper
// ---------------------------------------------------------------------------

impl State {
    fn active_pane_name(tabs: &[TabInfo], panes: &PaneManifest) -> Option<String> {
        let active_position = tabs.iter().find(|tab| tab.active)?.position;
        panes
            .panes
            .get(&active_position)?
            .iter()
            .find(|pane| pane.is_focused && !pane.is_suppressed)
            .map(|pane| pane.title.clone())
    }

    fn active_pane_name_from_snapshots(
        tabs: &[TabSnapshot],
        panes: &PaneManifest,
    ) -> Option<String> {
        let active_position = tabs.iter().find(|tab| tab.is_active)?.position;
        panes
            .panes
            .get(&active_position)?
            .iter()
            .find(|pane| pane.is_focused && !pane.is_suppressed)
            .map(|pane| pane.title.clone())
    }

    /// Attempts to populate session name and initial tabs using `get_session_list()`.
    fn resolve_session_from_snapshot(&mut self) {
        if let Ok(snapshot) = get_session_list() {
            if let Some(current) = snapshot.live_sessions.into_iter().find(|s| s.is_current_session) {
                let name_changed = self.session_name.as_deref() != Some(&current.name);
                self.session_name = Some(current.name.clone());
                let connected_clients = Some(current.connected_clients);
                let clients_changed = self.connected_clients != connected_clients;
                self.connected_clients = connected_clients;
                let active_pane = Self::active_pane_name(&current.tabs, &current.panes);
                let pane_changed = self.active_pane != active_pane;

                let new_tabs: Vec<TabSnapshot> = current
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
                    .collect();

                if name_changed || self.tabs != new_tabs || clients_changed || pane_changed {
                    self.tabs = new_tabs;
                    self.active_pane = active_pane;
                    self.export_state();
                }
            }
        }
    }

    /// Serializes session tabs to `/tmp/verij/states/<session_name>.json`.
    /// Uses atomic write (temp file + rename) to prevent torn reads by the file watcher.
    fn export_state(&self) {
        let Some(ref session_name) = self.session_name else {
            return;
        };

        let dir = Path::new(VERIJ_STATES_DIR);
        if let Err(e) = std::fs::create_dir_all(dir) {
            eprintln!("[verij-plugin] failed to create states dir {}: {}", dir.display(), e);
            return;
        }

        let snapshot = SessionSnapshot {
            name: session_name.clone(),
            is_current: true,
            tabs: self.tabs.clone(),
            active_pane: self.active_pane.clone(),
            connected_clients: self.connected_clients,
            needs_resurrection: false,
        };

        let json = match serde_json::to_string_pretty(&snapshot) {
            Ok(j) => j,
            Err(e) => {
                eprintln!("[verij-plugin] failed to serialize snapshot: {}", e);
                return;
            }
        };

        let target_path = dir.join(format!("{}.json", session_name));
        let tmp_path = dir.join(format!("{}.json.tmp", session_name));

        // Atomic write: write to .tmp then rename
        if let Err(e) = std::fs::write(&tmp_path, &json) {
            eprintln!("[verij-plugin] failed to write tmp state file {}: {}", tmp_path.display(), e);
            // Fallback to direct write
            let _ = std::fs::write(&target_path, &json);
            return;
        }

        if let Err(_rename_err) = std::fs::rename(&tmp_path, &target_path) {
            // If rename fails across wasi/fs boundaries, fallback to direct write
            let _ = std::fs::write(&target_path, &json);
            let _ = std::fs::remove_file(&tmp_path);
        }
    }
}
