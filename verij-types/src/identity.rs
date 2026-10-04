//! Process-qualified identities shared by native monitoring and stock control.
//! Names, titles and exporter epochs are metadata, not server lifetimes.
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct ProcessIdentity {
    pub pid: u32,
    pub start_jiffies: u64,
    pub boot_id: String,
    pub uid: u32,
}

macro_rules! string_identity {
    ($name:ident) => {
        #[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
        #[serde(transparent)]
        pub struct $name(pub String);
    };
}

string_identity!(SessionInstanceId);
string_identity!(AgentInstanceId);
string_identity!(HostKey);
string_identity!(ConversationId);
string_identity!(TurnId);

impl SessionInstanceId {
    pub fn from_process(process: &ProcessIdentity) -> Self {
        Self(format!(
            "linux:{}:{}:{}:{}",
            process.uid, process.boot_id, process.pid, process.start_jiffies
        ))
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(transparent)]
pub struct TerminalPaneId(pub u32);

#[derive(Debug, Clone, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
pub struct PaneKey {
    pub session: SessionInstanceId,
    pub terminal: TerminalPaneId,
}

/// Exact public plugin context. Epoch belongs to Verij, not stock IPC.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PluginContext {
    pub server_pid: u32,
    pub plugin_id: u32,
    pub client_id: u16,
    pub epoch: String,
}

/// These identifiers are used as path components; session identities are not.
pub fn valid_record_key(value: &str) -> bool {
    !value.is_empty()
        && value.len() <= 128
        && value
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || matches!(byte, b'-' | b'_'))
}
