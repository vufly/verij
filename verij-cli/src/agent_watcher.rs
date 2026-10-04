//! Agent updates are independent of slower Zellij topology discovery.
use anyhow::Result;
use notify::{RecursiveMode, Watcher};
use std::path::Path;
use std::time::Duration;
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
        let Ok(state) = serde_json::from_slice::<AgentState>(&state) else {
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
        records.push(AgentRecord { identity, state });
    }
    records.sort_by(|a, b| {
        a.identity
            .agent_instance_id
            .cmp(&b.identity.agent_instance_id)
    });
    records
}

pub fn spawn(tx: mpsc::Sender<Vec<AgentRecord>>) -> Result<tokio::task::JoinHandle<()>> {
    let root = crate::agent_store::resolve_v1_dir()?;
    Ok(tokio::task::spawn_blocking(move || {
        let (events, receiver) = std::sync::mpsc::channel();
        let mut watcher =
            notify::recommended_watcher(move |event: notify::Result<notify::Event>| {
                if event.is_ok() {
                    let _ = events.send(());
                }
            })
            .ok();
        if let Some(watcher) = watcher.as_mut() {
            let _ = watcher.watch(&root, RecursiveMode::Recursive);
        }
        let mut previous = None;
        while !tx.is_closed() {
            let records = read_records(&root);
            if previous.as_ref() != Some(&records) {
                previous = Some(records.clone());
                if tx.blocking_send(records).is_err() {
                    break;
                }
            }
            let _ = receiver.recv_timeout(Duration::from_secs(1));
            // Coalesce refreshes by rereading whole atomic state files; stored
            // completion revisions preserve edges even if notifications merge.
            while receiver.try_recv().is_ok() {}
        }
    }))
}
