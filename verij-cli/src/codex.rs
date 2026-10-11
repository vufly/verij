//! Codex lifecycle hook observations and state projection.
use anyhow::{bail, Result};
use serde::{Deserialize, Serialize};
use std::io::Read;
use verij_types::agent::*;

pub const SUPPORTED: &[&str] = &["0.160.1", "0.162.1"];

pub const SUPPORTED_EVENTS: &[&str] = &[
    "SessionStart",
    "UserPromptSubmit",
    "PreToolUse",
    "PermissionRequest",
    "PostToolUse",
    "Stop",
    "Interrupt",
    "SessionEnd",
];

#[derive(Debug, Default, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct Correlation {
    pub sample: u64,
    pub turn: u64,
    pub session_id: Option<String>,
    pub turn_id: Option<String>,
    pub stopped: bool,
    pub cancelled: bool,
    pub is_working: bool,
    #[serde(default)]
    pub pending_requests: Vec<PendingRequest>,
    #[serde(default)]
    pub retired_turns: Vec<String>,
    #[serde(default)]
    pub retired_sessions: Vec<String>,
}

#[derive(Debug, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Observation {
    pub schema_version: u32,
    pub version: String,
    pub event: String,
    pub sample: u64,
    pub session_id: Option<String>,
    pub turn_id: Option<String>,
    pub tool_name: Option<String>,
    pub tool_use_id: Option<String>,
    pub source: Option<String>,
    #[serde(default, rename = "stop_hook_active")]
    pub _stop_hook_active: Option<bool>,
    pub error_present: Option<bool>,
}

impl Observation {
    pub fn validate(&self) -> Result<()> {
        if self.schema_version != 1
            || !SUPPORTED.contains(&self.version.as_str())
            || self.sample == 0
        {
            bail!("unsupported Codex observation");
        }
        if !SUPPORTED_EVENTS.contains(&self.event.as_str()) {
            bail!("unsupported Codex event {}", self.event);
        }

        // Require nonempty session_id for every event
        match &self.session_id {
            Some(s) if !s.is_empty() && s.len() <= 256 && !s.chars().any(char::is_control) => {}
            _ => bail!("missing, empty, or invalid bounded session_id"),
        }

        // Turn-scoped events require nonempty bounded turn_id
        let is_turn_scoped = matches!(
            self.event.as_str(),
            "UserPromptSubmit"
                | "PreToolUse"
                | "PermissionRequest"
                | "PostToolUse"
                | "Stop"
                | "Interrupt"
        );
        if is_turn_scoped {
            match &self.turn_id {
                Some(t) if !t.is_empty() && t.len() <= 256 && !t.chars().any(char::is_control) => {}
                _ => bail!("missing, empty, or invalid bounded turn_id for turn-scoped event"),
            }
        }

        let check_bounded = |opt: &Option<String>, name: &str| -> Result<()> {
            if let Some(s) = opt {
                if s.is_empty() || s.len() > 256 || s.chars().any(char::is_control) {
                    bail!("invalid bounded Codex {}", name);
                }
            }
            Ok(())
        };
        check_bounded(&self.tool_name, "tool_name")?;
        check_bounded(&self.tool_use_id, "tool_use_id")?;

        if self.event == "SessionStart" {
            match &self.source {
                Some(src) if ["startup", "resume", "clear", "compact"].contains(&src.as_str()) => {}
                _ => bail!("missing or invalid SessionStart source"),
            }
        }

        Ok(())
    }

    pub fn project(&self, state: &mut AgentState) -> Result<PartialSourceRecord> {
        self.validate()?;

        let mut journal: Correlation = state
            .adapter_state
            .get("codex")
            .map(|v| serde_json::from_value(v.clone()))
            .transpose()?
            .unwrap_or_default();

        // Strict sample ordering
        if self.sample <= journal.sample
            || state
                .sources
                .get("codex")
                .is_some_and(|old| self.sample <= old.source_revision)
        {
            bail!("obsolete Codex callback");
        }

        // Session binding check
        if journal.session_id.is_none() {
            journal.session_id = self.session_id.clone();
        } else if self.session_id != journal.session_id {
            if self.event != "SessionStart" {
                bail!("foreign Codex session");
            }
            let is_resume = self.source.as_deref() == Some("resume");
            if let Some(sess) = &self.session_id {
                if journal.retired_sessions.contains(sess) {
                    if is_resume {
                        // Allow explicit resume of retired session
                        journal.retired_sessions.retain(|s| s != sess);
                    } else {
                        bail!("replay of retired Codex session");
                    }
                }
            }
            // Retire old session
            if let Some(old) = journal.session_id.take() {
                if journal.retired_sessions.len() >= 64 {
                    journal.retired_sessions.remove(0);
                }
                journal.retired_sessions.push(old);
            }
            journal.session_id = self.session_id.clone();
            journal.turn_id = None;
            journal.turn += 1;
            journal.stopped = false;
            journal.cancelled = false;
            journal.is_working = false;
            journal.pending_requests.clear();
            journal.retired_turns.clear();
            state.sources.remove("codex");
        }

        // Turn correlation & retired turns
        let is_turn_scoped = matches!(
            self.event.as_str(),
            "UserPromptSubmit"
                | "PreToolUse"
                | "PermissionRequest"
                | "PostToolUse"
                | "Stop"
                | "Interrupt"
        );
        if is_turn_scoped {
            let turn = self.turn_id.as_ref().unwrap();
            if journal.retired_turns.contains(turn) {
                bail!("replay of retired Codex turn");
            }
            if journal.turn_id.is_none() {
                if matches!(
                    self.event.as_str(),
                    "UserPromptSubmit" | "PreToolUse" | "PermissionRequest"
                ) {
                    // First work event initializes turn
                    journal.turn += 1;
                    journal.turn_id = self.turn_id.clone();
                    journal.is_working = true;
                    journal.stopped = false;
                    journal.cancelled = false;
                } else {
                    bail!("orphan turn event without active turn");
                }
            } else if self.turn_id != journal.turn_id {
                if self.event == "UserPromptSubmit" {
                    // Retire previous turn
                    if let Some(old_t) = journal.turn_id.take() {
                        if journal.retired_turns.len() >= 64 {
                            journal.retired_turns.remove(0);
                        }
                        journal.retired_turns.push(old_t);
                    }
                    journal.turn += 1;
                    journal.turn_id = self.turn_id.clone();
                    journal.stopped = false;
                    journal.cancelled = false;
                    journal.is_working = true;
                    journal.pending_requests.clear();
                    state.sources.remove("codex");
                } else {
                    bail!("mismatched turn-scoped event for inactive/stale turn");
                }
            }
        }

        let activity;
        let mut successful_completion = None;
        let mut execution_error = None;
        let mut capabilities = vec!["activity".to_string(), "permissions".to_string()];

        match self.event.as_str() {
            "SessionStart" => {
                let src = self.source.as_deref().unwrap();
                if src == "compact" {
                    // Compact preserves current work / permissions / terminal
                    if journal.is_working {
                        activity = ActivityKind::Working;
                    } else {
                        activity = ActivityKind::Idle;
                    }
                    if journal.stopped {
                        capabilities.push("terminal".to_string());
                    }
                } else {
                    // startup, resume, clear resets to Idle and clears prior source
                    journal.stopped = false;
                    journal.cancelled = false;
                    journal.is_working = false;
                    journal.pending_requests.clear();
                    if let Some(old_t) = journal.turn_id.take() {
                        if journal.retired_turns.len() >= 64 {
                            journal.retired_turns.remove(0);
                        }
                        journal.retired_turns.push(old_t);
                    }
                    state.sources.remove("codex");
                    activity = ActivityKind::Idle;
                }
            }
            "UserPromptSubmit" => {
                if journal.stopped {
                    // Same-turn Stop continuation reopens generation
                    journal.turn += 1;
                }
                journal.stopped = false;
                journal.cancelled = false;
                journal.is_working = true;
                journal.pending_requests.clear();
                state.sources.remove("codex");
                activity = ActivityKind::Working;
            }
            "PreToolUse" => {
                if journal.cancelled {
                    bail!("cannot process tool event on cancelled turn");
                }
                if journal.stopped {
                    // Continuation via tool hook in same turn reopens generation
                    journal.turn += 1;
                    journal.stopped = false;
                    state.sources.remove("codex");
                }
                journal.is_working = true;
                activity = ActivityKind::Working;
                // Preserves existing pending requests
            }
            "PermissionRequest" => {
                if journal.stopped || journal.cancelled {
                    bail!("cannot request permission on terminal or cancelled turn");
                }
                let req_id = self
                    .tool_use_id
                    .clone()
                    .unwrap_or_else(|| format!("codex-perm-{}", self.sample));
                if !journal.pending_requests.iter().any(|r| r.id == req_id)
                    && journal.pending_requests.len() < 32
                {
                    journal.pending_requests.push(PendingRequest {
                        id: req_id,
                        kind: RequestKind::Permission,
                        detail: self.tool_name.clone(),
                    });
                }
                journal.is_working = true;
                activity = ActivityKind::Working;
            }
            "PostToolUse" => {
                if journal.stopped || journal.cancelled {
                    bail!("cannot process post-tool on terminal or cancelled turn");
                }
                let mut removed = false;
                if let Some(tool_use_id) = &self.tool_use_id {
                    if let Some(pos) = journal
                        .pending_requests
                        .iter()
                        .position(|r| r.id == *tool_use_id)
                    {
                        journal.pending_requests.remove(pos);
                        removed = true;
                    }
                }
                if !removed {
                    if let Some(tool_name) = &self.tool_name {
                        if let Some(pos) = journal.pending_requests.iter().position(|r| {
                            r.id.starts_with("codex-perm-") && r.detail.as_ref() == Some(tool_name)
                        }) {
                            journal.pending_requests.remove(pos);
                        }
                    }
                }
                journal.is_working = true;
                activity = ActivityKind::Working;
            }
            "Stop" => {
                if journal.cancelled {
                    bail!("cannot Stop an interrupted turn");
                }
                if !journal.is_working && !journal.stopped {
                    bail!("cannot Stop inactive turn");
                }

                // Stop notification idempotent on same turn
                journal.stopped = true;
                journal.is_working = false;
                journal.pending_requests.clear();
                activity = ActivityKind::Idle;

                let wire_turn = journal.turn_id.clone().map(TurnId);
                if self.error_present == Some(true) {
                    execution_error = Some(ExecutionError {
                        turn_id: wire_turn,
                        turn_epoch: 1,
                        turn_revision: journal.turn,
                        message: "Codex execution failed".into(),
                        is_terminal: true,
                    });
                } else {
                    successful_completion = Some(SuccessfulCompletion {
                        turn_id: wire_turn,
                        turn_epoch: 1,
                        turn_revision: journal.turn,
                        completed_at_ms: crate::agent_store::now_ms(),
                        summary: None,
                    });
                }
                capabilities.push("terminal".to_string());
            }
            "Interrupt" => {
                journal.cancelled = true;
                journal.stopped = false;
                journal.is_working = false;
                journal.pending_requests.clear();
                state.sources.remove("codex");
                activity = ActivityKind::Idle;
            }
            "SessionEnd" => {
                if let Some(old_t) = journal.turn_id.take() {
                    if journal.retired_turns.len() >= 64 {
                        journal.retired_turns.remove(0);
                    }
                    journal.retired_turns.push(old_t);
                }
                journal.stopped = false;
                journal.cancelled = false;
                journal.is_working = false;
                journal.pending_requests.clear();
                state.sources.remove("codex");
                activity = ActivityKind::Idle;
            }
            _ => bail!("unsupported Codex event {}", self.event),
        }

        journal.sample = journal.sample.max(self.sample);

        let partial = PartialSourceRecord {
            source_id: "codex".into(),
            source_epoch: Some(1),
            source_revision: Some(self.sample),
            conversation_id: self
                .session_id
                .clone()
                .or_else(|| journal.session_id.clone())
                .map(ConversationId),
            turn_epoch: Some(1),
            turn_revision: Some(journal.turn),
            turn_id: journal.turn_id.clone().map(TurnId),
            activity: Some(activity),
            pending_requests: Some(journal.pending_requests.clone()),
            background_task_count: Some(0),
            successful_completion,
            execution_error,
            capabilities: Some(capabilities),
            ..Default::default()
        };

        state
            .adapter_state
            .insert("codex".into(), serde_json::to_value(journal)?);

        Ok(partial)
    }
}

pub fn run(pane: u32, pid: u32, birth: u64) -> Result<()> {
    let process = crate::process::identity(pid)?;
    if process.start_jiffies != birth {
        bail!("Codex process lifetime changed");
    }
    let executable = std::fs::read_link(format!("/proc/{pid}/exe"))?;
    if !executable
        .file_name()
        .and_then(|s| s.to_str())
        .is_some_and(|s| s == "codex" || s.starts_with("codex."))
    {
        bail!("callback owner is not Codex");
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
                                crate::process::verify_foreground(server, leader, &process).is_ok()
                            })
                    })
                })
            })
        })
        .collect();

    if owners.len() != 1 {
        bail!("Codex foreground pane ownership unavailable or ambiguous");
    }

    let mut buffer = Vec::new();
    std::io::stdin().take(262145).read_to_end(&mut buffer)?;
    if buffer.len() > 262144 {
        bail!("oversized Codex report");
    }

    let observation: Observation = serde_json::from_slice(&buffer)?;
    // Validate observation BEFORE registration
    observation.validate()?;

    let instance = crate::agent_cli::register(crate::agent_cli::RegisterArgs {
        session: owners[0].name.clone(),
        pane,
        pid,
        kind: "codex".into(),
        synthetic: false,
        runner_id: None,
    })?;

    crate::agent_store::update(
        &instance,
        |state| observation.project(state),
        |_| {
            let current = crate::process::identity(pid)?;
            if current != process {
                bail!("process identity changed during observation update");
            }
            Ok(())
        },
    )?;

    Ok(())
}

#[cfg(test)]
pub mod tests {
    use super::*;
    use serde_json::{json, Value};

    fn test_state() -> AgentState {
        serde_json::from_value(json!({
            "schema_version": 1,
            "agent_instance_id": "codex-test-inst",
            "record_revision": 1,
            "updated_at_ms": 1,
            "sources": {},
            "reduced": {
                "status": "unknown",
                "current_turn_epoch": 0,
                "current_turn_revision": 0,
                "background_task_count": 0,
                "completion_revision": 0
            },
            "completion_revision": 0,
            "completed_turn_epoch": 0,
            "completed_turn_revision": 0
        }))
        .unwrap()
    }

    fn apply(state: &mut AgentState, event: &str, sample: u64, fields: Value) -> Result<()> {
        let mut value = json!({
            "schema_version": 1,
            "version": "0.162.1",
            "event": event,
            "sample": sample,
            "session_id": "sess-1",
        });
        if let Some(obj) = fields.as_object() {
            value.as_object_mut().unwrap().extend(obj.clone());
        }
        let observation: Observation = serde_json::from_value(value)?;
        let partial = observation.project(state)?;
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
            AgentKind::Codex,
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
    fn test_lifecycle_working_needs_input_working_done_and_idempotence() {
        let mut s = test_state();

        // 1. Prompt submit -> Working
        apply(
            &mut s,
            "UserPromptSubmit",
            100,
            json!({"turn_id": "turn-1"}),
        )
        .unwrap();
        assert_eq!(s.reduced.status, AgentStatus::Working);
        assert_eq!(s.reduced.current_turn_revision, 1);
        assert_eq!(s.completion_revision, 0);

        // 2. Native PermissionRequest (HAS NO tool_use_id) -> NeedsInput
        apply(
            &mut s,
            "PermissionRequest",
            200,
            json!({"turn_id": "turn-1", "tool_name": "Bash"}),
        )
        .unwrap();
        assert_eq!(s.reduced.status, AgentStatus::NeedsInput);
        assert_eq!(s.reduced.detail, Some("permission required".to_string()));

        // 3. PostToolUse (has tool_use_id and tool_name) -> clears request -> Working
        apply(
            &mut s,
            "PostToolUse",
            300,
            json!({"turn_id": "turn-1", "tool_name": "Bash", "tool_use_id": "call_abc123"}),
        )
        .unwrap();
        assert_eq!(s.reduced.status, AgentStatus::Working);
        assert!(s.reduced.pending_requests.is_empty());

        // 4. Stop -> Done (rev 1)
        apply(&mut s, "Stop", 400, json!({"turn_id": "turn-1"})).unwrap();
        assert_eq!(s.reduced.status, AgentStatus::Done);
        assert_eq!(s.completion_revision, 1);

        // 5. Repeated Stop -> Idempotent, rev still 1
        apply(&mut s, "Stop", 500, json!({"turn_id": "turn-1"})).unwrap();
        assert_eq!(s.reduced.status, AgentStatus::Done);
        assert_eq!(s.completion_revision, 1);
    }

    #[test]
    fn test_pre_tool_use_continuation_clears_done_and_increments_revision_on_second_stop() {
        let mut s = test_state();
        apply(
            &mut s,
            "UserPromptSubmit",
            100,
            json!({"turn_id": "turn-1"}),
        )
        .unwrap();
        apply(&mut s, "Stop", 200, json!({"turn_id": "turn-1"})).unwrap();
        assert_eq!(s.reduced.status, AgentStatus::Done);
        assert_eq!(s.completion_revision, 1);

        // Fresh PreToolUse continuation on SAME turn reopens generation
        apply(&mut s, "PreToolUse", 300, json!({"turn_id": "turn-1"})).unwrap();
        assert_eq!(s.reduced.status, AgentStatus::Working);
        assert_eq!(s.completion_revision, 1);
        assert_eq!(s.reduced.current_turn_revision, 2);

        // Second stop produces completion revision 2!
        apply(&mut s, "Stop", 400, json!({"turn_id": "turn-1"})).unwrap();
        assert_eq!(s.reduced.status, AgentStatus::Done);
        assert_eq!(s.completion_revision, 2);
    }

    #[test]
    fn test_newer_prompt_rejects_older_turn_events() {
        let mut s = test_state();
        apply(
            &mut s,
            "UserPromptSubmit",
            100,
            json!({"turn_id": "turn-1"}),
        )
        .unwrap();
        apply(
            &mut s,
            "UserPromptSubmit",
            200,
            json!({"turn_id": "turn-2"}),
        )
        .unwrap();
        assert_eq!(s.reduced.current_turn_id, Some(TurnId("turn-2".into())));

        // Stale Stop from turn-1 rejected
        assert!(apply(&mut s, "Stop", 300, json!({"turn_id": "turn-1"})).is_err());
        // Stale PreToolUse from turn-1 rejected
        assert!(apply(&mut s, "PreToolUse", 310, json!({"turn_id": "turn-1"})).is_err());
        // Stale PostToolUse from turn-1 rejected
        assert!(apply(&mut s, "PostToolUse", 320, json!({"turn_id": "turn-1"})).is_err());
        // Stale PermissionRequest from turn-1 rejected
        assert!(apply(
            &mut s,
            "PermissionRequest",
            330,
            json!({"turn_id": "turn-1", "tool_name": "Bash"})
        )
        .is_err());
        // Stale Interrupt from turn-1 rejected
        assert!(apply(&mut s, "Interrupt", 340, json!({"turn_id": "turn-1"})).is_err());

        // State remains Working on turn-2
        assert_eq!(s.reduced.status, AgentStatus::Working);
        assert_eq!(s.reduced.current_turn_id, Some(TurnId("turn-2".into())));
    }

    #[test]
    fn test_interrupt_then_late_stop_rejected() {
        let mut s = test_state();
        apply(
            &mut s,
            "UserPromptSubmit",
            100,
            json!({"turn_id": "turn-1"}),
        )
        .unwrap();
        apply(&mut s, "Interrupt", 200, json!({"turn_id": "turn-1"})).unwrap();
        assert_eq!(s.reduced.status, AgentStatus::Idle);
        assert_eq!(s.completion_revision, 0);

        // Late Stop after Interrupt MUST be rejected
        assert!(apply(&mut s, "Stop", 300, json!({"turn_id": "turn-1"})).is_err());
        assert_eq!(s.reduced.status, AgentStatus::Idle);
        assert_eq!(s.completion_revision, 0);
    }

    #[test]
    fn test_compact_preserves_working_and_pending_while_startup_resets() {
        let mut s = test_state();
        apply(
            &mut s,
            "UserPromptSubmit",
            100,
            json!({"turn_id": "turn-1"}),
        )
        .unwrap();
        apply(
            &mut s,
            "PermissionRequest",
            200,
            json!({"turn_id": "turn-1", "tool_name": "Bash"}),
        )
        .unwrap();
        assert_eq!(s.reduced.status, AgentStatus::NeedsInput);

        // SessionStart with source: "compact" preserves work and pending requests
        apply(&mut s, "SessionStart", 300, json!({"source": "compact"})).unwrap();
        assert_eq!(s.reduced.status, AgentStatus::NeedsInput);
        assert_eq!(s.reduced.pending_requests.len(), 1);

        // SessionStart with source: "startup" clears and resets to Idle
        apply(&mut s, "SessionStart", 400, json!({"source": "startup"})).unwrap();
        assert_eq!(s.reduced.status, AgentStatus::Idle);
        assert!(s.reduced.pending_requests.is_empty());
    }

    #[test]
    fn test_foreign_session_and_replay_rejected() {
        let mut s = test_state();
        apply(
            &mut s,
            "UserPromptSubmit",
            100,
            json!({"turn_id": "turn-1", "session_id": "sess-1"}),
        )
        .unwrap();

        // Foreign work event without SessionStart rejected
        assert!(apply(
            &mut s,
            "UserPromptSubmit",
            200,
            json!({"turn_id": "turn-2", "session_id": "sess-2"})
        )
        .is_err());

        // SessionStart transitions to new session cleanly
        apply(
            &mut s,
            "SessionStart",
            300,
            json!({"session_id": "sess-2", "source": "startup"}),
        )
        .unwrap();
        assert_eq!(s.reduced.status, AgentStatus::Idle);

        // Old callback for sess-1 rejected
        assert!(apply(
            &mut s,
            "UserPromptSubmit",
            400,
            json!({"turn_id": "turn-1", "session_id": "sess-1"})
        )
        .is_err());

        // Replay of retired session rejected even on SessionStart
        assert!(apply(
            &mut s,
            "SessionStart",
            500,
            json!({"session_id": "sess-1", "source": "startup"})
        )
        .is_err());
    }

    #[test]
    fn test_missing_ids_and_schema_validation() {
        let mut s = test_state();

        // Empty session_id rejected
        assert!(apply(
            &mut s,
            "SessionStart",
            100,
            json!({"session_id": "", "source": "startup"})
        )
        .is_err());

        // Missing or invalid source for SessionStart rejected
        assert!(apply(&mut s, "SessionStart", 100, json!({})).is_err());
        assert!(apply(&mut s, "SessionStart", 100, json!({"source": "unknown"})).is_err());

        // Missing turn_id for turn-scoped event rejected
        assert!(apply(&mut s, "UserPromptSubmit", 100, json!({})).is_err());

        // Empty turn_id for turn-scoped event rejected
        assert!(apply(&mut s, "UserPromptSubmit", 100, json!({"turn_id": ""})).is_err());

        // Unsupported version rejected
        assert!(apply(
            &mut s,
            "SessionStart",
            100,
            json!({"version": "9.9.9", "source": "startup"})
        )
        .is_err());

        // Zero sample rejected
        assert!(apply(&mut s, "SessionStart", 0, json!({"source": "startup"})).is_err());

        // Oversized ID rejected
        assert!(apply(
            &mut s,
            "UserPromptSubmit",
            100,
            json!({"turn_id": "x".repeat(300)})
        )
        .is_err());

        // Control char ID rejected
        assert!(apply(
            &mut s,
            "UserPromptSubmit",
            100,
            json!({"turn_id": "turn\x00bad"})
        )
        .is_err());
    }

    #[test]
    fn test_first_work_event_initializes_turn() {
        let mut s = test_state();

        // Missed prompt: first event is PreToolUse
        apply(&mut s, "PreToolUse", 100, json!({"turn_id": "turn-auto"})).unwrap();
        assert_eq!(s.reduced.status, AgentStatus::Working);
        assert_eq!(
            s.reduced.current_turn_id,
            Some(TurnId("turn-auto".to_string()))
        );
        assert_eq!(s.reduced.current_turn_revision, 1);
    }
    #[test]
    fn test_permission_ids_unique_and_fallback_matching() {
        let mut s = test_state();
        apply(
            &mut s,
            "UserPromptSubmit",
            100,
            json!({"turn_id": "turn-1"}),
        )
        .unwrap();

        // 1. First native PermissionRequest (sample 200, no tool_use_id, tool_name Bash)
        apply(
            &mut s,
            "PermissionRequest",
            200,
            json!({"turn_id": "turn-1", "tool_name": "Bash"}),
        )
        .unwrap();
        assert_eq!(s.reduced.pending_requests.len(), 1);
        assert_eq!(s.reduced.pending_requests[0].id, "codex-perm-200");

        // 2. Second native PermissionRequest (sample 300, no tool_use_id, tool_name Edit)
        apply(
            &mut s,
            "PermissionRequest",
            300,
            json!({"turn_id": "turn-1", "tool_name": "Edit"}),
        )
        .unwrap();
        assert_eq!(s.reduced.pending_requests.len(), 2);
        assert_eq!(s.reduced.pending_requests[1].id, "codex-perm-300");

        // 3. Fallback PostToolUse for Bash (sample 400, no tool_use_id, tool_name Bash) removes first
        apply(
            &mut s,
            "PostToolUse",
            400,
            json!({"turn_id": "turn-1", "tool_name": "Bash"}),
        )
        .unwrap();
        assert_eq!(s.reduced.pending_requests.len(), 1);
        assert_eq!(s.reduced.pending_requests[0].id, "codex-perm-300");

        // 4. Third native PermissionRequest (sample 500, no tool_use_id, tool_name Bash)
        apply(
            &mut s,
            "PermissionRequest",
            500,
            json!({"turn_id": "turn-1", "tool_name": "Bash"}),
        )
        .unwrap();
        assert_eq!(s.reduced.pending_requests.len(), 2);
        assert_eq!(s.reduced.pending_requests[0].id, "codex-perm-300");
        assert_eq!(s.reduced.pending_requests[1].id, "codex-perm-500");

        // 5. Add an explicit pending request (tool_use_id "explicit-1", tool_name "CustomTool")
        apply(
            &mut s,
            "PermissionRequest",
            600,
            json!({"turn_id": "turn-1", "tool_name": "CustomTool", "tool_use_id": "explicit-1"}),
        )
        .unwrap();
        assert_eq!(s.reduced.pending_requests.len(), 3);

        // 6. PostToolUse with unmatched explicit tool_use_id ("explicit-unmatched", tool_name "CustomTool")
        // Must NOT remove explicit-1 via fallback!
        apply(&mut s, "PostToolUse", 700, json!({"turn_id": "turn-1", "tool_name": "CustomTool", "tool_use_id": "explicit-unmatched"})).unwrap();
        assert_eq!(s.reduced.pending_requests.len(), 3);
        assert!(s
            .reduced
            .pending_requests
            .iter()
            .any(|r| r.id == "explicit-1"));
    }

    #[test]
    fn test_session_start_retires_turn_and_prevents_late_revival() {
        let mut s = test_state();
        apply(
            &mut s,
            "UserPromptSubmit",
            100,
            json!({"turn_id": "turn-1"}),
        )
        .unwrap();
        assert_eq!(s.reduced.status, AgentStatus::Working);

        // SessionStart with startup resets active turn and retires turn-1
        apply(&mut s, "SessionStart", 200, json!({"source": "startup"})).unwrap();
        assert_eq!(s.reduced.status, AgentStatus::Idle);
        assert_eq!(s.reduced.current_turn_id, None);

        // Late PreToolUse on turn-1 rejected
        assert!(apply(&mut s, "PreToolUse", 300, json!({"turn_id": "turn-1"})).is_err());

        // Late PermissionRequest on turn-1 rejected
        assert!(apply(
            &mut s,
            "PermissionRequest",
            310,
            json!({"turn_id": "turn-1", "tool_name": "Bash"})
        )
        .is_err());

        // Late PostToolUse on turn-1 rejected
        assert!(apply(
            &mut s,
            "PostToolUse",
            320,
            json!({"turn_id": "turn-1", "tool_name": "Bash"})
        )
        .is_err());

        // Late Stop on turn-1 rejected
        assert!(apply(&mut s, "Stop", 330, json!({"turn_id": "turn-1"})).is_err());
    }

    #[test]
    fn test_session_resume_unretires_older_session() {
        let mut s = test_state();
        // Session 1 runs and finishes
        apply(
            &mut s,
            "UserPromptSubmit",
            100,
            json!({"session_id": "sess-1", "turn_id": "turn-1"}),
        )
        .unwrap();
        apply(
            &mut s,
            "Stop",
            200,
            json!({"session_id": "sess-1", "turn_id": "turn-1"}),
        )
        .unwrap();
        assert_eq!(s.reduced.status, AgentStatus::Done);

        // Switch to Session 2 via startup
        apply(
            &mut s,
            "SessionStart",
            300,
            json!({"session_id": "sess-2", "source": "startup"}),
        )
        .unwrap();
        assert_eq!(s.reduced.status, AgentStatus::Idle);
        apply(
            &mut s,
            "UserPromptSubmit",
            400,
            json!({"session_id": "sess-2", "turn_id": "turn-2"}),
        )
        .unwrap();
        apply(
            &mut s,
            "Stop",
            500,
            json!({"session_id": "sess-2", "turn_id": "turn-2"}),
        )
        .unwrap();
        assert_eq!(s.reduced.status, AgentStatus::Done);

        // User explicitly resumes Session 1 (which was retired)
        apply(
            &mut s,
            "SessionStart",
            600,
            json!({"session_id": "sess-1", "source": "resume"}),
        )
        .unwrap();
        // Switching back must NOT create false terminal
        assert_eq!(s.reduced.status, AgentStatus::Idle);

        // Session 2 is now retired: old callbacks for sess-2 must reject
        assert!(apply(
            &mut s,
            "UserPromptSubmit",
            700,
            json!({"session_id": "sess-2", "turn_id": "turn-3"})
        )
        .is_err());

        // Session 1 can proceed with work
        apply(
            &mut s,
            "UserPromptSubmit",
            800,
            json!({"session_id": "sess-1", "turn_id": "turn-4"}),
        )
        .unwrap();
        assert_eq!(s.reduced.status, AgentStatus::Working);
    }
}
