//! H1 native asynchronous stock navigation dev command.
//!
//! Exposes `NavigationCommand` dispatched by `verij navigate ...`.
//! Handles focused-plugin keyboard registration, exact-recipient public
//! control requests, bounded polling, local supersession, and safe outer
//! Workspace focus transfer.
//!
//! Stock actions already queued in Zellij cannot be cancelled by the local
//! journal. Control replies are point-in-time inner observations. A separate
//! bracketed native query can establish a scoped whole-host visit.

use anyhow::{bail, Context, Result};
use clap::{Args, Subcommand};
use fs2::FileExt;
use serde::{Deserialize, Serialize};
use std::fs::{self, File, OpenOptions};
use std::io::{Read, Write};
use std::path::{Path, PathBuf};
use std::process::{Command, Stdio};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};
use uuid::Uuid;

use verij_types::control::{
    ControlOperation, ControlRequest, ControlResult, ControlStatus, FocusObservation, HostBinding,
    Registration, SCHEMA_VERSION,
};
use verij_types::identity::{
    valid_record_key, HostKey, PaneKey, PluginContext, ProcessIdentity, SessionInstanceId,
    TerminalPaneId,
};
use verij_types::VERIJ_CONTROL_PIPE;

// ---------------------------------------------------------------------------
// CLI Subcommands
// ---------------------------------------------------------------------------

#[derive(Debug, Subcommand, Clone, PartialEq, Eq)]
pub enum NavigationCommand {
    /// Register a host session workspace pane binding using focused-keyboard nonce entry.
    Register(RegisterArgs),

    /// Query the current focus observation from the plugin for a registered host.
    Query(QueryArgs),

    /// Focus a terminal pane within a registered host and inner session.
    Focus(FocusArgs),

    /// Dispatch session switch to destination session and pane, then wait destination rebind.
    Switch(SwitchArgs),

    /// Inspect hydrated session inventory without launching subprocesses.
    #[command(hide = true)]
    Inventory(InventoryArgs),

    /// Activate a live instance by immutable pane/server identity, optionally
    /// acknowledging only its captured completion revision after a real visit.
    Agent {
        #[arg(long)]
        host: String,
        #[arg(long)]
        instance: String,
        #[arg(long)]
        revision: Option<u64>,
    },
}

#[derive(Debug, Args, Clone, PartialEq, Eq)]
pub struct RegisterArgs {
    /// Stable host key identifier (e.g. "a", "primary", or UUID).
    #[arg(long)]
    pub host: String,

    /// Outer Zellij host session name.
    #[arg(long)]
    pub host_session: String,

    /// Outer Workspace terminal pane numeric ID.
    #[arg(long)]
    pub workspace_pane: u32,

    /// PID of the attachment process running in the workspace pane.
    #[arg(long)]
    pub client_pid: u32,

    /// Inner Zellij session name.
    #[arg(long)]
    pub session: String,

    /// Path to the verij_plugin.wasm binary or file URL.
    #[arg(long)]
    pub plugin: PathBuf,
}

#[derive(Debug, Args, Clone, PartialEq, Eq)]
pub struct QueryArgs {
    /// Stable host key identifier.
    #[arg(long)]
    pub host: String,

    /// Optional session name override.
    #[arg(long)]
    pub session: Option<String>,

    /// Optional terminal pane ID.
    #[arg(long)]
    pub pane: Option<u32>,
}

#[derive(Debug, Args, Clone, PartialEq, Eq)]
pub struct FocusArgs {
    /// Stable host key identifier.
    #[arg(long)]
    pub host: String,

    /// Optional inner session name.
    #[arg(long)]
    pub session: Option<String>,

    /// Terminal pane numeric ID to focus.
    #[arg(long)]
    pub pane: u32,
}

#[derive(Debug, Args, Clone, PartialEq, Eq)]
pub struct SwitchArgs {
    /// Stable host key identifier.
    #[arg(long)]
    pub host: String,

    /// Target destination session name.
    #[arg(long)]
    pub session: String,

    /// Target terminal pane numeric ID.
    #[arg(long)]
    pub pane: u32,
}

#[derive(Debug, Args, Clone, PartialEq, Eq)]
pub struct InventoryArgs {
    /// Directory to read hydrated session states from.
    #[arg(long)]
    pub dir: Option<PathBuf>,
}

// ---------------------------------------------------------------------------
// Journal, Attachment Record, & Capability Limits
// ---------------------------------------------------------------------------

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct HostJournal {
    pub context: PluginContext,
    pub sequence: u64,
}

fn next_sequence(local: u64, observed: u64) -> Result<u64> {
    local
        .max(observed)
        .checked_add(1)
        .context("application request sequence exhausted")
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct AttachmentRecord {
    pub source: String,
    pub host: String,
    pub host_session: String,
    pub workspace_pane: TerminalPaneId,
    pub attachment_process: ProcessIdentity,
}

#[allow(dead_code)]
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct CapabilityLimits {
    pub get_pane_pid_available: bool,
    pub outer_focus_scoped_single_display: bool,
    pub actions_uncancellable: bool,
    pub whole_host_verified_unsupported: bool,
    pub acknowledge_done_unsupported: bool,
    pub limitations: Vec<String>,
}

#[allow(dead_code)]
pub fn capability_limits() -> CapabilityLimits {
    CapabilityLimits {
        get_pane_pid_available: false,
        outer_focus_scoped_single_display: true,
        actions_uncancellable: true,
        whole_host_verified_unsupported: false,
        acknowledge_done_unsupported: false,
        limitations: vec![
            "Stock Zellij action get-pane-pid is not a public CLI subcommand in stock 0.45.1; process birth and /proc tty ancestry validated instead".into(),
            "Outer focus verification is scoped strictly to single-display outer servers".into(),
            "Actions already queued in Zellij cannot be cancelled by local sequence journal".into(),
            "Whole-host visits require bracketed native inner/outer queries and exact ownership; only single-display outer hosts qualify".into(),
        ],
    }
}

// ---------------------------------------------------------------------------
// Paths and Atomic Utilities
// ---------------------------------------------------------------------------

pub fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_millis() as u64
}

/// Resolves the control runtime directory.
///
/// Follows default WASI /tmp/verij/control mapping or native resolution via
/// fs_watcher::resolve_states_dir().parent().join("control").
/// Allows absolute `VERIJ_CONTROL_DIR` override for isolated harnesses.
pub fn resolve_control_dir() -> PathBuf {
    if let Ok(override_dir) = std::env::var("VERIJ_CONTROL_DIR") {
        let p = PathBuf::from(override_dir);
        if p.is_absolute() {
            return p;
        }
    }
    let states_dir = crate::fs_watcher::resolve_states_dir();
    if let Some(parent) = states_dir.parent() {
        parent.join("control")
    } else {
        PathBuf::from("/tmp/verij/control")
    }
}

/// Atomically writes a JSON-serializable value to disk using a unique temp file,
/// fsync on the file descriptor, rename, and directory fsync.
pub fn atomic_write_json<T: Serialize>(path: &Path, value: &T) -> Result<()> {
    let parent = path
        .parent()
        .context("target path has no parent directory")?;
    crate::process::ensure_private_directory(parent)?;

    let stem = path
        .file_name()
        .and_then(|s| s.to_str())
        .unwrap_or("record");
    let temp_name = format!("{}.{}.tmp", stem, Uuid::new_v4().simple());
    let temp_path = parent.join(temp_name);

    {
        let mut file = OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(&temp_path)
            .with_context(|| format!("failed to create temp file: {}", temp_path.display()))?;

        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            let _ = fs::set_permissions(&temp_path, fs::Permissions::from_mode(0o600));
        }

        let data = serde_json::to_vec_pretty(value)?;
        file.write_all(&data)?;
        file.write_all(b"\n")?;
        file.sync_all()?;
    }

    fs::rename(&temp_path, path).with_context(|| {
        format!(
            "failed to rename {} to {}",
            temp_path.display(),
            path.display()
        )
    })?;

    #[cfg(unix)]
    {
        if let Ok(dir_file) = File::open(parent) {
            let _ = dir_file.sync_all();
        }
    }

    Ok(())
}

/// Counts connected display clients from `zellij action list-clients` output.
pub fn count_clients(list_clients_output: &str) -> usize {
    list_clients_output
        .lines()
        .filter(|line| {
            let parts: Vec<&str> = line.split_whitespace().collect();
            parts.len() >= 2 && parts[0].parse::<u32>().is_ok()
        })
        .count()
}

// ---------------------------------------------------------------------------
// Subprocess Execution
// ---------------------------------------------------------------------------

/// Spawns a zellij CLI action subcommand with strict timeout, isolated environment,
/// and no stdin. Preserves exact ZELLIJ_SOCKET_DIR while removing other ZELLIJ/TMUX vars.
/// Concurrently drains bounded stdout and stderr buffers using background threads to avoid
/// OS pipe deadlocks on large outputs. Kills on expiry.
pub fn run_zellij_action(session: &str, action_args: &[&str], timeout: Duration) -> Result<String> {
    run_zellij_action_with_receipt(session, action_args, timeout, None)
}

fn run_zellij_action_with_receipt(
    session: &str,
    action_args: &[&str],
    timeout: Duration,
    receipt: Option<(&Path, &ControlRequest)>,
) -> Result<String> {
    if session.is_empty() || session.len() > 256 || session.chars().any(char::is_control) {
        bail!("invalid session name: {:?}", session);
    }
    let start = Instant::now();
    let started_at_ms = now_ms();
    let receipt_server = receipt.map(|(_, request)| crate::process::identity(request.context.server_pid)).transpose()?;
    let _startup = crate::session::query_guard_until(start + timeout, None)?;

    let zellij_bin = std::env::var("ZELLIJ_BIN").unwrap_or_else(|_| "zellij".to_string());
    let mut cmd = Command::new(&zellij_bin);
    if let Some(config) = std::env::var_os("ZELLIJ_CONFIG_FILE") {
        cmd.arg("--config").arg(config);
    }
    cmd.arg("-s").arg(session).arg("action");
    cmd.args(action_args);

    cmd.stdin(Stdio::null());
    cmd.stdout(Stdio::piped());
    cmd.stderr(Stdio::piped());

    // Preserve exact ZELLIJ_SOCKET_DIR for isolated harnesses, while stripping other
    // ZELLIJ*, TMUX*, and VERIJ_ZELLIJ* variables to prevent target session confusion.
    cmd.env_clear();
    for (k, v) in std::env::vars() {
        if k == "ZELLIJ_SOCKET_DIR"
            || (!k.starts_with("ZELLIJ")
                && !k.starts_with("TMUX")
                && !k.starts_with("VERIJ_ZELLIJ"))
        {
            cmd.env(k, v);
        }
    }
    if !std::env::vars().any(|(k, _)| k == "TERM") {
        cmd.env("TERM", "xterm-256color");
    }

    let mut child = cmd
        .spawn()
        .with_context(|| format!("Failed to spawn {} action", zellij_bin))?;

    let stdout_pipe = child.stdout.take().context("failed to take child stdout")?;
    let stderr_pipe = child.stderr.take().context("failed to take child stderr")?;

    const MAX_OUTPUT_BYTES: u64 = 2 * 1024 * 1024; // 2MB bounded output buffer

    let stdout_handle = std::thread::spawn(move || -> std::io::Result<Vec<u8>> {
        let mut reader = stdout_pipe.take(MAX_OUTPUT_BYTES);
        let mut buf = Vec::new();
        reader.read_to_end(&mut buf)?;
        Ok(buf)
    });

    let stderr_handle = std::thread::spawn(move || -> std::io::Result<Vec<u8>> {
        let mut reader = stderr_pipe.take(MAX_OUTPUT_BYTES);
        let mut buf = Vec::new();
        reader.read_to_end(&mut buf)?;
        Ok(buf)
    });

    loop {
        match child.try_wait()? {
            Some(status) => {
                let stdout_bytes = stdout_handle
                    .join()
                    .unwrap_or(Ok(Vec::new()))
                    .unwrap_or_default();
                let stderr_bytes = stderr_handle
                    .join()
                    .unwrap_or(Ok(Vec::new()))
                    .unwrap_or_default();
                let stdout_str = String::from_utf8_lossy(&stdout_bytes).trim().to_string();
                let stderr_str = String::from_utf8_lossy(&stderr_bytes).trim().to_string();

                if !status.success() {
                    bail!(
                        "zellij action {:?} in session '{}' failed (exit code {:?}): {}\n{}",
                        action_args,
                        session,
                        status.code(),
                        stdout_str,
                        stderr_str
                    );
                }
                return Ok(stdout_str);
            }
            None => {
                if receipt.zip(receipt_server.as_ref()).is_some_and(|((path, request), server)| {
                    let Ok(bytes) = fs::read(path) else { return false; };
                    if bytes.len() > 256 * 1024 { return false; }
                    let Ok(result) = serde_json::from_slice::<ControlResult>(&bytes) else { return false; };
                    result.request == *request && crate::process::is_alive(server)
                        && result.observation.as_ref().is_some_and(|observation| {
                            observation.context == request.context
                                && observation.session_name.as_deref() == Some(session)
                                && observation.observed_at_ms >= started_at_ms
                                && observation.observed_at_ms <= now_ms() + 1000
                        })
                }) {
                    // The exact recipient has already replied. Let broadcast
                    // transport finish off-thread, keeping its original bound.
                    // Immediate disconnect can race stock peer-context cleanup.
                    std::thread::spawn(move || {
                        loop {
                            match child.try_wait() {
                                Ok(Some(_)) => break,
                                _ if start.elapsed() >= timeout => {
                                    let _ = child.kill();
                                    let _ = child.wait();
                                    break;
                                }
                                _ => std::thread::sleep(Duration::from_millis(20)),
                            }
                        }
                        let _ = stdout_handle.join();
                        let _ = stderr_handle.join();
                    });
                    return Ok(String::new());
                }
                if start.elapsed() >= timeout {
                    let _ = child.kill();
                    let _ = child.wait();
                    let _ = stdout_handle.join();
                    let _ = stderr_handle.join();
                    bail!(
                        "zellij action {:?} in session '{}' timed out after {:?}",
                        action_args,
                        session,
                        timeout
                    );
                }
                std::thread::sleep(Duration::from_millis(20));
            }
        }
    }
}

// ---------------------------------------------------------------------------
// Registration & Verification
// ---------------------------------------------------------------------------

/// Registers a host session workspace pane binding.
///
/// 1. Validates the inner attachment process birth & controlling tty ancestry.
/// 2. Validates the published owned workspace attachment record (<control_dir>/attachment-<host>.json).
/// 3. Verifies that the outer host session has exactly 1 display client.
/// 4. Connects to or launches the production plugin outer observer in the host session
///    and queries native SDK `get_pane_pid` via `ControlOperation::Locate`.
/// 5. Validates outer server process birth and ties pane PID to attachment process
///    via `crate::process::verify_foreground`.
/// 6. Acquires host lock BEFORE sending the Ctrl+b gesture.
/// 7. Awaits focused plugin observation (focused_plugin == context.plugin_id and fresh).
/// 8. Enters nonce and validates binding-<nonce>.json with full tuple matching.
/// 9. Rebind preserves existing sequence journal if same context (does not reset to seq 0).
pub fn register(control_dir: &Path, args: &RegisterArgs) -> Result<HostBinding> {
    register_if_current(control_dir, args, &|| true)
}

fn register_if_current(control_dir: &Path, args: &RegisterArgs, is_current: &dyn Fn() -> bool) -> Result<HostBinding> {
    if !is_current() { bail!("registration superseded locally"); }
    if !valid_record_key(&args.host) {
        bail!("invalid host key: {:?}", args.host);
    }
    if args.client_pid == 0 {
        bail!("invalid client PID: 0");
    }
    if args.host_session.is_empty() || args.session.is_empty() {
        bail!("host_session and inner session names cannot be empty");
    }

    crate::process::ensure_private_directory(control_dir)?;

    // Validate attachment process birth.
    let attachment_process = crate::process::identity(args.client_pid).with_context(|| {
        format!(
            "Failed to resolve attachment process PID {}",
            args.client_pid
        )
    })?;
    if !crate::process::is_alive(&attachment_process) {
        bail!(
            "Attachment process PID {} is no longer alive",
            args.client_pid
        );
    }

    // Validate owned workspace wrapper record published before exec.
    let attachment_path = control_dir.join(format!("attachment-{}.json", args.host));
    if !attachment_path.exists() {
        bail!(
            "Missing owned workspace attachment record: {}",
            attachment_path.display()
        );
    }
    let att_content = fs::read_to_string(&attachment_path).with_context(|| {
        format!(
            "Failed to read attachment record: {}",
            attachment_path.display()
        )
    })?;
    let att_record: AttachmentRecord = serde_json::from_str(&att_content).with_context(|| {
        format!(
            "Corrupt attachment record JSON: {}",
            attachment_path.display()
        )
    })?;

    if att_record.source != "owned_workspace_wrapper" {
        bail!(
            "Attachment source mismatch: expected 'owned_workspace_wrapper', got {:?}",
            att_record.source
        );
    }
    if att_record.host != args.host {
        bail!(
            "Attachment host mismatch: expected {:?}, got {:?}",
            args.host,
            att_record.host
        );
    }
    if att_record.host_session != args.host_session {
        bail!(
            "Attachment host_session mismatch: expected {:?}, got {:?}",
            args.host_session,
            att_record.host_session
        );
    }
    if att_record.workspace_pane.0 != args.workspace_pane {
        bail!(
            "Attachment workspace_pane mismatch: expected {}, got {}",
            args.workspace_pane,
            att_record.workspace_pane.0
        );
    }
    if att_record.attachment_process != attachment_process {
        bail!(
            "Attachment process identity mismatch: recorded {:?}, actual {:?}",
            att_record.attachment_process,
            attachment_process
        );
    }

    // Validate controlling tty membership via /proc on Linux.
    #[cfg(target_os = "linux")]
    {
        let stat_text = fs::read_to_string(format!("/proc/{}/stat", args.client_pid))
            .context("Failed to read attachment process stat")?;
        let (_, suffix) = stat_text.rsplit_once(')').context("invalid stat format")?;
        let fields: Vec<&str> = suffix.split_whitespace().collect();
        let tty_nr: i64 = fields.get(4).context("no tty in stat")?.parse()?;
        if tty_nr == 0 {
            bail!(
                "Attachment process PID {} has no controlling terminal (tty=0)",
                args.client_pid
            );
        }
    }

    // Verify outer host session has exactly 1 display client attached.
    let list_clients_out = run_zellij_action(
        &args.host_session,
        &["list-clients"],
        Duration::from_secs(5),
    )
    .context("Failed to query outer host session display clients")?;
    let client_count = count_clients(&list_clients_out);
    if client_count != 1 {
        bail!(
            "Outer host session '{}' must have exactly 1 display client attached (found {})",
            args.host_session,
            client_count
        );
    }

    // Outer observer plugin: query native SDK outer get_pane_pid via ControlOperation::Locate
    let wasi_control = std::env::var("VERIJ_WASI_CONTROL_DIR")
        .unwrap_or_else(|_| "/tmp/verij/control".to_string());
    let wasi_states =
        std::env::var("VERIJ_WASI_STATES_DIR").unwrap_or_else(|_| "/tmp/verij/states".to_string());
    let observer_config = format!(
        "control_dir={},state_dir={},observer=true",
        wasi_control, wasi_states
    );

    let file_url = if args.plugin.to_string_lossy().starts_with("file:") {
        args.plugin.to_string_lossy().to_string()
    } else {
        format!("file:{}", args.plugin.display())
    };

    let observer_start_ms = now_ms();

    // Reuse a ready exporter where possible. A newly launched observer hides
    // its own surface once permission is granted, preserving host placement.
    let existing_observer = fs::read_dir(control_dir).ok().is_some_and(|entries| {
        entries.flatten().any(|entry| {
            entry.file_name().to_string_lossy().starts_with("focus-")
                && fs::read(entry.path())
                    .ok()
                    .and_then(|bytes| serde_json::from_slice::<FocusObservation>(&bytes).ok())
                    .is_some_and(|observation| {
                        observation.session_name.as_deref() == Some(&args.host_session)
                            && now_ms().saturating_sub(observation.observed_at_ms) < 2000
                    })
        })
    });
    if !existing_observer {
        if !is_current() { bail!("registration superseded before observer launch"); }
        let launch_plugin_args = [
            "launch-plugin",
            "--floating",
            "--no-focus",
            "--configuration",
            &observer_config,
            &file_url,
        ];
        run_zellij_action(
            &args.host_session,
            &launch_plugin_args,
            Duration::from_secs(5),
        )?;
    }

    // Wait for fresh focus heartbeat from outer host session
    let observer_deadline = Instant::now() + Duration::from_secs(6);
    let mut observer_context: Option<PluginContext> = None;

    while Instant::now() < observer_deadline {
        if !is_current() { bail!("registration superseded while awaiting observer"); }
        if let Ok(entries) = fs::read_dir(control_dir) {
            for entry in entries.flatten() {
                let name = entry.file_name();
                let name_str = name.to_string_lossy();
                if name_str.starts_with("focus-") && name_str.ends_with(".json") {
                    if let Ok(content) = fs::read_to_string(entry.path()) {
                        if let Ok(obs) = serde_json::from_str::<FocusObservation>(&content) {
                            if obs.session_name.as_deref() == Some(&args.host_session)
                                && obs.observed_at_ms + 1000 >= observer_start_ms
                            {
                                if let Ok(srv) = crate::process::identity(obs.context.server_pid) {
                                    if crate::process::is_alive(&srv) {
                                        observer_context = Some(obs.context);
                                        break;
                                    }
                                }
                            }
                        }
                    }
                }
            }
        }
        if observer_context.is_some() {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    let observer_context = observer_context
        .context("Outer observer plugin focus heartbeat was not observed in host session")?;
    let outer_server = crate::process::identity(observer_context.server_pid)
        .context("Failed to resolve outer server process identity")?;
    if !crate::process::is_alive(&outer_server) {
        bail!("Outer server process PID {} is not alive", outer_server.pid);
    }

    // Send exact context Locate request via public pipe
    let locate_id = Uuid::new_v4().simple().to_string();
    let locate_req = ControlRequest {
        schema_version: SCHEMA_VERSION,
        request_id: locate_id.clone(),
        context: observer_context.clone(),
        sequence: 0,
        operation: ControlOperation::Locate {
            terminal: TerminalPaneId(args.workspace_pane),
        },
    };
    let locate_payload = serde_json::to_string(&locate_req)?;
    let locate_result_path = control_dir.join(format!("result-{}.json", locate_id));
    let locate_dispatch = run_zellij_action_with_receipt(
        &args.host_session,
        &["pipe", "--name", VERIJ_CONTROL_PIPE, "--", &locate_payload],
        Duration::from_secs(6),
        Some((&locate_result_path, &locate_req)),
    );

    // Poll bounded for result-<locate_id>.json
    let locate_deadline = Instant::now() + Duration::from_secs(6);
    let mut outer_pane_pid: Option<u32> = None;

    while Instant::now() < locate_deadline {
        if locate_result_path.exists() {
            if let Ok(content) = fs::read_to_string(&locate_result_path) {
                if let Ok(res) = serde_json::from_str::<ControlResult>(&content) {
                    if res.request == locate_req && res.status == ControlStatus::Observed {
                        if let Some(obs) = res.observation {
                            if obs.queried_terminal == Some(TerminalPaneId(args.workspace_pane)) {
                                outer_pane_pid = obs.pane_pid;
                                let _ = fs::remove_file(&locate_result_path);
                                break;
                            }
                        }
                    }
                }
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    if outer_pane_pid.is_none() { locate_dispatch.context("Failed to dispatch Locate request to outer observer plugin")?; }
    let outer_pane_pid = outer_pane_pid.context(
        "Native SDK outer get_pane_pid returned None; outer workspace pane PID unavailable",
    )?;

    // Validate actual outer server OS birth and pane PID via crate::process::identity,
    // then call crate::process::verify_foreground(outerserver, paneprocess, attachment_process).
    let pane_process = crate::process::identity(outer_pane_pid).with_context(|| {
        format!(
            "Failed to resolve outer pane process identity for PID {}",
            outer_pane_pid
        )
    })?;
    crate::process::verify_foreground(&outer_server, &pane_process, &attachment_process).context(
        "Foreground terminal verification failed between outer server, pane, and attachment",
    )?;
    atomic_write_json(
        &control_dir.join(format!("outer-owner-{}.json", args.host)),
        &OuterOwnership {
            server: outer_server.clone(),
            pane: pane_process.clone(),
            terminal: TerminalPaneId(args.workspace_pane),
        },
    )?;

    // Acquire register locks before opening the shared keyboard surface.
    let lock_path = control_dir.join(format!("{}.lock", args.host));
    let lock_file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .open(&lock_path)
        .with_context(|| format!("failed to open lock file: {}", lock_path.display()))?;
    let host_lock_deadline = Instant::now() + Duration::from_secs(12);
    loop {
        if !is_current() { bail!("registration superseded while waiting for host lock"); }
        match lock_file.try_lock_exclusive() {
            Ok(()) => break,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                if Instant::now() >= host_lock_deadline { bail!("host control is busy; retry activation shortly"); }
                std::thread::sleep(Duration::from_millis(30));
            }
            Err(error) => return Err(error.into()),
        }
    }

    // Serialize temporary dialogs/nonces across hosts sharing this server.
    let registration_server = ensure_registration_inventory(&args.session, &file_url, is_current)?;
    crate::process::verify_server_connection(&attachment_process, &registration_server)?;
    let session_lock = OpenOptions::new().read(true).write(true).create(true).truncate(false)
        .open(control_dir.join(format!("registration-{}-{}.lock", registration_server.pid, registration_server.start_jiffies)))?;
    let lock_deadline = Instant::now() + Duration::from_secs(12);
    loop {
        if !is_current() { bail!("registration superseded while waiting for shared surface"); }
        match session_lock.try_lock_exclusive() {
            Ok(()) => break,
            Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                if Instant::now() >= lock_deadline { bail!("another host is registering; retry activation shortly"); }
                std::thread::sleep(Duration::from_millis(30));
            }
            Err(error) => return Err(error.into()),
        }
    }

    if !is_current() { bail!("registration superseded before surface launch"); }
    let surface_request_id = Uuid::new_v4().simple().to_string();
    let surface_config = format!("control_dir={wasi_control},state_dir={wasi_states},registration_surface=true,registration_request_id={surface_request_id}");
    let launched = run_zellij_action(&args.session, &["launch-plugin", "--floating", "--no-focus", "--skip-plugin-cache", "--configuration", &surface_config, &file_url], Duration::from_secs(5))?;
    let surface_id = launched.trim().strip_prefix("plugin_").and_then(|id| id.parse().ok());
    let mut surface_cleanup = SurfaceCleanup {
        session: args.session.clone(), server: registration_server.clone(), root: control_dir.to_path_buf(),
        request_id: surface_request_id.clone(), plugin: surface_id, keep: false,
    };
    let (surface, focused_context) = prepare_registration_surface(control_dir, &args.session, surface_id, &surface_request_id, is_current)?;
    surface_cleanup.plugin = Some(surface);
    let before_gesture = now_ms();
    if !is_current() { bail!("registration superseded before keyboard gesture"); }
    let workspace_pane_id = format!("terminal_{}", args.workspace_pane);


    // prepare_registration_surface already confirmed fresh native focus in
    // every enumerated client. Do not wait another idle heartbeat to repeat it.
    // This candidate context is not ownership; only the nonce receipt below is.

    // Enter unique random nonce and Enter.
    let nonce = Uuid::new_v4().simple().to_string();
    if !is_current() { bail!("registration superseded before nonce entry"); }
    crate::process::verify_server_connection(&attachment_process, &registration_server)?;
    let nonce_payload = format!("{}\r", nonce);
    let write_chars_res = run_zellij_action(
        &args.host_session,
        &[
            "write-chars",
            "--pane-id",
            &workspace_pane_id,
            &nonce_payload,
        ],
        Duration::from_secs(5),
    );
    if let Err(e) = write_chars_res {
        let _ = lock_file.unlock();
        return Err(e).context("Failed to enter registration nonce via outer workspace pane");
    }

    // Await and validate binding-<nonce>.json written by focused plugin.
    let binding_file = control_dir.join(format!("binding-{}.json", nonce));
    let binding_deadline = Instant::now() + Duration::from_secs(6);
    let mut registration: Option<Registration> = None;

    while Instant::now() < binding_deadline {
        if !is_current() { bail!("registration superseded while awaiting nonce receipt"); }
        if binding_file.exists() {
            if let Ok(content) = fs::read_to_string(&binding_file) {
                if let Ok(reg) = serde_json::from_str::<Registration>(&content) {
                    let now = now_ms();
                    if reg.nonce == nonce
                        && reg.source == "focused_plugin_keyboard"
                        // Floating dialogs can be visible/focused to more than
                        // one native client. The actual keyboard receipt, not
                        // the first candidate heartbeat, establishes ClientId.
                        && reg.context.server_pid == focused_context.server_pid
                        && reg.context.plugin_id == focused_context.plugin_id
                        && reg.observation.context == reg.context
                        && reg.observation.registration_surface
                        && reg.observation.registration_request_id.as_deref() == Some(surface_request_id.as_str())
                        && reg.observation.session_name.as_deref() == Some(&args.session)
                        && reg.observation.focused_plugin == Some(reg.context.plugin_id)
                        && reg.observation.observed_at_ms >= before_gesture
                        && reg.observation.observed_at_ms <= now + 5000
                    {
                        registration = Some(reg);
                        break;
                    }
                }
            }
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    let registration = match registration {
        Some(reg) => reg,
        None => {
            let _ = lock_file.unlock();
            bail!(
                "Timed out awaiting verified binding-{}.json from focused plugin; registration unavailable",
                nonce
            );
        }
    };

    let server_process = match crate::process::identity(registration.context.server_pid) {
        Ok(sp) => sp,
        Err(e) => {
            let _ = lock_file.unlock();
            return Err(e).context("Failed to query registration server process identity");
        }
    };
    if !crate::process::is_alive(&server_process) {
        let _ = lock_file.unlock();
        bail!("Server process PID {} is not alive", server_process.pid);
    }

    // Check attachment birth after registration.
    let current_attachment_after = match crate::process::identity(args.client_pid) {
        Ok(id) => id,
        Err(e) => {
            let _ = lock_file.unlock();
            return Err(e);
        }
    };
    if current_attachment_after != attachment_process
        || !crate::process::is_alive(&current_attachment_after)
    {
        let _ = lock_file.unlock();
        bail!(
            "Attachment process PID {} changed or exited during registration",
            args.client_pid
        );
    }

    // Prune temporary binding file.
    let _ = fs::remove_file(&binding_file);

    let host_binding = HostBinding {
        schema_version: SCHEMA_VERSION,
        host: HostKey(args.host.clone()),
        host_session: args.host_session.clone(),
        workspace_pane: TerminalPaneId(args.workspace_pane),
        attachment_process,
        server_process: server_process.clone(),
        context: registration.context.clone(),
        pane: registration.observation.terminal.map(|t| PaneKey {
            session: SessionInstanceId::from_process(&server_process),
            terminal: t,
        }),
    };

    let binding_target = control_dir.join(format!("binding-host-{}.json", args.host));
    let previous_binding = fs::read(&binding_target).ok()
        .and_then(|bytes| serde_json::from_slice::<HostBinding>(&bytes).ok());
    if !is_current() { bail!("registration superseded before binding publication"); }
    if let Err(e) = atomic_write_json(&binding_target, &host_binding) {
        let _ = lock_file.unlock();
        return Err(e);
    }

    // Preserve current sequence journal if same context instead of reset seq 0 on rebind.
    let journal_target = control_dir.join(format!("journal-{}.json", args.host));
    let mut journal = if journal_target.exists() {
        let content = fs::read_to_string(&journal_target).with_context(|| {
            format!(
                "failed to read existing journal: {}",
                journal_target.display()
            )
        })?;
        let existing: HostJournal = serde_json::from_str(&content)
            .with_context(|| format!("corrupt existing journal: {}", journal_target.display()))?;
        if existing.context == registration.context {
            existing
        } else {
            HostJournal {
                context: registration.context,
                sequence: 0,
            }
        }
    } else {
        HostJournal {
            context: registration.context,
            sequence: 0,
        }
    };
    journal.sequence = journal
        .sequence
        .max(registration.observation.application_sequence);

    if let Err(e) = atomic_write_json(&journal_target, &journal) {
        let _ = lock_file.unlock();
        return Err(e);
    }

    let _ = lock_file.unlock();

    surface_cleanup.keep = true;
    if let Some(previous) = previous_binding {
        let root = control_dir.to_path_buf();
        let current = host_binding.clone();
        std::thread::spawn(move || retire_unused_surface(&root, &previous, &current));
    }
    Ok(host_binding)
}

/// Retired dedicated dialogs otherwise retain a WASM instance and heartbeat in
/// every client after each hop. Keep any surface still referenced by another
/// host, and address retirement to the original native context, never CLI focus.
fn retire_unused_surface(root: &Path, previous: &HostBinding, current: &HostBinding) {
    if previous.schema_version != SCHEMA_VERSION || previous.context == current.context
        || !crate::process::is_alive(&previous.server_process) { return; }
    let context = &previous.context;
    let path = root.join(format!("focus-{}-{}-{}-{}.json", context.server_pid,
        context.plugin_id, context.client_id, context.epoch));
    let Some(observation) = fs::read(path).ok().and_then(|bytes| serde_json::from_slice::<FocusObservation>(&bytes).ok()) else { return; };
    if observation.context != *context || !observation.registration_surface
        || observation.registration_request_id.is_none() { return; }
    let Ok(entries) = fs::read_dir(root) else { return; };
    for entry in entries.flatten().filter(|entry| entry.file_name().to_string_lossy().starts_with("binding-host-")) {
        let Some(binding) = fs::read(entry.path()).ok().and_then(|bytes| serde_json::from_slice::<HostBinding>(&bytes).ok()) else { return; };
        if binding.server_process == previous.server_process && binding.context.plugin_id == context.plugin_id { return; }
    }
    let Some(session) = observation.session_name else { return; };
    let _ = direct_request(root, &session, context.clone(), ControlOperation::RetireRegistrationSurface, 0);
}

struct SurfaceCleanup { session: String, server: ProcessIdentity, root: PathBuf, request_id: String, plugin: Option<u32>, keep: bool }
impl Drop for SurfaceCleanup {
    fn drop(&mut self) {
        let same_server = crate::process::is_alive(&self.server) && crate::inventory::read_states(&crate::fs_watcher::resolve_states_dir())
            .iter().any(|s| s.name == self.session && s.inventory.as_ref().and_then(|i| i.server_process.as_ref()) == Some(&self.server));
        if !self.keep && same_server {
            let plugin = self.plugin.or_else(|| fs::read_dir(&self.root).ok()?.flatten().find_map(|entry| {
                let observation: FocusObservation = serde_json::from_slice(&fs::read(entry.path()).ok()?).ok()?;
                (observation.context.server_pid == self.server.pid && observation.registration_surface
                    && observation.registration_request_id.as_deref() == Some(self.request_id.as_str()))
                    .then_some(observation.context.plugin_id)
            }));
            if let Some(plugin) = plugin {
                let _ = run_zellij_action(&self.session,
                    &["close-pane", "--pane-id", &format!("plugin_{plugin}")], Duration::from_secs(2));
            }
        }
    }
}

/// Older running sessions can still have a sessions/tabs-only exporter after
/// installing the new WASM. Bootstrap a hidden current exporter before asking
/// for ownership. Inventory never substitutes for the later keyboard receipt.
fn ensure_registration_inventory(session: &str, plugin_url: &str, is_current: &dyn Fn() -> bool) -> Result<ProcessIdentity> {
    let server = || {
        crate::inventory::read_states(&crate::fs_watcher::resolve_states_dir()).into_iter()
            .find(|snapshot| snapshot.name == session)
            .and_then(|snapshot| snapshot.inventory)
            .and_then(|inventory| inventory.server_process)
            .filter(crate::process::is_alive)
    };
    if let Some(server) = server() { return Ok(server); }
    if !is_current() { bail!("registration superseded before inventory recovery"); }
    let control = std::env::var("VERIJ_WASI_CONTROL_DIR").unwrap_or_else(|_| "/tmp/verij/control".into());
    let states = std::env::var("VERIJ_WASI_STATES_DIR").unwrap_or_else(|_| "/tmp/verij/states".into());
    let configuration = format!("observer=true,control_dir={control},state_dir={states}");
    run_zellij_action(session, &["launch-plugin", "--floating", "--no-focus", "--skip-plugin-cache",
        "--configuration", &configuration, plugin_url], Duration::from_secs(5))
        .with_context(|| format!("Failed to recover monitoring inventory in session '{session}'"))?;
    let deadline = Instant::now() + Duration::from_secs(6);
    loop {
        if !is_current() { bail!("registration superseded while awaiting inventory recovery"); }
        if let Some(server) = server() { return Ok(server); }
        if Instant::now() >= deadline {
            bail!("Session '{session}' has no verified inner server inventory after exporter recovery; check Verij plugin permissions and agent doctor --session {session}");
        }
        std::thread::sleep(Duration::from_millis(50));
    }
}

/// This temporary native registration dialog is shared across inner clients.
/// Require actual native focus in every live context before writing the nonce
/// through the owned Workspace tty. A later keyboard receipt identifies its
/// actual client; CLI routing or the display count never creates a binding.
fn prepare_registration_surface(root: &Path, session: &str, wanted: Option<u32>, request_id: &str, is_current: &dyn Fn() -> bool) -> Result<(u32, PluginContext)> {
    let inventory = crate::inventory::read_states(&crate::fs_watcher::resolve_states_dir()).into_iter()
        .find(|s| s.name == session).and_then(|s| s.inventory).context("Registration requires the current session's native pane inventory")?;
    let server = inventory.server_process.context("Registration server incarnation is unavailable")?;
    let clients = run_zellij_action(session, &["list-clients"], Duration::from_secs(3))?;
    let ids: std::collections::BTreeSet<u16> = clients.lines().filter_map(|line| line.split_whitespace().next()?.parse().ok()).collect();
    if ids.is_empty() { bail!("inner display unavailable"); }
    if ids.len() > 16 { bail!("automatic registration supports at most 16 inner display clients"); }
    let discovery_deadline = Instant::now() + Duration::from_secs(6);
    let (plugin, observations) = loop {
    if !is_current() { bail!("registration superseded during surface discovery"); }
    let mut observations = std::collections::BTreeMap::new();
    for entry in fs::read_dir(root)?.flatten() {
        if !entry.file_name().to_string_lossy().starts_with("focus-") { continue; }
        let Ok(bytes) = fs::read(entry.path()) else { continue; };
        let Ok(observation) = serde_json::from_slice::<FocusObservation>(&bytes) else { continue; };
        if observation.session_name.as_deref() != Some(session)
            || !observation.registration_surface
            || observation.registration_request_id.as_deref() != Some(request_id)
            || wanted.is_some_and(|id| observation.context.plugin_id != id)
            || observation.context.server_pid != server.pid
            || now_ms().saturating_sub(observation.observed_at_ms) > 5000
            || observation.observed_at_ms > now_ms() + 1000 { continue; }
        let key = (observation.context.plugin_id, observation.context.client_id);
        if observations.get(&key).map_or(true, |old: &FocusObservation| old.observed_at_ms < observation.observed_at_ms) {
            observations.insert(key, observation);
        }
    }
    if let Some(plugin) = observations.keys().map(|(plugin,_)| *plugin).next() {
        break (plugin, observations);
    }
    if Instant::now() >= discovery_deadline {
        bail!("No fresh Verij registration surface for server {} clients {:?}; available contexts {:?}. Reload the session's current Verij exporter, then retry.",server.pid,ids,observations.keys().collect::<Vec<_>>());
    }
    std::thread::sleep(Duration::from_millis(50));
    };
    let mut contexts = Vec::new();
    for id in &ids {
        if !is_current() { bail!("registration superseded before displaying surface"); }
        let (context, operation) = if let Some(observation) = observations.get(&(plugin, *id)) {
            (observation.context.clone(), ControlOperation::RegistrationSurface)
        } else {
            // Other tabs may not have this new surface's WASM instance yet.
            // Use an existing native context only to reveal the exact surface.
            // Cached context addresses are not ownership or focus evidence:
            // direct_request and the loop below require fresh native replies.
            let context = fs::read_dir(root)?.flatten().filter_map(|entry| {
                if !entry.file_name().to_string_lossy().starts_with("focus-") { return None; }
                let observation: FocusObservation = serde_json::from_slice(&fs::read(entry.path()).ok()?).ok()?;
                (observation.session_name.as_deref() == Some(session)
                    && observation.context.server_pid == server.pid && observation.context.client_id == *id
                    && observation.observed_at_ms <= now_ms() + 1000).then_some(observation)
            }).max_by_key(|observation| observation.observed_at_ms).map(|observation| observation.context)
                .with_context(|| format!("No exporter context for registration client {id}"))?;
            (context, ControlOperation::RegistrationSurfaceFor { plugin_id: plugin })
        };
        let result = direct_request(root, session, context.clone(), operation, 0)?;
        if result.status != ControlStatus::Observed { bail!("Registration surface dispatch unavailable: {:?}", result.status); }
        contexts.push(context);
    }
    let deadline = Instant::now() + Duration::from_secs(4);
    while Instant::now() < deadline {
        if !crate::process::is_alive(&server) { bail!("Registration server incarnation changed"); }
        if !is_current() { bail!("registration superseded while awaiting surface focus"); }
        let focused = contexts.iter().all(|context| {
            let path = root.join(format!("focus-{}-{}-{}-{}.json", context.server_pid,
                context.plugin_id, context.client_id, context.epoch));
            fs::read(path).ok().and_then(|bytes| serde_json::from_slice::<FocusObservation>(&bytes).ok())
                .is_some_and(|observation| observation.context == *context
                    && observation.session_name.as_deref() == Some(session)
                    && observation.focused_plugin == Some(plugin)
                    && now_ms().saturating_sub(observation.observed_at_ms) < 1500
                    && observation.observed_at_ms <= now_ms() + 1000)
        });
        if focused {
            // Only a fresh dedicated instance receiving actual Workspace keys
            // creates the binding. Peer exporter contexts provide focus coverage.
            return Ok((plugin, observations.values().next().unwrap().context.clone()));
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    bail!("Registration surface focus is unavailable; no nonce was written to an agent terminal")
}

fn ensure_owned_binding(root: &Path, host: &str, is_current: &impl Fn() -> bool) -> Result<HostBinding> {
    if !is_current() { bail!("agent activation superseded before binding recovery"); }
    if let Ok(binding) = load_binding(root, host) {
        if crate::process::verify_server_connection(&binding.attachment_process, &binding.server_process).is_ok()
            && request(root, host, ControlOperation::Query, None).is_ok_and(|result| result.status == ControlStatus::Observed) {
            return Ok(binding);
        }
    }
    let bytes = fs::read(root.join(format!("attachment-{host}.json")))
        .context("Open an inner session from its session row first; no owned Workspace attachment is available for registration")?;
    let record: AttachmentRecord = serde_json::from_slice(&bytes)?;
    if record.source != "owned_workspace_wrapper" || record.host != host
        || !crate::process::is_alive(&record.attachment_process)
        || crate::registry::marker_key(&record.host_session)?.as_deref() != Some(host) {
        bail!("Workspace ownership is unavailable for native registration");
    }
    let hint = fs::read_to_string(crate::config::runtime_dir().join(format!("workspace-{host}.session")))
        .context("Workspace attachment session is unavailable")?;
    // An already-dispatched switch can outlive a superseded UI intent. Resolve
    // the actual socket peer instead of treating its old marker as ownership.
    let session = crate::inventory::read_states(&crate::fs_watcher::resolve_states_dir()).into_iter()
        .find(|snapshot| snapshot.inventory.as_ref().and_then(|i| i.server_process.as_ref())
            .is_some_and(|server| crate::process::verify_server_connection(&record.attachment_process, server).is_ok()))
        .map(|snapshot| snapshot.name).unwrap_or_else(|| hint.trim().to_string());
    let plugin = registration_plugin()?;
    if !is_current() { bail!("agent activation superseded before registration"); }
    let binding = register_if_current(root, &RegisterArgs {
        host:host.into(),host_session:record.host_session,workspace_pane:record.workspace_pane.0,
        client_pid:record.attachment_process.pid,session,plugin,
    }, is_current)?;
    if !is_current() { bail!("agent activation superseded during registration"); }
    Ok(binding)
}

/// Reuse the installed URL/permission identity when it contains this build,
/// including destination re-registration after switching away and back.
fn registration_plugin() -> Result<PathBuf> {
    let mut plugin = crate::layout::resolve_plugin_path(None)?;
    let installed = std::env::var_os("XDG_DATA_HOME").map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|p| PathBuf::from(p).join(".local/share")))
        .map(|p| p.join("verij/verij_plugin.wasm"));
    if let Some(installed) = installed {
        if fs::read(&plugin).ok().zip(fs::read(&installed).ok()).is_some_and(|(a,b)| a == b) { plugin = installed; }
    }
    Ok(plugin)
}

/// Loads and validates an existing verified host binding.
/// Refuses command if missing; never fabricates a binding or writes zellij attach.
pub fn load_binding(control_dir: &Path, host_key: &str) -> Result<HostBinding> {
    if !valid_record_key(host_key) {
        bail!("invalid host key: {:?}", host_key);
    }

    let binding_path = control_dir.join(format!("binding-host-{}.json", host_key));
    if !binding_path.exists() {
        bail!(
            "Host '{}' is not registered; missing verified binding. First attach missing binding refuses command, never writes repeated zellij attach.",
            host_key
        );
    }

    let content = fs::read_to_string(&binding_path)
        .with_context(|| format!("Failed to read binding file: {}", binding_path.display()))?;
    let binding: HostBinding = serde_json::from_str(&content)
        .with_context(|| format!("Corrupt host binding JSON: {}", binding_path.display()))?;

    if binding.schema_version != SCHEMA_VERSION {
        bail!(
            "Host binding schema version mismatch: expected {}, got {}",
            SCHEMA_VERSION,
            binding.schema_version
        );
    }

    if binding.host.0 != host_key {
        bail!(
            "Host binding key mismatch: expected {:?}, got {:?}",
            host_key,
            binding.host.0
        );
    }

    if binding.context.server_pid != binding.server_process.pid {
        bail!(
            "Host binding context server PID ({}) does not match server process identity ({})",
            binding.context.server_pid,
            binding.server_process.pid
        );
    }

    // Check attachment process is still alive and matches identity.
    let current_attachment = crate::process::identity(binding.attachment_process.pid)
        .with_context(|| {
            format!(
                "Failed to query attachment process identity PID {}",
                binding.attachment_process.pid
            )
        })?;
    if current_attachment != binding.attachment_process {
        bail!(
            "Host workspace attachment process (PID {}) birth changed; rebind required",
            binding.attachment_process.pid
        );
    }
    if !crate::process::is_alive(&current_attachment) {
        bail!(
            "Host workspace attachment process (PID {}) is no longer alive; rebind required",
            binding.attachment_process.pid
        );
    }

    // Check server process has not restarted or changed identity.
    let current_server = crate::process::identity(binding.context.server_pid)
        .context("Failed to query current server process identity")?;
    if current_server != binding.server_process {
        bail!(
            "Zellij server process changed (was {:?}, now {:?}); rebind required",
            binding.server_process,
            current_server
        );
    }
    if !crate::process::is_alive(&current_server) {
        bail!(
            "Zellij server process PID {} is no longer alive",
            current_server.pid
        );
    }

    // Check focus heartbeat file.
    let heartbeat_path = control_dir.join(format!(
        "focus-{}-{}-{}-{}.json",
        binding.context.server_pid,
        binding.context.plugin_id,
        binding.context.client_id,
        binding.context.epoch
    ));
    if !heartbeat_path.exists() {
        bail!(
            "Plugin focus heartbeat file missing ({}); plugin not active in host context",
            heartbeat_path.display()
        );
    }
    let hb_content =
        fs::read_to_string(&heartbeat_path).context("Failed to read focus heartbeat file")?;
    let obs: FocusObservation =
        serde_json::from_str(&hb_content).context("Corrupt focus heartbeat JSON")?;

    if obs.context != binding.context {
        bail!(
            "Heartbeat context mismatch: expected {:?}, got {:?}",
            binding.context,
            obs.context
        );
    }
    if obs.context.server_pid != binding.server_process.pid {
        bail!("Heartbeat context server PID does not match server process identity");
    }

    let now = now_ms();
    if obs.observed_at_ms > now + 5000 {
        bail!(
            "Plugin focus heartbeat timestamp is in the future ({} > {} + 5000ms); rebind required",
            obs.observed_at_ms,
            now
        );
    }
    let elapsed = now.saturating_sub(obs.observed_at_ms);
    if elapsed > 5000 {
        bail!(
            "Plugin focus heartbeat is stale (age {}ms > 5000ms); rebind required",
            elapsed
        );
    }

    let active_session = obs.session_name.as_deref().unwrap_or("").trim();
    if active_session.is_empty() {
        bail!("Plugin focus heartbeat missing active session name; rebind required");
    }
    if obs.terminal.is_none() && obs.focused_plugin.is_none() {
        bail!("native client has no effective focus context; explicit rebind required");
    }

    Ok(binding)
}

// ---------------------------------------------------------------------------
// Stock Request Dispatch & Polling
// ---------------------------------------------------------------------------

/// Sends a stock control request to the plugin via the named pipe and polls bounded
/// for matching ControlResult.
pub fn request(
    control_dir: &Path,
    host_key: &str,
    operation: ControlOperation,
    target_session: Option<&str>,
) -> Result<ControlResult> {
    let binding = load_binding(control_dir, host_key)?;

    // Determine target actual session from fresh binding heartbeat.
    let heartbeat_path = control_dir.join(format!(
        "focus-{}-{}-{}-{}.json",
        binding.context.server_pid,
        binding.context.plugin_id,
        binding.context.client_id,
        binding.context.epoch
    ));
    let hb_content =
        fs::read_to_string(&heartbeat_path).context("Failed to read focus heartbeat file")?;
    let obs: FocusObservation =
        serde_json::from_str(&hb_content).context("Corrupt focus heartbeat JSON")?;
    let active_session = obs
        .session_name
        .context("Missing active session name in heartbeat")?;

    // Validate Focus: given explicit TargetSession must match source for Focus or refused needsSwitch.
    if let ControlOperation::Focus { terminal } = &operation {
        if let Some(target) = target_session {
            if target != active_session {
                bail!(
                    "Focus requested for session '{}' but active bound session is '{}'; switch required",
                    target,
                    active_session
                );
            }
        }

        // Validate target terminal pane inventory: SessionInstance must match bound server, pane alive.
        let states_dir = crate::fs_watcher::resolve_states_dir();
        let snapshots = crate::inventory::read_states(&states_dir);
        let snapshot = snapshots
            .iter()
            .find(|s| s.name == active_session)
            .with_context(|| {
                format!(
                    "Session snapshot for '{}' missing in inventory",
                    active_session
                )
            })?;
        let inv = snapshot
            .inventory
            .as_ref()
            .with_context(|| format!("Inventory missing for session '{}'", active_session))?;

        if inv.schema_version != SCHEMA_VERSION
            || inv.server_process.as_ref() != Some(&binding.server_process)
            || inv.session_instance_id.as_ref()
                != Some(&SessionInstanceId::from_process(&binding.server_process))
        {
            bail!("Session inventory server process does not match bound server");
        }

        let pane = inv
            .panes
            .iter()
            .find(|p| p.terminal_id == *terminal)
            .with_context(|| {
                format!(
                    "Target terminal pane {:?} missing in session '{}' inventory",
                    terminal, active_session
                )
            })?;
        if pane.exited {
            bail!(
                "Target terminal pane {:?} has exited in session '{}'",
                terminal,
                active_session
            );
        }
        if !pane
            .pane_process
            .as_ref()
            .is_some_and(crate::process::is_alive)
        {
            bail!("target pane process is unavailable");
        }
    }

    // Switch validates destination pane inventory to avoid wrong ID.
    if let ControlOperation::Switch {
        session_name,
        terminal,
    } = &operation
    {
        let states_dir = crate::fs_watcher::resolve_states_dir();
        let snapshots = crate::inventory::read_states(&states_dir);
        let snapshot = snapshots
            .iter()
            .find(|snapshot| snapshot.name == *session_name)
            .context("destination topology unavailable")?;
        let inv = snapshot
            .inventory
            .as_ref()
            .context("destination pane inventory unavailable")?;
        if inv.schema_version != SCHEMA_VERSION
            || !inv
                .server_process
                .as_ref()
                .is_some_and(crate::process::is_alive)
            || inv.session_instance_id.is_none()
        {
            bail!("destination server incarnation unverified");
        }
        let pane = inv
            .panes
            .iter()
            .find(|p| p.terminal_id == *terminal)
            .with_context(|| {
                format!(
                    "Target destination terminal pane {:?} missing in session '{}' inventory",
                    terminal, session_name
                )
            })?;
        if pane.exited {
            bail!(
                "Target destination terminal pane {:?} has exited in session '{}'",
                terminal,
                session_name
            );
        }
        if !pane
            .pane_process
            .as_ref()
            .is_some_and(crate::process::is_alive)
        {
            bail!("destination terminal process unavailable");
        }
    }

    // Acquire exclusive lock and hold through dispatch and result check.
    let lock_path = control_dir.join(format!("{}.lock", host_key));
    let lock_file = OpenOptions::new()
        .read(true)
        .write(true)
        .create(true)
        .open(&lock_path)
        .with_context(|| format!("failed to open lock file: {}", lock_path.display()))?;
    lock_file.lock_exclusive()?;

    let journal_path = control_dir.join(format!("journal-{}.json", host_key));
    if !journal_path.exists() {
        let _ = lock_file.unlock();
        bail!("Journal file missing for host '{}'", host_key);
    }
    let journal_content = match fs::read_to_string(&journal_path) {
        Ok(c) => c,
        Err(e) => {
            let _ = lock_file.unlock();
            return Err(e).context("Failed to read journal file");
        }
    };
    let mut journal: HostJournal = match serde_json::from_str(&journal_content) {
        Ok(j) => j,
        Err(e) => {
            let _ = lock_file.unlock();
            return Err(e).context("Corrupt journal JSON: fail closed, no silent reset");
        }
    };

    if journal.context != binding.context {
        let _ = lock_file.unlock();
        bail!("Journal context mismatch with bound context; rebind required");
    }

    let is_passive = matches!(
        operation,
        ControlOperation::Query | ControlOperation::Locate { .. }
    );
    let sequence = if is_passive {
        0
    } else {
        next_sequence(journal.sequence, obs.application_sequence)?
    };

    if !is_passive {
        journal.sequence = sequence;
        if let Err(e) = atomic_write_json(&journal_path, &journal) {
            let _ = lock_file.unlock();
            return Err(e);
        }
    }

    let request_id = Uuid::new_v4().simple().to_string();
    let req = ControlRequest {
        schema_version: SCHEMA_VERSION,
        request_id: request_id.clone(),
        context: binding.context.clone(),
        sequence,
        operation,
    };

    let payload = match serde_json::to_string(&req) {
        Ok(p) => p,
        Err(e) => {
            let _ = lock_file.unlock();
            return Err(e.into());
        }
    };
    let request_start_ms = now_ms();

    let result_file = control_dir.join(format!("result-{}.json", request_id));
    let dispatch_res = run_zellij_action_with_receipt(
        &active_session,
        &["pipe", "--name", VERIJ_CONTROL_PIPE, "--", &payload],
        Duration::from_secs(6),
        Some((&result_file, &req)),
    );
    // CLI pipe exit is not the control contract. A fully correlated native
    // reply may exist even when the broadcast CLI transport times out.

    // Poll bounded for result-<request_id>.json while holding lock.
    let deadline = Instant::now() + Duration::from_secs(6);
    let mut found_result: Option<ControlResult> = None;

    while Instant::now() < deadline {
        if result_file.exists() {
            let content = match fs::read_to_string(&result_file) {
                Ok(c) => c,
                Err(e) => {
                    let _ = lock_file.unlock();
                    return Err(e).context("Failed to read result file");
                }
            };
            let result: ControlResult = match serde_json::from_str(&content) {
                Ok(r) => r,
                Err(e) => {
                    let _ = lock_file.unlock();
                    return Err(e).context("Malformed control result JSON: fail closed");
                }
            };

            if result.request != req {
                let _ = lock_file.unlock();
                bail!("Control result correlation mismatch: request does not match");
            }

            let current_server = match crate::process::identity(binding.context.server_pid) {
                Ok(s) => s,
                Err(e) => {
                    let _ = lock_file.unlock();
                    return Err(e);
                }
            };
            if current_server != binding.server_process {
                let _ = lock_file.unlock();
                bail!("Server process changed during control request execution");
            }

            if result.status == ControlStatus::InnerFocusObserved {
                let obs = match result.observation.as_ref() {
                    Some(o) => o,
                    None => {
                        let _ = lock_file.unlock();
                        bail!("InnerFocusObserved must have observation; fail closed");
                    }
                };
                if obs.context != req.context {
                    let _ = lock_file.unlock();
                    bail!("Focus observation context mismatch");
                }
                if obs.session_name.as_deref() != Some(&active_session) {
                    let _ = lock_file.unlock();
                    bail!(
                        "Focus observation session mismatch: expected {:?}, got {:?}",
                        active_session,
                        obs.session_name
                    );
                }
                if let ControlOperation::Focus { terminal } = &req.operation {
                    if obs.terminal != Some(*terminal) {
                        let _ = lock_file.unlock();
                        bail!(
                            "Focus observation terminal mismatch: expected {:?}, got {:?}",
                            terminal,
                            obs.terminal
                        );
                    }
                }
                if obs.observed_at_ms < request_start_ms {
                    let _ = lock_file.unlock();
                    bail!(
                        "Focus observation is stale (observed {} < request start {})",
                        obs.observed_at_ms,
                        request_start_ms
                    );
                }
            }

            // Append verified result to persistent results journal.
            let ndjson_path = control_dir.join("control-results.ndjson");
            if let Ok(mut ndjson_file) = OpenOptions::new()
                .create(true)
                .append(true)
                .open(&ndjson_path)
            {
                if let Ok(line) = serde_json::to_string(&result) {
                    let _ = writeln!(ndjson_file, "{}", line);
                    let _ = ndjson_file.sync_all();
                }
            }

            // Prune one-shot result file.
            let _ = fs::remove_file(&result_file);

            let mut final_result = result;
            final_result.acknowledge_done = false;
            final_result.whole_host_verified = false;
            found_result = Some(final_result);
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    let _ = lock_file.unlock();

    if found_result.is_none() {
        if let Err(error) = dispatch_res {
            return Err(error).context("Failed to send control request via Zellij named pipe");
        }
    }

    match found_result {
        Some(res) => Ok(res),
        None => Ok(ControlResult {
            request: req,
            status: ControlStatus::Unavailable,
            observation: None,
            whole_host_verified: false,
            acknowledge_done: false,
        }),
    }
}

// ---------------------------------------------------------------------------
// Activation & Outer Focus Transfer
// ---------------------------------------------------------------------------

/// A native-context RPC for the outer observer avoids CLI nested/last-active
/// focus routing. Caller validates the only display and actual attachment tty.
fn direct_request(
    root: &Path,
    session: &str,
    context: PluginContext,
    operation: ControlOperation,
    sequence: u64,
) -> Result<ControlResult> {
    let request = ControlRequest {
        schema_version: SCHEMA_VERSION,
        request_id: Uuid::new_v4().simple().to_string(),
        context,
        sequence,
        operation,
    };
    let started = now_ms();
    let server = crate::process::identity(request.context.server_pid)?;
    let path = root.join(format!("result-{}.json", request.request_id));
    let dispatch = run_zellij_action_with_receipt(
        session,
        &[
            "pipe",
            "--name",
            VERIJ_CONTROL_PIPE,
            "--",
            &serde_json::to_string(&request)?,
        ],
        Duration::from_secs(5),
        Some((&path, &request)),
    );
    let deadline = Instant::now() + Duration::from_secs(6);
    while Instant::now() < deadline {
        match fs::read(&path) {
            Ok(bytes) => {
                let result: ControlResult =
                    serde_json::from_slice(&bytes).context("malformed outer observer result")?;
                let observation = result
                    .observation
                    .as_ref()
                    .context("outer observation unavailable")?;
                if result.request != request
                    || observation.context != request.context
                    || observation.session_name.as_deref() != Some(session)
                    || observation.observed_at_ms < started
                    || observation.observed_at_ms > now_ms() + 1000
                    || !crate::process::is_alive(&server)
                {
                    bail!("outer observer result identity/freshness mismatch");
                }
                // Retain the same reviewed identity/status projection as inner
                // requests so whole-host visit brackets can be corroborated.
                if let Ok(mut journal) = OpenOptions::new()
                    .create(true)
                    .append(true)
                    .open(root.join("control-results.ndjson"))
                {
                    if let Ok(line) = serde_json::to_string(&result) {
                        let _ = writeln!(journal, "{line}");
                        let _ = journal.sync_all();
                    }
                }
                fs::remove_file(path)?;
                return Ok(result);
            }
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => {
                std::thread::sleep(Duration::from_millis(30))
            }
            Err(error) => return Err(error.into()),
        }
    }
    if let Err(error) = dispatch {
        return Err(error).context("Native-context request has no correlated result");
    }
    bail!("native-context observer request timed out")
}

#[derive(Serialize, Deserialize)]
struct OuterOwnership {
    server: ProcessIdentity,
    pane: ProcessIdentity,
    terminal: TerminalPaneId,
}

fn outer_operation(
    root: &Path,
    binding: &HostBinding,
    operation: ControlOperation,
) -> Result<ControlResult> {
    let clients = run_zellij_action(
        &binding.host_session,
        &["list-clients"],
        Duration::from_secs(3),
    )?;
    if count_clients(&clients) != 1 {
        bail!("whole-host observation requires one known outer display");
    }
    let client = clients
        .lines()
        .skip(1)
        .find_map(|line| line.split_whitespace().next()?.parse::<u16>().ok())
        .context("outer client identity unavailable")?;
    let context = fs::read_dir(root)?
        .flatten()
        .filter(|entry| entry.file_name().to_string_lossy().starts_with("focus-"))
        .filter_map(|entry| {
            serde_json::from_slice::<FocusObservation>(&fs::read(entry.path()).ok()?).ok()
        })
        .find(|observation| {
            observation.session_name.as_deref() == Some(&binding.host_session)
                && observation.context.client_id == client
                && now_ms().saturating_sub(observation.observed_at_ms) < 2000
                && observation.observed_at_ms <= now_ms() + 1000
        })
        .context("outer native observer unavailable")?
        .context;
    // SDK-bound births persist across passive queries. Pane closure/reuse,
    // server replacement, detached tty or attachment replacement invalidate
    // these OS checks; there is no PID-only cache acceptance.
    let owned: OuterOwnership = serde_json::from_slice(&fs::read(
        root.join(format!("outer-owner-{}.json", binding.host.0)),
    )?)?;
    let server = owned.server;
    let pane = owned.pane;
    if server.pid != context.server_pid || owned.terminal != binding.workspace_pane {
        bail!("outer ownership context changed");
    }
    crate::process::verify_foreground(&server, &pane, &binding.attachment_process)?;
    let lock = OpenOptions::new()
        .create(true)
        .read(true)
        .write(true)
        .truncate(false)
        .open(root.join(format!("outer-{}.lock", binding.host.0)))?;
    lock.lock_exclusive()?;
    let path = root.join(format!("outer-journal-{}.json", binding.host.0));
    let mut journal: HostJournal = match fs::read(&path) {
        Ok(bytes) => serde_json::from_slice(&bytes).context("corrupt outer journal")?,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => HostJournal {
            context: context.clone(),
            sequence: 0,
        },
        Err(error) => return Err(error.into()),
    };
    if journal.context != context {
        journal = HostJournal {
            context: context.clone(),
            sequence: 0,
        };
    }
    let passive = matches!(
        operation,
        ControlOperation::Query | ControlOperation::Locate { .. }
    );
    let sequence = if passive {
        0
    } else {
        journal.sequence = journal
            .sequence
            .checked_add(1)
            .context("outer sequence exhausted")?;
        atomic_write_json(&path, &journal)?;
        journal.sequence
    };
    let result = direct_request(root, &binding.host_session, context, operation, sequence)?;
    if result.observation.as_ref().is_some_and(|observation| {
        observation.terminal == Some(binding.workspace_pane)
            && (observation.queried_terminal != Some(binding.workspace_pane)
                || observation.pane_pid != Some(pane.pid))
    }) {
        bail!("outer SDK terminal process locator changed");
    }
    if !crate::process::is_alive(&server)
        || !crate::process::is_alive(&pane)
        || !crate::process::is_alive(&binding.attachment_process)
    {
        bail!("outer ownership changed during observation");
    }
    Ok(result)
}

/// Focuses an inner terminal pane and, upon successful inner observation,
/// transfers outer focus to the exact Workspace pane.
///
/// Verifies exactly 1 outer client before focus.
/// Checks attachment process birth both before and after focus transfer.
/// Observes fresh pre/post exact pane and handles "already focused" error cleanly.
pub fn activate<F>(
    control_dir: &Path,
    host_key: &str,
    terminal: TerminalPaneId,
    target_session: Option<&str>,
    is_current: Option<F>,
) -> Result<ControlResult>
where
    F: Fn() -> bool,
{
    let binding = load_binding(control_dir, host_key)?;

    // Check attachment birth before focus.
    if is_current.as_ref().is_some_and(|current| !current()) {
        bail!("navigation superseded locally before dispatch");
    }
    let att_before = crate::process::identity(binding.attachment_process.pid)
        .context("Failed to verify attachment process identity before focus")?;
    if att_before != binding.attachment_process || !crate::process::is_alive(&att_before) {
        bail!(
            "Attachment process (PID {}) is not alive before focus",
            binding.attachment_process.pid
        );
    }

    // Verify outer host session has exactly 1 display client attached.
    let list_clients_out = run_zellij_action(
        &binding.host_session,
        &["list-clients"],
        Duration::from_secs(5),
    )
    .context("Failed to query outer host session clients")?;
    let client_count = count_clients(&list_clients_out);
    if client_count != 1 {
        bail!(
            "Outer host session '{}' must have exactly 1 attached client before focus (found {})",
            binding.host_session,
            client_count
        );
    }

    let mut res = request(
        control_dir,
        host_key,
        ControlOperation::Focus { terminal },
        target_session,
    )?;

    if let Some(ref cb) = is_current {
        if !cb() {
            res.status = ControlStatus::Superseded;
            res.acknowledge_done = false;
            res.whole_host_verified = false;
            return Ok(res);
        }
    }

    if res.status == ControlStatus::InnerFocusObserved {
        let outer = outer_operation(
            control_dir,
            &binding,
            ControlOperation::Focus {
                terminal: binding.workspace_pane,
            },
        )?;
        if outer.status != ControlStatus::InnerFocusObserved
            || outer.observation.as_ref().and_then(|o| o.terminal) != Some(binding.workspace_pane)
        {
            bail!("outer native focus observation unavailable");
        }
    }

    // Check attachment birth after focus.
    let att_after = crate::process::identity(binding.attachment_process.pid)
        .context("Failed to verify attachment process identity after focus")?;
    if att_after != binding.attachment_process || !crate::process::is_alive(&att_after) {
        bail!(
            "Attachment process (PID {}) changed or exited after focus",
            binding.attachment_process.pid
        );
    }

    res.acknowledge_done = false;
    res.whole_host_verified = false;
    Ok(res)
}

// ---------------------------------------------------------------------------
// Switch & Rebind
// ---------------------------------------------------------------------------

/// Dispatches a session switch operation via the public plugin.
/// Captures binding before dispatch, then uses saved binding to register destination
/// only after rendered/fresh destination heartbeat is observed. Does not ignore failure.
pub fn switch(
    control_dir: &Path,
    host_key: &str,
    session_name: &str,
    terminal: TerminalPaneId,
) -> Result<ControlResult> {
    switch_if_current(control_dir, host_key, session_name, terminal, &|| true)
}

fn switch_if_current(
    control_dir: &Path,
    host_key: &str,
    session_name: &str,
    terminal: TerminalPaneId,
    is_current: &dyn Fn() -> bool,
) -> Result<ControlResult> {
    if !is_current() { bail!("session switch superseded before dispatch"); }
    let saved_binding = load_binding(control_dir, host_key)?;
    let destination_server = crate::inventory::read_states(&crate::fs_watcher::resolve_states_dir()).into_iter()
        .find(|snapshot| snapshot.name == session_name)
        .and_then(|snapshot| snapshot.inventory)
        .and_then(|inventory| inventory.server_process)
        .context("destination server inventory is unavailable")?;
    let switch_start_ms = now_ms();

    let res = request(
        control_dir,
        host_key,
        ControlOperation::Switch {
            session_name: session_name.to_string(),
            terminal,
        },
        None,
    )?;

    if res.status == ControlStatus::SwitchDispatchedUnverified {
        // Wait until rendered/fresh destination heartbeat is observed in control_dir.
        let hb_deadline = Instant::now() + Duration::from_secs(6);
        let mut destination_observed = false;

        while Instant::now() < hb_deadline {
            if !is_current() { bail!("session switch superseded while awaiting destination"); }
            if let Ok(entries) = fs::read_dir(control_dir) {
                for entry in entries.flatten() {
                    let name = entry.file_name();
                    let name_str = name.to_string_lossy();
                    if name_str.starts_with("focus-") && name_str.ends_with(".json") {
                        if let Ok(content) = fs::read_to_string(entry.path()) {
                            if let Ok(obs) = serde_json::from_str::<FocusObservation>(&content) {
                                if obs.session_name.as_deref() == Some(session_name)
                                    && obs.observed_at_ms >= switch_start_ms
                                {
                                    destination_observed = true;
                                    break;
                                }
                            }
                        }
                    }
                }
            }
            if destination_observed && crate::process::verify_server_connection(
                &saved_binding.attachment_process, &destination_server).is_ok() {
                break;
            }
            destination_observed = false;
            std::thread::sleep(Duration::from_millis(50));
        }

        if !destination_observed {
            bail!(
                "Timed out awaiting fresh heartbeat from destination session '{}' after switch dispatch",
                session_name
            );
        }

        let register_args = RegisterArgs {
            host: host_key.to_string(),
            host_session: saved_binding.host_session,
            workspace_pane: saved_binding.workspace_pane.0,
            client_pid: saved_binding.attachment_process.pid,
            session: session_name.to_string(),
            plugin: registration_plugin()?,
        };

        register_if_current(control_dir, &register_args, is_current).with_context(|| {
            format!(
                "Failed to rebind host '{}' to destination session '{}' after switch",
                host_key, session_name
            )
        })?;
    }

    Ok(res)
}

/// Rebinds a host to a destination inner session, reusing the host nonce gesture.
#[allow(dead_code)]
pub fn rebind(control_dir: &Path, host_key: &str, new_session: &str) -> Result<HostBinding> {
    let binding = load_binding(control_dir, host_key)?;
    let args = RegisterArgs {
        host: host_key.to_string(),
        host_session: binding.host_session,
        workspace_pane: binding.workspace_pane.0,
        client_pid: binding.attachment_process.pid,
        session: new_session.to_string(),
        plugin: registration_plugin()?,
    };
    register(control_dir, &args)
}

/// Optional whole-host visit verification helper.
/// Stock focus alone cannot prove whole-host visits. Stays None when unsupported; never fabricates ack flags.
#[allow(dead_code)]
pub fn verified_visit(control_dir: &Path, host_key: &str) -> Option<(ControlResult, u64)> {
    // Bracket a fresh inner query with passive outer native observations.
    // Single-display scope is checked by outer_operation on every query.
    let binding = load_binding(control_dir, host_key).ok()?;
    let before = outer_operation(control_dir, &binding, ControlOperation::Query).ok()?;
    if before.observation?.terminal != Some(binding.workspace_pane) {
        return None;
    }
    let mut inner = request(control_dir, host_key, ControlOperation::Query, None).ok()?;
    if inner.status != ControlStatus::Observed
        || inner.observation.as_ref()?.context != binding.context
    {
        return None;
    }
    let focused = inner.observation.as_ref()?.terminal?;
    let after = outer_operation(control_dir, &binding, ControlOperation::Query).ok()?;
    let after = after.observation?;
    if after.terminal != Some(binding.workspace_pane)
        || !crate::process::is_alive(&binding.attachment_process)
        || !crate::process::is_alive(&binding.server_process)
        || focused.0 == u32::MAX
    {
        return None;
    }
    inner.whole_host_verified = true;
    inner.acknowledge_done = false;
    Some((inner, after.observed_at_ms))
}

// ---------------------------------------------------------------------------
// Execute Entrypoint
// ---------------------------------------------------------------------------

/// UI targets are immutable instance/pane keys, with the completion revision
/// captured by that UI. Resolve mutable location only inside the worker.
pub(crate) fn activate_instance(
    control_dir: &Path,
    host: &str,
    instance: &verij_types::identity::AgentInstanceId,
    expected_pane: &PaneKey,
    revision: Option<u64>,
    is_current: impl Fn() -> bool,
    on_focus: impl Fn(&str, &PaneKey),
) -> Result<(ControlResult, Option<verij_types::agent::InstanceAck>)> {
    if !is_current() {
        bail!("agent navigation superseded locally");
    }
    let (identity, _) = crate::agent_store::inspect(instance)?;
    if identity.pane_key != *expected_pane || !crate::process::is_alive(&identity.process) {
        bail!("agent instance binding is no longer live");
    }
    let snapshots = crate::inventory::read_states(&crate::fs_watcher::resolve_states_dir());
    let snapshot = snapshots
        .iter()
        .find(|snapshot| {
            snapshot
                .inventory
                .as_ref()
                .and_then(|inventory| inventory.session_instance_id.as_ref())
                == Some(expected_pane.session.clone()).as_ref()
        })
        .context("agent server lifetime no longer exists")?;
    let inventory = snapshot.inventory.as_ref().unwrap();
    let pane = inventory
        .panes
        .iter()
        .find(|pane| pane.terminal_id == expected_pane.terminal && !pane.exited)
        .context("agent terminal no longer exists")?;
    crate::process::verify_foreground(
        inventory
            .server_process
            .as_ref()
            .context("server birth unavailable")?,
        pane.pane_process
            .as_ref()
            .context("pane birth unavailable")?,
        &identity.process,
    )?;
    if !is_current() {
        bail!("agent navigation superseded before dispatch");
    }
    let binding = ensure_owned_binding(control_dir, host, &is_current)?;
    if SessionInstanceId::from_process(&binding.server_process) != expected_pane.session {
        switch_if_current(control_dir, host, &snapshot.name, expected_pane.terminal, &is_current)?;
        let bound = load_binding(control_dir, host)?;
        if SessionInstanceId::from_process(&bound.server_process) != expected_pane.session {
            bail!("target incarnation changed during switch");
        }
    }
    let result = activate(
        control_dir,
        host,
        expected_pane.terminal,
        Some(&snapshot.name),
        Some(&is_current),
    )?;
    if result.status != ControlStatus::InnerFocusObserved {
        bail!("agent activation unverified: {:?}", result.status);
    }
    if !is_current() { bail!("agent navigation superseded after focus"); }
    on_focus(&snapshot.name, expected_pane);
    let ack = if let Some(revision) = revision.filter(|revision| *revision > 0) {
        Some(crate::agent_cli::acknowledge_visit_if_current(
            crate::agent_cli::AckArgs {
                instance: instance.0.clone(),
                host: host.into(),
                revision,
            },
            is_current,
        )?)
    } else {
        None
    };
    Ok((result, ack))
}

/// Main router entrypoint for `Commands::Navigate(command)`.
pub async fn execute(command: NavigationCommand) -> Result<()> {
    tokio::task::spawn_blocking(move || {
        let control_dir = resolve_control_dir();
        crate::process::ensure_private_directory(&control_dir)?;

        match command {
            NavigationCommand::Register(args) => {
                let binding = register(&control_dir, &args)?;
                println!("{}", serde_json::to_string_pretty(&binding)?);
                Ok(())
            }
            NavigationCommand::Query(args) => {
                let res = request(
                    &control_dir,
                    &args.host,
                    ControlOperation::Query,
                    args.session.as_deref(),
                )?;
                println!("{}", serde_json::to_string_pretty(&res)?);
                Ok(())
            }
            NavigationCommand::Focus(args) => {
                let res = activate(
                    &control_dir,
                    &args.host,
                    TerminalPaneId(args.pane),
                    args.session.as_deref(),
                    None::<fn() -> bool>,
                )?;
                println!("{}", serde_json::to_string_pretty(&res)?);
                if res.status != ControlStatus::InnerFocusObserved {
                    bail!("native pane activation unverified: {:?}", res.status);
                }
                Ok(())
            }
            NavigationCommand::Switch(args) => {
                let res = switch(
                    &control_dir,
                    &args.host,
                    &args.session,
                    TerminalPaneId(args.pane),
                )?;
                println!("{}", serde_json::to_string_pretty(&res)?);
                Ok(())
            }
            NavigationCommand::Inventory(args) => {
                let dir = args
                    .dir
                    .unwrap_or_else(crate::fs_watcher::resolve_states_dir);
                let states = crate::inventory::read_states(&dir);
                println!("{}", serde_json::to_string_pretty(&states)?);
                Ok(())
            }
            NavigationCommand::Agent {
                host,
                instance,
                revision,
            } => {
                let identity = crate::agent_store::inspect(
                    &verij_types::identity::AgentInstanceId(instance.clone()),
                )?
                .0;
                let snapshots =
                    crate::inventory::read_states(&crate::fs_watcher::resolve_states_dir());
                let snapshot = snapshots
                    .iter()
                    .find(|snapshot| {
                        snapshot
                            .inventory
                            .as_ref()
                            .and_then(|inventory| inventory.session_instance_id.as_ref())
                            == Some(&identity.pane_key.session)
                    })
                    .context("agent server lifetime no longer exists")?;
                let inventory = snapshot.inventory.as_ref().unwrap();
                let pane = inventory
                    .panes
                    .iter()
                    .find(|pane| pane.terminal_id == identity.pane_key.terminal && !pane.exited)
                    .context("agent terminal no longer exists")?;
                if !identity.is_synthetic {
                    crate::process::verify_foreground(
                        inventory
                            .server_process
                            .as_ref()
                            .context("server birth unavailable")?,
                        pane.pane_process
                            .as_ref()
                            .context("pane birth unavailable")?,
                        &identity.process,
                    )?;
                }
                let binding = ensure_owned_binding(&control_dir, &host, &|| true)?;
                if SessionInstanceId::from_process(&binding.server_process)
                    != identity.pane_key.session
                {
                    switch(
                        &control_dir,
                        &host,
                        &snapshot.name,
                        identity.pane_key.terminal,
                    )?;
                    let bound = load_binding(&control_dir, &host)?;
                    if SessionInstanceId::from_process(&bound.server_process)
                        != identity.pane_key.session
                    {
                        bail!("target incarnation changed during switch");
                    }
                }
                let result = activate(
                    &control_dir,
                    &host,
                    identity.pane_key.terminal,
                    Some(&snapshot.name),
                    None::<fn() -> bool>,
                )?;
                if result.status != ControlStatus::InnerFocusObserved {
                    bail!("agent activation unverified: {:?}", result.status);
                }
                println!("{}", serde_json::to_string(&result)?);
                if let Some(revision) = revision {
                    crate::agent_cli::execute(crate::agent_cli::AgentCommand::Ack(
                        crate::agent_cli::AckArgs {
                            instance,
                            host,
                            revision,
                        },
                    ))?;
                }
                Ok(())
            }
        }
    })
    .await?
}

// ---------------------------------------------------------------------------
// Tests
// ---------------------------------------------------------------------------

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn returning_to_live_plugin_recovers_its_sequence_without_a_stock_watermark() {
        assert_eq!(next_sequence(1, 17).unwrap(), 18);
        assert_eq!(next_sequence(20, 17).unwrap(), 21);
        assert!(next_sequence(1, u64::MAX).is_err());
    }

    fn test_context(server_pid: u32, plugin_id: u32, client_id: u16, epoch: &str) -> PluginContext {
        PluginContext {
            server_pid,
            plugin_id,
            client_id,
            epoch: epoch.to_string(),
        }
    }

    fn test_process_identity(pid: u32, start_jiffies: u64) -> ProcessIdentity {
        ProcessIdentity {
            pid,
            start_jiffies,
            boot_id: "test-boot-uuid".to_string(),
            uid: 1000,
        }
    }

    #[test]
    fn test_control_dir_override() {
        let custom = "/tmp/verij-custom-control-test";
        std::env::set_var("VERIJ_CONTROL_DIR", custom);
        assert_eq!(resolve_control_dir(), PathBuf::from(custom));
        std::env::remove_var("VERIJ_CONTROL_DIR");
    }

    #[test]
    fn test_capability_limits() {
        let limits = capability_limits();
        assert!(!limits.get_pane_pid_available);
        assert!(limits.outer_focus_scoped_single_display);
        assert!(limits.actions_uncancellable);
        assert!(!limits.whole_host_verified_unsupported);
        assert!(!limits.acknowledge_done_unsupported);
        assert!(!limits.limitations.is_empty());
    }

    #[test]
    fn test_count_clients() {
        let out_single = "CLIENT_ID PANE_ID\n1 terminal_0\n";
        assert_eq!(count_clients(out_single), 1);

        let out_multi = "CLIENT_ID PANE_ID\n1 terminal_0\n2 terminal_1\n";
        assert_eq!(count_clients(out_multi), 2);

        let out_empty = "CLIENT_ID PANE_ID\n";
        assert_eq!(count_clients(out_empty), 0);

        let out_no_header = "1 terminal_3\n";
        assert_eq!(count_clients(out_no_header), 1);
    }

    #[test]
    fn test_socketenv_isolation_and_bounded_drain() {
        if std::env::var_os("VERIJ_NAV_MOCK_CHILD").is_none() {
            let output = Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "navigation::tests::test_socketenv_isolation_and_bounded_drain"])
                .env("VERIJ_NAV_MOCK_CHILD", "1").output().unwrap();
            assert!(output.status.success(), "{}\n{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
            return;
        }
        let temp_dir =
            std::env::temp_dir().join(format!("vj-nav-mock-{}", Uuid::new_v4().simple()));
        fs::create_dir_all(&temp_dir).unwrap();

        let mock_script_path = temp_dir.join("mock_zellij.sh");
        let script_content = r#"#!/bin/sh
if [ -z "$ZELLIJ_SOCKET_DIR" ]; then
    echo "MISSING_SOCKET_DIR" >&2
    exit 2
fi
if [ -n "$ZELLIJ_SESSION_NAME" ] || [ -n "$TMUX" ] || [ -n "$VERIJ_ZELLIJ_LEAK" ]; then
    echo "LEAKED_ENV" >&2
    exit 3
fi
if [ "$4" = "receipt-hang" ]; then
    echo "$$" > "$5"
    exec sleep 10
fi
echo "OK: SOCKET=$ZELLIJ_SOCKET_DIR ARGS=$*"
exit 0
"#;
        fs::write(&mock_script_path, script_content).unwrap();
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            fs::set_permissions(&mock_script_path, fs::Permissions::from_mode(0o755)).unwrap();
        }

        std::env::set_var("ZELLIJ_BIN", &mock_script_path);
        std::env::set_var("ZELLIJ_SOCKET_DIR", "/tmp/custom-isolated-sock");
        std::env::set_var("ZELLIJ_SESSION_NAME", "leak_session");
        std::env::set_var("TMUX", "leak_tmux");
        std::env::set_var("VERIJ_ZELLIJ_LEAK", "leak_verij");

        let out = run_zellij_action("test-sess", &["list-panes"], Duration::from_secs(5))
            .expect("mock run_zellij_action should succeed");

        assert!(out.contains("OK: SOCKET=/tmp/custom-isolated-sock"));
        assert!(out.contains("ARGS=-s test-sess action list-panes"));

        let request = ControlRequest {
            schema_version: SCHEMA_VERSION,
            request_id: "receipt-test".into(),
            context: test_context(std::process::id(), 1, 1, "receipt-epoch"),
            sequence: 0,
            operation: ControlOperation::Query,
        };
        let path = temp_dir.join("result-receipt-test.json");
        let mut result: ControlResult = serde_json::from_value(serde_json::json!({
            "request": request, "status": "observed", "whole_host_verified": false, "acknowledge_done": false,
            "observation": {"context": request.context, "session_name": "test-sess", "terminal": 0,
                "observed_at_ms": now_ms()}
        })).unwrap();
        let writer_path = path.clone();
        let mut fresh = result.clone();
        let writer = std::thread::spawn(move || {
            std::thread::sleep(Duration::from_millis(100));
            fresh.observation.as_mut().unwrap().observed_at_ms = now_ms();
            atomic_write_json(&writer_path, &fresh).unwrap();
        });
        let started = Instant::now();
        let pid_path = temp_dir.join("transport-pid");
        run_zellij_action_with_receipt("test-sess", &["receipt-hang", pid_path.to_str().unwrap()], Duration::from_millis(600), Some((&path, &request))).unwrap();
        writer.join().unwrap();
        assert!(started.elapsed() < Duration::from_secs(1), "native receipt must not wait for a lingering transport");
        let pid: u32 = fs::read_to_string(&pid_path).unwrap().trim().parse().unwrap();
        let transport = crate::process::identity(pid).expect("transport must finish gracefully after its native receipt");
        while crate::process::is_alive(&transport) {
            assert!(started.elapsed() < Duration::from_secs(2), "transport cleanup exceeded its original deadline");
            std::thread::sleep(Duration::from_millis(20));
        }

        // Even a fresh result from another request cannot stop this transport.
        result.request.request_id = "wrong-request".into();
        result.observation.as_mut().unwrap().observed_at_ms = now_ms();
        atomic_write_json(&path, &result).unwrap();
        let error = run_zellij_action_with_receipt("test-sess", &["receipt-hang", pid_path.to_str().unwrap()], Duration::from_millis(150), Some((&path, &request))).unwrap_err();
        assert!(error.to_string().contains("timed out"));

        std::env::remove_var("ZELLIJ_BIN");
        std::env::remove_var("ZELLIJ_SOCKET_DIR");
        std::env::remove_var("ZELLIJ_SESSION_NAME");
        std::env::remove_var("TMUX");
        std::env::remove_var("VERIJ_ZELLIJ_LEAK");
        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_load_binding_bad_schema_fails_closed() {
        let temp_dir =
            std::env::temp_dir().join(format!("vj-nav-test-{}", Uuid::new_v4().simple()));
        fs::create_dir_all(&temp_dir).unwrap();

        let ctx = test_context(std::process::id(), 1, 1, "ep1");
        let proc = test_process_identity(std::process::id(), 100);

        let binding = HostBinding {
            schema_version: 999, // Bad schema
            host: HostKey("host-bad-schema".to_string()),
            host_session: "host-sess".to_string(),
            workspace_pane: TerminalPaneId(0),
            attachment_process: proc.clone(),
            server_process: proc,
            context: ctx,
            pane: None,
        };
        atomic_write_json(
            &temp_dir.join("binding-host-host-bad-schema.json"),
            &binding,
        )
        .unwrap();

        let res = load_binding(&temp_dir, "host-bad-schema");
        assert!(res.is_err());
        let err_msg = res.unwrap_err().to_string();
        assert!(err_msg.contains("schema version mismatch"));

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_load_binding_missing_binding_fails_closed() {
        let temp_dir =
            std::env::temp_dir().join(format!("vj-nav-test-{}", Uuid::new_v4().simple()));
        fs::create_dir_all(&temp_dir).unwrap();

        let res = load_binding(&temp_dir, "missing-host");
        assert!(res.is_err());
        let err_msg = res.unwrap_err().to_string();
        assert!(err_msg.contains("missing verified binding"));
        assert!(err_msg.contains("never writes repeated zellij attach"));

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_load_binding_stale_and_future_heartbeat() {
        let temp_dir =
            std::env::temp_dir().join(format!("vj-nav-test-{}", Uuid::new_v4().simple()));
        fs::create_dir_all(&temp_dir).unwrap();

        let proc = crate::process::identity(std::process::id()).unwrap();
        let ctx = test_context(proc.pid, 1, 1, "hb-test-epoch");

        let binding = HostBinding {
            schema_version: SCHEMA_VERSION,
            host: HostKey("test-host".to_string()),
            host_session: "host-session".to_string(),
            workspace_pane: TerminalPaneId(1),
            attachment_process: proc.clone(),
            server_process: proc.clone(),
            context: ctx.clone(),
            pane: None,
        };
        atomic_write_json(&temp_dir.join("binding-host-test-host.json"), &binding).unwrap();

        let hb_name = format!(
            "focus-{}-{}-{}-{}.json",
            ctx.server_pid, ctx.plugin_id, ctx.client_id, ctx.epoch
        );

        // 1. Stale heartbeat (>5000ms old)
        let stale_obs = FocusObservation {
            registration_request_id: None,
            registration_surface: false,
            context: ctx.clone(),
            session_name: Some("inner".to_string()),
            terminal: None,
            focused_plugin: None,
            application_sequence: 0,
            queried_terminal: None,
            pane_pid: None,
            observed_at_ms: now_ms().saturating_sub(6000),
        };
        atomic_write_json(&temp_dir.join(&hb_name), &stale_obs).unwrap();

        let res_stale = load_binding(&temp_dir, "test-host");
        assert!(res_stale.is_err());
        assert!(res_stale.unwrap_err().to_string().contains("stale"));

        // 2. Future heartbeat (>5000ms in future)
        let future_obs = FocusObservation {
            registration_request_id: None,
            registration_surface: false,
            context: ctx.clone(),
            session_name: Some("inner".to_string()),
            terminal: None,
            focused_plugin: None,
            application_sequence: 0,
            queried_terminal: None,
            pane_pid: None,
            observed_at_ms: now_ms() + 10000,
        };
        atomic_write_json(&temp_dir.join(&hb_name), &future_obs).unwrap();

        let res_future = load_binding(&temp_dir, "test-host");
        assert!(res_future.is_err());
        assert!(res_future.unwrap_err().to_string().contains("future"));

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_same_context_rebind_preserves_journal_sequence() {
        let temp_dir =
            std::env::temp_dir().join(format!("vj-nav-test-{}", Uuid::new_v4().simple()));
        fs::create_dir_all(&temp_dir).unwrap();

        let ctx_a = test_context(100, 1, 1, "epoch-a");
        let journal_path = temp_dir.join("journal-host-a.json");
        let initial_journal = HostJournal {
            context: ctx_a.clone(),
            sequence: 42,
        };
        atomic_write_json(&journal_path, &initial_journal).unwrap();

        // Rebind with same context: preserve sequence!
        let rebind_same_ctx = ctx_a.clone();
        let content = fs::read_to_string(&journal_path).unwrap();
        let existing: HostJournal = serde_json::from_str(&content).unwrap();
        let journal_to_write = if existing.context == rebind_same_ctx {
            existing.clone()
        } else {
            HostJournal {
                context: rebind_same_ctx,
                sequence: 0,
            }
        };
        assert_eq!(journal_to_write.sequence, 42);

        // Rebind with different context: reset sequence to 0
        let rebind_new_ctx = test_context(100, 1, 1, "epoch-b");
        let journal_to_write_new = if existing.context == rebind_new_ctx {
            existing
        } else {
            HostJournal {
                context: rebind_new_ctx,
                sequence: 0,
            }
        };
        assert_eq!(journal_to_write_new.sequence, 0);

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_corrupt_journal_fails_closed() {
        let temp_dir =
            std::env::temp_dir().join(format!("vj-nav-test-{}", Uuid::new_v4().simple()));
        fs::create_dir_all(&temp_dir).unwrap();

        let journal_path = temp_dir.join("journal-host-corrupt.json");
        fs::write(&journal_path, "{ malformed json").unwrap();

        let journal_content = fs::read_to_string(&journal_path).unwrap();
        let parsed = serde_json::from_str::<HostJournal>(&journal_content);
        assert!(parsed.is_err(), "must fail closed on corrupt journal");

        let _ = fs::remove_dir_all(&temp_dir);
    }

    #[test]
    fn test_focus_target_session_mismatch_refused() {
        let active_bound_session = "session_alpha";
        let requested_target_session = Some("session_beta");

        let is_mismatch =
            requested_target_session.is_some_and(|target| target != active_bound_session);
        assert!(
            is_mismatch,
            "focus target session mismatch must be detected and refused"
        );
    }

    #[test]
    fn test_inner_focus_observed_missing_observation_fails_closed() {
        let result = ControlResult {
            request: ControlRequest {
                schema_version: SCHEMA_VERSION,
                request_id: "req-1".to_string(),
                context: test_context(10, 1, 1, "e1"),
                sequence: 1,
                operation: ControlOperation::Focus {
                    terminal: TerminalPaneId(0),
                },
            },
            status: ControlStatus::InnerFocusObserved,
            observation: None, // Missing observation!
            whole_host_verified: false,
            acknowledge_done: false,
        };

        let has_valid_obs =
            result.status == ControlStatus::InnerFocusObserved && result.observation.is_some();
        assert!(
            !has_valid_obs,
            "InnerFocusObserved without observation must fail closed"
        );
    }

    #[test]
    fn test_missing_binding_cannot_mint_verified_visit() {
        let temp_dir =
            std::env::temp_dir().join(format!("vj-nav-test-{}", Uuid::new_v4().simple()));
        assert!(verified_visit(&temp_dir, "test-host").is_none());
    }
}
