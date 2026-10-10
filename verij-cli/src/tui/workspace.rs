//! Workspace title inspection/renaming cannot block terminal input or rendering.
use std::sync::{
    atomic::{AtomicU64, Ordering},
    mpsc, Arc,
};
use std::time::{Duration, Instant};

pub struct Update {
    pub generation: u64,
    pub title: Option<String>,
    pub error: Option<String>,
    pub retry: bool,
}

pub struct Presentation {
    sender: mpsc::Sender<(u64, Option<String>)>,
    receiver: mpsc::Receiver<Update>,
    generation: Arc<AtomicU64>,
    requested: Option<Option<String>>,
    retry_after: Option<(Instant, Option<String>)>,
}

impl Presentation {
    pub fn start() -> Self {
        Self::start_with_executor(|title| {
            if let Some(title) = title {
                crate::actions::rename_workspace_pane(title).map(|()| Some(title.to_string()))
            } else {
                crate::actions::workspace_pane_title()
            }
        })
    }

    fn start_with_executor<F>(execute: F) -> Self
    where
        F: Fn(Option<&str>) -> anyhow::Result<Option<String>> + Send + 'static,
    {
        let (sender, jobs) = mpsc::channel::<(u64, Option<String>)>();
        let (updates, receiver) = mpsc::channel();
        let generation = Arc::new(AtomicU64::new(0));
        let current = generation.clone();
        std::thread::spawn(move || {
            while let Ok(mut job) = jobs.recv() {
                while let Ok(newer) = jobs.try_recv() {
                    job = newer;
                }
                if current.load(Ordering::Acquire) != job.0 {
                    continue;
                }
                let result = execute(job.1.as_deref());
                if current.load(Ordering::Acquire) != job.0 {
                    continue;
                }
                let (title, error, retry) = match result {
                    Ok(title) => (title, None, false),
                    // Session initialization temporarily blocks inspection. Keep
                    // the last title and retry without replacing action errors.
                    Err(error) if error.is::<crate::session::StartupBusy>() => (None, None, true),
                    Err(error) => (None, Some(format!("Workspace title: {error:#}")), true),
                };
                if updates
                    .send(Update {
                        generation: job.0,
                        title,
                        error,
                        retry,
                    })
                    .is_err()
                {
                    break;
                }
            }
        });
        Self {
            sender,
            receiver,
            generation,
            requested: None,
            retry_after: None,
        }
    }

    pub fn request(&mut self, title: Option<String>) {
        if self.retry_after.as_ref().is_some_and(|(deadline, pending)| pending == &title && Instant::now() < *deadline) {
            return;
        }
        if self.requested.as_ref() == Some(&title) {
            return;
        }
        let generation = self.generation.fetch_add(1, Ordering::AcqRel) + 1;
        if self.sender.send((generation, title.clone())).is_ok() {
            self.requested = Some(title);
            self.retry_after = None;
        }
    }

    pub fn take_update(&mut self) -> Option<Update> {
        while let Ok(update) = self.receiver.try_recv() {
            if update.generation == self.generation.load(Ordering::Acquire) {
                if update.retry {
                    if let Some(title) = self.requested.take() {
                        self.retry_after = Some((Instant::now() + Duration::from_secs(1), title));
                    }
                }
                return Some(update);
            }
        }
        None
    }

    pub fn retry(&mut self) {
        if let Some((deadline, title)) = self.retry_after.as_ref() {
            if Instant::now() >= *deadline {
                self.request(title.clone());
            }
        }
    }
}

impl Drop for Presentation {
    fn drop(&mut self) {
        self.generation.fetch_add(1, Ordering::AcqRel);
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn wait_update(presentation: &mut Presentation) -> Update {
        let deadline = Instant::now() + Duration::from_secs(2);
        loop {
            if let Some(update) = presentation.take_update() {
                return update;
            }
            assert!(Instant::now() < deadline, "presentation worker did not return");
            std::thread::sleep(Duration::from_millis(5));
        }
    }

    #[test]
    fn startup_contention_retries_same_title_without_error_or_topology_event() {
        let mut presentation = Presentation::start_with_executor({
            let calls = AtomicU64::new(0);
            move |title| {
                if calls.fetch_add(1, Ordering::AcqRel) == 0 {
                    return Err(anyhow::Error::new(crate::session::StartupBusy)
                        .context("Failed to inspect host panes"));
                }
                Ok(title.map(str::to_string))
            }
        });
        presentation.request(Some("session-a".into()));
        let update = wait_update(&mut presentation);
        assert!(update.retry);
        assert!(update.error.is_none());
        assert!(update.title.is_none());
        presentation.retry();
        assert!(presentation.requested.is_none(), "retry must back off");
        presentation.retry_after.as_mut().unwrap().0 = Instant::now();
        presentation.retry();
        let recovered = wait_update(&mut presentation);
        assert!(!recovered.retry);
        assert_eq!(recovered.title.as_deref(), Some("session-a"));
    }

    #[test]
    fn new_session_supersedes_failed_title_without_waiting_for_retry() {
        let mut presentation = Presentation::start_with_executor(|title| {
            if title == Some("old-session") {
                anyhow::bail!("pane unavailable");
            }
            Ok(title.map(str::to_string))
        });
        presentation.request(Some("old-session".into()));
        assert!(wait_update(&mut presentation).error.is_some());
        presentation.request(Some("new-session".into()));
        let update = wait_update(&mut presentation);
        assert_eq!(update.title.as_deref(), Some("new-session"));
        assert!(presentation.retry_after.is_none());
        presentation.retry();
        assert!(presentation.receiver.try_recv().is_err());
    }
}
