//! `verij-types`
//!
//! Shared data structures and constants between the WASM plugin (`verij-plugin`)
//! and the native CLI/TUI (`verij-cli`).
//!
//! Designed to have zero native OS or WASM dependencies, relying solely on
//! `serde` for serialization.

use serde::{Deserialize, Serialize};

/// The Zellij named pipe used to stream session snapshots from `verij-plugin`
/// to `verij-cli` (legacy, deprecated by filesystem watcher).
pub const VERIJ_EVENTS_PIPE: &str = "verij_events";

/// The Zellij named pipe used to inject control commands to `verij-plugin`.
pub const VERIJ_CONTROL_PIPE: &str = "verij_control";

/// Directory where distributed agents export their session JSON states.
pub const VERIJ_STATES_DIR: &str = "/tmp/verij/states";

/// Prefix for Verij host wrapper sessions. Sessions with this prefix must be
/// filtered out of workspace management and navigation.
pub const HOST_SESSION_PREFIX: &str = "__verij_host_";


/// A compact, serializable snapshot of a single Zellij session.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionSnapshot {
    /// Session name (globally unique within a Zellij server).
    pub name: String,
    /// True if this is the session the plugin is currently running inside.
    pub is_current: bool,
    /// Ordered list of tabs in this session.
    pub tabs: Vec<TabSnapshot>,
}

impl SessionSnapshot {
    /// Returns the currently active tab in this session, if any.
    pub fn active_tab(&self) -> Option<&TabSnapshot> {
        self.tabs.iter().find(|t| t.is_active)
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
        }];

        let json = serde_json::to_string(&snapshot).expect("serialize");
        let deserialized: Vec<SessionSnapshot> = serde_json::from_str(&json).expect("deserialize");

        assert_eq!(snapshot, deserialized);
        assert_eq!(
            snapshot[0].active_tab().map(|t| t.name.as_str()),
            Some("editor")
        );
    }
}
