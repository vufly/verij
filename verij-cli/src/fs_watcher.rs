/// verij-cli/src/fs_watcher.rs
///
/// Filesystem watcher for `/tmp/verij/states/`.
///
/// Watches directory for session state JSON files written by distributed agents
/// (`verij-plugin`). Aggregates individual session snapshots into a unified
/// `Vec<SessionSnapshot>` and forwards them to the TUI event loop.
use anyhow::{Context, Result};
use notify::{Config, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
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
    if let Some(directory) = std::env::var_os("VERIJ_STATES_DIR")
        .map(PathBuf::from)
        .filter(|path| path.is_absolute())
    {
        let _ = std::fs::create_dir_all(&directory);
        return directory;
    }
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

/// Concurrently drains bounded stdout and stderr buffers using background threads to avoid
/// OS pipe deadlocks on large outputs. Kills child process on timeout or prompt stop.
fn run_command_bounded(
    mut cmd: std::process::Command,
    timeout: Duration,
    max_bytes: u64,
    stop: Option<&AtomicBool>,
) -> Option<(Vec<u8>, Vec<u8>)> {
    cmd.stdin(std::process::Stdio::null());
    cmd.stdout(std::process::Stdio::piped());
    cmd.stderr(std::process::Stdio::piped());

    let mut child = cmd.spawn().ok()?;
    let stdout_pipe = child.stdout.take()?;
    let stderr_pipe = child.stderr.take()?;

    let stdout_handle = std::thread::spawn(move || -> std::io::Result<Vec<u8>> {
        use std::io::Read;
        let mut reader = stdout_pipe.take(max_bytes);
        let mut buf = Vec::new();
        reader.read_to_end(&mut buf)?;
        Ok(buf)
    });

    let stderr_handle = std::thread::spawn(move || -> std::io::Result<Vec<u8>> {
        use std::io::Read;
        let mut reader = stderr_pipe.take(max_bytes);
        let mut buf = Vec::new();
        reader.read_to_end(&mut buf)?;
        Ok(buf)
    });

    let start = Instant::now();
    let mut success = false;
    while start.elapsed() < timeout {
        if stop.is_some_and(|s| s.load(Ordering::Relaxed)) {
            let _ = child.kill();
            let _ = child.wait();
            let _ = stdout_handle.join();
            let _ = stderr_handle.join();
            return None;
        }
        match child.try_wait() {
            Ok(Some(status)) => {
                success = status.success();
                break;
            }
            Ok(None) => std::thread::sleep(Duration::from_millis(5)),
            Err(_) => break,
        }
    }

    if !success {
        let _ = child.kill();
        let _ = child.wait();
        let _ = stdout_handle.join();
        let _ = stderr_handle.join();
        return None;
    }

    let stdout_bytes = stdout_handle.join().ok()?.ok()?;
    let stderr_bytes = stderr_handle.join().ok()?.ok()?;
    Some((stdout_bytes, stderr_bytes))
}

/// Queries Zellij session statuses bounded by strict timeout using concurrent pipe draining.
/// Kills child process on timeout to prevent hanging UI shutdown.
#[allow(dead_code)]
pub fn query_session_statuses_bounded(
    timeout: Duration,
) -> Option<BTreeMap<String, SessionStatus>> {
    query_session_statuses_bounded_cancelable(timeout, None)
}

pub fn query_session_statuses_bounded_cancelable(
    timeout: Duration,
    stop: Option<&AtomicBool>,
) -> Option<BTreeMap<String, SessionStatus>> {
    let zellij_bin = std::env::var("ZELLIJ_BIN").unwrap_or_else(|_| "zellij".to_string());
    let mut cmd = std::process::Command::new(zellij_bin);
    if let Some(config) = std::env::var_os("ZELLIJ_CONFIG_FILE") {
        cmd.arg("--config").arg(config);
    }
    cmd.args(["list-sessions", "-n"]);

    const MAX_STATUS_BYTES: u64 = 1024 * 1024; // 1MB bounded output buffer
    let (stdout_bytes, stderr_bytes) = run_command_bounded(cmd, timeout, MAX_STATUS_BYTES, stop)?;

    let stdout_str = String::from_utf8_lossy(&stdout_bytes);
    let stderr_str = String::from_utf8_lossy(&stderr_bytes);
    if stdout_str.trim() == "No active zellij sessions found."
        || stderr_str.trim() == "No active zellij sessions found."
    {
        return Some(BTreeMap::new());
    }

    let statuses = stdout_str
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            let name = line.split_whitespace().next()?;
            let status = if line.contains("(EXITED") {
                SessionStatus::Exited
            } else {
                SessionStatus::Live
            };
            Some((name.to_string(), status))
        })
        .collect();

    Some(statuses)
}

fn resurrection_layout_is_host(name: &str) -> bool {
    let cache_home = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")));
    if let Some(cache) = cache_home {
        let layout_path = cache
            .join("zellij/contract_version_1/session_info")
            .join(name)
            .join("session-layout.kdl");
        if layout_path.exists() {
            return crate::registry::is_host_layout(&layout_path);
        }
    }
    false
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostClassification {
    Host,
    NotHost,
    Unknown,
}

pub fn classify_host_session_bounded(name: &str, timeout: Duration) -> HostClassification {
    if resurrection_layout_is_host(name) {
        return HostClassification::Host;
    }

    match crate::navigation::run_zellij_action(name, &["list-panes", "--all", "--json"], timeout) {
        Ok(stdout_str) => {
            if let Ok(panes) = serde_json::from_str::<Vec<serde_json::Value>>(&stdout_str) {
                if panes.iter().any(|pane| {
                    pane.get("pane_command")
                        .or_else(|| pane.get("terminal_command"))
                        .and_then(|v| v.as_str())
                        .is_some_and(|cmd| cmd.contains("verij ui"))
                }) {
                    HostClassification::Host
                } else {
                    HostClassification::NotHost
                }
            } else {
                HostClassification::Unknown
            }
        }
        Err(_) => {
            if resurrection_layout_is_host(name) {
                HostClassification::Host
            } else {
                HostClassification::Unknown
            }
        }
    }
}

#[allow(dead_code)]
pub fn is_host_session_bounded(name: &str, timeout: Duration) -> bool {
    classify_host_session_bounded(name, timeout) == HostClassification::Host
}

#[cfg(test)]
pub fn check_is_host(
    name: &str,
    hosts: &HashSet<String>,
    host_cache: &mut HashMap<String, HostClassification>,
) -> bool {
    check_is_host_cancelable(name, hosts, host_cache, None)
}

pub fn check_is_host_cancelable(
    name: &str,
    hosts: &HashSet<String>,
    host_cache: &mut HashMap<String, HostClassification>,
    stop: Option<&AtomicBool>,
) -> bool {
    if hosts.contains(name) {
        return true;
    }
    match host_cache.get(name) {
        Some(HostClassification::Host) => true,
        Some(HostClassification::NotHost) => false,
        Some(HostClassification::Unknown) | None => {
            if stop.is_some_and(|s| s.load(Ordering::Relaxed)) {
                return false;
            }
            let outcome = classify_host_session_bounded(name, Duration::from_millis(300));
            match outcome {
                HostClassification::Host => {
                    host_cache.insert(name.to_string(), HostClassification::Host);
                    true
                }
                HostClassification::NotHost => {
                    host_cache.insert(name.to_string(), HostClassification::NotHost);
                    false
                }
                HostClassification::Unknown => false,
            }
        }
    }
}

#[cfg(test)]
pub fn read_all_states_with_statuses(
    dir: &Path,
    hosts: &HashSet<String>,
    host_cache: &mut HashMap<String, HostClassification>,
    session_statuses: Option<&BTreeMap<String, SessionStatus>>,
    can_prune: bool,
) -> Vec<SessionSnapshot> {
    read_all_states_with_statuses_cancelable(
        dir,
        hosts,
        host_cache,
        session_statuses,
        can_prune,
        None,
    )
}

pub fn read_all_states_with_statuses_cancelable(
    dir: &Path,
    hosts: &HashSet<String>,
    host_cache: &mut HashMap<String, HostClassification>,
    session_statuses: Option<&BTreeMap<String, SessionStatus>>,
    can_prune: bool,
    stop: Option<&AtomicBool>,
) -> Vec<SessionSnapshot> {
    let Ok(entries) = std::fs::read_dir(dir) else {
        return Vec::new();
    };

    let mut sessions = Vec::new();
    for entry in entries.flatten() {
        if stop.is_some_and(|s| s.load(Ordering::Relaxed)) {
            return sessions;
        }
        let path = entry.path();
        if path.extension().and_then(|s| s.to_str()) == Some("json") {
            if let Ok(content) = std::fs::read_to_string(&path) {
                if let Ok(mut snapshot) = serde_json::from_str::<SessionSnapshot>(&content) {
                    // Filter and remove any registered host session states.
                    // Keep registered host filtered and never delete host file.
                    if check_is_host_cancelable(&snapshot.name, hosts, host_cache, stop) {
                        // Exporters own publication. Deleting a live host file
                        // here creates a remove/re-export feedback loop and
                        // can starve input, pipe control and watcher shutdown.
                        continue;
                    }

                    // Keep EXITED sessions so the sidebar can resurrect them, but remove
                    // snapshots for sessions Zellij no longer knows about.
                    if let Some(statuses) = session_statuses {
                        if let Some(status) = statuses.get(&snapshot.name) {
                            snapshot.needs_resurrection =
                                matches!(status, crate::session::SessionStatus::Exited);
                        } else if can_prune {
                            let _ = std::fs::remove_file(&path);
                            continue;
                        } else {
                            // File events or unconfirmed topology: unknown new session must not be deleted.
                            snapshot.needs_resurrection = false;
                        }
                    }
                    crate::inventory::hydrate(&mut snapshot);
                    sessions.push(snapshot);
                }
            }
        }
    }

    if let Some(statuses) = session_statuses {
        add_missing_sessions_cancelable(
            &mut sessions,
            statuses,
            |name| check_is_host_cancelable(name, hosts, host_cache, stop),
            stop,
        );
    }

    sessions.sort_by(|a, b| {
        a.name.cmp(&b.name).then_with(|| {
            let stamp =
                |s: &SessionSnapshot| s.inventory.as_ref().map(|i| i.exported_at_ms).unwrap_or(0);
            stamp(b).cmp(&stamp(a))
        })
    });
    sessions.dedup_by(|a, b| a.name == b.name);
    sessions
}

/// Runtime JSON lives in /tmp and can disappear on reboot. Zellij's durable
/// inventory still knows exited sessions, even though their plugins cannot export
/// fresh snapshots until resurrection. Keep these rows attachable without inventing
/// tab or pane details; a plugin snapshot supplies those once the session runs.
#[cfg(test)]
fn add_missing_sessions(
    sessions: &mut Vec<SessionSnapshot>,
    statuses: &BTreeMap<String, SessionStatus>,
    is_host: impl FnMut(&str) -> bool,
) {
    add_missing_sessions_cancelable(sessions, statuses, is_host, None);
}

fn add_missing_sessions_cancelable(
    sessions: &mut Vec<SessionSnapshot>,
    statuses: &BTreeMap<String, SessionStatus>,
    mut is_host: impl FnMut(&str) -> bool,
    stop: Option<&AtomicBool>,
) {
    let known: HashSet<_> = sessions
        .iter()
        .map(|session| session.name.clone())
        .collect();
    for (name, status) in statuses {
        if stop.is_some_and(|s| s.load(Ordering::Relaxed)) {
            return;
        }
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
            inventory: None,
        });
    }
}

/// Owned handle to a running filesystem watcher thread.
#[allow(dead_code)]
pub struct WatcherHandle {
    thread: Option<std::thread::JoinHandle<()>>,
    stop: Arc<AtomicBool>,
}

#[allow(dead_code)]
impl WatcherHandle {
    pub fn stop(&self) {
        self.stop.store(true, Ordering::Release);
    }

    pub fn is_finished(&self) -> bool {
        self.thread.as_ref().map_or(true, |t| t.is_finished())
    }

    pub fn join(mut self) -> std::thread::Result<()> {
        self.stop();
        if let Some(t) = self.thread.take() {
            t.join()
        } else {
            Ok(())
        }
    }
}

impl Drop for WatcherHandle {
    fn drop(&mut self) {
        self.stop();
    }
}

/// Spawns the filesystem watcher background thread targeting the states directory.
///
/// On startup, on file changes (create, modify, remove), and via periodic poll,
/// aggregates all valid session snapshots, auto-prunes dead files, and forwards updates.
pub fn spawn_fs_watcher(tx: mpsc::Sender<Vec<SessionSnapshot>>) -> Result<WatcherHandle> {
    let states_dir = resolve_states_dir();
    spawn_fs_watcher_inner(states_dir, tx)
}

pub fn spawn_fs_watcher_inner(
    states_dir: PathBuf,
    tx: mpsc::Sender<Vec<SessionSnapshot>>,
) -> Result<WatcherHandle> {
    std::fs::create_dir_all(&states_dir).with_context(|| {
        format!(
            "Failed to create states directory: {}",
            states_dir.display()
        )
    })?;

    let stop = Arc::new(AtomicBool::new(false));
    let stop_thread = stop.clone();

    // Receiver closure must interrupt discovery even while the worker is in
    // a bounded subprocess query. A weak sender does not keep a failed worker's
    // channel connected; handle Drop stops both threads on UI exit.
    let stop_watchdog = stop.clone();
    let sender = tx.downgrade();
    std::thread::Builder::new()
        .name("verij-fs-stop".into())
        .spawn(move || {
            while !stop_watchdog.load(Ordering::Acquire) {
                match sender.upgrade() {
                    Some(sender) if !sender.is_closed() => {}
                    _ => {
                        stop_watchdog.store(true, Ordering::Release);
                        break;
                    }
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        })?;

    let thread = std::thread::Builder::new()
        .name("verij-fs-watcher".to_string())
        .spawn(move || {
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

            // Initial queries executed inside watcher thread (NOT calling UI thread - solves #4)
            if tx.is_closed() || stop_thread.load(Ordering::Relaxed) {
                return;
            }

            let mut registry = crate::registry::Cache::new().ok();
            let registry_names = registry
                .as_ref()
                .map(|r| r.names.clone())
                .unwrap_or_default();
            let mut host_cache = HashMap::new();

            const STATUSES_TTL: Duration = Duration::from_millis(2500);
            const TOPOLOGY_INTERVAL: Duration = Duration::from_secs(2);

            let mut cached_statuses: Option<(BTreeMap<String, SessionStatus>, Instant)> = None;
            if let Some(statuses) = query_session_statuses_bounded_cancelable(
                Duration::from_millis(500),
                Some(&stop_thread),
            ) {
                cached_statuses = Some((statuses, Instant::now()));
            }

            if tx.is_closed() || stop_thread.load(Ordering::Relaxed) {
                return;
            }

            let initial_can_prune = cached_statuses.is_some();
            let initial_states = read_all_states_with_statuses_cancelable(
                &states_dir,
                &registry_names,
                &mut host_cache,
                cached_statuses.as_ref().map(|(s, _)| s),
                initial_can_prune,
                Some(&stop_thread),
            );

            if tx.is_closed() || stop_thread.load(Ordering::Relaxed) {
                return;
            }

            let mut pending_send: Option<Vec<SessionSnapshot>> = None;
            match tx.try_send(initial_states) {
                Ok(()) => {}
                Err(mpsc::error::TrySendError::Closed(_)) => return,
                Err(mpsc::error::TrySendError::Full(s)) => {
                    pending_send = Some(s);
                }
            }

            let mut next_topology_refresh = Instant::now() + TOPOLOGY_INTERVAL;

            while !tx.is_closed() && !stop_thread.load(Ordering::Relaxed) {
                // If previous send was full, retry sending now (solves #7)
                if let Some(pending) = pending_send.take() {
                    match tx.try_send(pending) {
                        Ok(()) => {}
                        Err(mpsc::error::TrySendError::Closed(_)) => break,
                        Err(mpsc::error::TrySendError::Full(s)) => {
                            pending_send = Some(s);
                        }
                    }
                }

                // Check monotonic deadline for periodic topology refresh (solves #2)
                let now = Instant::now();
                if now >= next_topology_refresh {
                    next_topology_refresh = now + TOPOLOGY_INTERVAL;
                    if let Some(ref mut reg) = registry {
                        let _ = reg.refresh();
                    }
                    let names = registry
                        .as_ref()
                        .map(|r| &r.names)
                        .unwrap_or(&registry_names);

                    let fresh_query = query_session_statuses_bounded_cancelable(
                        Duration::from_millis(500),
                        Some(&stop_thread),
                    );
                    let can_prune = fresh_query.is_some();
                    if let Some(fresh) = fresh_query {
                        cached_statuses = Some((fresh, Instant::now()));
                    } else {
                        // Periodic query failed: invalidate cached statuses (solves #1)
                        cached_statuses = None;
                    }

                    let snapshots = read_all_states_with_statuses_cancelable(
                        &states_dir,
                        names,
                        &mut host_cache,
                        cached_statuses.as_ref().map(|(s, _)| s),
                        can_prune,
                        Some(&stop_thread),
                    );
                    match tx.try_send(snapshots) {
                        Ok(()) => {
                            pending_send = None;
                        }
                        Err(mpsc::error::TrySendError::Closed(_)) => break,
                        Err(mpsc::error::TrySendError::Full(s)) => {
                            pending_send = Some(s);
                        }
                    }

                    if tx.is_closed() || stop_thread.load(Ordering::Relaxed) {
                        break;
                    }
                }

                let time_until_refresh =
                    next_topology_refresh.saturating_duration_since(Instant::now());
                let slice_timeout = Duration::from_millis(50)
                    .min(time_until_refresh)
                    .max(Duration::from_millis(1));

                match fs_rx.recv_timeout(slice_timeout) {
                    Ok(event) => {
                        if !event.paths.iter().any(|path| {
                            path.extension().and_then(|suffix| suffix.to_str()) == Some("json")
                                && !path
                                    .file_name()
                                    .is_some_and(|name| name.to_string_lossy().starts_with('.'))
                        }) {
                            continue;
                        }
                        match event.kind {
                            EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_) => {
                                // Debounce consecutive events within 60ms
                                let debounce_deadline = Instant::now() + Duration::from_millis(60);
                                while !tx.is_closed()
                                    && !stop_thread.load(Ordering::Relaxed)
                                    && Instant::now() < debounce_deadline
                                {
                                    if fs_rx.recv_timeout(Duration::from_millis(10)).is_err() {
                                        break;
                                    }
                                }
                                if tx.is_closed() || stop_thread.load(Ordering::Relaxed) {
                                    break;
                                }

                                if let Some(ref mut reg) = registry {
                                    let _ = reg.refresh();
                                }
                                let names = registry
                                    .as_ref()
                                    .map(|r| &r.names)
                                    .unwrap_or(&registry_names);

                                let valid_statuses = cached_statuses
                                    .as_ref()
                                    .filter(|(_, fetched)| fetched.elapsed() < STATUSES_TTL)
                                    .map(|(s, _)| s);

                                // On file event, can_prune is ALWAYS false (solves #1)
                                let snapshots = read_all_states_with_statuses_cancelable(
                                    &states_dir,
                                    names,
                                    &mut host_cache,
                                    valid_statuses,
                                    false,
                                    Some(&stop_thread),
                                );
                                match tx.try_send(snapshots) {
                                    Ok(()) => {
                                        pending_send = None;
                                    }
                                    Err(mpsc::error::TrySendError::Closed(_)) => break,
                                    Err(mpsc::error::TrySendError::Full(s)) => {
                                        pending_send = Some(s);
                                    }
                                }
                            }
                            _ => {}
                        }
                    }
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                        // Loop repeats, checks pending_send, stop_thread, and next_topology_refresh
                    }
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                        break;
                    }
                }
            }
        })?;

    Ok(WatcherHandle {
        thread: Some(thread),
        stop,
    })
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
            inventory: None,
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

    #[test]
    fn watchdog_full_channel_closed_stops_promptly() {
        let dir = tempfile::tempdir().unwrap();
        let (tx, rx) = mpsc::channel(1);
        tx.try_send(vec![]).unwrap();

        let handle = spawn_fs_watcher_inner(dir.path().to_path_buf(), tx).unwrap();
        drop(rx);

        let start = Instant::now();
        while !handle.is_finished() && start.elapsed() < Duration::from_millis(500) {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            handle.is_finished(),
            "FS watcher failed to stop promptly on closed channel"
        );
        handle.join().unwrap();
    }

    #[test]
    fn continuous_notification_stops_promptly() {
        let dir = tempfile::tempdir().unwrap();
        let (tx, rx) = mpsc::channel(16);
        let handle = spawn_fs_watcher_inner(dir.path().to_path_buf(), tx).unwrap();

        let stop_spammer = Arc::new(AtomicBool::new(false));
        let stop_spammer_clone = stop_spammer.clone();
        let dir_path = dir.path().to_path_buf();
        let spammer = std::thread::spawn(move || {
            let mut i = 0;
            while !stop_spammer_clone.load(Ordering::Relaxed) {
                let _ = std::fs::write(dir_path.join(format!("session-{i}.json")), b"{}");
                i += 1;
                std::thread::sleep(Duration::from_millis(2));
            }
        });

        std::thread::sleep(Duration::from_millis(50));
        drop(rx);

        let start = Instant::now();
        while !handle.is_finished() && start.elapsed() < Duration::from_millis(500) {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            handle.is_finished(),
            "FS watcher did not stop promptly during continuous notifications"
        );

        stop_spammer.store(true, Ordering::Relaxed);
        spammer.join().unwrap();
        handle.join().unwrap();
    }

    #[test]
    fn periodic_quiet_stream_shutdown_prompt() {
        let dir = tempfile::tempdir().unwrap();
        let (tx, rx) = mpsc::channel(16);
        let handle = spawn_fs_watcher_inner(dir.path().to_path_buf(), tx).unwrap();

        std::thread::sleep(Duration::from_millis(60));
        let start = Instant::now();
        drop(rx);

        while !handle.is_finished() && start.elapsed() < Duration::from_millis(500) {
            std::thread::sleep(Duration::from_millis(10));
        }
        let elapsed = start.elapsed();
        assert!(
            handle.is_finished(),
            "Quiet stream FS watcher failed to terminate"
        );
        assert!(
            elapsed < Duration::from_millis(250),
            "Shutdown took too long: {elapsed:?}"
        );
        handle.join().unwrap();
    }

    #[test]
    fn new_session_snapshot_not_pruned_on_stale_or_missing_statuses() {
        let dir = tempfile::tempdir().unwrap();
        let session_file = dir.path().join("new-agent.json");
        let snapshot = SessionSnapshot {
            name: "new-agent".into(),
            is_current: false,
            tabs: Vec::new(),
            active_pane: None,
            connected_clients: None,
            needs_resurrection: false,
            inventory: None,
        };
        std::fs::write(&session_file, serde_json::to_vec(&snapshot).unwrap()).unwrap();

        let hosts = HashSet::new();
        let mut host_cache = HashMap::new();
        // Stale statuses do NOT include "new-agent", and can_prune is false (simulating file event)
        let stale_statuses = BTreeMap::from([("other-agent".into(), SessionStatus::Live)]);
        let sessions = read_all_states_with_statuses(
            dir.path(),
            &hosts,
            &mut host_cache,
            Some(&stale_statuses),
            false,
        );

        assert!(
            session_file.exists(),
            "New session export was incorrectly pruned on stale statuses"
        );
        assert!(sessions.iter().any(|s| s.name == "new-agent"));
    }

    #[test]
    fn pruning_deletes_dead_session_only_on_fresh_query() {
        let dir = tempfile::tempdir().unwrap();
        let dead_file = dir.path().join("dead-agent.json");
        let snapshot = SessionSnapshot {
            name: "dead-agent".into(),
            is_current: false,
            tabs: Vec::new(),
            active_pane: None,
            connected_clients: None,
            needs_resurrection: false,
            inventory: None,
        };
        std::fs::write(&dead_file, serde_json::to_vec(&snapshot).unwrap()).unwrap();

        let hosts = HashSet::new();
        let mut host_cache = HashMap::new();
        let fresh_statuses = BTreeMap::from([("live-agent".into(), SessionStatus::Live)]);

        // 1. Fresh query with can_prune = true must delete dead file
        let sessions = read_all_states_with_statuses(
            dir.path(),
            &hosts,
            &mut host_cache,
            Some(&fresh_statuses),
            true,
        );
        assert!(
            !dead_file.exists(),
            "Dead session file was not pruned on fresh successful query"
        );
        assert!(!sessions.iter().any(|s| s.name == "dead-agent"));

        // 2. Failed query (session_statuses = None, can_prune = false) must NOT delete
        let alive_file = dir.path().join("alive-agent.json");
        let snapshot_alive = SessionSnapshot {
            name: "alive-agent".into(),
            is_current: false,
            tabs: Vec::new(),
            active_pane: None,
            connected_clients: None,
            needs_resurrection: false,
            inventory: None,
        };
        std::fs::write(&alive_file, serde_json::to_vec(&snapshot_alive).unwrap()).unwrap();

        let sessions2 =
            read_all_states_with_statuses(dir.path(), &hosts, &mut host_cache, None, false);
        assert!(
            alive_file.exists(),
            "File was pruned when session statuses query failed"
        );
        assert!(sessions2.iter().any(|s| s.name == "alive-agent"));
    }

    #[test]
    fn registered_host_session_never_deleted_and_filtered() {
        let dir = tempfile::tempdir().unwrap();
        let host_file = dir.path().join("host-workspace.json");
        let snapshot = SessionSnapshot {
            name: "host-workspace".into(),
            is_current: true,
            tabs: Vec::new(),
            active_pane: None,
            connected_clients: None,
            needs_resurrection: false,
            inventory: None,
        };
        std::fs::write(&host_file, serde_json::to_vec(&snapshot).unwrap()).unwrap();

        let hosts = HashSet::from(["host-workspace".to_string()]);
        let mut host_cache = HashMap::new();
        let statuses = BTreeMap::from([("other-session".into(), SessionStatus::Live)]);

        let sessions = read_all_states_with_statuses(
            dir.path(),
            &hosts,
            &mut host_cache,
            Some(&statuses),
            true,
        );

        assert!(
            host_file.exists(),
            "Host session state file must never be deleted"
        );
        assert!(
            !sessions.iter().any(|s| s.name == "host-workspace"),
            "Host session must be filtered out"
        );
    }

    #[test]
    fn host_classification_tri_state_unknown_not_cached() {
        let hosts = HashSet::new();
        let mut host_cache = HashMap::new();

        // Unknown session with unresolvable name should result in Unknown outcome, which is NOT cached
        let is_host = check_is_host(
            "unresolvable-random-session-xyz-1234",
            &hosts,
            &mut host_cache,
        );
        assert!(!is_host);
        assert!(
            !host_cache.contains_key("unresolvable-random-session-xyz-1234"),
            "Unknown classification outcome must not be memoized forever"
        );
    }

    #[test]
    fn channel_full_eventually_sends_latest_when_quiet() {
        let dir = tempfile::tempdir().unwrap();
        let (tx, mut rx) = mpsc::channel(1);
        // Fill channel completely
        tx.try_send(vec![]).unwrap();

        // Spawns watcher which attempts initial send, encounters Full channel, stores pending
        let handle = spawn_fs_watcher_inner(dir.path().to_path_buf(), tx).unwrap();

        // Channel remains full briefly
        std::thread::sleep(Duration::from_millis(20));

        // UI frees up the channel
        let drained = rx.try_recv();
        assert!(drained.is_ok());

        // Watcher must deliver pending snapshot on quiet slice
        let start = Instant::now();
        let mut delivered = false;
        while start.elapsed() < Duration::from_millis(2000) {
            if rx.try_recv().is_ok() {
                delivered = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(15));
        }
        assert!(
            delivered,
            "Watcher failed to deliver pending snapshot once channel freed up during quiet"
        );

        handle.join().unwrap();
    }
}
