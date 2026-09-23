/// verij-cli/src/tui/state.rs
///
/// Application state for the Ratatui TUI.
///
/// The state holds the current session snapshots (ingested from `/tmp/verij/states/`),
/// flattened navigable tree, selection cursor, collapsed set, active session tracking,
/// and ListState.
use ratatui::widgets::ListState;
use std::collections::HashSet;
use verij_types::SessionSnapshot;

use crate::config::{Config, WorkspaceMode};

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

    /// True if this node is the active tab of the currently attached workspace session.
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

/// TUI input mode: Normal tree navigation vs typing a new session name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum InputMode {
    #[default]
    Normal,
    NewSession,
}

/// The complete mutable state of the Verij TUI.
#[derive(Default)]
pub struct AppState {
    /// Loaded user configuration (colors, workspace mode, prefix, …).
    pub config: Config,

    /// Current snapshot of all non-host sessions, as received from the FS watcher.
    pub sessions: Vec<SessionSnapshot>,

    /// Flattened, ordered list of navigable tree nodes.
    /// Rebuilt whenever `sessions`, `collapsed`, or `active_session` changes.
    pub nodes: Vec<TreeNode>,

    /// Cursor position: index into `nodes`. Clamped to `nodes.len() - 1`.
    pub cursor: usize,

    /// Set of session names whose tab lists are collapsed.
    pub collapsed: HashSet<String>,

    /// Name of the session currently focused / attached in the right pane.
    pub active_session: Option<String>,

    /// Last name applied to the host Workspace pane.
    pub workspace_pane_name: Option<String>,

    /// Current input mode (Normal vs NewSession prompt).
    pub input_mode: InputMode,

    /// Text buffer when typing a new session name in `InputMode::NewSession`.
    pub input_buffer: String,

    /// Path to `verij_plugin.wasm` for injecting into newly created inner sessions.
    pub plugin_path: Option<std::path::PathBuf>,

    /// Ratatui list state managing scroll offset and selected item.
    pub list_state: ListState,

    /// Animation / event tick counter for spinners.
    pub tick: usize,

    /// Non-fatal error message to display in the status bar, if any.
    pub error: Option<String>,

    /// Whether the keyboard shortcut help bar is displayed at the bottom (toggled by '?').
    pub show_help: bool,
}

impl AppState {
    /// Build a new `AppState` with the given config.
    pub fn with_config(config: Config) -> Self {
        Self {
            config,
            ..Self::default()
        }
    }

    /// Return the effective host-session prefix (from config, defaults to `_vj_`).
    #[allow(dead_code)]
    pub fn prefix(&self) -> &str {
        self.config.prefix()
    }

    /// Return the configured default workspace mode.
    #[allow(dead_code)]
    pub fn workspace_mode(&self) -> WorkspaceMode {
        self.config.workspace.default_mode
    }

    /// Toggles the keyboard shortcut help bar.
    pub fn toggle_help(&mut self) {
        self.show_help = !self.show_help;
    }
    /// Opens the new session creation prompt.
    pub fn start_new_session_prompt(&mut self) {
        self.input_mode = InputMode::NewSession;
        self.input_buffer.clear();
        self.error = None;
    }

    /// Closes the new session creation prompt and returns to Normal mode.
    pub fn cancel_new_session_prompt(&mut self) {
        self.input_mode = InputMode::Normal;
        self.input_buffer.clear();
    }

    /// Appends an allowed character to the session name input buffer.
    pub fn handle_input_char(&mut self, c: char) {
        if self.input_buffer.len() < 32 && (c.is_alphanumeric() || c == '-' || c == '_') {
            self.input_buffer.push(c);
        }
    }

    /// Removes the last character from the session name input buffer.
    pub fn handle_input_backspace(&mut self) {
        self.input_buffer.pop();
    }
    /// Replace `sessions` with new snapshots and rebuild the node tree.
    ///
    /// Filters out any session starting with `_vj_`.
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

        // Strict filtering: filter out any host sessions
        let prefix = self.config.prefix().to_string();
        self.sessions = new_sessions
            .into_iter()
            .filter(|s| !s.name.starts_with(&prefix))
            .collect();

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

    /// Format the Workspace pane name from the target session's active tab and pane.
    pub fn format_workspace_pane_name(&self, session_name: &str) -> String {
        let snapshot = self.sessions.iter().find(|session| session.name == session_name);
        let tab = snapshot.and_then(|session| session.active_tab()).map(|tab| tab.name.as_str());
        let pane = snapshot.and_then(|session| session.active_pane());
        self.config
            .workspace
            .format_pane_name(session_name, tab, pane)
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
    pub fn rebuild_nodes(&mut self) {
        self.nodes.clear();

        for (session_index, session) in self.sessions.iter().enumerate() {
            // Defensively skip any host sessions
            if session.name.starts_with(self.config.prefix()) {
                continue;
            }

            let is_collapsed = self.collapsed.contains(&session.name);
            let tab_count = session.tabs.len();
            let is_attached = self.active_session.as_deref() == Some(&session.name);

            // Active tab in session
            let active_tab = session.active_tab().map(|t| t.name.clone());

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
                    // Exactly one tab is workspace active: active tab in active_session
                    let is_workspace_active = is_attached && tab.is_active;

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
                name: "backend".to_string(),
                is_current: false,
                tabs: vec![
                    TabSnapshot {
                        name: "editor".to_string(),
                        position: 0,
                        is_active: true,
                    },
                    TabSnapshot {
                        name: "logs".to_string(),
                        position: 1,
                        is_active: false,
                    },
                ],
                active_pane: None,
                connected_clients: None,
            },
            SessionSnapshot {
                name: "_vj_main".to_string(),
                is_current: true,
                tabs: vec![TabSnapshot {
                    name: "host".to_string(),
                    position: 0,
                    is_active: true,
                }],
                active_pane: None,
                connected_clients: None,
            },
            SessionSnapshot {
                name: "frontend".to_string(),
                is_current: false,
                tabs: vec![TabSnapshot {
                    name: "ui".to_string(),
                    position: 0,
                    is_active: true,
                }],
                active_pane: None,
                connected_clients: None,
            },
        ]
    }

    #[test]
    fn test_host_filtering() {
        let mut state = AppState::default();
        state.reconcile(make_test_sessions());

        // "_vj_main" MUST be filtered out
        assert_eq!(state.sessions.len(), 2);
        assert_eq!(state.sessions[0].name, "backend");
        assert_eq!(state.sessions[1].name, "frontend");

        // Verify nodes do not contain host session
        assert!(state
            .nodes
            .iter()
            .all(|n| !n.session_name().starts_with("_vj_")));
    }

    #[test]
    fn test_active_session_and_tab_tracking() {
        let mut state = AppState::default();
        state.reconcile(make_test_sessions());
        state.active_session = Some("backend".to_string());
        state.rebuild_nodes();

        // backend session + 2 tabs, frontend session + 1 tab = 5 nodes
        assert_eq!(state.nodes.len(), 5);

        // Node 0: Session "backend" (attached)
        // Node 1: Tab "editor" (active tab of attached session -> is_workspace_active: true)
        // Node 2: Tab "logs" (inactive -> is_workspace_active: false)
        // Node 3: Session "frontend" (not attached)
        // Node 4: Tab "ui" (is_active in its session, but frontend is not active_session -> is_workspace_active: false)
        assert!(state.nodes[1].is_workspace_active_tab());
        assert!(!state.nodes[2].is_workspace_active_tab());
        assert!(!state.nodes[4].is_workspace_active_tab());

        let active_count = state
            .nodes
            .iter()
            .filter(|n| n.is_workspace_active_tab())
            .count();
        assert_eq!(active_count, 1);
    }

    #[test]
    fn test_navigation_and_collapse() {
        let mut state = AppState::default();
        state.reconcile(make_test_sessions());

        assert_eq!(state.cursor, 0);
        state.cursor_down();
        assert_eq!(state.cursor, 1);
        assert!(
            matches!(state.selected_node(), Some(TreeNode::Tab { name, .. }) if name == "editor")
        );

        state.toggle_collapse();
        assert!(state.collapsed.contains("backend"));
        // backend collapsed (1) + frontend (1) + ui tab (1) = 3 nodes
        assert_eq!(state.nodes.len(), 3);
        assert_eq!(state.cursor, 0);

        state.toggle_collapse();
        assert!(!state.collapsed.contains("backend"));
        assert_eq!(state.nodes.len(), 5);
    }
}
