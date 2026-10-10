//! Agy metadata-only, one-shot observations with transactional correlation.
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::io::Read;
use verij_types::agent::*;

pub const SUPPORTED: &[&str] = &["1.2.14", "1.3.2", "1.3.3"];

#[derive(Debug, Default, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Correlation {
    pub sample: u64,
    pub turn: u64,
    pub conversation: Option<String>,
    pub invocation: Option<(u64, u64)>,
    pub posted: bool,
    pub stopped: bool,
    #[serde(default)]
    pub pre_sample: u64,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Observation {
    schema_version: u32,
    version: String,
    event: String,
    sample: u64,
    conversation_id: Option<String>,
    #[serde(rename = "conversationId")]
    lifecycle_conversation: Option<String>,
    agent_state: Option<String>,
    tool_confirmation_pending: Option<bool>,
    task_count: Option<usize>,
    #[serde(rename = "invocationNum")]
    invocation_num: Option<u64>,
    #[serde(rename = "initialNumSteps")]
    initial_steps: Option<u64>,
    #[serde(rename = "executionNum")]
    execution_num: Option<u64>,
    #[serde(rename = "fullyIdle")]
    fully_idle: Option<bool>,
    #[serde(rename = "terminationReason")]
    reason: Option<String>,
    error_present: Option<bool>,
}

impl Observation {
    fn project(&self, state: &mut AgentState) -> Result<PartialSourceRecord> {
        if self.schema_version != 1
            || !SUPPORTED.contains(&self.version.as_str())
            || self.sample == 0
        {
            bail!("unsupported Agy observation");
        }
        let conversation = self
            .conversation_id
            .as_ref()
            .or(self.lifecycle_conversation.as_ref())
            .filter(|id| !id.is_empty())
            .cloned();
        if conversation
            .as_ref()
            .is_some_and(|s| s.len() > 256 || s.chars().any(char::is_control))
            || self.task_count.is_some_and(|n| n > 1024)
        {
            bail!("invalid bounded Agy metadata");
        }
        let mut journal: Correlation = state
            .adapter_state
            .get("agy")
            .map(|v| serde_json::from_value(v.clone()))
            .transpose()?
            .unwrap_or_default();
        if self.event != "ui"
            && journal.conversation.is_some()
            && conversation != journal.conversation
        {
            bail!("foreign Agy lifecycle conversation");
        }
        let ui = self.event == "ui";
        let source = if ui { "agy-ui" } else { "agy-lifecycle" };
        // Source-specific ordering: a UI refresh can race a valid Stop. Arrival
        // from another source never invalidates its qualified terminal outcome.
        if state
            .sources
            .get(source)
            .is_some_and(|old| self.sample <= old.source_revision)
            || self.sample < journal.pre_sample
        {
            bail!("obsolete Agy callback");
        }
        let changed = conversation != journal.conversation;
        if changed {
            journal.conversation = conversation.clone();
            journal.invocation = None;
            journal.posted = false;
            journal.stopped = false;
            journal.turn += 1;
            state.sources.remove("agy-ui");
            state.sources.remove("agy-lifecycle");
        }
        let mut partial = PartialSourceRecord {
            source_id: source.into(),
            source_epoch: Some(1),
            source_revision: Some(self.sample),
            conversation_id: conversation.clone().map(ConversationId),
            turn_epoch: Some(1),
            turn_revision: Some(journal.turn),
            ..Default::default()
        };
        match self.event.as_str() {
            "ui" => {
                let mut activity = match self.agent_state.as_deref() {
                    Some("thinking" | "working" | "tool_use") => ActivityKind::Working,
                    Some("idle") => ActivityKind::Idle,
                    Some("initializing" | "authenticating") => ActivityKind::Initializing,
                    _ => ActivityKind::Unknown,
                };
                if activity == ActivityKind::Idle
                    && self.task_count.is_none()
                    && self.tool_confirmation_pending.is_none()
                    && !journal.stopped
                {
                    activity = ActivityKind::Unknown;
                }
                // A fresh work edge invalidates the prior terminal execution,
                // even if the lifecycle source has been disabled or missed.
                let was_idle = state
                    .sources
                    .get("agy-ui")
                    .is_some_and(|s| s.activity == Some(ActivityKind::Idle));
                if activity == ActivityKind::Working
                    && was_idle
                    && (journal.stopped || journal.invocation.is_none())
                {
                    journal.turn += 1;
                    journal.stopped = false;
                    journal.posted = false;
                    journal.invocation = None;
                    journal.pre_sample = self.sample;
                    state.sources.remove("agy-lifecycle");
                    state.sources.remove("agy-ui");
                }
                partial.activity = Some(activity);
                partial.background_task_count = self.task_count;
                // Verified ready-state payloads omit false confirmation flags.
                // Initializing/authenticating omissions remain non-authoritative.
                let pending =
                    self.tool_confirmation_pending
                        .or_else(|| match self.agent_state.as_deref() {
                            Some("working" | "thinking" | "idle") => Some(false),
                            _ => None,
                        });
                partial.pending_requests = pending.map(|pending| {
                    if pending {
                        vec![PendingRequest {
                            id: "agy-permission".into(),
                            kind: RequestKind::Permission,
                            detail: None,
                        }]
                    } else {
                        vec![]
                    }
                });
                partial.capabilities = Some(vec![
                    "activity".into(),
                    "permissions".into(),
                    "background_tasks".into(),
                ]);
                // Current UI Idle can resolve foreground activity. Missing
                // task/input fields remain Unknown; no successful outcome.
                if self.agent_state.as_deref() == Some("idle") && !journal.stopped {
                    if let Some(lifecycle) = state.sources.get_mut("agy-lifecycle") {
                        lifecycle.activity = Some(ActivityKind::Unknown);
                    }
                }
            }
            "PreInvocation" => {
                let key = (
                    self.initial_steps.context("missing invocation steps")?,
                    self.invocation_num.context("missing invocation counter")?,
                );
                if !changed && journal.invocation.is_some_and(|old| key <= old) {
                    bail!("duplicate or obsolete invocation");
                }
                journal.turn += 1;
                journal.invocation = Some(key);
                journal.pre_sample = self.sample;
                journal.posted = false;
                journal.stopped = false;
                state.sources.remove("agy-lifecycle");
                // Preserve current permission/task hints at the new generation.
                if let Some(s) = state.sources.get_mut("agy-ui") {
                    s.turn_revision = journal.turn;
                    s.turn_id = Some(TurnId(format!("agy-{}", journal.turn)));
                }
                partial.activity = Some(ActivityKind::Working);
            }
            "PostInvocation" => {
                let key = (
                    self.initial_steps.context("missing invocation steps")?,
                    self.invocation_num.context("missing invocation counter")?,
                );
                if journal.invocation != Some(key) || journal.stopped {
                    bail!("unmatched post-invocation");
                }
                journal.posted = true;
            }
            "Stop" => {
                self.execution_num.context("missing execution counter")?;
                if !journal.posted || journal.invocation.is_none() || conversation.is_none() {
                    bail!("uncorrelated Stop");
                }
                let idle = self.fully_idle.context("missing aggregate quiescence")?;
                let success = !self.error_present.context("missing outcome flag")?
                    && self.reason.as_deref() == Some("NO_TOOL_CALL");
                // Repeated fullyIdle=false/true are allowed for one execution;
                // the adapter generation, not executionNum, owns completion.
                journal.stopped = idle;
                partial.background_task_count = Some(if idle { 0 } else { 1 });
                partial.activity = Some(if idle {
                    ActivityKind::Idle
                } else {
                    ActivityKind::Working
                });
                if idle {
                    state.sources.remove("agy-lifecycle");
                    if let Some(s) = state.sources.get_mut("agy-ui") {
                        s.background_task_count = 0;
                        s.pending_requests.clear();
                    }
                    let turn_id = Some(TurnId(format!("agy-{}", journal.turn)));
                    if success {
                        partial.successful_completion = Some(SuccessfulCompletion {
                            turn_id,
                            turn_epoch: 1,
                            turn_revision: journal.turn,
                            completed_at_ms: crate::agent_store::now_ms(),
                            summary: None,
                        });
                    } else if self.error_present == Some(true) {
                        partial.execution_error = Some(ExecutionError {
                            turn_id,
                            turn_epoch: 1,
                            turn_revision: journal.turn,
                            message: "Agy execution failed".into(),
                            is_terminal: true,
                        });
                    }
                }
                partial.capabilities = Some(vec![
                    "aggregate_completion".into(),
                    "terminal_failure".into(),
                ]);
            }
            _ => bail!("unsupported non-gating Agy event"),
        }
        journal.sample = journal.sample.max(self.sample);
        partial.turn_revision = Some(journal.turn);
        partial.turn_id = Some(TurnId(format!("agy-{}", journal.turn)));
        state
            .adapter_state
            .insert("agy".into(), serde_json::to_value(journal)?);
        Ok(partial)
    }
}

pub fn run(pane: u32, pid: u32, birth: u64) -> Result<()> {
    let process = crate::process::identity(pid)?;
    if process.start_jiffies != birth {
        bail!("Agy process lifetime changed");
    }
    let executable = std::fs::read_link(format!("/proc/{pid}/exe"))?;
    if !executable
        .file_name()
        .and_then(|s| s.to_str())
        .is_some_and(|s| s == "agy" || s.starts_with("agy."))
    {
        bail!("callback owner is not Agy");
    }
    let snapshots = crate::inventory::read_states(&crate::fs_watcher::resolve_states_dir());
    let owners: Vec<_> = snapshots
        .iter()
        .filter(|snapshot| {
            snapshot.inventory.as_ref().is_some_and(|i| {
                i.server_process.as_ref().is_some_and(|server| {
                    i.panes.iter().any(|p| {
                        p.terminal_id.0 == pane
                            && !p.exited
                            && p.pane_process.as_ref().is_some_and(|leader| {
                                crate::process::verify_agy_foreground(server, leader, &process)
                                    .is_ok()
                            })
                    })
                })
            })
        })
        .collect();
    if owners.len() != 1 {
        bail!("Agy foreground pane ownership unavailable or ambiguous");
    }
    let mut buffer = Vec::new();
    std::io::stdin().take(262145).read_to_end(&mut buffer)?;
    if buffer.len() > 262144 {
        bail!("oversized Agy report");
    }
    let observation: Observation = serde_json::from_slice(&buffer)?;
    let instance = crate::agent_cli::register(crate::agent_cli::RegisterArgs {
        session: owners[0].name.clone(),
        pane,
        pid,
        kind: "agy".into(),
        synthetic: false,
        runner_id: None,
    })?;
    crate::agent_store::update(&instance, |state| observation.project(state), |_| Ok(()))?;
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::{json, Value};

    fn state() -> AgentState {
        serde_json::from_value(json!({"schema_version":1,"agent_instance_id":"test","record_revision":1,
            "updated_at_ms":1,"sources":{},"reduced":{"status":"unknown","current_turn_epoch":0,
            "current_turn_revision":0,"background_task_count":0,"completion_revision":0},"completion_revision":0,
            "completed_turn_epoch":0,"completed_turn_revision":0})).unwrap()
    }

    fn observation(event: &str, sample: u64, fields: Value) -> Observation {
        let mut value = json!({"schema_version":1,"version":"1.3.2","event":event,"sample":sample});
        value
            .as_object_mut()
            .unwrap()
            .extend(fields.as_object().unwrap().clone());
        serde_json::from_value(value).unwrap()
    }

    fn apply(state: &mut AgentState, event: &str, sample: u64, fields: Value) -> Result<()> {
        let partial = observation(event, sample, fields).project(state)?;
        let source = partial
            .merge_into(
                &state.agent_instance_id,
                state.sources.get(&partial.source_id),
                sample,
            )
            .map_err(|e| anyhow::anyhow!(e))?;
        state.sources.insert(partial.source_id, source);
        let (reduced, revision, turn, epoch, turn_revision) = reduce_sources(
            &state.sources,
            AgentKind::Agy,
            false,
            state.completion_revision,
            state.completed_turn.as_ref(),
            state.completed_turn_epoch,
            state.completed_turn_revision,
        );
        state.reduced = reduced;
        state.completion_revision = revision;
        state.completed_turn = turn;
        state.completed_turn_epoch = epoch;
        state.completed_turn_revision = turn_revision;
        Ok(())
    }

    #[test]
    fn permission_background_and_repeated_execution_zero_do_not_replay_completion() {
        let mut s = state();
        apply(
            &mut s,
            "PreInvocation",
            1,
            json!({"conversationId":"c","initialNumSteps":1,"invocationNum":0}),
        )
        .unwrap();
        apply(&mut s, "ui", 2, json!({"conversation_id":"c","agent_state":"tool_use","tool_confirmation_pending":true,"task_count":0})).unwrap();
        assert_eq!(s.reduced.status, AgentStatus::NeedsInput);
        apply(&mut s, "ui", 3, json!({"conversation_id":"c","agent_state":"idle","tool_confirmation_pending":false,"task_count":1})).unwrap();
        apply(
            &mut s,
            "PostInvocation",
            4,
            json!({"conversationId":"c","initialNumSteps":1,"invocationNum":0}),
        )
        .unwrap();
        apply(&mut s, "Stop", 5, json!({"conversationId":"c","executionNum":0,"fullyIdle":false,"terminationReason":"NO_TOOL_CALL","error_present":false})).unwrap();
        assert_eq!(s.reduced.status, AgentStatus::Working);
        assert_eq!(s.completion_revision, 0);
        apply(&mut s, "Stop", 6, json!({"conversationId":"c","executionNum":0,"fullyIdle":true,"terminationReason":"NO_TOOL_CALL","error_present":false})).unwrap();
        assert_eq!(s.reduced.status, AgentStatus::Done);
        assert_eq!(s.completion_revision, 1);
        apply(&mut s, "Stop", 7, json!({"conversationId":"c","executionNum":0,"fullyIdle":true,"terminationReason":"NO_TOOL_CALL","error_present":false})).unwrap();
        assert_eq!(s.completion_revision, 1);
    }

    #[test]
    fn stale_samples_unmatched_stops_and_new_work_cannot_consume_old_success() {
        let mut s = state();
        apply(
            &mut s,
            "ui",
            1,
            json!({"conversation_id":"c","agent_state":"idle"}),
        )
        .unwrap();
        assert_eq!(s.reduced.status, AgentStatus::Unknown);
        assert!(apply(&mut s, "Stop", 2, json!({"conversationId":"c","executionNum":0,"fullyIdle":true,"terminationReason":"NO_TOOL_CALL","error_present":false})).is_err());
        apply(
            &mut s,
            "PreInvocation",
            3,
            json!({"conversationId":"c","initialNumSteps":1,"invocationNum":0}),
        )
        .unwrap();
        assert!(apply(
            &mut s,
            "ui",
            2,
            json!({"conversation_id":"c","agent_state":"idle"})
        )
        .is_err());
        apply(
            &mut s,
            "PreInvocation",
            4,
            json!({"conversationId":"c","initialNumSteps":3,"invocationNum":1}),
        )
        .unwrap();
        assert!(apply(
            &mut s,
            "PostInvocation",
            5,
            json!({"conversationId":"c","initialNumSteps":1,"invocationNum":0})
        )
        .is_err());
        assert!(apply(&mut s, "Stop", 6, json!({"conversationId":"c","executionNum":0,"fullyIdle":true,"terminationReason":"NO_TOOL_CALL","error_present":false})).is_err());
        assert_eq!(s.reduced.status, AgentStatus::Working);
        assert_eq!(s.completion_revision, 0);
    }

    #[test]
    fn terminal_error_and_unknown_stop_are_never_success() {
        for error in [true, false] {
            let mut s = state();
            apply(
                &mut s,
                "PreInvocation",
                1,
                json!({"conversationId":"c","initialNumSteps":1,"invocationNum":0}),
            )
            .unwrap();
            apply(
                &mut s,
                "PostInvocation",
                2,
                json!({"conversationId":"c","initialNumSteps":1,"invocationNum":0}),
            )
            .unwrap();
            apply(&mut s, "Stop", 3, json!({"conversationId":"c","executionNum":0,"fullyIdle":true,"terminationReason":"CANCELLED","error_present":error})).unwrap();
            assert_eq!(s.completion_revision, 0);
            assert_eq!(
                s.reduced.status,
                if error {
                    AgentStatus::Error
                } else {
                    AgentStatus::Idle
                }
            );
        }
    }

    #[test]
    fn trailing_working_refresh_preserves_stop_but_fresh_idle_work_edge_supersedes_it() {
        let mut s = state();
        apply(
            &mut s,
            "ui",
            1,
            json!({"conversation_id":"c","agent_state":"working"}),
        )
        .unwrap();
        apply(
            &mut s,
            "PreInvocation",
            2,
            json!({"conversationId":"c","initialNumSteps":1,"invocationNum":0}),
        )
        .unwrap();
        apply(
            &mut s,
            "PostInvocation",
            3,
            json!({"conversationId":"c","initialNumSteps":1,"invocationNum":0}),
        )
        .unwrap();
        apply(&mut s,"Stop",4,json!({"conversationId":"c","executionNum":0,"fullyIdle":true,"terminationReason":"NO_TOOL_CALL","error_present":false})).unwrap();
        apply(
            &mut s,
            "ui",
            5,
            json!({"conversation_id":"c","agent_state":"working"}),
        )
        .unwrap();
        assert_eq!(s.reduced.status, AgentStatus::Done);
        apply(&mut s,"ui",6,json!({"conversation_id":"c","agent_state":"idle","task_count":0,"tool_confirmation_pending":false})).unwrap();
        apply(
            &mut s,
            "ui",
            7,
            json!({"conversation_id":"c","agent_state":"working"}),
        )
        .unwrap();
        assert_eq!(s.reduced.status, AgentStatus::Working);
        assert_eq!(s.completion_revision, 1);
    }

    #[test]
    fn native_ready_working_omits_false_permission_but_startup_omission_preserves_it() {
        let mut s = state();
        apply(&mut s,"ui",1,json!({"conversation_id":"c","agent_state":"tool_use","tool_confirmation_pending":true})).unwrap();
        apply(
            &mut s,
            "ui",
            2,
            json!({"conversation_id":"c","agent_state":"initializing"}),
        )
        .unwrap();
        assert_eq!(s.reduced.status, AgentStatus::NeedsInput);
        apply(
            &mut s,
            "ui",
            3,
            json!({"conversation_id":"c","agent_state":"working","task_count":1}),
        )
        .unwrap();
        assert_eq!(s.reduced.status, AgentStatus::Working);
        assert_eq!(s.reduced.background_task_count, 1);
    }

    #[test]
    fn concurrent_ui_refresh_cannot_drop_a_qualified_lifecycle_stop() {
        let mut s = state();
        apply(
            &mut s,
            "PreInvocation",
            1,
            json!({"conversationId":"c","initialNumSteps":1,"invocationNum":0}),
        )
        .unwrap();
        apply(
            &mut s,
            "PostInvocation",
            2,
            json!({"conversationId":"c","initialNumSteps":1,"invocationNum":0}),
        )
        .unwrap();
        apply(
            &mut s,
            "ui",
            4,
            json!({"conversation_id":"c","agent_state":"working"}),
        )
        .unwrap();
        apply(&mut s,"Stop",3,json!({"conversationId":"c","executionNum":0,"fullyIdle":true,"terminationReason":"NO_TOOL_CALL","error_present":false})).unwrap();
        assert_eq!(s.reduced.status, AgentStatus::Done);
        assert_eq!(s.completion_revision, 1);
        assert!(apply(
            &mut s,
            "ui",
            3,
            json!({"conversation_id":"c","agent_state":"idle"})
        )
        .is_err());
    }
}
