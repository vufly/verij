//! Legacy session/tab activation worker. Keeps terminal input/render unblocked.
//! Supersession is local; dispatched stock actions cannot be revoked.
use std::sync::{
    atomic::{AtomicU64, Ordering},
    mpsc, Arc,
};
use verij_types::agent::InstanceAck;
use verij_types::identity::{AgentInstanceId, PaneKey};

pub struct AgentTarget {
    pub host: String,
    pub instance: AgentInstanceId,
    pub pane: PaneKey,
    pub revision: Option<u64>,
}

pub enum Outcome {
    Legacy,
    Agent {
        session: String,
        pane: PaneKey,
        ack: Option<InstanceAck>,
        instance: AgentInstanceId,
    },
}

pub struct Activation {
    pub ticket: u64,
    pub old_session: Option<String>,
    pub session: String,
    pub tab: Option<usize>,
    pub title: Option<String>,
    pub agent: Option<AgentTarget>,
}

pub struct Finished {
    pub request: Activation,
    pub result: Result<Outcome, String>,
}

pub struct Focused {
    pub ticket: u64,
    pub session: String,
    pub pane: PaneKey,
}

pub struct Queue {
    sender: mpsc::Sender<Activation>,
    latest: Arc<AtomicU64>,
    pub receiver: mpsc::Receiver<Finished>,
    pub focus_receiver: Option<mpsc::Receiver<Focused>>,
    pub pending: bool,
}

impl Queue {
    pub fn start() -> Self {
        let (focus_sender, focus_receiver) = mpsc::channel();
        let mut queue = Self::start_with_executor(move |request, active| {
            if let Some(target) = request.agent.as_ref() {
                crate::navigation::activate_instance(
                    &crate::navigation::resolve_control_dir(),
                    &target.host,
                    &target.instance,
                    &target.pane,
                    target.revision,
                    active,
                    |session, pane| {
                        if active() {
                            let _ = focus_sender.send(Focused {
                                ticket: request.ticket,
                                session: session.to_string(),
                                pane: pane.clone(),
                            });
                        }
                    },
                )
                .and_then(|(result, ack)| {
                    let session = result
                        .observation
                        .and_then(|observation| observation.session_name)
                        .ok_or_else(|| anyhow::anyhow!("confirmed agent location unavailable"))?;
                    Ok(Outcome::Agent {
                        session,
                        pane: target.pane.clone(),
                        ack,
                        instance: target.instance.clone(),
                    })
                })
                .map_err(|error| format!("{error:#}"))
            } else {
                let old =
                    crate::actions::workspace_session().or_else(|| request.old_session.clone());
                crate::actions::switch_session_if_current(
                    old.as_deref(),
                    &request.session,
                    request.tab,
                    request.title.as_deref(),
                    active,
                )
                .map(|()| Outcome::Legacy)
                .map_err(|error| format!("{error:#}"))
            }
        });
        queue.focus_receiver = Some(focus_receiver);
        queue
    }

    pub(crate) fn start_with_executor<F>(executor: F) -> Self
    where
        F: Fn(&Activation, &dyn Fn() -> bool) -> Result<Outcome, String> + Send + 'static,
    {
        let (sender, requests) = mpsc::channel::<Activation>();
        let (results, receiver) = mpsc::channel();
        let latest = Arc::new(AtomicU64::new(0));
        let current = latest.clone();
        std::thread::spawn(move || {
            while let Ok(mut request) = requests.recv() {
                while let Ok(newer) = requests.try_recv() {
                    request = newer;
                }
                let active = || current.load(Ordering::Acquire) == request.ticket;
                if !active() {
                    continue;
                }
                let result = executor(&request, &active);
                if !active() {
                    continue;
                }
                if results.send(Finished { request, result }).is_err() {
                    break;
                }
            }
        });
        Self {
            sender,
            latest,
            receiver,
            focus_receiver: None,
            pending: false,
        }
    }

    pub fn enqueue(
        &mut self,
        old_session: Option<String>,
        session: String,
        tab: Option<usize>,
        title: Option<String>,
    ) -> anyhow::Result<()> {
        let ticket = self.latest.fetch_add(1, Ordering::AcqRel) + 1;
        self.sender.send(Activation {
            ticket,
            old_session,
            session,
            tab,
            title,
            agent: None,
        })?;
        self.pending = true;
        Ok(())
    }

    pub fn is_current(&self, ticket: u64) -> bool {
        self.latest.load(Ordering::Acquire) == ticket
    }

    pub fn take_focus(&self) -> Option<Focused> {
        let receiver = self.focus_receiver.as_ref()?;
        while let Ok(update) = receiver.try_recv() {
            if self.is_current(update.ticket) {
                return Some(update);
            }
        }
        None
    }

    pub fn enqueue_agent(&mut self, session: String, target: AgentTarget) -> anyhow::Result<()> {
        let ticket = self.latest.fetch_add(1, Ordering::AcqRel) + 1;
        self.sender.send(Activation {
            ticket,
            old_session: None,
            session,
            tab: None,
            title: None,
            agent: Some(target),
        })?;
        self.pending = true;
        Ok(())
    }

    pub fn invalidate(&mut self) {
        self.latest.fetch_add(1, Ordering::AcqRel);
        self.pending = false;
    }
}

impl Drop for Queue {
    fn drop(&mut self) {
        // Quitting the sidebar invalidates any locally pending follow-up.
        // A stock action already dispatched remains outside our control.
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
    fn confirmed_focus_arrives_before_completion_and_supersession_discards_old_updates() {
        let (sender, receiver) = mpsc::channel();
        let worker_sender = sender.clone();
        let barrier = Arc::new(Barrier::new(2));
        let worker_barrier = barrier.clone();
        let pane = PaneKey { session: verij_types::identity::SessionInstanceId("server".into()),
            terminal: verij_types::identity::TerminalPaneId(1) };
        let worker_pane = pane.clone();
        let mut queue = Queue::start_with_executor(move |request, _| {
            worker_sender.send(Focused { ticket: request.ticket, session: "inner".into(), pane: worker_pane.clone() }).unwrap();
            worker_barrier.wait();
            worker_barrier.wait();
            Ok(Outcome::Legacy)
        });
        queue.focus_receiver = Some(receiver);
        queue.enqueue(None, "inner".into(), None, None).unwrap();
        barrier.wait();
        assert_eq!(queue.take_focus().unwrap().pane, pane);
        assert!(queue.pending);
        assert!(queue.receiver.try_recv().is_err(), "completion still waits on visit work");
        queue.invalidate();
        sender.send(Focused { ticket: 1, session: "inner".into(), pane }).unwrap();
        assert!(queue.take_focus().is_none());
        barrier.wait();
    }

    #[test]
    fn barrier_controlled_two_intents_suppress_older_followup_and_result() {
        let barrier_enter = Arc::new(Barrier::new(2));
        let barrier_exit = Arc::new(Barrier::new(2));
        let first_marked = Arc::new(AtomicBool::new(false));
        let second_marked = Arc::new(AtomicBool::new(false));

        let b_enter = barrier_enter.clone();
        let b_exit = barrier_exit.clone();
        let f_marked = first_marked.clone();
        let s_marked = second_marked.clone();

        let mut queue = Queue::start_with_executor(move |request, active| {
            if request.ticket == 1 {
                b_enter.wait();
                b_exit.wait();
                if active() {
                    f_marked.store(true, Ordering::Release);
                }
                Ok(Outcome::Legacy)
            } else {
                if active() {
                    s_marked.store(true, Ordering::Release);
                }
                Ok(Outcome::Legacy)
            }
        });

        queue.enqueue(None, "session-1".into(), None, None).unwrap();
        barrier_enter.wait();

        // Enqueue second intent while first is blocked in worker executor
        queue.enqueue(None, "session-2".into(), None, None).unwrap();
        barrier_exit.wait();

        let finished = queue
            .receiver
            .recv_timeout(Duration::from_secs(2))
            .expect("must receive second intent result");
        assert_eq!(finished.request.ticket, 2);
        assert_eq!(finished.request.session, "session-2");

        // Older follow-up/marking suppressed
        assert!(!first_marked.load(Ordering::Acquire));
        assert!(second_marked.load(Ordering::Acquire));

        // Older result suppressed from channel
        assert!(queue.receiver.try_recv().is_err());
    }

    #[test]
    fn queued_coalesce_latest() {
        let barrier_enter = Arc::new(Barrier::new(2));
        let barrier_exit = Arc::new(Barrier::new(2));
        let b_enter = barrier_enter.clone();
        let b_exit = barrier_exit.clone();
        let executed_tickets = Arc::new(std::sync::Mutex::new(Vec::new()));
        let exec_tickets = executed_tickets.clone();

        let mut queue = Queue::start_with_executor(move |request, _active| {
            if request.ticket == 1 {
                b_enter.wait();
                b_exit.wait();
            }
            exec_tickets.lock().unwrap().push(request.ticket);
            Ok(Outcome::Legacy)
        });

        // Enqueue ticket 1 to hold worker
        queue.enqueue(None, "session-1".into(), None, None).unwrap();

        // Wait until worker is actively executing ticket 1
        barrier_enter.wait();

        // Enqueue multiple while ticket 1 is held
        queue.enqueue(None, "session-2".into(), None, None).unwrap();
        queue.enqueue(None, "session-3".into(), None, None).unwrap();
        queue.enqueue(None, "session-4".into(), None, None).unwrap();

        // Release ticket 1
        barrier_exit.wait();

        // Ticket 4 result should be received
        let finished = queue
            .receiver
            .recv_timeout(Duration::from_secs(2))
            .expect("must receive latest coalesced result");
        assert_eq!(finished.request.ticket, 4);

        let executed = executed_tickets.lock().unwrap().clone();
        // Ticket 1 ran, tickets 2 and 3 were coalesced without executing, ticket 4 ran
        assert_eq!(executed, vec![1, 4]);
    }

    #[test]
    fn drop_invalidates_current_ack_allowed_no_later_host_marking() {
        let barrier_enter = Arc::new(Barrier::new(2));
        let barrier_exit = Arc::new(Barrier::new(2));
        let marked = Arc::new(AtomicBool::new(false));

        let b_enter = barrier_enter.clone();
        let b_exit = barrier_exit.clone();
        let m = marked.clone();

        let mut queue = Queue::start_with_executor(move |_request, active| {
            b_enter.wait();
            b_exit.wait();
            if active() {
                m.store(true, Ordering::Release);
            }
            Ok(Outcome::Legacy)
        });

        queue.enqueue(None, "session-1".into(), None, None).unwrap();
        barrier_enter.wait();

        // Drop queue while worker is in-flight before marking
        drop(queue);
        barrier_exit.wait();

        std::thread::sleep(Duration::from_millis(50));
        assert!(!marked.load(Ordering::Acquire));
    }
}
