//! verij-cli/src/tui/tree_model.rs
//!
//! Three-level hierarchical tree model for sessions, tabs, and agent panes.

use serde::{Deserialize, Serialize};
use std::collections::HashSet;
use verij_types::identity::{AgentInstanceId, PaneKey, SessionInstanceId};

use crate::tui::agent_view::{AgentSummary, AgentView};

/// Stable or fallback typed identifier for a session.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum SessionKey {
    Verified(SessionInstanceId),
    Legacy(String),
}

/// Stable or fallback typed identifier for a tab.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum TabKey {
    Stable(usize),
    Position(usize),
}

/// Globally unique typed key for any row in the tree.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub enum NodeKey {
    Session(SessionKey),
    Tab {
        session: SessionKey,
        tab: TabKey,
    },
    Agent {
        pane: PaneKey,
        instance: AgentInstanceId,
    },
}

/// Structural metadata for a single row in the tree.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord, Serialize, Deserialize)]
pub struct RowMeta {
    pub key: NodeKey,
    pub parent: Option<NodeKey>,
    pub depth: usize,
    pub sibling_index: usize,
    pub sibling_count: usize,
    pub ancestor_continuations: Vec<bool>,
}

impl Default for RowMeta {
    fn default() -> Self {
        Self {
            key: NodeKey::Session(SessionKey::Legacy(String::new())),
            parent: None,
            depth: 0,
            sibling_index: 0,
            sibling_count: 0,
            ancestor_continuations: Vec::new(),
        }
    }
}

/// A single navigable item in the flattened session/tab/agent tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TreeNode {
    /// A top-level session row.
    Session {
        name: String,
        is_current: bool,
        is_attached: bool,
        needs_resurrection: bool,
        is_collapsed: bool,
        active_tab: Option<String>,
        tab_count: usize,
        session_index: usize,
        meta: RowMeta,
        summary: AgentSummary,
    },
    /// A tab row nested under an expanded session.
    Tab {
        session_name: String,
        session_index: usize,
        name: String,
        position: usize,
        is_workspace_active: bool,
        meta: RowMeta,
        summary: AgentSummary,
        is_collapsed: bool,
        has_children: bool,
    },
    /// An agent pane leaf row nested under an expanded tab.
    AgentPane {
        session_name: String,
        session_index: usize,
        tab_name: String,
        tab_position: usize,
        view: AgentView,
        meta: RowMeta,
    },
}

impl TreeNode {
    /// The session name that this node belongs to (or is).
    pub fn session_name(&self) -> &str {
        match self {
            TreeNode::Session { name, .. } => name,
            TreeNode::Tab { session_name, .. } => session_name,
            TreeNode::AgentPane { session_name, .. } => session_name,
        }
    }

    /// Tab position if this is a Tab or AgentPane node, otherwise None.
    pub fn tab_position(&self) -> Option<usize> {
        match self {
            TreeNode::Session { .. } => None,
            TreeNode::Tab { position, .. } => Some(*position),
            TreeNode::AgentPane { tab_position, .. } => Some(*tab_position),
        }
    }

    /// Row metadata for this node.
    pub fn meta(&self) -> &RowMeta {
        match self {
            TreeNode::Session { meta, .. } => meta,
            TreeNode::Tab { meta, .. } => meta,
            TreeNode::AgentPane { meta, .. } => meta,
        }
    }

    /// Agent summary for this node (leaf defaults to empty).
    pub fn summary(&self) -> AgentSummary {
        match self {
            TreeNode::Session { summary, .. } => summary.clone(),
            TreeNode::Tab { summary, .. } => summary.clone(),
            TreeNode::AgentPane { .. } => AgentSummary::default(),
        }
    }

    /// Node key accessible via meta.key.
    pub fn key(&self) -> &NodeKey {
        &self.meta().key
    }

    /// True if this node is the active tab of the currently attached workspace session.
    #[cfg(test)]
    pub fn is_workspace_active_tab(&self) -> bool {
        match self {
            TreeNode::Tab {
                is_workspace_active,
                ..
            } => *is_workspace_active,
            _ => false,
        }
    }
}

/// DTO for persisting tree state across sessions or runs.
#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct TreePersistableState {
    pub collapsed_keys: HashSet<NodeKey>,
    pub collapsed_tabs: HashSet<NodeKey>,
    pub selected_key: Option<NodeKey>,
}
