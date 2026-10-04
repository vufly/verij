//! Normalized agent execution and monitoring wire types.
//!
//! Zero native OS or WASM dependencies; relies solely on serde.
//! Source records, monotonic revisions, pure state reducer and visit proof.

pub use crate::identity::{
    valid_record_key, AgentInstanceId, ConversationId, HostKey, PaneKey, ProcessIdentity,
    SessionInstanceId, TerminalPaneId, TurnId,
};
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;

pub const AGENT_SCHEMA_VERSION: u32 = 1;

/// Monitored agent flavor.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentKind {
    Opencode,
    Agy,
    #[serde(other)]
    Unknown,
}

impl std::fmt::Display for AgentKind {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Opencode => write!(f, "opencode"),
            Self::Agy => write!(f, "agy"),
            Self::Unknown => write!(f, "unknown"),
        }
    }
}

/// Normalized display and execution status.
///
/// Precedence order: NeedsInput > Working > Error > Done > Idle > Unknown.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum AgentStatus {
    NeedsInput,
    Working,
    Error,
    Done,
    Idle,
    Unknown,
}

impl AgentStatus {
    pub fn precedence(&self) -> u8 {
        match self {
            Self::NeedsInput => 6,
            Self::Working => 5,
            Self::Error => 4,
            Self::Done => 3,
            Self::Idle => 2,
            Self::Unknown => 1,
        }
    }
}

/// Immutable identity binding for an agent process in a terminal pane.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentIdentity {
    pub schema_version: u32,
    pub agent_instance_id: AgentInstanceId,
    pub pane_key: PaneKey,
    pub kind: AgentKind,
    pub process: ProcessIdentity,
    pub registered_at_ms: u64,
    #[serde(default)]
    pub is_synthetic: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub runner_id: Option<String>,
}

/// Normalized activity reported by an observer source.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ActivityKind {
    Idle,
    Thinking,
    Working,
    Retrying,
    Initializing,
    #[serde(other)]
    Unknown,
}

/// Kind of user input requested by an agent.
#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum RequestKind {
    Permission,
    Question,
    Confirmation,
    #[serde(untagged)]
    Other(String),
}

/// Pending input request requiring user attention.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingRequest {
    pub id: String,
    pub kind: RequestKind,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// Authoritative successful execution outcome.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct SuccessfulCompletion {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<TurnId>,
    pub turn_epoch: u64,
    pub turn_revision: u64,
    pub completed_at_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub summary: Option<String>,
}

/// Authoritative execution error outcome.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ExecutionError {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<TurnId>,
    pub turn_epoch: u64,
    pub turn_revision: u64,
    pub message: String,
    pub is_terminal: bool,
}

/// Full normalized observation snapshot stored per source.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NormalizedSourceRecord {
    pub schema_version: u32,
    pub agent_instance_id: AgentInstanceId,
    pub source_id: String,
    pub source_epoch: u64,
    pub source_revision: u64,
    pub observed_at_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation_id: Option<ConversationId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation_title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<TurnId>,
    #[serde(default)]
    pub turn_epoch: u64,
    #[serde(default)]
    pub turn_revision: u64,
    #[serde(default)]
    pub activity: Option<ActivityKind>,
    #[serde(default)]
    pub pending_requests: Vec<PendingRequest>,
    #[serde(default)]
    pub background_task_count: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub successful_completion: Option<SuccessfulCompletion>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub execution_error: Option<ExecutionError>,
    #[serde(default)]
    pub capabilities: Vec<String>,
}

/// Incoming partial report. Missing fields preserve existing state on merge.
///
/// Note on field clearing: serde deserialization maps both JSON `null` and absent fields to `None`.
/// To explicitly clear collections or counts, provide empty containers or zero (e.g. `pending_requests: []`,
/// `background_task_count: 0`). Terminal outcomes (`successful_completion`, `execution_error`) are scoped to
/// turn identity; advancing to a new turn or new epoch automatically resets prior turn execution facts and terminal outcomes.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PartialSourceRecord {
    #[serde(default = "default_schema_version")]
    pub schema_version: u32,
    #[serde(default)]
    pub agent_instance_id: Option<AgentInstanceId>,
    pub source_id: String,
    pub source_epoch: Option<u64>,
    pub source_revision: Option<u64>,
    pub observed_at_ms: Option<u64>,
    pub conversation_id: Option<ConversationId>,
    pub conversation_title: Option<String>,
    pub turn_id: Option<TurnId>,
    pub turn_epoch: Option<u64>,
    pub turn_revision: Option<u64>,
    pub activity: Option<ActivityKind>,
    pub pending_requests: Option<Vec<PendingRequest>>,
    pub background_task_count: Option<usize>,
    pub successful_completion: Option<SuccessfulCompletion>,
    pub execution_error: Option<ExecutionError>,
    pub capabilities: Option<Vec<String>>,
}

fn default_schema_version() -> u32 {
    AGENT_SCHEMA_VERSION
}

impl Default for PartialSourceRecord {
    fn default() -> Self {
        Self {
            schema_version: AGENT_SCHEMA_VERSION,
            agent_instance_id: None,
            source_id: String::new(),
            source_epoch: None,
            source_revision: None,
            observed_at_ms: None,
            conversation_id: None,
            conversation_title: None,
            turn_id: None,
            turn_epoch: None,
            turn_revision: None,
            activity: None,
            pending_requests: None,
            background_task_count: None,
            successful_completion: None,
            execution_error: None,
            capabilities: None,
        }
    }
}

impl PartialSourceRecord {
    /// Merge partial observation into existing record, validating strict monotonic order.
    pub fn merge_into(
        &self,
        target_instance_id: &AgentInstanceId,
        existing: Option<&NormalizedSourceRecord>,
        now_ms: u64,
    ) -> Result<NormalizedSourceRecord, String> {
        if self.schema_version != AGENT_SCHEMA_VERSION {
            return Err(format!(
                "unsupported schema_version {}; expected {}",
                self.schema_version, AGENT_SCHEMA_VERSION
            ));
        }

        if let Some(ref supplied) = self.agent_instance_id {
            if supplied != target_instance_id {
                return Err(format!(
                    "mismatched agent_instance_id: supplied {:?} != target {:?}",
                    supplied, target_instance_id
                ));
            }
        }

        if self.source_id.is_empty()
            || self.source_id.len() > 128
            || !valid_record_key(&self.source_id)
        {
            return Err(format!("invalid bounded source_id: {:?}", self.source_id));
        }

        let inc_epoch = self.source_epoch.ok_or_else(|| {
            "missing source_epoch in partial record; strictly required for ordered snapshots"
                .to_string()
        })?;
        let inc_rev = self.source_revision.ok_or_else(|| {
            "missing source_revision in partial record; strictly required for ordered snapshots"
                .to_string()
        })?;
        if inc_epoch == 0 || inc_rev == 0 {
            return Err("source_epoch and source_revision must be >= 1".to_string());
        }

        if let Some(curr) = existing {
            if inc_epoch < curr.source_epoch {
                return Err(format!(
                    "stale source record: incoming epoch {} < existing epoch {}",
                    inc_epoch, curr.source_epoch
                ));
            }
            if inc_epoch == curr.source_epoch && inc_rev <= curr.source_revision {
                return Err(format!(
                    "stale or duplicate source record: incoming ({}, {}) <= existing ({}, {})",
                    inc_epoch, inc_rev, curr.source_epoch, curr.source_revision
                ));
            }
        }

        // Source lifetime changes reset source facts instead of inheriting old epoch state
        let is_new_epoch = existing.map(|c| inc_epoch > c.source_epoch).unwrap_or(true);

        let (is_new_turn, base_curr) = if is_new_epoch {
            (true, None)
        } else {
            let curr = existing.unwrap();
            let inc_turn_epoch = self.turn_epoch.unwrap_or(curr.turn_epoch);
            let inc_turn_rev = self.turn_revision.unwrap_or(curr.turn_revision);

            if self.turn_epoch.is_some() || self.turn_revision.is_some() {
                if (inc_turn_epoch, inc_turn_rev) < (curr.turn_epoch, curr.turn_revision) {
                    return Err(format!(
                        "lower turn rejected from same producer: incoming ({}, {}) < existing ({}, {})",
                        inc_turn_epoch, inc_turn_rev, curr.turn_epoch, curr.turn_revision
                    ));
                }
            }

            let is_new = (inc_turn_epoch, inc_turn_rev) > (curr.turn_epoch, curr.turn_revision);
            (is_new, Some(curr))
        };

        let turn_epoch = if let Some(curr) = base_curr {
            self.turn_epoch.unwrap_or(curr.turn_epoch)
        } else {
            self.turn_epoch.unwrap_or(0)
        };

        let turn_revision = if let Some(curr) = base_curr {
            self.turn_revision.unwrap_or(curr.turn_revision)
        } else {
            self.turn_revision.unwrap_or(0)
        };

        let turn_id = if is_new_turn {
            self.turn_id.clone()
        } else {
            self.turn_id
                .clone()
                .or_else(|| base_curr.and_then(|c| c.turn_id.clone()))
        };

        let activity = if is_new_turn {
            self.activity
        } else {
            self.activity.or_else(|| base_curr.and_then(|c| c.activity))
        };

        let pending_requests = match &self.pending_requests {
            Some(reqs) => reqs.clone(),
            None => {
                if is_new_turn {
                    Vec::new()
                } else {
                    base_curr
                        .map(|c| c.pending_requests.clone())
                        .unwrap_or_default()
                }
            }
        };

        let background_task_count = match self.background_task_count {
            Some(cnt) => cnt,
            None => {
                if is_new_turn {
                    0
                } else {
                    base_curr.map(|c| c.background_task_count).unwrap_or(0)
                }
            }
        };

        // Source authority normalization:
        // Agy UI source cannot declare terminal outcomes
        let is_agy_ui = self.source_id == "agy-ui";
        let (successful_completion, execution_error) = if is_agy_ui {
            (None, None)
        } else {
            let succ = match &self.successful_completion {
                Some(s) => Some(s.clone()),
                None => {
                    if is_new_turn {
                        None
                    } else {
                        base_curr.and_then(|c| c.successful_completion.clone())
                    }
                }
            };
            let err = match &self.execution_error {
                Some(e) => Some(e.clone()),
                None => {
                    if is_new_turn {
                        None
                    } else {
                        base_curr.and_then(|c| c.execution_error.clone())
                    }
                }
            };
            (succ, err)
        };

        let conversation_id = self
            .conversation_id
            .clone()
            .or_else(|| base_curr.and_then(|c| c.conversation_id.clone()));

        let sanitized_title = self
            .conversation_title
            .as_ref()
            .map(|t| sanitize_string(t, 256))
            .or_else(|| base_curr.and_then(|c| c.conversation_title.clone()));

        let capabilities = self
            .capabilities
            .clone()
            .or_else(|| base_curr.map(|c| c.capabilities.clone()))
            .unwrap_or_default();

        Ok(NormalizedSourceRecord {
            schema_version: self.schema_version,
            agent_instance_id: target_instance_id.clone(),
            source_id: self.source_id.clone(),
            source_epoch: inc_epoch,
            source_revision: inc_rev,
            observed_at_ms: self.observed_at_ms.unwrap_or(now_ms),
            conversation_id,
            conversation_title: sanitized_title,
            turn_id,
            turn_epoch,
            turn_revision,
            activity,
            pending_requests,
            background_task_count,
            successful_completion,
            execution_error,
            capabilities,
        })
    }
}

/// Reduced facts derived across all observer sources for an agent.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct ReducedExecutionState {
    pub status: AgentStatus,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub current_turn_id: Option<TurnId>,
    pub current_turn_epoch: u64,
    pub current_turn_revision: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation_id: Option<ConversationId>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation_title: Option<String>,
    #[serde(default)]
    pub pending_requests: Vec<PendingRequest>,
    pub background_task_count: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest_error: Option<ExecutionError>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest_completion: Option<SuccessfulCompletion>,
    pub completion_revision: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub detail: Option<String>,
}

/// Complete mutable state file persisted at `<runtime>/agents/v1/<id>/state.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AgentState {
    pub schema_version: u32,
    pub agent_instance_id: AgentInstanceId,
    pub record_revision: u64,
    pub updated_at_ms: u64,
    pub sources: BTreeMap<String, NormalizedSourceRecord>,
    pub reduced: ReducedExecutionState,
    pub completion_revision: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub completed_turn: Option<TurnId>,
    pub completed_turn_epoch: u64,
    pub completed_turn_revision: u64,
}

/// Fresh proof of whole-host and inner focus required for completion acknowledgement.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct VisitProof {
    pub host_key: HostKey,
    pub agent_instance_id: AgentInstanceId,
    pub pane_key: PaneKey,
    pub observed_completion_revision: u64,
    pub request_id: String,
    pub whole_host_focused: bool,
    pub inner_focus_verified: bool,
    pub verified_at_ms: u64,
}

/// Host-local record of a visited completion.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct InstanceAck {
    pub acknowledged_revision: u64,
    pub acknowledged_at_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub turn_id: Option<TurnId>,
    pub request_id: String,
}

/// Persistent host acknowledgements stored at `hosts/<key>.json`.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostAckStore {
    pub schema_version: u32,
    pub host_key: HostKey,
    pub updated_at_ms: u64,
    pub acknowledgements: BTreeMap<String, InstanceAck>,
}

impl HostAckStore {
    pub fn new(host_key: HostKey, now_ms: u64) -> Self {
        Self {
            schema_version: AGENT_SCHEMA_VERSION,
            host_key,
            updated_at_ms: now_ms,
            acknowledgements: BTreeMap::new(),
        }
    }
}

/// Sanitize display string: strip control chars, bound length. Never parse prompts.
pub fn sanitize_string(s: &str, max_len: usize) -> String {
    let sanitized: String = s
        .chars()
        .filter(|c| !c.is_control() || *c == ' ' || *c == '\t')
        .collect();
    let trimmed = sanitized.trim();
    if trimmed.chars().count() <= max_len {
        trimmed.to_string()
    } else {
        trimmed.chars().take(max_len).collect()
    }
}

/// Check if source is accepted under explicit source authority.
pub fn is_source_accepted(source_id: &str, kind: AgentKind, is_synthetic: bool) -> bool {
    match source_id {
        "opencode" => kind == AgentKind::Opencode || is_synthetic,
        "agy-ui" | "agy-lifecycle" => kind == AgentKind::Agy || is_synthetic,
        "magy-watch" => true,
        s if is_synthetic && s.starts_with("fixture-") => true,
        _ => false,
    }
}

/// Check if source has authority to declare terminal outcomes (completion or terminal error).
pub fn can_declare_terminal(source_id: &str, kind: AgentKind, is_synthetic: bool) -> bool {
    if !is_source_accepted(source_id, kind, is_synthetic) {
        return false;
    }
    match source_id {
        "agy-ui" => false, // UI source cannot declare terminal outcomes
        "agy-lifecycle" | "opencode" | "magy-watch" => true,
        s if is_synthetic && s.starts_with("fixture-") => true,
        _ => false,
    }
}

/// Check if source has authority to declare pending user requests.
pub fn can_declare_pending_requests(source_id: &str, kind: AgentKind, is_synthetic: bool) -> bool {
    if !is_source_accepted(source_id, kind, is_synthetic) {
        return false;
    }
    match source_id {
        "agy-lifecycle" => false, // Lifecycle source cannot declare pending input requests
        "agy-ui" | "opencode" => true,
        s if is_synthetic && s.starts_with("fixture-") => true,
        _ => false,
    }
}

fn source_family_priority(source_id: &str, kind: AgentKind) -> u8 {
    match kind {
        AgentKind::Agy => match source_id {
            "agy-ui" => 10,
            "agy-lifecycle" => 8,
            "magy-watch" => 5,
            _ => 1,
        },
        AgentKind::Opencode => match source_id {
            "opencode" => 10,
            "magy-watch" => 5,
            _ => 1,
        },
        AgentKind::Unknown => match source_id {
            "magy-watch" => 5,
            _ => 1,
        },
    }
}

/// Pure reducer combining normalized source records into reduced execution state.
///
/// Enforces:
/// 1. NeedsInput > Working > Error > Done > Idle > Unknown.
/// 2. Filter facts to current turn; stale stops from older turns cannot clear newer active turns.
/// 3. Aggregate completion requires task quiescence (`background_task_count == 0`).
/// 4. Qualified lifecycle terminal results resolve earlier same-turn work/input (no abandoned requests lingering).
/// 5. Late UI refresh cannot override authoritative terminal results; no fabricated terminal from UI idle.
/// 6. No wallclock ordering; completion idempotence requires genuine non-empty turn/order; zero turn cannot invent completion.
/// 7. Ignored/unsupported sources remain conservative Unknown.
pub fn reduce_sources(
    sources: &BTreeMap<String, NormalizedSourceRecord>,
    kind: AgentKind,
    is_synthetic: bool,
    prev_completion_rev: u64,
    prev_completed_turn: Option<&TurnId>,
    prev_completed_epoch: u64,
    prev_completed_rev: u64,
) -> (ReducedExecutionState, u64, Option<TurnId>, u64, u64) {
    let empty_reduced = ReducedExecutionState {
        status: AgentStatus::Unknown,
        current_turn_id: None,
        current_turn_epoch: 0,
        current_turn_revision: 0,
        conversation_id: None,
        conversation_title: None,
        pending_requests: Vec::new(),
        background_task_count: 0,
        latest_error: None,
        latest_completion: None,
        completion_revision: prev_completion_rev,
        detail: None,
    };

    if sources.is_empty() {
        return (
            empty_reduced,
            prev_completion_rev,
            prev_completed_turn.cloned(),
            prev_completed_epoch,
            prev_completed_rev,
        );
    }

    // Step 0: Filter sources to accepted sources only
    let mut accepted_sources: Vec<(&String, &NormalizedSourceRecord)> = sources
        .iter()
        .filter(|(id, _)| is_source_accepted(id, kind, is_synthetic))
        .collect();

    // A local current-state source owns conversation selection. A delayed
    // terminal event from another family must not create Done for this row.
    let family = accepted_sources
        .iter()
        .filter(|(id, _)| source_family_priority(id, kind) == 10)
        .max_by_key(|(_, source)| {
            (
                source.turn_epoch,
                source.turn_revision,
                source.source_epoch,
                source.source_revision,
            )
        })
        .and_then(|(_, source)| source.conversation_id.clone());
    if let Some(family) = family.as_ref() {
        accepted_sources.retain(|(_, source)| {
            source.conversation_id.as_ref() == Some(family) || source.conversation_id.is_none()
        });
    }

    if accepted_sources.is_empty() {
        return (
            empty_reduced,
            prev_completion_rev,
            prev_completed_turn.cloned(),
            prev_completed_epoch,
            prev_completed_rev,
        );
    }

    // Step 1: Find latest turn epoch & revision across authoritative sources
    let mut current_turn_epoch = 0u64;
    let mut current_turn_rev = 0u64;
    let mut current_turn_id: Option<TurnId> = None;

    for (_, src) in &accepted_sources {
        if family.is_some() && src.conversation_id.as_ref() != family.as_ref() {
            continue;
        }
        if (src.turn_epoch, src.turn_revision) > (current_turn_epoch, current_turn_rev) {
            current_turn_epoch = src.turn_epoch;
            current_turn_rev = src.turn_revision;
            current_turn_id = src.turn_id.clone();
        } else if (src.turn_epoch, src.turn_revision) == (current_turn_epoch, current_turn_rev)
            && current_turn_id.is_none()
        {
            current_turn_id = src.turn_id.clone();
        }
    }

    // Step 2: Extract active conversation metadata
    // Pick current family / turn, not alphabetical older-source
    let mut current_turn_sources: Vec<(&String, &NormalizedSourceRecord)> = accepted_sources
        .iter()
        .copied()
        .filter(|(_, src)| {
            if family.is_some() && src.conversation_id.as_ref() != family.as_ref() {
                return false;
            }
            (src.turn_epoch, src.turn_revision) == (current_turn_epoch, current_turn_rev)
        })
        .collect();

    current_turn_sources.sort_by(|(id_a, src_a), (id_b, src_b)| {
        let prio_a = source_family_priority(id_a, kind);
        let prio_b = source_family_priority(id_b, kind);
        prio_b.cmp(&prio_a).then_with(|| {
            (src_b.source_epoch, src_b.source_revision)
                .cmp(&(src_a.source_epoch, src_a.source_revision))
        })
    });

    let mut conv_id = None;
    let mut conv_title = None;

    for (_, src) in &current_turn_sources {
        if conv_id.is_none() && src.conversation_id.is_some() {
            conv_id = src.conversation_id.clone();
        }
        if conv_title.is_none() && src.conversation_title.is_some() {
            conv_title = src.conversation_title.clone();
        }
    }

    // Fallback to other accepted sources outside current turn if needed
    if conv_id.is_none() || conv_title.is_none() {
        let mut other_sources: Vec<(&String, &NormalizedSourceRecord)> = accepted_sources
            .iter()
            .copied()
            .filter(|(_, src)| {
                (src.turn_epoch, src.turn_revision) != (current_turn_epoch, current_turn_rev)
            })
            .collect();
        other_sources.sort_by(|(_, src_a), (_, src_b)| {
            (
                src_b.turn_epoch,
                src_b.turn_revision,
                src_b.source_epoch,
                src_b.source_revision,
            )
                .cmp(&(
                    src_a.turn_epoch,
                    src_a.turn_revision,
                    src_a.source_epoch,
                    src_a.source_revision,
                ))
        });
        for (_, src) in other_sources {
            if conv_id.is_none() && src.conversation_id.is_some() {
                conv_id = src.conversation_id.clone();
            }
            if conv_title.is_none() && src.conversation_title.is_some() {
                conv_title = src.conversation_title.clone();
            }
        }
    }

    // Step 3: Check qualified terminal outcomes for current turn
    // Completion idempotence requires genuine nonempty turn/order; zero turn must not invent completion
    let has_nonempty_turn =
        current_turn_epoch > 0 || current_turn_rev > 0 || current_turn_id.is_some();
    let mut terminal_error: Option<ExecutionError> = None;
    let mut successful_completion: Option<SuccessfulCompletion> = None;

    if has_nonempty_turn {
        for (src_id, src) in &accepted_sources {
            if family.is_some() && src.conversation_id.as_ref() != family.as_ref() {
                continue;
            }
            if !can_declare_terminal(src_id, kind, is_synthetic) {
                continue;
            }
            if let Some(err) = &src.execution_error {
                if (err.turn_epoch, err.turn_revision) == (current_turn_epoch, current_turn_rev)
                    && err.is_terminal
                {
                    terminal_error = Some(err.clone());
                    break;
                }
            }
            if let Some(succ) = &src.successful_completion {
                if (succ.turn_epoch, succ.turn_revision) == (current_turn_epoch, current_turn_rev) {
                    successful_completion = Some(succ.clone());
                    // Another qualified source may still report a terminal
                    // failure for this turn. Error precedence is independent
                    // of source-name sort order.
                }
            }
        }
    }

    // Step 4: Collect pending requests, background tasks and working activity (filtered to current turn)
    let mut pending_requests = Vec::new();
    let mut max_background_tasks = 0usize;
    let mut is_working = false;
    let mut is_retrying = false;
    let mut is_idle = false;

    for (src_id, src) in &accepted_sources {
        let unscoped = family.is_some() && src.conversation_id.is_none();
        if unscoped {
            // Missing family cannot establish quiescence or success, but a
            // verified same-instance background-work hint must delay Done.
            max_background_tasks = max_background_tasks.max(src.background_task_count);
            continue;
        }
        let is_current_turn = (src.turn_epoch, src.turn_revision)
            == (current_turn_epoch, current_turn_rev)
            || (src.turn_epoch == 0
                && src.turn_revision == 0
                && current_turn_epoch == 0
                && current_turn_rev == 0);

        if !is_current_turn {
            continue;
        }

        if src.background_task_count > max_background_tasks {
            max_background_tasks = src.background_task_count;
        }

        match src.activity {
            Some(ActivityKind::Working) | Some(ActivityKind::Thinking) => is_working = true,
            Some(ActivityKind::Retrying) => {
                is_working = true;
                is_retrying = true;
            }
            Some(ActivityKind::Idle) => is_idle = true,
            _ => {}
        }

        if can_declare_pending_requests(src_id, kind, is_synthetic) {
            for req in &src.pending_requests {
                if !pending_requests
                    .iter()
                    .any(|p: &PendingRequest| p.id == req.id)
                {
                    pending_requests.push(req.clone());
                }
            }
        }
    }

    // Qualified lifecycle terminal results resolve earlier same-turn work/input (abandoned requests at terminal stop)
    let has_terminal_result = terminal_error.is_some() || successful_completion.is_some();
    if has_terminal_result {
        pending_requests.clear();
    }

    // Step 5: Apply precedence
    // Precedence: NeedsInput > Working > Error > Done > Idle > Unknown
    let (status, detail) = if !pending_requests.is_empty() {
        let det = if pending_requests.len() == 1 {
            match &pending_requests[0].kind {
                RequestKind::Permission => Some("permission required".to_string()),
                RequestKind::Question => Some("question pending".to_string()),
                RequestKind::Confirmation => Some("confirmation pending".to_string()),
                RequestKind::Other(o) => Some(o.clone()),
            }
        } else {
            Some(format!("{} inputs requested", pending_requests.len()))
        };
        (AgentStatus::NeedsInput, det)
    } else if max_background_tasks > 0 {
        (
            AgentStatus::Working,
            Some(format!(
                "{} background task{}",
                max_background_tasks,
                if max_background_tasks == 1 { "" } else { "s" }
            )),
        )
    } else if is_working && !has_terminal_result {
        let det = if is_retrying {
            Some("retrying".to_string())
        } else {
            None
        };
        (AgentStatus::Working, det)
    } else if let Some(err) = terminal_error.as_ref() {
        (AgentStatus::Error, Some(sanitize_string(&err.message, 128)))
    } else if successful_completion.is_some() && max_background_tasks == 0 {
        let det = successful_completion
            .as_ref()
            .and_then(|c| c.summary.as_ref().map(|s| sanitize_string(s, 128)));
        (AgentStatus::Done, det)
    } else if is_idle {
        (AgentStatus::Idle, None)
    } else {
        (AgentStatus::Unknown, None)
    };

    // Step 6: Completion revision allocation
    // Idempotent: repeated success for the exact same turn does NOT increment revision.
    // Zero turn must not invent completion.
    let mut next_completion_rev = prev_completion_rev;
    let mut next_completed_turn = prev_completed_turn.cloned();
    let mut next_completed_epoch = prev_completed_epoch;
    let mut next_completed_rev_num = prev_completed_rev;

    if status == AgentStatus::Done && has_nonempty_turn {
        let is_already_recorded = prev_completed_epoch == current_turn_epoch
            && prev_completed_rev == current_turn_rev
            && (prev_completed_turn == current_turn_id.as_ref());

        if !is_already_recorded {
            next_completion_rev = prev_completion_rev + 1;
            next_completed_turn = current_turn_id.clone();
            next_completed_epoch = current_turn_epoch;
            next_completed_rev_num = current_turn_rev;
        }
    }

    let reduced = ReducedExecutionState {
        status,
        current_turn_id,
        current_turn_epoch,
        current_turn_revision: current_turn_rev,
        conversation_id: conv_id,
        conversation_title: conv_title,
        pending_requests,
        background_task_count: max_background_tasks,
        latest_error: terminal_error,
        latest_completion: successful_completion,
        completion_revision: next_completion_rev,
        detail,
    };

    (
        reduced,
        next_completion_rev,
        next_completed_turn,
        next_completed_epoch,
        next_completed_rev_num,
    )
}

/// Compute display status for a specific host, taking host-local acknowledgement into account.
///
/// An unacknowledged completion displays as Done.
/// Once acknowledged by that host, Done clears to Idle for that host only.
pub fn compute_display_status(
    reduced: &ReducedExecutionState,
    host_ack: Option<&InstanceAck>,
) -> AgentStatus {
    if reduced.status == AgentStatus::Done {
        if let Some(ack) = host_ack {
            if ack.acknowledged_revision >= reduced.completion_revision {
                return AgentStatus::Idle;
            }
        }
        AgentStatus::Done
    } else {
        reduced.status
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn terminal_error_wins_even_when_success_source_sorts_first() {
        let id = AgentInstanceId("terminal-priority".into());
        let success = PartialSourceRecord {
            source_id: "fixture-a-success".into(),
            source_epoch: Some(1),
            source_revision: Some(1),
            turn_epoch: Some(1),
            turn_revision: Some(1),
            turn_id: Some(TurnId("turn".into())),
            successful_completion: Some(SuccessfulCompletion {
                turn_id: Some(TurnId("turn".into())),
                turn_epoch: 1,
                turn_revision: 1,
                completed_at_ms: 1,
                summary: None,
            }),
            ..Default::default()
        }
        .merge_into(&id, None, 1)
        .unwrap();
        let failure = PartialSourceRecord {
            source_id: "fixture-z-failure".into(),
            source_epoch: Some(1),
            source_revision: Some(1),
            turn_epoch: Some(1),
            turn_revision: Some(1),
            turn_id: Some(TurnId("turn".into())),
            execution_error: Some(ExecutionError {
                turn_id: Some(TurnId("turn".into())),
                turn_epoch: 1,
                turn_revision: 1,
                message: "runner failed".into(),
                is_terminal: true,
            }),
            ..Default::default()
        }
        .merge_into(&id, None, 1)
        .unwrap();
        let sources = BTreeMap::from([
            ("fixture-a-success".into(), success),
            ("fixture-z-failure".into(), failure),
        ]);
        let (state, revision, _, _, _) =
            reduce_sources(&sources, AgentKind::Agy, true, 0, None, 0, 0);
        assert_eq!(state.status, AgentStatus::Error);
        assert_eq!(revision, 0);
    }

    #[test]
    fn unscoped_background_work_delays_owned_family_completion_but_cannot_create_it() {
        let id = AgentInstanceId("background-family".into());
        let root = PartialSourceRecord {
            source_id: "opencode".into(),
            source_epoch: Some(1),
            source_revision: Some(1),
            conversation_id: Some(ConversationId("owned".into())),
            turn_epoch: Some(1),
            turn_revision: Some(1),
            turn_id: Some(TurnId("turn".into())),
            successful_completion: Some(SuccessfulCompletion {
                turn_id: Some(TurnId("turn".into())),
                turn_epoch: 1,
                turn_revision: 1,
                completed_at_ms: 1,
                summary: None,
            }),
            ..Default::default()
        }
        .merge_into(&id, None, 1)
        .unwrap();
        let tasks = PartialSourceRecord {
            source_id: "magy-watch".into(),
            source_epoch: Some(1),
            source_revision: Some(1),
            background_task_count: Some(1),
            ..Default::default()
        }
        .merge_into(&id, None, 1)
        .unwrap();
        let mut sources = BTreeMap::from([("opencode".into(), root), ("magy-watch".into(), tasks)]);
        let (state, revision, _, _, _) =
            reduce_sources(&sources, AgentKind::Opencode, false, 0, None, 0, 0);
        assert_eq!(state.status, AgentStatus::Working);
        assert_eq!(revision, 0);
        sources.get_mut("magy-watch").unwrap().background_task_count = 0;
        let (state, revision, _, _, _) =
            reduce_sources(&sources, AgentKind::Opencode, false, 0, None, 0, 0);
        assert_eq!(state.status, AgentStatus::Done);
        assert_eq!(revision, 1);
    }

    #[test]
    fn foreign_conversation_cannot_supersede_owned_family_with_a_late_stop() {
        let id = AgentInstanceId("family-test".into());
        let ui = PartialSourceRecord {
            source_id: "agy-ui".into(),
            source_epoch: Some(1),
            source_revision: Some(1),
            conversation_id: Some(ConversationId("owned".into())),
            turn_epoch: Some(1),
            turn_revision: Some(2),
            turn_id: Some(TurnId("owned-turn".into())),
            activity: Some(ActivityKind::Working),
            ..Default::default()
        }
        .merge_into(&id, None, 1)
        .unwrap();
        let foreign = PartialSourceRecord {
            source_id: "agy-lifecycle".into(),
            source_epoch: Some(1),
            source_revision: Some(99),
            conversation_id: Some(ConversationId("foreign".into())),
            turn_epoch: Some(1),
            turn_revision: Some(99),
            turn_id: Some(TurnId("foreign-turn".into())),
            successful_completion: Some(SuccessfulCompletion {
                turn_id: Some(TurnId("foreign-turn".into())),
                turn_epoch: 1,
                turn_revision: 99,
                completed_at_ms: 99,
                summary: None,
            }),
            ..Default::default()
        }
        .merge_into(&id, None, 99)
        .unwrap();
        let sources = BTreeMap::from([("agy-ui".into(), ui), ("agy-lifecycle".into(), foreign)]);
        let (state, revision, _, _, _) =
            reduce_sources(&sources, AgentKind::Agy, false, 0, None, 0, 0);
        assert_eq!(state.status, AgentStatus::Working);
        assert_eq!(state.conversation_id, Some(ConversationId("owned".into())));
        assert_eq!(revision, 0);
    }

    fn dummy_instance_id() -> AgentInstanceId {
        AgentInstanceId("agent-test-1".to_string())
    }

    #[test]
    fn test_precedence_ordering() {
        assert!(AgentStatus::NeedsInput.precedence() > AgentStatus::Working.precedence());
        assert!(AgentStatus::Working.precedence() > AgentStatus::Error.precedence());
        assert!(AgentStatus::Error.precedence() > AgentStatus::Done.precedence());
        assert!(AgentStatus::Done.precedence() > AgentStatus::Idle.precedence());
        assert!(AgentStatus::Idle.precedence() > AgentStatus::Unknown.precedence());
    }

    #[test]
    fn test_initial_idle_no_fabricated_done() {
        let mut sources = BTreeMap::new();
        let src = NormalizedSourceRecord {
            schema_version: 1,
            agent_instance_id: dummy_instance_id(),
            source_id: "agy-ui".to_string(),
            source_epoch: 1,
            source_revision: 1,
            observed_at_ms: 1000,
            conversation_id: None,
            conversation_title: None,
            turn_id: None,
            turn_epoch: 1,
            turn_revision: 1,
            activity: Some(ActivityKind::Idle),
            pending_requests: Vec::new(),
            background_task_count: 0,
            successful_completion: None,
            execution_error: None,
            capabilities: vec!["activity".to_string()],
        };
        sources.insert("agy-ui".to_string(), src);

        let (reduced, comp_rev, _, _, _) =
            reduce_sources(&sources, AgentKind::Agy, false, 0, None, 0, 0);
        assert_eq!(reduced.status, AgentStatus::Idle);
        assert_eq!(comp_rev, 0);
    }

    #[test]
    fn test_needs_input_over_working() {
        let mut sources = BTreeMap::new();
        let src = NormalizedSourceRecord {
            schema_version: 1,
            agent_instance_id: dummy_instance_id(),
            source_id: "agy-ui".to_string(),
            source_epoch: 1,
            source_revision: 2,
            observed_at_ms: 2000,
            conversation_id: None,
            conversation_title: None,
            turn_id: Some(TurnId("turn-1".to_string())),
            turn_epoch: 1,
            turn_revision: 1,
            activity: Some(ActivityKind::Working),
            pending_requests: vec![PendingRequest {
                id: "req-1".to_string(),
                kind: RequestKind::Permission,
                detail: Some("run shell".to_string()),
            }],
            background_task_count: 0,
            successful_completion: None,
            execution_error: None,
            capabilities: vec!["permission".to_string()],
        };
        sources.insert("agy-ui".to_string(), src);

        let (reduced, _, _, _, _) = reduce_sources(&sources, AgentKind::Agy, false, 0, None, 0, 0);
        assert_eq!(reduced.status, AgentStatus::NeedsInput);
    }

    #[test]
    fn test_background_task_delays_done() {
        let mut sources = BTreeMap::new();
        let turn = TurnId("turn-1".to_string());

        // UI source shows background task
        let ui_src = NormalizedSourceRecord {
            schema_version: 1,
            agent_instance_id: dummy_instance_id(),
            source_id: "agy-ui".to_string(),
            source_epoch: 1,
            source_revision: 2,
            observed_at_ms: 2000,
            conversation_id: None,
            conversation_title: None,
            turn_id: Some(turn.clone()),
            turn_epoch: 1,
            turn_revision: 1,
            activity: Some(ActivityKind::Idle),
            pending_requests: Vec::new(),
            background_task_count: 1,
            successful_completion: None,
            execution_error: None,
            capabilities: vec![],
        };
        // Lifecycle source shows completion
        let lifecycle_src = NormalizedSourceRecord {
            schema_version: 1,
            agent_instance_id: dummy_instance_id(),
            source_id: "agy-lifecycle".to_string(),
            source_epoch: 1,
            source_revision: 1,
            observed_at_ms: 2000,
            conversation_id: None,
            conversation_title: None,
            turn_id: Some(turn.clone()),
            turn_epoch: 1,
            turn_revision: 1,
            activity: None,
            pending_requests: Vec::new(),
            background_task_count: 0,
            successful_completion: Some(SuccessfulCompletion {
                turn_id: Some(turn.clone()),
                turn_epoch: 1,
                turn_revision: 1,
                completed_at_ms: 2000,
                summary: Some("done".to_string()),
            }),
            execution_error: None,
            capabilities: vec![],
        };
        sources.insert("agy-ui".to_string(), ui_src);
        sources.insert("agy-lifecycle".to_string(), lifecycle_src);

        // Still Working because background_task_count == 1
        let (reduced, comp_rev, _, _, _) =
            reduce_sources(&sources, AgentKind::Agy, false, 0, None, 0, 0);
        assert_eq!(reduced.status, AgentStatus::Working);
        assert_eq!(comp_rev, 0);

        // When background task drops to 0
        if let Some(src) = sources.get_mut("agy-ui") {
            src.background_task_count = 0;
            src.source_revision = 3;
        }

        let (reduced_after, comp_rev_after, completed_turn, _, _) =
            reduce_sources(&sources, AgentKind::Agy, false, 0, None, 0, 0);
        assert_eq!(reduced_after.status, AgentStatus::Done);
        assert_eq!(comp_rev_after, 1);
        assert_eq!(completed_turn, Some(turn.clone()));

        // Repeated reduction is idempotent
        let (reduced_idempotent, comp_rev_idem, _, _, _) = reduce_sources(
            &sources,
            AgentKind::Agy,
            false,
            comp_rev_after,
            completed_turn.as_ref(),
            1,
            1,
        );
        assert_eq!(reduced_idempotent.status, AgentStatus::Done);
        assert_eq!(comp_rev_idem, 1);
    }

    #[test]
    fn test_stale_stop_cannot_clear_newer_turn() {
        let mut sources = BTreeMap::new();
        let turn_2 = TurnId("turn-2".to_string());

        // UI source is active on turn 2
        let ui_src = NormalizedSourceRecord {
            schema_version: 1,
            agent_instance_id: dummy_instance_id(),
            source_id: "agy-ui".to_string(),
            source_epoch: 1,
            source_revision: 5,
            observed_at_ms: 5000,
            conversation_id: None,
            conversation_title: None,
            turn_id: Some(turn_2.clone()),
            turn_epoch: 1,
            turn_revision: 2,
            activity: Some(ActivityKind::Working),
            pending_requests: Vec::new(),
            background_task_count: 0,
            successful_completion: None,
            execution_error: None,
            capabilities: vec![],
        };

        // Delayed lifecycle stop arrives for older turn 1
        let delayed_stop = NormalizedSourceRecord {
            schema_version: 1,
            agent_instance_id: dummy_instance_id(),
            source_id: "agy-lifecycle".to_string(),
            source_epoch: 1,
            source_revision: 3,
            observed_at_ms: 4500,
            conversation_id: None,
            conversation_title: None,
            turn_id: Some(TurnId("turn-1".to_string())),
            turn_epoch: 1,
            turn_revision: 1,
            activity: None,
            pending_requests: Vec::new(),
            background_task_count: 0,
            successful_completion: Some(SuccessfulCompletion {
                turn_id: Some(TurnId("turn-1".to_string())),
                turn_epoch: 1,
                turn_revision: 1,
                completed_at_ms: 4500,
                summary: Some("old turn finished late".to_string()),
            }),
            execution_error: None,
            capabilities: vec![],
        };

        sources.insert("agy-ui".to_string(), ui_src);
        sources.insert("agy-lifecycle".to_string(), delayed_stop);

        let (reduced, comp_rev, _, _, _) =
            reduce_sources(&sources, AgentKind::Agy, false, 0, None, 0, 0);
        // Working for turn 2 wins! Old stop for turn 1 is ignored.
        assert_eq!(reduced.status, AgentStatus::Working);
        assert_eq!(reduced.current_turn_id, Some(turn_2));
        assert_eq!(comp_rev, 0);
    }

    #[test]
    fn test_partial_merge_preserves_missing_fields_and_monotonicity() {
        let inst = dummy_instance_id();
        let existing = NormalizedSourceRecord {
            schema_version: 1,
            agent_instance_id: inst.clone(),
            source_id: "agy-ui".to_string(),
            source_epoch: 1,
            source_revision: 10,
            observed_at_ms: 1000,
            conversation_id: Some(ConversationId("conv-1".to_string())),
            conversation_title: Some("Initial Title".to_string()),
            turn_id: Some(TurnId("turn-1".to_string())),
            turn_epoch: 1,
            turn_revision: 1,
            activity: Some(ActivityKind::Working),
            pending_requests: vec![PendingRequest {
                id: "req-1".to_string(),
                kind: RequestKind::Permission,
                detail: None,
            }],
            background_task_count: 0,
            successful_completion: None,
            execution_error: None,
            capabilities: vec!["activity".to_string()],
        };

        // Partial update with title only on same turn preserves pending requests
        let partial = PartialSourceRecord {
            source_id: "agy-ui".to_string(),
            conversation_title: Some("Updated Title".to_string()),
            source_epoch: Some(1),
            source_revision: Some(11),
            ..Default::default()
        };

        let merged = partial.merge_into(&inst, Some(&existing), 2000).unwrap();
        assert_eq!(merged.conversation_title, Some("Updated Title".to_string()));
        assert_eq!(merged.conversation_id, existing.conversation_id);
        assert_eq!(merged.pending_requests, existing.pending_requests);
        assert_eq!(merged.source_revision, 11);

        // Lower or duplicate revision is rejected
        let stale = PartialSourceRecord {
            source_id: "agy-ui".to_string(),
            source_epoch: Some(1),
            source_revision: Some(10),
            ..Default::default()
        };
        assert!(stale.merge_into(&inst, Some(&merged), 3000).is_err());

        // Missing source_revision is strictly rejected
        let missing_rev = PartialSourceRecord {
            source_id: "agy-ui".to_string(),
            source_epoch: Some(1),
            source_revision: None,
            ..Default::default()
        };
        assert!(missing_rev.merge_into(&inst, Some(&merged), 3000).is_err());

        // New turn resets execution facts (old pending requests are cleared)
        let new_turn = PartialSourceRecord {
            source_id: "agy-ui".to_string(),
            source_epoch: Some(1),
            source_revision: Some(12),
            turn_epoch: Some(1),
            turn_revision: Some(2),
            turn_id: Some(TurnId("turn-2".to_string())),
            ..Default::default()
        };
        let merged_turn2 = new_turn.merge_into(&inst, Some(&merged), 4000).unwrap();
        assert_eq!(merged_turn2.turn_revision, 2);
        assert!(merged_turn2.pending_requests.is_empty());
        assert_eq!(merged_turn2.background_task_count, 0);

        // Lower turn from same producer is rejected
        let lower_turn = PartialSourceRecord {
            source_id: "agy-ui".to_string(),
            source_epoch: Some(1),
            source_revision: Some(13),
            turn_epoch: Some(1),
            turn_revision: Some(1),
            turn_id: Some(TurnId("turn-1".to_string())),
            ..Default::default()
        };
        assert!(lower_turn
            .merge_into(&inst, Some(&merged_turn2), 5000)
            .is_err());

        // Source lifetime change (new epoch) resets facts
        let new_epoch = PartialSourceRecord {
            source_id: "agy-ui".to_string(),
            source_epoch: Some(2),
            source_revision: Some(1),
            ..Default::default()
        };
        let merged_epoch2 = new_epoch
            .merge_into(&inst, Some(&merged_turn2), 6000)
            .unwrap();
        assert_eq!(merged_epoch2.source_epoch, 2);
        assert_eq!(merged_epoch2.source_revision, 1);
        assert_eq!(merged_epoch2.turn_epoch, 0);
        assert_eq!(merged_epoch2.turn_revision, 0);
    }

    #[test]
    fn test_edge_case_abandoned_requests_at_terminal_stop() {
        let mut sources = BTreeMap::new();
        let turn = TurnId("turn-1".to_string());

        // UI source had a pending request
        let ui_src = NormalizedSourceRecord {
            schema_version: 1,
            agent_instance_id: dummy_instance_id(),
            source_id: "agy-ui".to_string(),
            source_epoch: 1,
            source_revision: 2,
            observed_at_ms: 2000,
            conversation_id: None,
            conversation_title: None,
            turn_id: Some(turn.clone()),
            turn_epoch: 1,
            turn_revision: 1,
            activity: Some(ActivityKind::Working),
            pending_requests: vec![PendingRequest {
                id: "abandoned-req".to_string(),
                kind: RequestKind::Permission,
                detail: Some("run shell".to_string()),
            }],
            background_task_count: 0,
            successful_completion: None,
            execution_error: None,
            capabilities: vec![],
        };

        // Lifecycle source terminates turn with success
        let lifecycle_src = NormalizedSourceRecord {
            schema_version: 1,
            agent_instance_id: dummy_instance_id(),
            source_id: "agy-lifecycle".to_string(),
            source_epoch: 1,
            source_revision: 1,
            observed_at_ms: 2500,
            conversation_id: None,
            conversation_title: None,
            turn_id: Some(turn.clone()),
            turn_epoch: 1,
            turn_revision: 1,
            activity: Some(ActivityKind::Idle),
            pending_requests: Vec::new(),
            background_task_count: 0,
            successful_completion: Some(SuccessfulCompletion {
                turn_id: Some(turn.clone()),
                turn_epoch: 1,
                turn_revision: 1,
                completed_at_ms: 2500,
                summary: Some("done".to_string()),
            }),
            execution_error: None,
            capabilities: vec![],
        };

        sources.insert("agy-ui".to_string(), ui_src);
        sources.insert("agy-lifecycle".to_string(), lifecycle_src);

        // Terminal stop resolves the abandoned request -> status is Done, not NeedsInput
        let (reduced, comp_rev, _, _, _) =
            reduce_sources(&sources, AgentKind::Agy, false, 0, None, 0, 0);
        assert_eq!(reduced.status, AgentStatus::Done);
        assert!(reduced.pending_requests.is_empty());
        assert_eq!(comp_rev, 1);
    }

    #[test]
    fn test_edge_case_late_ui_refresh_not_overriding_terminal() {
        let mut sources = BTreeMap::new();
        let turn = TurnId("turn-1".to_string());

        // Lifecycle finished turn 1
        let lifecycle_src = NormalizedSourceRecord {
            schema_version: 1,
            agent_instance_id: dummy_instance_id(),
            source_id: "agy-lifecycle".to_string(),
            source_epoch: 1,
            source_revision: 1,
            observed_at_ms: 2000,
            conversation_id: None,
            conversation_title: None,
            turn_id: Some(turn.clone()),
            turn_epoch: 1,
            turn_revision: 1,
            activity: Some(ActivityKind::Idle),
            pending_requests: Vec::new(),
            background_task_count: 0,
            successful_completion: Some(SuccessfulCompletion {
                turn_id: Some(turn.clone()),
                turn_epoch: 1,
                turn_revision: 1,
                completed_at_ms: 2000,
                summary: Some("done".to_string()),
            }),
            execution_error: None,
            capabilities: vec![],
        };

        // Late UI refresh arrives with idle activity
        let late_ui = NormalizedSourceRecord {
            schema_version: 1,
            agent_instance_id: dummy_instance_id(),
            source_id: "agy-ui".to_string(),
            source_epoch: 1,
            source_revision: 10,
            observed_at_ms: 3000,
            conversation_id: None,
            conversation_title: None,
            turn_id: Some(turn.clone()),
            turn_epoch: 1,
            turn_revision: 1,
            activity: Some(ActivityKind::Idle),
            pending_requests: Vec::new(),
            background_task_count: 0,
            successful_completion: None,
            execution_error: None,
            capabilities: vec![],
        };

        sources.insert("agy-lifecycle".to_string(), lifecycle_src);
        sources.insert("agy-ui".to_string(), late_ui);

        let (reduced, comp_rev, _, _, _) =
            reduce_sources(&sources, AgentKind::Agy, false, 0, None, 0, 0);
        // Late UI idle refresh cannot override authoritative Done
        assert_eq!(reduced.status, AgentStatus::Done);
        assert_eq!(comp_rev, 1);
    }

    #[test]
    fn test_edge_case_unrelated_source_family() {
        let mut sources = BTreeMap::new();

        // Opencode agent receiving rogue or unrelated agy sources
        let unrelated = NormalizedSourceRecord {
            schema_version: 1,
            agent_instance_id: dummy_instance_id(),
            source_id: "agy-ui".to_string(),
            source_epoch: 1,
            source_revision: 1,
            observed_at_ms: 1000,
            conversation_id: None,
            conversation_title: None,
            turn_id: Some(TurnId("turn-1".to_string())),
            turn_epoch: 1,
            turn_revision: 1,
            activity: Some(ActivityKind::Working),
            pending_requests: Vec::new(),
            background_task_count: 0,
            successful_completion: None,
            execution_error: None,
            capabilities: vec![],
        };
        sources.insert("agy-ui".to_string(), unrelated);

        let (reduced, _, _, _, _) =
            reduce_sources(&sources, AgentKind::Opencode, false, 0, None, 0, 0);
        // Unrelated source ignored -> conservative Unknown
        assert_eq!(reduced.status, AgentStatus::Unknown);
    }

    #[test]
    fn test_edge_case_background_stop_partial() {
        let mut sources = BTreeMap::new();
        let turn = TurnId("turn-1".to_string());

        // Lifecycle declares completion
        let lifecycle_src = NormalizedSourceRecord {
            schema_version: 1,
            agent_instance_id: dummy_instance_id(),
            source_id: "agy-lifecycle".to_string(),
            source_epoch: 1,
            source_revision: 1,
            observed_at_ms: 2000,
            conversation_id: None,
            conversation_title: None,
            turn_id: Some(turn.clone()),
            turn_epoch: 1,
            turn_revision: 1,
            activity: Some(ActivityKind::Idle),
            pending_requests: Vec::new(),
            background_task_count: 0,
            successful_completion: Some(SuccessfulCompletion {
                turn_id: Some(turn.clone()),
                turn_epoch: 1,
                turn_revision: 1,
                completed_at_ms: 2000,
                summary: Some("done".to_string()),
            }),
            execution_error: None,
            capabilities: vec![],
        };

        // But UI reports 2 background tasks still running
        let ui_src = NormalizedSourceRecord {
            schema_version: 1,
            agent_instance_id: dummy_instance_id(),
            source_id: "agy-ui".to_string(),
            source_epoch: 1,
            source_revision: 2,
            observed_at_ms: 2000,
            conversation_id: None,
            conversation_title: None,
            turn_id: Some(turn.clone()),
            turn_epoch: 1,
            turn_revision: 1,
            activity: Some(ActivityKind::Idle),
            pending_requests: Vec::new(),
            background_task_count: 2,
            successful_completion: None,
            execution_error: None,
            capabilities: vec![],
        };

        sources.insert("agy-lifecycle".to_string(), lifecycle_src);
        sources.insert("agy-ui".to_string(), ui_src);

        let (reduced, comp_rev, _, _, _) =
            reduce_sources(&sources, AgentKind::Agy, false, 0, None, 0, 0);
        // Background tasks keep status as Working
        assert_eq!(reduced.status, AgentStatus::Working);
        assert_eq!(reduced.detail, Some("2 background tasks".to_string()));
        assert_eq!(comp_rev, 0);
    }

    #[test]
    fn test_edge_case_zero_turn_cannot_invent_completion() {
        let mut sources = BTreeMap::new();

        // Lifecycle reporting completion for turn 0
        let zero_src = NormalizedSourceRecord {
            schema_version: 1,
            agent_instance_id: dummy_instance_id(),
            source_id: "agy-lifecycle".to_string(),
            source_epoch: 1,
            source_revision: 1,
            observed_at_ms: 1000,
            conversation_id: None,
            conversation_title: None,
            turn_id: None,
            turn_epoch: 0,
            turn_revision: 0,
            activity: Some(ActivityKind::Idle),
            pending_requests: Vec::new(),
            background_task_count: 0,
            successful_completion: Some(SuccessfulCompletion {
                turn_id: None,
                turn_epoch: 0,
                turn_revision: 0,
                completed_at_ms: 1000,
                summary: Some("bogus zero turn completion".to_string()),
            }),
            execution_error: None,
            capabilities: vec![],
        };
        sources.insert("agy-lifecycle".to_string(), zero_src);

        let (reduced, comp_rev, _, _, _) =
            reduce_sources(&sources, AgentKind::Agy, false, 0, None, 0, 0);
        // Zero turn must not invent completion
        assert_ne!(reduced.status, AgentStatus::Done);
        assert_eq!(comp_rev, 0);
    }

    #[test]
    fn test_conversation_metadata_picks_current_family_and_turn() {
        let mut sources = BTreeMap::new();

        // Older turn source alphabetically first ("a-older")
        let older_src = NormalizedSourceRecord {
            schema_version: 1,
            agent_instance_id: dummy_instance_id(),
            source_id: "a-older".to_string(),
            source_epoch: 1,
            source_revision: 1,
            observed_at_ms: 500,
            conversation_id: Some(ConversationId("old-conv".to_string())),
            conversation_title: Some("Old Conversation Title".to_string()),
            turn_id: Some(TurnId("turn-1".to_string())),
            turn_epoch: 1,
            turn_revision: 1,
            activity: None,
            pending_requests: Vec::new(),
            background_task_count: 0,
            successful_completion: None,
            execution_error: None,
            capabilities: vec![],
        };

        // Current turn agy-ui source
        let current_src = NormalizedSourceRecord {
            schema_version: 1,
            agent_instance_id: dummy_instance_id(),
            source_id: "agy-ui".to_string(),
            source_epoch: 1,
            source_revision: 2,
            observed_at_ms: 1000,
            conversation_id: Some(ConversationId("curr-conv".to_string())),
            conversation_title: Some("Current Conversation Title".to_string()),
            turn_id: Some(TurnId("turn-2".to_string())),
            turn_epoch: 1,
            turn_revision: 2,
            activity: Some(ActivityKind::Working),
            pending_requests: Vec::new(),
            background_task_count: 0,
            successful_completion: None,
            execution_error: None,
            capabilities: vec![],
        };

        sources.insert("a-older".to_string(), older_src);
        sources.insert("agy-ui".to_string(), current_src);

        let (reduced, _, _, _, _) = reduce_sources(&sources, AgentKind::Agy, true, 0, None, 0, 0);
        // Picked current turn metadata
        assert_eq!(
            reduced.conversation_id,
            Some(ConversationId("curr-conv".to_string()))
        );
        assert_eq!(
            reduced.conversation_title,
            Some("Current Conversation Title".to_string())
        );
    }

    #[test]
    fn test_host_local_acknowledgement() {
        let reduced = ReducedExecutionState {
            status: AgentStatus::Done,
            current_turn_id: Some(TurnId("turn-1".to_string())),
            current_turn_epoch: 1,
            current_turn_revision: 1,
            conversation_id: None,
            conversation_title: None,
            pending_requests: Vec::new(),
            background_task_count: 0,
            latest_error: None,
            latest_completion: None,
            completion_revision: 4,
            detail: None,
        };

        // Host 1 has not acknowledged revision 4
        let host1_ack = Some(InstanceAck {
            acknowledged_revision: 3,
            acknowledged_at_ms: 1000,
            turn_id: Some(TurnId("turn-0".to_string())),
            request_id: "req-1".to_string(),
        });
        assert_eq!(
            compute_display_status(&reduced, host1_ack.as_ref()),
            AgentStatus::Done
        );

        // Host 2 has acknowledged revision 4
        let host2_ack = Some(InstanceAck {
            acknowledged_revision: 4,
            acknowledged_at_ms: 2000,
            turn_id: Some(TurnId("turn-1".to_string())),
            request_id: "req-2".to_string(),
        });
        assert_eq!(
            compute_display_status(&reduced, host2_ack.as_ref()),
            AgentStatus::Idle
        );
    }
}
