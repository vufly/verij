/// verij-cli/src/tui/state.rs
///
/// Application state for the Ratatui TUI.
///
/// The state holds the current session snapshots (ingested from `/tmp/verij/states/`),
/// flattened navigable tree, selection cursor, collapsed set, active session tracking,
/// and ListState.
pub use super::tree_model::*;

use ratatui::widgets::ListState;
use std::collections::HashSet;
use verij_types::identity::PaneKey;
use verij_types::SessionSnapshot;

use super::agent_view::{AgentSummary, AgentView};
use super::tree_format::TreeFormatter;
use crate::config::{Config, WorkspaceMode};

/// TUI input mode: Normal tree navigation vs typing a new session name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum InputMode {
    #[default]
    Normal,
    NewSession,
    RenameHost,
}

#[derive(Debug, Clone)]
struct SelectionTarget {
    key: NodeKey,
    parent: Option<NodeKey>,
    session_name: String,
    tab_position: Option<usize>,
}

/// The complete mutable state of the Verij TUI.
#[derive(Default)]
pub struct AppState {
    /// Loaded user configuration (colors and host options).
    pub config: Config,

    /// Current snapshot of all non-host sessions, as received from the FS watcher.
    pub sessions: Vec<SessionSnapshot>,

    /// Normalized agent presentation views provided by main.
    pub agent_views: Vec<AgentView>,

    /// Flattened, ordered list of navigable tree nodes.
    /// Rebuilt whenever `sessions`, `collapsed`, `collapsed_tabs`, `collapsed_keys`,
    /// or `active_session` changes.
    pub nodes: Vec<TreeNode>,

    /// Cursor position: index into `nodes`. Clamped to `nodes.len() - 1`.
    pub cursor: usize,

    /// Set of session names whose tab lists are collapsed (legacy fallback).
    pub collapsed: HashSet<String>,

    /// Set of tab keys that are collapsed.
    pub collapsed_tabs: HashSet<NodeKey>,

    /// Set of all node keys (sessions and tabs) that are collapsed.
    pub collapsed_keys: HashSet<NodeKey>,

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

    /// Compiled row templates; never reparse while drawing frames.
    pub tree_formatter: TreeFormatter,

    /// Fold-marker cell ranges, indexed by navigable row after each draw.
    pub fold_hitboxes: Vec<Option<std::ops::Range<u16>>>,

    /// Animation / event tick counter for spinners.
    pub tick: usize,

    /// Non-fatal error message to display in the status bar, if any.
    pub error: Option<String>,

    /// Whether the keyboard shortcut help bar is displayed at the bottom (toggled by '?').
    pub show_help: bool,

    /// Session/tab requests are serialized outside terminal input/render.
    pub activation: Option<crate::activation::Queue>,

    /// Normalized native records; agent tree presentation follows at H2.
    pub agents: Vec<crate::agent_watcher::AgentRecord>,
}

impl AppState {
    /// Build a new `AppState` with the given config.
    pub fn with_config(config: Config) -> Self {
        let tree_formatter = TreeFormatter::new(&config.tui.tree, &config.colors);
        Self {
            config,
            tree_formatter,
            ..Self::default()
        }
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

    pub fn start_rename_host_prompt(&mut self) {
        self.input_mode = InputMode::RenameHost;
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

    /// Set host-derived agent views, rebuilding nodes while preserving selection.
    pub fn set_agent_views(&mut self, views: Vec<AgentView>) {
        let prev = self.capture_selection();
        self.agent_views = views;
        self.rebuild_nodes();
        self.restore_selection(&prev);
        self.sync_list_state();
    }

    /// Update active status across agent views and rebuild without resetting selection.
    pub fn apply_focus(&mut self, focused_pane: Option<PaneKey>) {
        let prev = self.capture_selection();
        for view in &mut self.agent_views {
            view.is_active = focused_pane.as_ref().map_or(false, |p| p == &view.pane);
        }
        self.rebuild_nodes();
        self.restore_selection(&prev);
        self.sync_list_state();
    }

    /// Save persistable tree state DTO.
    pub fn save_persistable_state(&self) -> TreePersistableState {
        TreePersistableState {
            collapsed_keys: self.collapsed_keys.clone(),
            collapsed_tabs: self.collapsed_tabs.clone(),
            selected_key: self.selected_node().map(|n| n.key().clone()),
        }
    }

    /// Restore tree folds and selection from persistable state DTO.
    pub fn restore_tree_state(&mut self, state: TreePersistableState) {
        self.collapsed_keys = state.collapsed_keys;
        self.collapsed_tabs = state.collapsed_tabs;
        self.rebuild_nodes();
        if let Some(key) = state.selected_key {
            let target = SelectionTarget {
                key,
                parent: None,
                session_name: String::new(),
                tab_position: None,
            };
            self.restore_selection(&Some(target));
        }
        self.sync_list_state();
    }

    /// Replace `sessions` with new snapshots and rebuild the node tree.
    /// Preserves cursor position on the same logical node when possible.
    pub fn reconcile(&mut self, new_sessions: Vec<SessionSnapshot>) {
        let prev = self.capture_selection();
        self.sessions = new_sessions;
        self.rebuild_nodes();
        self.restore_selection(&prev);
        self.sync_list_state();
    }

    /// Toggles the collapsed state of the session or tab at the cursor.
    /// Space/Tab:
    /// - agent leaf folds parent tab and selects parent
    /// - tab with children toggles tab only
    /// - tab without agents folds parent session (legacy behavior)
    /// - session toggles session fold (exited session stays folded)
    pub fn toggle_collapse(&mut self) {
        let Some(node) = self.selected_node().cloned() else {
            return;
        };

        match node {
            TreeNode::Session {
                name,
                needs_resurrection,
                meta,
                ..
            } => {
                if needs_resurrection {
                    return;
                }
                let is_collapsed =
                    self.collapsed_keys.contains(&meta.key) || self.collapsed.contains(&name);
                if is_collapsed {
                    self.collapsed_keys.remove(&meta.key);
                    self.collapsed.remove(&name);
                } else {
                    self.collapsed_keys.insert(meta.key.clone());
                    if matches!(meta.key, NodeKey::Session(SessionKey::Legacy(_))) {
                        self.collapsed.insert(name.clone());
                    }
                }

                self.rebuild_nodes();

                if let Some(pos) = self.nodes.iter().position(|n| n.key() == &meta.key) {
                    self.cursor = pos;
                }
                self.sync_list_state();
            }
            TreeNode::Tab {
                session_name,
                has_children,
                meta,
                ..
            } => {
                if has_children {
                    let is_collapsed = self.collapsed_tabs.contains(&meta.key)
                        || self.collapsed_keys.contains(&meta.key);
                    if is_collapsed {
                        self.collapsed_tabs.remove(&meta.key);
                        self.collapsed_keys.remove(&meta.key);
                    } else {
                        self.collapsed_tabs.insert(meta.key.clone());
                        self.collapsed_keys.insert(meta.key.clone());
                    }

                    self.rebuild_nodes();

                    if let Some(pos) = self.nodes.iter().position(|n| n.key() == &meta.key) {
                        self.cursor = pos;
                    }
                    self.sync_list_state();
                } else {
                    if let Some(parent_key) = &meta.parent {
                        let is_collapsed = self.collapsed_keys.contains(parent_key)
                            || self.collapsed.contains(&session_name);
                        if is_collapsed {
                            self.collapsed_keys.remove(parent_key);
                            self.collapsed.remove(&session_name);
                        } else {
                            self.collapsed_keys.insert(parent_key.clone());
                            if matches!(parent_key, NodeKey::Session(SessionKey::Legacy(_))) {
                                self.collapsed.insert(session_name.clone());
                            }
                        }

                        self.rebuild_nodes();

                        if let Some(pos) = self.nodes.iter().position(|n| n.key() == parent_key) {
                            self.cursor = pos;
                        }
                        self.sync_list_state();
                    } else {
                        if self.collapsed.contains(&session_name) {
                            self.collapsed.remove(&session_name);
                        } else {
                            self.collapsed.insert(session_name.clone());
                        }
                        self.rebuild_nodes();
                        if let Some(pos) = self.nodes.iter().position(|n| match n {
                            TreeNode::Session { name, .. } => name == &session_name,
                            _ => false,
                        }) {
                            self.cursor = pos;
                        }
                        self.sync_list_state();
                    }
                }
            }
            TreeNode::AgentPane { meta, .. } => {
                if let Some(parent_key) = &meta.parent {
                    self.collapsed_tabs.insert(parent_key.clone());
                    self.collapsed_keys.insert(parent_key.clone());

                    self.rebuild_nodes();

                    if let Some(pos) = self.nodes.iter().position(|n| n.key() == parent_key) {
                        self.cursor = pos;
                    }
                    self.sync_list_state();
                }
            }
        }
    }

    /// Expands the selected branch, or moves to its first child.
    pub fn expand_selected(&mut self) {
        let Some(node) = self.selected_node().cloned() else {
            return;
        };

        match node {
            TreeNode::Session {
                name,
                is_collapsed,
                tab_count,
                needs_resurrection,
                meta,
                ..
            } => {
                if needs_resurrection {
                    return;
                }
                if is_collapsed {
                    self.collapsed_keys.remove(&meta.key);
                    self.collapsed.remove(&name);
                    self.rebuild_nodes();
                    self.sync_list_state();
                } else if tab_count > 0 {
                    self.cursor_down();
                }
            }
            TreeNode::Tab {
                has_children,
                is_collapsed,
                meta,
                ..
            } => {
                if has_children {
                    if is_collapsed {
                        self.collapsed_tabs.remove(&meta.key);
                        self.collapsed_keys.remove(&meta.key);
                        self.rebuild_nodes();
                        self.sync_list_state();
                    } else {
                        self.cursor_down();
                    }
                }
            }
            TreeNode::AgentPane { .. } => {}
        }
    }

    /// Collapses the selected branch, or moves to its parent.
    pub fn collapse_selected(&mut self) {
        let Some(node) = self.selected_node().cloned() else {
            return;
        };

        match node {
            TreeNode::Session {
                name,
                is_collapsed,
                meta,
                ..
            } => {
                if !is_collapsed {
                    if matches!(meta.key, NodeKey::Session(SessionKey::Legacy(_))) {
                        self.collapsed.insert(name);
                    }
                    self.collapsed_keys.insert(meta.key);
                    self.rebuild_nodes();
                    self.sync_list_state();
                }
            }
            TreeNode::Tab {
                has_children,
                is_collapsed,
                meta,
                session_name,
                ..
            } => {
                if has_children && !is_collapsed {
                    self.collapsed_tabs.insert(meta.key.clone());
                    self.collapsed_keys.insert(meta.key.clone());
                    self.rebuild_nodes();
                    self.sync_list_state();
                } else {
                    if let Some(parent_key) = &meta.parent {
                        if let Some(pos) = self.nodes.iter().position(|n| n.key() == parent_key) {
                            self.cursor = pos;
                            self.sync_list_state();
                            return;
                        }
                    }
                    if let Some(pos) = self.nodes.iter().position(|n| match n {
                        TreeNode::Session { name, .. } => name == &session_name,
                        _ => false,
                    }) {
                        self.cursor = pos;
                        self.sync_list_state();
                    }
                }
            }
            TreeNode::AgentPane { meta, .. } => {
                if let Some(parent_key) = &meta.parent {
                    if let Some(pos) = self.nodes.iter().position(|n| n.key() == parent_key) {
                        self.cursor = pos;
                        self.sync_list_state();
                    }
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
        let snapshot = self
            .sessions
            .iter()
            .find(|session| session.name == session_name);
        let tab = snapshot
            .and_then(|session| session.active_tab())
            .map(|tab| tab.name.as_str());
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

    fn capture_selection(&self) -> Option<SelectionTarget> {
        self.selected_node().map(|node| SelectionTarget {
            key: node.key().clone(),
            parent: node.meta().parent.clone(),
            session_name: node.session_name().to_string(),
            tab_position: node.tab_position(),
        })
    }

    fn restore_selection(&mut self, target: &Option<SelectionTarget>) {
        let Some(target) = target else {
            self.cursor = self.cursor.min(self.nodes.len().saturating_sub(1));
            return;
        };

        // 1. Exact typed key match
        if let Some(pos) = self.nodes.iter().position(|n| n.key() == &target.key) {
            self.cursor = pos;
            return;
        }
        if let Some(pos) = self.visible_parent_for(&target.key) {
            self.cursor = pos;
            return;
        }

        // 2. Parent fallback if hidden / deleted / moved
        if let Some(parent_key) = &target.parent {
            if let Some(pos) = self.nodes.iter().position(|n| n.key() == parent_key) {
                self.cursor = pos;
                return;
            }
            if let NodeKey::Tab { session, .. } = parent_key {
                let session_key = NodeKey::Session(session.clone());
                if let Some(pos) = self.nodes.iter().position(|n| n.key() == &session_key) {
                    self.cursor = pos;
                    return;
                }
            }
        }

        // 3. Fallback for Agent whose parent wasn't captured or tab changed
        if let NodeKey::Agent { pane, .. } = &target.key {
            let tab_pos = self.nodes.iter().position(|n| match n {
                TreeNode::Tab { meta, .. } => {
                    if let NodeKey::Tab {
                        session: SessionKey::Verified(sid),
                        ..
                    } = &meta.key
                    {
                        sid == &pane.session
                    } else {
                        false
                    }
                }
                _ => false,
            });
            let session_pos = self.nodes.iter().position(|n| match n {
                TreeNode::Session { meta, .. } => {
                    if let NodeKey::Session(SessionKey::Verified(sid)) = &meta.key {
                        sid == &pane.session
                    } else {
                        false
                    }
                }
                _ => false,
            });
            if let Some(pos) = tab_pos.or(session_pos) {
                self.cursor = pos;
                return;
            }
        }

        // 4. Legacy fallback: match by (session_name, tab_position)
        let legacy_tab_pos = target.tab_position.and_then(|p| {
            self.nodes.iter().position(|n| match n {
                TreeNode::Tab {
                    session_name,
                    position,
                    ..
                } => session_name == &target.session_name && *position == p,
                _ => false,
            })
        });
        let legacy_session_pos = self.nodes.iter().position(|n| match n {
            TreeNode::Session { name, .. } => name == &target.session_name,
            _ => false,
        });
        if let Some(pos) = legacy_tab_pos.or(legacy_session_pos) {
            self.cursor = pos;
            return;
        }

        // 5. Clamping
        self.cursor = self.cursor.min(self.nodes.len().saturating_sub(1));
    }

    pub fn visible_parent_for(&self, key: &NodeKey) -> Option<usize> {
        match key {
            NodeKey::Agent { pane, .. } => {
                let session = self.sessions.iter().find(|session| {
                    session
                        .inventory
                        .as_ref()
                        .and_then(|inventory| inventory.session_instance_id.as_ref())
                        == Some(&pane.session)
                })?;
                let inventory = session.inventory.as_ref()?;
                if let Some(location) = inventory
                    .panes
                    .iter()
                    .find(|candidate| candidate.terminal_id == pane.terminal)
                {
                    let tab = location
                        .tab_id
                        .map(TabKey::Stable)
                        .unwrap_or(TabKey::Position(location.tab_position));
                    let parent = NodeKey::Tab {
                        session: SessionKey::Verified(pane.session.clone()),
                        tab,
                    };
                    if let Some(index) = self.nodes.iter().position(|node| node.key() == &parent) {
                        return Some(index);
                    }
                }
                let root = NodeKey::Session(SessionKey::Verified(pane.session.clone()));
                self.nodes.iter().position(|node| node.key() == &root)
            }
            NodeKey::Tab { session, .. } => self
                .nodes
                .iter()
                .position(|node| node.key() == &NodeKey::Session(session.clone())),
            NodeKey::Session(_) => None,
        }
    }

    /// Rebuild flattened navigable tree respecting folds and agent membership.
    pub fn rebuild_nodes(&mut self) {
        self.nodes.clear();

        let total_sessions = self.sessions.len();

        for (session_index, session) in self.sessions.iter().enumerate() {
            let verified_sid = session
                .inventory
                .as_ref()
                .and_then(|inv| inv.session_instance_id.clone());

            let session_key = match &verified_sid {
                Some(sid) => SessionKey::Verified(sid.clone()),
                None => SessionKey::Legacy(session.name.clone()),
            };

            let session_node_key = NodeKey::Session(session_key.clone());

            let is_collapsed = session.needs_resurrection
                || self.collapsed_keys.contains(&session_node_key)
                || self.collapsed.contains(&session.name);

            let tab_count = session.tabs.len();
            let is_attached = self.active_session.as_deref() == Some(&session.name);
            let active_tab = session.active_tab().map(|t| t.name.clone());

            struct TabData {
                tab_node_key: NodeKey,
                tab_name: String,
                position: usize,
                is_workspace_active: bool,
                is_tab_collapsed: bool,
                views: Vec<AgentView>,
                summary: AgentSummary,
            }

            let mut tabs_data = Vec::with_capacity(session.tabs.len());
            let mut session_summary = AgentSummary::default();

            for tab in &session.tabs {
                let tab_meta = session
                    .inventory
                    .as_ref()
                    .and_then(|inv| inv.tabs.iter().find(|t| t.position == tab.position));
                let tab_key = match tab_meta {
                    Some(m) => TabKey::Stable(m.tab_id),
                    None => TabKey::Position(tab.position),
                };
                let tab_node_key = NodeKey::Tab {
                    session: session_key.clone(),
                    tab: tab_key,
                };

                let is_tab_collapsed = self.collapsed_tabs.contains(&tab_node_key)
                    || self.collapsed_keys.contains(&tab_node_key);

                // Collect agent views for this tab
                let mut tab_views = Vec::new();
                if let (Some(sid), Some(inv)) = (&verified_sid, &session.inventory) {
                    for view in &self.agent_views {
                        if &view.pane.session == sid {
                            if let Some(pane) = inv
                                .panes
                                .iter()
                                .find(|p| p.terminal_id == view.pane.terminal)
                            {
                                let matches_tab = if let (Some(pane_tab_id), Some(meta)) =
                                    (pane.tab_id, tab_meta)
                                {
                                    pane_tab_id == meta.tab_id
                                } else {
                                    pane.tab_position == tab.position
                                };
                                if matches_tab {
                                    tab_views.push(view.clone());
                                }
                            } else if !view.navigable
                                && view.status == verij_types::agent::AgentStatus::Unknown
                            {
                                if let Some((position, id)) = view.location {
                                    let matches_tab = match (id, tab_meta) {
                                        (Some(id), Some(meta)) => id == meta.tab_id,
                                        _ => position == tab.position,
                                    };
                                    if matches_tab {
                                        tab_views.push(view.clone());
                                    }
                                }
                            }
                        }
                    }
                }

                let mut tab_summary = AgentSummary::default();
                for view in &tab_views {
                    tab_summary.add(view.status);
                }
                session_summary.merge(&tab_summary);

                let is_workspace_active = is_attached && tab.is_active;

                tabs_data.push(TabData {
                    tab_node_key,
                    tab_name: tab.name.clone(),
                    position: tab.position,
                    is_workspace_active,
                    is_tab_collapsed,
                    views: tab_views,
                    summary: tab_summary,
                });
            }

            let session_meta = RowMeta {
                key: session_node_key.clone(),
                parent: None,
                depth: 0,
                sibling_index: session_index,
                sibling_count: total_sessions,
                ancestor_continuations: Vec::new(),
            };

            self.nodes.push(TreeNode::Session {
                name: session.name.clone(),
                is_current: session.is_current,
                is_attached,
                needs_resurrection: session.needs_resurrection,
                is_collapsed,
                active_tab,
                tab_count,
                session_index,
                meta: session_meta,
                summary: session_summary,
            });

            if !is_collapsed {
                let session_has_more = session_index + 1 < total_sessions;
                let total_tabs = tabs_data.len();

                for (tab_index, tab_data) in tabs_data.into_iter().enumerate() {
                    let has_children = !tab_data.views.is_empty();
                    let tab_meta = RowMeta {
                        key: tab_data.tab_node_key.clone(),
                        parent: Some(session_node_key.clone()),
                        depth: 1,
                        sibling_index: tab_index,
                        sibling_count: total_tabs,
                        ancestor_continuations: vec![session_has_more],
                    };

                    self.nodes.push(TreeNode::Tab {
                        session_name: session.name.clone(),
                        session_index,
                        name: tab_data.tab_name.clone(),
                        position: tab_data.position,
                        is_workspace_active: tab_data.is_workspace_active,
                        meta: tab_meta,
                        summary: tab_data.summary,
                        is_collapsed: tab_data.is_tab_collapsed,
                        has_children,
                    });

                    if !tab_data.is_tab_collapsed && has_children {
                        let tab_has_more = tab_index + 1 < total_tabs;
                        let total_agents = tab_data.views.len();

                        for (agent_index, view) in tab_data.views.into_iter().enumerate() {
                            let agent_key = NodeKey::Agent {
                                pane: view.pane.clone(),
                                instance: view.instance.clone(),
                            };
                            let agent_meta = RowMeta {
                                key: agent_key,
                                parent: Some(tab_data.tab_node_key.clone()),
                                depth: 2,
                                sibling_index: agent_index,
                                sibling_count: total_agents,
                                ancestor_continuations: vec![session_has_more, tab_has_more],
                            };

                            self.nodes.push(TreeNode::AgentPane {
                                session_name: session.name.clone(),
                                session_index,
                                tab_name: tab_data.tab_name.clone(),
                                tab_position: tab_data.position,
                                view,
                                meta: agent_meta,
                            });
                        }
                    }
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
    use verij_types::agent::{AgentKind, AgentStatus};
    use verij_types::identity::{AgentInstanceId, PaneKey, SessionInstanceId, TerminalPaneId};
    use verij_types::inventory::{PaneSnapshot, SessionInventory, TabMetadata};
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
                needs_resurrection: false,
                inventory: None,
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
                needs_resurrection: false,
                inventory: None,
            },
        ]
    }

    fn make_inventory_session(
        name: &str,
        session_id: &str,
        tabs: Vec<(&str, usize, usize)>, // (name, position, tab_id)
        panes: Vec<(u32, usize, usize)>, // (terminal_id, position, tab_id)
    ) -> SessionSnapshot {
        let tab_snapshots: Vec<TabSnapshot> = tabs
            .iter()
            .map(|(tname, pos, _)| TabSnapshot {
                name: tname.to_string(),
                position: *pos,
                is_active: *pos == 0,
            })
            .collect();

        let tab_metas: Vec<TabMetadata> = tabs
            .iter()
            .map(|(_, pos, tid)| TabMetadata {
                tab_id: *tid,
                position: *pos,
                floating_visible: false,
                fullscreen_active: false,
            })
            .collect();

        let pane_snapshots: Vec<PaneSnapshot> = panes
            .iter()
            .map(|(term_id, pos, tid)| PaneSnapshot {
                terminal_id: TerminalPaneId(*term_id),
                tab_id: Some(*tid),
                tab_position: *pos,
                title: format!("pane-{}", term_id),
                is_floating: false,
                is_suppressed: false,
                is_fullscreen: false,
                layer_focused: false,
                exited: false,
                pane_pid: None,
                pane_process: None,
                stack_id: None,
            })
            .collect();

        SessionSnapshot {
            name: name.to_string(),
            is_current: false,
            tabs: tab_snapshots,
            active_pane: None,
            connected_clients: None,
            needs_resurrection: false,
            inventory: Some(SessionInventory {
                schema_version: 1,
                producer: verij_types::identity::PluginContext {
                    server_pid: 1000,
                    plugin_id: 1,
                    client_id: 1,
                    epoch: "epoch-1".to_string(),
                },
                revision: 1,
                exported_at_ms: 1000,
                session_instance_id: Some(SessionInstanceId(session_id.to_string())),
                server_process: None,
                capabilities: vec![],
                tabs: tab_metas,
                panes: pane_snapshots,
            }),
        }
    }

    fn make_agent_view(
        session_id: &str,
        terminal_id: u32,
        agent_id: &str,
        status: AgentStatus,
    ) -> AgentView {
        AgentView {
            location: None,
            instance: AgentInstanceId(agent_id.to_string()),
            pane: PaneKey {
                session: SessionInstanceId(session_id.to_string()),
                terminal: TerminalPaneId(terminal_id),
            },
            kind: AgentKind::Agy,
            title: format!("agent-{}", agent_id),
            pane_title: format!("pane-{}", terminal_id),
            conversation_title: "conv".to_string(),
            status,
            detail: "detail".to_string(),
            is_floating: false,
            stacked: None,
            completion_revision: 1,
            is_active: false,
            is_synthetic: false,
            navigable: true,
        }
    }

    #[test]
    fn test_prefix_shaped_inner_session_is_visible() {
        let mut state = AppState::default();
        let mut sessions = make_test_sessions();
        sessions.push(SessionSnapshot {
            name: "_vj_main".to_string(),
            is_current: false,
            tabs: vec![],
            active_pane: None,
            connected_clients: None,
            needs_resurrection: false,
            inventory: None,
        });
        state.reconcile(sessions);

        assert_eq!(state.sessions.len(), 3);
        assert_eq!(state.sessions[0].name, "backend");
        assert_eq!(state.sessions[1].name, "frontend");
        assert_eq!(state.sessions[2].name, "_vj_main");
    }

    #[test]
    fn test_active_session_and_tab_tracking() {
        let mut state = AppState::default();
        state.reconcile(make_test_sessions());
        state.active_session = Some("backend".to_string());
        state.rebuild_nodes();

        assert_eq!(state.nodes.len(), 5);
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
    fn test_resurrectable_session_stays_folded_until_live() {
        let mut state = AppState::default();
        let mut sessions = make_test_sessions();
        sessions[0].needs_resurrection = true;
        state.reconcile(sessions);

        assert!(matches!(
            state.nodes[0],
            TreeNode::Session {
                needs_resurrection: true,
                is_collapsed: true,
                ..
            }
        ));
        assert_eq!(state.nodes.len(), 3);

        state.toggle_collapse();
        state.expand_selected();
        assert_eq!(state.nodes.len(), 3);
        assert!(!state.collapsed.contains("backend"));

        state.reconcile(make_test_sessions());
        assert_eq!(state.nodes.len(), 5);
        assert!(matches!(
            state.nodes[0],
            TreeNode::Session {
                is_collapsed: false,
                ..
            }
        ));
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
        assert_eq!(state.nodes.len(), 3);
        assert_eq!(state.cursor, 0);

        state.toggle_collapse();
        assert!(!state.collapsed.contains("backend"));
        assert_eq!(state.nodes.len(), 5);
    }

    #[test]
    fn old_snapshot_node_count_unchanged() {
        let mut state = AppState::default();
        let sessions = make_test_sessions();
        state.reconcile(sessions);

        // backend session (0) + 2 tabs (1, 2) + frontend session (3) + 1 tab (4) = 5 nodes
        assert_eq!(state.nodes.len(), 5);
        assert!(matches!(state.nodes[0], TreeNode::Session { .. }));
        assert!(matches!(state.nodes[1], TreeNode::Tab { .. }));
        assert!(matches!(state.nodes[2], TreeNode::Tab { .. }));
        assert!(matches!(state.nodes[3], TreeNode::Session { .. }));
        assert!(matches!(state.nodes[4], TreeNode::Tab { .. }));

        // Check RowMeta depth
        assert_eq!(state.nodes[0].meta().depth, 0);
        assert_eq!(state.nodes[1].meta().depth, 1);
        assert_eq!(state.nodes[2].meta().depth, 1);
        assert_eq!(state.nodes[3].meta().depth, 0);
        assert_eq!(state.nodes[4].meta().depth, 1);
    }

    #[test]
    fn moving_agent_across_tabs() {
        let mut state = AppState::default();
        // Initially pane 100 is in tab 0 (tab_id: 10)
        let session = make_inventory_session(
            "sess",
            "sess-1",
            vec![("editor", 0, 10), ("logs", 1, 20)],
            vec![(100, 0, 10)],
        );
        state.reconcile(vec![session]);
        state.set_agent_views(vec![make_agent_view(
            "sess-1",
            100,
            "agent-1",
            AgentStatus::Working,
        )]);

        // Nodes: Session(0), Tab editor(1), Agent agent-1(2), Tab logs(3)
        assert_eq!(state.nodes.len(), 4);
        assert!(matches!(
            state.nodes[2],
            TreeNode::AgentPane { ref tab_name, .. } if tab_name == "editor"
        ));

        // Select the agent leaf
        state.select_index(2);
        assert_eq!(state.cursor, 2);

        // Now move pane 100 to tab 1 (tab_id: 20)
        let moved_session = make_inventory_session(
            "sess",
            "sess-1",
            vec![("editor", 0, 10), ("logs", 1, 20)],
            vec![(100, 1, 20)],
        );
        state.reconcile(vec![moved_session]);

        // Nodes: Session(0), Tab editor(1), Tab logs(2), Agent agent-1(3)
        assert_eq!(state.nodes.len(), 4);
        assert!(matches!(
            state.nodes[3],
            TreeNode::AgentPane { ref tab_name, .. } if tab_name == "logs"
        ));
        // Selection is maintained on the agent by its typed key!
        assert_eq!(state.cursor, 3);
        assert!(matches!(
            state.selected_node(),
            Some(TreeNode::AgentPane { .. })
        ));
    }

    #[test]
    fn rename_preserves_selection() {
        let mut state = AppState::default();
        let session = make_inventory_session(
            "old-name",
            "sess-verified",
            vec![("editor", 0, 10)],
            vec![(100, 0, 10)],
        );
        state.reconcile(vec![session]);
        state.set_agent_views(vec![make_agent_view(
            "sess-verified",
            100,
            "agent-1",
            AgentStatus::Working,
        )]);

        // Select agent leaf (index 2)
        state.select_index(2);
        assert_eq!(state.cursor, 2);

        // Rename session to "new-name" with same instance ID
        let renamed_session = make_inventory_session(
            "new-name",
            "sess-verified",
            vec![("editor", 0, 10)],
            vec![(100, 0, 10)],
        );
        state.reconcile(vec![renamed_session]);

        // Cursor stays on agent leaf
        assert_eq!(state.cursor, 2);
        assert_eq!(state.selected_node().unwrap().session_name(), "new-name");
    }

    #[test]
    fn reorder_preserves_selection() {
        let mut state = AppState::default();
        let session1 = make_inventory_session(
            "sess-alpha",
            "sid-alpha",
            vec![("tab-a", 0, 1)],
            vec![(10, 0, 1)],
        );
        let session2 = make_inventory_session(
            "sess-beta",
            "sid-beta",
            vec![("tab-b", 0, 2)],
            vec![(20, 0, 2)],
        );
        state.reconcile(vec![session1.clone(), session2.clone()]);
        state.set_agent_views(vec![
            make_agent_view("sid-alpha", 10, "ag-alpha", AgentStatus::Working),
            make_agent_view("sid-beta", 20, "ag-beta", AgentStatus::Working),
        ]);

        // Alpha session (0), tab-a (1), ag-alpha (2), Beta session (3), tab-b (4), ag-beta (5)
        // Select ag-beta at index 5
        state.select_index(5);
        assert_eq!(state.cursor, 5);

        // Reorder sessions: Beta first, Alpha second
        state.reconcile(vec![session2, session1]);

        // Beta session (0), tab-b (1), ag-beta (2), Alpha session (3), tab-a (4), ag-alpha (5)
        // ag-beta is now at index 2
        assert_eq!(state.cursor, 2);
        assert!(matches!(
            state.selected_node(),
            Some(TreeNode::AgentPane { ref view, .. }) if view.instance.0 == "ag-beta"
        ));
    }

    #[test]
    fn deletion_fallback() {
        let mut state = AppState::default();
        let session =
            make_inventory_session("sess", "sid-1", vec![("editor", 0, 10)], vec![(100, 0, 10)]);
        state.reconcile(vec![session]);
        state.set_agent_views(vec![make_agent_view(
            "sid-1",
            100,
            "ag-1",
            AgentStatus::Working,
        )]);

        // Select agent leaf at index 2
        state.select_index(2);
        assert_eq!(state.cursor, 2);

        // Delete agent view
        state.set_agent_views(vec![]);

        // Nodes: Session(0), Tab editor(1)
        // Agent was deleted -> falls back to parent Tab (index 1)
        assert_eq!(state.cursor, 1);
        assert!(matches!(state.selected_node(), Some(TreeNode::Tab { .. })));

        // Now delete the tab as well
        let empty_session = make_inventory_session("sess", "sid-1", vec![], vec![]);
        state.reconcile(vec![empty_session]);

        // Nodes: Session(0)
        // Tab was deleted -> falls back to parent Session (index 0)
        assert_eq!(state.cursor, 0);
        assert!(matches!(
            state.selected_node(),
            Some(TreeNode::Session { .. })
        ));
    }

    #[test]
    fn fold_leaf_and_nochild_legacy_behavior() {
        let mut state = AppState::default();
        let session = make_inventory_session(
            "sess",
            "sid-1",
            vec![("editor", 0, 10), ("empty-tab", 1, 20)],
            vec![(100, 0, 10)],
        );
        state.reconcile(vec![session]);
        state.set_agent_views(vec![make_agent_view(
            "sid-1",
            100,
            "ag-1",
            AgentStatus::Working,
        )]);

        // Nodes:
        // 0: Session sess
        // 1: Tab editor (has_children: true)
        // 2: Agent ag-1
        // 3: Tab empty-tab (has_children: false)

        // 1. Space on agent leaf -> folds parent tab and selects parent
        state.select_index(2);
        state.toggle_collapse();
        // Tab editor is now collapsed; agent leaf is hidden
        // Nodes: Session(0), Tab editor(1), Tab empty-tab(2)
        assert_eq!(state.nodes.len(), 3);
        assert_eq!(state.cursor, 1);
        assert!(matches!(
            state.nodes[1],
            TreeNode::Tab {
                is_collapsed: true,
                has_children: true,
                ..
            }
        ));

        // 2. Right/l on collapsed tab with children -> expands tab
        state.expand_selected();
        assert_eq!(state.nodes.len(), 4);
        assert!(matches!(
            state.nodes[1],
            TreeNode::Tab {
                is_collapsed: false,
                has_children: true,
                ..
            }
        ));

        // 3. Space on tab with children -> toggles tab only
        state.toggle_collapse();
        assert_eq!(state.nodes.len(), 3);
        assert_eq!(state.cursor, 1);
        // Expand it again
        state.toggle_collapse();
        assert_eq!(state.nodes.len(), 4);

        // 4. Space on tab without children (no agents) -> folds parent session (legacy behavior)
        state.select_index(3); // empty-tab
        state.toggle_collapse();
        // Session is now collapsed!
        assert_eq!(state.nodes.len(), 1);
        assert_eq!(state.cursor, 0);
        assert!(matches!(
            state.nodes[0],
            TreeNode::Session {
                is_collapsed: true,
                ..
            }
        ));

        // 5. Expand session
        state.expand_selected();
        assert_eq!(state.nodes.len(), 4);

        // 6. Left/h on agent leaf -> moves to parent tab
        state.select_index(2);
        state.collapse_selected();
        assert_eq!(state.cursor, 1);

        // 7. Left/h on expanded tab with children -> collapses tab
        state.collapse_selected();
        assert_eq!(state.nodes.len(), 3);
        assert_eq!(state.cursor, 1);

        // 8. Left/h on collapsed tab -> moves to parent session
        state.collapse_selected();
        assert_eq!(state.cursor, 0);
    }

    #[test]
    fn collapsed_summary_updates() {
        let mut state = AppState::default();
        let session = make_inventory_session(
            "sess",
            "sid-1",
            vec![("editor", 0, 10)],
            vec![(100, 0, 10), (101, 0, 10)],
        );
        state.reconcile(vec![session]);
        state.set_agent_views(vec![
            make_agent_view("sid-1", 100, "ag-1", AgentStatus::Working),
            make_agent_view("sid-1", 101, "ag-2", AgentStatus::NeedsInput),
        ]);

        // Check summaries when expanded
        let tab_summary = state.nodes[1].summary();
        assert_eq!(tab_summary.total, 2);
        assert_eq!(tab_summary.working, 1);
        assert_eq!(tab_summary.needs_input, 1);

        let session_summary = state.nodes[0].summary();
        assert_eq!(session_summary.total, 2);
        assert_eq!(session_summary.working, 1);
        assert_eq!(session_summary.needs_input, 1);

        // Collapse tab
        state.select_index(1);
        state.toggle_collapse();
        assert!(matches!(
            state.nodes[1],
            TreeNode::Tab {
                is_collapsed: true,
                ..
            }
        ));

        // Summaries are preserved even when folded!
        let tab_summary_folded = state.nodes[1].summary();
        assert_eq!(tab_summary_folded.total, 2);
        assert_eq!(tab_summary_folded.working, 1);
        assert_eq!(tab_summary_folded.needs_input, 1);

        // Update an agent status while tab is collapsed
        state.set_agent_views(vec![
            make_agent_view("sid-1", 100, "ag-1", AgentStatus::Done),
            make_agent_view("sid-1", 101, "ag-2", AgentStatus::Done),
        ]);

        let updated_tab_summary = state.nodes[1].summary();
        assert_eq!(updated_tab_summary.total, 2);
        assert_eq!(updated_tab_summary.done, 2);
        assert_eq!(updated_tab_summary.working, 0);

        let updated_session_summary = state.nodes[0].summary();
        assert_eq!(updated_session_summary.total, 2);
        assert_eq!(updated_session_summary.done, 2);
    }

    #[test]
    fn exited_session_stays_folded() {
        let mut state = AppState::default();
        let mut session =
            make_inventory_session("sess", "sid-1", vec![("editor", 0, 10)], vec![(100, 0, 10)]);
        session.needs_resurrection = true;
        state.reconcile(vec![session]);
        state.set_agent_views(vec![make_agent_view(
            "sid-1",
            100,
            "ag-1",
            AgentStatus::Working,
        )]);

        // Exited session stays folded, agent does not resurrect it
        assert_eq!(state.nodes.len(), 1);
        assert!(matches!(
            state.nodes[0],
            TreeNode::Session {
                needs_resurrection: true,
                is_collapsed: true,
                ..
            }
        ));

        state.toggle_collapse();
        assert_eq!(state.nodes.len(), 1);

        state.expand_selected();
        assert_eq!(state.nodes.len(), 1);
    }

    #[test]
    fn persistable_tree_state() {
        let mut state = AppState::default();
        let session =
            make_inventory_session("sess", "sid-1", vec![("editor", 0, 10)], vec![(100, 0, 10)]);
        state.reconcile(vec![session]);
        state.set_agent_views(vec![make_agent_view(
            "sid-1",
            100,
            "ag-1",
            AgentStatus::Working,
        )]);

        state.select_index(2); // Agent leaf
        let saved = state.save_persistable_state();
        let serialized = serde_json::to_string(&saved).unwrap();

        let deserialized: TreePersistableState = serde_json::from_str(&serialized).unwrap();
        let mut new_state = AppState::default();
        let session =
            make_inventory_session("sess", "sid-1", vec![("editor", 0, 10)], vec![(100, 0, 10)]);
        new_state.reconcile(vec![session]);
        new_state.set_agent_views(vec![make_agent_view(
            "sid-1",
            100,
            "ag-1",
            AgentStatus::Working,
        )]);

        new_state.restore_tree_state(deserialized);
        assert_eq!(new_state.cursor, 2);
    }
}
