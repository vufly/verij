/// verij-cli/src/tui/state.rs
///
/// Application state for the Ratatui TUI.
///
/// The state is a "dumb view" — it is replaced atomically on every snapshot
/// message from the pipe. No delta tracking; each message is the full truth.
use crate::pipe_reader::SessionSnapshot;

// ---------------------------------------------------------------------------
// Tree node model
// ---------------------------------------------------------------------------

/// A single navigable item in the flattened session/tab tree.
///
/// The cursor is an index into a `Vec<TreeNode>` that is rebuilt from the
/// current `AppState.sessions` on every reconciliation.
#[derive(Debug, Clone)]
pub enum TreeNode {
    /// A top-level session row.
    Session {
        name: String,
        is_current: bool,
        _session_index: usize,
    },
    /// A tab row nested under a session.
    Tab {
        session_name: String,
        _session_index: usize,
        name: String,
        position: usize,
        is_active: bool,
    },
}

impl TreeNode {
    /// Display label with indentation for the tree level.
    pub fn display_label(&self) -> String {
        match self {
            TreeNode::Session { name, is_current, .. } => {
                let marker = if *is_current { "▶ " } else { "  " };
                format!("{marker}{name}")
            }
            TreeNode::Tab { name, is_active, .. } => {
                let marker = if *is_active { "● " } else { "  " };
                format!("    {marker}{name}")
            }
        }
    }

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
#[derive(Default)]
pub struct AppState {
    /// Current snapshot of all sessions, as received from the plugin.
    pub sessions: Vec<SessionSnapshot>,

    /// Flattened, ordered list of navigable tree nodes.
    /// Rebuilt by `reconcile()` whenever `sessions` changes.
    pub nodes: Vec<TreeNode>,

    /// Cursor position: index into `nodes`. Clamped to `nodes.len() - 1`.
    pub cursor: usize,

    /// Non-fatal error message to display in the status bar, if any.
    pub error: Option<String>,
}

impl AppState {
    /// Replace `sessions` with the new snapshot and rebuild the node tree.
    ///
    /// Attempts to preserve the cursor position by name-matching after
    /// reconciliation. Falls back to clamping if the previously selected node
    /// is no longer present.
    pub fn reconcile(&mut self, new_sessions: Vec<SessionSnapshot>) {
        // Remember what was selected before the update.
        let previous_name = self.nodes.get(self.cursor).map(|n| match n {
            TreeNode::Session { name, .. } => (name.clone(), None),
            TreeNode::Tab { session_name, position, .. } => {
                (session_name.clone(), Some(*position))
            }
        });

        self.sessions = new_sessions;
        self.rebuild_nodes();

        // Restore cursor to the same logical node if it still exists.
        if let Some((prev_session, prev_tab)) = previous_name {
            let found = self.nodes.iter().position(|n| match (n, prev_tab) {
                (TreeNode::Session { name, .. }, None) => name == &prev_session,
                (TreeNode::Tab { session_name, position, .. }, Some(p)) => {
                    session_name == &prev_session && *position == p
                }
                _ => false,
            });

            self.cursor = found.unwrap_or(self.cursor.min(self.nodes.len().saturating_sub(1)));
        } else {
            self.cursor = self.cursor.min(self.nodes.len().saturating_sub(1));
        }
    }

    /// Move the cursor up (towards index 0), with clamping.
    pub fn cursor_up(&mut self) {
        self.cursor = self.cursor.saturating_sub(1);
    }

    /// Move the cursor down (towards `nodes.len()`), with clamping.
    pub fn cursor_down(&mut self) {
        if !self.nodes.is_empty() {
            self.cursor = (self.cursor + 1).min(self.nodes.len() - 1);
        }
    }

    /// Return the currently selected tree node, if any.
    pub fn selected_node(&self) -> Option<&TreeNode> {
        self.nodes.get(self.cursor)
    }

    // -----------------------------------------------------------------------
    // Private helpers
    // -----------------------------------------------------------------------

    /// Flatten `self.sessions` into `self.nodes` in display order:
    /// Session → its Tabs → next Session → …
    fn rebuild_nodes(&mut self) {
        self.nodes.clear();

        for (session_index, session) in self.sessions.iter().enumerate() {
            self.nodes.push(TreeNode::Session {
                name: session.name.clone(),
                is_current: session.is_current,
                _session_index: session_index,
            });

            for tab in &session.tabs {
                self.nodes.push(TreeNode::Tab {
                    session_name: session.name.clone(),
                    _session_index: session_index,
                    name: tab.name.clone(),
                    position: tab.position,
                    is_active: tab.is_active,
                });
            }
        }
    }
}
