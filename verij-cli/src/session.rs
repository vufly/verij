use anyhow::{bail, Context, Result};
use std::collections::BTreeMap;
use std::ffi::OsStr;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Output, Stdio};
use std::sync::{mpsc, Mutex};
use std::time::{Duration, Instant, SystemTime};

#[cfg(unix)]
use std::os::unix::process::CommandExt;

// ---------------------------------------------------------------------------
// Session queries
// ---------------------------------------------------------------------------

/// List all active Zellij session names.
pub fn list_sessions() -> Result<Vec<String>> {
    let output = query_zellij(Command::new("zellij").args(["list-sessions", "-s", "-n"]))?;

    if !output.status.success() {
        return Ok(Vec::new());
    }

    let stdout = String::from_utf8_lossy(&output.stdout);
    let sessions = stdout
        .lines()
        .map(|l| l.trim().to_string())
        .filter(|l| !l.is_empty())
        .collect();

    Ok(sessions)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionStatus {
    Live,
    Exited,
    Missing,
}

/// Returns whether Zellij knows a session as live, exited/resurrectable, or missing.
pub fn session_status(name: &str) -> Result<SessionStatus> {
    let output = query_zellij(Command::new("zellij").args(["list-sessions", "-n"]))?;

    if !output.status.success() {
        return Ok(SessionStatus::Missing);
    }

    Ok(parse_session_status(
        &String::from_utf8_lossy(&output.stdout),
        name,
    ))
}

/// Management commands must distinguish an empty inventory from a failed query.
pub fn checked_session_statuses() -> Result<BTreeMap<String, SessionStatus>> {
    let output = query_zellij(Command::new("zellij").args(["list-sessions", "-n"]))?;

    // Zellij versions differ in where they print the empty-inventory message
    // and whether they return a successful exit code for it.
    if String::from_utf8_lossy(&output.stdout).trim() == "No active zellij sessions found."
        || String::from_utf8_lossy(&output.stderr).trim() == "No active zellij sessions found."
    {
        return Ok(BTreeMap::new());
    }
    if !output.status.success() {
        let stderr = String::from_utf8_lossy(&output.stderr);
        bail!("'zellij list-sessions' failed: {}", stderr.trim());
    }

    Ok(parse_session_statuses(&String::from_utf8_lossy(
        &output.stdout,
    )))
}

fn parse_session_statuses(output: &str) -> BTreeMap<String, SessionStatus> {
    if output.trim() == "No active zellij sessions found." {
        return BTreeMap::new();
    }
    output
        .lines()
        .filter_map(|line| {
            let line = line.trim();
            let name = line.split_whitespace().next()?;
            let status = if line.contains("(EXITED") {
                SessionStatus::Exited
            } else {
                SessionStatus::Live
            };
            Some((name.to_string(), status))
        })
        .collect()
}

pub fn is_verij_host(name: &str) -> Result<bool> {
    if matches!(session_status(name)?, SessionStatus::Live) {
        if let Some(is_host) = live_host_panes(name)? {
            return Ok(is_host);
        }
    }
    let Some(layout) = resurrection_layout_path(name) else {
        return Ok(false);
    };
    Ok(crate::registry::is_host_layout(&layout))
}

/// Deleting a live session requires live pane evidence: an old serialized
/// layout cannot establish the identity of a new session that reused its name.
pub fn is_verij_host_for_deletion(name: &str) -> Result<bool> {
    if matches!(session_status(name)?, SessionStatus::Live) {
        return Ok(live_host_panes(name)?.unwrap_or(false));
    }
    is_verij_host(name)
}

fn live_host_panes(name: &str) -> Result<Option<bool>> {
    let output = query_zellij(Command::new("zellij")
        .args(["--session", name, "action", "list-panes", "--all", "--json"]))?;
    if !output.status.success() {
        return Ok(None);
    }
    let panes: Vec<serde_json::Value> = serde_json::from_slice(&output.stdout)?;
    Ok(Some(panes.iter().any(|pane| {
        pane.get("pane_command")
            .or_else(|| pane.get("terminal_command"))
            .and_then(|v| v.as_str())
            .is_some_and(|cmd| cmd.contains("verij ui"))
    })))
}

pub fn rename_host(old: &str, new: &str) -> Result<()> {
    if old == new {
        return Ok(());
    }
    crate::registry::validate_name(new)?;
    if !matches!(session_status(new)?, SessionStatus::Missing) {
        bail!("Zellij session '{new}' already exists");
    }
    if crate::registry::marker_key(old)?.is_none() {
        bail!("Host '{old}' is not registered");
    }
    if crate::registry::marker_key(new)?.is_some()
        || crate::config::load_hosts().hosts.contains_key(new)
    {
        bail!("Host '{new}' already has registered state");
    }
    if !matches!(session_status(old)?, SessionStatus::Live) {
        bail!("Host '{old}' must be live to rename; attach it first");
    }
    zellij_action(&["--session", old, "action", "rename-session", new])?;
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline {
        if matches!(session_status(new)?, SessionStatus::Live) {
            break;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    if !matches!(session_status(new)?, SessionStatus::Live) {
        bail!("Zellij did not report renamed host '{new}'");
    }
    if let Err(error) = crate::registry::rename(old, new) {
        let _ = zellij_action(&["--session", new, "action", "rename-session", old]);
        return Err(error);
    }
    std::env::set_var("ZELLIJ_SESSION_NAME", new);
    std::env::set_var("VERIJ_HOST_NAME", new);
    Ok(())
}

// ---------------------------------------------------------------------------
// Process execution
// ---------------------------------------------------------------------------

const QUERY_TIMEOUT: Duration = Duration::from_secs(3);
const CLIENT_EXIT_TIMEOUT: Duration = Duration::from_secs(2);

// Zellij 0.45.1 panics if a socket probe disconnects before FirstClientConnected.
// Inventory queries probe every socket, including sessions still starting.
static ZELLIJ_STARTUP: Mutex<()> = Mutex::new(());

pub(crate) struct StartupGuard {
    #[cfg(unix)]
    _file: std::fs::File,
    _local: std::sync::MutexGuard<'static, ()>,
}

fn startup_guard() -> Result<StartupGuard> {
    startup_guard_until(Instant::now() + Duration::from_secs(15), None)
}

/// Monitoring queries share startup exclusion without sacrificing their own
/// deadline or stoppable watcher shutdown while a temporary client initializes.
pub(crate) fn startup_guard_until(
    deadline: Instant,
    stop: Option<&std::sync::atomic::AtomicBool>,
) -> Result<StartupGuard> {
    let check = || -> Result<()> {
        if stop.is_some_and(|stop| stop.load(std::sync::atomic::Ordering::Relaxed)) {
            bail!("Zellij query cancelled while waiting for startup");
        }
        if Instant::now() >= deadline {
            bail!("Zellij startup lock deadline expired");
        }
        Ok(())
    };
    let local = loop {
        check()?;
        match ZELLIJ_STARTUP.try_lock() {
            Ok(guard) => break guard,
            Err(std::sync::TryLockError::Poisoned(error)) => break error.into_inner(),
            Err(std::sync::TryLockError::WouldBlock) => {
                std::thread::sleep(Duration::from_millis(5));
            }
        }
    };
    #[cfg(unix)]
    {
        use std::os::fd::AsRawFd;
        // Other Verij host sidebars also poll the complete socket inventory.
        let runtime = crate::config::runtime_dir();
        std::fs::create_dir_all(&runtime)?;
        let file = std::fs::OpenOptions::new().read(true).write(true).create(true).truncate(false)
            .open(runtime.join("zellij-startup.lock"))?;
        loop {
            check()?;
            if unsafe { libc::flock(file.as_raw_fd(), libc::LOCK_EX | libc::LOCK_NB) } == 0 {
                break;
            }
            let error = std::io::Error::last_os_error();
            if !matches!(error.kind(), std::io::ErrorKind::Interrupted | std::io::ErrorKind::WouldBlock) {
                return Err(error.into());
            }
            std::thread::sleep(Duration::from_millis(5));
        }
        Ok(StartupGuard { _file: file, _local: local })
    }
    #[cfg(not(unix))]
    Ok(StartupGuard { _local: local })
}

fn wait_until(child: &mut Child, deadline: Instant) -> std::io::Result<Option<ExitStatus>> {
    loop {
        if let Some(status) = child.try_wait()? {
            return Ok(Some(status));
        }
        if Instant::now() >= deadline {
            return Ok(None);
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

/// Drain both pipes concurrently: large pane inventories can fill stdout before
/// Zellij exits. The deadline covers process exit and inherited pipe handles.
pub(crate) fn query_zellij(command: &mut Command) -> Result<Output> {
    let _startup = startup_guard()?;
    let description = format!("{command:?}");
    let deadline = Instant::now() + QUERY_TIMEOUT;
    let mut child = command
        .stdin(Stdio::null())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .with_context(|| format!("Failed to execute {description}"))?;
    let stdout = child.stdout.take().unwrap();
    let stderr = child.stderr.take().unwrap();
    let (send, receive) = mpsc::channel();
    fn drain(
        mut pipe: impl Read + Send + 'static,
        send: mpsc::Sender<(bool, std::io::Result<Vec<u8>>)>,
        is_stdout: bool,
    ) {
        std::thread::spawn(move || {
            let mut bytes = Vec::new();
            let result = pipe.read_to_end(&mut bytes).map(|_| bytes);
            let _ = send.send((is_stdout, result));
        });
    }
    drain(stdout, send.clone(), true);
    drain(stderr, send, false);
    let status = match wait_until(&mut child, deadline) {
        Ok(Some(status)) => status,
        result => {
            let _ = child.kill();
            let _ = child.wait();
            result?;
            bail!(
                "Zellij command timed out after {}s: {description}",
                QUERY_TIMEOUT.as_secs()
            );
        }
    };
    let mut output = Output {
        status,
        stdout: Vec::new(),
        stderr: Vec::new(),
    };
    for _ in 0..2 {
        let (is_stdout, bytes) = receive
            .recv_timeout(deadline.saturating_duration_since(Instant::now()))
            .with_context(|| format!("Timed out reading Zellij command output: {description}"))?;
        if is_stdout {
            output.stdout = bytes?;
        } else {
            output.stderr = bytes?;
        }
    }
    Ok(output)
}

/// Sidebar actions must never inherit stdout/stderr: any printed diagnostic
/// changes terminal cells and cursor behind Ratatui's back. Return failures so
/// the event loop can render them in its status bar instead.
pub fn zellij_action(args: &[&str]) -> Result<()> {
    let output = zellij_output(args)?;
    if !output.status.success() {
        let diagnostic = [
            String::from_utf8_lossy(&output.stderr).trim().to_string(),
            String::from_utf8_lossy(&output.stdout).trim().to_string(),
        ].into_iter().filter(|text| !text.is_empty()).collect::<Vec<_>>().join("; ");
        bail!(
            "Zellij {} failed ({}): {}",
            args.join(" "),
            output.status,
            diagnostic,
        );
    }
    Ok(())
}

pub fn zellij_output(args: &[&str]) -> Result<Output> {
    query_zellij(Command::new("zellij").args(args))
}

/// Own the temporary PTY client even if readiness checks return an error.
/// util-linux script forwards SIGTERM to its client and reaps it; use SIGKILL
/// only as a final fallback when script itself fails to exit.
pub(crate) struct TemporaryClient(Child);

impl TemporaryClient {
    /// Wait for freshly published session metadata before allowing CLI probes.
    /// A socket existing or answering ConnStatus does not mean it is initialized.
    pub(crate) fn start_zellij(session_name: &str, command: &str) -> Result<Self> {
        let _startup = startup_guard()?;
        let path = session_cache_dir(session_name)
            .context("Cannot locate Zellij session metadata")?
            .join("session-metadata.kdl");
        let previous_modified = std::fs::metadata(&path).and_then(|metadata| metadata.modified()).ok();
        let mut client = Self::spawn(command)?;
        client.wait_for_publication(session_name, &path, previous_modified)?;
        Ok(client)
    }

    fn wait_for_publication(&mut self, session_name: &str, path: &Path, previous_modified: Option<SystemTime>) -> Result<()> {
        let deadline = Instant::now() + Duration::from_secs(10);
        while Instant::now() < deadline {
            if let Some(status) = self.try_wait()? {
                bail!("Temporary client for '{session_name}' exited before its layout was ready: {status}");
            }
            if session_metadata_published(path, session_name, previous_modified) {
                return Ok(());
            }
            std::thread::sleep(Duration::from_millis(50));
        }
        bail!("Zellij session '{session_name}' did not publish initialized session metadata within 10s");
    }

    pub(crate) fn spawn(command: &str) -> Result<Self> {
        // script's stdin is /dev/null, so it cannot inherit a viewport.
        let (columns, rows) = crossterm::terminal::size().ok()
            .filter(|(columns, rows)| *columns > 0 && *rows > 0)
            .unwrap_or((120, 40));
        let command = format!("stty rows {rows} cols {columns}; exec {command}");
        let mut script = Command::new("script");
        script
            .args(["-q", "-e", "-c", &command, "/dev/null"])
            .env_remove("ZELLIJ")
            .env_remove("ZELLIJ_SESSION_NAME")
            .env_remove("ZELLIJ_PANE_ID")
            .stdin(Stdio::null())
            .stdout(Stdio::null())
            .stderr(Stdio::null());
        // Cleanup must not signal the sidebar's own process group.
        #[cfg(unix)]
        script.process_group(0);
        Ok(Self(script.spawn().context("Failed to start temporary PTY client")?))
    }

    pub(crate) fn try_wait(&mut self) -> std::io::Result<Option<ExitStatus>> {
        self.0.try_wait()
    }

    pub(crate) fn finish(&mut self) -> Result<()> {
        if wait_until(&mut self.0, Instant::now() + CLIENT_EXIT_TIMEOUT)?.is_some() {
            return Ok(());
        }
        #[cfg(unix)]
        unsafe {
            libc::kill(self.0.id() as libc::pid_t, libc::SIGTERM);
        }
        #[cfg(not(unix))]
        self.0.kill()?;
        if wait_until(&mut self.0, Instant::now() + Duration::from_secs(1))?.is_none() {
            self.0.kill()?;
            self.0.wait()?;
        }
        Ok(())
    }
}

impl Drop for TemporaryClient {
    fn drop(&mut self) {
        let _ = self.finish();
    }
}

/// Replaces the current process with Zellij, or spawns and waits if exec is unavailable.
pub fn exec_zellij<I, S>(args: I) -> Result<()>
where
    I: IntoIterator<Item = S>,
    S: AsRef<OsStr>,
{
    let mut cmd = std::process::Command::new("zellij");
    cmd.args(args);

    #[cfg(unix)]
    {
        let err = cmd.exec();
        bail!("Failed to exec zellij: {}", err);
    }

    #[cfg(not(unix))]
    {
        let mut child = cmd.spawn().context("Failed to spawn zellij")?;
        let status = child.wait().context("Failed to wait for zellij")?;
        if !status.success() {
            std::process::exit(status.code().unwrap_or(1));
        }
        Ok(())
    }
}

// ---------------------------------------------------------------------------
// High-level session actions
// ---------------------------------------------------------------------------

/// Launches the Verij host session.
///
/// If the session is already running and `no_attach` is false, automatically attaches.
pub fn start_host_session(
    session_name: &str,
    layout_path: &Path,
    no_attach: bool,
    host_config_path: &Path,
) -> Result<()> {
    match session_status(session_name)? {
        SessionStatus::Live | SessionStatus::Exited => {
            if no_attach {
                bail!(
                    "Session '{}' already exists. Specify a different name with --session-name or attach with 'verij attach'.",
                    session_name
                );
            }

            eprintln!(
                "Session '{}' already exists. Attaching to it...",
                session_name
            );
            return attach_session(session_name, host_config_path);
        }
        SessionStatus::Missing => {}
    }

    // Use -n (--new-session-with-layout) with -s to always create and attach to a new named session
    // with the given layout. In Zellij CLI, passing -l with -s treats it as adding tabs to an
    // existing session, which fails if the session does not already exist.
    exec_zellij(host_start_args(session_name, layout_path, host_config_path))
}

fn host_start_args(session_name: &str, layout_path: &Path, host_config_path: &Path) -> Vec<String> {
    vec![
        "--config".to_string(),
        host_config_path.to_string_lossy().into_owned(),
        "-s".to_string(),
        session_name.to_string(),
        "-n".to_string(),
        layout_path.to_string_lossy().into_owned(),
    ]
}

/// Attaches to an existing host session.
pub fn attach_session(session_name: &str, host_config_path: &Path) -> Result<()> {
    if matches!(session_status(session_name)?, SessionStatus::Exited) {
        let last_session = crate::config::last_host_session(session_name);
        prepare_resurrection_layout(session_name)?;
        prepare_host_workspace_layout(session_name, last_session.as_deref())?;
    }
    exec_zellij(host_attach_args(session_name, host_config_path))
}

fn host_attach_args(session_name: &str, host_config_path: &Path) -> Vec<String> {
    vec![
        "--config".to_string(),
        host_config_path.to_string_lossy().into_owned(),
        "attach".to_string(),
        "--force-run-commands".to_string(),
        session_name.to_string(),
    ]
}

/// Resurrects an exited session through a short-lived fake PTY, then detaches it.
/// This makes its panes and plugins available before a host Workspace pane attaches.
pub fn resurrect_session(session_name: &str) -> Result<()> {
    if !matches!(session_status(session_name)?, SessionStatus::Exited) {
        return Ok(());
    }

    prepare_resurrection_layout(session_name)?;
    let expects_zjstatus = resurrection_layout_path(session_name)
        .and_then(|path| std::fs::read_to_string(path).ok())
        .is_some_and(|layout| layout.contains("zjstatus"));

    let mut fake_client = TemporaryClient::start_zellij(session_name, &resurrection_command(session_name))
        .with_context(|| format!("Failed to resurrect session '{session_name}'"))?;

    // The first status pane can appear before the remaining serialized tabs
    // restore. Detaching then leaves those tabs without their status plugin.
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut stable_since = None;
    let mut layout_ready = false;
    while Instant::now() < deadline {
        if let Some(status) = fake_client.try_wait()? {
            bail!("Resurrection client for '{session_name}' exited before its layout was ready: {status}");
        }
        let ready = matches!(session_status(session_name)?, SessionStatus::Live)
            && (!expects_zjstatus || session_tabs_have_tiled_plugin(session_name)?);
        if ready {
            let since = stable_since.get_or_insert_with(Instant::now);
            if !expects_zjstatus || since.elapsed() >= Duration::from_millis(1200) {
                layout_ready = true;
                break;
            }
        } else {
            stable_since = None;
        }
        std::thread::sleep(Duration::from_millis(50));
    }

    let detach_status = query_zellij(Command::new("zellij")
        .args(["-s", session_name, "action", "detach"]))?.status;
    fake_client.finish()?;

    if !detach_status.success() {
        bail!("Failed to detach resurrection client for '{session_name}'");
    }
    if !matches!(session_status(session_name)?, SessionStatus::Live) {
        bail!("Session '{session_name}' did not become live after resurrection");
    }
    if !layout_ready {
        bail!("Session '{session_name}' did not restore zjstatus on every tab");
    }

    Ok(())
}

/// Resurrects the last inner session remembered by a host, when it still exists.
pub fn restore_last_inner_session(host_session: &str) -> Result<()> {
    let Some(inner_session) = crate::config::last_host_session(host_session) else {
        return Ok(());
    };

    resurrect_session(&inner_session)
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\\', "\\\\").replace('\'', "'\\''"))
}

fn resurrection_command(session_name: &str) -> String {
    format!(
        "zellij attach --force-run-commands {}",
        shell_quote(session_name)
    )
}

/// Removes Zellij's serialized `start_suspended` state before resurrection.
/// Zellij 0.45.1 can retain this flag even with `--force-run-commands`.
pub fn prepare_resurrection_layout(session_name: &str) -> Result<()> {
    let Some(path) = resurrection_layout_path(session_name) else {
        return Ok(());
    };
    let content = std::fs::read_to_string(&path)?;
    let updated = content.replace("start_suspended true", "start_suspended false");
    if updated == content {
        return Ok(());
    }

    let temporary = path.with_extension(format!("kdl.tmp-{}", std::process::id()));
    std::fs::write(&temporary, updated)?;
    std::fs::rename(&temporary, path)?;
    Ok(())
}

/// Rewrites a resurrected host's stale nested attach command to its durable target.
pub fn prepare_host_workspace_layout(
    host_session: &str,
    target_session: Option<&str>,
) -> Result<()> {
    let Some(target_session) = target_session else {
        return Ok(());
    };
    let Some(path) = resurrection_layout_path(host_session) else {
        return Ok(());
    };
    let content = std::fs::read_to_string(&path)?;
    let updated = rewrite_host_workspace_attach(&content, target_session);
    if updated == content {
        return Ok(());
    }

    let temporary = path.with_extension(format!("kdl.tmp-{}", std::process::id()));
    std::fs::write(&temporary, updated)?;
    std::fs::rename(&temporary, path)?;
    Ok(())
}

fn session_cache_dir(session_name: &str) -> Option<PathBuf> {
    let cache_home = std::env::var_os("XDG_CACHE_HOME")
        .map(PathBuf::from)
        .or_else(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join(".cache")))?;
    Some(cache_home
        .join("zellij/contract_version_1/session_info")
        .join(session_name))
}

fn session_metadata_published(path: &Path, session_name: &str, previous_modified: Option<SystemTime>) -> bool {
    let Ok(modified) = std::fs::metadata(path).and_then(|metadata| metadata.modified()) else {
        return false;
    };
    if Some(modified) == previous_modified {
        return false;
    }
    let Ok(source) = std::fs::read_to_string(path) else { return false; };
    let Ok(document) = source.parse::<kdl::KdlDocument>() else { return false; };
    let name = document.get("name")
        .and_then(|node| node.get(0))
        .and_then(|entry| entry.value().as_string());
    let has_tabs = document.get("tabs")
        .and_then(|node| node.children())
        .is_some_and(|tabs| tabs.nodes().iter().any(|node| node.name().value() == "tab"));
    name == Some(session_name) && has_tabs
}

fn resurrection_layout_path(session_name: &str) -> Option<PathBuf> {
    let path = session_cache_dir(session_name)?.join("session-layout.kdl");
    path.exists().then_some(path)
}

fn session_tabs_have_tiled_plugin(session_name: &str) -> Result<bool> {
    let output = query_zellij(Command::new("zellij")
        .args([
            "--session",
            session_name,
            "action",
            "list-panes",
            "--all",
            "--json",
        ]))?;

    if !output.status.success() {
        return Ok(false);
    }

    let Ok(panes) = serde_json::from_slice::<Vec<serde_json::Value>>(&output.stdout) else {
        return Ok(false);
    };

    Ok(terminal_tabs_have_tiled_plugin(&panes))
}

fn terminal_tabs_have_tiled_plugin(panes: &[serde_json::Value]) -> bool {
    let terminal_tabs = panes
        .iter()
        .filter(|pane| pane.get("is_plugin").and_then(|value| value.as_bool()) == Some(false))
        .filter_map(|pane| pane.get("tab_position").and_then(|value| value.as_u64()))
        .collect::<std::collections::BTreeSet<_>>();

    !terminal_tabs.is_empty()
        && terminal_tabs.iter().all(|tab_position| {
            panes.iter().any(|pane| {
                pane.get("is_plugin").and_then(|value| value.as_bool()) == Some(true)
                    && pane.get("tab_position").and_then(|value| value.as_u64())
                        == Some(*tab_position)
                    && pane.get("is_suppressed").and_then(|value| value.as_bool()) == Some(false)
                    && pane.get("is_floating").and_then(|value| value.as_bool()) == Some(false)
            })
        })
}

fn rewrite_host_workspace_attach(layout: &str, target_session: &str) -> String {
    let target = kdl_quote(target_session);
    let mut in_workspace_pane = false;
    let mut rewritten = Vec::new();

    for line in layout.lines() {
        let trimmed = line.trim_start();
        if trimmed.starts_with("pane command=\"zellij\"") {
            in_workspace_pane = true;
        }

        if in_workspace_pane && trimmed.starts_with("args \"attach\"") {
            let indent = &line[..line.len() - trimmed.len()];
            rewritten.push(format!(
                "{indent}args \"attach\" \"--force-run-commands\" {target}"
            ));
            continue;
        }

        rewritten.push(line.to_string());
        if in_workspace_pane && trimmed == "}" {
            in_workspace_pane = false;
        }
    }

    let mut result = rewritten.join("\n");
    if layout.ends_with('\n') {
        result.push('\n');
    }
    result
}

fn kdl_quote(value: &str) -> String {
    format!("\"{}\"", value.replace('\\', "\\\\").replace('"', "\\\""))
}

fn parse_session_status(output: &str, name: &str) -> SessionStatus {
    let prefix = format!("{name} ");
    output
        .lines()
        .map(str::trim)
        .find(|line| *line == name || line.starts_with(&prefix))
        .map(|line| {
            if line.contains("(EXITED") {
                SessionStatus::Exited
            } else {
                SessionStatus::Live
            }
        })
        .unwrap_or(SessionStatus::Missing)
}

#[cfg(test)]
mod tests {
    use super::{
        host_attach_args, host_start_args, parse_session_status, parse_session_statuses,
        resurrection_command, terminal_tabs_have_tiled_plugin, SessionStatus,
    };
    use serde_json::json;
    use std::path::Path;

    #[test]
    fn monitoring_startup_lock_waits_respect_cancellation_and_deadline() {
        use std::sync::{atomic::{AtomicBool, Ordering}, Arc};
        let guard = super::startup_guard().unwrap();
        let stop = Arc::new(AtomicBool::new(false));
        let child_stop = stop.clone();
        let waiting = std::thread::spawn(move || {
            let started = std::time::Instant::now();
            let error = super::startup_guard_until(started + std::time::Duration::from_secs(2), Some(&child_stop))
                .err().expect("startup exclusion must block another query");
            (started.elapsed(), error.to_string())
        });
        std::thread::sleep(std::time::Duration::from_millis(20));
        stop.store(true, Ordering::Relaxed);
        let (elapsed, error) = waiting.join().unwrap();
        assert!(elapsed < std::time::Duration::from_millis(250), "watcher cancellation blocked on startup");
        assert!(error.contains("cancelled"));
        let waiting = std::thread::spawn(|| {
            super::startup_guard_until(std::time::Instant::now() + std::time::Duration::from_millis(30), None)
                .err().expect("startup wait must respect its query deadline").to_string()
        });
        assert!(waiting.join().unwrap().contains("deadline"));
        drop(guard);
        assert!(super::startup_guard().is_ok());
    }

    #[test]
    fn startup_publication_requires_fresh_valid_metadata_for_target_with_tabs() {
        let dir = std::env::temp_dir().join(format!("verij-startup-metadata-{}", std::process::id()));
        std::fs::create_dir_all(&dir).unwrap();
        let path = dir.join("session-metadata.kdl");
        for source in ["name \"inner\"\ntabs {", "name \"other\"\ntabs { tab; }", "name \"inner\"\ntabs {}"] {
            std::fs::write(&path, source).unwrap();
            assert!(!super::session_metadata_published(&path, "inner", None));
        }
        std::fs::write(&path, "name \"inner\"\ntabs { tab { position 0; }; }").unwrap();
        assert!(super::session_metadata_published(&path, "inner", None));
        let modified = std::fs::metadata(&path).unwrap().modified().unwrap();
        assert!(!super::session_metadata_published(&path, "inner", Some(modified)));
        std::fs::remove_dir_all(&dir).unwrap();
    }

    #[cfg(unix)]
    #[test]
    fn temporary_client_starts_with_sized_pty_and_without_host_identity() {
        let mut client = super::TemporaryClient::spawn(
            "sh -c 'test -z \"${ZELLIJ+x}${ZELLIJ_SESSION_NAME+x}${ZELLIJ_PANE_ID+x}\" || exit 1; set -- $(stty size); test \"$1\" -gt 0 && test \"$2\" -gt 0'",
        ).unwrap();
        let status = super::wait_until(&mut client.0, std::time::Instant::now() + std::time::Duration::from_secs(3))
            .unwrap().expect("temporary PTY command hung");
        assert!(status.success(), "temporary PTY had no viewport or inherited host identity: {status}");
    }

    #[cfg(unix)]
    #[test]
    fn temporary_client_cleanup_is_bounded_and_isolated_from_sidebar() {
        let mut client = super::TemporaryClient::spawn("sleep 60").unwrap();
        let pid = client.0.id() as libc::pid_t;
        assert_ne!(unsafe { libc::getpgid(pid) }, unsafe { libc::getpgrp() });
        let started = std::time::Instant::now();
        client.finish().unwrap();
        assert!(started.elapsed() < std::time::Duration::from_secs(4));
        assert_eq!(unsafe { libc::kill(pid, 0) }, -1, "temporary script process leaked");
        assert_eq!(std::io::Error::last_os_error().raw_os_error(), Some(libc::ESRCH));
    }

    #[cfg(unix)]
    #[test]
    fn captures_large_query_output_without_blocking_on_full_pipes() {
        let output = super::query_zellij(std::process::Command::new("sh").args([
            "-c",
            "i=0; while [ \"$i\" -lt 128 ]; do printf '%01024d' 0; printf '%01024d' 0 >&2; i=$((i+1)); done",
        ])).unwrap();
        assert!(output.status.success());
        assert_eq!(output.stdout.len(), 128 * 1024);
        assert_eq!(output.stderr.len(), 128 * 1024);
    }

    #[test]
    fn parses_live_exited_and_missing_sessions() {
        let output =
            "backend [Created 1m ago]\nold [Created 2m ago] (EXITED - attach to resurrect)\n";

        assert_eq!(parse_session_status(output, "backend"), SessionStatus::Live);
        assert_eq!(parse_session_status(output, "old"), SessionStatus::Exited);
        assert_eq!(
            parse_session_status(output, "other"),
            SessionStatus::Missing
        );
    }

    #[test]
    fn does_not_match_session_name_prefixes() {
        let output = "backend-old [Created 1m ago]\n";

        assert_eq!(
            parse_session_status(output, "backend"),
            SessionStatus::Missing
        );
    }

    #[test]
    fn inventory_includes_exited_sessions_and_exact_names() {
        let statuses = parse_session_statuses(
            "host [Created 1m ago]\nhost-old [Created 2m ago] (EXITED - attach to resurrect)\n",
        );
        assert_eq!(statuses.get("host"), Some(&SessionStatus::Live));
        assert_eq!(statuses.get("host-old"), Some(&SessionStatus::Exited));
        assert_eq!(statuses.get("missing"), None);
        assert!(parse_session_statuses("").is_empty());
        assert!(parse_session_statuses("No active zellij sessions found.\n").is_empty());
    }

    #[test]
    fn removes_serialized_suspended_state() {
        let layout = "pane command=\"watch\" start_suspended true\n";
        assert_eq!(
            layout.replace("start_suspended true", "start_suspended false"),
            "pane command=\"watch\" start_suspended false\n"
        );
    }

    #[test]
    fn rewrites_stale_host_workspace_attach() {
        let layout =
            "layout {\n    pane command=\"zellij\" {\n        args \"attach\" \"old\"\n    }\n}\n";

        assert_eq!(
            super::rewrite_host_workspace_attach(layout, "new"),
            "layout {\n    pane command=\"zellij\" {\n        args \"attach\" \"--force-run-commands\" \"new\"\n    }\n}\n"
        );
    }

    #[test]
    fn host_commands_use_generated_config() {
        let config = Path::new("/tmp/verij/host-config.kdl");
        assert_eq!(
            host_start_args("host", Path::new("/tmp/host.kdl"), config),
            vec![
                "--config",
                "/tmp/verij/host-config.kdl",
                "-s",
                "host",
                "-n",
                "/tmp/host.kdl",
            ]
        );
        assert_eq!(
            host_attach_args("host", config),
            vec!["--config", "/tmp/verij/host-config.kdl", "attach", "--force-run-commands", "host"]
        );
        assert_eq!(
            resurrection_command("inner"),
            "zellij attach --force-run-commands 'inner'"
        );
    }

    #[test]
    fn waits_for_tiled_plugin_on_every_terminal_tab() {
        let terminal = |tab_position| json!({"is_plugin": false, "tab_position": tab_position});
        let tiled_plugin = |tab_position| {
            json!({
                "is_plugin": true, "tab_position": tab_position,
                "is_suppressed": false, "is_floating": false
            })
        };
        let floating_plugin = json!({
            "is_plugin": true, "tab_position": 1,
            "is_suppressed": false, "is_floating": true
        });

        assert!(!terminal_tabs_have_tiled_plugin(&[
            terminal(0),
            tiled_plugin(0),
            terminal(1),
            floating_plugin,
        ]));
        assert!(terminal_tabs_have_tiled_plugin(&[
            terminal(0),
            tiled_plugin(0),
            terminal(1),
            tiled_plugin(1),
        ]));
    }
}
