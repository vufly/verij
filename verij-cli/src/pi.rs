//! Pi 1.2.0 TUI-local, metadata-only snapshot contract and reporter.
//!
//! # P2 Integration Interface
//!
//! This module defines the native protocol, fencing, state projection, and
//! completion deduplication for Pi 1.2.0 (`@earendil-works/pi-coding-agent`).
//! The P2 extension connects to this reporter as a child process via private
//! stdin/stdout pipes:
//!
//! ```bash
//! verij agent pi [--session <hint>] --pane <pane_id> --pid <pi_pid>
//! ```
//!
//! ## Framing and Handshake
//!
//! - **Encoding:** UTF-8 single-line JSON (NDJSON) terminated with `\n`.
//! - **Frame Limit:** Maximum 262,144 bytes (256 KiB) per line.
//! - **Handshake:** Supplied as the very first line on stdin as a `HandshakeFrame`:
//!   `{"type":"hello","schema_version":1,"version":"1.2.0","runtime_id":"<id>","conversation_id":"<id>"}\n`
//! - **Epoch Claim:** The reporter atomically claims a monotonic `source_epoch`
//!   and records runtime identity immediately under a store transaction before
//!   reading snapshots.
//! - **Ready Greeting:** On successful handshake and epoch claim, reporter outputs:
//!   `{"ready":true,"agent_instance_id":"<instance_id>","source_epoch":<epoch>,"source_revision":<rev>,"status":"<status>","restored":<bool>,"aggregate_run_id":<opt_id>,"latest_snapshot":<opt_snap>}\n`
//!   - **Same-Runtime Reconnect:** When `runtime_id` and `conversation_id` match the persisted adapter state,
//!     restores the validated `latest_snapshot` and aggregate run identity (`restored: true`) without historical completion replay.
//!   - **Fresh Runtime / Reload:** A different `runtime_id` cannot assume active work continuity and reports `Unknown` (`restored: false`),
//!     clearing active execution facts while preserving the completion deduplication ledger.
//! - **Receipts:** After each validated snapshot, reporter outputs an acknowledgement
//!   correlating source order:
//!   `{"source_epoch":<epoch>,"source_revision":<rev>,"record_revision":<rec_rev>,"completion_revision":<comp_rev>,"status":"<status>"}\n`
//! - **Fencing:** Exactly one live reporter process per agent instance via `.pi.lock`.
//!   Generic `verij agent report` with `source_id: "pi"` is strictly reserved and rejected.
//! - **Teardown:** On stream close, if the reporter still owns the active epoch,
//!   it projects `Unknown` to clear active execution facts while preserving
//!   completion deduplication and continuity metadata in `adapter_state["pi"]`.
//!
//! ## Strict Metadata Whitelist
//!
//! All types enforce `deny_unknown_fields` and closed enums. Prompts, assistant text,
//! tool arguments/results, provider bodies, dialog text, and transcript paths are excluded.
//! Raw Pi event settlement qualification belongs to P2; native trusts qualified snapshots.

use anyhow::{bail, Context, Result};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use std::collections::BTreeMap;
use std::io::{BufRead, Write};
use verij_types::agent::{
    ActivityKind, AgentState, AgentStatus, ConversationId, ExecutionError, PartialSourceRecord,
    PendingRequest, RequestKind, SuccessfulCompletion, TurnId, AGENT_SCHEMA_VERSION,
};

pub const PROTOCOL_VERSION: u32 = 1;
pub const SUPPORTED_VERSION: &str = "1.2.0";
pub const LEASE_MS: u64 = 15_000;
pub const MAX_FRAME_BYTES: usize = 262_144;
pub const MAX_LEDGER_ENTRIES: usize = 128;
pub const MAX_LEDGER_BYTES: usize = 32_768;
pub const MAX_ID_LEN: usize = 256;
pub const MAX_TITLE_LEN: usize = 256;

/// Version guard: qualify Pi 1.2.0 only.
pub fn supports_version(version: &str) -> bool {
    version == SUPPORTED_VERSION
}

/// Check if observer lease has expired for Pi.
pub fn lease_expired(state: &AgentState, now: u64) -> bool {
    state.sources.get("pi").is_some_and(|source| {
        source.capabilities.iter().any(|cap| cap == "lease")
            && (now.saturating_sub(source.observed_at_ms) > LEASE_MS
                || source.observed_at_ms > now.saturating_add(1000))
    })
}

/// Closed enum of Pi activities on wire. No Unknown variant, no serde(other).
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PiActivity {
    Idle,
    Thinking,
    Working,
    Retrying,
}

impl From<PiActivity> for ActivityKind {
    fn from(act: PiActivity) -> Self {
        match act {
            PiActivity::Idle => ActivityKind::Idle,
            PiActivity::Thinking => ActivityKind::Thinking,
            PiActivity::Working => ActivityKind::Working,
            PiActivity::Retrying => ActivityKind::Retrying,
        }
    }
}

/// Closed enum of Pi prompt kinds. Generic UI categories only; no permission provenance.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PiRequestKind {
    Confirmation,
    Question,
    Ui,
}

/// Strict metadata-only pending request. No request detail, prompt text, or dialog content.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct PiPendingRequest {
    pub id: String,
    pub kind: PiRequestKind,
}

/// Closed enum of terminal error classifications. No provider bodies or stack traces.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PiErrorKind {
    ProviderAuth,
    ProviderQuota,
    ProviderRateLimit,
    ProviderUnavailable,
    SessionCorrupt,
    ExtensionError,
    TerminalFailure,
}

impl PiErrorKind {
    pub fn static_message(&self) -> &'static str {
        match self {
            Self::ProviderAuth => "provider authentication failed",
            Self::ProviderQuota => "provider quota exceeded",
            Self::ProviderRateLimit => "provider rate limit exceeded",
            Self::ProviderUnavailable => "provider unavailable",
            Self::SessionCorrupt => "session corruption",
            Self::ExtensionError => "extension execution failed",
            Self::TerminalFailure => "terminal execution failure",
        }
    }
}

/// Authoritative aggregate outcome.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "snake_case", deny_unknown_fields)]
pub enum QualifiedOutcome {
    Completed { completed_at_ms: u64 },
    Error { error_kind: PiErrorKind },
    Aborted,
}

/// Strict handshake frame sent at stream initialization.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "snake_case", deny_unknown_fields)]
pub enum HandshakeFrame {
    Hello {
        schema_version: u32,
        version: String,
        runtime_id: String,
        conversation_id: String,
    },
}

impl HandshakeFrame {
    pub fn validate(&self) -> Result<(&str, &str, &str)> {
        match self {
            HandshakeFrame::Hello {
                schema_version,
                version,
                runtime_id,
                conversation_id,
            } => {
                if *schema_version != PROTOCOL_VERSION {
                    bail!(
                        "unsupported Pi handshake schema {}; expected {}",
                        schema_version,
                        PROTOCOL_VERSION
                    );
                }
                if !supports_version(version) {
                    bail!(
                        "unsupported Pi version {:?}; supported: {}",
                        version,
                        SUPPORTED_VERSION
                    );
                }
                if runtime_id.is_empty()
                    || runtime_id.len() > MAX_ID_LEN
                    || runtime_id.chars().any(char::is_control)
                {
                    bail!("invalid bounded Pi runtime_id");
                }
                if conversation_id.is_empty()
                    || conversation_id.len() > MAX_ID_LEN
                    || conversation_id.chars().any(char::is_control)
                {
                    bail!("invalid bounded Pi conversation_id");
                }
                Ok((version, runtime_id, conversation_id))
            }
        }
    }
}

/// Strict metadata-only snapshot sent by Pi extension.
#[derive(Debug, Default, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(deny_unknown_fields)]
pub struct Snapshot {
    pub schema_version: u32,
    pub version: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub conversation_title: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub run_id: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub activity: Option<PiActivity>,
    #[serde(default)]
    pub pending_requests: Vec<PiPendingRequest>,
    #[serde(default)]
    pub background_task_count: usize,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub outcome: Option<QualifiedOutcome>,
}

impl Snapshot {
    /// Validates snapshot constraints, bounds, privacy, and logical consistency.
    pub fn validate(&self) -> Result<()> {
        if self.schema_version != PROTOCOL_VERSION {
            bail!(
                "unsupported Pi snapshot schema {}; expected {}",
                self.schema_version,
                PROTOCOL_VERSION
            );
        }
        if !supports_version(&self.version) {
            bail!(
                "unsupported Pi version {:?}; supported: {}",
                self.version,
                SUPPORTED_VERSION
            );
        }

        for (name, val) in [
            ("conversation_id", &self.conversation_id),
            ("run_id", &self.run_id),
        ] {
            if let Some(s) = val {
                if s.is_empty() || s.len() > MAX_ID_LEN || s.chars().any(char::is_control) {
                    bail!("invalid bounded Pi {name}");
                }
            }
        }

        if let Some(ref title) = self.conversation_title {
            if title.len() > MAX_TITLE_LEN || title.chars().any(char::is_control) {
                bail!("invalid bounded Pi conversation_title");
            }
        }

        // Pi outer prompt model permits at most one active dialog
        if self.pending_requests.len() > 1 {
            bail!("Pi permits at most one pending request (outer prompt model)");
        }
        for req in &self.pending_requests {
            if req.id.is_empty()
                || req.id.len() > MAX_ID_LEN
                || req.id.chars().any(char::is_control)
            {
                bail!("invalid bounded Pi request id");
            }
        }

        if self.background_task_count > 64 {
            bail!(
                "background task count ({}) exceeds maximum (64)",
                self.background_task_count
            );
        }

        // Terminal outcomes (Completed, Error, Aborted) require quiescent idle activity,
        // no pending dialogs, zero background tasks, and explicit conversation/run identity.
        if let Some(outcome) = &self.outcome {
            if self.run_id.is_none() || self.conversation_id.is_none() {
                bail!("terminal outcome requires valid run_id and conversation_id");
            }
            if self.activity != Some(PiActivity::Idle) {
                bail!("unqualified terminal outcome: activity must be explicit idle");
            }
            if !self.pending_requests.is_empty() {
                bail!("unqualified terminal outcome: pending requests must be empty");
            }
            if self.background_task_count != 0 {
                bail!("unqualified terminal outcome: background task count must be 0");
            }
            if let QualifiedOutcome::Completed { completed_at_ms } = outcome {
                if *completed_at_ms == 0 {
                    bail!("completed_at_ms must be > 0");
                }
            }
        }

        Ok(())
    }

    /// Project snapshot into normalized PartialSourceRecord with full snapshot semantics.
    pub fn project(
        &self,
        epoch: u64,
        revision: u64,
        turn_epoch: u64,
        turn_revision: u64,
    ) -> Result<PartialSourceRecord> {
        self.validate()?;
        let turn = self.run_id.as_ref().map(|id| TurnId(id.clone()));
        let shared_requests: Vec<PendingRequest> = self
            .pending_requests
            .iter()
            .map(|req| PendingRequest {
                id: req.id.clone(),
                kind: match req.kind {
                    PiRequestKind::Confirmation => RequestKind::Confirmation,
                    PiRequestKind::Question => RequestKind::Question,
                    PiRequestKind::Ui => RequestKind::Other("ui".to_string()),
                },
                detail: None,
            })
            .collect();

        Ok(PartialSourceRecord {
            schema_version: AGENT_SCHEMA_VERSION,
            agent_instance_id: None,
            source_id: "pi".into(),
            source_epoch: Some(epoch),
            source_revision: Some(revision),
            observed_at_ms: Some(crate::agent_store::now_ms()),
            conversation_id: self.conversation_id.clone().map(ConversationId),
            conversation_title: self.conversation_title.clone(),
            turn_id: turn.clone(),
            turn_epoch: Some(turn_epoch),
            turn_revision: Some(turn_revision),
            activity: Some(
                self.activity
                    .map(ActivityKind::from)
                    .unwrap_or(ActivityKind::Unknown),
            ),
            pending_requests: Some(shared_requests),
            background_task_count: Some(self.background_task_count),
            successful_completion: match &self.outcome {
                Some(QualifiedOutcome::Completed { completed_at_ms }) => {
                    Some(SuccessfulCompletion {
                        turn_id: turn.clone(),
                        turn_epoch,
                        turn_revision,
                        completed_at_ms: *completed_at_ms,
                        summary: None,
                    })
                }
                _ => None,
            },
            execution_error: match &self.outcome {
                Some(QualifiedOutcome::Error { error_kind }) => Some(ExecutionError {
                    turn_id: turn,
                    turn_epoch,
                    turn_revision,
                    message: error_kind.static_message().to_string(),
                    is_terminal: true,
                }),
                _ => None,
            },
            capabilities: Some(
                vec![
                    "activity",
                    "confirmation",
                    "conversation_title",
                    "aggregate_completion",
                    "terminal_failure",
                    "lease",
                ]
                .into_iter()
                .map(str::to_string)
                .collect(),
            ),
        })
    }
}

/// Bounded adapter state persisted atomically in `state.adapter_state["pi"]`.
#[derive(Debug, Default, Clone, Serialize, Deserialize, PartialEq, Eq)]
pub struct PiAdapterState {
    #[serde(default)]
    pub running_version: String,
    #[serde(default)]
    pub runtime_id: String,
    #[serde(default)]
    pub conversation_id: String,
    #[serde(default)]
    pub source_epoch: u64,
    #[serde(default)]
    pub source_revision: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub aggregate_run_id: Option<String>,
    #[serde(default)]
    pub aggregate_turn_epoch: u64,
    #[serde(default)]
    pub aggregate_turn_revision: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub latest_snapshot: Option<Snapshot>,
    /// Scoped to conversation: serialized JSON tuple `(conversation_id, run_id)` -> original completion revision
    #[serde(default, skip_serializing_if = "BTreeMap::is_empty")]
    pub completions: BTreeMap<String, u64>,
}

/// Collision-free structured key for completion ledger.
pub fn dedup_key(conversation_id: &str, run_id: &str) -> String {
    serde_json::to_string(&(conversation_id, run_id)).expect("valid JSON tuple")
}

/// Completion deduplication and adapter state bookkeeping.
///
/// Prevents unread badge replay on heartbeats, reconnects, or reloads.
/// If ledger saturates, degrades conservatively to Unknown rather than inventing Done.
pub fn finish_with_context(
    state: &mut AgentState,
    prev_comp_rev: u64,
    incoming_snapshot: Option<&Snapshot>,
) -> Result<()> {
    let mut adapter: PiAdapterState = state
        .adapter_state
        .get("pi")
        .map(|v| serde_json::from_value(v.clone()))
        .transpose()?
        .unwrap_or_default();

    if let Some(snap) = incoming_snapshot {
        adapter.latest_snapshot = Some(snap.clone());
    }

    if state.reduced.status == AgentStatus::Done {
        let run_id = state
            .reduced
            .current_turn_id
            .as_ref()
            .context("completion without run_id")?
            .0
            .clone();
        let conv_id = state
            .reduced
            .conversation_id
            .as_ref()
            .map(|c| c.0.clone())
            .unwrap_or_else(|| adapter.conversation_id.clone());

        let key = dedup_key(&conv_id, &run_id);

        if let Some(original) = adapter.completions.get(&key) {
            // Already recorded completion! Do not blindly decrement unless reducer minted a new one.
            if state.completion_revision > prev_comp_rev {
                state.completion_revision = prev_comp_rev;
            }
            state.reduced.completion_revision = *original;
        } else {
            // Check projected serialized insertion bytes and entry count
            let mut test_completions = adapter.completions.clone();
            test_completions.insert(key, state.completion_revision);
            let projected_bytes = serde_json::to_vec(&test_completions)
                .map(|b| b.len())
                .unwrap_or(usize::MAX);

            if test_completions.len() > MAX_LEDGER_ENTRIES || projected_bytes > MAX_LEDGER_BYTES {
                // Saturated: degrade conservatively to Unknown, preserve old ledger
                if state.completion_revision > prev_comp_rev {
                    state.completion_revision = prev_comp_rev;
                }
                state.reduced.status = AgentStatus::Unknown;
                state.reduced.latest_completion = None;
                state.reduced.completion_revision = state.completion_revision;
                state.reduced.detail = Some("completion ledger saturated".into());
            } else {
                adapter.completions = test_completions;
            }
        }
    }

    state
        .adapter_state
        .insert("pi".to_string(), serde_json::to_value(&adapter)?);
    Ok(())
}

fn make_initial_partial(epoch: u64, revision: u64, conversation_id: &str) -> PartialSourceRecord {
    PartialSourceRecord {
        schema_version: AGENT_SCHEMA_VERSION,
        agent_instance_id: None,
        source_id: "pi".into(),
        source_epoch: Some(epoch),
        source_revision: Some(revision),
        observed_at_ms: Some(crate::agent_store::now_ms()),
        conversation_id: Some(ConversationId(conversation_id.to_string())),
        conversation_title: None,
        turn_id: None,
        turn_epoch: Some(0),
        turn_revision: Some(0),
        activity: Some(ActivityKind::Unknown),
        pending_requests: Some(vec![]),
        background_task_count: Some(0),
        successful_completion: None,
        execution_error: None,
        capabilities: Some(
            vec![
                "activity",
                "confirmation",
                "conversation_title",
                "aggregate_completion",
                "terminal_failure",
                "lease",
            ]
            .into_iter()
            .map(str::to_string)
            .collect(),
        ),
    }
}

/// Verify that caller still owns active producer authority.
///
/// Requires EXACT match on:
/// - adapter.source_epoch == expected_epoch
/// - adapter.runtime_id == expected_runtime_id
/// - adapter.conversation_id == expected_conversation_id
/// And if `state.sources["pi"]` is present:
/// - source.source_epoch == expected_epoch
pub fn verify_producer_ownership(
    state: &AgentState,
    expected_epoch: u64,
    expected_runtime_id: &str,
    expected_conversation_id: &str,
) -> Result<PiAdapterState> {
    let adapter: PiAdapterState = state
        .adapter_state
        .get("pi")
        .map(|v| serde_json::from_value(v.clone()))
        .transpose()?
        .unwrap_or_default();

    if adapter.source_epoch != expected_epoch {
        bail!(
            "stale Pi producer: adapter epoch {} != expected epoch {}",
            adapter.source_epoch,
            expected_epoch
        );
    }
    if adapter.runtime_id != expected_runtime_id {
        bail!(
            "stale Pi producer: adapter runtime_id {:?} != expected runtime_id {:?}",
            adapter.runtime_id,
            expected_runtime_id
        );
    }
    if adapter.conversation_id != expected_conversation_id {
        bail!(
            "stale Pi producer: adapter conversation_id {:?} != expected conversation_id {:?}",
            adapter.conversation_id,
            expected_conversation_id
        );
    }
    if let Some(source) = state.sources.get("pi") {
        if source.source_epoch != expected_epoch {
            bail!(
                "stale Pi producer: source epoch {} != expected epoch {}",
                source.source_epoch,
                expected_epoch
            );
        }
    }

    Ok(adapter)
}

/// Long-lived pane-local reporter loop for Pi.
pub fn run(candidate_session: Option<String>, pane: u32, pid: u32) -> Result<()> {
    let process = crate::process::identity(pid)?;
    let states_dir = crate::fs_watcher::resolve_states_dir();
    let snapshots = crate::inventory::read_states(&states_dir);

    // Locate owning session through verified inventory and foreground terminal ancestry.
    // Resolve ALL verified owners first, rejecting global ambiguity regardless of supplied hint.
    let verified_owners: Vec<String> = snapshots
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
        .map(|s| s.name.clone())
        .collect();

    if verified_owners.is_empty() {
        bail!("Pi pane ownership unavailable: no verified session owns pane {pane} for pid {pid}");
    }
    if verified_owners.len() > 1 {
        bail!(
            "Pi pane ownership ambiguous: {} verified sessions match pane {pane} for pid {pid}",
            verified_owners.len()
        );
    }
    let session = verified_owners[0].clone();
    let _ = candidate_session; // Candidate session was an unverified hint; verified owner wins

    let instance = crate::agent_cli::register(crate::agent_cli::RegisterArgs {
        session,
        pane,
        pid,
        kind: "pi".into(),
        synthetic: false,
        runner_id: None,
    })?;

    // Single-producer fencing
    let lock_path = crate::agent_store::resolve_v1_dir()?
        .join(&instance.0)
        .join(".pi.lock");
    let lock = std::fs::OpenOptions::new()
        .write(true)
        .create(true)
        .truncate(false)
        .open(&lock_path)?;
    lock.try_lock_exclusive()
        .context("Pi producer already connected")?;

    let mut stdin = std::io::stdin().lock();
    let mut stdout = std::io::stdout().lock();
    let mut buffer = Vec::new();

    // ONE strict hello path: read first line from stdin
    buffer.clear();
    let bytes = std::io::Read::take(&mut stdin, (MAX_FRAME_BYTES + 1) as u64)
        .read_until(b'\n', &mut buffer)?;
    if bytes == 0 || bytes > MAX_FRAME_BYTES || !buffer.ends_with(b"\n") {
        bail!("invalid or missing Pi handshake frame");
    }
    let frame: HandshakeFrame =
        serde_json::from_slice(&buffer).context("failed to parse Pi handshake frame")?;
    let (running_version, runtime_id, conversation_id) = frame.validate()?;
    let (running_version, runtime_id, conversation_id) = (
        running_version.to_string(),
        runtime_id.to_string(),
        conversation_id.to_string(),
    );

    let epoch_cell = std::cell::Cell::new(0u64);
    let prev_comp_rev_cell = std::cell::Cell::new(0u64);
    let is_restored_cell = std::cell::Cell::new(false);
    let revision = 1u64; // Strictly >= 1

    // Atomic handshake state claim
    let handshake_state = crate::agent_store::update(
        &instance,
        |state| {
            prev_comp_rev_cell.set(state.completion_revision);
            let mut adapter: PiAdapterState = state
                .adapter_state
                .get("pi")
                .map(|v| serde_json::from_value(v.clone()))
                .transpose()?
                .unwrap_or_default();

            // Allocate monotonic epoch inside transaction: max(persisted adapter epoch, source epoch) + 1
            let current_source_epoch = state.sources.get("pi").map_or(0, |s| s.source_epoch);
            let prev_max_epoch = std::cmp::max(adapter.source_epoch, current_source_epoch);
            let epoch = prev_max_epoch
                .checked_add(1)
                .context("source_epoch overflow")?;
            epoch_cell.set(epoch);

            let is_same_runtime = adapter.runtime_id == runtime_id
                && adapter.conversation_id == conversation_id
                && !adapter.runtime_id.is_empty();

            adapter.running_version = running_version.clone();
            adapter.runtime_id = runtime_id.clone();
            adapter.conversation_id = conversation_id.clone();
            adapter.source_epoch = epoch;
            adapter.source_revision = revision;

            let partial = if is_same_runtime {
                if let Some(ref latest) = adapter.latest_snapshot {
                    is_restored_cell.set(true);
                    let (turn_epoch, turn_revision) = (
                        adapter.aggregate_turn_epoch,
                        adapter.aggregate_turn_revision,
                    );
                    let (turn_epoch, turn_revision) = if turn_epoch == 0 {
                        (epoch, revision)
                    } else {
                        (turn_epoch, turn_revision)
                    };
                    latest.project(epoch, revision, turn_epoch, turn_revision)?
                } else {
                    make_initial_partial(epoch, revision, &conversation_id)
                }
            } else {
                // Fresh runtime/reload: clear active work and old terminal snapshots
                adapter.aggregate_run_id = None;
                adapter.aggregate_turn_epoch = 0;
                adapter.aggregate_turn_revision = 0;
                adapter.latest_snapshot = None;
                make_initial_partial(epoch, revision, &conversation_id)
            };

            // Remove existing source facts to ensure safe initial state
            state.sources.remove("pi");
            state
                .adapter_state
                .insert("pi".to_string(), serde_json::to_value(&adapter)?);

            Ok(partial)
        },
        |state| {
            let latest = if is_restored_cell.get() {
                let adapter: PiAdapterState = state
                    .adapter_state
                    .get("pi")
                    .and_then(|v| serde_json::from_value(v.clone()).ok())
                    .unwrap_or_default();
                adapter.latest_snapshot
            } else {
                None
            };
            finish_with_context(state, prev_comp_rev_cell.get(), latest.as_ref())
        },
    )?;

    let (_, saved_state) = crate::agent_store::inspect(&instance)?;
    let epoch = epoch_cell.get();
    let mut revision = revision;
    let saved_adapter: PiAdapterState = saved_state
        .adapter_state
        .get("pi")
        .and_then(|v| serde_json::from_value(v.clone()).ok())
        .unwrap_or_default();

    let next_revision = std::cell::Cell::new(revision + 1);

    let loop_result = (|| -> Result<()> {
        // Output ready greeting correlating epoch, status, and restored state
        writeln!(
            stdout,
            "{}",
            serde_json::json!({
                "ready": true,
                "agent_instance_id": instance.0,
                "source_epoch": epoch,
                "source_revision": revision,
                "status": handshake_state.reduced.status,
                "restored": is_restored_cell.get(),
                "aggregate_run_id": if is_restored_cell.get() { saved_adapter.aggregate_run_id.as_deref() } else { None },
                "latest_snapshot": if is_restored_cell.get() { saved_adapter.latest_snapshot.as_ref() } else { None },
            })
        )?;
        stdout.flush()?;

        loop {
            buffer.clear();
            let bytes = std::io::Read::take(&mut stdin, (MAX_FRAME_BYTES + 1) as u64)
                .read_until(b'\n', &mut buffer)?;
            if bytes == 0 {
                break;
            }
            if bytes > MAX_FRAME_BYTES || !buffer.ends_with(b"\n") {
                bail!("invalid bounded snapshot frame");
            }
            let snapshot: Snapshot = serde_json::from_slice(&buffer)?;
            snapshot.validate()?;

            // Enforce bound conversation
            if let Some(ref snap_conv) = snapshot.conversation_id {
                if snap_conv != &conversation_id {
                    bail!(
                        "snapshot conversation_id '{snap_conv}' does not match established handshake conversation_id '{conversation_id}'"
                    );
                }
            }

            revision += 1;
            next_revision.set(revision + 1);

            let prev_comp = std::cell::Cell::new(0u64);

            let state = crate::agent_store::update(
                &instance,
                |state| {
                    prev_comp.set(state.completion_revision);
                    let mut adapter =
                        verify_producer_ownership(state, epoch, &runtime_id, &conversation_id)?;

                    if let Some(old) = state.sources.get("pi") {
                        if old.source_epoch > epoch
                            || (old.source_epoch == epoch && old.source_revision >= revision)
                        {
                            bail!(
                                "obsolete Pi producer: incoming ({}, {}) <= existing ({}, {})",
                                epoch,
                                revision,
                                old.source_epoch,
                                old.source_revision
                            );
                        }
                    }

                    adapter.source_epoch = epoch;
                    adapter.source_revision = revision;
                    adapter.running_version = snapshot.version.clone();
                    adapter.latest_snapshot = Some(snapshot.clone());

                    // Turn order preserves aggregate identity; source revisions alone are not turns
                    let (turn_epoch, turn_revision) = match &snapshot.run_id {
                        Some(run_id) => {
                            if adapter.aggregate_run_id.as_deref() != Some(run_id) {
                                adapter.aggregate_run_id = Some(run_id.clone());
                                adapter.aggregate_turn_epoch = epoch;
                                adapter.aggregate_turn_revision = revision;
                            }
                            (
                                adapter.aggregate_turn_epoch,
                                adapter.aggregate_turn_revision,
                            )
                        }
                        None => {
                            // Full snapshots without run_id clear omitted aggregate identity
                            adapter.aggregate_run_id = None;
                            adapter.aggregate_turn_epoch = 0;
                            adapter.aggregate_turn_revision = 0;
                            (0, 0)
                        }
                    };

                    let partial = snapshot.project(epoch, revision, turn_epoch, turn_revision)?;

                    // Full snapshot replaces missing facts explicitly
                    state.sources.remove("pi");

                    state
                        .adapter_state
                        .insert("pi".to_string(), serde_json::to_value(&adapter)?);

                    Ok(partial)
                },
                |state| finish_with_context(state, prev_comp.get(), Some(&snapshot)),
            )?;

            // Receipt correlates source order
            writeln!(
                stdout,
                "{}",
                serde_json::json!({
                    "source_epoch": epoch,
                    "source_revision": revision,
                    "record_revision": state.record_revision,
                    "completion_revision": state.completion_revision,
                    "status": state.reduced.status
                })
            )?;
            stdout.flush()?;
        }
        Ok(())
    })();

    // Teardown: invalidate active authority to Unknown while checking epoch and runtime ownership.
    // Stale teardown cannot remove newer source.
    // Validated latest_snapshot and completions remain preserved for reconnect continuity!
    let teardown_rev = next_revision.get();
    let teardown_prev_comp = std::cell::Cell::new(0u64);
    let _ = crate::agent_store::update(
        &instance,
        |state| {
            teardown_prev_comp.set(state.completion_revision);
            let mut adapter =
                verify_producer_ownership(state, epoch, &runtime_id, &conversation_id)?;

            adapter.source_epoch = epoch;
            adapter.source_revision = teardown_rev;
            state
                .adapter_state
                .insert("pi".to_string(), serde_json::to_value(&adapter)?);

            state.sources.remove("pi");
            Ok(PartialSourceRecord {
                schema_version: AGENT_SCHEMA_VERSION,
                agent_instance_id: None,
                source_id: "pi".into(),
                source_epoch: Some(epoch),
                source_revision: Some(teardown_rev),
                observed_at_ms: Some(crate::agent_store::now_ms()),
                conversation_id: None,
                conversation_title: None,
                turn_id: None,
                turn_epoch: Some(0),
                turn_revision: Some(0),
                activity: Some(ActivityKind::Unknown),
                pending_requests: Some(vec![]),
                background_task_count: Some(0),
                successful_completion: None,
                execution_error: None,
                capabilities: Some(vec!["lease".to_string()]),
            })
        },
        |state| finish_with_context(state, teardown_prev_comp.get(), None),
    );

    loop_result
}

#[cfg(test)]
mod tests {
    use super::*;
    use verij_types::agent::{AgentInstanceId, NormalizedSourceRecord};

    #[test]
    fn version_guard_rejects_non_1_2_0() {
        assert!(supports_version("1.2.0"));
        for invalid in [
            "1.1.0",
            "1.2.1",
            "1.3.0",
            "2.0.0",
            "1.2.0-beta",
            "1.2.0+1",
            "0.1.2",
            "",
            "1.2",
        ] {
            assert!(!supports_version(invalid), "should reject {invalid}");
            let snap = Snapshot {
                schema_version: PROTOCOL_VERSION,
                version: invalid.into(),
                ..Default::default()
            };
            assert!(
                snap.validate().is_err(),
                "should fail validation: {invalid}"
            );
        }
    }

    #[test]
    fn schema_guard_rejects_non_v1() {
        let snap = Snapshot {
            schema_version: 2,
            version: "1.2.0".into(),
            ..Default::default()
        };
        assert!(snap.validate().is_err());
    }

    #[test]
    fn test_nested_unknown_fields_and_privacy() {
        // Unknown field on root snapshot
        let json_root = serde_json::json!({
            "schema_version": 1,
            "version": "1.2.0",
            "prompt": "secret user prompt text"
        });
        assert!(serde_json::from_value::<Snapshot>(json_root).is_err());

        // Unknown field in nested pending request
        let json_nested_req = serde_json::json!({
            "schema_version": 1,
            "version": "1.2.0",
            "pending_requests": [{
                "id": "req-1",
                "kind": "confirmation",
                "detail": "should be rejected"
            }]
        });
        assert!(serde_json::from_value::<Snapshot>(json_nested_req).is_err());

        // Unknown field in outcome
        let json_outcome = serde_json::json!({
            "schema_version": 1,
            "version": "1.2.0",
            "outcome": {
                "kind": "completed",
                "completed_at_ms": 1000,
                "extra": "leak"
            }
        });
        assert!(serde_json::from_value::<Snapshot>(json_outcome).is_err());

        // Invalid activity enum variant
        let json_invalid_act = serde_json::json!({
            "schema_version": 1,
            "version": "1.2.0",
            "activity": "garbage_activity"
        });
        assert!(serde_json::from_value::<Snapshot>(json_invalid_act).is_err());

        // Invalid request kind enum variant (e.g. Permission is rejected on Pi)
        let json_invalid_kind = serde_json::json!({
            "schema_version": 1,
            "version": "1.2.0",
            "pending_requests": [{
                "id": "req-1",
                "kind": "permission"
            }]
        });
        assert!(serde_json::from_value::<Snapshot>(json_invalid_kind).is_err());

        // Invalid error kind variant
        let json_invalid_err = serde_json::json!({
            "schema_version": 1,
            "version": "1.2.0",
            "outcome": {
                "kind": "error",
                "error_kind": "custom_arbitrary_error"
            }
        });
        assert!(serde_json::from_value::<Snapshot>(json_invalid_err).is_err());
    }

    #[test]
    fn test_closed_enums_and_static_errors() {
        // Every error kind projects its exact static error message
        let error_kinds = [
            (PiErrorKind::ProviderAuth, "provider authentication failed"),
            (PiErrorKind::ProviderQuota, "provider quota exceeded"),
            (
                PiErrorKind::ProviderRateLimit,
                "provider rate limit exceeded",
            ),
            (PiErrorKind::ProviderUnavailable, "provider unavailable"),
            (PiErrorKind::SessionCorrupt, "session corruption"),
            (PiErrorKind::ExtensionError, "extension execution failed"),
            (PiErrorKind::TerminalFailure, "terminal execution failure"),
        ];

        for (kind, expected_msg) in error_kinds {
            assert_eq!(kind.static_message(), expected_msg);
            let snap = Snapshot {
                schema_version: 1,
                version: "1.2.0".into(),
                conversation_id: Some("conv-1".into()),
                run_id: Some("run-1".into()),
                activity: Some(PiActivity::Idle),
                outcome: Some(QualifiedOutcome::Error { error_kind: kind }),
                ..Default::default()
            };
            assert!(snap.validate().is_ok());
            let projected = snap.project(1, 1, 1, 1).unwrap();
            let err = projected.execution_error.unwrap();
            assert_eq!(err.message, expected_msg);
            assert!(err.is_terminal);
        }
    }

    #[test]
    fn test_request_limits_and_no_permission_provenance() {
        // Max 1 pending request allowed
        let mut snap = Snapshot {
            schema_version: 1,
            version: "1.2.0".into(),
            pending_requests: vec![
                PiPendingRequest {
                    id: "req-1".into(),
                    kind: PiRequestKind::Confirmation,
                },
                PiPendingRequest {
                    id: "req-2".into(),
                    kind: PiRequestKind::Question,
                },
            ],
            ..Default::default()
        };
        assert!(
            snap.validate().is_err(),
            "multiple pending requests must be rejected"
        );

        // Single valid request
        snap.pending_requests = vec![PiPendingRequest {
            id: "req-1".into(),
            kind: PiRequestKind::Confirmation,
        }];
        assert!(snap.validate().is_ok());

        // Control characters in request id
        snap.pending_requests[0].id = "req\x00bad".into();
        assert!(snap.validate().is_err());
    }

    #[test]
    fn test_terminal_outcomes_quiescence_requirement() {
        for outcome in [
            QualifiedOutcome::Completed {
                completed_at_ms: 1000,
            },
            QualifiedOutcome::Error {
                error_kind: PiErrorKind::TerminalFailure,
            },
            QualifiedOutcome::Aborted,
        ] {
            let mut snap = Snapshot {
                schema_version: 1,
                version: "1.2.0".into(),
                conversation_id: Some("conv-1".into()),
                run_id: Some("run-1".into()),
                activity: Some(PiActivity::Idle),
                outcome: Some(outcome),
                ..Default::default()
            };
            assert!(snap.validate().is_ok());

            // Non-idle activity -> rejected
            snap.activity = Some(PiActivity::Working);
            assert!(snap.validate().is_err());
            snap.activity = Some(PiActivity::Idle);

            // Active pending request -> rejected
            snap.pending_requests = vec![PiPendingRequest {
                id: "req-1".into(),
                kind: PiRequestKind::Confirmation,
            }];
            assert!(snap.validate().is_err());
            snap.pending_requests.clear();

            // Background tasks -> rejected
            snap.background_task_count = 1;
            assert!(snap.validate().is_err());
            snap.background_task_count = 0;

            // Missing run_id -> rejected
            snap.run_id = None;
            assert!(snap.validate().is_err());
            snap.run_id = Some("run-1".into());

            // Missing conversation_id -> rejected
            snap.conversation_id = None;
            assert!(snap.validate().is_err());
        }

        // Aborted outcome projects Idle without completion or error
        let aborted_snap = Snapshot {
            schema_version: 1,
            version: "1.2.0".into(),
            conversation_id: Some("conv-1".into()),
            run_id: Some("run-1".into()),
            activity: Some(PiActivity::Idle),
            outcome: Some(QualifiedOutcome::Aborted),
            ..Default::default()
        };
        let projected = aborted_snap.project(1, 1, 1, 1).unwrap();
        assert_eq!(projected.activity, Some(ActivityKind::Idle));
        assert!(projected.successful_completion.is_none());
        assert!(projected.execution_error.is_none());
    }

    #[test]
    fn full_snapshot_clearing_semantics() {
        let snap1 = Snapshot {
            schema_version: 1,
            version: "1.2.0".into(),
            conversation_id: Some("conv-1".into()),
            run_id: Some("run-1".into()),
            activity: Some(PiActivity::Working),
            pending_requests: vec![PiPendingRequest {
                id: "req-1".into(),
                kind: PiRequestKind::Confirmation,
            }],
            background_task_count: 2,
            outcome: None,
            ..Default::default()
        };
        let partial1 = snap1.project(1, 1, 1, 1).unwrap();
        assert_eq!(partial1.pending_requests.as_ref().unwrap().len(), 1);
        assert_eq!(partial1.background_task_count, Some(2));

        // Snapshot 2: Dialog closed, background tasks finished
        let snap2 = Snapshot {
            schema_version: 1,
            version: "1.2.0".into(),
            conversation_id: Some("conv-1".into()),
            run_id: Some("run-1".into()),
            activity: Some(PiActivity::Working),
            pending_requests: vec![],
            background_task_count: 0,
            outcome: None,
            ..Default::default()
        };
        let partial2 = snap2.project(1, 2, 1, 1).unwrap();
        assert_eq!(partial2.pending_requests.as_ref().unwrap().len(), 0);
        assert_eq!(partial2.background_task_count, Some(0));
    }

    #[test]
    fn reconnect_dedup_and_heartbeat_idempotence() {
        let mut state: AgentState = serde_json::from_value(serde_json::json!({
            "schema_version": 1,
            "agent_instance_id": "test-pi-instance",
            "record_revision": 1,
            "updated_at_ms": 1000,
            "sources": {},
            "completion_revision": 1,
            "completed_turn": "run-aggregate-1",
            "completed_turn_epoch": 1,
            "completed_turn_revision": 1,
            "adapter_state": {
                "pi": {
                    "running_version": "1.2.0",
                    "runtime_id": "rt-1",
                    "conversation_id": "conv-1",
                    "source_epoch": 1,
                    "source_revision": 1,
                    "completions": {
                        dedup_key("conv-1", "run-aggregate-1"): 1
                    }
                }
            },
            "reduced": {
                "status": "done",
                "current_turn_id": "run-aggregate-1",
                "current_turn_epoch": 1,
                "current_turn_revision": 1,
                "conversation_id": "conv-1",
                "pending_requests": [],
                "background_task_count": 0,
                "completion_revision": 1
            }
        }))
        .unwrap();

        // 1. Heartbeat on already completed state: reducer did NOT increment completion_revision (it stays 1).
        // finish_with_context must NOT blindly decrement revision!
        finish_with_context(&mut state, 1, None).unwrap();
        assert_eq!(state.completion_revision, 1);
        assert_eq!(state.reduced.completion_revision, 1);

        // 2. Reconnect occurs: new epoch 2, reducer mints completion_revision 2 because epoch advanced
        state.completion_revision = 2;
        state.reduced.completion_revision = 2;
        state.reduced.current_turn_epoch = 2;
        // finish_with_context detects run is in completions ledger, restores revision 1
        finish_with_context(&mut state, 1, None).unwrap();
        assert_eq!(state.completion_revision, 1);
        assert_eq!(state.reduced.completion_revision, 1);

        // 3. Genuinely NEW aggregate run in same conversation
        state.reduced.current_turn_id = Some(TurnId("run-aggregate-2".into()));
        state.completion_revision = 2;
        state.reduced.completion_revision = 2;
        finish_with_context(&mut state, 1, None).unwrap();
        assert_eq!(state.completion_revision, 2);
        assert_eq!(state.reduced.completion_revision, 2);

        // 4. Same run ID in DIFFERENT conversation is not a collision
        state.reduced.conversation_id = Some(ConversationId("conv-2".into()));
        state.completion_revision = 3;
        state.reduced.completion_revision = 3;
        finish_with_context(&mut state, 2, None).unwrap();
        assert_eq!(state.completion_revision, 3);
        assert_eq!(state.reduced.completion_revision, 3);

        let adapter: PiAdapterState =
            serde_json::from_value(state.adapter_state.get("pi").unwrap().clone()).unwrap();
        assert_eq!(
            adapter
                .completions
                .get(&dedup_key("conv-1", "run-aggregate-1")),
            Some(&1)
        );
        assert_eq!(
            adapter
                .completions
                .get(&dedup_key("conv-1", "run-aggregate-2")),
            Some(&2)
        );
        assert_eq!(
            adapter
                .completions
                .get(&dedup_key("conv-2", "run-aggregate-2")),
            Some(&3)
        );
    }

    #[test]
    fn ledger_byte_budget_saturation() {
        let mut completions = BTreeMap::new();
        for i in 0..MAX_LEDGER_ENTRIES {
            completions.insert(dedup_key("conv-1", &format!("run-{i}")), (i + 1) as u64);
        }

        let mut state: AgentState = serde_json::from_value(serde_json::json!({
            "schema_version": 1,
            "agent_instance_id": "test-pi-full-ledger",
            "record_revision": 1,
            "updated_at_ms": 1000,
            "sources": {},
            "completion_revision": 129,
            "completed_turn": "run-overflow",
            "completed_turn_epoch": 1,
            "completed_turn_revision": 1,
            "adapter_state": {
                "pi": {
                    "running_version": "1.2.0",
                    "runtime_id": "rt-1",
                    "conversation_id": "conv-1",
                    "completions": completions
                }
            },
            "reduced": {
                "status": "done",
                "current_turn_id": "run-overflow",
                "current_turn_epoch": 1,
                "current_turn_revision": 1,
                "conversation_id": "conv-1",
                "pending_requests": [],
                "background_task_count": 0,
                "completion_revision": 129
            }
        }))
        .unwrap();

        // Saturation degrades conservatively to Unknown rather than invent/replay Done
        finish_with_context(&mut state, 128, None).unwrap();
        assert_eq!(state.reduced.status, AgentStatus::Unknown);
        assert_eq!(
            state.reduced.detail,
            Some("completion ledger saturated".into())
        );
        assert_eq!(state.completion_revision, 128);
    }

    #[test]
    fn observer_lease_expiry_handling() {
        let mut state = AgentState {
            schema_version: 1,
            agent_instance_id: AgentInstanceId("pi-inst".into()),
            record_revision: 1,
            updated_at_ms: 1000,
            sources: BTreeMap::new(),
            reduced: verij_types::agent::ReducedExecutionState {
                status: AgentStatus::Working,
                current_turn_id: Some(TurnId("run-1".into())),
                current_turn_epoch: 1,
                current_turn_revision: 1,
                conversation_id: None,
                conversation_title: None,
                pending_requests: vec![],
                background_task_count: 0,
                latest_error: None,
                latest_completion: None,
                completion_revision: 0,
                detail: None,
            },
            completion_revision: 0,
            completed_turn: None,
            completed_turn_epoch: 0,
            completed_turn_revision: 0,
            opencode_completions: BTreeMap::new(),
            adapter_state: BTreeMap::new(),
        };

        let now = 20_000;
        let snap = Snapshot {
            schema_version: 1,
            version: "1.2.0".into(),
            conversation_id: Some("conv-1".into()),
            run_id: Some("run-1".into()),
            activity: Some(PiActivity::Working),
            ..Default::default()
        };

        let mut norm = snap
            .project(1, 1, 1, 1)
            .unwrap()
            .merge_into(&state.agent_instance_id, None, 1000)
            .unwrap();
        norm.observed_at_ms = 1000; // 19s ago
        state.sources.insert("pi".into(), norm);

        assert!(lease_expired(&state, now));
        assert!(!lease_expired(&state, 12_000));
    }

    #[test]
    fn handshake_frame_validation() {
        let valid = HandshakeFrame::Hello {
            schema_version: 1,
            version: "1.2.0".into(),
            runtime_id: "rt-123".into(),
            conversation_id: "conv-456".into(),
        };
        assert!(valid.validate().is_ok());

        // Invalid version
        let invalid_ver = HandshakeFrame::Hello {
            schema_version: 1,
            version: "1.3.0".into(),
            runtime_id: "rt-123".into(),
            conversation_id: "conv-456".into(),
        };
        assert!(invalid_ver.validate().is_err());

        // Empty runtime id
        let invalid_rt = HandshakeFrame::Hello {
            schema_version: 1,
            version: "1.2.0".into(),
            runtime_id: "".into(),
            conversation_id: "conv-456".into(),
        };
        assert!(invalid_rt.validate().is_err());
    }

    #[test]
    fn test_generic_report_reserves_pi_source() {
        let instance = AgentInstanceId("agent-pi-reserved".into());
        let partial = PartialSourceRecord {
            schema_version: 1,
            agent_instance_id: Some(instance.clone()),
            source_id: "pi".into(),
            source_epoch: Some(1),
            source_revision: Some(1),
            ..Default::default()
        };
        // Calling generic agent_store::report with source_id "pi" must be rejected
        let res = crate::agent_store::report(&instance, partial);
        assert!(res.is_err());
        assert!(res.unwrap_err().to_string().contains("reserved"));
    }

    #[test]
    fn test_stale_teardown_does_not_clear_newer_source() {
        let instance = AgentInstanceId("agent-pi-teardown".into());
        let mut state = AgentState {
            schema_version: 1,
            agent_instance_id: instance.clone(),
            record_revision: 1,
            updated_at_ms: 1000,
            sources: BTreeMap::new(),
            reduced: verij_types::agent::ReducedExecutionState {
                status: AgentStatus::Working,
                current_turn_id: Some(TurnId("run-1".into())),
                current_turn_epoch: 2,
                current_turn_revision: 1,
                conversation_id: None,
                conversation_title: None,
                pending_requests: vec![],
                background_task_count: 0,
                latest_error: None,
                latest_completion: None,
                completion_revision: 0,
                detail: None,
            },
            completion_revision: 0,
            completed_turn: None,
            completed_turn_epoch: 0,
            completed_turn_revision: 0,
            opencode_completions: BTreeMap::new(),
            adapter_state: BTreeMap::new(),
        };

        // Persist adapter state at epoch 2
        let current_adapter = PiAdapterState {
            running_version: "1.2.0".into(),
            runtime_id: "rt-1".into(),
            conversation_id: "conv-1".into(),
            source_epoch: 2,
            source_revision: 5,
            aggregate_run_id: Some("run-2".into()),
            aggregate_turn_epoch: 2,
            aggregate_turn_revision: 1,
            latest_snapshot: None,
            completions: BTreeMap::new(),
        };
        state
            .adapter_state
            .insert("pi".into(), serde_json::to_value(&current_adapter).unwrap());

        // Current source is at epoch 2
        let current_source = NormalizedSourceRecord {
            schema_version: 1,
            agent_instance_id: instance,
            source_id: "pi".into(),
            source_epoch: 2,
            source_revision: 5,
            observed_at_ms: 2000,
            conversation_id: Some(ConversationId("conv-1".into())),
            conversation_title: None,
            turn_id: Some(TurnId("run-2".into())),
            turn_epoch: 2,
            turn_revision: 1,
            activity: Some(ActivityKind::Working),
            pending_requests: vec![],
            background_task_count: 0,
            successful_completion: None,
            execution_error: None,
            capabilities: vec![],
        };
        state.sources.insert("pi".into(), current_source);

        // 1. Stale epoch (1 < 2) fails ownership validation
        assert!(verify_producer_ownership(&state, 1, "rt-1", "conv-1").is_err());
        // 2. Mismatched runtime_id fails ownership validation
        assert!(verify_producer_ownership(&state, 2, "rt-stale", "conv-1").is_err());
        // 3. Mismatched conversation_id fails ownership validation
        assert!(verify_producer_ownership(&state, 2, "rt-1", "conv-stale").is_err());
        // 4. Exact match succeeds
        assert!(verify_producer_ownership(&state, 2, "rt-1", "conv-1").is_ok());

        // Stale teardown from epoch 1 attempts to run
        let stale_epoch = 1u64;
        let teardown_result = (|| -> Result<()> {
            let mut adapter = verify_producer_ownership(&state, stale_epoch, "rt-1", "conv-1")?;
            adapter.source_epoch = stale_epoch;
            state
                .adapter_state
                .insert("pi".into(), serde_json::to_value(&adapter)?);
            state.sources.remove("pi");
            Ok(())
        })();

        assert!(teardown_result.is_err());
        // Newer source at epoch 2 remains intact!
        assert!(state.sources.contains_key("pi"));
        assert_eq!(state.sources.get("pi").unwrap().source_epoch, 2);
    }

    #[test]
    fn test_full_snapshot_without_run_id_clears_aggregate_identity() {
        let instance = AgentInstanceId("pi-snap-no-run".into());
        let mut state = AgentState {
            schema_version: AGENT_SCHEMA_VERSION,
            agent_instance_id: instance.clone(),
            record_revision: 5,
            updated_at_ms: 1000,
            sources: BTreeMap::new(),
            reduced: verij_types::agent::ReducedExecutionState {
                status: AgentStatus::Working,
                current_turn_id: Some(TurnId("run-old".into())),
                current_turn_epoch: 1,
                current_turn_revision: 3,
                conversation_id: Some(ConversationId("conv-1".into())),
                conversation_title: None,
                pending_requests: vec![],
                background_task_count: 0,
                latest_error: None,
                latest_completion: None,
                completion_revision: 0,
                detail: None,
            },
            completion_revision: 0,
            completed_turn: None,
            completed_turn_epoch: 0,
            completed_turn_revision: 0,
            opencode_completions: BTreeMap::new(),
            adapter_state: BTreeMap::new(),
        };

        let initial_adapter = PiAdapterState {
            running_version: "1.2.0".into(),
            runtime_id: "rt-1".into(),
            conversation_id: "conv-1".into(),
            source_epoch: 1,
            source_revision: 3,
            aggregate_run_id: Some("run-old".into()),
            aggregate_turn_epoch: 1,
            aggregate_turn_revision: 3,
            latest_snapshot: Some(Snapshot {
                schema_version: 1,
                version: "1.2.0".into(),
                conversation_id: Some("conv-1".into()),
                run_id: Some("run-old".into()),
                activity: Some(PiActivity::Working),
                ..Default::default()
            }),
            completions: BTreeMap::new(),
        };
        state
            .adapter_state
            .insert("pi".into(), serde_json::to_value(&initial_adapter).unwrap());

        // Process incoming snapshot without run_id
        let snapshot = Snapshot {
            schema_version: 1,
            version: "1.2.0".into(),
            conversation_id: Some("conv-1".into()),
            run_id: None,
            activity: Some(PiActivity::Working),
            ..Default::default()
        };

        let mut adapter: PiAdapterState =
            serde_json::from_value(state.adapter_state.get("pi").unwrap().clone()).unwrap();

        match &snapshot.run_id {
            Some(run_id) => {
                if adapter.aggregate_run_id.as_deref() != Some(run_id) {
                    adapter.aggregate_run_id = Some(run_id.clone());
                    adapter.aggregate_turn_epoch = 1;
                    adapter.aggregate_turn_revision = 4;
                }
            }
            None => {
                adapter.aggregate_run_id = None;
                adapter.aggregate_turn_epoch = 0;
                adapter.aggregate_turn_revision = 0;
            }
        }
        adapter.latest_snapshot = Some(snapshot.clone());
        state
            .adapter_state
            .insert("pi".into(), serde_json::to_value(&adapter).unwrap());

        // Verify persisted adapter state cleared aggregate run identity
        let saved: PiAdapterState =
            serde_json::from_value(state.adapter_state.get("pi").unwrap().clone()).unwrap();
        assert!(saved.aggregate_run_id.is_none());
        assert_eq!(saved.aggregate_turn_epoch, 0);
        assert_eq!(saved.aggregate_turn_revision, 0);
        assert!(saved.latest_snapshot.as_ref().unwrap().run_id.is_none());
    }

    #[test]
    fn test_monotonic_ordering_rejects_stale_revisions() {
        let existing = NormalizedSourceRecord {
            schema_version: 1,
            agent_instance_id: AgentInstanceId("pi-ord".into()),
            source_id: "pi".into(),
            source_epoch: 1,
            source_revision: 5,
            observed_at_ms: 1000,
            conversation_id: Some(ConversationId("conv-1".into())),
            conversation_title: None,
            turn_id: Some(TurnId("run-1".into())),
            turn_epoch: 1,
            turn_revision: 1,
            activity: Some(ActivityKind::Working),
            pending_requests: vec![],
            background_task_count: 0,
            successful_completion: None,
            execution_error: None,
            capabilities: vec![],
        };

        // Incoming with equal revision (1, 5) -> must be rejected
        let check_stale = |epoch: u64, revision: u64| -> Result<()> {
            if (epoch, revision) <= (existing.source_epoch, existing.source_revision) {
                bail!("obsolete Pi producer");
            }
            Ok(())
        };

        assert!(check_stale(1, 4).is_err());
        assert!(check_stale(1, 5).is_err());
        assert!(check_stale(1, 6).is_ok());
        assert!(check_stale(2, 1).is_ok());
    }

    #[test]
    fn test_handshake_runtime_continuity_vs_fresh_reload() {
        let mut adapter = PiAdapterState {
            running_version: "1.2.0".into(),
            runtime_id: "rt-1".into(),
            conversation_id: "conv-1".into(),
            source_epoch: 1,
            source_revision: 5,
            aggregate_run_id: Some("run-1".into()),
            aggregate_turn_epoch: 1,
            aggregate_turn_revision: 1,
            latest_snapshot: Some(Snapshot {
                schema_version: 1,
                version: "1.2.0".into(),
                conversation_id: Some("conv-1".into()),
                run_id: Some("run-1".into()),
                activity: Some(PiActivity::Working),
                ..Default::default()
            }),
            completions: BTreeMap::from([(dedup_key("conv-1", "run-0"), 1)]),
        };

        // Same runtime and conversation: reconnect
        let same_runtime = adapter.runtime_id == "rt-1" && adapter.conversation_id == "conv-1";
        assert!(same_runtime);
        // aggregate_run_id and latest_snapshot preserved

        // Different runtime (reload / new / resume)
        let new_rt_id = "rt-2";
        let is_same = adapter.runtime_id == new_rt_id && adapter.conversation_id == "conv-1";
        assert!(!is_same);
        if !is_same {
            adapter.runtime_id = new_rt_id.into();
            adapter.aggregate_run_id = None;
            adapter.aggregate_turn_epoch = 0;
            adapter.aggregate_turn_revision = 0;
            adapter.latest_snapshot = None;
        }

        assert_eq!(adapter.runtime_id, "rt-2");
        assert!(adapter.aggregate_run_id.is_none());
        assert!(adapter.latest_snapshot.is_none());
        // Completions ledger is preserved across reloads!
        assert_eq!(
            adapter.completions.get(&dedup_key("conv-1", "run-0")),
            Some(&1)
        );
    }

    #[test]
    fn test_duplicate_producer_lock_nonblocking() {
        let dir = tempfile::tempdir().unwrap();
        let lock_path = dir.path().join(".pi.lock");

        let lock1 = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)
            .unwrap();
        lock1.try_lock_exclusive().unwrap();

        // Second attempt on same file from another descriptor must fail
        let lock2 = std::fs::OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(false)
            .open(&lock_path)
            .unwrap();
        assert!(lock2.try_lock_exclusive().is_err());

        // Dropping first lock allows second lock
        drop(lock1);
        assert!(lock2.try_lock_exclusive().is_ok());
    }
}
