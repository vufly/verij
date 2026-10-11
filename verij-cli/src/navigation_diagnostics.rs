//! Navigation diagnostics, structured errors, and verification evaluation.
//!
//! Provides typed classification, serialized diagnostics journaling,
//! and centralized control result validation across all navigation stages.

use std::fmt;
use std::fs::OpenOptions;
use std::io::Write;
use std::path::Path;
use std::sync::{Arc, Mutex};
use std::time::{Instant, SystemTime, UNIX_EPOCH};

use fs2::FileExt;
use serde::{Deserialize, Serialize};

use verij_types::control::*;
use verij_types::identity::{PluginContext, ProcessIdentity, TerminalPaneId};

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

// ---------------------------------------------------------------------------
// Stage & Error Classification
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NavigationStage {
    Precondition,
    Switch,
    Rebind,
    InnerFocus,
    OuterFocus,
    Verification,
}

impl fmt::Display for NavigationStage {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Precondition => write!(f, "Precondition"),
            Self::Switch => write!(f, "Switch"),
            Self::Rebind => write!(f, "Rebind"),
            Self::InnerFocus => write!(f, "Inner focus"),
            Self::OuterFocus => write!(f, "Outer focus"),
            Self::Verification => write!(f, "Verification"),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum NavigationErrorKind {
    PreconditionFailure,
    OwnershipFailure,
    DispatchFailure,
    Timeout,
    ControlFailure,
    MissingObservation,
    ObservationMismatch,
    Superseded,
}

// ---------------------------------------------------------------------------
// Partial Progress
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
pub struct NavigationProgress {
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub switch_dispatched: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub switch_observed: bool,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub rebound: bool,
    pub inner_confirmed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub inner_terminal: Option<TerminalPaneId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub inner_session: Option<String>,
    pub outer_confirmed: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub outer_pane: Option<TerminalPaneId>,
}

// ---------------------------------------------------------------------------
// Diagnostic Record & Evidence Builder
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct NavigationDiagnostic {
    pub timestamp_ms: u64,
    pub intent_id: String,
    pub stage: NavigationStage,
    pub success: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub failure_kind: Option<NavigationErrorKind>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub control_status: Option<ControlStatus>,
    pub progress: NavigationProgress,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub host: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub expected_terminal: Option<TerminalPaneId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub observed_terminal: Option<TerminalPaneId>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub requested_session: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub observed_session: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub requested_context: Option<PluginContext>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub observed_context: Option<PluginContext>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub observation_time_ms: Option<u64>,
    pub elapsed_ms: u64,
    pub request_ids: Vec<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub current_request_id: Option<String>,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub error_message: Option<String>,
    pub validated: bool,
}

#[derive(Debug, Clone, Default)]
pub struct DiagnosticEvidence {
    pub expected_terminal: Option<TerminalPaneId>,
    pub observed_terminal: Option<TerminalPaneId>,
    pub requested_session: Option<String>,
    pub observed_session: Option<String>,
    pub requested_context: Option<PluginContext>,
    pub observed_context: Option<PluginContext>,
    pub observation_time_ms: Option<u64>,
    pub current_request_id: Option<String>,
}

#[allow(dead_code)]
impl DiagnosticEvidence {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn expected_terminal(mut self, t: impl Into<Option<TerminalPaneId>>) -> Self {
        self.expected_terminal = t.into();
        self
    }

    pub fn observed_terminal(mut self, t: impl Into<Option<TerminalPaneId>>) -> Self {
        self.observed_terminal = t.into();
        self
    }

    pub fn requested_session(mut self, s: impl Into<Option<String>>) -> Self {
        self.requested_session = s.into();
        self
    }

    pub fn observed_session(mut self, s: impl Into<Option<String>>) -> Self {
        self.observed_session = s.into();
        self
    }

    pub fn requested_context(mut self, c: impl Into<Option<PluginContext>>) -> Self {
        self.requested_context = c.into();
        self
    }

    pub fn observed_context(mut self, c: impl Into<Option<PluginContext>>) -> Self {
        self.observed_context = c.into();
        self
    }

    pub fn current_request_id(mut self, id: impl Into<Option<String>>) -> Self {
        self.current_request_id = id.into();
        self
    }

    pub fn with_request(mut self, req: &ControlRequest) -> Self {
        self.requested_context = Some(req.context.clone());
        self.current_request_id = Some(req.request_id.clone());
        self
    }

    pub fn with_observation(mut self, obs: Option<&FocusObservation>) -> Self {
        if let Some(o) = obs {
            self.observed_terminal = o.terminal;
            self.observed_session = o.session_name.clone();
            self.observed_context = Some(o.context.clone());
            self.observation_time_ms = Some(o.observed_at_ms);
        }
        self
    }

    pub fn with_result(self, res: &ControlResult) -> Self {
        self.with_request(&res.request)
            .with_observation(res.observation.as_ref())
    }
}

// ---------------------------------------------------------------------------
// Typed Error with Cause Preserved
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct NavigationError {
    pub diagnostic: NavigationDiagnostic,
    pub cause: Option<Arc<anyhow::Error>>,
}

impl NavigationError {
    pub fn display_reason(&self) -> String {
        if self.diagnostic.failure_kind == Some(NavigationErrorKind::Superseded) {
            return "superseded".to_string();
        }
        if self.diagnostic.validated {
            if let Some(ref status) = self.diagnostic.control_status {
                return match status {
                    ControlStatus::PaneMissing => "pane missing",
                    ControlStatus::StaleContext => "stale context",
                    ControlStatus::Superseded => "superseded",
                    ControlStatus::Unavailable => "unavailable",
                    ControlStatus::InnerFocusObserved | ControlStatus::Observed => "observed",
                    ControlStatus::SwitchDispatchedUnverified => "switch dispatched unverified",
                }
                .to_string();
            }
        }
        if let Some(ref kind) = self.diagnostic.failure_kind {
            match kind {
                NavigationErrorKind::Timeout => "timed out".to_string(),
                NavigationErrorKind::DispatchFailure => "dispatch failed".to_string(),
                NavigationErrorKind::MissingObservation => "observation unavailable".to_string(),
                NavigationErrorKind::ObservationMismatch => "observation mismatch".to_string(),
                NavigationErrorKind::OwnershipFailure => "ownership unverified".to_string(),
                NavigationErrorKind::Superseded => "superseded".to_string(),
                NavigationErrorKind::PreconditionFailure => self.concise_precondition_reason(),
                NavigationErrorKind::ControlFailure => {
                    if self.diagnostic.validated {
                        "control failure".to_string()
                    } else {
                        "control rejected".to_string()
                    }
                }
            }
        } else if self.diagnostic.validated {
            "focus unconfirmed".to_string()
        } else {
            "untrusted result".to_string()
        }
    }

    pub fn concise_precondition_reason(&self) -> String {
        if let Some(ref msg) = self.diagnostic.error_message {
            let msg_lower = msg.to_lowercase();
            if msg_lower.contains("must have exactly 1") || msg_lower.contains("display client") {
                "single-display requirement failed".to_string()
            } else if msg_lower.contains("missing verified binding")
                || msg_lower.contains("not registered")
            {
                "missing host binding".to_string()
            } else if msg_lower.contains("attachment")
                && (msg_lower.contains("missing") || msg_lower.contains("no owned"))
            {
                "missing workspace attachment".to_string()
            } else if msg_lower.contains("no longer exists")
                || (msg_lower.contains("terminal pane") && msg_lower.contains("missing"))
            {
                "target missing".to_string()
            } else if msg_lower.contains("exited") {
                "target pane exited".to_string()
            } else if msg_lower.contains("ownership") || msg_lower.contains("alive") {
                "ownership unverified".to_string()
            } else if msg_lower.contains("superseded") {
                "superseded".to_string()
            } else {
                "precondition failed".to_string()
            }
        } else {
            "precondition failed".to_string()
        }
    }
}

impl std::error::Error for NavigationError {
    fn source(&self) -> Option<&(dyn std::error::Error + 'static)> {
        self.cause
            .as_ref()
            .map(|c| c.as_ref().as_ref() as &(dyn std::error::Error + 'static))
    }
}

impl fmt::Display for NavigationError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let reason = self.display_reason();

        if self.diagnostic.progress.inner_confirmed && self.diagnostic.progress.outer_confirmed {
            write!(
                f,
                "Focus observed; final ownership/visit verification failed ({reason}). select Workspace or retry."
            )
        } else if self.diagnostic.progress.inner_confirmed {
            write!(
                f,
                "Inner terminal selected; Workspace focus not confirmed ({reason})."
            )?;
            if let Some(expected) = self.diagnostic.expected_terminal {
                write!(f, " Expected outer pane {}", expected.0)?;
                if let Some(observed) = self.diagnostic.observed_terminal {
                    write!(f, ", observed pane {}", observed.0)?;
                }
                write!(f, ";")?;
            }
            write!(f, " select Workspace or retry.")
        } else {
            match self.diagnostic.stage {
                NavigationStage::Precondition => {
                    let pre_reason = self.concise_precondition_reason();
                    write!(
                        f,
                        "Navigation precondition failed: {pre_reason}; verify target and retry."
                    )
                }
                NavigationStage::Switch => {
                    if let Some(ref session) = self.diagnostic.requested_session {
                        write!(
                            f,
                            "Switch to session '{session}' not confirmed ({reason}); retry."
                        )
                    } else {
                        write!(f, "Session switch not confirmed ({reason}); retry.")
                    }
                }
                NavigationStage::Rebind => {
                    if let Some(ref session) = self.diagnostic.requested_session {
                        write!(
                            f,
                            "Rebind to session '{session}' failed ({reason}); retry registration."
                        )
                    } else {
                        write!(f, "Rebind failed ({reason}); retry registration.")
                    }
                }
                NavigationStage::InnerFocus => {
                    write!(f, "Inner focus not confirmed ({reason}).")?;
                    if let Some(expected) = self.diagnostic.expected_terminal {
                        write!(f, " Expected terminal {}", expected.0)?;
                        if let Some(observed) = self.diagnostic.observed_terminal {
                            write!(f, ", observed {}", observed.0)?;
                        }
                        write!(f, ";")?;
                    }
                    write!(f, " retry.")
                }
                NavigationStage::OuterFocus => {
                    write!(f, "Workspace focus not confirmed ({reason}).")?;
                    if let Some(expected) = self.diagnostic.expected_terminal {
                        write!(f, " Expected outer pane {}", expected.0)?;
                        if let Some(observed) = self.diagnostic.observed_terminal {
                            write!(f, ", observed pane {}", observed.0)?;
                        }
                        write!(f, ";")?;
                    }
                    write!(f, " select Workspace or retry.")
                }
                NavigationStage::Verification => {
                    write!(
                        f,
                        "Post-focus verification failed ({reason}). Expected target ownership not verified; select Workspace or retry."
                    )
                }
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Journal Logging
// ---------------------------------------------------------------------------

pub fn append_navigation_diagnostic(control_dir: &Path, diagnostic: &NavigationDiagnostic) {
    let journal_path = control_dir.join("navigation-diagnostics.ndjson");
    let lock_path = control_dir.join("navigation-diagnostics.lock");

    let lock_file = match OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .open(&lock_path)
    {
        Ok(f) => f,
        Err(_) => return,
    };

    if lock_file.lock_exclusive().is_err() {
        return;
    }

    if let Ok(mut journal_file) = OpenOptions::new()
        .create(true)
        .append(true)
        .open(&journal_path)
    {
        if let Ok(line) = serde_json::to_string(diagnostic) {
            let _ = writeln!(journal_file, "{}", line);
            let _ = journal_file.sync_all();
        }
    }

    let _ = lock_file.unlock();
}

// ---------------------------------------------------------------------------
// Navigation Intent Tracker
// ---------------------------------------------------------------------------

#[derive(Debug, Clone)]
pub struct NavigationIntent {
    pub id: String,
    pub host: Option<String>,
    started: Instant,
    request_ids: Arc<Mutex<Vec<String>>>,
    progress: Arc<Mutex<NavigationProgress>>,
}

impl NavigationIntent {
    pub fn new(host: Option<&str>) -> Self {
        Self::with_id(&uuid::Uuid::new_v4().to_string(), host)
    }

    pub fn with_id(id: &str, host: Option<&str>) -> Self {
        Self {
            id: id.to_string(),
            host: host.map(str::to_string),
            started: Instant::now(),
            request_ids: Arc::new(Mutex::new(Vec::new())),
            progress: Arc::new(Mutex::new(NavigationProgress::default())),
        }
    }

    pub fn progress(&self) -> NavigationProgress {
        self.progress
            .lock()
            .map(|p| p.clone())
            .unwrap_or_default()
    }

    pub fn update_progress<F: FnOnce(&mut NavigationProgress)>(&self, f: F) {
        if let Ok(mut p) = self.progress.lock() {
            f(&mut p);
        }
    }

    pub fn record_request_id(&self, request_id: &str) {
        if let Ok(mut ids) = self.request_ids.lock() {
            ids.push(request_id.to_string());
        }
    }

    pub fn request_ids(&self) -> Vec<String> {
        self.request_ids
            .lock()
            .map(|ids| ids.clone())
            .unwrap_or_default()
    }

    pub fn elapsed_ms(&self) -> u64 {
        self.started.elapsed().as_millis() as u64
    }

    #[allow(clippy::too_many_arguments)]
    pub fn build_diagnostic(
        &self,
        stage: NavigationStage,
        success: bool,
        failure_kind: Option<NavigationErrorKind>,
        control_status: Option<ControlStatus>,
        progress: &NavigationProgress,
        evidence: &DiagnosticEvidence,
        validated: bool,
        error_message: Option<String>,
    ) -> NavigationDiagnostic {
        NavigationDiagnostic {
            timestamp_ms: now_ms(),
            intent_id: self.id.clone(),
            stage,
            success,
            failure_kind,
            control_status,
            progress: progress.clone(),
            host: self.host.clone(),
            expected_terminal: evidence.expected_terminal,
            observed_terminal: evidence.observed_terminal,
            requested_session: evidence.requested_session.clone(),
            observed_session: evidence.observed_session.clone(),
            requested_context: evidence.requested_context.clone(),
            observed_context: evidence.observed_context.clone(),
            observation_time_ms: evidence.observation_time_ms,
            elapsed_ms: self.elapsed_ms(),
            request_ids: self.request_ids(),
            current_request_id: evidence.current_request_id.clone(),
            error_message,
            validated,
        }
    }

    pub fn record_success(
        &self,
        control_dir: &Path,
        stage: NavigationStage,
        control_status: Option<ControlStatus>,
        progress: &NavigationProgress,
        evidence: DiagnosticEvidence,
    ) -> NavigationDiagnostic {
        let diagnostic = self.build_diagnostic(
            stage,
            true,
            None,
            control_status,
            progress,
            &evidence,
            true,
            None,
        );
        append_navigation_diagnostic(control_dir, &diagnostic);
        diagnostic
    }

    #[allow(clippy::too_many_arguments)]
    pub fn record_failure(
        &self,
        control_dir: &Path,
        stage: NavigationStage,
        failure_kind: NavigationErrorKind,
        control_status: Option<ControlStatus>,
        progress: &NavigationProgress,
        evidence: DiagnosticEvidence,
        validated: bool,
        cause: impl Into<anyhow::Error>,
    ) -> NavigationError {
        let err: anyhow::Error = cause.into();
        let error_message = Some(format!("{err:#}"));
        let diagnostic = self.build_diagnostic(
            stage,
            false,
            Some(failure_kind),
            control_status,
            progress,
            &evidence,
            validated,
            error_message,
        );
        append_navigation_diagnostic(control_dir, &diagnostic);
        NavigationError {
            diagnostic,
            cause: Some(Arc::new(err)),
        }
    }
}

// ---------------------------------------------------------------------------
// Compact Stage Context Helper
// ---------------------------------------------------------------------------

#[derive(Clone)]
pub struct StageContext<'a> {
    pub intent: &'a NavigationIntent,
    pub control_dir: &'a Path,
    pub stage: NavigationStage,
    pub expected_terminal: Option<TerminalPaneId>,
    pub requested_session: Option<String>,
    pub requested_context: Option<PluginContext>,
}

#[allow(dead_code)]
impl<'a> StageContext<'a> {
    pub fn new(intent: &'a NavigationIntent, control_dir: &'a Path, stage: NavigationStage) -> Self {
        Self {
            intent,
            control_dir,
            stage,
            expected_terminal: None,
            requested_session: None,
            requested_context: None,
        }
    }

    pub fn target(mut self, t: impl Into<Option<TerminalPaneId>>) -> Self {
        self.expected_terminal = t.into();
        self
    }

    pub fn session(mut self, s: impl Into<Option<String>>) -> Self {
        self.requested_session = s.into();
        self
    }

    pub fn context(mut self, c: impl Into<Option<PluginContext>>) -> Self {
        self.requested_context = c.into();
        self
    }

    pub fn evidence(&self) -> DiagnosticEvidence {
        DiagnosticEvidence {
            expected_terminal: self.expected_terminal,
            requested_session: self.requested_session.clone(),
            requested_context: self.requested_context.clone(),
            ..Default::default()
        }
    }

    pub fn fail(&self, kind: NavigationErrorKind, err: impl Into<anyhow::Error>) -> NavigationError {
        self.fail_with(kind, err, self.evidence(), false)
    }

    pub fn fail_with(
        &self,
        kind: NavigationErrorKind,
        err: impl Into<anyhow::Error>,
        evidence: DiagnosticEvidence,
        validated: bool,
    ) -> NavigationError {
        self.fail_status(kind, None, evidence, validated, err)
    }

    pub fn fail_status(
        &self,
        kind: NavigationErrorKind,
        status: Option<ControlStatus>,
        evidence: DiagnosticEvidence,
        validated: bool,
        err: impl Into<anyhow::Error>,
    ) -> NavigationError {
        self.intent.record_failure(
            self.control_dir,
            self.stage,
            kind,
            status,
            &self.intent.progress(),
            evidence,
            validated,
            err.into(),
        )
    }

    pub fn record_success(
        &self,
        status: Option<ControlStatus>,
        evidence: DiagnosticEvidence,
    ) -> NavigationDiagnostic {
        self.intent.record_success(
            self.control_dir,
            self.stage,
            status,
            &self.intent.progress(),
            evidence,
        )
    }

    pub fn wrap<T>(&self, kind: NavigationErrorKind, res: Result<T, anyhow::Error>) -> Result<T, anyhow::Error> {
        res.map_err(|err| {
            if err.is::<NavigationError>() {
                err
            } else {
                self.fail(kind, err).into()
            }
        })
    }

    pub fn record_outcome(&self, outcome: RequestOutcome) -> Result<ControlResult, NavigationError> {
        record_outcome_diagnostic(
            self.intent,
            self.control_dir,
            self.stage,
            outcome,
            self.expected_terminal,
            self.requested_session.clone(),
        )
    }
}

// ---------------------------------------------------------------------------
// Typed Request Outcome & Shared Validation Reducer
// ---------------------------------------------------------------------------

#[derive(Debug)]
pub enum RequestOutcome {
    Success(ControlResult),
    ExplicitFailure(ControlResult),
    Rejected {
        request: ControlRequest,
        failure_kind: NavigationErrorKind,
        raw_status: Option<ControlStatus>,
        raw_observation: Option<FocusObservation>,
        error: anyhow::Error,
    },
    TimedOut {
        request: ControlRequest,
    },
    DispatchFailed {
        request: ControlRequest,
        error: anyhow::Error,
    },
}

struct RejectReason {
    kind: NavigationErrorKind,
    cause: anyhow::Error,
}

fn validate_raw_result(
    result: &ControlResult,
    expected_request: &ControlRequest,
    expected_session: &str,
    expected_server: &ProcessIdentity,
    started_at_ms: u64,
    current_time_ms: u64,
) -> Result<bool, RejectReason> {
    if result.request != *expected_request {
        return Err(RejectReason {
            kind: NavigationErrorKind::ObservationMismatch,
            cause: anyhow::anyhow!(
                "Control result correlation mismatch: received request ID {:?} does not match expected {:?}",
                result.request.request_id,
                expected_request.request_id
            ),
        });
    }

    let current_server = crate::process::identity(expected_request.context.server_pid)
        .map_err(|e| RejectReason {
            kind: NavigationErrorKind::OwnershipFailure,
            cause: e.context("Failed to resolve server process identity"),
        })?;
    if current_server != *expected_server || !crate::process::is_alive(&current_server) {
        return Err(RejectReason {
            kind: NavigationErrorKind::OwnershipFailure,
            cause: anyhow::anyhow!(
                "Server process changed or exited during control request execution (expected {:?}, current {:?})",
                expected_server,
                current_server
            ),
        });
    }

    let is_explicit_failure = matches!(
        result.status,
        ControlStatus::PaneMissing
            | ControlStatus::StaleContext
            | ControlStatus::Superseded
            | ControlStatus::Unavailable
    );

    if !is_explicit_failure {
        match &expected_request.operation {
            ControlOperation::Focus { .. } => {
                if result.status != ControlStatus::InnerFocusObserved {
                    return Err(RejectReason {
                        kind: NavigationErrorKind::ObservationMismatch,
                        cause: anyhow::anyhow!(
                            "Focus operation requires InnerFocusObserved status, got {:?}",
                            result.status
                        ),
                    });
                }
            }
            ControlOperation::Switch { .. } => {
                if result.status != ControlStatus::SwitchDispatchedUnverified {
                    return Err(RejectReason {
                        kind: NavigationErrorKind::ObservationMismatch,
                        cause: anyhow::anyhow!(
                            "Switch operation expected SwitchDispatchedUnverified status, got {:?}",
                            result.status
                        ),
                    });
                }
            }
            ControlOperation::Query
            | ControlOperation::Locate { .. }
            | ControlOperation::RegistrationSurface
            | ControlOperation::RegistrationSurfaceFor { .. }
            | ControlOperation::RetireRegistrationSurface => {
                if result.status != ControlStatus::Observed {
                    return Err(RejectReason {
                        kind: NavigationErrorKind::ObservationMismatch,
                        cause: anyhow::anyhow!(
                            "Operation {:?} requires Observed status, got {:?}",
                            expected_request.operation,
                            result.status
                        ),
                    });
                }
            }
        }
    }

    let is_switch = matches!(expected_request.operation, ControlOperation::Switch { .. });
    let obs_opt = result.observation.as_ref();
    if !is_explicit_failure && !is_switch && obs_opt.is_none() {
        return Err(RejectReason {
            kind: NavigationErrorKind::MissingObservation,
            cause: anyhow::anyhow!("Missing observation for successful operation; fail closed"),
        });
    }

    if let Some(obs) = obs_opt {
        let prefix = if is_switch { "Switch observation" } else { "Observation" };
        if obs.context != expected_request.context {
            return Err(RejectReason {
                kind: NavigationErrorKind::ObservationMismatch,
                cause: anyhow::anyhow!(
                    "{} context mismatch: expected {:?}, got {:?}",
                    prefix,
                    expected_request.context,
                    obs.context
                ),
            });
        }
        if obs.session_name.as_deref() != Some(expected_session) {
            return Err(RejectReason {
                kind: NavigationErrorKind::ObservationMismatch,
                cause: anyhow::anyhow!(
                    "{} session mismatch: expected {:?}, got {:?}",
                    prefix,
                    expected_session,
                    obs.session_name
                ),
            });
        }
        if obs.observed_at_ms < started_at_ms || obs.observed_at_ms > current_time_ms + 1000 {
            return Err(RejectReason {
                kind: NavigationErrorKind::ObservationMismatch,
                cause: anyhow::anyhow!(
                    "{} freshness invalid: observed {}, window [{}, {}]",
                    prefix,
                    obs.observed_at_ms,
                    started_at_ms,
                    current_time_ms + 1000
                ),
            });
        }

        if !is_explicit_failure {
            match &expected_request.operation {
                ControlOperation::Focus { terminal } => {
                    if obs.terminal != Some(*terminal) {
                        return Err(RejectReason {
                            kind: NavigationErrorKind::ObservationMismatch,
                            cause: anyhow::anyhow!(
                                "Focus observation terminal mismatch: expected {:?}, got {:?}",
                                Some(*terminal),
                                obs.terminal
                            ),
                        });
                    }
                }
                ControlOperation::Locate { terminal } => {
                    if obs.queried_terminal != Some(*terminal) {
                        let kind = if obs.queried_terminal.is_none() {
                            NavigationErrorKind::MissingObservation
                        } else {
                            NavigationErrorKind::ObservationMismatch
                        };
                        return Err(RejectReason {
                            kind,
                            cause: anyhow::anyhow!(
                                "Locate observation queried_terminal mismatch: expected {:?}, got {:?}",
                                Some(*terminal),
                                obs.queried_terminal
                            ),
                        });
                    }
                }
                _ => {}
            }
        }
    }

    Ok(is_explicit_failure)
}

/// Evaluates a received control result against the expected request, session,
/// server process, and timing constraints.
///
/// Central contract enforcement:
/// - Operation and status compatibility:
///   - `Focus`: requires `InnerFocusObserved` (rejects `Observed`).
///   - `Query` / `Locate`: requires `Observed`.
///   - `Switch`: requires `SwitchDispatchedUnverified`.
/// - `SwitchDispatchedUnverified` validates supplied observation context, session, and freshness.
/// - Valid correlated explicit failure is accepted even if observation is absent.
/// - Acknowledgement and whole-host verification flags are always sanitized to false.
pub fn evaluate_control_result(
    result: ControlResult,
    expected_request: &ControlRequest,
    expected_session: &str,
    expected_server: &ProcessIdentity,
    started_at_ms: u64,
    current_time_ms: u64,
) -> RequestOutcome {
    match validate_raw_result(
        &result,
        expected_request,
        expected_session,
        expected_server,
        started_at_ms,
        current_time_ms,
    ) {
        Ok(is_explicit_failure) => {
            let mut sanitized = result;
            sanitized.acknowledge_done = false;
            sanitized.whole_host_verified = false;
            if is_explicit_failure {
                RequestOutcome::ExplicitFailure(sanitized)
            } else {
                RequestOutcome::Success(sanitized)
            }
        }
        Err(reason) => RequestOutcome::Rejected {
            request: expected_request.clone(),
            failure_kind: reason.kind,
            raw_status: Some(result.status),
            raw_observation: result.observation,
            error: reason.cause,
        },
    }
}

/// Classifies transport dispatch and receipt into a typed `RequestOutcome`.
///
/// Deterministic contract:
/// - (a) no reply + ok dispatch -> `RequestOutcome::TimedOut`
/// - (b) no reply + dispatch fail -> `RequestOutcome::DispatchFailed`
/// - (c) valid correlated native result wins despite dispatch failure
pub fn classify_transport_outcome(
    dispatch_res: Result<(), anyhow::Error>,
    evaluated_result: Option<RequestOutcome>,
    fallback_request: ControlRequest,
) -> RequestOutcome {
    if let Some(outcome) = evaluated_result {
        outcome
    } else {
        match dispatch_res {
            Ok(()) => RequestOutcome::TimedOut {
                request: fallback_request,
            },
            Err(err) => RequestOutcome::DispatchFailed {
                request: fallback_request,
                error: err,
            },
        }
    }
}

/// Production helper converting `RequestOutcome` into stage success or structured `NavigationError`.
/// Streamlines match arms across navigation stages and maintains diagnostic journal fidelity.
#[allow(clippy::result_large_err)]
pub fn record_outcome_diagnostic(
    intent: &NavigationIntent,
    control_dir: &Path,
    stage: NavigationStage,
    outcome: RequestOutcome,
    expected_terminal: Option<TerminalPaneId>,
    expected_session: Option<String>,
) -> Result<ControlResult, NavigationError> {
    let base_evidence = || {
        DiagnosticEvidence::new()
            .expected_terminal(expected_terminal)
            .requested_session(expected_session.clone())
    };

    match outcome {
        RequestOutcome::Success(res) => {
            match (stage, &res.status) {
                (NavigationStage::InnerFocus, ControlStatus::InnerFocusObserved) => {
                    let term = res.observation.as_ref().and_then(|o| o.terminal);
                    let sess = res.observation.as_ref().and_then(|o| o.session_name.clone());
                    intent.update_progress(|p| {
                        p.inner_confirmed = true;
                        p.inner_terminal = term;
                        p.inner_session = sess;
                    });
                }
                (NavigationStage::OuterFocus, ControlStatus::InnerFocusObserved) => {
                    let term = res.observation.as_ref().and_then(|o| o.terminal);
                    intent.update_progress(|p| {
                        p.outer_confirmed = true;
                        p.outer_pane = term;
                    });
                }
                (NavigationStage::Switch, ControlStatus::SwitchDispatchedUnverified) => {
                    intent.update_progress(|p| {
                        p.switch_dispatched = true;
                    });
                }
                _ => {}
            }

            let evidence = base_evidence().with_result(&res);
            intent.record_success(
                control_dir,
                stage,
                Some(res.status.clone()),
                &intent.progress(),
                evidence,
            );
            Ok(res)
        }
        RequestOutcome::ExplicitFailure(res) => {
            let failure_kind = if res.status == ControlStatus::Superseded {
                NavigationErrorKind::Superseded
            } else {
                NavigationErrorKind::ControlFailure
            };
            let evidence = base_evidence().with_result(&res);
            Err(intent.record_failure(
                control_dir,
                stage,
                failure_kind,
                Some(res.status.clone()),
                &intent.progress(),
                evidence,
                true,
                anyhow::anyhow!("{stage} rejected: {:?}", res.status),
            ))
        }
        RequestOutcome::Rejected {
            request,
            failure_kind,
            raw_status,
            raw_observation,
            error,
        } => {
            let evidence = base_evidence()
                .with_request(&request)
                .with_observation(raw_observation.as_ref());
            Err(intent.record_failure(
                control_dir,
                stage,
                failure_kind,
                raw_status,
                &intent.progress(),
                evidence,
                false,
                error,
            ))
        }
        RequestOutcome::TimedOut { request } => {
            let evidence = base_evidence().with_request(&request);
            Err(intent.record_failure(
                control_dir,
                stage,
                NavigationErrorKind::Timeout,
                None,
                &intent.progress(),
                evidence,
                false,
                anyhow::anyhow!("{stage} request timed out"),
            ))
        }
        RequestOutcome::DispatchFailed { request, error } => {
            let evidence = base_evidence().with_request(&request);
            Err(intent.record_failure(
                control_dir,
                stage,
                NavigationErrorKind::DispatchFailure,
                None,
                &intent.progress(),
                evidence,
                false,
                error,
            ))
        }
    }
}

// ---------------------------------------------------------------------------
// Unit Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;
    use std::path::PathBuf;
    use std::time::Duration;

    fn test_proc() -> ProcessIdentity {
        crate::process::identity(std::process::id()).unwrap()
    }

    fn sample_req(operation: ControlOperation) -> ControlRequest {
        ControlRequest {
            schema_version: SCHEMA_VERSION,
            request_id: "test-req-123".to_string(),
            context: PluginContext {
                server_pid: std::process::id(),
                plugin_id: 1,
                client_id: 1,
                epoch: "e1".to_string(),
            },
            sequence: 1,
            operation,
        }
    }

    fn sample_obs(req: &ControlRequest, session: &str, terminal: Option<TerminalPaneId>) -> FocusObservation {
        FocusObservation {
            registration_request_id: None,
            registration_surface: false,
            context: req.context.clone(),
            session_name: Some(session.to_string()),
            terminal,
            focused_plugin: None,
            application_sequence: 1,
            queried_terminal: None,
            pane_pid: None,
            observed_at_ms: now_ms(),
        }
    }

    fn sample_res(req: &ControlRequest, status: ControlStatus, obs: Option<FocusObservation>) -> ControlResult {
        ControlResult {
            request: req.clone(),
            status,
            observation: obs,
            whole_host_verified: true,
            acknowledge_done: true,
        }
    }

    fn test_dir(prefix: &str) -> (PathBuf, impl Drop) {
        struct Cleanup(PathBuf);
        impl Drop for Cleanup {
            fn drop(&mut self) {
                let _ = std::fs::remove_dir_all(&self.0);
            }
        }
        let p = std::env::temp_dir().join(format!("vj-{prefix}-{}", uuid::Uuid::new_v4().simple()));
        let _ = std::fs::create_dir_all(&p);
        (p.clone(), Cleanup(p))
    }

    #[test]
    fn evaluate_valid_success_sanitizes_flags() {
        let req = sample_req(ControlOperation::Focus { terminal: TerminalPaneId(4) });
        let proc = test_proc();
        let obs = sample_obs(&req, "test-session", Some(TerminalPaneId(4)));
        let raw_res = sample_res(&req, ControlStatus::InnerFocusObserved, Some(obs));

        match evaluate_control_result(raw_res, &req, "test-session", &proc, now_ms().saturating_sub(50), now_ms()) {
            RequestOutcome::Success(clean) => {
                assert!(!clean.whole_host_verified);
                assert!(!clean.acknowledge_done);
                assert_eq!(clean.status, ControlStatus::InnerFocusObserved);
            }
            other => panic!("Expected Success, got {other:?}"),
        }
    }

    #[test]
    fn evaluate_observed_for_focus_rejected_as_observation_mismatch() {
        let req = sample_req(ControlOperation::Focus { terminal: TerminalPaneId(4) });
        let proc = test_proc();
        let obs = sample_obs(&req, "test-session", Some(TerminalPaneId(4)));
        let raw_res = sample_res(&req, ControlStatus::Observed, Some(obs));

        match evaluate_control_result(raw_res, &req, "test-session", &proc, now_ms().saturating_sub(50), now_ms()) {
            RequestOutcome::Rejected { failure_kind, raw_status, error, .. } => {
                assert_eq!(failure_kind, NavigationErrorKind::ObservationMismatch);
                assert_eq!(raw_status, Some(ControlStatus::Observed));
                assert!(error.to_string().contains("Focus operation requires InnerFocusObserved"));
            }
            other => panic!("Expected Rejected, got {other:?}"),
        }
    }

    #[test]
    fn evaluate_correlation_mismatch_rejected() {
        let req = sample_req(ControlOperation::Focus { terminal: TerminalPaneId(4) });
        let proc = test_proc();
        let mut bad_req = req.clone();
        bad_req.request_id = "different-request-id".to_string();
        let raw_res = sample_res(&bad_req, ControlStatus::InnerFocusObserved, None);

        match evaluate_control_result(raw_res, &req, "test-session", &proc, now_ms().saturating_sub(50), now_ms()) {
            RequestOutcome::Rejected { failure_kind, .. } => {
                assert_eq!(failure_kind, NavigationErrorKind::ObservationMismatch);
            }
            other => panic!("Expected Rejected, got {other:?}"),
        }
    }

    #[test]
    fn evaluate_server_process_death_rejected() {
        let mut req = sample_req(ControlOperation::Focus { terminal: TerminalPaneId(4) });
        req.context.server_pid = 999_999;
        let proc = test_proc();
        let raw_res = sample_res(&req, ControlStatus::InnerFocusObserved, None);

        match evaluate_control_result(raw_res, &req, "test-session", &proc, now_ms().saturating_sub(50), now_ms()) {
            RequestOutcome::Rejected { failure_kind, .. } => {
                assert_eq!(failure_kind, NavigationErrorKind::OwnershipFailure);
            }
            other => panic!("Expected Rejected, got {other:?}"),
        }
    }

    #[test]
    fn evaluate_explicit_failure_accepted_without_observation() {
        let req = sample_req(ControlOperation::Focus { terminal: TerminalPaneId(4) });
        let proc = test_proc();
        let started = now_ms().saturating_sub(50);
        let now = now_ms();

        for status in [
            ControlStatus::PaneMissing,
            ControlStatus::StaleContext,
            ControlStatus::Superseded,
            ControlStatus::Unavailable,
        ] {
            let res = sample_res(&req, status.clone(), None);
            match evaluate_control_result(res, &req, "test-session", &proc, started, now) {
                RequestOutcome::ExplicitFailure(clean) => {
                    assert_eq!(clean.status, status);
                    assert!(!clean.whole_host_verified);
                    assert!(!clean.acknowledge_done);
                }
                other => panic!("Expected ExplicitFailure for {status:?}, got {other:?}"),
            }
        }
    }

    #[test]
    fn evaluate_switch_unverified_with_supplied_observation_validates() {
        let req = sample_req(ControlOperation::Switch {
            session_name: "dest-sess".into(),
            terminal: TerminalPaneId(5),
        });
        let proc = test_proc();
        let started = now_ms().saturating_sub(50);
        let now = now_ms();

        // 1. Switch without observation succeeds
        let res_no_obs = sample_res(&req, ControlStatus::SwitchDispatchedUnverified, None);
        match evaluate_control_result(res_no_obs, &req, "dest-sess", &proc, started, now) {
            RequestOutcome::Success(clean) => {
                assert_eq!(clean.status, ControlStatus::SwitchDispatchedUnverified);
                assert!(!clean.whole_host_verified);
            }
            other => panic!("Expected Success without obs, got {other:?}"),
        }

        // 2. Switch with matching observation succeeds
        let obs = sample_obs(&req, "dest-sess", Some(TerminalPaneId(5)));
        let res_valid = sample_res(&req, ControlStatus::SwitchDispatchedUnverified, Some(obs));
        match evaluate_control_result(res_valid, &req, "dest-sess", &proc, started, now) {
            RequestOutcome::Success(clean) => {
                assert_eq!(clean.status, ControlStatus::SwitchDispatchedUnverified);
            }
            other => panic!("Expected Success with valid obs, got {other:?}"),
        }

        // 3. Switch with mismatched observation session is rejected
        let bad_obs = sample_obs(&req, "wrong-sess", Some(TerminalPaneId(5)));
        let res_bad = sample_res(&req, ControlStatus::SwitchDispatchedUnverified, Some(bad_obs));
        match evaluate_control_result(res_bad, &req, "dest-sess", &proc, started, now) {
            RequestOutcome::Rejected { failure_kind, error, .. } => {
                assert_eq!(failure_kind, NavigationErrorKind::ObservationMismatch);
                assert!(error.to_string().contains("Switch observation session mismatch"));
            }
            other => panic!("Expected Rejected with bad sess, got {other:?}"),
        }
    }

    #[test]
    fn evaluate_inner_success_without_observation_is_missing_observation() {
        let req = sample_req(ControlOperation::Focus { terminal: TerminalPaneId(4) });
        let proc = test_proc();
        let raw_res = sample_res(&req, ControlStatus::InnerFocusObserved, None);

        match evaluate_control_result(raw_res, &req, "test-session", &proc, now_ms().saturating_sub(50), now_ms()) {
            RequestOutcome::Rejected { failure_kind, raw_status, .. } => {
                assert_eq!(failure_kind, NavigationErrorKind::MissingObservation);
                assert_eq!(raw_status, Some(ControlStatus::InnerFocusObserved));
            }
            other => panic!("Expected Rejected, got {other:?}"),
        }
    }

    #[test]
    fn evaluate_terminal_mismatch_is_rejected() {
        let req = sample_req(ControlOperation::Focus { terminal: TerminalPaneId(4) });
        let proc = test_proc();
        let obs = sample_obs(&req, "test-session", Some(TerminalPaneId(99)));
        let raw_res = sample_res(&req, ControlStatus::InnerFocusObserved, Some(obs));

        match evaluate_control_result(raw_res, &req, "test-session", &proc, now_ms().saturating_sub(50), now_ms()) {
            RequestOutcome::Rejected { failure_kind, raw_status, .. } => {
                assert_eq!(failure_kind, NavigationErrorKind::ObservationMismatch);
                assert_eq!(raw_status, Some(ControlStatus::InnerFocusObserved));
            }
            other => panic!("Expected Rejected, got {other:?}"),
        }
    }

    #[test]
    fn evaluate_context_session_and_freshness_failures() {
        let req = sample_req(ControlOperation::Focus { terminal: TerminalPaneId(4) });
        let proc = test_proc();
        let started = now_ms().saturating_sub(50);
        let now = now_ms();

        let cases = vec![
            (
                {
                    let mut o = sample_obs(&req, "test-session", Some(TerminalPaneId(4)));
                    o.context.client_id = 99;
                    o
                },
                "Observation context mismatch",
            ),
            (
                sample_obs(&req, "other-session", Some(TerminalPaneId(4))),
                "Observation session mismatch",
            ),
            (
                {
                    let mut o = sample_obs(&req, "test-session", Some(TerminalPaneId(4)));
                    o.observed_at_ms = started - 10;
                    o
                },
                "Observation freshness invalid",
            ),
            (
                {
                    let mut o = sample_obs(&req, "test-session", Some(TerminalPaneId(4)));
                    o.observed_at_ms = now + 5000;
                    o
                },
                "Observation freshness invalid",
            ),
        ];

        for (obs, err_sub) in cases {
            let res = sample_res(&req, ControlStatus::InnerFocusObserved, Some(obs));
            match evaluate_control_result(res, &req, "test-session", &proc, started, now) {
                RequestOutcome::Rejected { failure_kind, error, .. } => {
                    assert_eq!(failure_kind, NavigationErrorKind::ObservationMismatch);
                    assert!(error.to_string().contains(err_sub), "Expected {err_sub} in {error}");
                }
                other => panic!("Expected Rejected, got {other:?}"),
            }
        }
    }

    #[test]
    fn test_evaluate_locate_queried_terminal_validation() {
        let req = sample_req(ControlOperation::Locate { terminal: TerminalPaneId(42) });
        let proc = test_proc();
        let started = now_ms().saturating_sub(50);
        let now = now_ms();

        // 1. None queried_terminal must be rejected (MissingObservation)
        let mut obs_none = sample_obs(&req, "test-session", Some(TerminalPaneId(10)));
        obs_none.pane_pid = Some(12345);
        obs_none.queried_terminal = None;
        let res_none = sample_res(&req, ControlStatus::Observed, Some(obs_none));
        match evaluate_control_result(res_none, &req, "test-session", &proc, started, now) {
            RequestOutcome::Rejected { failure_kind, raw_status, raw_observation, error, .. } => {
                assert_eq!(failure_kind, NavigationErrorKind::MissingObservation);
                assert_eq!(raw_status, Some(ControlStatus::Observed));
                assert_eq!(raw_observation.as_ref().and_then(|o| o.pane_pid), Some(12345));
                assert!(error.to_string().contains("Locate observation queried_terminal mismatch"));
            }
            other => panic!("Expected Rejected with MissingObservation, got {other:?}"),
        }

        // 2. Wrong queried_terminal must be rejected (ObservationMismatch)
        let mut obs_wrong = sample_obs(&req, "test-session", Some(TerminalPaneId(10)));
        obs_wrong.pane_pid = Some(12345);
        obs_wrong.queried_terminal = Some(TerminalPaneId(99));
        let res_wrong = sample_res(&req, ControlStatus::Observed, Some(obs_wrong));
        match evaluate_control_result(res_wrong, &req, "test-session", &proc, started, now) {
            RequestOutcome::Rejected { failure_kind, raw_status, raw_observation, .. } => {
                assert_eq!(failure_kind, NavigationErrorKind::ObservationMismatch);
                assert_eq!(raw_status, Some(ControlStatus::Observed));
                assert_eq!(raw_observation.as_ref().and_then(|o| o.pane_pid), Some(12345));
            }
            other => panic!("Expected Rejected with ObservationMismatch, got {other:?}"),
        }

        // 3. Exact queried_terminal is accepted as valid Observed
        let mut obs_exact = sample_obs(&req, "test-session", Some(TerminalPaneId(10)));
        obs_exact.pane_pid = Some(12345);
        obs_exact.queried_terminal = Some(TerminalPaneId(42));
        let res_exact = sample_res(&req, ControlStatus::Observed, Some(obs_exact));
        match evaluate_control_result(res_exact, &req, "test-session", &proc, started, now) {
            RequestOutcome::Success(clean) => {
                assert_eq!(clean.status, ControlStatus::Observed);
                assert_eq!(clean.observation.as_ref().and_then(|o| o.queried_terminal), Some(TerminalPaneId(42)));
                assert_eq!(clean.observation.as_ref().and_then(|o| o.pane_pid), Some(12345));
                assert!(!clean.whole_host_verified);
                assert!(!clean.acknowledge_done);
            }
            other => panic!("Expected Success, got {other:?}"),
        }
    }

    #[test]
    fn classify_transport_outcome_deterministic() {
        let req = sample_req(ControlOperation::Query);
        let outcome = classify_transport_outcome(Ok(()), None, req.clone());
        assert!(matches!(outcome, RequestOutcome::TimedOut { .. }));

        let outcome = classify_transport_outcome(Err(anyhow::anyhow!("pipe broke")), None, req.clone());
        assert!(matches!(outcome, RequestOutcome::DispatchFailed { .. }));

        let success = RequestOutcome::Success(sample_res(&req, ControlStatus::Observed, None));
        let outcome = classify_transport_outcome(Err(anyhow::anyhow!("pipe broke")), Some(success), req);
        assert!(matches!(outcome, RequestOutcome::Success(_)));
    }

    #[test]
    fn test_record_outcome_diagnostic_updates_progress_and_preserves_on_supersede() {
        let (temp_dir, _guard) = test_dir("progress");
        let intent = NavigationIntent::new(Some("test-host"));
        assert!(!intent.progress().inner_confirmed);

        let req = sample_req(ControlOperation::Focus { terminal: TerminalPaneId(42) });
        let obs = sample_obs(&req, "test-session", Some(TerminalPaneId(42)));
        let res = sample_res(&req, ControlStatus::InnerFocusObserved, Some(obs));

        let recorded = record_outcome_diagnostic(
            &intent,
            &temp_dir,
            NavigationStage::InnerFocus,
            RequestOutcome::Success(res),
            Some(TerminalPaneId(42)),
            Some("test-session".to_string()),
        )
        .unwrap();
        assert_eq!(recorded.status, ControlStatus::InnerFocusObserved);

        let progress = intent.progress();
        assert!(progress.inner_confirmed);
        assert_eq!(progress.inner_terminal, Some(TerminalPaneId(42)));
        assert_eq!(progress.inner_session.as_deref(), Some("test-session"));

        let content = std::fs::read_to_string(temp_dir.join("navigation-diagnostics.ndjson")).unwrap();
        let diag: NavigationDiagnostic = serde_json::from_str(content.lines().last().unwrap()).unwrap();
        assert!(diag.progress.inner_confirmed);

        let superseded_err = intent.record_failure(
            &temp_dir,
            NavigationStage::OuterFocus,
            NavigationErrorKind::Superseded,
            Some(ControlStatus::InnerFocusObserved),
            &intent.progress(),
            DiagnosticEvidence::new().expected_terminal(TerminalPaneId(10)),
            true,
            anyhow::anyhow!("agent navigation superseded after inner focus"),
        );
        assert!(superseded_err.diagnostic.progress.inner_confirmed);
        assert_eq!(superseded_err.diagnostic.control_status, Some(ControlStatus::InnerFocusObserved));
        assert_eq!(superseded_err.diagnostic.failure_kind, Some(NavigationErrorKind::Superseded));
        assert_eq!(superseded_err.display_reason(), "superseded");
    }

    #[test]
    fn test_current_request_id_not_falling_back_to_previous_stage() {
        let (temp_dir, _guard) = test_dir("reqid");
        let intent = NavigationIntent::new(Some("reqid-host"));
        intent.record_request_id("stage1-request-id");
        assert_eq!(intent.request_ids(), vec!["stage1-request-id"]);

        let diag = intent.record_failure(
            &temp_dir,
            NavigationStage::OuterFocus,
            NavigationErrorKind::PreconditionFailure,
            None,
            &intent.progress(),
            DiagnosticEvidence::new().expected_terminal(TerminalPaneId(10)),
            false,
            anyhow::anyhow!("outer precondition failed"),
        );
        assert_eq!(diag.diagnostic.current_request_id, None);
        assert_eq!(diag.diagnostic.request_ids, vec!["stage1-request-id"]);
    }

    #[test]
    fn test_navigation_intent_monotonic_elapsed_independent_of_wall_clock() {
        let mut intent = NavigationIntent::new(Some("test-host"));
        intent.started = Instant::now() - Duration::from_millis(250);
        assert!(intent.elapsed_ms() >= 250);
    }

    #[test]
    fn test_navigation_error_display_scenarios() {
        let (temp_dir, _guard) = test_dir("nav-err");
        let intent = NavigationIntent::new(Some("test-host"));

        // Scenario A: Inner confirmed, outer unconfirmed (explicit PaneMissing)
        let prog_a = NavigationProgress {
            inner_confirmed: true,
            inner_terminal: Some(TerminalPaneId(2)),
            inner_session: Some("sess-a".into()),
            ..Default::default()
        };
        let err_a = intent.record_failure(
            &temp_dir,
            NavigationStage::OuterFocus,
            NavigationErrorKind::ControlFailure,
            Some(ControlStatus::PaneMissing),
            &prog_a,
            DiagnosticEvidence::new().expected_terminal(TerminalPaneId(12)).observed_terminal(TerminalPaneId(7)),
            true,
            anyhow::anyhow!("Outer pane missing"),
        );
        assert_eq!(
            err_a.to_string(),
            "Inner terminal selected; Workspace focus not confirmed (pane missing). Expected outer pane 12, observed pane 7; select Workspace or retry."
        );

        // Scenario B: Both inner AND outer confirmed, but post-verification failed
        let prog_b = NavigationProgress {
            inner_confirmed: true,
            inner_terminal: Some(TerminalPaneId(2)),
            inner_session: Some("sess-a".into()),
            outer_confirmed: true,
            outer_pane: Some(TerminalPaneId(12)),
            ..Default::default()
        };
        let err_b = intent.record_failure(
            &temp_dir,
            NavigationStage::Verification,
            NavigationErrorKind::OwnershipFailure,
            None,
            &prog_b,
            DiagnosticEvidence::new(),
            true,
            anyhow::anyhow!("attachment process died after outer focus"),
        );
        assert_eq!(
            err_b.to_string(),
            "Focus observed; final ownership/visit verification failed (ownership unverified). select Workspace or retry."
        );

        // Scenario C: Untrusted rejection prioritizes rejection type over raw status
        let err_c = intent.record_failure(
            &temp_dir,
            NavigationStage::InnerFocus,
            NavigationErrorKind::ObservationMismatch,
            Some(ControlStatus::InnerFocusObserved),
            &NavigationProgress::default(),
            DiagnosticEvidence::new().expected_terminal(TerminalPaneId(5)).observed_terminal(TerminalPaneId(9)),
            false,
            anyhow::anyhow!("Focus observation terminal mismatch: expected 5, got 9"),
        );
        assert_eq!(
            err_c.to_string(),
            "Inner focus not confirmed (observation mismatch). Expected terminal 5, observed 9; retry."
        );

        // Scenario D: Precondition failure with concise reason extracted
        let err_d = intent.record_failure(
            &temp_dir,
            NavigationStage::Precondition,
            NavigationErrorKind::PreconditionFailure,
            None,
            &NavigationProgress::default(),
            DiagnosticEvidence::new(),
            false,
            anyhow::anyhow!("Outer host session 'h' must have exactly 1 display client attached (found 2)"),
        );
        assert_eq!(
            err_d.to_string(),
            "Navigation precondition failed: single-display requirement failed; verify target and retry."
        );

        // Scenario E: Local cancellation after confirmed focus retains actual wire status in diagnostic, but renders (superseded)
        let err_e = intent.record_failure(
            &temp_dir,
            NavigationStage::OuterFocus,
            NavigationErrorKind::Superseded,
            Some(ControlStatus::InnerFocusObserved),
            &prog_a,
            DiagnosticEvidence::new().expected_terminal(TerminalPaneId(2)),
            true,
            anyhow::anyhow!("agent navigation superseded after inner focus"),
        );
        assert_eq!(err_e.display_reason(), "superseded");
        assert_eq!(
            err_e.to_string(),
            "Inner terminal selected; Workspace focus not confirmed (superseded). Expected outer pane 2; select Workspace or retry."
        );

        // Scenario F: Unconfirmed inner focus superseded renders inner focus reason
        let err_f = intent.record_failure(
            &temp_dir,
            NavigationStage::InnerFocus,
            NavigationErrorKind::Superseded,
            Some(ControlStatus::InnerFocusObserved),
            &NavigationProgress::default(),
            DiagnosticEvidence::new().expected_terminal(TerminalPaneId(2)),
            true,
            anyhow::anyhow!("agent navigation superseded during inner focus"),
        );
        assert_eq!(err_f.display_reason(), "superseded");
        assert_eq!(
            err_f.to_string(),
            "Inner focus not confirmed (superseded). Expected terminal 2; retry."
        );
    }

    #[test]
    fn test_concurrent_journal_append_integrity() {
        let (temp_dir, _guard) = test_dir("journal-conc");
        let intent = NavigationIntent::new(Some("conc-host"));
        let threads: Vec<_> = (0..10)
            .map(|i| {
                let dir = temp_dir.clone();
                let int = intent.clone();
                std::thread::spawn(move || {
                    int.record_failure(
                        &dir,
                        NavigationStage::InnerFocus,
                        NavigationErrorKind::Timeout,
                        None,
                        &NavigationProgress::default(),
                        DiagnosticEvidence::new().expected_terminal(TerminalPaneId(i)),
                        false,
                        anyhow::anyhow!("Thread timeout {i}"),
                    );
                })
            })
            .collect();

        for t in threads {
            t.join().unwrap();
        }

        let content = std::fs::read_to_string(temp_dir.join("navigation-diagnostics.ndjson")).unwrap();
        let lines: Vec<&str> = content.lines().filter(|l| !l.is_empty()).collect();
        assert_eq!(lines.len(), 10);
        for line in lines {
            assert!(serde_json::from_str::<NavigationDiagnostic>(line).is_ok());
        }
    }
}
