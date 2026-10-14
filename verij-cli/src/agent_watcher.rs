//! Agent updates are independent of slower Zellij topology discovery.
use anyhow::Result;
use notify::{Config, Event, EventKind, RecommendedWatcher, RecursiveMode, Watcher};
use std::collections::{BTreeMap, HashMap, HashSet};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use verij_types::agent::{
    AgentIdentity, AgentInstanceId, AgentState, AgentStatus, AGENT_SCHEMA_VERSION,
};
use verij_types::identity::valid_record_key;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentRecord {
    pub identity: AgentIdentity,
    pub state: AgentState,
}

pub use crate::agent_store::FileStamp;

#[derive(Debug, Clone)]
pub struct IdentityCacheEntry {
    pub stamp: FileStamp,
    pub identity: AgentIdentity,
    pub process: verij_types::identity::ProcessIdentity,
    pub last_liveness_check_ms: u64,
    pub is_alive: bool,
}

pub fn read_single_agent(
    root: &Path,
    id: &AgentInstanceId,
    identity_cache: &mut HashMap<AgentInstanceId, IdentityCacheEntry>,
) -> Option<AgentRecord> {
    let dir = root.join(&id.0);
    if !dir.is_dir() || dir.join(".tombstone").exists() {
        identity_cache.remove(id);
        return None;
    }

    let id_path = dir.join("identity.json");

    // 1. Resolve identity, using cache qualified by process lifetime and file stamp
    let identity = if let Some(cached) = identity_cache.get_mut(id) {
        let meta_opt = std::fs::metadata(&id_path).ok().filter(|m| m.is_file());
        let current_stamp = meta_opt.as_ref().map(FileStamp::from_metadata);
        if current_stamp.is_some() && current_stamp.as_ref() == Some(&cached.stamp) {
            let now = crate::agent_store::now_ms();
            if now.saturating_sub(cached.last_liveness_check_ms) >= 1000 {
                cached.last_liveness_check_ms = now;
                cached.is_alive = if cached.identity.is_synthetic {
                    Path::new(&format!("/proc/{}", cached.process.pid)).exists()
                } else {
                    crate::process::is_alive(&cached.process)
                };
            }
            if cached.is_alive {
                cached.identity.clone()
            } else {
                identity_cache.remove(id);
                return None;
            }
        } else {
            // File identity stamp changed on disk or stat failed (atomic replacement or reuse)
            identity_cache.remove(id);
            let (identity, file_meta) = match crate::agent_store::read_bounded_json_with_meta::<
                AgentIdentity,
            >(
                &id_path, crate::agent_store::IDENTITY_MAX_BYTES
            ) {
                Ok(res) => res,
                Err(_) => return None,
            };
            if identity.schema_version != AGENT_SCHEMA_VERSION
                || identity.agent_instance_id.0 != id.0
            {
                return None;
            }
            let is_alive = if identity.is_synthetic {
                Path::new(&format!("/proc/{}", identity.process.pid)).exists()
            } else {
                crate::process::is_alive(&identity.process)
            };
            if !is_alive {
                return None;
            }
            let stamp = FileStamp::from_metadata(&file_meta);
            identity_cache.insert(
                id.clone(),
                IdentityCacheEntry {
                    stamp,
                    identity: identity.clone(),
                    process: identity.process.clone(),
                    last_liveness_check_ms: crate::agent_store::now_ms(),
                    is_alive: true,
                },
            );
            identity
        }
    } else {
        let (identity, file_meta) = match crate::agent_store::read_bounded_json_with_meta::<
            AgentIdentity,
        >(&id_path, crate::agent_store::IDENTITY_MAX_BYTES)
        {
            Ok(res) => res,
            Err(_) => return None,
        };
        if identity.schema_version != AGENT_SCHEMA_VERSION || identity.agent_instance_id.0 != id.0 {
            return None;
        }
        let is_alive = if identity.is_synthetic {
            Path::new(&format!("/proc/{}", identity.process.pid)).exists()
        } else {
            crate::process::is_alive(&identity.process)
        };
        if !is_alive {
            return None;
        }
        let stamp = FileStamp::from_metadata(&file_meta);
        identity_cache.insert(
            id.clone(),
            IdentityCacheEntry {
                stamp,
                identity: identity.clone(),
                process: identity.process.clone(),
                last_liveness_check_ms: crate::agent_store::now_ms(),
                is_alive: true,
            },
        );
        identity
    };

    // 2. Read state.json bounded
    let state_path = dir.join("state.json");
    let Ok(mut state) = crate::agent_store::read_bounded_json::<AgentState>(
        &state_path,
        crate::agent_store::STATE_MAX_BYTES,
    ) else {
        return None;
    };
    if state.schema_version != AGENT_SCHEMA_VERSION
        || state.agent_instance_id != identity.agent_instance_id
    {
        return None;
    }
    if crate::opencode::lease_expired(&state, crate::agent_store::now_ms()) {
        state.reduced.status = verij_types::agent::AgentStatus::Unknown;
        state.reduced.pending_requests.clear();
        state.reduced.latest_completion = None;
        state.reduced.latest_error = None;
        state.reduced.detail = Some("OpenCode reporter unavailable".into());
    }

    degrade_expired_pi_lease(&mut state, crate::agent_store::now_ms());

    Some(AgentRecord { identity, state })
}

/// Read-side projection only: preserve source snapshots and the completion ledger.
/// Idempotence avoids publishing the same expired cached row every sweep.
fn degrade_expired_pi_lease(state: &mut AgentState, now_ms: u64) -> bool {
    if !crate::pi::lease_expired(state, now_ms) {
        return false;
    }
    let reduced = &mut state.reduced;
    let changed = reduced.status != AgentStatus::Unknown
        || !reduced.pending_requests.is_empty()
        || reduced.background_task_count != 0
        || reduced.latest_completion.is_some()
        || reduced.latest_error.is_some()
        || reduced.detail.as_deref() != Some("Pi reporter unavailable");
    if changed {
        reduced.status = AgentStatus::Unknown;
        reduced.pending_requests.clear();
        reduced.background_task_count = 0;
        reduced.latest_completion = None;
        reduced.latest_error = None;
        reduced.detail = Some("Pi reporter unavailable".into());
    }
    changed
}

pub fn read_records(root: &Path) -> Vec<AgentRecord> {
    let mut records = Vec::new();
    let Ok(entries) = std::fs::read_dir(root) else {
        return records;
    };
    let mut identity_cache = HashMap::new();
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name_str) = name.to_str().filter(|id| valid_record_key(id)) else {
            continue;
        };
        let id = AgentInstanceId(name_str.to_string());
        if let Some(record) = read_single_agent(root, &id, &mut identity_cache) {
            records.push(record);
        }
    }
    records.sort_by(|a, b| {
        a.identity
            .agent_instance_id
            .cmp(&b.identity.agent_instance_id)
    });
    records
}

pub const MAX_DIRTY_INGRESS: usize = 128;

pub struct AgentNotificationSignal {
    pub dirty_ids: std::sync::Mutex<HashSet<AgentInstanceId>>,
    pub full_rescan: AtomicBool,
    pub wake_tx: std::sync::mpsc::SyncSender<()>,
    pub wake_rx: std::sync::Mutex<std::sync::mpsc::Receiver<()>>,
}

impl AgentNotificationSignal {
    pub fn new() -> Self {
        let (wake_tx, wake_rx) = std::sync::mpsc::sync_channel(1);
        Self {
            dirty_ids: std::sync::Mutex::new(HashSet::new()),
            full_rescan: AtomicBool::new(false),
            wake_tx,
            wake_rx: std::sync::Mutex::new(wake_rx),
        }
    }

    pub fn notify_full_rescan(&self) {
        crate::resource_metrics::fs_incoming(true);
        self.full_rescan.store(true, Ordering::Release);
        let _ = self.wake_tx.try_send(());
    }

    pub fn notify_ids(&self, ids: &[AgentInstanceId]) {
        crate::resource_metrics::fs_incoming(true);
        match self.dirty_ids.try_lock() {
            Ok(mut guard) => {
                for id in ids {
                    if guard.len() >= MAX_DIRTY_INGRESS && !guard.contains(id) {
                        self.full_rescan.store(true, Ordering::Release);
                        crate::resource_metrics::fs_coalesced(true);
                    } else {
                        guard.insert(id.clone());
                    }
                }
            }
            Err(_) => {
                self.full_rescan.store(true, Ordering::Release);
                crate::resource_metrics::fs_coalesced(true);
            }
        }
        let _ = self.wake_tx.try_send(());
    }

    #[allow(dead_code)]
    pub fn notify(&self, dirty_id: Option<AgentInstanceId>) {
        match dirty_id {
            Some(id) => self.notify_ids(&[id]),
            None => self.notify_full_rescan(),
        }
    }

    pub fn notify_all(&self) {
        let _ = self.wake_tx.try_send(());
    }

    pub fn drain(&self) -> (HashSet<AgentInstanceId>, bool) {
        let full_rescan = self.full_rescan.swap(false, Ordering::AcqRel);
        let ids = match self.dirty_ids.try_lock() {
            Ok(mut guard) => std::mem::take(&mut *guard),
            Err(_) => {
                self.full_rescan.store(true, Ordering::Release);
                HashSet::new()
            }
        };
        (ids, full_rescan)
    }

    pub fn wait_timeout(&self, timeout: Duration) -> bool {
        let Ok(rx) = self.wake_rx.lock() else {
            return false;
        };
        if timeout.is_zero() {
            rx.try_recv().is_ok()
        } else {
            rx.recv_timeout(timeout).is_ok()
        }
    }
}

/// Owned handle to a running agent watcher thread.
#[allow(dead_code)]
pub struct WatcherHandle {
    thread: Option<std::thread::JoinHandle<()>>,
    stop: Arc<AtomicBool>,
    signal: Arc<AgentNotificationSignal>,
}

#[allow(dead_code)]
impl WatcherHandle {
    pub fn stop(&self) {
        self.stop.store(true, Ordering::Release);
        self.signal.notify_all();
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

pub fn spawn(tx: mpsc::Sender<Vec<AgentRecord>>) -> Result<WatcherHandle> {
    let root = crate::agent_store::resolve_v1_dir()?;
    spawn_inner(root, tx)
}

/// Ingests a filesystem event directly into the notification signal.
/// Filters access events and temporary dotfiles, enqueues both source and destination IDs
/// on atomic renames, and triggers full rescan on root events or need_rescan().
pub fn ingest_event(root: &Path, signal: &AgentNotificationSignal, event: &notify::Event) {
    if event.need_rescan() {
        signal.notify_full_rescan();
        return;
    }

    match event.kind {
        EventKind::Create(_) | EventKind::Modify(_) | EventKind::Remove(_) | EventKind::Any => {}
        _ => return,
    }

    let mut is_root = false;
    let mut discovered: [Option<AgentInstanceId>; 4] = [None, None, None, None];
    let mut discovered_len = 0;

    for path in &event.paths {
        if path == root {
            is_root = true;
            break;
        }
        if let Ok(rel) = path.strip_prefix(root) {
            let mut comps = rel.components();
            if let Some(first) = comps.next() {
                let first_str = first.as_os_str().to_string_lossy();
                if valid_record_key(&first_str) {
                    if let Some(second) = comps.next() {
                        let second_str = second.as_os_str().to_string_lossy();
                        if second_str == ".tombstone"
                            || second_str == "state.json"
                            || second_str == "identity.json"
                        {
                            let id = AgentInstanceId(first_str.to_string());
                            if discovered_len < discovered.len()
                                && !discovered[..discovered_len]
                                    .iter()
                                    .any(|d| d.as_ref() == Some(&id))
                            {
                                discovered[discovered_len] = Some(id);
                                discovered_len += 1;
                            }
                        }
                    } else {
                        let id = AgentInstanceId(first_str.to_string());
                        if discovered_len < discovered.len()
                            && !discovered[..discovered_len]
                                .iter()
                                .any(|d| d.as_ref() == Some(&id))
                        {
                            discovered[discovered_len] = Some(id);
                            discovered_len += 1;
                        }
                    }
                }
            } else {
                is_root = true;
                break;
            }
        }
    }

    if is_root {
        signal.notify_full_rescan();
    } else if discovered_len > 0 {
        match signal.dirty_ids.try_lock() {
            Ok(mut guard) => {
                let mut added = false;
                for opt in &discovered[..discovered_len] {
                    if let Some(id) = opt {
                        if guard.len() >= MAX_DIRTY_INGRESS && !guard.contains(id) {
                            signal.full_rescan.store(true, Ordering::Release);
                            crate::resource_metrics::fs_coalesced(true);
                        } else {
                            guard.insert(id.clone());
                            added = true;
                        }
                    }
                }
                if added || signal.full_rescan.load(Ordering::Acquire) {
                    let _ = signal.wake_tx.try_send(());
                }
            }
            Err(_) => {
                signal.full_rescan.store(true, Ordering::Release);
                crate::resource_metrics::fs_coalesced(true);
                let _ = signal.wake_tx.try_send(());
            }
        }
    }
}

pub fn spawn_inner(root: PathBuf, tx: mpsc::Sender<Vec<AgentRecord>>) -> Result<WatcherHandle> {
    let stop = Arc::new(AtomicBool::new(false));
    let stop_thread = stop.clone();
    let signal = Arc::new(AgentNotificationSignal::new());
    let signal_stop = signal.clone();
    let signal_cb = signal.clone();
    let signal_worker = signal.clone();

    let stop_watchdog = stop.clone();
    let sender = tx.downgrade();
    std::thread::Builder::new()
        .name("verij-agent-stop".into())
        .spawn(move || {
            while !stop_watchdog.load(Ordering::Acquire) {
                match sender.upgrade() {
                    Some(sender) if !sender.is_closed() => {}
                    _ => {
                        stop_watchdog.store(true, Ordering::Release);
                        signal_stop.notify_all();
                        break;
                    }
                }
                std::thread::sleep(Duration::from_millis(50));
            }
        })?;

    let root_cb = root.clone();
    let thread = std::thread::Builder::new()
        .name("verij-agent-watcher".to_string())
        .spawn(move || {
            let mut watcher = RecommendedWatcher::new(
                move |event: notify::Result<Event>| match event {
                    Ok(event) => ingest_event(&root_cb, &signal_cb, &event),
                    Err(_) => signal_cb.notify_full_rescan(),
                },
                Config::default(),
            )
            .ok();

            if let Some(watcher) = watcher.as_mut() {
                let _ = watcher.watch(&root, RecursiveMode::Recursive);
            }

            const MAGY_INTERVAL: Duration = Duration::from_millis(1000);
            const PRUNE_INTERVAL: Duration = Duration::from_secs(5);
            const FULL_RESCAN_INTERVAL: Duration = Duration::from_secs(10);
            const LEASE_LIVENESS_INTERVAL: Duration = Duration::from_millis(1000);

            let mut identity_cache = HashMap::new();
            let mut current_records: BTreeMap<AgentInstanceId, AgentRecord> = BTreeMap::new();
            let mut magy_reconciler = crate::magy::Reconciler::new().ok();
            let mut prune_cursor: Option<std::fs::ReadDir> = None;
            let mut scan_cursor: Option<std::fs::ReadDir> = None;
            let mut scan_in_progress = true; // Initial scan
            let mut followup_rescan = false;
            let mut seen_ids_in_sweep = HashSet::new();
            let mut updated_during_sweep = HashSet::new();
            let mut previous: Option<Vec<AgentRecord>> = None;
            let mut pending_send: Option<Vec<AgentRecord>> = None;
            let mut lease_cursor_key: Option<AgentInstanceId> = None;

            let mut next_magy = Instant::now() + MAGY_INTERVAL;
            let mut next_prune = Instant::now() + PRUNE_INTERVAL;
            let mut next_full_rescan = Instant::now() + FULL_RESCAN_INTERVAL;
            let mut next_lease_liveness = Instant::now() + LEASE_LIVENESS_INTERVAL;

            // Initial magy slice
            if let Some(r) = magy_reconciler.as_mut() {
                let _ = r.step(Some(&stop_thread));
            }

            while !tx.is_closed() && !stop_thread.load(Ordering::Relaxed) {
                // If previous send was full, retry sending pending now
                if let Some(pending) = pending_send.take() {
                    match tx.try_send(pending.clone()) {
                        Ok(()) => {
                            previous = Some(pending);
                        }
                        Err(mpsc::error::TrySendError::Closed(_)) => break,
                        Err(mpsc::error::TrySendError::Full(r)) => {
                            pending_send = Some(r);
                        }
                    }
                }

                let now = Instant::now();
                let time_until = next_magy
                    .min(next_prune)
                    .min(next_full_rescan)
                    .min(next_lease_liveness)
                    .saturating_duration_since(now);
                let slice_timeout = if scan_in_progress {
                    Duration::from_millis(1)
                } else if pending_send.is_some() {
                    Duration::from_millis(10)
                } else {
                    Duration::from_millis(50)
                        .min(time_until)
                        .max(Duration::from_millis(1))
                };

                let signaled = signal_worker.wait_timeout(slice_timeout);

                if tx.is_closed() || stop_thread.load(Ordering::Relaxed) {
                    break;
                }

                let mut changed = false;
                if signaled {
                    // Bounded debounce: sleep in small intervals checking stop, no busy spin
                    let debounce_end = Instant::now() + Duration::from_millis(50);
                    while Instant::now() < debounce_end
                        && !tx.is_closed()
                        && !stop_thread.load(Ordering::Relaxed)
                    {
                        std::thread::sleep(
                            Duration::from_millis(10)
                                .min(debounce_end.saturating_duration_since(Instant::now())),
                        );
                    }
                    if tx.is_closed() || stop_thread.load(Ordering::Relaxed) {
                        break;
                    }

                    let (dirty_ids, full_rescan) = signal_worker.drain();
                    if full_rescan {
                        if scan_in_progress {
                            followup_rescan = true;
                        } else {
                            scan_in_progress = true;
                            scan_cursor = None;
                            seen_ids_in_sweep.clear();
                            updated_during_sweep.clear();
                        }
                    } else if !dirty_ids.is_empty() {
                        for id in &dirty_ids {
                            if tx.is_closed() || stop_thread.load(Ordering::Relaxed) {
                                break;
                            }
                            if scan_in_progress {
                                updated_during_sweep.insert(id.clone());
                            }
                            if let Some(record) = read_single_agent(&root, id, &mut identity_cache)
                            {
                                current_records.insert(id.clone(), record);
                            } else {
                                current_records.remove(id);
                                identity_cache.remove(id);
                            }
                        }
                        changed = true;
                    }
                }

                // Resumable sliced directory scanning
                if scan_in_progress {
                    if scan_cursor.is_none() {
                        scan_cursor = std::fs::read_dir(&root).ok();
                    }
                    if let Some(read_dir) = scan_cursor.as_mut() {
                        let mut slice_count = 0;
                        while slice_count < 32 {
                            if stop_thread.load(Ordering::Relaxed) || tx.is_closed() {
                                break;
                            }
                            match read_dir.next() {
                                Some(Ok(entry)) => {
                                    slice_count += 1;
                                    let name = entry.file_name();
                                    if let Some(name_str) =
                                        name.to_str().filter(|id| valid_record_key(id))
                                    {
                                        let id = AgentInstanceId(name_str.to_string());
                                        seen_ids_in_sweep.insert(id.clone());
                                        if let Some(record) =
                                            read_single_agent(&root, &id, &mut identity_cache)
                                        {
                                            if current_records.get(&id) != Some(&record) {
                                                current_records.insert(id, record);
                                                changed = true;
                                            }
                                        } else if current_records.remove(&id).is_some() {
                                            identity_cache.remove(&id);
                                            changed = true;
                                        }
                                    }
                                }
                                Some(Err(_)) => {
                                    slice_count += 1;
                                }
                                None => {
                                    // Scan completed EOF: remove disappeared IDs not seen or updated in this sweep
                                    let mut to_remove = Vec::new();
                                    for key in current_records.keys() {
                                        if stop_thread.load(Ordering::Relaxed) || tx.is_closed() {
                                            break;
                                        }
                                        if !seen_ids_in_sweep.contains(key)
                                            && !updated_during_sweep.contains(key)
                                        {
                                            to_remove.push(key.clone());
                                        }
                                    }
                                    if !to_remove.is_empty() {
                                        for id in to_remove {
                                            if stop_thread.load(Ordering::Relaxed) || tx.is_closed()
                                            {
                                                break;
                                            }
                                            current_records.remove(&id);
                                            identity_cache.remove(&id);
                                        }
                                        changed = true;
                                    }
                                    // Clear orphan identity_cache entries not just current_records upon successful sweep;
                                    // read failures currently leave orphan cache entries.
                                    identity_cache.retain(|id, _| {
                                        seen_ids_in_sweep.contains(id)
                                            || updated_during_sweep.contains(id)
                                    });
                                    seen_ids_in_sweep.clear();
                                    updated_during_sweep.clear();

                                    if followup_rescan {
                                        followup_rescan = false;
                                        scan_in_progress = true;
                                        scan_cursor = None;
                                    } else {
                                        scan_in_progress = false;
                                        scan_cursor = None;
                                        if previous.is_none() {
                                            changed = true;
                                        }
                                    }
                                    break;
                                }
                            }
                        }
                    } else {
                        // Directory open error: preserve current_records (do not falsely publish deletion)
                        scan_in_progress = false;
                        if previous.is_none() {
                            changed = true;
                        }
                    }
                }

                let now = Instant::now();
                if let Some(r) = magy_reconciler.as_mut() {
                    if now >= next_magy || r.has_pending() {
                        let _ = r.step(Some(&stop_thread));
                        if r.has_pending() {
                            next_magy = now;
                        } else {
                            next_magy = now + MAGY_INTERVAL;
                        }
                    }
                }
                if now >= next_prune {
                    next_prune = now + PRUNE_INTERVAL;
                    let _ = crate::agent_store::prune_slice_in_root(
                        &root,
                        &mut prune_cursor,
                        16,
                        false,
                        Some(&stop_thread),
                    );
                }
                if now >= next_full_rescan {
                    next_full_rescan = now + FULL_RESCAN_INTERVAL;
                    if scan_in_progress {
                        followup_rescan = true;
                    } else {
                        scan_in_progress = true;
                        // Do not reset active cursor on periodic fallback
                        if scan_cursor.is_none() {
                            seen_ids_in_sweep.clear();
                            updated_during_sweep.clear();
                        }
                    }
                }

                // In-memory lease projection & cheap liveness check bounded to 32 records per slice
                if now >= next_lease_liveness {
                    let now_ms = crate::agent_store::now_ms();
                    let mut dead_ids = Vec::new();
                    let mut checked_count = 0;
                    let mut reached_end = true;

                    let range: Box<dyn Iterator<Item = (&AgentInstanceId, &mut AgentRecord)>> =
                        match &lease_cursor_key {
                            Some(key) => Box::new(current_records.range_mut((
                                std::ops::Bound::Excluded(key),
                                std::ops::Bound::Unbounded,
                            ))),
                            None => Box::new(current_records.iter_mut()),
                        };

                    for (id, record) in range {
                        if stop_thread.load(Ordering::Relaxed) || tx.is_closed() {
                            reached_end = false;
                            break;
                        }
                        checked_count += 1;
                        lease_cursor_key = Some(id.clone());

                        if record.state.reduced.status != AgentStatus::Unknown
                            && crate::opencode::lease_expired(&record.state, now_ms)
                        {
                            record.state.reduced.status = AgentStatus::Unknown;
                            record.state.reduced.pending_requests.clear();
                            record.state.reduced.latest_completion = None;
                            record.state.reduced.latest_error = None;
                            record.state.reduced.detail =
                                Some("OpenCode reporter lease expired".into());
                            changed = true;
                        }

                        changed |= degrade_expired_pi_lease(&mut record.state, now_ms);

                        // Cached liveness recheck
                        let is_dead = if let Some(cached) = identity_cache.get_mut(id) {
                            if now_ms.saturating_sub(cached.last_liveness_check_ms) >= 1000 {
                                cached.last_liveness_check_ms = now_ms;
                                cached.is_alive = if cached.identity.is_synthetic {
                                    Path::new(&format!("/proc/{}", cached.process.pid)).exists()
                                } else {
                                    !crate::process::is_definitely_dead(&cached.process)
                                };
                            }
                            !cached.is_alive
                        } else if record.identity.is_synthetic {
                            !Path::new(&format!("/proc/{}", record.identity.process.pid)).exists()
                        } else {
                            crate::process::is_definitely_dead(&record.identity.process)
                        };

                        if is_dead {
                            dead_ids.push(id.clone());
                        }

                        if checked_count >= 32 {
                            reached_end = false;
                            break;
                        }
                    }

                    for id in dead_ids {
                        if stop_thread.load(Ordering::Relaxed) || tx.is_closed() {
                            break;
                        }
                        current_records.remove(&id);
                        identity_cache.remove(&id);
                        changed = true;
                    }

                    if reached_end {
                        lease_cursor_key = None;
                        next_lease_liveness = now + LEASE_LIVENESS_INTERVAL;
                    } else {
                        next_lease_liveness = now;
                    }
                }

                if tx.is_closed() || stop_thread.load(Ordering::Relaxed) {
                    break;
                }

                if changed || pending_send.is_some() {
                    let records: Vec<AgentRecord> = current_records.values().cloned().collect();
                    if previous.as_ref() != Some(&records) || pending_send.is_some() {
                        match tx.try_send(records.clone()) {
                            Ok(()) => {
                                previous = Some(records);
                                pending_send = None;
                            }
                            Err(mpsc::error::TrySendError::Closed(_)) => break,
                            Err(mpsc::error::TrySendError::Full(rejected)) => {
                                // Channel saturated: replace pending_send with the NEWEST state
                                pending_send = Some(rejected);
                                crate::resource_metrics::fs_coalesced(true);
                            }
                        }
                    }
                }
            }
        })?;

    Ok(WatcherHandle {
        thread: Some(thread),
        stop,
        signal,
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn watchdog_full_channel_closed_stops_promptly() {
        let dir = tempfile::tempdir().unwrap();
        // Bounded channel with capacity 1
        let (tx, rx) = mpsc::channel(1);
        // Fill channel completely
        tx.try_send(vec![]).unwrap();

        let handle = spawn_inner(dir.path().to_path_buf(), tx).unwrap();
        // Dropping receiver must cause watcher to terminate promptly without blocking on full channel
        drop(rx);

        let start = Instant::now();
        while !handle.is_finished() && start.elapsed() < Duration::from_millis(500) {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            handle.is_finished(),
            "Watcher failed to stop promptly on closed channel"
        );
        handle.join().unwrap();
    }

    #[test]
    fn continuous_notification_stops_promptly() {
        let dir = tempfile::tempdir().unwrap();
        let (tx, rx) = mpsc::channel(16);
        let handle = spawn_inner(dir.path().to_path_buf(), tx).unwrap();

        let stop_spammer = Arc::new(AtomicBool::new(false));
        let stop_spammer_clone = stop_spammer.clone();
        let dir_path = dir.path().to_path_buf();
        let spammer = std::thread::spawn(move || {
            let mut i = 0;
            while !stop_spammer_clone.load(Ordering::Relaxed) {
                let _ = std::fs::write(dir_path.join(format!("dummy-{i}")), b"test");
                i += 1;
                std::thread::sleep(Duration::from_millis(2));
            }
        });

        // Let notifications stream
        std::thread::sleep(Duration::from_millis(50));
        // Drop receiver during active continuous notification flood
        drop(rx);

        let start = Instant::now();
        while !handle.is_finished() && start.elapsed() < Duration::from_millis(500) {
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            handle.is_finished(),
            "Watcher did not stop promptly during continuous notifications"
        );

        stop_spammer.store(true, Ordering::Relaxed);
        spammer.join().unwrap();
        handle.join().unwrap();
    }

    #[test]
    fn periodic_quiet_stream_shutdown_prompt() {
        let dir = tempfile::tempdir().unwrap();
        let (tx, rx) = mpsc::channel(16);
        let handle = spawn_inner(dir.path().to_path_buf(), tx).unwrap();

        // Stream is quiet (no file events)
        std::thread::sleep(Duration::from_millis(60));
        let start = Instant::now();
        drop(rx);

        while !handle.is_finished() && start.elapsed() < Duration::from_millis(500) {
            std::thread::sleep(Duration::from_millis(10));
        }
        let elapsed = start.elapsed();
        assert!(
            handle.is_finished(),
            "Quiet stream watcher failed to terminate"
        );
        assert!(
            elapsed < Duration::from_millis(250),
            "Shutdown took too long: {elapsed:?}"
        );
        handle.join().unwrap();
    }

    #[test]
    fn channel_full_eventually_sends_latest_when_quiet() {
        let dir = tempfile::tempdir().unwrap();
        let (tx, mut rx) = mpsc::channel(1);
        tx.try_send(vec![]).unwrap();

        let handle = spawn_inner(dir.path().to_path_buf(), tx).unwrap();
        std::thread::sleep(Duration::from_millis(20));

        let drained = rx.try_recv();
        assert!(drained.is_ok());

        let start = Instant::now();
        let mut delivered = false;
        while start.elapsed() < Duration::from_millis(500) {
            if rx.try_recv().is_ok() {
                delivered = true;
                break;
            }
            std::thread::sleep(Duration::from_millis(15));
        }
        assert!(
            delivered,
            "Agent watcher failed to deliver pending records once channel freed up during quiet"
        );

        handle.join().unwrap();
    }

    fn write_test_agent(
        root: &Path,
        id_str: &str,
        runner_id: Option<&str>,
        rev: u64,
        status: AgentStatus,
    ) {
        let dir = root.join(id_str);
        std::fs::create_dir_all(&dir).unwrap();
        let identity = AgentIdentity {
            schema_version: AGENT_SCHEMA_VERSION,
            agent_instance_id: AgentInstanceId(id_str.to_string()),
            pane_key: verij_types::agent::PaneKey {
                session: verij_types::identity::SessionInstanceId("sess-1".into()),
                terminal: verij_types::identity::TerminalPaneId(1),
            },
            kind: verij_types::agent::AgentKind::Opencode,
            process: verij_types::identity::ProcessIdentity {
                pid: std::process::id(),
                start_jiffies: 100,
                boot_id: "boot".into(),
                uid: unsafe { libc::getuid() },
            },
            registered_at_ms: 1000,
            is_synthetic: true,
            runner_id: runner_id.map(String::from),
        };
        crate::agent_store::atomic_write_json(&dir.join("identity.json"), &identity).unwrap();

        let state = AgentState {
            schema_version: AGENT_SCHEMA_VERSION,
            agent_instance_id: AgentInstanceId(id_str.to_string()),
            record_revision: rev,
            updated_at_ms: 1000 + rev,
            sources: BTreeMap::new(),
            reduced: verij_types::agent::ReducedExecutionState {
                status,
                current_turn_id: None,
                current_turn_epoch: 0,
                current_turn_revision: 0,
                conversation_id: None,
                conversation_title: None,
                pending_requests: Vec::new(),
                background_task_count: 0,
                latest_error: None,
                latest_completion: None,
                completion_revision: if status == AgentStatus::Done { rev } else { 0 },
                detail: None,
            },
            completion_revision: if status == AgentStatus::Done { rev } else { 0 },
            completed_turn: None,
            completed_turn_epoch: 0,
            completed_turn_revision: if status == AgentStatus::Done { rev } else { 0 },
            opencode_completions: BTreeMap::new(),
            adapter_state: BTreeMap::new(),
        };
        crate::agent_store::atomic_write_json(&dir.join("state.json"), &state).unwrap();
    }

    #[test]
    fn test_dirty_ingress_bounds_and_overflow_coalescing() {
        let signal = AgentNotificationSignal::new();
        let ids: Vec<AgentInstanceId> = (0..200)
            .map(|i| AgentInstanceId(format!("agent-test-{i}")))
            .collect();

        signal.notify_ids(&ids);
        let (drained, rescan) = signal.drain();
        // Fixed bound: at most MAX_DIRTY_INGRESS (128)
        assert!(drained.len() <= MAX_DIRTY_INGRESS);
        // Overflow flag must be set
        assert!(rescan, "Expected overflow to trigger full_rescan");
    }

    #[test]
    fn test_incremental_reads_and_immutable_identity_caching() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let agent_id = AgentInstanceId("agent-cached-1".to_string());
        write_test_agent(
            root,
            &agent_id.0,
            Some("runner-orig"),
            1,
            AgentStatus::Working,
        );

        let mut cache = HashMap::new();
        let rec1 = read_single_agent(root, &agent_id, &mut cache).expect("first read succeeds");
        assert_eq!(rec1.identity.runner_id.as_deref(), Some("runner-orig"));
        assert_eq!(rec1.state.record_revision, 1);
        assert_eq!(rec1.state.reduced.status, AgentStatus::Working);
        assert!(cache.contains_key(&agent_id));

        // Update state.json only (identity.json untouched)
        write_test_agent(root, &agent_id.0, Some("runner-orig"), 2, AgentStatus::Done);
        let rec2 = read_single_agent(root, &agent_id, &mut cache).expect("second read succeeds");
        // State is updated to revision 2
        assert_eq!(rec2.state.record_revision, 2);
        assert_eq!(rec2.state.reduced.status, AgentStatus::Done);
        // Cached identity entry was reused
        assert_eq!(rec2.identity.runner_id.as_deref(), Some("runner-orig"));
    }

    #[test]
    fn test_id_reuse_and_atomic_replacement_invalidates_cache() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let agent_id = AgentInstanceId("agent-reused-1".to_string());
        write_test_agent(
            root,
            &agent_id.0,
            Some("runner-v1"),
            1,
            AgentStatus::Working,
        );

        let mut cache = HashMap::new();
        let rec1 = read_single_agent(root, &agent_id, &mut cache).expect("first read succeeds");
        assert_eq!(rec1.identity.runner_id.as_deref(), Some("runner-v1"));

        // Sleep briefly to ensure mtime/ctime changes
        std::thread::sleep(Duration::from_millis(15));

        // Replace identity.json atomically with a different runner
        write_test_agent(
            root,
            &agent_id.0,
            Some("runner-v2"),
            1,
            AgentStatus::Working,
        );
        let rec2 = read_single_agent(root, &agent_id, &mut cache).expect("second read succeeds");
        // Identity cache must invalidate on stamp change and load new runner
        assert_eq!(rec2.identity.runner_id.as_deref(), Some("runner-v2"));
    }

    #[test]
    fn test_saturated_channel_delivers_latest_snapshot_with_done() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let (tx, mut rx) = mpsc::channel(1);

        // Pre-saturate channel so watcher's next update cannot immediately send
        tx.try_send(vec![]).unwrap();

        let handle = spawn_inner(root.to_path_buf(), tx).unwrap();
        std::thread::sleep(Duration::from_millis(20));

        let agent_id = "agent-saturated-1";
        // Revision 1: Working
        write_test_agent(root, agent_id, None, 1, AgentStatus::Working);
        std::thread::sleep(Duration::from_millis(20));

        // Revision 2: Done (completion revision 2)
        write_test_agent(root, agent_id, None, 2, AgentStatus::Done);
        std::thread::sleep(Duration::from_millis(60));

        // Drain initial pre-fill
        let first = rx.try_recv().expect("drain pre-fill");
        assert!(first.is_empty());

        // Wait for next snapshot delivery
        let start = Instant::now();
        let mut latest_records = None;
        while start.elapsed() < Duration::from_millis(2000) {
            if let Ok(records) = rx.try_recv() {
                if !records.is_empty() {
                    latest_records = Some(records);
                    break;
                }
            }
            std::thread::sleep(Duration::from_millis(20));
        }

        let records =
            latest_records.expect("Watcher must deliver latest snapshot after channel drain");
        assert_eq!(records.len(), 1);
        let record = &records[0];
        assert_eq!(record.identity.agent_instance_id.0, agent_id);
        // CRITICAL CHECK: latest state revision 2 and Done status survived saturation!
        assert_eq!(record.state.record_revision, 2);
        assert_eq!(record.state.reduced.status, AgentStatus::Done);

        handle.join().unwrap();
    }

    #[test]
    fn test_ingest_event_filters_access_and_temp_and_handles_renames() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let signal = AgentNotificationSignal::new();

        // 1. Access event must be ignored completely
        let access_event = notify::Event {
            kind: notify::EventKind::Access(notify::event::AccessKind::Read),
            paths: vec![root.join("agent-1/state.json")],
            attrs: Default::default(),
        };
        ingest_event(root, &signal, &access_event);
        let (drained, rescan) = signal.drain();
        assert!(drained.is_empty());
        assert!(!rescan);

        // 2. Temp and lock files must be ignored without triggering full rescan
        let lock_event = notify::Event {
            kind: notify::EventKind::Modify(notify::event::ModifyKind::Data(
                notify::event::DataChange::Any,
            )),
            paths: vec![root.join("agent-1/.lock")],
            attrs: Default::default(),
        };
        ingest_event(root, &signal, &lock_event);
        let (drained, rescan) = signal.drain();
        assert!(drained.is_empty());
        assert!(!rescan);

        let tmp_event = notify::Event {
            kind: notify::EventKind::Create(notify::event::CreateKind::File),
            paths: vec![root.join("agent-1/.state.json.tmp.12345")],
            attrs: Default::default(),
        };
        ingest_event(root, &signal, &tmp_event);
        let (drained, rescan) = signal.drain();
        assert!(drained.is_empty());
        assert!(!rescan);

        // 3. State atomic rename within instance: enqueues agent-1
        let rename_event = notify::Event {
            kind: notify::EventKind::Modify(notify::event::ModifyKind::Name(
                notify::event::RenameMode::Both,
            )),
            paths: vec![
                root.join("agent-1/.state.json.tmp.12345"),
                root.join("agent-1/state.json"),
            ],
            attrs: Default::default(),
        };
        ingest_event(root, &signal, &rename_event);
        let (drained, rescan) = signal.drain();
        assert_eq!(drained.len(), 1);
        assert!(drained.contains(&AgentInstanceId("agent-1".into())));
        assert!(!rescan);

        // 4. Cross-instance rename: enqueues BOTH agent-1 and agent-2
        let cross_rename = notify::Event {
            kind: notify::EventKind::Modify(notify::event::ModifyKind::Name(
                notify::event::RenameMode::Both,
            )),
            paths: vec![
                root.join("agent-1/state.json"),
                root.join("agent-2/state.json"),
            ],
            attrs: Default::default(),
        };
        ingest_event(root, &signal, &cross_rename);
        let (drained, rescan) = signal.drain();
        assert_eq!(drained.len(), 2);
        assert!(drained.contains(&AgentInstanceId("agent-1".into())));
        assert!(drained.contains(&AgentInstanceId("agent-2".into())));
        assert!(!rescan);

        // 5. need_rescan triggers full rescan
        let rescan_event =
            notify::Event::new(notify::EventKind::Any).set_flag(notify::event::Flag::Rescan);
        assert!(rescan_event.need_rescan());
        ingest_event(root, &signal, &rescan_event);
        let (_, rescan) = signal.drain();
        assert!(rescan, "need_rescan must trigger full_rescan");
    }

    #[test]
    fn test_lease_expiration_projects_done_and_needs_input() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let id_done = AgentInstanceId("agent-exp-done".to_string());
        write_test_agent(root, &id_done.0, None, 1, AgentStatus::Done);

        // Invalidate lease by inserting expired lease in opencode source record
        let state_path = root.join(&id_done.0).join("state.json");
        let mut state: AgentState =
            crate::agent_store::read_bounded_json(&state_path, 256 * 1024).unwrap();
        state.sources.insert(
            "opencode".into(),
            verij_types::agent::NormalizedSourceRecord {
                schema_version: 1,
                agent_instance_id: id_done.clone(),
                source_id: "opencode".into(),
                source_epoch: 1,
                source_revision: 1,
                observed_at_ms: 100,
                conversation_id: None,
                conversation_title: None,
                turn_id: None,
                turn_epoch: 1,
                turn_revision: 1,
                activity: None,
                pending_requests: Vec::new(),
                background_task_count: 0,
                successful_completion: None,
                execution_error: None,
                capabilities: vec!["lease".into()],
            },
        );
        crate::agent_store::atomic_write_json(&state_path, &state).unwrap();

        let mut cache = HashMap::new();
        let rec = read_single_agent(root, &id_done, &mut cache).expect("read succeeds");
        assert_eq!(rec.state.reduced.status, AgentStatus::Unknown);
        assert_eq!(
            rec.state.reduced.detail.as_deref(),
            Some("OpenCode reporter unavailable")
        );

        // Also verify NeedsInput projects to Unknown on lease expiration
        let id_input = AgentInstanceId("agent-exp-input".to_string());
        write_test_agent(root, &id_input.0, None, 1, AgentStatus::NeedsInput);
        let input_path = root.join(&id_input.0).join("state.json");
        let mut input_state: AgentState =
            crate::agent_store::read_bounded_json(&input_path, 256 * 1024).unwrap();
        input_state.sources.insert(
            "opencode".into(),
            verij_types::agent::NormalizedSourceRecord {
                schema_version: 1,
                agent_instance_id: id_input.clone(),
                source_id: "opencode".into(),
                source_epoch: 1,
                source_revision: 1,
                observed_at_ms: 100,
                conversation_id: None,
                conversation_title: None,
                turn_id: None,
                turn_epoch: 1,
                turn_revision: 1,
                activity: None,
                pending_requests: Vec::new(),
                background_task_count: 0,
                successful_completion: None,
                execution_error: None,
                capabilities: vec!["lease".into()],
            },
        );
        crate::agent_store::atomic_write_json(&input_path, &input_state).unwrap();
        let rec_input = read_single_agent(root, &id_input, &mut cache).expect("read succeeds");
        assert_eq!(rec_input.state.reduced.status, AgentStatus::Unknown);
        assert_eq!(
            rec_input.state.reduced.detail.as_deref(),
            Some("OpenCode reporter unavailable")
        );
    }

    #[test]
    fn test_full_scan_eof_removes_missed_deletion() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let (tx, mut rx) = mpsc::channel(16);

        write_test_agent(root, "agent-live-1", None, 1, AgentStatus::Working);
        write_test_agent(root, "agent-to-delete", None, 1, AgentStatus::Working);

        let handle = spawn_inner(root.to_path_buf(), tx).unwrap();

        // Wait until initial snapshot arrives
        let start = Instant::now();
        let mut got_both = false;
        while start.elapsed() < Duration::from_millis(1000) {
            if let Ok(records) = rx.try_recv() {
                if records.len() == 2 {
                    got_both = true;
                    break;
                }
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(got_both, "Expected initial delivery with both agents");

        // Delete agent-to-delete directory directly from disk without notify event
        let del_dir = root.join("agent-to-delete");
        let _ = std::fs::remove_file(del_dir.join("state.json"));
        let _ = std::fs::remove_file(del_dir.join("identity.json"));
        let _ = std::fs::remove_dir(&del_dir);

        // Trigger full rescan via signal
        handle.signal.notify_full_rescan();

        // Check that next snapshot removes agent-to-delete at scan EOF
        let start = Instant::now();
        let mut removed = false;
        while start.elapsed() < Duration::from_millis(2000) {
            if let Ok(records) = rx.try_recv() {
                if records.len() == 1 && records[0].identity.agent_instance_id.0 == "agent-live-1" {
                    removed = true;
                    break;
                }
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(
            removed,
            "Full scan EOF must remove deleted agent from snapshot"
        );

        handle.join().unwrap();
    }

    #[test]
    fn test_overflow_recovers_latest_done_end_to_end() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let (tx, mut rx) = mpsc::channel(1);

        // Pre-fill channel to saturate output
        tx.try_send(vec![]).unwrap();

        // Write 150 agent records on disk
        for i in 0..150 {
            let status = if i == 100 {
                AgentStatus::Done
            } else {
                AgentStatus::Working
            };
            let rev = if i == 100 { 2 } else { 1 };
            write_test_agent(root, &format!("agent-batch-{i:03}"), None, rev, status);
        }

        let handle = spawn_inner(root.to_path_buf(), tx).unwrap();
        std::thread::sleep(Duration::from_millis(20));

        // Inject >128 IDs into notification signal
        let flood_ids: Vec<AgentInstanceId> = (0..150)
            .map(|i| AgentInstanceId(format!("agent-batch-{i:03}")))
            .collect();
        handle.signal.notify_ids(&flood_ids);

        // Verify fixed set bound in signal: <= 128
        let (drained, rescan) = handle.signal.drain();
        assert!(drained.len() <= MAX_DIRTY_INGRESS);
        assert!(rescan, "Overflow must set full_rescan flag");

        // Put full_rescan back so worker runs full scan
        handle.signal.notify_full_rescan();

        // Free up the output channel
        let prefill = rx.try_recv().expect("drain saturated prefill");
        assert!(prefill.is_empty());

        // Wait for worker to complete sliced scan and deliver full snapshot
        let start = Instant::now();
        let mut delivered = None;
        while start.elapsed() < Duration::from_millis(4000) {
            if let Ok(records) = rx.try_recv() {
                if records.len() >= 150 {
                    delivered = Some(records);
                    break;
                }
            }
            std::thread::sleep(Duration::from_millis(25));
        }

        let records =
            delivered.expect("Worker failed to deliver recovered snapshot after overflow");
        assert_eq!(records.len(), 150);
        let done_record = records
            .iter()
            .find(|r| r.identity.agent_instance_id.0 == "agent-batch-100")
            .expect("target agent-batch-100 must be present in snapshot");
        assert_eq!(done_record.state.record_revision, 2);
        assert_eq!(done_record.state.reduced.status, AgentStatus::Done);

        handle.join().unwrap();
    }

    #[test]
    fn test_full_scan_clears_orphan_identity_cache_entries() {
        let dir = tempfile::tempdir().unwrap();
        let root = dir.path();
        let (tx, mut rx) = mpsc::channel(16);

        write_test_agent(root, "agent-orphan-live", None, 1, AgentStatus::Working);
        write_test_agent(root, "agent-orphan-dead", None, 1, AgentStatus::Working);

        let handle = spawn_inner(root.to_path_buf(), tx).unwrap();

        // Wait until initial snapshot arrives
        let start = Instant::now();
        let mut initial_ok = false;
        while start.elapsed() < Duration::from_millis(1000) {
            if let Ok(records) = rx.try_recv() {
                if records.len() == 2 {
                    initial_ok = true;
                    break;
                }
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(initial_ok, "Initial snapshot delivery failed");

        // Remove dead agent directly from disk
        let del_dir = root.join("agent-orphan-dead");
        let _ = std::fs::remove_file(del_dir.join("state.json"));
        let _ = std::fs::remove_file(del_dir.join("identity.json"));
        let _ = std::fs::remove_dir(&del_dir);

        // Trigger full rescan
        handle.signal.notify_full_rescan();

        // Wait for snapshot with only live agent
        let start = Instant::now();
        let mut live_only = false;
        while start.elapsed() < Duration::from_millis(2000) {
            if let Ok(records) = rx.try_recv() {
                if records.len() == 1
                    && records[0].identity.agent_instance_id.0 == "agent-orphan-live"
                {
                    live_only = true;
                    break;
                }
            }
            std::thread::sleep(Duration::from_millis(20));
        }
        assert!(live_only, "Snapshot did not reflect orphan removal");

        handle.join().unwrap();
    }

    /// Synthetic live process identity with deliberately stale presentation fields.
    fn write_test_pi(
        root: &Path,
        id: &str,
        status: AgentStatus,
        observed_at_ms: u64,
    ) -> AgentState {
        write_test_agent(root, id, None, 7, status);
        let dir = root.join(id);
        let mut identity: AgentIdentity = crate::agent_store::read_bounded_json(
            &dir.join("identity.json"),
            crate::agent_store::IDENTITY_MAX_BYTES,
        )
        .unwrap();
        identity.kind = verij_types::agent::AgentKind::Pi;
        crate::agent_store::atomic_write_json(&dir.join("identity.json"), &identity).unwrap();
        let mut state: AgentState = crate::agent_store::read_bounded_json(
            &dir.join("state.json"),
            crate::agent_store::STATE_MAX_BYTES,
        )
        .unwrap();
        let completion = verij_types::agent::SuccessfulCompletion {
            turn_id: Some(verij_types::agent::TurnId("run-1".into())),
            turn_epoch: 1,
            turn_revision: 1,
            completed_at_ms: 1000,
            summary: None,
        };
        let request = verij_types::agent::PendingRequest {
            id: "req-1".into(),
            kind: verij_types::agent::RequestKind::Confirmation,
            detail: None,
        };
        let error = verij_types::agent::ExecutionError {
            turn_id: completion.turn_id.clone(),
            turn_epoch: 1,
            turn_revision: 1,
            message: "terminal execution failure".into(),
            is_terminal: true,
        };
        state.sources.insert(
            "pi".into(),
            verij_types::agent::NormalizedSourceRecord {
                schema_version: AGENT_SCHEMA_VERSION,
                agent_instance_id: identity.agent_instance_id,
                source_id: "pi".into(),
                source_epoch: 1,
                source_revision: 7,
                observed_at_ms,
                conversation_id: Some(verij_types::agent::ConversationId("conv-1".into())),
                conversation_title: None,
                turn_id: completion.turn_id.clone(),
                turn_epoch: 1,
                turn_revision: 1,
                activity: Some(verij_types::agent::ActivityKind::Working),
                pending_requests: vec![request.clone()],
                background_task_count: 2,
                successful_completion: Some(completion.clone()),
                execution_error: Some(error.clone()),
                capabilities: vec!["lease".into()],
            },
        );
        state.reduced.pending_requests = vec![request];
        state.reduced.background_task_count = 2;
        state.reduced.latest_completion = Some(completion);
        state.reduced.latest_error = Some(error);
        state.reduced.detail = Some("stale presentation".into());
        state.completion_revision = 3;
        state.reduced.completion_revision = 3;
        state.completed_turn = Some(verij_types::agent::TurnId("run-1".into()));
        state.completed_turn_epoch = 1;
        state.completed_turn_revision = 1;
        state.adapter_state.insert(
            "pi".into(),
            serde_json::json!({
                "runtime_id": "rt-1", "latest_snapshot": {"run_id": "run-1"},
                "completions": {"dedup-key": 3}
            }),
        );
        crate::agent_store::atomic_write_json(&dir.join("state.json"), &state).unwrap();
        state
    }

    fn assert_pi_projection_only(projected: &AgentState, original: &AgentState) {
        let mut expected = original.clone();
        expected.reduced.status = AgentStatus::Unknown;
        expected.reduced.pending_requests.clear();
        expected.reduced.background_task_count = 0;
        expected.reduced.latest_completion = None;
        expected.reduced.latest_error = None;
        expected.reduced.detail = Some("Pi reporter unavailable".into());
        assert_eq!(
            projected, &expected,
            "only unavailable presentation may change"
        );
    }

    #[test]
    fn pi_expiry_is_idempotent_for_all_presentations_and_preserves_ledger() {
        let dir = tempfile::tempdir().unwrap();
        for status in [
            AgentStatus::Working,
            AgentStatus::Done,
            AgentStatus::NeedsInput,
            AgentStatus::Unknown,
        ] {
            let original = write_test_pi(dir.path(), "agent-pi-projection", status, 1000);
            let mut state = original.clone();
            assert!(
                !degrade_expired_pi_lease(&mut state, 16_000),
                "lease boundary is inclusive"
            );
            assert_eq!(state, original);
            assert!(degrade_expired_pi_lease(&mut state, 16_001));
            assert_pi_projection_only(&state, &original);
            assert!(
                !degrade_expired_pi_lease(&mut state, 16_002),
                "expired rows must not republish"
            );
        }
    }

    #[test]
    fn pi_incremental_expiry_then_heartbeat_restores_without_historical_done() {
        let dir = tempfile::tempdir().unwrap();
        let id = AgentInstanceId("agent-pi-incremental".into());
        let original = write_test_pi(dir.path(), &id.0, AgentStatus::Done, 1000);
        let path = dir.path().join(&id.0).join("state.json");
        let bytes = std::fs::read(&path).unwrap();
        let mut cache = HashMap::new();
        let expired = read_single_agent(dir.path(), &id, &mut cache).unwrap();
        assert_eq!(expired.identity.kind, verij_types::agent::AgentKind::Pi);
        assert_pi_projection_only(&expired.state, &original);
        assert_eq!(
            std::fs::read(&path).unwrap(),
            bytes,
            "expiry cannot write source state"
        );

        // A committed fresh native snapshot is authoritative, not the cached projection.
        let mut fresh = original.clone();
        fresh.sources.get_mut("pi").unwrap().observed_at_ms = crate::agent_store::now_ms();
        crate::agent_store::atomic_write_json(&path, &fresh).unwrap();
        let restored = read_single_agent(dir.path(), &id, &mut cache).unwrap();
        assert_eq!(restored.state, fresh);
        assert_eq!(
            restored.state.completion_revision,
            expired.state.completion_revision
        );
        assert_eq!(restored.state.adapter_state, expired.state.adapter_state);
    }

    #[test]
    fn pi_quiet_cached_sweep_expires_all_statuses_across_32_record_slices() {
        let dir = tempfile::tempdir().unwrap();
        let statuses = [
            AgentStatus::Working,
            AgentStatus::Done,
            AgentStatus::NeedsInput,
            AgentStatus::Unknown,
        ];
        let observed = crate::agent_store::now_ms().saturating_sub(12_000);
        let mut originals = BTreeMap::new();
        let mut disk_bytes = BTreeMap::new();
        for i in 0..40 {
            let id = AgentInstanceId(format!("agent-pi-quiet-{i:03}"));
            let state = write_test_pi(dir.path(), &id.0, statuses[i % 4], observed);
            disk_bytes.insert(
                id.clone(),
                std::fs::read(dir.path().join(&id.0).join("state.json")).unwrap(),
            );
            originals.insert(id, state);
        }
        let (tx, mut rx) = mpsc::channel(16);
        let handle = spawn_inner(dir.path().to_path_buf(), tx).unwrap();
        let start = Instant::now();
        let mut initial = false;
        let mut expired = false;
        // No disk writes/notifications after startup; fallback rescan is ten seconds away.
        while start.elapsed() < Duration::from_secs(7) {
            if let Ok(records) = rx.try_recv() {
                if records.len() == 40 {
                    initial |= records
                        .iter()
                        .all(|r| r.state == originals[&r.identity.agent_instance_id]);
                    if records.iter().all(|r| {
                        r.state.reduced.detail.as_deref() == Some("Pi reporter unavailable")
                    }) {
                        for record in &records {
                            assert_pi_projection_only(
                                &record.state,
                                &originals[&record.identity.agent_instance_id],
                            );
                        }
                        expired = true;
                        break;
                    }
                }
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(initial, "live snapshots must arrive before expiry");
        assert!(
            expired,
            "quiet expiry must reach records beyond first 32 without rescan"
        );
        for (id, bytes) in disk_bytes {
            assert_eq!(
                std::fs::read(dir.path().join(id.0).join("state.json")).unwrap(),
                bytes
            );
        }
        // Another periodic sweep is a no-op, not a repeated UI publication.
        std::thread::sleep(Duration::from_millis(1200));
        assert!(rx.try_recv().is_err());
        handle.join().unwrap();
    }

    #[test]
    fn pi_overflow_and_full_ui_channel_deliver_latest_completion_and_stop() {
        let dir = tempfile::tempdir().unwrap();
        let now = crate::agent_store::now_ms();
        for i in 0..150 {
            write_test_pi(
                dir.path(),
                &format!("agent-pi-overflow-{i:03}"),
                AgentStatus::Working,
                now,
            );
        }
        let (tx, mut rx) = mpsc::channel(1);
        tx.try_send(vec![]).unwrap();
        let handle = spawn_inner(dir.path().to_path_buf(), tx).unwrap();
        // Wait for the initial sweep, then replace its target while publication is blocked.
        std::thread::sleep(Duration::from_millis(150));
        let target = "agent-pi-overflow-149";
        let mut latest = write_test_pi(dir.path(), target, AgentStatus::Done, now);
        latest.record_revision = 9;
        latest.completion_revision = 4;
        latest.reduced.completion_revision = 4;
        crate::agent_store::atomic_write_json(&dir.path().join(target).join("state.json"), &latest)
            .unwrap();
        let ids: Vec<_> = (0..150)
            .map(|i| AgentInstanceId(format!("agent-pi-overflow-{i:03}")))
            .collect();
        handle.signal.notify_ids(&ids);
        std::thread::sleep(Duration::from_millis(150));
        assert!(rx.try_recv().unwrap().is_empty());
        let start = Instant::now();
        let mut delivered = false;
        while start.elapsed() < Duration::from_secs(3) {
            if let Ok(records) = rx.try_recv() {
                delivered |= records.len() == 150
                    && records
                        .iter()
                        .any(|r| r.identity.agent_instance_id.0 == target && r.state == latest);
                if delivered {
                    break;
                }
            }
            std::thread::sleep(Duration::from_millis(10));
        }
        assert!(
            delivered,
            "overflow and saturation must retain newest Pi state/completion"
        );
        let start = Instant::now();
        handle.join().unwrap();
        assert!(start.elapsed() < Duration::from_millis(500));
    }

    #[test]
    fn test_read_records_expired_pi_lease_degrades_to_unknown_while_preserving_row() {
        let dir = tempfile::tempdir().unwrap();
        let instance_name = "agent-test-pi-lease";
        let inst_dir = dir.path().join(instance_name);
        std::fs::create_dir_all(&inst_dir).unwrap();

        let inst_id = verij_types::agent::AgentInstanceId(instance_name.to_string());
        let identity = AgentIdentity {
            schema_version: AGENT_SCHEMA_VERSION,
            agent_instance_id: inst_id.clone(),
            pane_key: verij_types::identity::PaneKey {
                session: verij_types::identity::SessionInstanceId("test-sess".to_string()),
                terminal: verij_types::identity::TerminalPaneId(1),
            },
            kind: verij_types::agent::AgentKind::Pi,
            process: verij_types::identity::ProcessIdentity {
                pid: std::process::id(),
                start_jiffies: 0,
                boot_id: "test-boot".to_string(),
                uid: unsafe { libc::getuid() },
            },
            registered_at_ms: 1000,
            is_synthetic: true,
            runner_id: None,
        };
        std::fs::write(
            inst_dir.join("identity.json"),
            serde_json::to_string(&identity).unwrap(),
        )
        .unwrap();

        // Expired lease: observed_at_ms is 30 seconds in the past
        let expired_time = crate::agent_store::now_ms().saturating_sub(30_000);
        let mut sources = std::collections::BTreeMap::new();
        sources.insert(
            "pi".to_string(),
            verij_types::agent::NormalizedSourceRecord {
                schema_version: 1,
                agent_instance_id: inst_id.clone(),
                source_id: "pi".to_string(),
                source_epoch: 1,
                source_revision: 5,
                observed_at_ms: expired_time,
                conversation_id: Some(verij_types::agent::ConversationId("conv-1".to_string())),
                conversation_title: Some("Pi Session".to_string()),
                turn_id: Some(verij_types::agent::TurnId("run-1".to_string())),
                turn_epoch: 1,
                turn_revision: 5,
                activity: Some(verij_types::agent::ActivityKind::Working),
                pending_requests: vec![verij_types::agent::PendingRequest {
                    id: "req-1".to_string(),
                    kind: verij_types::agent::RequestKind::Confirmation,
                    detail: None,
                }],
                background_task_count: 2,
                successful_completion: None,
                execution_error: None,
                capabilities: vec!["activity".to_string(), "lease".to_string()],
            },
        );

        let mut adapter_state = std::collections::BTreeMap::new();
        let mut adapter_completions = std::collections::BTreeMap::new();
        adapter_completions.insert("(\"conv-1\",\"run-1\")".to_string(), 1u64);
        adapter_state.insert(
            "pi".to_string(),
            serde_json::json!({
                "running_version": "1.2.0",
                "runtime_id": "rt-1",
                "conversation_id": "conv-1",
                "source_epoch": 1,
                "source_revision": 5,
                "aggregate_run_id": "run-1",
                "aggregate_turn_epoch": 1,
                "aggregate_turn_revision": 5,
                "latest_snapshot": null,
                "completions": adapter_completions
            }),
        );

        let state = AgentState {
            schema_version: AGENT_SCHEMA_VERSION,
            agent_instance_id: inst_id,
            record_revision: 5,
            updated_at_ms: 1000,
            sources,
            reduced: verij_types::agent::ReducedExecutionState {
                status: verij_types::agent::AgentStatus::NeedsInput,
                detail: Some("confirmation pending".to_string()),
                conversation_id: Some(verij_types::agent::ConversationId("conv-1".to_string())),
                conversation_title: Some("Pi Session".to_string()),
                current_turn_id: Some(verij_types::agent::TurnId("run-1".to_string())),
                current_turn_epoch: 1,
                current_turn_revision: 5,
                pending_requests: vec![verij_types::agent::PendingRequest {
                    id: "req-1".to_string(),
                    kind: verij_types::agent::RequestKind::Confirmation,
                    detail: None,
                }],
                background_task_count: 2,
                latest_completion: Some(verij_types::agent::SuccessfulCompletion {
                    turn_id: Some(verij_types::agent::TurnId("run-1".to_string())),
                    turn_epoch: 1,
                    turn_revision: 5,
                    completed_at_ms: 1000,
                    summary: None,
                }),
                latest_error: None,
                completion_revision: 1,
            },
            completion_revision: 1,
            completed_turn: None,
            completed_turn_epoch: 0,
            completed_turn_revision: 0,
            opencode_completions: Default::default(),
            adapter_state,
        };
        std::fs::write(
            inst_dir.join("state.json"),
            serde_json::to_string(&state).unwrap(),
        )
        .unwrap();

        let records = read_records(dir.path());
        assert_eq!(records.len(), 1, "expired lease row must be preserved");
        let rec = &records[0];
        assert_eq!(rec.identity.agent_instance_id.0, instance_name);
        assert_eq!(
            rec.state.reduced.status,
            verij_types::agent::AgentStatus::Unknown
        );
        assert_eq!(
            rec.state.reduced.detail,
            Some("Pi reporter unavailable".into())
        );
        assert!(
            rec.state.reduced.pending_requests.is_empty(),
            "pending requests cleared"
        );
        assert_eq!(
            rec.state.reduced.background_task_count, 0,
            "background tasks cleared"
        );
        assert!(
            rec.state.reduced.latest_completion.is_none(),
            "completion cleared"
        );
        assert!(rec.state.reduced.latest_error.is_none(), "error cleared");

        // Verify adapter_state["pi"] is preserved
        let adapter_val = rec
            .state
            .adapter_state
            .get("pi")
            .expect("pi adapter_state preserved");
        assert_eq!(adapter_val["running_version"], "1.2.0");
        assert_eq!(adapter_val["runtime_id"], "rt-1");
        assert_eq!(adapter_val["completions"]["(\"conv-1\",\"run-1\")"], 1);
    }
}
