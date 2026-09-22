/// verij-cli/src/tui/state.rs
///
/// Application state for the Ratatui TUI.
///
/// The state holds the current session snapshot, flattened navigable tree,
/// selection cursor, collapsed set, and Ratatui ListState for viewport scrolling.
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
        is_active: bool,
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
}

// ---------------------------------------------------------------------------
// Application state
// ---------------------------------------------------------------------------

/// The complete mutable state of the Verij TUI.
pub struct AppState {
    /// Current snapshot of all sessions, as received from the plugin.
    pub sessions: Vec<SessionSnapshot>,

    /// Flattened, ordered list of navigable tree nodes.
    /// Rebuilt whenever `sessions` or `collapsed` changes.
    pub nodes: Vec<TreeNode>,

    /// Cursor position: index into `nodes`. Clamped to `nodes.len() - 1`.
    pub cursor: usize,

    /// Set of session names whose tab lists are collapsed.
    pub collapsed: HashSet<String>,

    /// Ratatui list state managing scroll offset and selected item.
    pub list_state: ListState,

    /// Animation / event tick counter for spinners.
    pub tick: usize,

    /// Non-fatal error message to display in the status bar, if any.
    pub error: Option<String>,
}

impl Default for AppState {
    fn default() -> Self {
        Self {
            sessions: Vec::new(),
            nodes: Vec::new(),
            cursor: 0,
            collapsed: HashSet::new(),
            list_state: ListState::default(),
            tick: 0,
            error: None,
        }
    }
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

        self.sessions = new_sessions;
        self.rebuild_nodes();

        // Restore cursor to the same logical node if it still exists.
        if let Some((prev_session, prev_tab)) = previous_target {
            let found = self.nodes.iter().position(|n| match (n, prev_tab) {
                (TreeNode::Session { name, .. }, None) => name == &prev_session,
                (TreeNode::Tab {
                    session_name,
                    position,
                    ..
                }, Some(p)) => session_name == &prev_session && *position == p,
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

        if let TreeNode::Session { name, is_collapsed, tab_count, .. } = node {
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
            TreeNode::Session { name, is_collapsed, .. } => {
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
            let active_tab = session.active_tab().map(|t| t.name.clone());
            let tab_count = session.tabs.len();

            self.nodes.push(TreeNode::Session {
                name: session.name.clone(),
                is_current: session.is_current,
                is_collapsed,
                active_tab,
                tab_count,
                session_index,
            });

            if !is_collapsed {
                for tab in &session.tabs {
                    self.nodes.push(TreeNode::Tab {
                        session_name: session.name.clone(),
                        session_index,
                        name: tab.name.clone(),
                        position: tab.position,
                        is_active: tab.is_active,
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
                name: "dev".to_string(),
                is_current: true,
                tabs: vec![
                    TabSnapshot {
                        name: "editor".to_string(),
                        position: 0,
                        is_active: true,
                    },
                    TabSnapshot {
                        name: "term".to_string(),
                        position: 1,
                        is_active: false,
                    },
                ],
            },
            SessionSnapshot {
                name: "test".to_string(),
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
    fn test_reconcile_and_navigation() {
        let mut state = AppState::default();
        state.reconcile(make_test_sessions());

        // dev (0), editor (1), term (2), test (3), runner (4) = 5 nodes
        assert_eq!(state.nodes.len(), 5);
        assert_eq!(state.cursor, 0);

        state.cursor_down();
        assert_eq!(state.cursor, 1);
        assert_eq!(
            state.selected_node(),
            Some(&TreeNode::Tab {
                session_name: "dev".to_string(),
                session_index: 0,
                name: "editor".to_string(),
                position: 0,
                is_active: true,
            })
        );

        state.cursor_end();
        assert_eq!(state.cursor, 4);

        state.cursor_home();
        assert_eq!(state.cursor, 0);
    }

    #[test]
    fn test_collapse_and_expand() {
        let mut state = AppState::default();
        state.reconcile(make_test_sessions());

        // Toggle collapse on first session ("dev")
        state.toggle_collapse();
        assert!(state.collapsed.contains("dev"));
        // dev (0), test (1), runner (2) = 3 nodes
        assert_eq!(state.nodes.len(), 3);
        assert_eq!(state.cursor, 0);

        // Expand again
        state.toggle_collapse();
        assert!(!state.collapsed.contains("dev"));
        assert_eq!(state.nodes.len(), 5);
    }

    #[test]
    fn test_cursor_preservation_when_collapsed() {
        let mut state = AppState::default();
        state.reconcile(make_test_sessions());

        // Select "term" tab (index 2)
        state.select_index(2);
        assert_eq!(state.cursor, 2);

        // Collapse "dev" session via collapse_selected from tab
        state.collapse_selected();
        // Cursor snaps to parent session "dev" (index 0)
        assert_eq!(state.cursor, 0);
        assert_eq!(state.selected_node().unwrap().session_name(), "dev");
    }
}
