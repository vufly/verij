//! Public stock topology facts. Unsupported stack/visibility facts stay unknown.
use crate::identity::{PluginContext, ProcessIdentity, SessionInstanceId, TerminalPaneId};
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SessionInventory {
    pub schema_version: u32,
    pub producer: PluginContext,
    pub revision: u64,
    pub exported_at_ms: u64,
    /// Filled by native readers only after validating the server's OS birth.
    #[serde(default)]
    pub session_instance_id: Option<SessionInstanceId>,
    #[serde(default)]
    pub server_process: Option<ProcessIdentity>,
    #[serde(default)]
    pub capabilities: Vec<String>,
    #[serde(default)]
    pub tabs: Vec<TabMetadata>,
    #[serde(default)]
    pub panes: Vec<PaneSnapshot>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct TabMetadata {
    pub tab_id: usize,
    pub position: usize,
    pub floating_visible: bool,
    pub fullscreen_active: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PaneSnapshot {
    pub terminal_id: TerminalPaneId,
    pub tab_id: Option<usize>,
    pub tab_position: usize,
    pub title: String,
    pub is_floating: bool,
    pub is_suppressed: bool,
    pub is_fullscreen: bool,
    /// Layer-local remembered focus is not effective client focus.
    pub layer_focused: bool,
    pub exited: bool,
    #[serde(default)]
    pub pane_pid: Option<u32>,
    #[serde(default)]
    pub pane_process: Option<ProcessIdentity>,
    #[serde(default)]
    pub stack_id: Option<String>,
}

/// Safely encodes a session name for use in filenames on disk.
/// Characters outside `[a-zA-Z0-9_-]` are percent-encoded (`%{:02X}`).
pub fn safe_encode_name(name: &str) -> String {
    let mut encoded = String::with_capacity(name.len());
    for b in name.bytes() {
        if b.is_ascii_alphanumeric() || b == b'-' || b == b'_' {
            encoded.push(b as char);
        } else {
            use std::fmt::Write;
            let _ = write!(encoded, "%{:02X}", b);
        }
    }
    if encoded.is_empty() {
        "unnamed".to_string()
    } else {
        encoded
    }
}

/// Generates a unique per-exporter inventory filename.
pub fn per_exporter_filename(
    session_name: &str,
    server_pid: u32,
    plugin_id: u32,
    client_id: u16,
) -> String {
    format!(
        "{}.{}_{}_{}.json",
        safe_encode_name(session_name),
        server_pid,
        plugin_id,
        client_id
    )
}

/// Generates the legacy session state filename for backward compatibility.
pub fn legacy_filename(session_name: &str) -> String {
    format!("{}.json", safe_encode_name(session_name))
}

/// Generates a unique temporary filename for atomic writes (write + rename).
pub fn temp_filename(
    session_name: &str,
    server_pid: u32,
    plugin_id: u32,
    client_id: u16,
    revision: u64,
) -> String {
    format!(
        ".tmp.{}.{}_{}_{}_{}.json",
        safe_encode_name(session_name),
        server_pid,
        plugin_id,
        client_id,
        revision
    )
}

/// Capability identifiers advertised by the plugin inventory.
pub const CAP_FULL_INVENTORY: &str = "full_inventory";
pub const CAP_TERMINAL_PANES: &str = "terminal_panes";
pub const CAP_PANE_PID: &str = "pane_pid";
pub const CAP_PANE_FOCUS: &str = "layer_focus_flags";

pub fn default_capabilities() -> Vec<String> {
    vec![
        CAP_FULL_INVENTORY.to_string(),
        CAP_TERMINAL_PANES.to_string(),
        CAP_PANE_PID.to_string(),
        CAP_PANE_FOCUS.to_string(),
    ]
}
