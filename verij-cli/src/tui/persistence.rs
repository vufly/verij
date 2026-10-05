//! Stable host-local tree state. Display labels never become identity keys.
use super::state::AppState;
use super::tree_model::NodeKey;
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::path::PathBuf;
use verij_types::identity::valid_record_key;

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct SavedTree {
    pub schema_version: u32,
    pub selected: Option<NodeKey>,
    pub collapsed: Vec<NodeKey>,
    pub collapsed_tabs: Vec<NodeKey>,
    pub scroll_offset: usize,
}

pub fn path(host: &str) -> Result<PathBuf> {
    if !valid_record_key(host) {
        bail!("invalid tree host key");
    }
    Ok(crate::config::state_dir()
        .context("tree state directory unavailable")?
        .join("agent-monitoring/tree")
        .join(format!("{host}.json")))
}

pub fn load(host: &str) -> Result<Option<SavedTree>> {
    let path = path(host)?;
    load_path(&path)
}

fn load_path(path: &std::path::Path) -> Result<Option<SavedTree>> {
    let metadata = match std::fs::symlink_metadata(&path) {
        Ok(m) => m,
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => return Ok(None),
        Err(e) => return Err(e.into()),
    };
    if !metadata.file_type().is_file() {
        bail!("tree state is not a regular file");
    }
    if metadata.len() > 256 * 1024 {
        bail!("tree state exceeds limit");
    }
    use std::io::Read;
    let file = std::fs::File::open(&path)?;
    let mut bytes = Vec::with_capacity(metadata.len() as usize);
    file.take(256 * 1024 + 1).read_to_end(&mut bytes)?;
    if bytes.len() > 256 * 1024 {
        bail!("tree state exceeds limit");
    }
    let saved: SavedTree = serde_json::from_slice(&bytes)?;
    if saved.schema_version != 1 {
        bail!("unsupported tree state version");
    }
    Ok(Some(saved))
}

pub fn capture(state: &AppState) -> SavedTree {
    let snapshot = state.save_persistable_state();
    let mut collapsed: Vec<_> = snapshot.collapsed_keys.into_iter().collect();
    let mut collapsed_tabs: Vec<_> = snapshot.collapsed_tabs.into_iter().collect();
    collapsed.sort();
    collapsed_tabs.sort();
    SavedTree {
        schema_version: 1,
        selected: state.selected_node().map(|node| node.meta().key.clone()),
        collapsed,
        collapsed_tabs,
        scroll_offset: state.list_state.offset(),
    }
}

pub fn restore_folds(state: &mut AppState, saved: &SavedTree) {
    state.restore_tree_state(super::tree_model::TreePersistableState {
        collapsed_keys: saved.collapsed.iter().cloned().collect(),
        collapsed_tabs: saved.collapsed_tabs.iter().cloned().collect(),
        selected_key: None,
    });
}

pub fn restore_selection(state: &mut AppState, saved: &SavedTree) {
    if let Some(key) = saved.selected.as_ref() {
        if let Some(index) = state
            .nodes
            .iter()
            .position(|node| &node.meta().key == key)
            .or_else(|| state.visible_parent_for(key))
        {
            state.select_index(index);
        }
    }
    let max_offset = state.nodes.len().saturating_sub(1);
    *state.list_state.offset_mut() = saved.scroll_offset.min(max_offset);
    state.sync_list_state();
}

pub fn save(host: &str, saved: &SavedTree) -> Result<()> {
    let path = path(host)?;
    save_path(&path, saved)
}

fn save_path(path: &std::path::Path, saved: &SavedTree) -> Result<()> {
    crate::process::ensure_private_directory(path.parent().unwrap())?;
    crate::agent_store::atomic_write_json(&path, saved)
}

#[cfg(test)]
mod tests {
    use super::super::tree_model::{SessionKey, TabKey};
    use super::*;
    use verij_types::identity::{AgentInstanceId, PaneKey, SessionInstanceId, TerminalPaneId};
    use verij_types::inventory::{PaneSnapshot, SessionInventory, TabMetadata};
    use verij_types::{SessionSnapshot, TabSnapshot};

    fn isolate_persistence_env() -> (tempfile::TempDir, PathBuf) {
        let temp = tempfile::tempdir().unwrap();
        let tree_dir = temp.path().join("verij/agent-monitoring/tree");
        crate::process::ensure_private_directory(&tree_dir).unwrap();
        (temp, tree_dir)
    }

    fn make_test_session(
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

    #[test]
    fn test_load_file_limit_checked_before_unbounded_read() {
        let (_guard, tree_dir) = isolate_persistence_env();
        let target = tree_dir.join("oversized.json");
        // Write 256 KiB + 16 bytes
        let oversized = vec![b'a'; 256 * 1024 + 16];
        std::fs::write(&target, &oversized).unwrap();

        let result = load_path(&target);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("tree state exceeds limit"));
    }

    #[test]
    fn test_load_missing_file_returns_none() {
        let (_guard, tree_dir) = isolate_persistence_env();
        let result = load_path(&tree_dir.join("nonexistent.json")).unwrap();
        assert_eq!(result, None);
    }

    #[test]
    fn test_load_invalid_host_key_bails() {
        assert!(load("../bad_path").is_err());
        assert!(load("").is_err());
        assert!(path("bad/name").is_err());
    }

    #[test]
    fn test_load_corrupt_json_bails() {
        let (_guard, tree_dir) = isolate_persistence_env();
        let target = tree_dir.join("corrupt.json");
        std::fs::write(&target, b"{ not json }").unwrap();

        let result = load_path(&target);
        assert!(result.is_err());
    }

    #[test]
    fn test_load_schema_version_mismatch_bails() {
        let (_guard, tree_dir) = isolate_persistence_env();
        let target = tree_dir.join("version2.json");
        let content = serde_json::json!({
            "schema_version": 2,
            "selected": null,
            "collapsed": [],
            "collapsed_tabs": [],
            "scroll_offset": 0
        });
        std::fs::write(&target, serde_json::to_vec(&content).unwrap()).unwrap();

        let result = load_path(&target);
        assert!(result.is_err());
        assert!(result
            .unwrap_err()
            .to_string()
            .contains("unsupported tree state version"));
    }

    #[test]
    fn test_save_atomic_and_load_roundtrip() {
        let (_guard, tree_dir) = isolate_persistence_env();
        let host = "roundtrip-host";
        let saved = SavedTree {
            schema_version: 1,
            selected: Some(NodeKey::Tab {
                session: SessionKey::Verified(SessionInstanceId("sid-42".into())),
                tab: TabKey::Stable(7),
            }),
            collapsed: vec![
                NodeKey::Session(SessionKey::Verified(SessionInstanceId("sid-1".into()))),
                NodeKey::Session(SessionKey::Verified(SessionInstanceId("sid-2".into()))),
            ],
            collapsed_tabs: vec![NodeKey::Tab {
                session: SessionKey::Verified(SessionInstanceId("sid-1".into())),
                tab: TabKey::Position(0),
            }],
            scroll_offset: 12,
        };

        let file_path = tree_dir.join(format!("{host}.json"));
        save_path(&file_path, &saved).unwrap();

        let loaded = load_path(&file_path)
            .unwrap()
            .expect("saved file must be present");
        assert_eq!(loaded, saved);

        // Verify unix file permissions 0600 on saved file
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let mode = std::fs::metadata(&file_path).unwrap().permissions().mode() & 0o777;
            assert_eq!(mode, 0o600);
        }
    }

    #[test]
    #[cfg(unix)]
    fn non_regular_state_file_is_rejected_without_blocking() {
        let (_guard, tree_dir) = isolate_persistence_env();
        let target = tree_dir.join("state.json");
        let name = std::ffi::CString::new(target.as_os_str().as_encoded_bytes()).unwrap();
        assert_eq!(unsafe { libc::mkfifo(name.as_ptr(), 0o600) }, 0);
        assert!(load_path(&target)
            .unwrap_err()
            .to_string()
            .contains("regular file"));
    }

    #[test]
    fn test_restore_selection_fallback_to_tab_parent() {
        let mut state = AppState::default();
        let session = make_test_session(
            "my-session",
            "sid-100",
            vec![("editor", 0, 10)],
            vec![(42, 0, 10)],
        );
        state.reconcile(vec![session]);

        // Selection references an agent on pane 42 that is no longer present in agent_views
        let saved = SavedTree {
            schema_version: 1,
            selected: Some(NodeKey::Agent {
                pane: PaneKey {
                    session: SessionInstanceId("sid-100".into()),
                    terminal: TerminalPaneId(42),
                },
                instance: AgentInstanceId("old-agent".into()),
            }),
            collapsed: vec![],
            collapsed_tabs: vec![],
            scroll_offset: 0,
        };

        restore_selection(&mut state, &saved);
        // Falls back to parent tab row (index 1: 0 is Session, 1 is Tab)
        assert_eq!(state.cursor, 1);
        let selected = state.selected_node().unwrap();
        assert!(
            matches!(selected, super::super::tree_model::TreeNode::Tab { name, .. } if name == "editor")
        );
    }

    #[test]
    fn test_restore_selection_fallback_to_session_parent() {
        let mut state = AppState::default();
        let session = make_test_session(
            "my-session",
            "sid-100",
            vec![("editor", 0, 10)],
            vec![(42, 0, 10)],
        );
        state.reconcile(vec![session]);

        // Collapse the session so the tab row is hidden
        let session_key =
            NodeKey::Session(SessionKey::Verified(SessionInstanceId("sid-100".into())));
        let saved = SavedTree {
            schema_version: 1,
            selected: Some(NodeKey::Agent {
                pane: PaneKey {
                    session: SessionInstanceId("sid-100".into()),
                    terminal: TerminalPaneId(42),
                },
                instance: AgentInstanceId("old-agent".into()),
            }),
            collapsed: vec![session_key],
            collapsed_tabs: vec![],
            scroll_offset: 0,
        };

        restore_folds(&mut state, &saved);
        // With session collapsed, only session row exists
        assert_eq!(state.nodes.len(), 1);

        restore_selection(&mut state, &saved);
        // Falls back to parent session row (index 0)
        assert_eq!(state.cursor, 0);
        let selected = state.selected_node().unwrap();
        assert!(
            matches!(selected, super::super::tree_model::TreeNode::Session { name, .. } if name == "my-session")
        );
    }

    #[test]
    fn test_restore_selection_clamps_scroll_offset() {
        let mut state = AppState::default();
        let session = make_test_session("my-session", "sid-100", vec![("editor", 0, 10)], vec![]);
        state.reconcile(vec![session]);
        assert_eq!(state.nodes.len(), 2); // Session + Tab

        let saved = SavedTree {
            schema_version: 1,
            selected: None,
            collapsed: vec![],
            collapsed_tabs: vec![],
            scroll_offset: 5000,
        };

        restore_selection(&mut state, &saved);
        // Clamped to nodes.len() - 1 = 1
        assert_eq!(state.list_state.offset(), 1);
    }

    #[test]
    fn test_restore_selection_missing_session_graceful() {
        let mut state = AppState::default();
        let session = make_test_session("my-session", "sid-100", vec![("editor", 0, 10)], vec![]);
        state.reconcile(vec![session]);

        let saved = SavedTree {
            schema_version: 1,
            selected: Some(NodeKey::Session(SessionKey::Verified(SessionInstanceId(
                "sid-nonexistent".into(),
            )))),
            collapsed: vec![],
            collapsed_tabs: vec![],
            scroll_offset: 0,
        };

        // Must not panic, cursor remains within bounds
        restore_selection(&mut state, &saved);
        assert!(state.cursor < state.nodes.len());
    }

    #[test]
    fn test_capture_and_restore_folds_roundtrip() {
        let mut state = AppState::default();
        let session = make_test_session("sess1", "sid-1", vec![("t1", 0, 1), ("t2", 1, 2)], vec![]);
        state.reconcile(vec![session]);

        let tab_key = NodeKey::Tab {
            session: SessionKey::Verified(SessionInstanceId("sid-1".into())),
            tab: TabKey::Stable(1),
        };
        state.collapsed_tabs.insert(tab_key.clone());
        state.collapsed_keys.insert(tab_key.clone());
        state.rebuild_nodes();

        let captured = capture(&state);
        assert!(captured.collapsed.contains(&tab_key));
        assert!(captured.collapsed_tabs.contains(&tab_key));

        let mut fresh_state = AppState::default();
        let session = make_test_session("sess1", "sid-1", vec![("t1", 0, 1), ("t2", 1, 2)], vec![]);
        fresh_state.reconcile(vec![session]);

        restore_folds(&mut fresh_state, &captured);
        assert!(fresh_state.collapsed_keys.contains(&tab_key));
        assert!(fresh_state.collapsed_tabs.contains(&tab_key));
    }
}
