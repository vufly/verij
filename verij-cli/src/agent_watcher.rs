//! Agent updates are independent of slower Zellij topology discovery.
use anyhow::Result;
use notify::{RecursiveMode, Watcher};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::Arc;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;
use verij_types::agent::{AgentIdentity, AgentState, AGENT_SCHEMA_VERSION};
use verij_types::identity::valid_record_key;

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AgentRecord {
    pub identity: AgentIdentity,
    pub state: AgentState,
}

pub fn read_records(root: &Path) -> Vec<AgentRecord> {
    let mut records = Vec::new();
    let Ok(entries) = std::fs::read_dir(root) else {
        return records;
    };
    for entry in entries.flatten() {
        let name = entry.file_name();
        let Some(name) = name.to_str().filter(|id| valid_record_key(id)) else {
            continue;
        };
        let directory = entry.path();
        if !directory.is_dir() {
            continue;
        }
        let Ok(identity) = std::fs::read(directory.join("identity.json")) else {
            continue;
        };
        let Ok(state) = std::fs::read(directory.join("state.json")) else {
            continue;
        };
        if identity.len() > 64 * 1024 || state.len() > 256 * 1024 {
            continue;
        }
        let Ok(identity) = serde_json::from_slice::<AgentIdentity>(&identity) else {
            continue;
        };
        let Ok(mut state) = serde_json::from_slice::<AgentState>(&state) else {
            continue;
        };
        if identity.schema_version != AGENT_SCHEMA_VERSION
            || state.schema_version != AGENT_SCHEMA_VERSION
            || identity.agent_instance_id.0 != name
            || identity.agent_instance_id != state.agent_instance_id
        {
            continue;
        }
        if !identity.is_synthetic && !crate::process::is_alive(&identity.process) {
            continue;
        }
        if crate::opencode::lease_expired(&state, crate::agent_store::now_ms()) {
            state.reduced.status = verij_types::agent::AgentStatus::Unknown;
            state.reduced.pending_requests.clear();
            state.reduced.latest_completion = None;
            state.reduced.latest_error = None;
            state.reduced.detail = Some("OpenCode reporter unavailable".into());
        }
        records.push(AgentRecord { identity, state });
    }
    records.sort_by(|a, b| {
        a.identity
            .agent_instance_id
            .cmp(&b.identity.agent_instance_id)
    });
    records
}

/// Owned handle to a running agent watcher thread.
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

pub fn spawn(tx: mpsc::Sender<Vec<AgentRecord>>) -> Result<WatcherHandle> {
    let root = crate::agent_store::resolve_v1_dir()?;
    spawn_inner(root, tx)
}

pub fn spawn_inner(root: PathBuf, tx: mpsc::Sender<Vec<AgentRecord>>) -> Result<WatcherHandle> {
    let stop = Arc::new(AtomicBool::new(false));
    let stop_thread = stop.clone();

    let thread = std::thread::Builder::new()
        .name("verij-agent-watcher".to_string())
        .spawn(move || {
            let (events, receiver) = std::sync::mpsc::channel();
            let mut watcher =
                notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
                    if event.is_ok_and(|event| {
                        matches!(
                            event.kind,
                            notify::EventKind::Create(_)
                                | notify::EventKind::Modify(_)
                                | notify::EventKind::Remove(_)
                        )
                    }) {
                        let _ = events.send(());
                    }
                })
                .ok();
            if let Some(watcher) = watcher.as_mut() {
                let _ = watcher.watch(&root, RecursiveMode::Recursive);
            }

            const FALLBACK_INTERVAL: Duration = Duration::from_millis(1000);
            let mut previous: Option<Vec<AgentRecord>> = None;
            let mut pending_send: Option<Vec<AgentRecord>> = None;
            let mut next_poll = Instant::now() + FALLBACK_INTERVAL;

            // Initial read
            let records = read_records(&root);
            match tx.try_send(records.clone()) {
                Ok(()) => {
                    previous = Some(records);
                }
                Err(mpsc::error::TrySendError::Closed(_)) => return,
                Err(mpsc::error::TrySendError::Full(r)) => {
                    pending_send = Some(r);
                }
            }

            while !tx.is_closed() && !stop_thread.load(Ordering::Relaxed) {
                // Retry pending send if channel was previously full (solves #7)
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
                let poll_due = now >= next_poll;

                let time_until_poll = next_poll.saturating_duration_since(now);
                let slice_timeout = Duration::from_millis(50)
                    .min(time_until_poll)
                    .max(Duration::from_millis(1));

                let mut should_read = poll_due;
                match receiver.recv_timeout(slice_timeout) {
                    Ok(()) => {
                        // Coalesce rapid consecutive notifications with bounded window (up to 50ms)
                        let coalesce_deadline = Instant::now() + Duration::from_millis(50);
                        while Instant::now() < coalesce_deadline
                            && !tx.is_closed()
                            && !stop_thread.load(Ordering::Relaxed)
                        {
                            if receiver.recv_timeout(Duration::from_millis(10)).is_err() {
                                break;
                            }
                        }
                        should_read = true;
                    }
                    Err(std::sync::mpsc::RecvTimeoutError::Timeout) => {
                        if Instant::now() >= next_poll {
                            should_read = true;
                        }
                    }
                    Err(std::sync::mpsc::RecvTimeoutError::Disconnected) => {
                        break;
                    }
                }

                if tx.is_closed() || stop_thread.load(Ordering::Relaxed) {
                    break;
                }

                if should_read {
                    next_poll = Instant::now() + FALLBACK_INTERVAL;
                    let records = read_records(&root);
                    if previous.as_ref() != Some(&records) || pending_send.is_some() {
                        match tx.try_send(records.clone()) {
                            Ok(()) => {
                                previous = Some(records);
                                pending_send = None;
                            }
                            Err(mpsc::error::TrySendError::Closed(_)) => break,
                            Err(mpsc::error::TrySendError::Full(r)) => {
                                pending_send = Some(r);
                            }
                        }
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
}
