//! Legacy session/tab activation worker. Keeps terminal input/render unblocked.
//! Supersession is local; dispatched stock actions cannot be revoked.
use std::sync::{
    atomic::{AtomicU64, Ordering},
    mpsc, Arc,
};

pub struct Activation {
    pub ticket: u64,
    pub old_session: Option<String>,
    pub session: String,
    pub tab: Option<usize>,
    pub title: Option<String>,
}

pub struct Finished {
    pub request: Activation,
    pub result: Result<(), String>,
}

pub struct Queue {
    sender: mpsc::Sender<Activation>,
    latest: Arc<AtomicU64>,
    pub receiver: mpsc::Receiver<Finished>,
    pub pending: bool,
}

impl Queue {
    pub fn start() -> Self {
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
                let old =
                    crate::actions::workspace_session().or_else(|| request.old_session.clone());
                let result = crate::actions::switch_session_if_current(
                    old.as_deref(),
                    &request.session,
                    request.tab,
                    request.title.as_deref(),
                    active,
                )
                .map_err(|error| error.to_string());
                if results.send(Finished { request, result }).is_err() {
                    break;
                }
            }
        });
        Self {
            sender,
            latest,
            receiver,
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
        })?;
        self.pending = true;
        Ok(())
    }

    pub fn is_current(&self, ticket: u64) -> bool {
        self.latest.load(Ordering::Acquire) == ticket
    }
}

impl Drop for Queue {
    fn drop(&mut self) {
        // Quitting the sidebar invalidates any locally pending follow-up.
        // A stock action already dispatched remains outside our control.
        self.latest.fetch_add(1, Ordering::AcqRel);
    }
}
