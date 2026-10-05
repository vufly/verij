//! Native focus observation and acknowledgement run outside input/render.
use anyhow::{bail, Result};
use std::collections::BTreeMap;
use std::io::Read;
use std::sync::{
    atomic::{AtomicU64, Ordering},
    mpsc, Arc,
};
use verij_types::agent::{HostAckStore, InstanceAck, AGENT_SCHEMA_VERSION};
use verij_types::identity::{valid_record_key, AgentInstanceId, PaneKey, SessionInstanceId};

pub struct Update {
    pub focus: Option<PaneKey>,
    pub acknowledgements: Option<BTreeMap<String, InstanceAck>>,
    pub error: Option<String>,
    pub generation: u64,
}

struct Job {
    generation: u64,
    candidates: Vec<(AgentInstanceId, PaneKey, u64)>,
}

pub struct Monitor {
    sender: mpsc::Sender<Job>,
    pub receiver: mpsc::Receiver<Update>,
    generation: Arc<AtomicU64>,
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
        Self::start_with_runner(host, |host, candidates, active| {
            let root = crate::navigation::resolve_control_dir();
            // Cached public focus is a negative polling hint only. A positive
            // hint still requires the complete fresh native proof below.
            let likely_workspace = crate::navigation::load_binding(&root, host)
                .ok()
                .is_some_and(|binding| {
                    std::fs::read_dir(&root).ok().is_some_and(|entries| {
                        entries.flatten().any(|entry| {
                            entry.file_name().to_string_lossy().starts_with("focus-")
                                && std::fs::read(entry.path())
                                    .ok()
                                    .and_then(|bytes| {
                                        serde_json::from_slice::<
                                            verij_types::control::FocusObservation,
                                        >(&bytes)
                                        .ok()
                                    })
                                    .is_some_and(|observation| {
                                        observation.session_name.as_deref()
                                            == Some(&binding.host_session)
                                            && observation.terminal == Some(binding.workspace_pane)
                                            && crate::agent_store::now_ms()
                                                .saturating_sub(observation.observed_at_ms)
                                                < 1000
                                    })
                        })
                    })
                });

            let focus = if likely_workspace {
                if !active() {
                    return None;
                }
                let visit = crate::navigation::verified_visit(&root, host);
                if !active() {
                    return None;
                }
                visit.and_then(|(result, _)| {
                    let terminal = result.observation?.terminal?;
                    let binding = crate::navigation::load_binding(&root, host).ok()?;
                    Some(PaneKey {
                        session: SessionInstanceId::from_process(&binding.server_process),
                        terminal,
                    })
                })
            } else {
                None
            };

            let mut error = None;
            if let Some(focused) = focus.as_ref() {
                for (instance, pane, revision) in
                    candidates.iter().filter(|(_, pane, _)| pane == focused)
                {
                    let _ = pane;
                    if !active() {
                        return None;
                    }
                    // Failures preserve Done; only an actual native visit can
                    // acknowledge the revision captured by this sidebar.
                    if let Err(failure) = crate::agent_cli::acknowledge_visit_if_current(
                        crate::agent_cli::AckArgs {
                            instance: instance.0.clone(),
                            host: host.to_string(),
                            revision: *revision,
                        },
                        active,
                    ) {
                        error = Some(format!("Visit remains unverified: {failure}"));
                    }
                }
            }

            if !active() {
                return None;
            }

            let acknowledgements = match read_acknowledgements(host) {
                Ok(value) => Some(value),
                Err(failure) => {
                    error = Some(format!("Acknowledgement store: {failure}"));
                    None
                }
            };

            if !active() {
                return None;
            }

            Some((focus, acknowledgements, error))
        })
    }

    pub(crate) fn start_with_runner<F>(host: String, runner: F) -> Self
    where
        F: Fn(
                &str,
                &[(AgentInstanceId, PaneKey, u64)],
                &dyn Fn() -> bool,
            ) -> Option<(
                Option<PaneKey>,
                Option<BTreeMap<String, InstanceAck>>,
                Option<String>,
            )> + Send
            + 'static,
    {
        let (sender, jobs) = mpsc::channel::<Job>();
        let (updates, receiver) = mpsc::channel();
        let generation = Arc::new(AtomicU64::new(0));
        let worker_gen = generation.clone();
        std::thread::spawn(move || {
            while let Ok(mut job) = jobs.recv() {
                while let Ok(newer) = jobs.try_recv() {
                    job = newer;
                }
                let active = {
                    let gen = worker_gen.clone();
                    let job_gen = job.generation;
                    move || gen.load(Ordering::Acquire) == job_gen
                };
                if !active() {
                    continue;
                }
                if let Some((focus, acknowledgements, error)) =
                    runner(&host, &job.candidates, &active)
                {
                    if !active() {
                        continue;
                    }
                    if updates
                        .send(Update {
                            focus,
                            acknowledgements,
                            error,
                            generation: job.generation,
                        })
                        .is_err()
                    {
                        break;
                    }
                }
            }
        });
        Self {
            sender,
            receiver,
            generation,
            pending: false,
        }
    }

    pub fn request(&mut self, candidates: Vec<(AgentInstanceId, PaneKey, u64)>) -> bool {
        if self.pending {
            return false;
        }
        let generation = self.generation.load(Ordering::Acquire);
        self.pending = self
            .sender
            .send(Job {
                generation,
                candidates,
            })
            .is_ok();
        self.pending
    }

    pub fn take_update(&mut self) -> Option<Update> {
        let current = self.generation.load(Ordering::Acquire);
        while let Ok(update) = self.receiver.try_recv() {
            if update.generation == current {
                self.pending = false;
                return Some(update);
            }
        }
        None
    }

    pub fn invalidate(&mut self) {
        self.generation.fetch_add(1, Ordering::AcqRel);
        self.pending = false;
    }

    #[allow(dead_code)]
    pub fn generation(&self) -> u64 {
        self.generation.load(Ordering::Acquire)
    }

    #[allow(dead_code)]
    pub fn is_pending(&self) -> bool {
        self.pending
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
    use std::sync::atomic::AtomicBool;
    use std::sync::Barrier;
    use std::time::Duration;

    #[test]
    fn invalidate_clears_pending_and_bumps_generation() {
        let mut monitor =
            Monitor::start_with_runner("host-a".into(), |_host, _candidates, _active| None);
        assert_eq!(monitor.generation(), 0);
        assert!(!monitor.is_pending());

        assert!(monitor.request(vec![]));
        assert!(monitor.is_pending());

        monitor.invalidate();
        assert_eq!(monitor.generation(), 1);
        assert!(!monitor.is_pending());
    }

    #[test]
    fn invalidate_suppresses_in_flight_ack_and_update() {
        let barrier_enter = Arc::new(Barrier::new(2));
        let barrier_exit = Arc::new(Barrier::new(2));
        let acked = Arc::new(AtomicBool::new(false));

        let b_enter = barrier_enter.clone();
        let b_exit = barrier_exit.clone();
        let a = acked.clone();

        let mut monitor =
            Monitor::start_with_runner("host-a".into(), move |_host, _candidates, active| {
                b_enter.wait();
                b_exit.wait();
                if active() {
                    a.store(true, Ordering::Release);
                    Some((None, Some(BTreeMap::new()), None))
                } else {
                    None
                }
            });

        assert!(monitor.request(vec![]));
        barrier_enter.wait();

        // Invalidate while worker is in-flight
        monitor.invalidate();
        barrier_exit.wait();

        std::thread::sleep(Duration::from_millis(50));
        assert!(!acked.load(Ordering::Acquire));
        assert!(monitor.take_update().is_none());
    }

    #[test]
    fn stale_update_discarded_after_fresher_generation_request() {
        let mut monitor =
            Monitor::start_with_runner("host-a".into(), move |_host, _candidates, _active| {
                Some((None, Some(BTreeMap::new()), None))
            });

        // Request job 0
        assert!(monitor.request(vec![]));

        // Invalidate bumps to gen 1 and clears pending
        monitor.invalidate();
        assert_eq!(monitor.generation(), 1);

        // Request job 1
        assert!(monitor.request(vec![]));

        // Wait for update; stale gen 0 update must be discarded, gen 1 accepted
        let mut update = None;
        for _ in 0..50 {
            if let Some(up) = monitor.take_update() {
                update = Some(up);
                break;
            }
            std::thread::sleep(Duration::from_millis(10));
        }

        let up = update.expect("must receive fresh generation update");
        assert_eq!(up.generation, 1);
    }

    #[test]
    fn drop_invalidates_in_flight_runner() {
        let barrier_enter = Arc::new(Barrier::new(2));
        let barrier_exit = Arc::new(Barrier::new(2));
        let acked = Arc::new(AtomicBool::new(false));

        let b_enter = barrier_enter.clone();
        let b_exit = barrier_exit.clone();
        let a = acked.clone();

        let mut monitor =
            Monitor::start_with_runner("host-a".into(), move |_host, _candidates, active| {
                b_enter.wait();
                b_exit.wait();
                if active() {
                    a.store(true, Ordering::Release);
                }
                None
            });

        assert!(monitor.request(vec![]));
        barrier_enter.wait();

        // Drop monitor while worker is waiting at barrier
        drop(monitor);
        barrier_exit.wait();

        std::thread::sleep(Duration::from_millis(50));
        assert!(!acked.load(Ordering::Acquire));
    }
}
