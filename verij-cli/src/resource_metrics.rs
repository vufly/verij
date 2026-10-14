//! Opt-in, bounded counters for isolated monitoring experiments.
//! No event logs; one replaceable snapshot per process, outside watched roots.
use serde::Serialize;
use std::collections::BTreeMap;
use std::path::PathBuf;
use std::sync::{Mutex, OnceLock};

#[derive(Default, Serialize)]
struct Counters {
    cli: BTreeMap<String, u64>,
    cli_overflow: u64,
    proof_attempts: u64,
    native_record_writes: u64,
    topology_fs_incoming: u64,
    topology_fs_coalesced: u64,
    agent_fs_incoming: u64,
    agent_fs_coalesced: u64,
}

static COUNTERS: Mutex<Option<Counters>> = Mutex::new(None);
static DIRECTORY: OnceLock<Option<PathBuf>> = OnceLock::new();
static PUBLISH: Mutex<()> = Mutex::new(());

impl Counters {
    fn cli(&mut self, target: &str, operation: &str) {
        let key = format!(
            "{}:{}",
            target.chars().take(256).collect::<String>(),
            operation.chars().take(64).collect::<String>()
        );
        if self.cli.contains_key(&key) || self.cli.len() < 32 {
            *self.cli.entry(key).or_default() += 1;
        } else {
            self.cli_overflow += 1;
        }
    }
}

fn update(f: impl FnOnce(&mut Counters)) {
    if DIRECTORY.get().is_some_and(Option::is_some) {
        if let Ok(mut counters) = COUNTERS.lock() {
            if let Some(counters) = counters.as_mut() {
                f(counters);
            }
        }
    }
}

pub fn command(command: &std::process::Command) {
    update(|c| {
        let args: Vec<_> = command.get_args().map(|a| a.to_string_lossy()).collect();
        let target = args
            .windows(2)
            .find(|a| matches!(a[0].as_ref(), "-s" | "--session"))
            .map(|a| a[1].as_ref())
            .unwrap_or("all-servers");
        let operation = args
            .windows(2)
            .find(|a| a[0] == "action")
            .map(|a| a[1].as_ref())
            .unwrap_or_else(|| {
                if args.iter().any(|a| a == "list-sessions") {
                    "list-sessions"
                } else {
                    "other"
                }
            });
        c.cli(target, operation);
    });
}

pub fn proof() {
    update(|c| c.proof_attempts += 1);
}
pub fn record_write() {
    update(|c| c.native_record_writes += 1);
}
pub fn fs_incoming(agent: bool) {
    update(|c| {
        if agent {
            c.agent_fs_incoming += 1
        } else {
            c.topology_fs_incoming += 1
        }
    });
}
pub fn fs_coalesced(agent: bool) {
    update(|c| {
        if agent {
            c.agent_fs_coalesced += 1
        } else {
            c.topology_fs_coalesced += 1
        }
    });
}

fn publish() {
    let Some(Some(dir)) = DIRECTORY.get() else {
        return;
    };
    let Ok(_publisher) = PUBLISH.lock() else {
        return;
    };
    let bytes = {
        let Ok(counters) = COUNTERS.lock() else {
            return;
        };
        let value = serde_json::json!({"pid": std::process::id(),
            "observed_at_ms": crate::agent_store::now_ms(), "counters": *counters});
        serde_json::to_vec(&value)
    };
    // Notification callbacks never wait behind snapshot filesystem I/O.
    if let Ok(bytes) = bytes {
        let target = dir.join(format!("native-{}.json", std::process::id()));
        let temp = target.with_extension("tmp");
        if std::fs::write(&temp, bytes).is_ok() {
            let _ = std::fs::rename(temp, target);
        }
    }
}

pub struct Guard;
impl Drop for Guard {
    fn drop(&mut self) {
        publish();
    }
}

pub fn start() -> Guard {
    let directory = std::env::var_os("VERIJ_RESOURCE_METRICS_DIR")
        .map(PathBuf::from)
        .filter(|p| p.is_absolute())
        .filter(|p| crate::process::ensure_private_directory(p).is_ok());
    let enabled = directory.is_some();
    let _ = DIRECTORY.set(directory);
    if enabled {
        if let Ok(mut counters) = COUNTERS.lock() {
            *counters = Some(Counters::default());
        }
        let _ = std::thread::Builder::new()
            .name("verij-resource-metrics".into())
            .spawn(|| loop {
                publish();
                std::thread::sleep(std::time::Duration::from_secs(1));
            });
    }
    Guard
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn target_churn_cannot_expand_counter_storage() {
        let mut counters = Counters::default();
        for n in 0..10_000 {
            counters.cli(&format!("session-{n}"), "pipe");
        }
        assert_eq!(counters.cli.len(), 32);
        assert_eq!(counters.cli_overflow, 9_968);
        counters.cli("session-0", "pipe");
        assert_eq!(counters.cli["session-0:pipe"], 2);
        assert_eq!(counters.cli_overflow, 9_968);
    }

    #[test]
    fn unicode_names_have_fixed_storage_bound() {
        let mut counters = Counters::default();
        counters.cli(&"🦀".repeat(1_000), &"🦀".repeat(1_000));
        let key = counters.cli.keys().next().unwrap();
        assert_eq!(key.len(), 4 * (256 + 64) + 1);
        assert_eq!(key.chars().count(), 256 + 64 + 1);
    }
}
