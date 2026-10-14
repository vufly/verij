/// verij-plugin/src/main.rs
///
/// Distributed WASM Agent for Verij ("The Inside Man").
///
/// Runs headless inside EVERY inner Zellij session.
///
/// Responsibilities:
///   1. State & Inventory Export: From native topology events and bounded validation,
///      serializes the session's own tabs and full terminal pane topology into
///      `SessionSnapshot` with `SessionInventory` and writes it to
///      `/tmp/verij/states/`.
///   2. Publication Isolation: Writes unique per-exporter inventory files using
///      atomic write+rename to prevent publication races between attached clients.
///   3. Action Listener: Listens for the `verij_control` pipe. When a payload
///      `switch:<target>` arrives, executes Zellij's native `switch_session(Some(target))`
///      from within the inner session to achieve in-place client switching
///      ("The Inception Switch").
use std::collections::BTreeMap;
use std::path::Path;
use std::time::{Duration, Instant};
use verij_types::identity::{PluginContext, TerminalPaneId};
use verij_types::inventory::{
    default_capabilities, legacy_filename, per_exporter_filename, temp_filename, PaneSnapshot,
    SessionInventory, TabMetadata,
};
use verij_types::{SessionSnapshot, TabSnapshot, VERIJ_CONTROL_PIPE, VERIJ_STATES_DIR};
use zellij_tile::prelude::*;
mod control;
mod queries;

const SESSION_SCAN_INTERVAL: Duration = Duration::from_secs(10);

// ---------------------------------------------------------------------------
// Process Locator Helper
// ---------------------------------------------------------------------------

fn resolve_pane_pid(pane_id: u32) -> Option<u32> {
    #[cfg(target_arch = "wasm32")]
    {
        get_pane_pid(PaneId::Terminal(pane_id))
            .ok()
            .filter(|&pid| pid > 0)
            .map(|pid| pid as u32)
    }
    #[cfg(not(target_arch = "wasm32"))]
    {
        let _ = pane_id;
        None
    }
}

// ---------------------------------------------------------------------------
// Plugin state
// ---------------------------------------------------------------------------

#[derive(Default, Clone, Debug)]
pub struct State {
    pub export_dir: String,
    // Native pane leader stays stable while its foreground job changes. Query
    // only new/restarted panes, not every cursor/title/focus refresh.
    pub pane_pids: BTreeMap<u32, (bool, bool, Option<String>, Option<u32>)>,
    /// The name of this session (discovered from `SessionUpdate` where `is_current_session` is true, or `get_session_list()`).
    pub session_name: Option<String>,

    /// Cached list of tabs for this session (for legacy TabSnapshot).
    pub tabs: Vec<TabSnapshot>,

    /// Cached raw tab info from Zellij (for topology and stable tab_id).
    pub raw_tabs: Vec<TabInfo>,

    /// Cached full pane manifest for this session.
    pub panes: PaneManifest,
    /// Direct client events supersede the session-list bootstrap projection.
    pub pane_updates_seen: bool,
    pub tab_updates_seen: bool,

    /// Number of clients attached to this session.
    pub connected_clients: Option<usize>,

    /// Title of the focused pane in the active tab.
    pub active_pane: Option<String>,

    /// Verij-owned plugin load epoch (captured at initialization).
    pub epoch: String,

    /// Monotonic exporter revision.
    pub revision: u64,

    /// Cached producer context.
    pub producer: Option<PluginContext>,

    /// Latest exported inventory snapshot.
    pub last_inventory: Option<SessionInventory>,

    /// Inventory polling shares control's timer without following its fast focus polls.
    pub next_inventory_poll: Option<Instant>,
    /// Full SDK session discovery broadcasts SessionUpdate to every subscriber.
    /// Use it only for bootstrap/slow recovery; topology events keep our cache current.
    pub next_session_scan: Option<Instant>,
    /// Asynchronous own-client query avoids a global session scan.
    pub observed_clients: Option<usize>,
}

thread_local! {
    static STATE: std::cell::RefCell<State> = std::cell::RefCell::new(State::new());
    static CONTROL: std::cell::RefCell<Option<control::Control>> = const { std::cell::RefCell::new(None) };
}

/// Extension helper for sibling modules to interact with State.
pub fn with_state<R>(f: impl FnOnce(&mut State) -> R) -> R {
    STATE.with(|s| {
        let mut state = s.borrow_mut();
        f(&mut state)
    })
}

fn main() {}

// ---------------------------------------------------------------------------
// Manual WASM exports
// ---------------------------------------------------------------------------

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
            let controller = control::Control::load(&plugin_configuration);
            s.epoch = controller.context().epoch.clone();
            s.producer = Some(controller.context().clone());
            CONTROL.with(|control| *control.borrow_mut() = Some(controller));
            s.load(plugin_configuration);
        }
    });

    // GetSessionList is not a passive read in stock 0.45.1: it broadcasts a
    // SessionUpdate and makes all status bars decode/render the global inventory.
    // Receive our small native topology events instead, with slow scan recovery.
    // InitialKeybinds keeps unused keybindings out of ModeUpdate payloads.
    eprintln!("[verij-plugin] load: event-driven topology, validation (1s), session recovery (10s)");
    subscribe(&[
        EventType::PermissionRequestResult,
        EventType::TabUpdate,
        EventType::PaneUpdate,
        EventType::ModeUpdate,
        EventType::InitialKeybinds,
        EventType::ListClients,
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
                        CONTROL.with(|control| {
                            if let Some(controller) = control.borrow_mut().as_mut() {
                                controller.update(&event);
                            }
                        });
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
                        let handled = CONTROL.with(|control| {
                            control
                                .borrow_mut()
                                .as_mut()
                                .is_some_and(|controller| controller.pipe(&pipe_message))
                        });
                        if handled {
                            return false;
                        }
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
    CONTROL.with(|control| {
        if let Some(controller) = control.borrow().as_ref() {
            controller.render();
        }
    });
}

#[no_mangle]
pub fn plugin_version() {
    println!("{}", zellij_tile::prelude::VERSION);
}

// ---------------------------------------------------------------------------
// ZellijPlugin implementation
// ---------------------------------------------------------------------------

impl ZellijPlugin for State {
    fn load(&mut self, configuration: BTreeMap<String, String>) {
        self.export_dir = configuration
            .get("state_dir")
            .cloned()
            .unwrap_or_else(|| VERIJ_STATES_DIR.into());
        self.ensure_epoch();
        eprintln!(
            "[verij-plugin] State::load initialized, epoch={}",
            self.epoch
        );
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
                    #[cfg(target_arch = "wasm32")]
                    switch_session(Some(target_session));
                } else {
                    eprintln!(
                        "[verij-plugin] unknown verij_control command: '{}'",
                        trimmed
                    );
                }
            } else {
                eprintln!("[verij-plugin] verij_control pipe received with no payload");
            }

            // If invocation originated from CLI pipe, unblock it so calling process exits cleanly.
            #[cfg(target_arch = "wasm32")]
            if let PipeSource::Cli(ref pipe_id) = pipe_message.source {
                unblock_cli_pipe_input(pipe_id);
            }
        }

        false // headless: never request a render
    }

    /// State & Inventory Export: triggered on `SessionUpdate`, `TabUpdate`, or `PaneUpdate`.
    fn update(&mut self, event: Event) -> bool {
        match event {
            Event::ModeUpdate(info) => {
                if let Some(name) = info.session_name {
                    if self.session_name.as_deref() != Some(name.as_str()) {
                        if let Some(old) = self.session_name.clone() {
                            self.cleanup_renamed_session_file(&old);
                        }
                        self.session_name = Some(name.clone());
                        #[cfg(target_arch = "wasm32")]
                        CONTROL.with(|control| {
                            if let Some(controller) = control.borrow_mut().as_mut() {
                                controller.set_session(&name);
                            }
                        });
                        if self.last_inventory.is_some() {
                            self.export_state();
                        }
                    }
                }
            }
            Event::ListClients(clients) => {
                self.observed_clients = Some(clients.len());
            }
            Event::SessionUpdate(session_infos, _resurrectable) => {
                if let Some(current) = session_infos.into_iter().find(|s| s.is_current_session) {
                    let old_name = self.session_name.clone();
                    let name_changed = self.session_name.as_deref() != Some(&current.name);
                    self.session_name = Some(current.name.clone());

                    let connected_clients = Some(current.connected_clients);
                    let clients_changed = self.connected_clients != connected_clients;
                    self.connected_clients = connected_clients;

                    let (new_tabs, raw_tabs_changed) = if self.tab_updates_seen {
                        (self.tabs.clone(), false)
                    } else {
                        self.update_tabs_from_info(&current.tabs)
                    };
                    let tabs_changed = self.tabs != new_tabs;
                    self.tabs = new_tabs;

                    let panes_changed = if self.pane_updates_seen {
                        false
                    } else {
                        let changed = Self::terminal_manifest(&self.panes)
                            != Self::terminal_manifest(&current.panes);
                        self.panes = current.panes;
                        changed
                    };

                    let active_pane = Self::active_pane_name(&self.raw_tabs, &self.panes);
                    let pane_changed = self.active_pane != active_pane;
                    self.active_pane = active_pane;

                    if name_changed
                        || tabs_changed
                        || raw_tabs_changed
                        || clients_changed
                        || pane_changed
                        || panes_changed
                    {
                        if name_changed {
                            if let Some(ref old) = old_name {
                                self.cleanup_renamed_session_file(old);
                            }
                        }
                        self.export_state();
                    }
                }
            }

            Event::TabUpdate(tabs) => {
                if self.session_name.is_none() {
                    self.resolve_session_from_snapshot();
                }

                // TabUpdate must retain pane cache, recalculate active title.
                self.tab_updates_seen = true;
                let (new_tabs, raw_tabs_changed) = self.update_tabs_from_info(&tabs);
                let tabs_changed = self.tabs != new_tabs;
                self.tabs = new_tabs;

                let active_pane = Self::active_pane_name(&self.raw_tabs, &self.panes);
                let pane_changed = self.active_pane != active_pane;
                self.active_pane = active_pane;

                if tabs_changed || raw_tabs_changed || pane_changed {
                    self.export_state();
                }
            }

            Event::PaneUpdate(panes) => {
                if self.session_name.is_none() {
                    self.resolve_session_from_snapshot();
                }

                // PaneUpdate caches full terminal panes and recalculates active pane
                self.pane_updates_seen = true;
                let panes_changed =
                    Self::terminal_manifest(&self.panes) != Self::terminal_manifest(&panes);
                self.panes = panes;

                let active_pane = Self::active_pane_name(&self.raw_tabs, &self.panes);
                let pane_changed = self.active_pane != active_pane;
                self.active_pane = active_pane;

                if panes_changed || pane_changed {
                    self.export_state();
                }
            }

            Event::PermissionRequestResult(PermissionStatus::Granted) => {
                self.next_inventory_poll = Some(Instant::now() + Duration::from_secs(1));
                #[cfg(target_arch = "wasm32")]
                list_clients();
                self.resolve_session_from_snapshot();
            }

            Event::Timer(_) if self.inventory_poll_due(Instant::now()) => {
                #[cfg(target_arch = "wasm32")]
                list_clients();
                self.resolve_session_from_snapshot();
            }

            _ => {}
        }

        false // headless: never request a render
    }

    fn render(&mut self, _rows: usize, _cols: usize) {}
}

// ---------------------------------------------------------------------------
// State Methods & Export Helpers
// ---------------------------------------------------------------------------

impl State {
    fn session_scan_due(&self, now: Instant) -> bool {
        self.next_session_scan.is_none_or(|next| now >= next)
    }

    fn cached_session_snapshot(&self) -> Option<SessionInfo> {
        Some(SessionInfo {
            name: self.session_name.clone()?,
            tabs: self.raw_tabs.clone(),
            panes: self.panes.clone(),
            connected_clients: self.observed_clients.or(self.connected_clients).unwrap_or(0),
            is_current_session: true,
            ..Default::default()
        })
    }

    fn inventory_poll_due(&mut self, now: Instant) -> bool {
        match self.next_inventory_poll {
            Some(next) if now >= next => {
                self.next_inventory_poll = Some(now + Duration::from_secs(1));
                true
            }
            _ => false,
        }
    }

    pub fn new() -> Self {
        let epoch = match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
            Ok(d) => d.as_nanos().to_string(),
            Err(_) => "0".to_string(),
        };
        Self {
            export_dir: VERIJ_STATES_DIR.into(),
            epoch,
            revision: 0,
            ..Default::default()
        }
    }

    pub fn ensure_epoch(&mut self) -> &str {
        if self.epoch.is_empty() {
            self.epoch = match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
                Ok(d) => d.as_nanos().to_string(),
                Err(_) => "0".to_string(),
            };
        }
        &self.epoch
    }

    pub fn ensure_producer(&mut self) -> Option<&PluginContext> {
        self.ensure_epoch();
        if self.producer.is_none() {
            #[cfg(target_arch = "wasm32")]
            {
                let ids = get_plugin_ids();
                self.producer = Some(PluginContext {
                    server_pid: ids.zellij_pid,
                    plugin_id: ids.plugin_id,
                    client_id: ids.client_id,
                    epoch: self.epoch.clone(),
                });
            }
            #[cfg(not(target_arch = "wasm32"))]
            {
                self.producer = Some(PluginContext {
                    server_pid: 1,
                    plugin_id: 1,
                    client_id: 1,
                    epoch: self.epoch.clone(),
                });
            }
        }
        self.producer.as_ref()
    }

    pub fn update_tabs_from_info(&mut self, tabs: &[TabInfo]) -> (Vec<TabSnapshot>, bool) {
        let facts = |tabs: &[TabInfo]| {
            tabs.iter()
                .map(|tab| {
                    (
                        tab.tab_id,
                        tab.position,
                        tab.name.clone(),
                        tab.active,
                        tab.are_floating_panes_visible,
                        tab.is_fullscreen_active,
                    )
                })
                .collect::<Vec<_>>()
        };
        let raw_tabs_changed = facts(&self.raw_tabs) != facts(tabs);
        self.raw_tabs = tabs.to_vec();

        let new_tabs: Vec<TabSnapshot> = tabs
            .iter()
            .map(|t| TabSnapshot {
                name: if t.name.is_empty() {
                    format!("Tab {}", t.position + 1)
                } else {
                    t.name.clone()
                },
                position: t.position,
                is_active: t.active,
            })
            .collect();

        (new_tabs, raw_tabs_changed)
    }

    fn terminal_manifest(manifest: &PaneManifest) -> BTreeMap<usize, Vec<PaneInfo>> {
        manifest
            .panes
            .iter()
            .map(|(position, panes)| {
                (
                    *position,
                    panes
                        .iter()
                        .filter(|pane| !pane.is_plugin)
                        .cloned()
                        .map(|mut pane| {
                            // Cursor blinking and plugin bookkeeping are not inventory facts.
                            pane.cursor_coordinates_in_pane = None;
                            pane
                        })
                        .collect(),
                )
            })
            .collect()
    }

    pub fn active_pane_name(tabs: &[TabInfo], panes: &PaneManifest) -> Option<String> {
        let active_position = tabs.iter().find(|tab| tab.active)?.position;
        panes
            .panes
            .get(&active_position)?
            .iter()
            .find(|pane| pane.is_focused && !pane.is_suppressed)
            .map(|pane| pane.title.clone())
    }

    pub fn active_pane_name_from_snapshots(
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

    pub fn build_inventory(
        &mut self,
        now_ms: u64,
        revision: u64,
        producer: &PluginContext,
    ) -> SessionInventory {
        // TabMetadata: stable TabInfo.tab_id where available
        let mut tabs: Vec<TabMetadata> = self
            .raw_tabs
            .iter()
            .map(|t| TabMetadata {
                tab_id: t.tab_id,
                position: t.position,
                floating_visible: t.are_floating_panes_visible,
                fullscreen_active: t.is_fullscreen_active,
            })
            .collect();
        tabs.sort_by_key(|t| t.position);

        // PaneSnapshots: cached full terminal panes for SessionUpdate/PaneUpdate
        // including hidden floats/background tabs/suppressed.
        let mut panes = Vec::new();
        for (&tab_pos, pane_list) in &self.panes.panes {
            let tab_id = self
                .raw_tabs
                .iter()
                .find(|t| t.position == tab_pos)
                .map(|t| t.tab_id);

            for pane in pane_list {
                // Exclude plugin panes, terminal panes only
                if pane.is_plugin {
                    continue;
                }

                let signature = (pane.exited, pane.is_held, pane.terminal_command.clone());
                let pane_pid = match self.pane_pids.get(&pane.id) {
                    Some((exited, held, command, pid))
                        if pid.is_some() && (*exited, *held, command.clone()) == signature =>
                    {
                        *pid
                    }
                    _ => {
                        let pid = if pane.exited {
                            None
                        } else {
                            resolve_pane_pid(pane.id)
                        };
                        self.pane_pids
                            .insert(pane.id, (signature.0, signature.1, signature.2, pid));
                        pid
                    }
                };

                panes.push(PaneSnapshot {
                    terminal_id: TerminalPaneId(pane.id),
                    tab_id,
                    tab_position: tab_pos,
                    title: pane.title.clone(),
                    is_floating: pane.is_floating,
                    is_suppressed: pane.is_suppressed,
                    is_fullscreen: pane.is_fullscreen,
                    layer_focused: pane.is_focused,
                    exited: pane.exited,
                    pane_pid,
                    pane_process: None, // Hydrated by native reader
                    stack_id: None,     // Unsupported stack_id stays None
                });
            }
        }
        panes.sort_by_key(|p| (p.tab_position, p.terminal_id.0));
        self.pane_pids
            .retain(|id, _| panes.iter().any(|pane| pane.terminal_id.0 == *id));

        SessionInventory {
            schema_version: 1,
            producer: producer.clone(),
            revision,
            exported_at_ms: now_ms,
            session_instance_id: None,
            server_process: None,
            capabilities: default_capabilities(),
            tabs,
            panes,
        }
    }

    /// Apply a fresh SDK snapshot, projecting active-tab state for this exporter client.
    /// Global session metadata does not identify which tab a particular client views.
    #[cfg(any(target_arch = "wasm32", test))]
    fn apply_polled_snapshot(&mut self, mut current: SessionInfo, focus: Option<(usize, PaneId)>) {
        if let Some((tab_id, _)) = focus {
            for tab in &mut current.tabs {
                tab.active = tab.tab_id as usize == tab_id;
            }
        }
        let old_name = self.session_name.clone();
        let name_changed = self.session_name.as_deref() != Some(&current.name);
        self.session_name = Some(current.name);
        let clients_changed = self.connected_clients != Some(current.connected_clients);
        self.connected_clients = Some(current.connected_clients);
        let (tabs, raw_tabs_changed) = self.update_tabs_from_info(&current.tabs);
        let tabs_changed = self.tabs != tabs;
        self.tabs = tabs;
        let panes_changed = Self::terminal_manifest(&self.panes)
            != Self::terminal_manifest(&current.panes);
        self.panes = current.panes;
        let active_pane = focus
            .and_then(|(_, pane)| match pane {
                PaneId::Terminal(id) => self.panes.panes.values().flatten()
                    .find(|pane| !pane.is_plugin && pane.id == id)
                    .map(|pane| pane.title.clone()),
                PaneId::Plugin(_) => None,
            });
        let pane_changed = self.active_pane != active_pane;
        self.active_pane = active_pane;
        if name_changed || clients_changed || tabs_changed || raw_tabs_changed
            || panes_changed || pane_changed || self.last_inventory.is_none() {
            if name_changed {
                if let Some(old) = old_name { self.cleanup_renamed_session_file(&old); }
            }
            self.export_state();
        }
    }

    /// Session-list replies can retain a detached client's old topology. Resolve
    /// stable IDs against the live screen before publishing another observation.
    /// A missing object rejects the entire cached manifest; a failed query leaves
    /// the last publication intact until the next poll.
    #[cfg(any(target_arch = "wasm32", test))]
    fn refresh_polled_topology(
        current: &mut SessionInfo,
        mut tab_info: impl FnMut(usize) -> Result<Option<TabInfo>, String>,
        mut pane_info: impl FnMut(PaneId) -> Result<Option<PaneInfo>, String>,
    ) -> Result<bool, String> {
        let mut tabs = Vec::new();
        let mut panes = PaneManifest::default();
        for cached_tab in &current.tabs {
            let Some(tab) = tab_info(cached_tab.tab_id)? else {
                return Ok(false);
            };
            let mut live_panes = Vec::new();
            if let Some(cached_panes) = current.panes.panes.get(&cached_tab.position) {
                for cached_pane in cached_panes.iter().filter(|pane| !pane.is_plugin) {
                    let Some(mut pane) = pane_info(PaneId::Terminal(cached_pane.id))? else {
                        return Ok(false);
                    };
                    // Stock GetPaneInfo has no client context and always returns
                    // is_focused=false. It is not a negative layer-focus fact.
                    // Keep the client-scoped event flag while validating identity,
                    // membership and live geometry; otherwise every timer erases
                    // focus and the next PaneUpdate restores it, rewriting state.
                    pane.is_focused = cached_pane.is_focused;
                    live_panes.push(pane);
                }
            }
            panes.panes.insert(tab.position, live_panes);
            tabs.push(tab);
        }
        tabs.sort_by_key(|tab| tab.position);
        current.tabs = tabs;
        current.panes = panes;
        Ok(true)
    }

    #[cfg(any(target_arch = "wasm32", test))]
    fn collect_inventory(
        &mut self,
        now: Instant,
        mut session_list: impl FnMut() -> Result<SessionListSnapshot, String>,
        tab_info: impl FnMut(usize) -> Result<Option<TabInfo>, String>,
        pane_info: impl FnMut(PaneId) -> Result<Option<PaneInfo>, String>,
        mut focused_pane: impl FnMut() -> Result<(usize, PaneId), String>,
    ) -> Result<Option<(SessionInfo, Option<(usize, PaneId)>)>, String> {
        let current = if self.session_scan_due(now) {
            let current = session_list()?.live_sessions.into_iter().find(|s| s.is_current_session);
            if current.is_some() {
                self.next_session_scan = Some(now + SESSION_SCAN_INTERVAL);
            }
            current
        } else {
            self.cached_session_snapshot()
        };
        let Some(mut current) = current else { return Ok(None); };
        if let Some(clients) = self.observed_clients {
            current.connected_clients = clients;
        }
        if !Self::refresh_polled_topology(&mut current, tab_info, pane_info)? {
            // Never refresh authority for a closed entity from stale metadata.
            // An actual cache mismatch may repair at the next bounded tick;
            // unchanged topology must not repeatedly broadcast global scans.
            self.cleanup_renamed_session_file(&current.name);
            self.last_inventory = None;
            self.next_session_scan = Some(now);
            return Ok(None);
        }
        let focus = (current.connected_clients > 0).then(|| focused_pane().ok()).flatten();
        Ok(Some((current, focus)))
    }

    /// Native events discover new topology; direct queries validate existing facts.
    /// The slow session scan also recovers missed events and detached topology.
    pub fn resolve_session_from_snapshot(&mut self) {
        #[cfg(target_arch = "wasm32")]
        if let Ok(Some((current, focus))) = self.collect_inventory(
            Instant::now(), queries::session_list, queries::tab_info,
            queries::live_pane_info, queries::focused_pane,
        ) {
            CONTROL.with(|control| {
                if let Some(controller) = control.borrow_mut().as_mut() {
                    controller.set_session(&current.name);
                }
            });
            self.apply_polled_snapshot(current, focus);
        }
    }

    /// Serializes session tabs and full inventory to `/tmp/verij/states/`.
    ///
    /// Implements publication isolation:
    /// - Unique per-exporter filename prevents publication races between attached clients.
    /// - Unique temp paths with rename-only (no direct torn-write fallback) ensures atomic publication.
    /// - Safely encoded session names prevent filesystem path injection.
    /// - Legacy reader compatibility file is maintained atomically.
    pub fn export_state(&mut self) {
        let Some(session_name) = self.session_name.clone() else {
            return;
        };

        let dir = std::path::PathBuf::from(&self.export_dir);
        if let Err(e) = std::fs::create_dir_all(&dir) {
            eprintln!(
                "[verij-plugin] failed to create states dir {}: {}",
                dir.display(),
                e
            );
            return;
        }

        self.ensure_producer();
        let Some(producer) = self.producer.clone() else {
            return;
        };

        self.revision += 1;

        let now_ms = match std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH) {
            Ok(d) => d.as_millis() as u64,
            Err(_) => 0,
        };

        let inventory = self.build_inventory(now_ms, self.revision, &producer);

        let snapshot = SessionSnapshot {
            name: session_name.clone(),
            is_current: true,
            tabs: self.tabs.clone(),
            active_pane: self.active_pane.clone(),
            connected_clients: self.connected_clients,
            needs_resurrection: false,
            inventory: Some(inventory.clone()),
        };

        self.last_inventory = Some(inventory);

        let json = match serde_json::to_string_pretty(&snapshot) {
            Ok(j) => j,
            Err(e) => {
                eprintln!("[verij-plugin] failed to serialize snapshot: {}", e);
                return;
            }
        };

        // 1. Unique per-exporter file
        // Prevents per-client publication races with unique temp paths and rename only (no direct torn-write fallback)
        let exporter_filename = per_exporter_filename(
            &session_name,
            producer.server_pid,
            producer.plugin_id,
            producer.client_id,
        );
        let exporter_target = dir.join(&exporter_filename);
        let temp_exporter_filename = temp_filename(
            &session_name,
            producer.server_pid,
            producer.plugin_id,
            producer.client_id,
            self.revision,
        );
        let temp_exporter_path = dir.join(&temp_exporter_filename);

        Self::atomic_write_rename(&temp_exporter_path, &exporter_target, &json);

        // 2. Legacy reader compatibility file
        // Preserving legacy readers while maintaining unique per-exporter new file naming
        let legacy_target = dir.join(legacy_filename(&session_name));
        let temp_legacy_filename = format!("{}.legacy", temp_exporter_filename);
        let temp_legacy_path = dir.join(&temp_legacy_filename);

        Self::atomic_write_rename(&temp_legacy_path, &legacy_target, &json);
    }

    pub fn atomic_write_rename(tmp_path: &Path, target_path: &Path, content: &str) {
        if let Err(e) = std::fs::write(tmp_path, content) {
            eprintln!(
                "[verij-plugin] failed to write tmp state file {}: {}",
                tmp_path.display(),
                e
            );
            return;
        }
        if let Err(e) = std::fs::rename(tmp_path, target_path) {
            eprintln!(
                "[verij-plugin] failed to rename {} to {}: {}",
                tmp_path.display(),
                target_path.display(),
                e
            );
            let _ = std::fs::remove_file(tmp_path);
        }
    }

    /// Safely cleans up the previous state files when a session is renamed,
    /// ONLY if the file was written by the current producer.
    pub fn cleanup_renamed_session_file(&self, old_session_name: &str) {
        let Some(ref producer) = self.producer else {
            return;
        };
        let dir = Path::new(&self.export_dir);

        // 1. Old per-exporter file
        let old_exporter_filename = per_exporter_filename(
            old_session_name,
            producer.server_pid,
            producer.plugin_id,
            producer.client_id,
        );
        let old_exporter_path = dir.join(old_exporter_filename);
        Self::try_remove_if_owned(&old_exporter_path, producer);

        // 2. Old legacy file
        let old_legacy_filename = legacy_filename(old_session_name);
        let old_legacy_path = dir.join(old_legacy_filename);
        Self::try_remove_if_owned(&old_legacy_path, producer);
    }

    pub fn try_remove_if_owned(path: &Path, expected_producer: &PluginContext) {
        if !path.exists() {
            return;
        }
        if let Ok(content) = std::fs::read_to_string(path) {
            if let Ok(snapshot) = serde_json::from_str::<SessionSnapshot>(&content) {
                if let Some(ref inventory) = snapshot.inventory {
                    if &inventory.producer == expected_producer {
                        let _ = std::fs::remove_file(path);
                        eprintln!(
                            "[verij-plugin] removed old renamed state file owned by current producer: {}",
                            path.display()
                        );
                    }
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::cell::Cell;
    use std::collections::HashMap;

    #[test]
    fn inventory_validation_reuses_topology_without_rebroadcasting_session_scans() {
        let directory = std::env::temp_dir().join(format!("verij-scan-budget-{}", uuid_for_test()));
        let mut state = State::new();
        state.export_dir = directory.to_string_lossy().into_owned();
        let tab = TabInfo { tab_id: 7, position: 0, active: true, name: "one".into(), ..Default::default() };
        let pane = PaneInfo { id: 2, title: "terminal".into(), is_focused: true, ..Default::default() };
        let session = SessionInfo {
            name: "scan-budget".into(), is_current_session: true, connected_clients: 1,
            tabs: vec![tab.clone()], panes: PaneManifest { panes: HashMap::from([(0, vec![pane.clone()])]) },
            ..Default::default()
        };
        let scans = Cell::new(0);
        let validations = Cell::new(0);
        let now = Instant::now();
        for seconds in 0..=10 {
            let (current, focus) = state.collect_inventory(
                now + Duration::from_secs(seconds),
                || {
                    scans.set(scans.get() + 1);
                    Ok(SessionListSnapshot { live_sessions: vec![session.clone()], resurrectable_sessions: vec![] })
                },
                |_| { validations.set(validations.get() + 1); Ok(Some(tab.clone())) },
                |_| Ok(Some(pane.clone())),
                || Ok((7, PaneId::Terminal(2))),
            ).unwrap().unwrap();
            state.apply_polled_snapshot(current, focus);
            assert_eq!(scans.get(), if seconds < 10 { 1 } else { 2 });
        }
        assert_eq!(validations.get(), 11, "native validation retains one-second freshness");
        assert_eq!(state.revision, 1, "unchanged validation must not rewrite inventory");
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn native_topology_events_discover_new_panes_before_session_scan_recovery() {
        let directory = std::env::temp_dir().join(format!("verij-native-events-{}", uuid_for_test()));
        let mut state = State::new();
        state.export_dir = directory.to_string_lossy().into_owned();
        state.session_name = Some("native-events".into());
        let now = Instant::now();
        state.next_session_scan = Some(now + SESSION_SCAN_INTERVAL);
        let tab = TabInfo { tab_id: 9, position: 0, active: true, name: "new".into(), ..Default::default() };
        let pane = PaneInfo { id: 6, title: "new terminal".into(), is_focused: true, ..Default::default() };
        state.update(Event::TabUpdate(vec![tab.clone()]));
        state.update(Event::PaneUpdate(PaneManifest { panes: HashMap::from([(0, vec![pane.clone()])]) }));
        state.observed_clients = Some(1);
        let (current, focus) = state.collect_inventory(
            now + Duration::from_secs(1),
            || panic!("native topology events must not force a global session scan"),
            |_| Ok(Some(tab.clone())), |_| Ok(Some(pane.clone())), || Ok((9, PaneId::Terminal(6))),
        ).unwrap().unwrap();
        assert_eq!(current.tabs[0].tab_id, 9);
        assert_eq!(current.panes.panes[&0][0].id, 6);
        state.apply_polled_snapshot(current, focus);
        assert_eq!(state.last_inventory.as_ref().unwrap().panes[0].terminal_id.0, 6);
        state.update(Event::PaneUpdate(PaneManifest::default()));
        assert!(state.last_inventory.as_ref().unwrap().panes.is_empty());
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn context_free_pane_validation_does_not_erase_client_scoped_layer_focus() {
        let cached = PaneInfo { id: 4, title: "old".into(), is_focused: true, ..Default::default() };
        let tab = TabInfo { tab_id: 3, position: 0, active: true, ..Default::default() };
        let mut current = SessionInfo {
            tabs: vec![tab.clone()],
            panes: PaneManifest { panes: HashMap::from([(0, vec![cached])]) },
            ..Default::default()
        };
        assert!(State::refresh_polled_topology(&mut current,
            |_| Ok(Some(tab.clone())),
            |_| Ok(Some(PaneInfo { id: 4, title: "fresh".into(), is_focused: false, ..Default::default() })),
        ).unwrap());
        assert!(current.panes.panes[&0][0].is_focused);
        assert_eq!(current.panes.panes[&0][0].title, "fresh");
    }

    #[test]
    fn inventory_timer_ignores_fast_focus_and_obsolete_callbacks() {
        let now = Instant::now();
        let mut state = State::new();
        assert!(!state.inventory_poll_due(now), "permissions must enable inventory polling");
        state.next_inventory_poll = Some(now + Duration::from_secs(1));
        for ms in [20, 200, 400, 800] {
            assert!(!state.inventory_poll_due(now + Duration::from_millis(ms)));
        }
        assert!(state.inventory_poll_due(now + Duration::from_secs(1)));
        assert!(!state.inventory_poll_due(now + Duration::from_secs(1)));
        assert!(!state.inventory_poll_due(now + Duration::from_millis(1200)));
        assert!(state.inventory_poll_due(now + Duration::from_secs(2)));
    }

    #[test]
    fn polled_inventory_projects_client_focus_and_removes_closed_panes() {
        let directory = std::env::temp_dir().join(format!("verij-poll-{}", uuid_for_test()));
        let mut state = State::new();
        state.export_dir = directory.to_string_lossy().into_owned();
        let mut current = SessionInfo {
            name: "polled".into(), connected_clients: 2,
            tabs: vec![
                TabInfo { tab_id: 7, position: 0, active: true, name: "first".into(), ..Default::default() },
                TabInfo { tab_id: 12, position: 1, name: "second".into(), ..Default::default() },
            ],
            panes: PaneManifest { panes: HashMap::from([
                (0, vec![PaneInfo { id: 1, title: "one".into(), is_focused: true, ..Default::default() }]),
                (1, vec![PaneInfo { id: 2, title: "two".into(), is_focused: true, ..Default::default() }]),
            ]) }, ..Default::default()
        };
        state.apply_polled_snapshot(current.clone(), Some((12, PaneId::Terminal(2))));
        assert!(!state.tabs[0].is_active);
        assert!(state.tabs[1].is_active);
        assert_eq!(state.active_pane.as_deref(), Some("two"));
        let revision = state.revision;
        state.apply_polled_snapshot(current.clone(), Some((12, PaneId::Terminal(2))));
        assert_eq!(state.revision, revision, "unchanged polls must not write inventory");
        current.panes.panes.remove(&1);
        current.tabs.remove(1);
        current.connected_clients = 0;
        state.apply_polled_snapshot(current, None);
        assert_eq!(state.last_inventory.as_ref().unwrap().panes.len(), 1);
        assert_eq!(state.connected_clients, Some(0));
        assert!(state.active_pane.is_none());
        std::fs::remove_dir_all(directory).unwrap();
    }

    fn uuid_for_test() -> u128 {
        std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()
    }

    #[test]
    fn cached_closed_tabs_and_panes_cannot_be_republished_by_polling() {
        let cached = SessionInfo {
            tabs: vec![TabInfo { tab_id: 7, position: 1, name: "old".into(), ..Default::default() }],
            panes: PaneManifest { panes: HashMap::from([
                (1, vec![PaneInfo { id: 4, title: "old pane".into(), ..Default::default() }]),
            ]) },
            ..Default::default()
        };
        let live_tab = TabInfo { tab_id: 7, position: 0, name: "live".into(), ..Default::default() };
        let live_pane = PaneInfo { id: 4, title: "live pane".into(), ..Default::default() };
        let mut current = cached.clone();
        assert!(!State::refresh_polled_topology(&mut current, |_| Ok(None), |_| panic!("closed tab must stop pane queries")).unwrap());
        let mut current = cached.clone();
        assert!(!State::refresh_polled_topology(&mut current, |_| Ok(Some(live_tab.clone())), |_| Ok(None)).unwrap());
        let mut current = cached.clone();
        assert!(State::refresh_polled_topology(&mut current, |_| Err("denied".into()), |_| Ok(None)).is_err());
        assert_eq!(current.tabs, cached.tabs);
        assert_eq!(current.panes, cached.panes);
        let mut current = cached;
        assert!(State::refresh_polled_topology(&mut current,
            |id| { assert_eq!(id, 7); Ok(Some(live_tab.clone())) },
            |id| { assert_eq!(id, PaneId::Terminal(4)); Ok(Some(live_pane.clone())) },
        ).unwrap());
        assert_eq!(current.tabs, vec![live_tab]);
        assert_eq!(current.panes.panes, HashMap::from([(0, vec![live_pane])]));
    }

    #[test]
    fn test_tab_update_retains_panes_and_recalculates_title() {
        let mut state = State::new();
        state.session_name = Some("test-session".to_string());

        let mut panes = HashMap::new();
        panes.insert(
            0,
            vec![PaneInfo {
                id: 1,
                is_plugin: false,
                is_focused: true,
                is_suppressed: false,
                title: "editor_pane".to_string(),
                ..Default::default()
            }],
        );
        panes.insert(
            1,
            vec![PaneInfo {
                id: 2,
                is_plugin: false,
                is_focused: true,
                is_suppressed: false,
                title: "shell_pane".to_string(),
                ..Default::default()
            }],
        );
        state.panes = PaneManifest { panes };

        let tabs = vec![
            TabInfo {
                position: 0,
                name: "first".to_string(),
                active: true,
                tab_id: 100,
                ..Default::default()
            },
            TabInfo {
                position: 1,
                name: "second".to_string(),
                active: false,
                tab_id: 101,
                ..Default::default()
            },
        ];
        state.update(Event::TabUpdate(tabs));

        assert_eq!(state.active_pane.as_deref(), Some("editor_pane"));
        assert_eq!(state.raw_tabs.len(), 2);
        assert_eq!(state.panes.panes.len(), 2);

        // Switch active tab to 1
        let tabs_switched = vec![
            TabInfo {
                position: 0,
                name: "first".to_string(),
                active: false,
                tab_id: 100,
                ..Default::default()
            },
            TabInfo {
                position: 1,
                name: "second".to_string(),
                active: true,
                tab_id: 101,
                ..Default::default()
            },
        ];
        state.update(Event::TabUpdate(tabs_switched));

        // Active pane recalculated to tab 1's focused pane, panes cache preserved
        assert_eq!(state.active_pane.as_deref(), Some("shell_pane"));
        assert_eq!(state.panes.panes.len(), 2);
    }

    #[test]
    fn test_background_pane_update_captured_in_inventory() {
        let mut state = State::new();
        state.session_name = Some("test-session".to_string());
        state.producer = Some(PluginContext {
            server_pid: 1234,
            plugin_id: 1,
            client_id: 1,
            epoch: "test-epoch".to_string(),
        });

        let tabs = vec![
            TabInfo {
                position: 0,
                name: "code".to_string(),
                active: true,
                tab_id: 1,
                ..Default::default()
            },
            TabInfo {
                position: 1,
                name: "server".to_string(),
                active: false,
                tab_id: 2,
                ..Default::default()
            },
        ];
        state.update_tabs_from_info(&tabs);

        let mut panes = HashMap::new();
        panes.insert(
            0,
            vec![PaneInfo {
                id: 10,
                is_plugin: false,
                is_focused: true,
                title: "editor".to_string(),
                ..Default::default()
            }],
        );
        panes.insert(
            1,
            vec![
                PaneInfo {
                    id: 20,
                    is_plugin: false,
                    is_focused: true,
                    title: "bg-daemon".to_string(),
                    ..Default::default()
                },
                PaneInfo {
                    id: 30,
                    is_plugin: true, // Should be excluded from terminal inventory
                    title: "status-bar".to_string(),
                    ..Default::default()
                },
            ],
        );
        state.update(Event::PaneUpdate(PaneManifest { panes }));

        let producer = state.producer.clone().unwrap();
        let inv = state.build_inventory(1000, 1, &producer);
        assert_eq!(inv.tabs.len(), 2);
        assert_eq!(inv.tabs[0].tab_id, 1);
        assert_eq!(inv.tabs[1].tab_id, 2);

        // Terminal panes only (status-bar plugin excluded)
        assert_eq!(inv.panes.len(), 2);
        assert_eq!(inv.panes[0].terminal_id.0, 10);
        assert_eq!(inv.panes[0].tab_id, Some(1));
        assert_eq!(inv.panes[1].terminal_id.0, 20);
        assert_eq!(inv.panes[1].tab_id, Some(2));
        assert_eq!(inv.panes[1].title, "bg-daemon");
    }

    #[test]
    fn session_list_bootstrap_cannot_resurrect_closed_pane_or_undo_direct_tab_update() {
        let directory = std::env::temp_dir().join(format!(
            "verij-event-order-{}-{}",
            std::process::id(),
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&directory).unwrap();
        let mut state = State::new();
        state.export_dir = directory.to_string_lossy().into_owned();
        let live = PaneInfo {
            id: 0,
            title: "live".into(),
            is_focused: true,
            ..Default::default()
        };
        let closed = PaneInfo {
            id: 4,
            title: "closed".into(),
            ..Default::default()
        };
        let old = SessionInfo {
            name: "fixture-events".into(),
            is_current_session: true,
            connected_clients: 2,
            tabs: vec![TabInfo {
                tab_id: 10,
                position: 0,
                name: "old".into(),
                active: true,
                ..Default::default()
            }],
            panes: PaneManifest {
                panes: HashMap::from([(0, vec![live.clone(), closed])]),
            },
            ..Default::default()
        };
        state.update(Event::SessionUpdate(vec![old.clone()], vec![]));
        state.update(Event::PaneUpdate(PaneManifest {
            panes: HashMap::from([(0, vec![live])]),
        }));
        state.update(Event::TabUpdate(vec![TabInfo {
            tab_id: 10,
            position: 0,
            name: "renamed".into(),
            active: true,
            ..Default::default()
        }]));
        let revision = state.revision;
        for _ in 0..3 {
            state.update(Event::SessionUpdate(vec![old.clone()], vec![]));
        }
        assert_eq!(
            state.revision, revision,
            "stale aggregate events must not republish alternating inventory"
        );
        assert_eq!(state.tabs[0].name, "renamed");
        assert_eq!(
            state.panes.panes[&0]
                .iter()
                .map(|pane| pane.id)
                .collect::<Vec<_>>(),
            vec![0]
        );
        std::fs::remove_dir_all(directory).unwrap();
    }

    #[test]
    fn test_rename_cleanup_verifies_ownership() {
        let temp_dir = std::env::temp_dir().join(format!(
            "verij_plugin_test_{}",
            std::time::SystemTime::now()
                .duration_since(std::time::UNIX_EPOCH)
                .unwrap()
                .as_nanos()
        ));
        std::fs::create_dir_all(&temp_dir).unwrap();
        let producer_me = PluginContext {
            server_pid: 999,
            plugin_id: 1,
            client_id: 1,
            epoch: "epoch-me".to_string(),
        };
        let producer_other = PluginContext {
            server_pid: 888,
            plugin_id: 1,
            client_id: 1,
            epoch: "epoch-other".to_string(),
        };

        let my_file = temp_dir.join("my_file.json");
        let other_file = temp_dir.join("other_file.json");

        let my_snapshot = SessionSnapshot {
            name: "old-session".to_string(),
            is_current: true,
            tabs: vec![],
            active_pane: None,
            connected_clients: None,
            needs_resurrection: false,
            inventory: Some(SessionInventory {
                schema_version: 1,
                producer: producer_me.clone(),
                revision: 1,
                exported_at_ms: 100,
                session_instance_id: None,
                server_process: None,
                capabilities: vec![],
                tabs: vec![],
                panes: vec![],
            }),
        };
        std::fs::write(&my_file, serde_json::to_string(&my_snapshot).unwrap()).unwrap();

        let other_snapshot = SessionSnapshot {
            name: "other-session".to_string(),
            is_current: true,
            tabs: vec![],
            active_pane: None,
            connected_clients: None,
            needs_resurrection: false,
            inventory: Some(SessionInventory {
                schema_version: 1,
                producer: producer_other,
                revision: 1,
                exported_at_ms: 100,
                session_instance_id: None,
                server_process: None,
                capabilities: vec![],
                tabs: vec![],
                panes: vec![],
            }),
        };
        std::fs::write(&other_file, serde_json::to_string(&other_snapshot).unwrap()).unwrap();

        // Trying to remove other_file with producer_me must not remove it
        State::try_remove_if_owned(&other_file, &producer_me);
        assert!(other_file.exists());

        // Trying to remove my_file with producer_me must remove it
        State::try_remove_if_owned(&my_file, &producer_me);
        assert!(!my_file.exists());

        let _ = std::fs::remove_dir_all(&temp_dir);
    }
}
