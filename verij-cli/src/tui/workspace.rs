//! Workspace title inspection/renaming cannot block terminal input or rendering.
use std::sync::{
    atomic::{AtomicU64, Ordering},
    mpsc, Arc,
};

pub struct Update {
    pub generation: u64,
    pub title: Option<String>,
    pub error: Option<String>,
}

pub struct Presentation {
    sender: mpsc::Sender<(u64, Option<String>)>,
    receiver: mpsc::Receiver<Update>,
    generation: Arc<AtomicU64>,
    requested: Option<Option<String>>,
}

impl Presentation {
    pub fn start() -> Self {
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
                let result = if let Some(title) = job.1.as_ref() {
                    crate::actions::rename_workspace_pane(title).map(|()| Some(title.clone()))
                } else {
                    crate::actions::workspace_pane_title()
                };
                if current.load(Ordering::Acquire) != job.0 {
                    continue;
                }
                let (title, error) = match result {
                    Ok(title) => (title, None),
                    Err(error) => (None, Some(format!("Workspace title: {error:#}"))),
                };
                if updates
                    .send(Update {
                        generation: job.0,
                        title,
                        error,
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
        }
    }

    pub fn request(&mut self, title: Option<String>) {
        if self.requested.as_ref() == Some(&title) {
            return;
        }
        let generation = self.generation.fetch_add(1, Ordering::AcqRel) + 1;
        if self.sender.send((generation, title.clone())).is_ok() {
            self.requested = Some(title);
        }
    }

    pub fn take_update(&mut self) -> Option<Update> {
        while let Ok(update) = self.receiver.try_recv() {
            if update.generation == self.generation.load(Ordering::Acquire) {
                if update.error.is_some() {
                    self.requested = None;
                }
                return Some(update);
            }
        }
        None
    }
}

impl Drop for Presentation {
    fn drop(&mut self) {
        self.generation.fetch_add(1, Ordering::AcqRel);
    }
}
