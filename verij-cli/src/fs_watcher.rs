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
use std::collections::{HashMap, HashSet};
use std::time::Duration;
use tokio::sync::mpsc;
use verij_types::{SessionSnapshot, VERIJ_STATES_DIR};

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

/// Reads and aggregates all session JSON files currently present in the states directory.
///
/// Prunes stale state files for sessions that no longer exist in Zellij, and filters
/// out registered host sessions.
pub fn read_all_states(hosts: &HashSet<String>, unknown: &mut HashMap<String, bool>) -> Vec<SessionSnapshot> {
    let dir = resolve_states_dir();
    let Ok(entries) = std::fs::read_dir(&dir) else {
        return Vec::new();
    };

    let live_sessions = crate::session::list_sessions().ok();

    let mut sessions = Vec::new();
    for entry in entries.flatten() {
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) == Some("json") {
            if let Ok(content) = std::fs::read_to_string(&path) {
                if let Ok(snapshot) = serde_json::from_str::<SessionSnapshot>(&content) {
                    // Filter and remove any registered host session states.
                    if hosts.contains(&snapshot.name) || *unknown.entry(snapshot.name.clone())
                        .or_insert_with(|| crate::session::is_verij_host(&snapshot.name).unwrap_or(false)) {
                        let _ = std::fs::remove_file(&path);
                        continue;
                    }

                    // If session is no longer active in Zellij, prune the orphaned file
                    if let Some(ref live) = live_sessions {
                        if !live.contains(&snapshot.name) {
                            let _ = std::fs::remove_file(&path);
                            continue;
                        }
                    }

                    sessions.push(snapshot);
                }
            }
        }
    }

    sessions.sort_by(|a, b| a.name.cmp(&b.name));
    sessions
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
