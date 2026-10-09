//! OpenCode 1.18.x TUI-local, metadata-only snapshot contract.
use anyhow::{bail, Context, Result};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use std::io::{BufRead, Write};
use verij_types::agent::{
    ActivityKind, AgentState, AgentStatus, ExecutionError, PartialSourceRecord, PendingRequest,
    SuccessfulCompletion, TurnId,
};

pub const BRIDGE_VERSION: u32 = 1;
pub const SUPPORTED: &[&str] = &["1.18.32", "1.18.33", "1.18.34"];
pub const LEASE_MS: u64 = 15_000;

pub fn lease_expired(state: &AgentState, now: u64) -> bool {
    state.sources.get("opencode").is_some_and(|source| {
        source.capabilities.iter().any(|cap| cap == "lease")
            && (now.saturating_sub(source.observed_at_ms) > LEASE_MS
                || source.observed_at_ms > now.saturating_add(1000))
    })
}

#[derive(Debug, Default, Clone, Deserialize, Serialize)]
#[serde(deny_unknown_fields)]
pub struct Snapshot {
    pub schema_version: u32,
    pub version: String,
    pub conversation_id: Option<String>,
    pub conversation_title: Option<String>,
    /// Immutable upstream root/user-message pair, not wall-clock ordering.
    pub turn_id: Option<String>,
    pub activity: Option<ActivityKind>,
    #[serde(default)]
    pub pending_requests: Vec<PendingRequest>,
    #[serde(default)]
    pub background_task_count: usize,
    pub completed_at_ms: Option<u64>,
    /// Error *type* only. Never retain provider bodies or tool output.
    pub error_name: Option<String>,
}

impl Snapshot {
    fn validate(&self) -> Result<()> {
        if self.schema_version != BRIDGE_VERSION || !SUPPORTED.contains(&self.version.as_str()) {
            bail!("unsupported OpenCode snapshot/version");
        }
        for id in [&self.conversation_id, &self.turn_id, &self.error_name]
            .into_iter()
            .flatten()
        {
            if id.is_empty() || id.len() > 256 || id.chars().any(char::is_control) {
                bail!("invalid bounded OpenCode metadata");
            }
        }
        if self.pending_requests.len() > 100 || self.background_task_count > 64 {
            bail!("OpenCode family exceeds adapter bounds");
        }
        if self.completed_at_ms.is_some()
            && (self.turn_id.is_none()
                || self.conversation_id.is_none()
                || self.activity != Some(ActivityKind::Idle)
                || !self.pending_requests.is_empty()
                || self.background_task_count != 0
                || self.error_name.is_some())
        {
            bail!("unqualified aggregate completion");
        }
        Ok(())
    }

    fn project(&self, epoch: u64, revision: u64) -> Result<PartialSourceRecord> {
        self.validate()?;
        let turn = self.turn_id.as_ref().map(|id| TurnId(id.clone()));
        Ok(PartialSourceRecord {
            source_id: "opencode".into(),
            source_epoch: Some(epoch),
            source_revision: Some(revision),
            observed_at_ms: Some(crate::agent_store::now_ms()),
            // Full snapshots deliberately reset missing metadata/outcomes at
            // every ordered observation. Source/turn order is adapter-owned.
            turn_epoch: Some(epoch),
            turn_revision: Some(revision),
            turn_id: turn.clone(),
            conversation_id: self
                .conversation_id
                .clone()
                .map(verij_types::agent::ConversationId),
            conversation_title: self.conversation_title.clone(),
            activity: Some(self.activity.unwrap_or(ActivityKind::Unknown)),
            pending_requests: Some(self.pending_requests.clone()),
            background_task_count: Some(self.background_task_count),
            successful_completion: self.completed_at_ms.map(|completed_at_ms| {
                SuccessfulCompletion {
                    turn_id: turn.clone(),
                    turn_epoch: epoch,
                    turn_revision: revision,
                    completed_at_ms,
                    summary: None,
                }
            }),
            execution_error: self
                .error_name
                .as_ref()
                .filter(|_| turn.is_some())
                .map(|name| ExecutionError {
                    turn_id: turn,
                    turn_epoch: epoch,
                    turn_revision: revision,
                    message: name.clone(),
                    is_terminal: true,
                }),
            capabilities: Some(
                vec![
                    "activity",
                    "permissions",
                    "questions",
                    "conversation_title",
                    "aggregate_completion",
                    "terminal_failure",
                    "lease",
                ]
                .into_iter()
                .map(str::to_string)
                .collect(),
            ),
            ..Default::default()
        })
    }
}

/// Restore the original revision on a repeated upstream outcome, including
/// plugin reload and Home -> conversation navigation. Ledger and source are
/// committed together. Exhaustion degrades to Idle rather than replaying Done.
pub(crate) fn finish(state: &mut AgentState) -> Result<()> {
    if state.reduced.status != AgentStatus::Done {
        return Ok(());
    }
    let key = state
        .reduced
        .current_turn_id
        .as_ref()
        .context("completion without turn")?
        .0
        .clone();
    if let Some(original) = state.opencode_completions.get(&key) {
        state.completion_revision -= 1;
        state.reduced.completion_revision = *original;
    } else if state.opencode_completions.len() >= 1024 {
        state.completion_revision -= 1;
        state.reduced.status = AgentStatus::Idle;
        state.reduced.latest_completion = None;
        state.reduced.completion_revision = state.completion_revision;
        state.reduced.detail = Some("completion ledger full".into());
    } else {
        state
            .opencode_completions
            .insert(key, state.completion_revision);
    }
    Ok(())
}

pub fn run(_session: String, pane: u32, pid: u32) -> Result<()> {
    // Environment gives a candidate terminal, not durable session identity.
    // Locate the actual owner even after rename; reject ambiguity.
    let process = crate::process::identity(pid)?;
    let snapshots = crate::inventory::read_states(&crate::fs_watcher::resolve_states_dir());
    let owners: Vec<_> = snapshots
        .iter()
        .filter(|snapshot| {
            snapshot.inventory.as_ref().is_some_and(|inventory| {
                inventory.server_process.as_ref().is_some_and(|server| {
                    inventory.panes.iter().any(|p| {
                        p.terminal_id.0 == pane
                            && !p.exited
                            && p.pane_process.as_ref().is_some_and(|owner| {
                                crate::process::verify_foreground(server, owner, &process).is_ok()
                            })
                    })
                })
            })
        })
        .collect();
    if owners.len() != 1 {
        bail!("OpenCode pane ownership unavailable or ambiguous");
    }
    let session = owners[0].name.clone();
    let instance = crate::agent_cli::register(crate::agent_cli::RegisterArgs {
        session,
        pane,
        pid,
        kind: "opencode".into(),
        synthetic: false,
        runner_id: None,
    })?;
    // Reloads cannot interleave producers for the same process. Never wait
    // behind an obsolete bridge: the new bridge retries after bounded teardown.
    let lock = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .open(
            crate::agent_store::resolve_v1_dir()?
                .join(&instance.0)
                .join(".opencode.lock"),
        )?;
    lock.try_lock_exclusive()
        .context("OpenCode producer already connected")?;
    let (_, current) = crate::agent_store::inspect(&instance)?;
    let epoch = current
        .sources
        .get("opencode")
        .map_or(1, |s| s.source_epoch + 1);
    let mut revision = 0;
    let mut stdout = std::io::stdout().lock();
    writeln!(
        stdout,
        "{}",
        serde_json::json!({"ready":true,"agent_instance_id":instance.0})
    )?;
    stdout.flush()?;
    let mut stdin = std::io::stdin().lock();
    let mut buffer = Vec::new();
    let result = (|| -> Result<()> {
        loop {
            buffer.clear();
            // read_until alone would permit unbounded allocation.
            let bytes = std::io::Read::take(&mut stdin, 262_145).read_until(b'\n', &mut buffer)?;
            if bytes == 0 {
                break;
            }
            if bytes > 262_144 || !buffer.ends_with(b"\n") {
                bail!("invalid bounded snapshot frame");
            }
            let snapshot: Snapshot = serde_json::from_slice(&buffer)?;
            revision += 1;
            let state = crate::agent_store::update(
                &instance,
                |state| {
                    let partial = snapshot.project(epoch, revision)?;
                    // This adapter sends complete snapshots, unlike generic partial
                    // report clients. Null ownership/outcomes must clear old facts.
                    if let Some(old) = state.sources.get("opencode") {
                        if (epoch, revision) <= (old.source_epoch, old.source_revision) {
                            bail!("obsolete OpenCode producer");
                        }
                    }
                    state.sources.remove("opencode");
                    Ok(partial)
                },
                finish,
            )?;
            writeln!(
                stdout,
                "{}",
                serde_json::json!({"record_revision":state.record_revision,
                "completion_revision":state.completion_revision,"status":state.reduced.status})
            )?;
            stdout.flush()?;
        }
        Ok(())
    })();
    // Disabling/reloading the observer does not imply agent exit or success.
    let unknown = Snapshot {
        schema_version: BRIDGE_VERSION,
        version: SUPPORTED[2].into(),
        ..Default::default()
    };
    let _ = crate::agent_store::update(
        &instance,
        |state| {
            state.sources.remove("opencode");
            unknown.project(epoch, revision + 1)
        },
        |_| Ok(()),
    );
    result
}

#[cfg(test)]
mod tests {
    use super::*;
    #[test]
    fn outcomes_require_owned_turn_and_quiescence() {
        let mut snapshot = Snapshot {
            schema_version: 1,
            version: "1.18.34".into(),
            conversation_id: Some("root".into()),
            turn_id: Some("root/user".into()),
            activity: Some(ActivityKind::Idle),
            completed_at_ms: Some(42),
            ..Default::default()
        };
        assert!(snapshot
            .project(1, 1)
            .unwrap()
            .successful_completion
            .is_some());
        snapshot.background_task_count = 1;
        assert!(snapshot.project(1, 2).is_err());
        snapshot.background_task_count = 0;
        snapshot.error_name = Some("MessageAbortedError".into());
        assert!(snapshot.project(1, 3).is_err());
        snapshot.completed_at_ms = None;
        assert!(snapshot.project(1, 4).unwrap().execution_error.is_some());
        snapshot.version = "2.0.0".into();
        assert!(snapshot.project(1, 5).is_err());
    }

    #[test]
    fn completion_ledger_restores_original_revision_after_reload_and_reselection() {
        let mut state: AgentState = serde_json::from_value(serde_json::json!({
            "schema_version":1,"agent_instance_id":"test","record_revision":1,"updated_at_ms":1,
            "sources":{},"completion_revision":3,"completed_turn":"root/user", "completed_turn_epoch":2,
            "completed_turn_revision":1,"opencode_completions":{"root/user":1,"another/user":2},
            "reduced":{"status":"done","current_turn_id":"root/user","current_turn_epoch":2,
                "current_turn_revision":1,"pending_requests":[],"background_task_count":0,"completion_revision":3}
        })).unwrap();
        finish(&mut state).unwrap();
        assert_eq!(state.completion_revision, 2);
        assert_eq!(state.reduced.completion_revision, 1);
        assert_eq!(state.opencode_completions.len(), 2);
        let old: AgentState = serde_json::from_value(serde_json::json!({
            "schema_version":1,"agent_instance_id":"test","record_revision":1,"updated_at_ms":1,
            "sources":{},"completion_revision":0,"completed_turn_epoch":0,"completed_turn_revision":0,
            "reduced":{"status":"unknown","current_turn_epoch":0,"current_turn_revision":0,
                "pending_requests":[],"background_task_count":0,"completion_revision":0}
        })).unwrap();
        assert!(old.opencode_completions.is_empty());
    }
}
