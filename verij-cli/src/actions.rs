/// verij-cli/src/actions.rs
///
/// Action dispatcher: handles session and tab switching using "The Inception Switch".
///
/// Design rationale:
///   - Session switching: Injects a `switch:<target>` command into the
///     *currently active inner session* via `zellij -s <old_session> pipe --name verij_control`.
///     The `verij-plugin` agent running inside that session executes native
///     `switch_session(Some(target))`, switching the attached client cleanly
///     from the inside.
///   - Re-focus: Immediately executes `zellij action move-focus right` so keyboard
///     focus transfers to the attached inner session.
///   - Tab navigation: Directly executes `zellij --session <target> action go-to-tab <pos+1>`.
use anyhow::{Context, Result};
use serde::Deserialize;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::time::{Duration, Instant};
use verij_types::VERIJ_CONTROL_PIPE;

/// Creates a new inner session in the background, ensures the WASM agent is running,
/// and attaches/switches the Workspace pane to it.
pub fn create_inner_session(
    old_active_session: Option<&str>,
    new_session_name: &str,
    plugin_path: Option<&Path>,
    workspace_pane_name: &str,
) -> Result<()> {
    crate::registry::validate_name(new_session_name)?;
    if !matches!(crate::session::session_status(new_session_name)?, crate::session::SessionStatus::Missing) {
        anyhow::bail!("Zellij session '{new_session_name}' already exists");
    }
    // 1. Create the session with a fake PTY via `script` so zellij has a valid viewport.
    //
    // Zellij 0.45.x regression (zellij-org/zellij#5594): tabs created against a session
    // with no attached client have no viewport, so `default_tab_template` layout fails with
    // "Not enough room for panes" and the tab (+ zjstatus pane) is silently discarded when
    // the first real client attaches. Using -b (headless) triggers this every time.
    //
    // Fix: attach through `script -q -c "zellij attach -c <name>" /dev/null` which provides
    // a pseudo-terminal, giving zellij a viewport to size the first tab from. After the
    // layout settles we send `zellij action detach` to that fake client and wait for the
    // `script` process to exit cleanly before proceeding.
    let default_layout = configured_zellij_layout();

    // The inner session inherits Zellij config; the layout is inspected only for readiness.
    let zellij_cmd = format!("zellij attach -c {}", shell_quote(new_session_name));

    let mut fake_client = crate::session::TemporaryClient::start_zellij(new_session_name, &zellij_cmd)
        .with_context(|| {
            format!("Failed to create session '{new_session_name}' via fake-PTY attach")
        })?;

    // Wait for the actual first-tab status plugin, not an unrelated visible plugin.
    let expected_plugin = default_layout.as_deref().and_then(default_tab_plugin);
    wait_for_inner_layout(new_session_name, expected_plugin.as_deref(), &mut fake_client)?;

    // Detach the fake client — the session stays alive, layout already applied.
    crate::session::zellij_action(&["-s", new_session_name, "action", "detach"])
        .with_context(|| format!("Failed to detach temporary client for '{new_session_name}'"))?;

    // Wait for `script` to exit after zellij detaches (prevents zombie processes).
    fake_client.finish()?;

    if !session_layout_ready(new_session_name, expected_plugin.as_deref())? {
        anyhow::bail!("Session '{new_session_name}' lost its initial layout after detaching temporary client");
    }

    // 2. Check if agent state file appears (auto-loaded via Zellij load_plugins).
    // If not after brief delay, explicitly launch plugin with --floating --no-focus fallback.
    std::thread::sleep(std::time::Duration::from_millis(150));
    let state_file =
        std::path::PathBuf::from(format!("/tmp/verij/states/{new_session_name}.json"));
    if !state_file.exists() {
        if let Some(path) = plugin_path {
            let plugin_url = format!("file:{}", path.display());
            let _ = crate::session::zellij_action(&[
                    "-s",
                    new_session_name,
                    "action",
                    "launch-plugin",
                    "--floating",
                    "--no-focus",
                    &plugin_url,
                ]);
        }
    }

    // 3. Switch right pane to this new session and rename the Workspace pane
    switch_session(
        old_active_session,
        new_session_name,
        None,
        Some(workspace_pane_name),
    )?;

    Ok(())
}

/// Switch to the target session and optionally navigate to a specific tab.
///
/// If `old_active_session` is present and differs from `target_session`,
/// triggers the Inception Switch via pipe injection into `old_active_session`.
/// If no session was previously active (e.g. initial launch in empty pane),
/// falls back to attaching directly via `zellij action write-chars`.
///
/// `workspace_pane_name`: if `Some(name)`, renames the host session's Workspace pane
/// to `name` after switching. Pass `None` to skip renaming.
pub fn switch_session(
    old_active_session: Option<&str>,
    target_session: &str,
    tab_position: Option<usize>,
    workspace_pane_name: Option<&str>,
) -> Result<()> {
    match old_active_session {
        Some(old) if old == target_session => {
            match tab_position {
                Some(pos) => {
                    // Tab click on the currently active session: just navigate the tab.
                    // The right pane should already be attached; switch_tab is safe.
                    switch_tab(target_session, pos)?;
                }
                None => {
                    // `active_session` identifies the Workspace attachment. Do not
                    // write another attach command into an already attached pane.
                }
            }
        }
        Some(old) => {
            // Native switch_session resurrects an EXITED target without Zellij's
            // --force-run-commands option. Restore it explicitly first so its
            // saved commands run rather than waiting for Enter in each pane.
            match crate::session::session_status(target_session)? {
                crate::session::SessionStatus::Exited => {
                    crate::session::resurrect_session(target_session)?;
                }
                crate::session::SessionStatus::Live => {}
                crate::session::SessionStatus::Missing => {
                    anyhow::bail!("Session '{target_session}' is no longer available");
                }
            }

            // The Inception Switch: trigger switch from inside the current session
            inception_switch(old, target_session)?;

            // If a specific tab is requested, navigate after brief delay
            if let Some(pos) = tab_position {
                if pos > 0 {
                    let target = target_session.to_string();
                    std::thread::spawn(move || {
                        std::thread::sleep(std::time::Duration::from_millis(150));
                        let _ = switch_tab(&target, pos);
                    });
                }
            }
        }
        None => {
            // First attach fallback: attach into the right pane
            attach_in_right_pane(target_session)?;

            if let Some(pos) = tab_position {
                if pos > 0 {
                    let target = target_session.to_string();
                    std::thread::spawn(move || {
                        std::thread::sleep(std::time::Duration::from_millis(200));
                        let _ = switch_tab(&target, pos);
                    });
                }
            }
        }
    }

    // Ensure inner session ready before refocusing
    std::thread::sleep(std::time::Duration::from_millis(200));
    // Always re-focus the right pane so keyboard input goes to the attached workspace
    re_focus_right_pane()?;

    // Rename the right-side Workspace pane, not the host session tab.
    if let Some(name) = workspace_pane_name {
        rename_workspace_pane(name)?;
    }
    set_workspace_session(target_session)?;

    Ok(())
}

const WORKSPACE_SESSION_ENV: &str = "VERIJ_WORKSPACE_SESSION";

fn workspace_marker_path() -> Option<PathBuf> {
    let host_session = std::env::var("ZELLIJ_SESSION_NAME").ok()?;
    let marker_key = std::env::var("VERIJ_HOST_MARKER_KEY").ok()
        .or_else(|| crate::registry::marker_key(&host_session).ok().flatten())
        .unwrap_or(host_session);
    Some(
        crate::config::runtime_dir().join(format!("workspace-{marker_key}.session")),
    )
}

/// Returns session recorded as attached to this host Workspace pane.
pub fn workspace_session() -> Option<String> {
    if let Some(path) = workspace_marker_path() {
        return std::fs::read_to_string(path)
            .ok()
            .map(|session| session.trim().to_string())
            .filter(|session| !session.is_empty());
    }
    std::env::var(WORKSPACE_SESSION_ENV)
        .ok()
        .filter(|session| !session.is_empty())
}

/// Records the inner session currently attached to the host Workspace pane.
pub fn set_workspace_session(session: &str) -> Result<()> {
    std::env::set_var(WORKSPACE_SESSION_ENV, session);
    let Some(path) = workspace_marker_path() else {
        return Ok(());
    };
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    std::fs::write(path, session)?;
    if let Some(host) = std::env::var("VERIJ_HOST_NAME").ok()
        .or_else(|| std::env::var("ZELLIJ_SESSION_NAME").ok()) {
        crate::config::set_last_host_session(&host, session)?;
    }
    Ok(())
}

/// Clears host Workspace attachment marker.
pub fn clear_workspace_session() -> Result<()> {
    std::env::remove_var(WORKSPACE_SESSION_ENV);
    if let Some(path) = workspace_marker_path() {
        match std::fs::remove_file(path) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(e.into()),
        }
    }
    Ok(())
}

#[derive(Clone, Debug, Deserialize)]
struct HostPaneInfo {
    id: u32,
    is_plugin: bool,
    pane_x: usize,
    tab_id: u32,
    title: String,
}

/// Returns pane immediately right of current TUI pane.
fn workspace_pane() -> Result<Option<HostPaneInfo>> {
    let output = crate::session::zellij_output(&["action", "list-panes", "--tab", "--json"])
        .context("Failed to inspect host panes")?;

    if !output.status.success() {
        return Ok(None);
    }

    let panes: Vec<HostPaneInfo> = serde_json::from_slice(&output.stdout)
        .context("Failed to parse host pane information")?;
    Ok(workspace_pane_from_list(
        &panes,
        std::env::var("ZELLIJ_PANE_ID").ok().as_deref(),
    ))
}

fn workspace_pane_from_list(panes: &[HostPaneInfo], sidebar_id: Option<&str>) -> Option<HostPaneInfo> {
    // Zellij supplies plain numeric ZELLIJ_PANE_ID in terminal processes;
    // CLI pane identifiers also accept the explicit terminal_<id> form.
    let sidebar_id = sidebar_id?;
    let current_id = sidebar_id
        .strip_prefix("terminal_")
        .unwrap_or(sidebar_id)
        .parse::<u32>()
        .ok()?;
    // Plugin and terminal IDs are separate namespaces. A plugin may have the
    // same numeric ID as the sidebar terminal, so require the terminal kind.
    let sidebar = panes.iter().find(|pane| !pane.is_plugin && pane.id == current_id)?;

    panes
        .iter()
        .filter(|pane| !pane.is_plugin && pane.tab_id == sidebar.tab_id && pane.pane_x > sidebar.pane_x)
        .min_by_key(|pane| pane.pane_x)
        .cloned()
}

/// Returns title of pane immediately right of current TUI pane.
/// Used to recover attachment state from hosts created before the marker existed.
pub fn workspace_pane_title() -> Result<Option<String>> {
    Ok(workspace_pane()?.map(|pane| pane.title))
}

/// Renames host Workspace pane, explicitly targeting pane right of sidebar.
pub fn rename_workspace_pane(name: &str) -> Result<()> {
    let pane_id = workspace_pane()?
        .map(|pane| format!("terminal_{}", pane.id))
        .context("Failed to identify host Workspace pane")?;
    crate::session::zellij_action(&["action", "rename-pane", "--pane-id", &pane_id, name])
        .context("Failed to rename workspace pane")
}

/// Replace only this host's nested client with a fresh shell. In-place replacement
/// preserves the host layout and leaves the inner session's server running.
pub fn reset_workspace(name: &str) -> Result<()> {
    let old_pane = workspace_pane()?.context("Failed to identify host Workspace pane")?;
    let pane_id = format!("terminal_{}", old_pane.id);
    crate::session::zellij_action(&[
        "action", "new-pane", "--in-place", "--close-replaced-pane", "--pane-id", &pane_id,
        "--no-focus", "--name", name, "--borderless", "true",
    ])?;
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline {
        if workspace_pane()?.is_some_and(|pane| pane.id != old_pane.id) {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    anyhow::bail!("Workspace pane replacement did not complete")
}

pub fn focus_sidebar() -> Result<()> {
    let id = std::env::var("ZELLIJ_PANE_ID").context("Sidebar pane ID is not available")?;
    let id = id.strip_prefix("terminal_").unwrap_or(&id).parse::<u32>()
        .context("Invalid sidebar pane ID")?;
    let output = crate::session::zellij_output(&["action", "focus-pane-id", &format!("terminal_{id}")])?;
    let diagnostic = format!("{} {}", String::from_utf8_lossy(&output.stderr).trim(),
        String::from_utf8_lossy(&output.stdout).trim());
    // Zellij returns a nonzero status for this successful no-op.
    if output.status.success()
        || diagnostic.contains(&format!("Pane Terminal({id}) is already focused")) {
        return Ok(());
    }
    anyhow::bail!("Failed to focus sidebar: {}", diagnostic.trim())
}

pub fn forget_workspace_session() -> Result<()> {
    clear_workspace_session()?;
    if let Ok(host) = std::env::var("VERIJ_HOST_NAME").or_else(|_| std::env::var("ZELLIJ_SESSION_NAME")) {
        crate::config::clear_last_host_session(&host)?;
    }
    Ok(())
}

#[derive(Debug, Deserialize)]
struct TabInfo {
    tab_id: u32,
    position: usize,
    name: String,
}

/// Resolve the selected snapshot to a stable tab ID; never change active tabs
/// merely to close one, and reject a stale position/name instead of closing another.
pub fn close_tab(session_name: &str, position: usize, name: &str) -> Result<bool> {
    let output = crate::session::zellij_output(&["--session", session_name, "action", "list-tabs", "--json"])?;
    if !output.status.success() {
        anyhow::bail!("Failed to inspect tabs in '{session_name}'");
    }
    let tabs: Vec<TabInfo> = serde_json::from_slice(&output.stdout).context("Invalid Zellij tab inventory")?;
    let tab = tabs.iter().find(|tab| tab.position == position && tab.name == name)
        .context("Selected tab changed; wait for sidebar to refresh and retry")?;
    crate::session::zellij_action(&[
        "--session", session_name, "action", "close-tab-by-id", &tab.tab_id.to_string(),
    ])?;
    Ok(tabs.len() == 1)
}

pub fn kill_session(session_name: &str) -> Result<()> {
    crate::session::zellij_action(&["kill-session", session_name])?;
    let deadline = Instant::now() + Duration::from_secs(3);
    while Instant::now() < deadline {
        if crate::session::session_status(session_name)? != crate::session::SessionStatus::Live {
            return Ok(());
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    anyhow::bail!("Session '{session_name}' did not stop")
}

/// Executes "The Inception Switch":
/// `zellij -s <old_session> pipe --name verij_control -- switch:<new_session>`
pub fn inception_switch(old_active_session: &str, new_selected_session: &str) -> Result<()> {
    let payload = format!("switch:{}", new_selected_session);

    crate::session::zellij_action(&[
            "-s",
            old_active_session,
            "pipe",
            "--name",
            VERIJ_CONTROL_PIPE,
            "--",
            &payload,
        ])
        .with_context(|| {
            format!(
                "Failed to dispatch Inception Switch pipe to session '{}'",
                old_active_session
            )
        })
}

/// Re-focuses the right pane: `zellij action move-focus right`.
pub fn re_focus_right_pane() -> Result<()> {
    crate::session::zellij_action(&["action", "move-focus", "right"])
        .context("Failed to execute `zellij action move-focus right`")
}

/// Switch to a specific tab within the named session:
/// `zellij --session <target> action go-to-tab <position + 1>`.
pub fn switch_tab(session_name: &str, tab_position: usize) -> Result<()> {
    let tab_arg = (tab_position + 1).to_string();
    crate::session::zellij_action(&["--session", session_name, "action", "go-to-tab", &tab_arg])
        .with_context(|| {
            format!(
                "Failed to navigate to tab {} in session '{}'",
                tab_position, session_name
            )
        })
}

/// Initial attach fallback when no session was previously active in the right pane:
/// sends `stty sane; zellij attach <target>\n` to the active pane.
fn attach_in_right_pane(target_session: &str) -> Result<()> {
    let pane_id = workspace_pane()?.map(|pane| format!("terminal_{}", pane.id))
        .context("Failed to identify host Workspace pane")?;
    set_workspace_session(target_session)?;
    let marker_cleanup = workspace_marker_path()
        .map(|path| format!("; rm -f {}", shell_quote(&path.to_string_lossy())))
        .unwrap_or_default();
    let resurrection_flag = matches!(
        crate::session::session_status(target_session),
        Ok(crate::session::SessionStatus::Exited)
    );
    if resurrection_flag {
        let _ = crate::session::prepare_resurrection_layout(target_session);
    }
    let attach_options = if resurrection_flag {
        "--force-run-commands "
    } else {
        ""
    };
    let attach_cmd = format!(
        "export {WORKSPACE_SESSION_ENV}={}; stty sane; zellij attach {attach_options}{}; unset {WORKSPACE_SESSION_ENV}{marker_cleanup}\n",
        shell_quote(target_session),
        shell_quote(target_session),
    );
    let result = crate::session::zellij_action(&["action", "write-chars", "--pane-id", &pane_id, &attach_cmd])
        .with_context(|| {
            format!(
                "Failed to send initial attach command for session '{}'",
                target_session
            )
        });

    if result.is_err() {
        let _ = clear_workspace_session();
    }
    result
}

fn shell_quote(value: &str) -> String {
    format!("'{}'", value.replace('\\', "\\\\").replace('\'', "'\\''"))
}

fn default_tab_plugin(layout: &Path) -> Option<String> {
    let source = std::fs::read_to_string(layout).ok()?;
    let template = source.split_once("default_tab_template")?.1;
    template.lines().find_map(|line| {
        line.trim()
            .strip_prefix("plugin location=\"")
            .and_then(|value| value.split_once('"').map(|(location, _)| location.to_string()))
    })
}

/// Find the user's configured layout without overriding it on inner-session creation.
fn configured_zellij_layout() -> Option<PathBuf> {
    let config_dir = crate::zellij_config::config_dir()?;
    let config_file = crate::zellij_config::effective_config_path()
        .ok()
        .flatten()
        .unwrap_or_else(|| config_dir.join("config.kdl"));
    let config = std::fs::read_to_string(config_file).unwrap_or_default();
    let layout_dir = kdl_option(&config, "layout_dir")
        .map(PathBuf::from)
        .unwrap_or_else(|| config_dir.join("layouts"));
    let layout = kdl_option(&config, "default_layout").unwrap_or_else(|| "default".to_string());
    let path = PathBuf::from(&layout);
    let path = if path.is_absolute() {
        path
    } else if path.extension().is_some() {
        layout_dir.join(path)
    } else {
        layout_dir.join(format!("{layout}.kdl"))
    };
    path.exists().then_some(path)
}

fn kdl_option(config: &str, key: &str) -> Option<String> {
    config.lines().find_map(|line| {
        line.trim()
            .strip_prefix(key)?
            .trim_start()
            .strip_prefix('"')?
            .split_once('"')
            .map(|(value, _)| value.to_string())
    })
}

fn wait_for_inner_layout(
    session_name: &str,
    location: Option<&str>,
    client: &mut crate::session::TemporaryClient,
) -> Result<()> {
    let deadline = Instant::now() + Duration::from_secs(10);
    let mut stable_since = None;
    while Instant::now() < deadline {
        if let Some(status) = client.try_wait()? {
            anyhow::bail!("Creation client for '{session_name}' exited before its layout was ready: {status}");
        }
        if session_layout_ready(session_name, location)? {
            let since = stable_since.get_or_insert_with(Instant::now);
            if since.elapsed() >= Duration::from_millis(1200) {
                return Ok(());
            }
        } else {
            stable_since = None;
        }
        std::thread::sleep(Duration::from_millis(50));
    }
    anyhow::bail!("Session '{session_name}' did not initialize its first-tab layout within 10s");
}

fn session_layout_ready(session_name: &str, location: Option<&str>) -> Result<bool> {
    if !matches!(crate::session::session_status(session_name)?, crate::session::SessionStatus::Live) {
        return Ok(false);
    }
    let output = crate::session::query_zellij(Command::new("zellij")
        .args(["--session", session_name, "action", "list-panes", "--all", "--json"])
    )?;

    if !output.status.success() {
        return Ok(false);
    }

    let Ok(panes) = serde_json::from_slice::<Vec<serde_json::Value>>(&output.stdout) else {
        return Ok(false);
    };

    Ok(match location {
        Some(location) => first_tab_plugin_ready(&panes, location),
        None => first_tab_has_terminal(&panes),
    })
}

fn first_tab_plugin_ready(panes: &[serde_json::Value], location: &str) -> bool {
    let plugin = panes.iter().any(|pane| {
        let url = pane
            .get("plugin_url")
            .and_then(|value| value.as_str())
            .unwrap_or("");
        pane.get("is_plugin").and_then(|value| value.as_bool()) == Some(true)
            && pane.get("tab_position").and_then(|value| value.as_u64()) == Some(0)
            && pane.get("is_suppressed").and_then(|value| value.as_bool()) == Some(false)
            && pane.get("is_floating").and_then(|value| value.as_bool()) == Some(false)
            && (url == location || url.contains(location))
    });
    plugin && first_tab_has_terminal(panes)
}

fn first_tab_has_terminal(panes: &[serde_json::Value]) -> bool {
    panes.iter().any(|pane| {
        pane.get("is_plugin").and_then(|value| value.as_bool()) == Some(false)
            && pane.get("tab_position").and_then(|value| value.as_u64()) == Some(0)
            && pane.get("pane_rows").and_then(|value| value.as_u64()).unwrap_or(0) > 0
    })
}

#[cfg(test)]
mod readiness_tests {
    use super::{first_tab_plugin_ready, kdl_option};
    use serde_json::json;

    #[cfg(unix)]
    #[test]
    fn creation_reports_early_client_exit_without_waiting_for_readiness() {
        let mut client = crate::session::TemporaryClient::spawn("sh -c 'exit 7'").unwrap();
        let deadline = std::time::Instant::now() + std::time::Duration::from_secs(3);
        while client.try_wait().unwrap().is_none() {
            assert!(std::time::Instant::now() < deadline, "temporary client did not exit");
            std::thread::sleep(std::time::Duration::from_millis(20));
        }
        let error = super::wait_for_inner_layout("failed-creation", None, &mut client).unwrap_err();
        assert!(error.to_string().contains("exited before its layout was ready"));
    }

    #[test]
    fn requires_target_plugin_on_first_tab_and_terminal() {
        let unrelated = json!({
            "is_plugin": true, "plugin_url": "zellij:session-manager",
            "tab_position": 0, "is_suppressed": false, "is_floating": true
        });
        let status_on_second_tab = json!({
            "is_plugin": true, "plugin_url": "https://example.com/zjstatus.wasm",
            "tab_position": 1, "is_suppressed": false, "is_floating": false
        });
        let terminal = json!({"is_plugin": false, "tab_position": 0, "pane_rows": 20});
        assert!(!first_tab_plugin_ready(
            &[unrelated, status_on_second_tab, terminal.clone()],
            "zjstatus"
        ));
        let status = json!({
            "is_plugin": true, "plugin_url": "https://example.com/zjstatus.wasm",
            "tab_position": 0, "is_suppressed": false, "is_floating": false
        });
        assert!(first_tab_plugin_ready(&[status, terminal], "zjstatus"));
    }

    #[test]
    fn layout_probe_uses_active_zellij_option_not_commented_default() {
        let config = "// default_layout \"classic\"\ndefault_layout \"my-layout\"\n";
        assert_eq!(kdl_option(config, "default_layout").as_deref(), Some("my-layout"));
    }
}

#[cfg(test)]
mod workspace_pane_tests {
    use super::{workspace_pane_from_list, HostPaneInfo};

    fn pane(id: u32, is_plugin: bool, tab_id: u32, pane_x: usize) -> HostPaneInfo {
        HostPaneInfo {
            id,
            is_plugin,
            tab_id,
            pane_x,
            title: format!("Pane {id}"),
        }
    }

    #[test]
    fn finds_workspace_from_sidebar_terminal_even_if_plugin_has_same_id() {
        let panes = [
            pane(0, true, 0, 25),
            pane(3, false, 1, 0),
            pane(0, false, 0, 0),
            pane(2, false, 0, 60),
            pane(1, false, 0, 25),
        ];
        assert_eq!(workspace_pane_from_list(&panes, Some("terminal_0")).unwrap().id, 1);
        assert_eq!(workspace_pane_from_list(&panes, Some("0")).unwrap().id, 1);
        assert!(workspace_pane_from_list(&panes, Some("plugin_0")).is_none());
    }

    #[test]
    fn does_not_target_panes_on_other_tabs_or_when_sidebar_is_missing() {
        let panes = [pane(0, false, 0, 0), pane(1, false, 1, 25)];
        assert!(workspace_pane_from_list(&panes, Some("terminal_0")).is_none());
        assert!(workspace_pane_from_list(&panes, Some("terminal_3")).is_none());
    }
}
