/// verij-cli/src/tui/state.rs
///
/// Application state for the Ratatui TUI.
///
/// The state holds the current session snapshot, flattened navigable tree,
/// selection cursor, collapsed set, attached workspace session, active tab position,
/// and ListState.
use ratatui::widgets::ListState;
use std::collections::HashSet;
use verij_types::SessionSnapshot;

// ---------------------------------------------------------------------------
// Tree node model
// ---------------------------------------------------------------------------

/// A single navigable item in the flattened session/tab tree.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TreeNode {
    /// A top-level session row.
    Session {
        name: String,
        is_current: bool,
        is_attached: bool,
        is_collapsed: bool,
        active_tab: Option<String>,
        tab_count: usize,
        session_index: usize,
    },
    /// A tab row nested under an expanded session.
    Tab {
        session_name: String,
        session_index: usize,
        name: String,
        position: usize,
        is_workspace_active: bool,
    },
}

impl TreeNode {
    /// The session name that this node belongs to (or is).
    pub fn session_name(&self) -> &str {
        match self {
            TreeNode::Session { name, .. } => name,
            TreeNode::Tab { session_name, .. } => session_name,
        }
    }

    /// Tab position if this is a Tab node, otherwise None.
    pub fn tab_position(&self) -> Option<usize> {
        match self {
            TreeNode::Tab { position, .. } => Some(*position),
            TreeNode::Session { .. } => None,
        }
    }

    /// True if this node is the single active tab of the session attached in the Workspace pane.
    pub fn is_workspace_active_tab(&self) -> bool {
        match self {
            TreeNode::Tab {
                is_workspace_active,
                ..
            } => *is_workspace_active,
            TreeNode::Session { .. } => false,
        }
    }
}

// ---------------------------------------------------------------------------
// Application state
// ---------------------------------------------------------------------------

/// The complete mutable state of the Verij TUI.
#[derive(Default)]
pub struct AppState {
    /// Current snapshot of all sessions, as received from the plugin.
    pub sessions: Vec<SessionSnapshot>,

    /// Flattened, ordered list of navigable tree nodes.
    /// Rebuilt whenever `sessions`, `collapsed`, `attached_session`, or `active_tab_position` changes.
    pub nodes: Vec<TreeNode>,

    /// Cursor position: index into `nodes`. Clamped to `nodes.len() - 1`.
    pub cursor: usize,

    /// Set of session names whose tab lists are collapsed.
    pub collapsed: HashSet<String>,

    /// Name of the session currently attached inside the Workspace pane (if detected).
    pub attached_session: Option<String>,

    /// Position of the active tab within the attached Workspace session (0-indexed).
    pub active_tab_position: Option<usize>,

    /// Ratatui list state managing scroll offset and selected item.
    pub list_state: ListState,

    /// Animation / event tick counter for spinners.
    pub tick: usize,

    /// Non-fatal error message to display in the status bar, if any.
    pub error: Option<String>,
}

impl AppState {
    /// Replace `sessions` with the new snapshot and rebuild the node tree.
    ///
    /// Preserves cursor position on the same logical node when possible.
    pub fn reconcile(&mut self, new_sessions: Vec<SessionSnapshot>) {
        let previous_target = self.nodes.get(self.cursor).map(|n| match n {
            TreeNode::Session { name, .. } => (name.clone(), None),
            TreeNode::Tab {
                session_name,
                position,
                ..
            } => (session_name.clone(), Some(*position)),
        });

        // Detect attached session and active tab in Workspace pane
        let host_session = std::env::var("ZELLIJ_SESSION_NAME").ok();
        let ws = crate::actions::detect_workspace_state(host_session.as_deref());
        self.attached_session = ws.attached_session;
        self.active_tab_position = ws.active_tab_position;

        self.sessions = new_sessions;
        self.rebuild_nodes();

        // Restore cursor to the same logical node if it still exists.
        if let Some((prev_session, prev_tab)) = previous_target {
            let found = self.nodes.iter().position(|n| match (n, prev_tab) {
                (TreeNode::Session { name, .. }, None) => name == &prev_session,
                (
                    TreeNode::Tab {
                        session_name,
                        position,
                        ..
                    },
                    Some(p),
                ) => session_name == &prev_session && *position == p,
                _ => false,
            });

            // If a tab was selected but its session is now collapsed, snap cursor to the session row.
            let fallback = self.nodes.iter().position(|n| match n {
                TreeNode::Session { name, .. } => name == &prev_session,
                _ => false,
            });

            self.cursor = found
                .or(fallback)
                .unwrap_or_else(|| self.cursor.min(self.nodes.len().saturating_sub(1)));
        } else {
            self.cursor = self.cursor.min(self.nodes.len().saturating_sub(1));
        }

        self.sync_list_state();
    }

    /// Refresh attached session and active tab state from process tree / Zellij action.
    pub fn update_attached_session(&mut self) {
        let host_session = std::env::var("ZELLIJ_SESSION_NAME").ok();
        let ws = crate::actions::detect_workspace_state(host_session.as_deref());
        if self.attached_session != ws.attached_session
            || self.active_tab_position != ws.active_tab_position
        {
            self.attached_session = ws.attached_session;
            self.active_tab_position = ws.active_tab_position;
            self.rebuild_nodes();
            self.sync_list_state();
        }
    }

    /// Toggles the collapsed state of the session at (or containing) the cursor.
    pub fn toggle_collapse(&mut self) {
        let Some(node) = self.nodes.get(self.cursor).cloned() else {
            return;
        };

        let session_name = node.session_name().to_string();
        if self.collapsed.contains(&session_name) {
            self.collapsed.remove(&session_name);
        } else {
            self.collapsed.insert(session_name.clone());
        }

        self.rebuild_nodes();

        // Snap cursor to the session node
        if let Some(pos) = self.nodes.iter().position(|n| match n {
            TreeNode::Session { name, .. } => name == &session_name,
            _ => false,
        }) {
            self.cursor = pos;
        }

        self.sync_list_state();
    }

    /// Expands the selected session if collapsed, or moves to its first tab.
    pub fn expand_selected(&mut self) {
        let Some(node) = self.nodes.get(self.cursor).cloned() else {
            return;
        };

        if let TreeNode::Session {
            name,
            is_collapsed,
            tab_count,
            ..
        } = node
        {
            if is_collapsed {
                self.collapsed.remove(&name);
                self.rebuild_nodes();
                self.sync_list_state();
            } else if tab_count > 0 {
                self.cursor_down();
            }
        }
    }

    /// Collapses the selected session if expanded, or moves to its parent session.
    pub fn collapse_selected(&mut self) {
        let Some(node) = self.nodes.get(self.cursor).cloned() else {
            return;
        };

        match node {
            TreeNode::Session {
                name, is_collapsed, ..
            } => {
                if !is_collapsed {
                    self.collapsed.insert(name);
                    self.rebuild_nodes();
                    self.sync_list_state();
                }
            }
            TreeNode::Tab { session_name, .. } => {
                // Move cursor to parent session
                if let Some(pos) = self.nodes.iter().position(|n| match n {
                    TreeNode::Session { name, .. } => name == &session_name,
                    _ => false,
                }) {
                    self.cursor = pos;
                    self.sync_list_state();
                }
            }
        }
    }

    /// Move the cursor up by one row.
    pub fn cursor_up(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
        self.sync_list_state();
    }

    /// Move the cursor down by one row.
    pub fn cursor_down(&mut self) {
        if !self.nodes.is_empty() {
            self.cursor = (self.cursor + 1).min(self.nodes.len() - 1);
        }
        self.sync_list_state();
    }

    /// Move cursor up by `step` rows (PageUp).
    pub fn page_up(&mut self, step: usize) {
        self.cursor = self.cursor.saturating_sub(step);
        self.sync_list_state();
    }

    /// Move cursor down by `step` rows (PageDown).
    pub fn page_down(&mut self, step: usize) {
        if !self.nodes.is_empty() {
            self.cursor = (self.cursor + step).min(self.nodes.len() - 1);
        }
        self.sync_list_state();
    }

    /// Jump to the first node (Home / 'g').
    pub fn cursor_home(&mut self) {
        self.cursor = 0;
        self.sync_list_state();
    }

    /// Jump to the last node (End / 'G').
    pub fn cursor_end(&mut self) {
        self.cursor = self.nodes.len().saturating_sub(1);
        self.sync_list_state();
    }

    /// Select specific node index directly (used by mouse click).
    pub fn select_index(&mut self, index: usize) {
        if index < self.nodes.len() {
            self.cursor = index;
            self.sync_list_state();
        }
    }

    /// Return the currently selected tree node, if any.
    pub fn selected_node(&self) -> Option<&TreeNode> {
        self.nodes.get(self.cursor)
    }

    /// Synchronize `list_state` with current cursor.
    pub fn sync_list_state(&mut self) {
        if self.nodes.is_empty() {
            self.list_state.select(None);
        } else {
            self.list_state.select(Some(self.cursor));
        }
    }

    // -----------------------------------------------------------------------
    // Private helpers
    // -----------------------------------------------------------------------

    /// Flatten `self.sessions` into `self.nodes` respecting `self.collapsed`.
    fn rebuild_nodes(&mut self) {
        self.nodes.clear();

        for (session_index, session) in self.sessions.iter().enumerate() {
            let is_collapsed = self.collapsed.contains(&session.name);
            let tab_count = session.tabs.len();
            let is_attached = self.attached_session.as_deref() == Some(&session.name);

            // If this is the attached session, resolve active tab name from active_tab_position
            let active_tab = if is_attached {
                if let Some(pos) = self.active_tab_position {
                    session
                        .tabs
                        .iter()
                        .find(|t| t.position == pos)
                        .map(|t| t.name.clone())
                } else {
                    session.active_tab().map(|t| t.name.clone())
                }
            } else {
                session.active_tab().map(|t| t.name.clone())
            };

            self.nodes.push(TreeNode::Session {
                name: session.name.clone(),
                is_current: session.is_current,
                is_attached,
                is_collapsed,
                active_tab,
                tab_count,
                session_index,
            });

            if !is_collapsed {
                for tab in &session.tabs {
                    // Exactly one tab can be active across the manager:
                    // must be in the attached workspace session AND match active_tab_position
                    let is_workspace_active = if is_attached {
                        if let Some(pos) = self.active_tab_position {
                            tab.position == pos
                        } else {
                            tab.is_active
                        }
                    } else {
                        false
                    };

                    self.nodes.push(TreeNode::Tab {
                        session_name: session.name.clone(),
                        session_index,
                        name: tab.name.clone(),
                        position: tab.position,
                        is_workspace_active,
                    });
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
    use verij_types::TabSnapshot;

    fn make_test_sessions() -> Vec<SessionSnapshot> {
        vec![
            SessionSnapshot {
                name: "ss".to_string(),
                is_current: false,
                tabs: vec![
                    TabSnapshot {
                        name: "Tab #1".to_string(),
                        position: 0,
                        is_active: false, // Simulating Zellij stale data
                    },
                    TabSnapshot {
                        name: "Portal".to_string(),
                        position: 1,
                        is_active: true, // Ghost active from Zellij
                    },
                    TabSnapshot {
                        name: "magy".to_string(),
                        position: 4,
                        is_active: true, // Ghost active from Zellij
                    },
                ],
            },
            SessionSnapshot {
                name: "other".to_string(),
                is_current: false,
                tabs: vec![TabSnapshot {
                    name: "runner".to_string(),
                    position: 0,
                    is_active: true,
                }],
            },
        ]
    }

    #[test]
    fn test_authoritative_single_active_tab() {
        let mut state = AppState::default();
        state.sessions = make_test_sessions();
        // Attached to "ss", true active tab position is 0 ("Tab #1")
        state.attached_session = Some("ss".to_string());
        state.active_tab_position = Some(0);
        state.rebuild_nodes();

        // Node 0: Session "ss"
        // Node 1: Tab #1 (position 0) -> MUST be active
        // Node 2: Portal (position 1) -> MUST NOT be active despite is_active == true
        // Node 3: magy (position 4) -> MUST NOT be active despite is_active == true
        // Node 4: Session "other"
        // Node 5: runner (position 0) -> MUST NOT be active because "other" is not attached
        assert_eq!(state.nodes.len(), 6);

        assert!(state.nodes[1].is_workspace_active_tab());
        assert!(!state.nodes[2].is_workspace_active_tab());
        assert!(!state.nodes[3].is_workspace_active_tab());
        assert!(!state.nodes[5].is_workspace_active_tab());

        // Count how many tabs are workspace active in the whole tree: STRICTLY 1
        let active_count = state
            .nodes
            .iter()
            .filter(|n| n.is_workspace_active_tab())
            .count();
        assert_eq!(active_count, 1);
    }

    #[test]
    fn test_reconcile_and_navigation() {
        let mut state = AppState::default();
        state.reconcile(make_test_sessions());

        assert_eq!(state.cursor, 0);
        state.cursor_down();
        assert_eq!(state.cursor, 1);
        assert!(
            matches!(state.selected_node(), Some(TreeNode::Tab { name, .. }) if name == "Tab #1")
        );

        state.cursor_end();
        assert_eq!(state.cursor, 5);

        state.cursor_home();
        assert_eq!(state.cursor, 0);
    }

    #[test]
    fn test_collapse_and_expand() {
        let mut state = AppState::default();
        state.reconcile(make_test_sessions());

        state.toggle_collapse();
        assert!(state.collapsed.contains("ss"));
        // ss (0), other (1), runner (2) = 3 nodes
        assert_eq!(state.nodes.len(), 3);
        assert_eq!(state.cursor, 0);

        state.toggle_collapse();
        assert!(!state.collapsed.contains("ss"));
        assert_eq!(state.nodes.len(), 6);
    }
}
