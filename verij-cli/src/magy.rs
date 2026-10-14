//! Existing Magy watch records are observed without reading request/prompt data.
use anyhow::{bail, Context, Result};
use serde::{Deserialize, Serialize};
use std::io::{Read, Seek, SeekFrom};
use std::path::{Path, PathBuf};
use verij_types::agent::*;
use verij_types::identity::{PaneKey, ProcessIdentity, SessionInstanceId, TerminalPaneId};

pub fn state_directory() -> Result<PathBuf> {
    let path = std::env::var_os("VERIJ_MAGY_STATE_DIR")
        .or_else(|| std::env::var_os("MAGY_STATE_DIR"))
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("XDG_STATE_HOME").map(|p| PathBuf::from(p).join("magy")))
        .or_else(|| {
            std::env::var_os("MAGY_REAL_HOME")
                .or_else(|| std::env::var_os("HOME"))
                .map(|p| PathBuf::from(p).join(".local/state/magy"))
        })
        .context("Magy state directory unavailable")?;
    if !path.is_absolute() {
        bail!("Magy state directory must be absolute");
    }
    Ok(path)
}

#[derive(Debug, Clone, Deserialize)]
struct WatchState {
    watch_id: String,
    status: String,
    #[allow(dead_code)]
    session: Option<String>,
    pane_id: Option<String>,
    runner_pid: Option<u32>,
    runner_create_time: Option<f64>,
    conversation_id: Option<String>,
    pty_log_path: Option<PathBuf>,
}

fn valid_watch(id: &str) -> bool {
    id.strip_prefix("watch_")
        .is_some_and(|suffix| suffix.len() == 16 && suffix.bytes().all(|b| b.is_ascii_hexdigit()))
}

fn read_state(directory: &Path) -> Result<WatchState> {
    let path = directory.join("state.json");
    let (state, _meta): (WatchState, _) =
        crate::agent_store::read_bounded_json_with_meta(&path, 65536)?;
    if !valid_watch(&state.watch_id)
        || directory.file_name().and_then(|p| p.to_str()) != Some(&state.watch_id)
        || !matches!(
            state.status.as_str(),
            "running" | "completed" | "failed" | "cancelled"
        )
    {
        bail!("invalid Magy watch state");
    }
    Ok(state)
}

fn runner_binding(
    directory: &Path,
    state: &WatchState,
    server: &ProcessIdentity,
    pane: &ProcessIdentity,
) -> Result<ProcessIdentity> {
    let runner = crate::process::identity(state.runner_pid.context("watch has no runner")?)?;
    let expected = state
        .runner_create_time
        .context("watch runner birth unavailable")?;
    let actual = crate::process::started_at_ms(&runner)? as f64 / 1000.0;
    if !expected.is_finite() || (expected - actual).abs() > 0.02 {
        bail!("Magy runner lifetime changed");
    }
    // The trusted renderer owns pane stdio; redirected Agy children do not.
    crate::process::verify_foreground(server, pane, &runner)?;
    let mut command = Vec::new();
    std::fs::File::open(format!("/proc/{}/cmdline", runner.pid))?
        .take(16385)
        .read_to_end(&mut command)?;
    if command.len() > 16384 {
        bail!("oversized runner identity");
    }
    let args: Vec<_> = command
        .split(|b| *b == 0)
        .filter(|s| !s.is_empty())
        .collect();
    let canonical = directory.canonicalize()?;
    if !args.windows(3).any(|w| {
        w[0] == b"-m"
            && w[1] == b"magy.watch_runner"
            && std::str::from_utf8(w[2]).ok().is_some_and(|path| {
                Path::new(path).canonicalize().ok().as_ref() == Some(&canonical)
            })
    }) {
        bail!("process is not this watch's renderer");
    }
    // Python's setenv may not update its initial /proc environ range. The
    // direct Magy child receives both markers at exec, proving runner binding
    // independently from the launcher's inherited Zellij pane environment.
    let mut children = String::new();
    std::fs::File::open(format!("/proc/{}/task/{}/children", runner.pid, runner.pid))?
        .take(65537)
        .read_to_string(&mut children)?;
    if children.len() > 65536 {
        bail!("oversized runner task list");
    }
    let watch_marker = format!("MAGY_WATCH_ID={}", state.watch_id);
    let run_marker = format!("MAGY_RUN_ID={}", state.watch_id);
    let marked_child = children
        .split_whitespace()
        .take(32)
        .filter_map(|p| p.parse::<u32>().ok())
        .any(|pid| {
            std::fs::File::open(format!("/proc/{pid}/environ"))
                .ok()
                .and_then(|f| {
                    let mut environment = Vec::new();
                    f.take(262145).read_to_end(&mut environment).ok()?;
                    if environment.len() > 262144 {
                        return None;
                    }
                    Some(environment)
                })
                .is_some_and(|environment| {
                    environment
                        .split(|b| *b == 0)
                        .any(|value| value == watch_marker.as_bytes())
                        && environment
                            .split(|b| *b == 0)
                            .any(|value| value == run_marker.as_bytes())
                })
        });
    if !marked_child {
        bail!("watch renderer's marked child is unavailable");
    }
    Ok(runner)
}

/// Revalidate runner identity for navigation, rendering and acknowledgement.
pub fn verify_owner(
    identity: &AgentIdentity,
    server: &ProcessIdentity,
    pane: &ProcessIdentity,
) -> Result<()> {
    if let Some(id) = identity.runner_id.as_ref() {
        if identity.kind != AgentKind::Agy || !valid_watch(id) {
            bail!("invalid monitored runner");
        }
        let directory = state_directory()?.join("watches").join(id);
        let state = read_state(&directory)?;
        if state
            .pane_id
            .as_deref()
            .and_then(|p| p.strip_prefix("terminal_"))
            .and_then(|p| p.parse::<u32>().ok())
            != Some(identity.pane_key.terminal.0)
            || SessionInstanceId::from_process(server) != identity.pane_key.session
            || runner_binding(&directory, &state, server, pane)? != identity.process
        {
            bail!("watch pane/runner binding changed");
        }
        Ok(())
    } else {
        if identity.kind == AgentKind::Agy && !identity.is_synthetic {
            crate::process::verify_agy_foreground(server, pane, &identity.process)
        } else {
            crate::process::verify_foreground(server, pane, &identity.process)
        }
    }
}

#[derive(Debug, Default, Serialize, Deserialize)]
struct Cursor {
    offset: u64,
    device: u64,
    inode: u64,
    conversation: Option<String>,
    result: Option<String>,
    saw_activity: bool,
    #[serde(default)]
    watch_status: String,
    #[serde(default)]
    skipping_line: bool,
}

impl Cursor {
    fn consume(&mut self, event: &serde_json::Value) {
        let kind = event["event"].as_str().unwrap_or("");
        let conversation = match kind {
            "init" => event["conversation_id"].as_str(),
            "step_update" => event["step_update"]["conversation_id"].as_str(),
            "result" => event["result"]["conversation_id"].as_str(),
            _ => return,
        }
        .filter(|s| !s.is_empty() && s.len() <= 256 && !s.chars().any(char::is_control));
        let Some(conversation) = conversation else {
            return;
        };
        if self
            .conversation
            .as_deref()
            .is_some_and(|id| id != conversation)
        {
            return;
        }
        if self.conversation.is_none() && kind != "init" {
            return;
        }
        self.conversation = Some(conversation.into());
        match kind {
            "init" | "step_update" => self.saw_activity = true,
            "result" => {
                if let Some(status @ ("SUCCESS" | "ERROR")) = event["result"]["status"].as_str() {
                    self.result = Some(status.into());
                }
            }
            _ => (),
        }
    }

    fn tail(&mut self, path: &Path) -> Result<()> {
        let mut options = std::fs::OpenOptions::new();
        options.read(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.custom_flags(libc::O_NONBLOCK | libc::O_CLOEXEC);
        }
        let mut file = options.open(path)?;
        let metadata = file.metadata()?;
        if !metadata.is_file() {
            bail!("pty log {:?} is not a regular file", path);
        }

        #[cfg(unix)]
        {
            use std::os::unix::fs::MetadataExt;
            if (self.device, self.inode) != (metadata.dev(), metadata.ino())
                || metadata.len() < self.offset
            {
                *self = Self {
                    device: metadata.dev(),
                    inode: metadata.ino(),
                    ..Default::default()
                };
            }
        }
        file.seek(SeekFrom::Start(self.offset))?;
        let mut bytes = Vec::new();
        // Bound allocation and time under record lock; remaining data is read
        // next poll. Huge transcript/output frames are skipped, never stored.
        file.take(1024 * 1024).read_to_end(&mut bytes)?;
        let start = if self.skipping_line {
            match bytes.iter().position(|b| *b == b'\n') {
                Some(n) => {
                    self.skipping_line = false;
                    n + 1
                }
                None => {
                    self.offset += bytes.len() as u64;
                    return Ok(());
                }
            }
        } else {
            0
        };
        let complete = bytes[start..]
            .iter()
            .rposition(|b| *b == b'\n')
            .map_or(start, |n| n + start + 1);
        for line in bytes[start..complete].split(|b| *b == b'\n') {
            if line.len() <= 262144 {
                if let Ok(value) = serde_json::from_slice(line) {
                    self.consume(&value);
                }
            }
        }
        self.offset += complete as u64;
        if complete == start && bytes.len() == 1024 * 1024 {
            // Advance across oversized lines without interpreting fragments.
            // The next newline is skipped before parsing resumes.
            self.offset += (bytes.len() - complete) as u64;
            self.skipping_line = true;
        }
        Ok(())
    }

    fn project(&self, state: &WatchState, revision: u64) -> PartialSourceRecord {
        let turn_id = Some(TurnId(state.watch_id.clone()));
        let success = state.status == "completed" && self.result.as_deref() == Some("SUCCESS");
        let failed = state.status == "failed"
            || (state.status == "running" && self.result.as_deref() == Some("ERROR"));
        let activity = if matches!(state.status.as_str(), "cancelled" | "completed")
            || self.result.is_some()
        {
            ActivityKind::Idle
        } else if self.saw_activity {
            ActivityKind::Working
        } else {
            ActivityKind::Initializing
        };
        PartialSourceRecord {
            source_id: "magy-watch".into(),
            source_epoch: Some(1),
            source_revision: Some(revision),
            turn_epoch: Some(1),
            turn_revision: Some(1),
            turn_id: turn_id.clone(),
            conversation_id: self
                .conversation
                .clone()
                .or_else(|| state.conversation_id.clone())
                .map(ConversationId),
            activity: Some(activity),
            background_task_count: Some(0),
            pending_requests: Some(vec![]),
            successful_completion: success.then(|| SuccessfulCompletion {
                turn_id: turn_id.clone(),
                turn_epoch: 1,
                turn_revision: 1,
                completed_at_ms: crate::agent_store::now_ms(),
                summary: None,
            }),
            execution_error: failed.then(|| ExecutionError {
                turn_id,
                turn_epoch: 1,
                turn_revision: 1,
                message: "Magy watch failed".into(),
                is_terminal: true,
            }),
            capabilities: Some(vec!["activity".into(), "watch_outcome".into()]),
            ..Default::default()
        }
    }
}

#[derive(Default, Serialize)]
pub struct Diagnostics {
    observed: usize,
    rejected: Vec<serde_json::Value>,
}

pub fn reconcile() -> Result<Diagnostics> {
    let mut reconciler = Reconciler::new()?;
    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(5);
    let mut loops = 0;
    while (!reconciler.is_finished() || reconciler.has_pending()) && loops < 100 {
        if std::time::Instant::now() >= deadline {
            break;
        }
        loops += 1;
        let had_pending = reconciler.has_pending();
        reconciler.step(None)?;
        if reconciler.is_finished() && !reconciler.has_pending() {
            break;
        }
        if had_pending && reconciler.has_pending() {
            std::thread::sleep(std::time::Duration::from_millis(10));
        }
    }
    if reconciler.has_pending() {
        if reconciler.diagnostics.rejected.len() < 64 {
            reconciler.diagnostics.rejected.push(serde_json::json!({
                "reason": "reconcile deadline or step budget exceeded during pending operations"
            }));
        }
        reconciler.pending = None;
    }
    Ok(reconciler.take_diagnostics())
}

#[derive(Clone)]
struct CachedWatchBinding {
    runner: ProcessIdentity,
    server: ProcessIdentity,
    pane: ProcessIdentity,
    pane_id: String,
    terminal: u32,
    instance_id: AgentInstanceId,
    root_runtime: PathBuf,
}

enum PendingWatchState {
    SearchingInventory {
        watch_id: String,
        directory: PathBuf,
        state: WatchState,
        runner: ProcessIdentity,
        orig_pane_id: Option<String>,
        terminal: u32,
        candidate_owners: Vec<(ProcessIdentity, ProcessIdentity)>,
    },
    Registering {
        watch_id: String,
        directory: PathBuf,
        state: WatchState,
        runner: ProcessIdentity,
        server: ProcessIdentity,
        pane: ProcessIdentity,
        pane_id: String,
        terminal: u32,
        cursor: crate::agent_store::RegistrationCursor,
    },
}

/// Owned reconciler context held by agent_watcher.
/// Owns persistent watch ReadDir, at most one pending watch/binding operation,
/// persistent incremental inventory cursor, <=128 bounded active cache, and bounded diagnostics.
pub struct Reconciler {
    root: PathBuf,
    watch_dir: Option<std::fs::ReadDir>,
    inventory_cursor: Option<std::fs::ReadDir>,
    pending: Option<PendingWatchState>,
    active_cache: std::collections::HashMap<String, CachedWatchBinding>,
    diagnostics: Diagnostics,
    started: bool,
}

impl Reconciler {
    pub fn new() -> Result<Self> {
        let root = state_directory()?.join("watches");
        Ok(Self {
            root,
            watch_dir: None,
            inventory_cursor: None,
            pending: None,
            active_cache: std::collections::HashMap::new(),
            diagnostics: Diagnostics::default(),
            started: false,
        })
    }

    pub fn has_pending(&self) -> bool {
        self.pending.is_some()
    }

    pub fn is_finished(&self) -> bool {
        self.started && self.watch_dir.is_none() && self.pending.is_none()
    }

    pub fn take_diagnostics(&mut self) -> Diagnostics {
        std::mem::take(&mut self.diagnostics)
    }

    pub fn step(&mut self, stop: Option<&std::sync::atomic::AtomicBool>) -> Result<Diagnostics> {
        if stop.is_some_and(|s| s.load(std::sync::atomic::Ordering::Relaxed)) {
            return Ok(Diagnostics::default());
        }

        let mut slice_diag = Diagnostics::default();

        // 1. Progress pending operation if any
        if let Some(mut pending) = self.pending.take() {
            let (watch_id, directory, runner, orig_pane_id) = match &pending {
                PendingWatchState::SearchingInventory {
                    watch_id,
                    directory,
                    runner,
                    orig_pane_id,
                    ..
                } => (
                    watch_id.clone(),
                    directory.clone(),
                    runner.clone(),
                    orig_pane_id.clone(),
                ),
                PendingWatchState::Registering {
                    watch_id,
                    directory,
                    runner,
                    pane_id,
                    ..
                } => (
                    watch_id.clone(),
                    directory.clone(),
                    runner.clone(),
                    Some(pane_id.clone()),
                ),
            };

            let fresh_state = match read_state(&directory) {
                Ok(s) => s,
                Err(err) => {
                    if self.diagnostics.rejected.len() < 64 {
                        self.diagnostics.rejected.push(serde_json::json!({
                            "watch_id": watch_id,
                            "reason": err.to_string(),
                        }));
                    }
                    if slice_diag.rejected.len() < 64 {
                        slice_diag.rejected.push(serde_json::json!({
                            "watch_id": watch_id,
                            "reason": err.to_string(),
                        }));
                    }
                    self.pending = None;
                    return Ok(slice_diag);
                }
            };

            let expected_birth = fresh_state.runner_create_time.unwrap_or(0.0);
            let actual_birth = match crate::process::started_at_ms(&runner) {
                Ok(ms) => ms as f64 / 1000.0,
                Err(_) => -1.0,
            };
            let birth_changed = !expected_birth.is_finite()
                || actual_birth < 0.0
                || (expected_birth - actual_birth).abs() > 0.02;

            let expected_log = directory.join("pty.log");
            let log_changed = fresh_state
                .pty_log_path
                .as_ref()
                .is_some_and(|p| p != &expected_log);

            if fresh_state.watch_id != watch_id
                || fresh_state.pane_id != orig_pane_id
                || fresh_state.runner_pid != Some(runner.pid)
                || birth_changed
                || log_changed
            {
                if self.diagnostics.rejected.len() < 64 {
                    self.diagnostics.rejected.push(serde_json::json!({
                        "watch_id": watch_id,
                        "reason": "watch identity or binding changed during pending step"
                    }));
                }
                if slice_diag.rejected.len() < 64 {
                    slice_diag.rejected.push(serde_json::json!({
                        "watch_id": watch_id,
                        "reason": "watch identity or binding changed during pending step"
                    }));
                }
                self.pending = None;
                return Ok(slice_diag);
            }

            match &mut pending {
                PendingWatchState::SearchingInventory { ref mut state, .. } => {
                    *state = fresh_state;
                }
                PendingWatchState::Registering { ref mut state, .. } => {
                    *state = fresh_state;
                }
            }

            match pending {
                PendingWatchState::SearchingInventory {
                    ref watch_id,
                    ref directory,
                    ref state,
                    ref runner,
                    orig_pane_id: _,
                    terminal,
                    ref mut candidate_owners,
                } => {
                    let states_dir = crate::fs_watcher::resolve_states_dir();
                    let chunk = crate::inventory::read_states_slice(
                        &states_dir,
                        &mut self.inventory_cursor,
                        16,
                        stop,
                    );
                    for snapshot in chunk {
                        let Some(inventory) = snapshot.inventory.as_ref() else {
                            continue;
                        };
                        let Some(server) = inventory.server_process.as_ref() else {
                            continue;
                        };
                        let Some(pane_info) = inventory
                            .panes
                            .iter()
                            .find(|p| p.terminal_id.0 == terminal && !p.exited)
                        else {
                            continue;
                        };
                        let Some(pane) = pane_info.pane_process.as_ref() else {
                            continue;
                        };
                        if let Ok(bound_runner) = runner_binding(directory, state, server, pane) {
                            if bound_runner == *runner {
                                let pair = (server.clone(), pane.clone());
                                if !candidate_owners.contains(&pair) {
                                    if candidate_owners.len() < 2 {
                                        candidate_owners.push(pair);
                                    }
                                }
                            }
                        }
                    }

                    if self.inventory_cursor.is_some() {
                        self.pending = Some(pending);
                        return Ok(slice_diag);
                    }

                    // EOF reached for inventory!
                    if candidate_owners.len() != 1 {
                        if self.diagnostics.rejected.len() < 64 {
                            self.diagnostics.rejected.push(serde_json::json!({
                                "watch_id": watch_id,
                                "reason": "watch native pane ownership unavailable or ambiguous"
                            }));
                        }
                        if slice_diag.rejected.len() < 64 {
                            slice_diag.rejected.push(serde_json::json!({
                                "watch_id": watch_id,
                                "reason": "watch native pane ownership unavailable or ambiguous"
                            }));
                        }
                        self.pending = None;
                        return Ok(slice_diag);
                    }

                    let (server, pane) = candidate_owners.remove(0);
                    // Revalidate live owner immediately before creating RegistrationCursor
                    if !crate::process::is_alive(&server)
                        || !crate::process::is_alive(&pane)
                        || !crate::process::is_alive(runner)
                        || runner_binding(directory, state, &server, &pane).is_err()
                    {
                        if self.diagnostics.rejected.len() < 64 {
                            self.diagnostics.rejected.push(serde_json::json!({
                                "watch_id": watch_id,
                                "reason": "watch runner or owner died during discovery"
                            }));
                        }
                        if slice_diag.rejected.len() < 64 {
                            slice_diag.rejected.push(serde_json::json!({
                                "watch_id": watch_id,
                                "reason": "watch runner or owner died during discovery"
                            }));
                        }
                        self.pending = None;
                        return Ok(slice_diag);
                    }

                    let reg_cursor = crate::agent_store::RegistrationCursor::new(
                        PaneKey {
                            session: SessionInstanceId::from_process(&server),
                            terminal: TerminalPaneId(terminal),
                        },
                        AgentKind::Agy,
                        runner.clone(),
                        false,
                        Some(watch_id.clone()),
                    )?;
                    pending = PendingWatchState::Registering {
                        watch_id: watch_id.clone(),
                        directory: directory.clone(),
                        state: state.clone(),
                        runner: runner.clone(),
                        server,
                        pane,
                        pane_id: state.pane_id.clone().unwrap_or_default(),
                        terminal,
                        cursor: reg_cursor,
                    };
                }
                _ => {}
            }

            if let PendingWatchState::Registering {
                ref watch_id,
                ref directory,
                ref state,
                ref runner,
                ref server,
                ref pane,
                ref pane_id,
                terminal,
                ref mut cursor,
            } = pending
            {
                // Revalidate immediately before cursor.step
                if !crate::process::is_alive(server)
                    || !crate::process::is_alive(pane)
                    || !crate::process::is_alive(runner)
                    || runner_binding(directory, state, server, pane).is_err()
                {
                    if self.diagnostics.rejected.len() < 64 {
                        self.diagnostics.rejected.push(serde_json::json!({
                            "watch_id": watch_id,
                            "reason": "watch runner, server, or pane died or binding invalidated during registration"
                        }));
                    }
                    if slice_diag.rejected.len() < 64 {
                        slice_diag.rejected.push(serde_json::json!({
                            "watch_id": watch_id,
                            "reason": "watch runner, server, or pane died or binding invalidated during registration"
                        }));
                    }
                    self.pending = None;
                    return Ok(slice_diag);
                }

                match cursor.step(stop)? {
                    crate::agent_store::RegistrationOutcome::Pending => {
                        self.pending = Some(pending);
                        return Ok(slice_diag);
                    }
                    crate::agent_store::RegistrationOutcome::Complete(instance_id) => {
                        // Revalidate immediately before update_instance
                        if !crate::process::is_alive(server)
                            || !crate::process::is_alive(pane)
                            || !crate::process::is_alive(runner)
                            || runner_binding(directory, state, server, pane).is_err()
                        {
                            if self.diagnostics.rejected.len() < 64 {
                                self.diagnostics.rejected.push(serde_json::json!({
                                    "watch_id": watch_id,
                                    "reason": "watch runner, server, or pane died or binding invalidated before update"
                                }));
                            }
                            if slice_diag.rejected.len() < 64 {
                                slice_diag.rejected.push(serde_json::json!({
                                    "watch_id": watch_id,
                                    "reason": "watch runner, server, or pane died or binding invalidated before update"
                                }));
                            }
                            self.pending = None;
                            return Ok(slice_diag);
                        }

                        if self.active_cache.len() >= 128 {
                            self.active_cache
                                .retain(|_, b| crate::process::is_alive(&b.runner));
                            if self.active_cache.len() >= 128 {
                                if let Some(first_key) = self.active_cache.keys().next().cloned() {
                                    self.active_cache.remove(&first_key);
                                }
                            }
                        }
                        self.active_cache.insert(
                            watch_id.clone(),
                            CachedWatchBinding {
                                runner: runner.clone(),
                                server: server.clone(),
                                pane: pane.clone(),
                                pane_id: pane_id.clone(),
                                terminal,
                                instance_id: instance_id.clone(),
                                root_runtime: self.root.clone(),
                            },
                        );
                        let log = directory.join("pty.log");
                        let res = update_instance(state, &instance_id, &log);
                        match res {
                            Ok(()) => {
                                self.diagnostics.observed += 1;
                                slice_diag.observed += 1;
                            }
                            Err(error) if error.to_string() == "watch observation unchanged" => {
                                self.diagnostics.observed += 1;
                                slice_diag.observed += 1;
                            }
                            Err(error) => {
                                self.active_cache.remove(watch_id);
                                if self.diagnostics.rejected.len() < 64 {
                                    self.diagnostics.rejected.push(serde_json::json!({
                                        "watch_id": watch_id,
                                        "reason": error.to_string()
                                    }));
                                }
                                if slice_diag.rejected.len() < 64 {
                                    slice_diag.rejected.push(serde_json::json!({
                                        "watch_id": watch_id,
                                        "reason": error.to_string()
                                    }));
                                }
                            }
                        }
                        self.pending = None;
                        return Ok(slice_diag);
                    }
                }
            }
        }

        // 2. Discover <=32 ALL watch entries per slice
        if self.watch_dir.is_none() {
            self.watch_dir = std::fs::read_dir(&self.root).ok();
            self.started = true;
        }

        let Some(read_dir) = self.watch_dir.as_mut() else {
            return Ok(slice_diag);
        };

        let mut count = 0;
        while count < 32 {
            if stop.is_some_and(|s| s.load(std::sync::atomic::Ordering::Relaxed)) {
                break;
            }

            let entry = match read_dir.next() {
                Some(Ok(e)) => {
                    count += 1;
                    e
                }
                Some(Err(_)) => {
                    count += 1;
                    continue;
                }
                None => {
                    self.watch_dir = None;
                    break;
                }
            };

            let name = entry.file_name();
            let Some(watch_id) = name.to_str().filter(|s| valid_watch(s)) else {
                continue;
            };

            let directory = self.root.join(watch_id);
            let state = match read_state(&directory) {
                Ok(s) => s,
                Err(err) => {
                    if self.diagnostics.rejected.len() < 64 {
                        self.diagnostics.rejected.push(serde_json::json!({
                            "watch_id": watch_id,
                            "reason": err.to_string()
                        }));
                    }
                    if slice_diag.rejected.len() < 64 {
                        slice_diag.rejected.push(serde_json::json!({
                            "watch_id": watch_id,
                            "reason": err.to_string()
                        }));
                    }
                    continue;
                }
            };

            // Dead history rejected before inventory
            let runner_pid = match state.runner_pid {
                Some(pid) => pid,
                None => {
                    let item = serde_json::json!({
                        "watch_id": watch_id,
                        "reason": "watch has no runner"
                    });
                    if self.diagnostics.rejected.len() < 64 {
                        self.diagnostics.rejected.push(item.clone());
                    }
                    if slice_diag.rejected.len() < 64 {
                        slice_diag.rejected.push(item);
                    }
                    continue;
                }
            };
            let runner = match crate::process::identity(runner_pid) {
                Ok(r) => r,
                Err(err) => {
                    let item = serde_json::json!({
                        "watch_id": watch_id,
                        "reason": err.to_string()
                    });
                    if self.diagnostics.rejected.len() < 64 {
                        self.diagnostics.rejected.push(item.clone());
                    }
                    if slice_diag.rejected.len() < 64 {
                        slice_diag.rejected.push(item);
                    }
                    continue;
                }
            };
            let expected_birth = match state.runner_create_time {
                Some(b) => b,
                None => {
                    let item = serde_json::json!({
                        "watch_id": watch_id,
                        "reason": "watch runner birth unavailable"
                    });
                    if self.diagnostics.rejected.len() < 64 {
                        self.diagnostics.rejected.push(item.clone());
                    }
                    if slice_diag.rejected.len() < 64 {
                        slice_diag.rejected.push(item);
                    }
                    continue;
                }
            };
            let actual_birth = match crate::process::started_at_ms(&runner) {
                Ok(ms) => ms as f64 / 1000.0,
                Err(err) => {
                    let item = serde_json::json!({
                        "watch_id": watch_id,
                        "reason": err.to_string()
                    });
                    if self.diagnostics.rejected.len() < 64 {
                        self.diagnostics.rejected.push(item.clone());
                    }
                    if slice_diag.rejected.len() < 64 {
                        slice_diag.rejected.push(item);
                    }
                    continue;
                }
            };
            if !expected_birth.is_finite() || (expected_birth - actual_birth).abs() > 0.02 {
                let item = serde_json::json!({
                    "watch_id": watch_id,
                    "reason": "Magy runner lifetime changed"
                });
                if self.diagnostics.rejected.len() < 64 {
                    self.diagnostics.rejected.push(item.clone());
                }
                if slice_diag.rejected.len() < 64 {
                    slice_diag.rejected.push(item);
                }
                continue;
            }
            if !crate::process::is_alive(&runner) {
                let item = serde_json::json!({
                    "watch_id": watch_id,
                    "reason": "Magy runner is dead"
                });
                if self.diagnostics.rejected.len() < 64 {
                    self.diagnostics.rejected.push(item.clone());
                }
                if slice_diag.rejected.len() < 64 {
                    slice_diag.rejected.push(item);
                }
                continue;
            }

            let log = directory.join("pty.log");
            if state.pty_log_path.as_ref().is_some_and(|p| p != &log) {
                let item = serde_json::json!({
                    "watch_id": watch_id,
                    "reason": "watch log binding differs from owned directory"
                });
                if self.diagnostics.rejected.len() < 64 {
                    self.diagnostics.rejected.push(item.clone());
                }
                if slice_diag.rejected.len() < 64 {
                    slice_diag.rejected.push(item);
                }
                continue;
            }

            let terminal = match state
                .pane_id
                .as_deref()
                .and_then(|p| p.strip_prefix("terminal_"))
                .and_then(|p| p.parse::<u32>().ok())
            {
                Some(t) => t,
                None => {
                    let item = serde_json::json!({
                        "watch_id": watch_id,
                        "reason": "watch has no native terminal"
                    });
                    if self.diagnostics.rejected.len() < 64 {
                        self.diagnostics.rejected.push(item.clone());
                    }
                    if slice_diag.rejected.len() < 64 {
                        slice_diag.rejected.push(item);
                    }
                    continue;
                }
            };

            // Check cache hit
            let cached_valid = if let Some(cached) = self.active_cache.get(&state.watch_id) {
                let same_runtime = cached.root_runtime == self.root;
                let same_runner = cached.runner == runner;
                let same_pane_id = state.pane_id.as_deref() == Some(&cached.pane_id);
                let alive_server = crate::process::is_alive(&cached.server);
                let alive_pane = crate::process::is_alive(&cached.pane);
                if same_runtime && same_runner && same_pane_id && alive_server && alive_pane {
                    let ident = AgentIdentity {
                        schema_version: AGENT_SCHEMA_VERSION,
                        agent_instance_id: cached.instance_id.clone(),
                        pane_key: PaneKey {
                            session: SessionInstanceId::from_process(&cached.server),
                            terminal: TerminalPaneId(cached.terminal),
                        },
                        kind: AgentKind::Agy,
                        process: cached.runner.clone(),
                        registered_at_ms: 0,
                        is_synthetic: false,
                        runner_id: Some(state.watch_id.clone()),
                    };
                    if verify_owner(&ident, &cached.server, &cached.pane).is_ok() {
                        Some(cached.instance_id.clone())
                    } else {
                        None
                    }
                } else {
                    None
                }
            } else {
                None
            };

            if let Some(instance_id) = cached_valid {
                let res = update_instance(&state, &instance_id, &log);
                match res {
                    Ok(()) => {
                        self.diagnostics.observed += 1;
                        slice_diag.observed += 1;
                    }
                    Err(error) if error.to_string() == "watch observation unchanged" => {
                        self.diagnostics.observed += 1;
                        slice_diag.observed += 1;
                    }
                    Err(error) => {
                        self.active_cache.remove(&state.watch_id);
                        if self.diagnostics.rejected.len() < 64 {
                            self.diagnostics.rejected.push(serde_json::json!({
                                "watch_id": watch_id,
                                "reason": error.to_string()
                            }));
                        }
                        if slice_diag.rejected.len() < 64 {
                            slice_diag.rejected.push(serde_json::json!({
                                "watch_id": watch_id,
                                "reason": error.to_string()
                            }));
                        }
                    }
                }
            } else {
                // Cache miss! Remove stale entry if any
                self.active_cache.remove(&state.watch_id);

                // Setup ONE pending watch/binding operation
                self.inventory_cursor = None;
                self.pending = Some(PendingWatchState::SearchingInventory {
                    watch_id: state.watch_id.clone(),
                    directory: directory.clone(),
                    orig_pane_id: state.pane_id.clone(),
                    state,
                    runner,
                    terminal,
                    candidate_owners: Vec::with_capacity(2),
                });
                break;
            }
        }

        if self.watch_dir.is_none() {
            self.active_cache.retain(|_, b| {
                crate::process::is_alive(&b.runner)
                    && crate::process::is_alive(&b.server)
                    && crate::process::is_alive(&b.pane)
            });
        }

        Ok(slice_diag)
    }
}

fn update_instance(state: &WatchState, instance: &AgentInstanceId, log: &Path) -> Result<()> {
    let updated = crate::agent_store::update_try(
        instance,
        |stored| {
            let mut cursor: Cursor = stored
                .adapter_state
                .get("magy-watch")
                .map(|value| serde_json::from_value(value.clone()))
                .transpose()?
                .unwrap_or_default();
            cursor.tail(log)?;
            cursor.watch_status = state.status.clone();
            if state
                .conversation_id
                .as_ref()
                .is_some_and(|id| cursor.conversation.as_ref().is_some_and(|c| c != id))
            {
                bail!("watch conversation differs from bound stream");
            }
            let revision = stored
                .sources
                .get("magy-watch")
                .map_or(1, |s| s.source_revision + 1);
            let partial = cursor.project(state, revision);
            if stored.adapter_state.get("magy-watch") == Some(&serde_json::to_value(&cursor)?) {
                bail!("watch observation unchanged");
            }
            stored.sources.remove("magy-watch");
            stored
                .adapter_state
                .insert("magy-watch".into(), serde_json::to_value(cursor)?);
            Ok(partial)
        },
        |_| Ok(()),
    );
    match updated {
        Ok(Some(_)) | Ok(None) => Ok(()),
        Err(err) => Err(err),
    }
}

#[allow(dead_code)]
pub fn reconcile_slice(
    reconciler: &mut Reconciler,
    stop: Option<&std::sync::atomic::AtomicBool>,
) -> Result<Diagnostics> {
    reconciler.step(stop)
}

pub fn run(once: bool) -> Result<()> {
    let mut reconciler = Reconciler::new()?;
    loop {
        let diagnostics = if once {
            reconcile()?
        } else {
            reconciler.step(None)?
        };
        if once {
            println!("{}", serde_json::to_string(&diagnostics)?);
            return Ok(());
        }
        std::thread::sleep(std::time::Duration::from_secs(1));
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use serde_json::json;

    fn watch(status: &str) -> WatchState {
        serde_json::from_value(json!({"watch_id":"watch_0123456789abcdef","status":status}))
            .unwrap()
    }

    #[test]
    fn tool_error_is_not_run_failure_and_stream_success_waits_for_watch_finalization() {
        let mut cursor = Cursor::default();
        cursor.consume(&json!({"event":"init","conversation_id":"c"}));
        cursor.consume(&json!({"event":"step_update","step_update":{"conversation_id":"c","state":"ERROR","step_type":"tool"}}));
        let partial = cursor.project(&watch("running"), 1);
        assert_eq!(partial.activity, Some(ActivityKind::Working));
        assert!(partial.execution_error.is_none());
        cursor.consume(
            &json!({"event":"result","result":{"conversation_id":"foreign","status":"SUCCESS"}}),
        );
        assert!(cursor.result.is_none());
        cursor.consume(
            &json!({"event":"result","result":{"conversation_id":"c","status":"SUCCESS"}}),
        );
        assert!(cursor
            .project(&watch("running"), 2)
            .successful_completion
            .is_none());
        assert!(cursor
            .project(&watch("failed"), 3)
            .execution_error
            .is_some());
        assert!(cursor
            .project(&watch("cancelled"), 4)
            .successful_completion
            .is_none());
        assert!(cursor
            .project(&watch("completed"), 5)
            .successful_completion
            .is_some());
    }

    #[test]
    fn saved_offset_partial_lines_truncation_rotation_and_large_frames_are_bounded() {
        use std::io::Write;
        let root = tempfile::tempdir().unwrap();
        let path = root.path().join("pty.log");
        std::fs::write(
            &path,
            "{\"event\":\"init\",\"conversation_id\":\"c\"}\n{\"event\":\"result\"",
        )
        .unwrap();
        let mut cursor = Cursor::default();
        cursor.tail(&path).unwrap();
        assert_eq!(cursor.conversation.as_deref(), Some("c"));
        let offset = cursor.offset;
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        file.write_all(b",\"result\":{\"conversation_id\":\"c\",\"status\":\"SUCCESS\"}}\n")
            .unwrap();
        cursor.tail(&path).unwrap();
        assert!(cursor.offset > offset);
        assert_eq!(cursor.result.as_deref(), Some("SUCCESS"));
        let serialized = serde_json::to_value(&cursor).unwrap();
        let mut restored: Cursor = serde_json::from_value(serialized).unwrap();
        restored.tail(&path).unwrap();
        assert_eq!(restored.offset, cursor.offset);
        std::fs::write(
            &path,
            "{\"event\":\"init\",\"conversation_id\":\"replacement\"}\n",
        )
        .unwrap();
        restored.tail(&path).unwrap();
        assert!(restored.result.is_none());
        assert_eq!(restored.conversation.as_deref(), Some("replacement"));
        let old = root.path().join("old.log");
        std::fs::rename(&path, old).unwrap();
        std::fs::write(&path, "x".repeat(2 * 1024 * 1024)).unwrap();
        let mut file = std::fs::OpenOptions::new()
            .append(true)
            .open(&path)
            .unwrap();
        file.write_all(b"\n{\"event\":\"init\",\"conversation_id\":\"new\"}\n")
            .unwrap();
        restored.tail(&path).unwrap();
        restored.tail(&path).unwrap();
        restored.tail(&path).unwrap();
        assert_eq!(restored.conversation.as_deref(), Some("new"));
    }

    #[test]
    #[cfg(unix)]
    fn test_fifo_rejection_nonblocking() {
        use std::ffi::CString;
        let root = tempfile::tempdir().unwrap();
        let fifo_path = root.path().join("fifo.log");
        let c_path = CString::new(fifo_path.to_str().unwrap()).unwrap();
        unsafe {
            libc::mkfifo(c_path.as_ptr(), 0o644);
        }
        let mut cursor = Cursor::default();
        let tail_res = cursor.tail(&fifo_path);
        assert!(tail_res.is_err(), "Opening FIFO must fail without blocking");

        let watch_dir = root.path().join("watch_0123456789abcdef");
        std::fs::create_dir_all(&watch_dir).unwrap();
        let state_fifo = watch_dir.join("state.json");
        let c_state = CString::new(state_fifo.to_str().unwrap()).unwrap();
        unsafe {
            libc::mkfifo(c_state.as_ptr(), 0o644);
        }
        let state_res = read_state(&watch_dir);
        assert!(
            state_res.is_err(),
            "Reading FIFO state must fail without blocking"
        );
    }

    #[test]
    fn test_reconciler_dead_history_rejected_before_inventory() {
        let root = tempfile::tempdir().unwrap();
        let watch_dir = root.path().join("watch_0123456789abcdef");
        std::fs::create_dir_all(&watch_dir).unwrap();
        let state = serde_json::json!({
            "watch_id": "watch_0123456789abcdef",
            "status": "running",
            "runner_pid": 999999,
            "runner_create_time": 100.0,
            "pane_id": "terminal_1"
        });
        std::fs::write(
            watch_dir.join("state.json"),
            serde_json::to_vec(&state).unwrap(),
        )
        .unwrap();
        std::fs::write(watch_dir.join("pty.log"), b"").unwrap();

        let mut reconciler = Reconciler {
            root: root.path().to_path_buf(),
            watch_dir: None,
            inventory_cursor: None,
            pending: None,
            active_cache: std::collections::HashMap::new(),
            diagnostics: Diagnostics::default(),
            started: false,
        };

        // Cancellation test: stop flag set
        let stop = std::sync::atomic::AtomicBool::new(true);
        let diag_stopped = reconciler.step(Some(&stop)).unwrap();
        assert_eq!(diag_stopped.observed, 0);

        // Resume with stop = false: dead runner should be rejected before inventory search
        let stop_false = std::sync::atomic::AtomicBool::new(false);
        let diag = reconciler.step(Some(&stop_false)).unwrap();
        assert_eq!(diag.observed, 0);
        assert!(!diag.rejected.is_empty(), "Dead history must be rejected");
        assert!(
            reconciler.pending.is_none(),
            "Dead history must never enter pending inventory search"
        );
    }

    #[test]
    fn test_candidate_owners_dedup_and_capped_at_two() {
        let p1 = ProcessIdentity {
            pid: 100,
            start_jiffies: 1,
            boot_id: "b1".into(),
            uid: 1000,
        };
        let p2 = ProcessIdentity {
            pid: 101,
            start_jiffies: 2,
            boot_id: "b1".into(),
            uid: 1000,
        };
        let p3 = ProcessIdentity {
            pid: 102,
            start_jiffies: 3,
            boot_id: "b1".into(),
            uid: 1000,
        };
        let p4 = ProcessIdentity {
            pid: 103,
            start_jiffies: 4,
            boot_id: "b1".into(),
            uid: 1000,
        };
        let mut candidates: Vec<(ProcessIdentity, ProcessIdentity)> = Vec::with_capacity(2);

        // 1. Duplicate snapshots of same owner must not manufacture ambiguity
        for _ in 0..5 {
            let pair = (p1.clone(), p2.clone());
            if !candidates.contains(&pair) {
                if candidates.len() < 2 {
                    candidates.push(pair);
                }
            }
        }
        assert_eq!(candidates.len(), 1);
        assert_eq!(candidates.capacity(), 2);

        // 2. Second distinct owner pair
        let pair2 = (p1.clone(), p3.clone());
        if !candidates.contains(&pair2) {
            if candidates.len() < 2 {
                candidates.push(pair2);
            }
        }
        assert_eq!(candidates.len(), 2);
        assert_eq!(candidates.capacity(), 2);

        // 3. >2 distinct owners: capped at 2, never allocates beyond 2
        let pair3 = (p1.clone(), p4.clone());
        if !candidates.contains(&pair3) {
            if candidates.len() < 2 {
                candidates.push(pair3);
            }
        }
        assert_eq!(candidates.len(), 2);
        assert!(
            candidates.capacity() <= 2,
            "Never allocate beyond 2 candidate owners"
        );
    }

    #[test]
    fn test_pane_replacement_invalidates_pending() {
        let root = tempfile::tempdir().unwrap();
        let watch_dir = root.path().join("watch_0123456789abcdef");
        std::fs::create_dir_all(&watch_dir).unwrap();

        let runner_pid = std::process::id();
        let runner = crate::process::identity(runner_pid).unwrap();
        let started_ms = crate::process::started_at_ms(&runner).unwrap();
        let birth = started_ms as f64 / 1000.0;
        let pty_log = watch_dir.join("pty.log");
        std::fs::write(&pty_log, b"").unwrap();

        let state_val = serde_json::json!({
            "watch_id": "watch_0123456789abcdef",
            "status": "running",
            "runner_pid": runner_pid,
            "runner_create_time": birth,
            "pane_id": "terminal_1",
            "pty_log_path": pty_log
        });
        std::fs::write(
            watch_dir.join("state.json"),
            serde_json::to_vec(&state_val).unwrap(),
        )
        .unwrap();

        let watch_state: WatchState = serde_json::from_value(state_val).unwrap();

        let mut reconciler = Reconciler {
            root: root.path().to_path_buf(),
            watch_dir: None,
            inventory_cursor: None,
            pending: Some(PendingWatchState::SearchingInventory {
                watch_id: "watch_0123456789abcdef".to_string(),
                directory: watch_dir.clone(),
                state: watch_state,
                runner,
                orig_pane_id: Some("terminal_1".to_string()),
                terminal: 1,
                candidate_owners: Vec::with_capacity(2),
            }),
            active_cache: std::collections::HashMap::new(),
            diagnostics: Diagnostics::default(),
            started: false,
        };

        // Pane replacement: rewrite state.json with pane_id = "terminal_2"
        let updated_state = serde_json::json!({
            "watch_id": "watch_0123456789abcdef",
            "status": "running",
            "runner_pid": runner_pid,
            "runner_create_time": birth,
            "pane_id": "terminal_2",
            "pty_log_path": pty_log
        });
        std::fs::write(
            watch_dir.join("state.json"),
            serde_json::to_vec(&updated_state).unwrap(),
        )
        .unwrap();

        let diag = reconciler.step(None).unwrap();
        assert!(
            reconciler.pending.is_none(),
            "Pending must be invalidated and retired"
        );
        assert!(
            diag.rejected
                .iter()
                .any(|r| r["reason"] == "watch identity or binding changed during pending step"),
            "Expected binding change diagnostic"
        );
    }

    #[test]
    fn test_pending_completed_to_cancelled_cannot_authorize_done() {
        let root = tempfile::tempdir().unwrap();
        let watch_dir = root.path().join("watch_0123456789abcdef");
        std::fs::create_dir_all(&watch_dir).unwrap();

        let pty_log = watch_dir.join("pty.log");
        std::fs::write(
            &pty_log,
            b"{\"event\":\"init\",\"conversation_id\":\"conv-1\"}\n{\"event\":\"result\",\"result\":{\"conversation_id\":\"conv-1\",\"status\":\"SUCCESS\"}}\n",
        )
        .unwrap();

        let mut cursor = Cursor::default();
        cursor.tail(&pty_log).unwrap();
        assert_eq!(cursor.result.as_deref(), Some("SUCCESS"));

        // If watch completed, project gives successful_completion
        let completed_watch = serde_json::from_value::<WatchState>(serde_json::json!({
            "watch_id": "watch_0123456789abcdef",
            "status": "completed",
            "conversation_id": "conv-1"
        }))
        .unwrap();
        let completed_proj = cursor.project(&completed_watch, 1);
        assert!(completed_proj.successful_completion.is_some());

        // But when watch is cancelled, projection CANNOT authorize Done
        let cancelled_watch = serde_json::from_value::<WatchState>(serde_json::json!({
            "watch_id": "watch_0123456789abcdef",
            "status": "cancelled",
            "conversation_id": "conv-1"
        }))
        .unwrap();
        let cancelled_proj = cursor.project(&cancelled_watch, 2);
        assert!(
            cancelled_proj.successful_completion.is_none(),
            "Cancelled status must never authorize successful_completion"
        );
        assert_eq!(cancelled_proj.activity, Some(ActivityKind::Idle));

        // When applied to an instance in agent store, status is NOT Done
        let (_guard, _runtime, _state) = {
            let dir = tempfile::tempdir().unwrap();
            let runtime = dir.path().join("runtime");
            std::fs::create_dir_all(&runtime).unwrap();
            std::env::set_var("VERIJ_AGENT_STATE_DIR", &runtime);
            (dir, runtime, ())
        };
        let pane_key = PaneKey {
            session: SessionInstanceId("s-cancelled".into()),
            terminal: TerminalPaneId(1),
        };
        let proc = ProcessIdentity {
            pid: std::process::id(),
            start_jiffies: 10,
            boot_id: "boot-c".into(),
            uid: unsafe { libc::getuid() },
        };
        let inst =
            crate::agent_store::register(pane_key, AgentKind::Agy, proc, true, None).unwrap();

        // Update instance with cancelled state
        update_instance(&cancelled_watch, &inst, &pty_log).unwrap();

        let (_id, state) = crate::agent_store::inspect(&inst).unwrap();
        assert_ne!(
            state.reduced.status,
            verij_types::agent::AgentStatus::Done,
            "Cancelled watch must never transition instance to Done"
        );
        assert!(state.reduced.latest_completion.is_none());
    }
}
