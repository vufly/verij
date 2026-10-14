//! Cached presentation hints and bounded passive acknowledgement work.
use anyhow::{bail, Result};
use std::collections::BTreeMap;
use std::io::Read;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    mpsc, Arc, Mutex,
};
use std::time::{Duration, Instant};
use verij_types::agent::{HostAckStore, InstanceAck, AGENT_SCHEMA_VERSION};
use verij_types::identity::{valid_record_key, AgentInstanceId, PaneKey};

type Candidate = (AgentInstanceId, PaneKey, u64);
type Outcome = (
    Option<PaneKey>,
    Option<BTreeMap<String, InstanceAck>>,
    Option<String>,
);
type Presentation = (
    Option<PaneKey>,
    Option<PaneKey>,
    Option<BTreeMap<String, InstanceAck>>,
);
const MIN_RETRY: Duration = Duration::from_secs(2);

pub struct Update {
    pub focus: Option<PaneKey>,
    pub acknowledgements: Option<BTreeMap<String, InstanceAck>>,
    pub error: Option<String>,
    pub generation: u64,
    pub successful: bool,
}

struct Job {
    generation: u64,
    candidates: Vec<Candidate>,
    hint: Option<crate::navigation::CachedFocus>,
}

#[derive(Default)]
struct Retry {
    failures: u8,
    after: Option<Instant>,
}

impl Retry {
    fn failed(&mut self, now: Instant) {
        self.failures = self.failures.saturating_add(1).min(5);
        self.after = Some(now + Duration::from_secs((2_u64 << (self.failures - 1)).min(30)));
    }
    fn reset(&mut self) {
        self.failures = 0;
        self.after = None;
    }
    fn ready(&self, now: Instant, last_attempt: Option<Instant>) -> bool {
        self.after.is_none_or(|after| now >= after)
            && last_attempt.is_none_or(|last| now.saturating_duration_since(last) >= MIN_RETRY)
    }
}

pub struct Monitor {
    host: String,
    signal: mpsc::SyncSender<()>,
    jobs: Arc<Mutex<Option<Job>>>,
    updates: Arc<Mutex<Option<Update>>>,
    generation: Arc<AtomicU64>,
    last_attempt: Arc<Mutex<Option<Instant>>>,
    hint: Option<crate::navigation::CachedFocus>,
    candidates: Vec<Candidate>,
    retry: Retry,
    require_hint: bool,
    pub pending: bool,
}

pub fn read_acknowledgements(host: &str) -> Result<BTreeMap<String, InstanceAck>> {
    if !valid_record_key(host) {
        bail!("invalid host acknowledgement key");
    }
    let path = crate::agent_store::resolve_hosts_dir()?.join(format!("{host}.json"));
    let file = match std::fs::File::open(path) {
        Ok(file) => file,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return Ok(BTreeMap::new()),
        Err(error) => return Err(error.into()),
    };
    let mut bytes = Vec::new();
    file.take(262_145).read_to_end(&mut bytes)?;
    if bytes.len() > 256 * 1024 {
        bail!("host acknowledgement record exceeds limit");
    }
    let store: HostAckStore = serde_json::from_slice(&bytes)?;
    if store.schema_version != AGENT_SCHEMA_VERSION || store.host_key.0 != host {
        bail!("host acknowledgement identity/schema mismatch");
    }
    Ok(store.acknowledgements)
}

impl Monitor {
    pub fn start(host: String) -> Self {
        let mut monitor = Self::start_with_runner(host, |host, candidates, active| {
            // No RPC can be produced by an empty or conflicting candidate set.
            if candidates.len() != 1 || !active() {
                return None;
            }
            let root = crate::navigation::resolve_control_dir();
            let (instance, expected_pane, revision) = &candidates[0];
            let Ok((identity, state)) = crate::agent_store::inspect(instance) else {
                return None;
            };
            if identity.pane_key != *expected_pane
                || state.reduced.status != verij_types::agent::AgentStatus::Done
                || state.reduced.completion_revision != *revision
            {
                return None;
            }
            let acknowledgements = read_acknowledgements(host).ok();
            if acknowledgements.as_ref().is_some_and(|acks| {
                acks.get(&instance.0)
                    .is_some_and(|ack| ack.acknowledged_revision >= *revision)
            }) {
                return Some((None, acknowledgements, None));
            }
            let mut error = None;
            let mut focus = None;
            match crate::navigation::verified_visit_if_current(&root, host, active) {
                Some(visit) if visit.pane() == *expected_pane => {
                    focus = Some(visit.pane());
                    if let Err(failure) = crate::agent_cli::acknowledge_verified_visit_if_current(
                        crate::agent_cli::AckArgs {
                            instance: instance.0.clone(),
                            host: host.into(),
                            revision: *revision,
                        },
                        visit,
                        &identity,
                        active,
                    ) {
                        error = Some(format!("Visit remains unverified: {failure}"));
                    }
                }
                _ => error = Some("Visit remains unverified".into()),
            }
            Some((focus, read_acknowledgements(host).ok(), error))
        });
        monitor.require_hint = true;
        monitor
    }

    /// Cheap heartbeat observations update presentation and invalidate work; none
    /// of these cached values establish a visit or consume an unread completion.
    pub fn observe(&mut self) -> Presentation {
        let hint =
            crate::navigation::cached_focus(&crate::navigation::resolve_control_dir(), &self.host);
        if hint != self.hint {
            self.invalidate();
            self.hint = hint;
        }
        (
            self.hint
                .as_ref()
                .and_then(|hint| hint.eligible_pane())
                .cloned(),
            self.hint.as_ref().and_then(|hint| hint.pane.clone()),
            read_acknowledgements(&self.host).ok(),
        )
    }

    pub fn eligible_pane(&self) -> Option<&PaneKey> {
        self.hint.as_ref().and_then(|hint| hint.eligible_pane())
    }

    fn start_with_runner<F>(host: String, runner: F) -> Self
    where
        F: Fn(&str, &[Candidate], &dyn Fn() -> bool) -> Option<Outcome> + Send + 'static,
    {
        let (signal, receiver) = mpsc::sync_channel(1);
        let jobs: Arc<Mutex<Option<Job>>> = Arc::new(Mutex::new(None));
        let updates = Arc::new(Mutex::new(None));
        let generation = Arc::new(AtomicU64::new(0));
        let last_attempt: Arc<Mutex<Option<Instant>>> = Arc::new(Mutex::new(None));
        let worker_jobs = jobs.clone();
        let worker_updates = updates.clone();
        let worker_gen = generation.clone();
        let worker_attempt = last_attempt.clone();
        let worker_host = host.clone();
        std::thread::spawn(move || {
            while receiver.recv().is_ok() {
                let Some(job) = worker_jobs.lock().unwrap().take() else {
                    continue;
                };
                let active = || {
                    worker_gen.load(Ordering::Acquire) == job.generation
                        && job.hint.as_ref().is_none_or(|hint| {
                            crate::navigation::cached_focus(
                                &crate::navigation::resolve_control_dir(),
                                &worker_host,
                            )
                            .as_ref()
                                == Some(hint)
                        })
                };
                // Enqueue time is not proof-start time. Enforce the minimum at the
                // actual worker boundary, including replacement jobs behind a slow RPC.
                while worker_gen.load(Ordering::Acquire) == job.generation
                    && worker_attempt
                        .lock()
                        .unwrap()
                        .is_some_and(|last| last.elapsed() < MIN_RETRY)
                {
                    std::thread::sleep(Duration::from_millis(25));
                }
                let outcome = if active() && !job.candidates.is_empty() {
                    *worker_attempt.lock().unwrap() = Some(Instant::now());
                    let outcome = runner(&worker_host, &job.candidates, &active);
                    // A slow metadata read must not move the real proof start far
                    // past the scheduling timestamp and permit a following burst.
                    *worker_attempt.lock().unwrap() = Some(Instant::now());
                    outcome
                } else {
                    None
                };
                if worker_gen.load(Ordering::Acquire) != job.generation {
                    continue;
                }
                let (mut focus, acknowledgements, error) = outcome.unwrap_or((None, None, None));
                if !active() {
                    focus = None;
                }
                let successful = acknowledgements.as_ref().is_some_and(|acks| {
                    job.candidates.iter().all(|(instance, _, revision)| {
                        acks.get(&instance.0)
                            .is_some_and(|ack| ack.acknowledged_revision >= *revision)
                    })
                });
                *worker_updates.lock().unwrap() = Some(Update {
                    focus,
                    acknowledgements,
                    error,
                    generation: job.generation,
                    successful,
                });
            }
        });
        Self {
            host,
            signal,
            jobs,
            updates,
            generation,
            last_attempt,
            hint: None,
            candidates: Vec::new(),
            retry: Retry::default(),
            require_hint: false,
            pending: false,
        }
    }

    pub fn request(&mut self, mut candidates: Vec<Candidate>) -> bool {
        candidates.retain(|(_, pane, revision)| {
            *revision > 0 && (!self.require_hint || self.eligible_pane() == Some(pane))
        });
        candidates.sort_by(|a, b| a.0.cmp(&b.0));
        candidates.dedup();
        if candidates != self.candidates {
            self.invalidate();
            self.candidates = candidates;
        }
        if self.candidates.len() != 1
            || self.pending
            || !self
                .retry
                .ready(Instant::now(), *self.last_attempt.lock().unwrap())
        {
            return false;
        }
        *self.jobs.lock().unwrap() = Some(Job {
            generation: self.generation.load(Ordering::Acquire),
            candidates: self.candidates.clone(),
            hint: self.hint.clone(),
        });
        self.pending = true;
        match self.signal.try_send(()) {
            Ok(()) | Err(mpsc::TrySendError::Full(_)) => true,
            Err(mpsc::TrySendError::Disconnected(_)) => {
                self.pending = false;
                false
            }
        }
    }

    pub fn take_update(&mut self) -> Option<Update> {
        let update = self.updates.lock().unwrap().take()?;
        if update.generation != self.generation.load(Ordering::Acquire) {
            return None;
        }
        self.pending = false;
        if update.successful {
            self.retry.reset();
        } else {
            self.retry.failed(Instant::now());
        }
        Some(update)
    }

    pub fn invalidate(&mut self) {
        self.generation.fetch_add(1, Ordering::AcqRel);
        self.pending = false;
        *self.jobs.lock().unwrap() = None;
        self.retry.reset();
    }
}

impl Drop for Monitor {
    fn drop(&mut self) {
        self.invalidate();
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use std::sync::atomic::{AtomicBool, AtomicUsize};
    use std::sync::Barrier;
    use verij_types::identity::{SessionInstanceId, TerminalPaneId};

    fn candidate(revision: u64) -> Vec<Candidate> {
        vec![(
            AgentInstanceId("instance".into()),
            PaneKey {
                session: SessionInstanceId("session".into()),
                terminal: TerminalPaneId(0),
            },
            revision,
        )]
    }

    #[test]
    fn empty_candidates_never_schedule_runner_or_proof() {
        let calls = Arc::new(AtomicUsize::new(0));
        let worker_calls = calls.clone();
        let mut monitor = Monitor::start_with_runner("host".into(), move |_, _, _| {
            worker_calls.fetch_add(1, Ordering::Relaxed);
            None
        });
        for _ in 0..1000 {
            assert!(!monitor.request(vec![]));
        }
        assert!(!monitor.pending);
        assert_eq!(calls.load(Ordering::Relaxed), 0);
    }

    #[test]
    fn missing_focus_zero_revision_and_conflicting_owners_do_not_start_native_work() {
        let mut monitor =
            Monitor::start_with_runner("host".into(), |_, _, _| panic!("ineligible job"));
        monitor.require_hint = true;
        assert!(!monitor.request(candidate(1))); // No current verified binding hint.
        monitor.require_hint = false;
        let mut conflicting = candidate(1);
        let mut other = conflicting[0].clone();
        other.0 = AgentInstanceId("conflicting-owner".into());
        conflicting.push(other);
        assert!(!monitor.request(conflicting));
        assert!(!monitor.request(candidate(0)));
        assert!(!monitor.pending);
    }

    #[test]
    fn retries_back_off_to_thirty_seconds_and_changes_keep_minimum_attempt_interval() {
        let now = Instant::now();
        let mut retry = Retry::default();
        for seconds in [2, 4, 8, 16, 30, 30] {
            retry.failed(now);
            assert!(!retry.ready(
                now + Duration::from_secs(seconds) - Duration::from_millis(1),
                None
            ));
            assert!(retry.ready(now + Duration::from_secs(seconds), None));
        }
        retry.reset();
        assert!(!retry.ready(now + Duration::from_millis(1999), Some(now)));
        assert!(retry.ready(now + MIN_RETRY, Some(now)));
    }

    #[test]
    fn invalidation_and_drop_suppress_in_flight_acknowledgement() {
        for drop_monitor in [false, true] {
            let enter = Arc::new(Barrier::new(2));
            let leave = Arc::new(Barrier::new(2));
            let acked = Arc::new(AtomicBool::new(false));
            let (e, l, a) = (enter.clone(), leave.clone(), acked.clone());
            let mut monitor = Monitor::start_with_runner("host".into(), move |_, _, active| {
                e.wait();
                l.wait();
                a.store(active(), Ordering::Release);
                Some((None, None, None))
            });
            assert!(monitor.request(candidate(1)));
            enter.wait();
            monitor.invalidate();
            assert!(!monitor.pending);
            if drop_monitor {
                drop(monitor);
            }
            leave.wait();
            std::thread::sleep(Duration::from_millis(25));
            assert!(!acked.load(Ordering::Acquire));
        }
    }

    #[test]
    fn burst_of_replacements_keeps_one_pending_job_and_one_worker() {
        let enter = Arc::new(Barrier::new(2));
        let leave = Arc::new(Barrier::new(2));
        let seen = Arc::new(Mutex::new(Vec::new()));
        let (e, l, s) = (enter.clone(), leave.clone(), seen.clone());
        let mut monitor =
            Monitor::start_with_runner("host".into(), move |_, candidates, active| {
                let revision = candidates[0].2;
                s.lock().unwrap().push(revision);
                if revision == 1 {
                    e.wait();
                    l.wait();
                }
                active().then_some((None, None, None))
            });
        assert!(monitor.request(candidate(1)));
        enter.wait();
        // Simulate a slow native sequence: replacement work becomes eligible
        // while the old worker still owns the sole execution slot.
        std::thread::sleep(MIN_RETRY);
        for revision in 2..100 {
            assert!(monitor.request(candidate(revision)));
        }
        assert_eq!(
            monitor.jobs.lock().unwrap().as_ref().unwrap().candidates[0].2,
            99
        );
        leave.wait();
        let end = Instant::now() + Duration::from_secs(3);
        while monitor.take_update().is_none() && Instant::now() < end {
            std::thread::sleep(Duration::from_millis(5));
        }
        assert_eq!(*seen.lock().unwrap(), vec![1, 99]);
        assert!(!monitor.pending);
    }
}
