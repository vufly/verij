//! `verij-types`
//!
//! Shared data structures and constants between the WASM plugin (`verij-plugin`)
//! and the native CLI/TUI (`verij-cli`).
//!
//! Designed to have zero native OS or WASM dependencies, relying solely on
//! `serde` for serialization.

use serde::{Deserialize, Serialize};

pub mod identity;
pub mod inventory;
pub mod control;
pub mod agent;

/// The Zellij named pipe used to stream session snapshots from `verij-plugin`
/// to `verij-cli` (legacy, deprecated by filesystem watcher).
pub const VERIJ_EVENTS_PIPE: &str = "verij_events";

/// The Zellij named pipe used to inject control commands to `verij-plugin`.
pub const VERIJ_CONTROL_PIPE: &str = "verij_control";

/// Directory where distributed agents export their session JSON states.
pub const VERIJ_STATES_DIR: &str = "/tmp/verij/states";

/// A compact, serializable snapshot of a single Zellij session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionSnapshot {
    /// Session name (globally unique within a Zellij server).
    pub name: String,
    /// True if this is the session the plugin is currently running inside.
    pub is_current: bool,
    /// Ordered list of tabs in this session.
    pub tabs: Vec<TabSnapshot>,
    /// Title of the focused pane in the active tab, if available.
    /// `None` if the exporting plugin predates this field or no pane is focused.
    #[serde(default)]
    pub active_pane: Option<String>,
    /// Number of clients currently attached to this session.
    /// `None` if the exporting plugin predates this field.
    /// `Some(0)` means all clients have detached.
    #[serde(default)]
    pub connected_clients: Option<usize>,
    /// True when Zellij reports this session as exited and it must be resurrected
    /// before it can be attached. This is populated by the CLI, not the plugin.
    #[serde(default, skip_serializing)]
    pub needs_resurrection: bool,
    /// Full terminal topology, absent in legacy snapshots.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub inventory: Option<inventory::SessionInventory>,
}

impl SessionSnapshot {
    /// Returns the currently active tab in this session, if any.
    pub fn active_tab(&self) -> Option<&TabSnapshot> {
        self.tabs.iter().find(|t| t.is_active)
    }

    /// Returns the title of the focused pane in the active tab, if available.
    pub fn active_pane(&self) -> Option<&str> {
        self.active_pane.as_deref()
    }
}

/// A compact, serializable snapshot of a single tab within a session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TabSnapshot {
    /// Display name of the tab.
    pub name: String,
    /// Zero-based position index (used for `go-to-tab` actions).
    pub position: usize,
    /// Whether this tab is currently focused in its session.
    pub is_active: bool,
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn test_serde_roundtrip() {
        let snapshot = vec![SessionSnapshot {
            name: "test-session".to_string(),
            is_current: true,
            tabs: vec![
                TabSnapshot {
                    name: "editor".to_string(),
                    position: 0,
                    is_active: true,
                },
                TabSnapshot {
                    name: "server".to_string(),
                    position: 1,
                    is_active: false,
                },
            ],
            active_pane: None,
            connected_clients: None,
            needs_resurrection: false,
            inventory: None,
        }];

        let json = serde_json::to_string(&snapshot).expect("serialize");
        let deserialized: Vec<SessionSnapshot> = serde_json::from_str(&json).expect("deserialize");

        assert_eq!(snapshot, deserialized);
        assert_eq!(
            snapshot[0].active_tab().map(|t| t.name.as_str()),
            Some("editor")
        );
    }

    #[test]
    fn test_legacy_snapshot_deserialization_without_inventory() {
        let json = r#"{
            "name": "legacy-session",
            "is_current": false,
            "tabs": [
                {"name": "tab1", "position": 0, "is_active": true}
            ],
            "active_pane": "shell",
            "connected_clients": 1
        }"#;

        let snapshot: SessionSnapshot = serde_json::from_str(json).expect("deserialize legacy");
        assert_eq!(snapshot.name, "legacy-session");
        assert!(!snapshot.is_current);
        assert_eq!(snapshot.tabs.len(), 1);
        assert_eq!(snapshot.active_pane.as_deref(), Some("shell"));
        assert_eq!(snapshot.connected_clients, Some(1));
        assert!(!snapshot.needs_resurrection);
        assert_eq!(snapshot.inventory, None);
    }

    #[test]
    fn test_snapshot_with_full_inventory_roundtrip() {
        use crate::identity::{PluginContext, TerminalPaneId};
        use crate::inventory::{PaneSnapshot, SessionInventory, TabMetadata};

        let producer = PluginContext {
            server_pid: 4242,
            plugin_id: 1,
            client_id: 2,
            epoch: "1790954114000".to_string(),
        };

        let inventory = SessionInventory {
            schema_version: 1,
            producer: producer.clone(),
            revision: 7,
            exported_at_ms: 1790954120000,
            session_instance_id: None,
            server_process: None,
            capabilities: crate::inventory::default_capabilities(),
            tabs: vec![
                TabMetadata {
                    tab_id: 10,
                    position: 0,
                    floating_visible: false,
                    fullscreen_active: false,
                },
                TabMetadata {
                    tab_id: 11,
                    position: 1,
                    floating_visible: true,
                    fullscreen_active: false,
                },
            ],
            panes: vec![
                PaneSnapshot {
                    terminal_id: TerminalPaneId(1),
                    tab_id: Some(10),
                    tab_position: 0,
                    title: "editor".to_string(),
                    is_floating: false,
                    is_suppressed: false,
                    is_fullscreen: false,
                    layer_focused: true,
                    exited: false,
                    pane_pid: Some(5001),
                    pane_process: None,
                    stack_id: None,
                },
                PaneSnapshot {
                    terminal_id: TerminalPaneId(2),
                    tab_id: Some(11),
                    tab_position: 1,
                    title: "worker".to_string(),
                    is_floating: true,
                    is_suppressed: false,
                    is_fullscreen: false,
                    layer_focused: false,
                    exited: false,
                    pane_pid: Some(5002),
                    pane_process: None,
                    stack_id: None,
                },
            ],
        };

        let snapshot = SessionSnapshot {
            name: "inventory-session".to_string(),
            is_current: true,
            tabs: vec![
                TabSnapshot {
                    name: "main".to_string(),
                    position: 0,
                    is_active: true,
                },
                TabSnapshot {
                    name: "bg".to_string(),
                    position: 1,
                    is_active: false,
                },
            ],
            active_pane: Some("editor".to_string()),
            connected_clients: Some(1),
            needs_resurrection: false,
            inventory: Some(inventory),
        };

        let json = serde_json::to_string(&snapshot).expect("serialize");
        let deserialized: SessionSnapshot = serde_json::from_str(&json).expect("deserialize");

        assert_eq!(snapshot, deserialized);
        let inv = deserialized.inventory.expect("inventory present");
        assert_eq!(inv.schema_version, 1);
        assert_eq!(inv.producer, producer);
        assert_eq!(inv.revision, 7);
        assert_eq!(inv.tabs.len(), 2);
        assert_eq!(inv.panes.len(), 2);
        assert_eq!(inv.panes[0].title, "editor");
        assert_eq!(inv.panes[1].title, "worker");
    }

    #[test]
    fn test_background_pane_changes_in_inventory() {
        use crate::identity::{PluginContext, TerminalPaneId};
        use crate::inventory::{PaneSnapshot, SessionInventory, TabMetadata};

        let producer = PluginContext {
            server_pid: 1234,
            plugin_id: 1,
            client_id: 1,
            epoch: "1790954000".to_string(),
        };

        // Active tab is tab 0 ("editor"). Background tab is tab 1 ("server").
        let mut panes = vec![
            PaneSnapshot {
                terminal_id: TerminalPaneId(1),
                tab_id: Some(10),
                tab_position: 0,
                title: "editor".to_string(),
                is_floating: false,
                is_suppressed: false,
                is_fullscreen: false,
                layer_focused: true,
                exited: false,
                pane_pid: Some(101),
                pane_process: None,
                stack_id: None,
            },
            PaneSnapshot {
                terminal_id: TerminalPaneId(2),
                tab_id: Some(11),
                tab_position: 1,
                title: "server: running".to_string(),
                is_floating: false,
                is_suppressed: false,
                is_fullscreen: false,
                layer_focused: true,
                exited: false,
                pane_pid: Some(102),
                pane_process: None,
                stack_id: None,
            },
        ];

        let snapshot1 = SessionSnapshot {
            name: "test".to_string(),
            is_current: true,
            tabs: vec![
                TabSnapshot { name: "edit".into(), position: 0, is_active: true },
                TabSnapshot { name: "srv".into(), position: 1, is_active: false },
            ],
            active_pane: Some("editor".to_string()),
            connected_clients: Some(1),
            needs_resurrection: false,
            inventory: Some(SessionInventory {
                schema_version: 1,
                producer: producer.clone(),
                revision: 1,
                exported_at_ms: 100,
                session_instance_id: None,
                server_process: None,
                capabilities: crate::inventory::default_capabilities(),
                tabs: vec![
                    TabMetadata { tab_id: 10, position: 0, floating_visible: false, fullscreen_active: false },
                    TabMetadata { tab_id: 11, position: 1, floating_visible: false, fullscreen_active: false },
                ],
                panes: panes.clone(),
            }),
        };

        // Now simulate a background pane update: pane 2 in background tab changes title and exits.
        panes[1].title = "server: stopped".to_string();
        panes[1].exited = true;

        let snapshot2 = SessionSnapshot {
            name: "test".to_string(),
            is_current: true,
            tabs: snapshot1.tabs.clone(),
            // Active pane title remains "editor" because the active tab's pane didn't change!
            active_pane: Some("editor".to_string()),
            connected_clients: Some(1),
            needs_resurrection: false,
            inventory: Some(SessionInventory {
                schema_version: 1,
                producer: producer.clone(),
                revision: 2,
                exported_at_ms: 200,
                session_instance_id: None,
                server_process: None,
                capabilities: crate::inventory::default_capabilities(),
                tabs: snapshot1.inventory.as_ref().unwrap().tabs.clone(),
                panes: panes.clone(),
            }),
        };

        // Active pane is unchanged
        assert_eq!(snapshot1.active_pane, snapshot2.active_pane);
        // But inventory has detected the background pane change and updated revision
        assert_ne!(snapshot1.inventory, snapshot2.inventory);
        let inv2 = snapshot2.inventory.unwrap();
        assert_eq!(inv2.revision, 2);
        assert_eq!(inv2.panes[1].title, "server: stopped");
        assert!(inv2.panes[1].exited);
    }

    #[test]
    fn test_tab_event_preserving_panes() {
        use crate::identity::{PluginContext, TerminalPaneId};
        use crate::inventory::{PaneSnapshot, SessionInventory, TabMetadata};

        let producer = PluginContext {
            server_pid: 1234,
            plugin_id: 1,
            client_id: 1,
            epoch: "1790954000".to_string(),
        };

        let cached_panes = vec![
            PaneSnapshot {
                terminal_id: TerminalPaneId(1),
                tab_id: Some(10),
                tab_position: 0,
                title: "editor_pane".to_string(),
                is_floating: false,
                is_suppressed: false,
                is_fullscreen: false,
                layer_focused: true,
                exited: false,
                pane_pid: Some(101),
                pane_process: None,
                stack_id: None,
            },
            PaneSnapshot {
                terminal_id: TerminalPaneId(2),
                tab_id: Some(11),
                tab_position: 1,
                title: "terminal_pane".to_string(),
                is_floating: false,
                is_suppressed: false,
                is_fullscreen: false,
                layer_focused: true,
                exited: false,
                pane_pid: Some(102),
                pane_process: None,
                stack_id: None,
            },
        ];

        // Initially Tab 0 is active
        let active_pane_0 = cached_panes.iter().find(|p| p.tab_position == 0 && p.layer_focused).map(|p| p.title.clone());
        assert_eq!(active_pane_0.as_deref(), Some("editor_pane"));

        // Tab event occurs: user switches to Tab 1. Panes are NOT cleared!
        let tabs_switched = vec![
            TabSnapshot { name: "tab0".into(), position: 0, is_active: false },
            TabSnapshot { name: "tab1".into(), position: 1, is_active: true },
        ];
        // Active pane recalculated to tab 1's focused pane using retained pane cache
        let active_pane_1 = cached_panes.iter().find(|p| p.tab_position == 1 && p.layer_focused).map(|p| p.title.clone());
        assert_eq!(active_pane_1.as_deref(), Some("terminal_pane"));

        let snapshot = SessionSnapshot {
            name: "test".to_string(),
            is_current: true,
            tabs: tabs_switched,
            active_pane: active_pane_1,
            connected_clients: Some(1),
            needs_resurrection: false,
            inventory: Some(SessionInventory {
                schema_version: 1,
                producer,
                revision: 2,
                exported_at_ms: 300,
                session_instance_id: None,
                server_process: None,
                capabilities: crate::inventory::default_capabilities(),
                tabs: vec![
                    TabMetadata { tab_id: 10, position: 0, floating_visible: false, fullscreen_active: false },
                    TabMetadata { tab_id: 11, position: 1, floating_visible: false, fullscreen_active: false },
                ],
                panes: cached_panes.clone(),
            }),
        };

        // All panes preserved
        assert_eq!(snapshot.inventory.as_ref().unwrap().panes.len(), 2);
        assert_eq!(snapshot.active_pane.as_deref(), Some("terminal_pane"));
    }

    #[test]
    fn test_rename_and_old_producer_ownership() {
        use crate::identity::PluginContext;
        use crate::inventory::{per_exporter_filename, safe_encode_name, SessionInventory};

        // 1. Safe name encoding
        assert_eq!(safe_encode_name("normal_name-123"), "normal_name-123");
        assert_eq!(safe_encode_name("with spaces"), "with%20spaces");
        assert_eq!(safe_encode_name("foo/bar:baz"), "foo%2Fbar%3Abaz");
        assert_eq!(safe_encode_name(""), "unnamed");

        // 2. Per-exporter unique filenames prevent client races
        let file_client1 = per_exporter_filename("my-session", 1000, 1, 1);
        let file_client2 = per_exporter_filename("my-session", 1000, 1, 2);
        assert_eq!(file_client1, "my-session.1000_1_1.json");
        assert_eq!(file_client2, "my-session.1000_1_2.json");
        assert_ne!(file_client1, file_client2);

        // 3. Producer ownership verification for session rename cleanup
        let producer_a = PluginContext {
            server_pid: 1000,
            plugin_id: 1,
            client_id: 1,
            epoch: "epoch-1".to_string(),
        };
        let producer_b = PluginContext {
            server_pid: 1000,
            plugin_id: 1,
            client_id: 2,
            epoch: "epoch-1".to_string(),
        };

        let old_snapshot = SessionSnapshot {
            name: "old-name".to_string(),
            is_current: true,
            tabs: vec![],
            active_pane: None,
            connected_clients: Some(1),
            needs_resurrection: false,
            inventory: Some(SessionInventory {
                schema_version: 1,
                producer: producer_a.clone(),
                revision: 1,
                exported_at_ms: 100,
                session_instance_id: None,
                server_process: None,
                capabilities: vec![],
                tabs: vec![],
                panes: vec![],
            }),
        };

        // Producer A owns old_snapshot -> allowed to clean up old file
        let is_owned_by_a = old_snapshot
            .inventory
            .as_ref()
            .map(|i| &i.producer == &producer_a)
            .unwrap_or(false);
        assert!(is_owned_by_a);

        // Producer B does NOT own old_snapshot -> must NOT delete it
        let is_owned_by_b = old_snapshot
            .inventory
            .as_ref()
            .map(|i| &i.producer == &producer_b)
            .unwrap_or(false);
        assert!(!is_owned_by_b);
    }
}
