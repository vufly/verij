//! Verij application protocol carried by stock public pipes.
use crate::identity::{HostKey, PaneKey, PluginContext, ProcessIdentity, TerminalPaneId};
use serde::{Deserialize, Serialize};

pub const SCHEMA_VERSION: u32 = 1;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ControlRequest {
    pub schema_version: u32,
    pub request_id: String,
    pub context: PluginContext,
    pub sequence: u64,
    pub operation: ControlOperation,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case")]
pub enum ControlOperation {
    Query,
    Locate {
        terminal: TerminalPaneId,
    },
    Focus {
        terminal: TerminalPaneId,
    },
    Switch {
        session_name: String,
        terminal: TerminalPaneId,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct FocusObservation {
    pub context: PluginContext,
    pub session_name: Option<String>,
    pub terminal: Option<TerminalPaneId>,
    #[serde(default)]
    pub focused_plugin: Option<u32>,
    #[serde(default)]
    pub queried_terminal: Option<TerminalPaneId>,
    #[serde(default)]
    pub pane_pid: Option<u32>,
    pub observed_at_ms: u64,
    /// Last request accepted by this Verij plugin instance, not a stock server
    /// execution watermark or evidence that a native action completed.
    #[serde(default)]
    pub application_sequence: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ControlStatus {
    Observed,
    InnerFocusObserved,
    SwitchDispatchedUnverified,
    PaneMissing,
    Superseded,
    StaleContext,
    Unavailable,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ControlResult {
    pub request: ControlRequest,
    pub status: ControlStatus,
    pub observation: Option<FocusObservation>,
    pub whole_host_verified: bool,
    pub acknowledge_done: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct Registration {
    pub nonce: String,
    pub context: PluginContext,
    pub observation: FocusObservation,
    pub source: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostBinding {
    pub schema_version: u32,
    pub host: HostKey,
    pub host_session: String,
    pub workspace_pane: TerminalPaneId,
    pub attachment_process: ProcessIdentity,
    pub server_process: ProcessIdentity,
    pub context: PluginContext,
    pub pane: Option<PaneKey>,
}
