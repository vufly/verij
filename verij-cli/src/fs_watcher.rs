/// verij-cli/src/fs_watcher.rs
///
/// Filesystem watcher for `/tmp/verij/states/`.
///
/// Watches directory for session state JSON files written by distributed agents
/// (`verij-plugin`). Aggregates individual session snapshots into a unified
/// `Vec<SessionSnapshot>` and forwards them to the TUI event loop.
use anyhow::{Context, Result};
use notify::{Config, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::path::{Path, PathBuf};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::time::Duration;
use tokio::sync::mpsc;
use verij_types::{SessionSnapshot, VERIJ_STATES_DIR};

use crate::session::SessionStatus;

/// Resolves and prepares the session states directory.
///
/// In Zellij, WASI sandboxing maps `/tmp` to `/tmp/zellij-<uid>/`.
/// This helper ensures `/tmp/zellij-<uid>/verij/states/` exists and creates
/// `/tmp/verij -> /tmp/zellij-<uid>/verij` symlink so paths resolve identically
/// across both the host OS and the WASM plugin environment.
pub fn resolve_states_dir() -> PathBuf {
    #[cfg(unix)]
    {
        use std::os::unix::fs::MetadataExt;
        if let Ok(meta) = std::fs::metadata("/proc/self") {
            let uid = meta.uid();
            let zellij_verij_dir = PathBuf::from(format!("/tmp/zellij-{uid}/verij"));
            let states_dir = zellij_verij_dir.join("states");
            let _ = std::fs::create_dir_all(&states_dir);

            let host_tmp = Path::new("/tmp/verij");
            if !host_tmp.exists() {
                let _ = std::os::unix::fs::symlink(&zellij_verij_dir, host_tmp);
            }
            return states_dir;
        }
    }

    let fallback = PathBuf::from(VERIJ_STATES_DIR);
    let _ = std::fs::create_dir_all(&fallback);
    fallback
}

/// Combines plugin snapshots with Zellij's session inventory.
///
/// Prunes stale state files for sessions that no longer exist in Zellij, and filters
/// out host sessions. Sessions without a plugin snapshot remain visible after reboot.
pub fn read_all_states(hosts: &HashSet<String>, unknown: &mut HashMap<String, bool>) -> Vec<SessionSnapshot> {
    let dir = resolve_states_dir();
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };

    let session_statuses = crate::session::session_statuses().ok();

    let mut sessions = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) == Some("json") {
            if let Ok(content) = std::fs::read_to_string(&path) {
                if let Ok(mut snapshot) = serde_json::from_str::<SessionSnapshot>(&content) {
                    // Filter and remove any registered host session states.
                    if hosts.contains(&snapshot.name) || *unknown.entry(snapshot.name.clone())
                        .or_insert_with(|| crate::session::is_verij_host(&snapshot.name).unwrap_or(false)) {
                        let _ = std::fs::remove_file(&path);
                        continue;
                    }

                    // Keep EXITED sessions so the sidebar can resurrect them, but remove
                    // snapshots for sessions Zellij no longer knows about.
                    if let Some(ref statuses) = session_statuses {
                        let Some(status) = statuses.get(&snapshot.name) else {
                            let _ = std::fs::remove_file(&path);
                            continue;
                        };
                        snapshot.needs_resurrection = matches!(status, crate::session::SessionStatus::Exited);
                    }

                    sessions.push(snapshot);
                }
            }
        }
    }

    if let Some(ref statuses) = session_statuses {
        add_missing_sessions(&mut sessions, statuses, |name| {
            hosts.contains(name) || *unknown.entry(name.to_string())
                .or_insert_with(|| crate::session::is_verij_host(name).unwrap_or(false))
        });
    }

    sessions.sort_by(|a, b| a.name.cmp(&b.name));
    sessions
}

/// Runtime JSON lives in /tmp and can disappear on reboot. Zellij's durable
/// inventory still knows exited sessions, even though their plugins cannot export
/// fresh snapshots until resurrection. Keep these rows attachable without inventing
/// tab or pane details; a plugin snapshot supplies those once the session runs.
fn add_missing_sessions(
    sessions: &mut Vec<SessionSnapshot>,
    statuses: &BTreeMap<String, SessionStatus>,
    mut is_host: impl FnMut(&str) -> bool,
) {
    let known: HashSet<_> = sessions.iter().map(|session| session.name.clone()).collect();
    for (name, status) in statuses {
        if known.contains(name) || is_host(name) || matches!(status, SessionStatus::Missing) {
            continue;
        }
        sessions.push(SessionSnapshot {
            name: name.clone(),
            is_current: false,
            tabs: Vec::new(),
            active_pane: None,
            connected_clients: None,
            needs_resurrection: matches!(status, SessionStatus::Exited),
        });
    }
}

/// Spawns the filesystem watcher background task targeting the states directory.
///
/// On startup, on file changes (create, modify, remove), and via periodic poll,
/// aggregates all valid session snapshots, auto-prunes dead files, and forwards updates.
///
pub fn spawn_fs_watcher(
    tx: mpsc::Sender<Vec<SessionSnapshot>>,
) -> Result<tokio::task::JoinHandle<()>> {
    let states_dir = resolve_states_dir();
    std::fs::create_dir_all(&states_dir)
        .with_context(|| format!("Failed to create states directory: {}", states_dir.display()))?;

    // Send initial state immediately (also prunes stale files on startup)
    let mut registry = crate::registry::Cache::new()?;
    let mut unknown = HashMap::new();
    let initial_states = read_all_states(&registry.names, &mut unknown);
    let _ = tx.try_send(initial_states);

    let handle = tokio::task::spawn_blocking(move || {
        let (fs_tx, fs_rx) = std::sync::mpsc::channel();

        let watcher_result = RecommendedWatcher::new(
            move |res: Result<Event, notify::Error>| {
                if let Ok(event) = res {
                    let _ = fs_tx.send(event);
                }
            },
            Config::default(),
        );

        let mut watcher = match watcher_result {
            Ok(w) => w,
            Err(e) => {
                eprintln!("[verij-cli] Failed to initialize file watcher: {e}");
                return;
            }
        };

        if let Err(e) = watcher.watch(&states_dir, RecursiveMode::NonRecursive) {
            eprintln!(
                "[verij-cli] Failed to watch directory {}: {e}",
                states_dir.display()
            );
            return;
        }

        let mut poll_ticks = 0;
        while !tx.is_closed() {
            // Wait for filesystem event with a brief timeout
            match fs_rx.recv_timeout(Duration::from_millis(200)) {
                Ok(event) => {
                    match event.kind {
                        EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_) => {
                            // Debounce: drain any rapid consecutive events within 30ms
                            while fs_rx.recv_timeout(Duration::from_millis(30)).is_ok() {}

                            let _ = registry.refresh();
                            let snapshots = read_all_states(&registry.names, &mut unknown);
                            if tx.blocking_send(snapshots).is_err() {
                                break;
                            }
                        }
                        _ => {}
                    }
                }
                Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                    // Periodic poll (~2s) to prune dead sessions and catch external changes
                    poll_ticks += 1;
                    if poll_ticks >= 10 {
                        poll_ticks = 0;
                        let _ = registry.refresh();
                        let snapshots = read_all_states(&registry.names, &mut unknown);
                        if tx.blocking_send(snapshots).is_err() {
                            break;
                        }
                    }
                }
                Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                    break;
                }
            }
        }
    });

    Ok(handle)
}

#[cfg(test)]
mod tests {
    use super::*;
    use verij_types::TabSnapshot;

    #[test]
    fn reboot_without_runtime_snapshots_keeps_live_and_exited_inner_sessions() {
        let statuses = BTreeMap::from([
            ("exited-inner".into(), SessionStatus::Exited),
            ("exited-host".into(), SessionStatus::Exited),
            ("live-inner".into(), SessionStatus::Live),
            ("live-host".into(), SessionStatus::Live),
        ]);
        let mut sessions = Vec::new();

        add_missing_sessions(&mut sessions, &statuses, |name| name.ends_with("-host"));

        assert_eq!(sessions.len(), 2);
        assert_eq!(sessions[0].name, "exited-inner");
        assert!(sessions[0].needs_resurrection);
        assert_eq!(sessions[1].name, "live-inner");
        assert!(!sessions[1].needs_resurrection);
        assert!(sessions.iter().all(|session| session.tabs.is_empty()));
    }

    #[test]
    fn inventory_fallback_preserves_plugin_details_without_duplicate_rows() {
        let snapshot = SessionSnapshot {
            name: "inner".into(),
            is_current: true,
            tabs: vec![TabSnapshot {
                name: "editor".into(),
                position: 0,
                is_active: true,
            }],
            active_pane: Some("shell".into()),
            connected_clients: Some(1),
            needs_resurrection: false,
        };
        let statuses = BTreeMap::from([
            ("inner".into(), SessionStatus::Live),
            ("old".into(), SessionStatus::Exited),
        ]);
        let mut sessions = vec![snapshot.clone()];

        add_missing_sessions(&mut sessions, &statuses, |_| false);
        add_missing_sessions(&mut sessions, &statuses, |_| false);

        assert_eq!(sessions.len(), 2);
        assert_eq!(sessions[0], snapshot);
        assert_eq!(sessions[1].name, "old");
        assert!(sessions[1].needs_resurrection);
    }
}
