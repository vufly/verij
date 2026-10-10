pub mod agent_view;
mod persistence;
/// verij-cli/src/tui/mod.rs
///
/// The Ratatui event loop for `verij ui`.
///
/// Handles:
///   1. Keyboard input (navigation, fold/unfold, actions, shortcuts).
///   2. Mouse input (select/attach, caret folding, wheel scroll).
///   3. Session snapshot messages from the filesystem watcher (`/tmp/verij/states/`).
///   4. Tick-based spinner animation for empty/connecting states.
///   5. Dynamic terminal resize handling.
pub mod render;
pub mod state;
pub mod tree_format;
pub mod tree_model;
mod visits;
mod workspace;

use anyhow::{Context, Result};
use crossterm::{
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event as CEvent, KeyCode, KeyEvent, KeyEventKind,
        KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
    },
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{
    backend::{Backend, CrosstermBackend},
    Terminal,
};
use std::io;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

use crate::actions;
use crate::config::{AfterSessionAction, Config};
use crate::fs_watcher;
use state::{AppState, InputMode, PendingAction, TreeNode};

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

/// Launch the Verij TUI.
pub async fn run(config: Config) -> Result<()> {
    // Validate formats before entering the alternate screen so warnings remain visible.
    let state = AppState::with_config(config);
    enable_raw_mode().context("Failed to enable raw mode")?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)
        .context("Failed to enter alternate screen")?;

    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend).context("Failed to create terminal")?;
    // `Terminal::clear()` queries terminal cursor position, which can time out
    // inside Zellij during a resize. Clearing backend directly needs no query.
    terminal.backend_mut().clear()?;

    let result = event_loop(&mut terminal, state).await;

    // --- Restore terminal (always runs, even on error) ---
    disable_raw_mode().ok();
    execute!(
        terminal.backend_mut(),
        LeaveAlternateScreen,
        DisableMouseCapture
    )
    .ok();
    terminal.show_cursor().ok();

    result
}

// ---------------------------------------------------------------------------
// Event loop
// ---------------------------------------------------------------------------

async fn event_loop(
    terminal: &mut Terminal<CrosstermBackend<io::Stdout>>,
    mut state: AppState,
) -> Result<()> {
    let host_key = std::env::var("VERIJ_HOST_MARKER_KEY")
        .ok()
        .filter(|key| verij_types::identity::valid_record_key(key));
    let mut acknowledgements = host_key
        .as_deref()
        .and_then(|host| visits::read_acknowledgements(host).ok())
        .unwrap_or_default();
    let mut focus = None;
    let mut presentation_cache = agent_view::PresentationCache::default();
    let mut last_model_refresh = Instant::now();
    let mut focus_monitor = host_key.clone().map(visits::Monitor::start);
    let saved = host_key
        .as_deref()
        .and_then(|host| persistence::load(host).ok())
        .flatten();
    if let Some(saved) = saved.as_ref() {
        persistence::restore_folds(&mut state, saved);
    }
    let mut saved_selection_restored = false;
    let mut topology_ready = false;
    let mut records_ready = false;
    let mut ready_reported = false;
    let mut last_persisted = saved.clone();
    let mut last_focus_poll = Instant::now() - Duration::from_secs(2);
    let mut workspace_presentation = workspace::Presentation::start();
    let mut recovered_workspace_title = None;
    let mut workspace_title_error = None;
    let (tx, mut rx) = mpsc::channel(16);
    let (agent_tx, mut agent_rx) = mpsc::channel(16);
    let agent_watcher = match crate::agent_watcher::spawn(agent_tx) {
        Ok(handle) => Some(handle),
        Err(error) => {
            state.error = Some(format!("Agent watcher: {error}"));
            None
        }
    };
    state.plugin_path = crate::layout::resolve_plugin_path(None).ok();

    // Spawn filesystem watcher background task targeting /tmp/verij/states/
    let topology_watcher = match fs_watcher::spawn_fs_watcher(tx) {
        Ok(handle) => Some(handle),
        Err(e) => {
            state.error = Some(format!("FS watcher error: {e}"));
            None
        }
    };

    restore_last_workspace_session(&mut state);
    state.activation = Some(crate::activation::Queue::start());

    // Initial render
    terminal.draw(|frame| render::render(frame, &mut state))?;

    let mut last_click: Option<(Instant, usize)> = None;

    loop {
        let mut needs_render = false;
        let mut model_changed = false;
        if let Some(update) = workspace_presentation.take_update() {
            if !update.retry && state.error == workspace_title_error {
                state.error = None;
                workspace_title_error = None;
            }
            if let Some(error) = update.error {
                workspace_title_error = Some(error.clone());
                state.error = Some(error);
            }
            if let Some(title) = update.title {
                recovered_workspace_title = Some(title.clone());
                state.workspace_pane_name = Some(title);
            }
            needs_render = true;
        }
        workspace_presentation.retry();
        for _ in 0..16 {
            let Ok(records) = agent_rx.try_recv() else {
                break;
            };
            state.agents = records;
            records_ready = true;
            model_changed = true;
            needs_render = true;
        }
        if let Some(monitor) = focus_monitor.as_mut() {
            if let Some(update) = monitor.take_update() {
                focus = update.focus;
                state.apply_focus(focus.clone());
                if let Some(acks) = update.acknowledgements {
                    merge_acknowledgements(&mut acknowledgements, acks);
                }
                if let Some(error) = update.error {
                    state.error = Some(error);
                }
                model_changed = true;
                needs_render = true;
            }
        }
        let mut focused = Vec::new();
        let mut finished = Vec::new();
        if let Some(queue) = state.activation.as_mut() {
            while let Some(update) = queue.take_focus() {
                focused.push(update);
            }
            while let Ok(result) = queue.receiver.try_recv() {
                if queue.is_current(result.request.ticket) {
                    queue.pending = false;
                    finished.push(result);
                }
            }
        }
        for update in focused {
            state.active_session = Some(update.session.clone());
            focus = Some(update.pane);
            state.apply_focus(focus.clone());
            let _ = actions::set_workspace_session(&update.session);
            model_changed = true;
            needs_render = true;
        }
        for completion in finished {
            state.progress = None;
            match completion.result {
                Ok(crate::activation::Outcome::Legacy) => {
                    state.workspace_focus = None;
                    state.active_session = Some(completion.request.session);
                    if completion.request.title.is_some() {
                        state.workspace_pane_name = completion.request.title;
                    }
                    state.error = None;
                }
                Ok(crate::activation::Outcome::Agent {
                    session,
                    pane,
                    ack,
                    instance,
                }) => {
                    focus = Some(pane);
                    if let Some(ack) = ack {
                        merge_acknowledgements(&mut acknowledgements, [(instance.0, ack)]);
                    }
                    state.active_session = Some(session.clone());
                    state.apply_focus(focus.clone());
                    let _ = actions::set_workspace_session(&session);
                    model_changed = true;
                    state.error = None;
                }
                Err(error) => state.error = Some(error),
            }
            state.rebuild_nodes();
            state.sync_list_state();
            needs_render = true;
        }

        // Poll for terminal events (100 ms timeout for animation & responsiveness)
        if event::poll(Duration::from_millis(100)).context("Failed to poll terminal events")? {
            match event::read().context("Failed to read terminal event")? {
                CEvent::Key(key) => {
                    if handle_key(&mut state, key, focus_monitor.as_mut())? {
                        break;
                    }
                    needs_render = true;
                }
                CEvent::Mouse(mouse) => {
                    handle_mouse(&mut state, mouse, &mut last_click, focus_monitor.as_mut())?;
                    needs_render = true;
                }
                CEvent::Resize(_cols, _rows) => {
                    // `draw` autoresizes and clears its fullscreen viewport. Calling
                    // Terminal::clear() here queries cursor position and can time out
                    // while Zellij is still processing a drag-resize.
                    needs_render = true;
                }
                _ => {}
            }
        }

        if let Some(name) = state.creation_pending.take() {
            invalidate_navigation(&mut state, focus_monitor.as_mut());
            state.progress = Some(format!("Creating session '{name}'…"));
            state.error = None;
            terminal.draw(|frame| render::render(frame, &mut state))?;
            let old_active = state.active_session.clone();
            let workspace_pane_name = state.format_workspace_pane_name(&name);
            let result = actions::create_inner_session(
                old_active.as_deref(),
                &name,
                state.plugin_path.as_deref(),
                &workspace_pane_name,
            );
            state.progress = None;
            match result {
                Ok(()) => {
                    state.active_session = Some(name);
                    state.workspace_pane_name = Some(workspace_pane_name);
                }
                Err(error) => state.error = Some(format!("Create error: {error:#}")),
            }
            needs_render = true;
        }

        if let Some(action) = state.pending_action.take() {
            if let Some(message) = action_progress(&state, action) {
                state.progress = Some(message);
                state.error = None;
                // Actions synchronously wait for Zellij. Flush the message before
                // dispatch so it stays visible throughout resurrection.
                terminal.draw(|frame| render::render(frame, &mut state))?;
                let result = dispatch_pending_action(&mut state, action, focus_monitor.as_mut());
                if action != PendingAction::Activate
                    || !state.activation.as_ref().is_some_and(|queue| queue.pending)
                {
                    state.progress = None;
                }
                if action != PendingAction::Activate {
                    focus = None;
                    state.apply_focus(None);
                    model_changed = true;
                }
                result?;
                needs_render = true;
            }
        }

        // Drain pending session updates from the filesystem watcher
        for _ in 0..16 {
            match rx.try_recv() {
                Ok(snapshot) => {
                    state.reconcile(snapshot);
                    topology_ready = true;
                    model_changed = true;

                    // Restore or clear Workspace attachment from host-pane marker.
                    let marked_session = actions::workspace_session();
                    let recovered_title = if marked_session.is_none() {
                        workspace_presentation.request(None);
                        recovered_workspace_title.clone()
                            .filter(|title| title != &state.config.workspace.pane_default)
                    } else {
                        None
                    };
                    let attached_session = marked_session.clone().or_else(|| {
                        recovered_title.as_deref().and_then(|title| {
                            state
                                .sessions
                                .iter()
                                .find(|session| {
                                    state.format_workspace_pane_name(&session.name) == title
                                })
                                .map(|session| session.name.clone())
                        })
                    });
                    if state.active_session.is_none() {
                        if let Some(session_name) = attached_session.as_deref() {
                            if state
                                .sessions
                                .iter()
                                .any(|session| session.name == session_name)
                            {
                                state.active_session = Some(session_name.to_string());
                                if marked_session.is_none() {
                                    let _ = actions::set_workspace_session(session_name);
                                }
                                state.rebuild_nodes();
                            }
                        }
                    } else if attached_session.as_deref() != state.active_session.as_deref()
                        && !state.activation.as_ref().is_some_and(|queue| queue.pending)
                    {
                        state.active_session = None;
                        let _ = actions::clear_workspace_session();
                        let default_name = state.config.workspace.pane_default.clone();
                        workspace_presentation.request(Some(default_name));
                        state.rebuild_nodes();
                    }

                    if let Some(active) = state.active_session.clone() {
                        let desired_name = state.format_workspace_pane_name(&active);
                        if state.workspace_pane_name.as_deref() != Some(&desired_name) {
                            workspace_presentation.request(Some(desired_name));
                        }
                    }
                    needs_render = true;
                }
                Err(mpsc::error::TryRecvError::Empty) => break,
                Err(mpsc::error::TryRecvError::Disconnected) => {
                    state.error = Some("Filesystem watcher disconnected.".into());
                    needs_render = true;
                    break;
                }
            }
        }

        if model_changed || last_model_refresh.elapsed() >= Duration::from_secs(1) {
            let views = presentation_cache.refresh(
                &state.sessions,
                &state.agents,
                &acknowledgements,
                &state.config.agents,
                focus.as_ref(),
            );
            state.set_agent_views(views);
            last_model_refresh = Instant::now();
            needs_render = true;
        }
        if topology_ready && records_ready && !saved_selection_restored {
            if let Some(saved) = saved.as_ref() {
                persistence::restore_selection(&mut state, saved);
            }
            saved_selection_restored = true;
            needs_render = true;
        }
        if last_focus_poll.elapsed() >= Duration::from_secs(1)
            && !state.agent_views.is_empty()
            && !state.activation.as_ref().is_some_and(|queue| queue.pending)
        {
            if let Some(monitor) = focus_monitor.as_mut() {
                let candidates = state
                    .agent_views
                    .iter()
                    .filter(|view| {
                        view.navigable && view.status == verij_types::agent::AgentStatus::Done
                    })
                    .map(|view| {
                        (
                            view.instance.clone(),
                            view.pane.clone(),
                            view.completion_revision,
                        )
                    })
                    .collect();
                if monitor.request(candidates) {
                    last_focus_poll = Instant::now();
                }
            }
        }

        // Tick counter for animations (e.g. connecting spinner)
        state.tick = state.tick.wrapping_add(1);
        if state.nodes.is_empty() || state.nodes.iter().any(|node|node.summary().working>0
            || matches!(node,state::TreeNode::AgentPane {view,..} if view.status==verij_types::agent::AgentStatus::Working)) {
            needs_render = true;
        }

        if needs_render {
            terminal.draw(|frame| render::render(frame, &mut state))?;
            if !ready_reported
                && topology_ready
                && records_ready
                && saved_selection_restored
                && !state.nodes.is_empty()
            {
                // Opt-in private harness readiness: emitted only after this
                // process rendered its hydrated/restored model. Contains no
                // terminal text, conversation data or semantic assertions.
                if let Some(path) = std::env::var_os("VERIJ_UI_READY_FILE")
                    .map(std::path::PathBuf::from)
                    .filter(|path| path.is_absolute())
                {
                    if let Ok(process) = crate::process::identity(std::process::id()) {
                        let _ = crate::agent_store::atomic_write_json(&path, &process);
                    }
                }
                ready_reported = true;
            }
        }
        if saved_selection_restored && !state.nodes.is_empty() {
            if let Some(host) = host_key.as_deref() {
                let current = persistence::capture(&state);
                if last_persisted.as_ref() != Some(&current) {
                    if let Err(error) = persistence::save(host, &current) {
                        state.error = Some(format!("Tree state: {error}"));
                    } else {
                        last_persisted = Some(current);
                    }
                }
            }
        }
    }

    drop(rx);
    drop(agent_rx);
    if let Some(watcher) = topology_watcher {
        watcher.stop();
    }
    if let Some(watcher) = agent_watcher {
        watcher.stop();
    }
    Ok(())
}

/// Poll results can race with a newer activation receipt. Host acknowledgements
/// only advance; a delayed snapshot must not resurrect an already-read turn.
fn merge_acknowledgements(
    current: &mut std::collections::BTreeMap<String, verij_types::agent::InstanceAck>,
    incoming: impl IntoIterator<Item = (String, verij_types::agent::InstanceAck)>,
) {
    for (instance, ack) in incoming {
        let replace = current.get(&instance).map_or(true, |existing| {
            (ack.acknowledged_revision, ack.acknowledged_at_ms)
                > (existing.acknowledged_revision, existing.acknowledged_at_ms)
        });
        if replace {
            current.insert(instance, ack);
        }
    }
}

/// Reconnects the host Workspace pane to its durable last inner session.
fn restore_last_workspace_session(state: &mut AppState) {
    if actions::workspace_session().is_some() {
        return;
    }

    let Ok(host_session) = std::env::var("ZELLIJ_SESSION_NAME") else {
        return;
    };
    let Some(last_session) = crate::config::last_host_session(&host_session) else {
        return;
    };

    let deadline = std::time::Instant::now() + std::time::Duration::from_secs(1);
    while std::time::Instant::now() < deadline {
        if actions::workspace_session().as_deref() == Some(last_session.as_str()) {
            return;
        }
        if let Ok(Some(title)) = actions::workspace_pane_title() {
            if title_matches_session(&title, &last_session) {
                let _ = actions::set_workspace_session(&last_session);
                return;
            }
        }
        std::thread::sleep(std::time::Duration::from_millis(50));
    }

    let status = crate::session::session_status(&last_session);
    if !matches!(
        status,
        Ok(crate::session::SessionStatus::Live | crate::session::SessionStatus::Exited)
    ) {
        return;
    }

    let pane_name = state
        .config
        .workspace
        .format_pane_name(&last_session, None, None);
    if let Err(error) = actions::switch_session(None, &last_session, None, Some(&pane_name)) {
        state.error = Some(format!("Failed to restore '{last_session}': {error}"));
    }
}

fn title_matches_session(title: &str, session: &str) -> bool {
    title == session || title.split(" | ").any(|part| part.trim() == session)
}

// ---------------------------------------------------------------------------
// Input handlers
// ---------------------------------------------------------------------------

/// Process keyboard events. Returns `true` if TUI should quit.
fn handle_key(
    state: &mut AppState,
    key: KeyEvent,
    _monitor: Option<&mut visits::Monitor>,
) -> Result<bool> {
    if key.kind == KeyEventKind::Release {
        return Ok(false);
    }
    if state.input_mode != InputMode::Normal {
        match key.code {
            KeyCode::Esc => {
                state.cancel_new_session_prompt();
            }
            KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => {
                state.cancel_new_session_prompt();
            }
            KeyCode::Backspace => {
                state.handle_input_backspace();
            }
            KeyCode::Enter => {
                let name = state.input_buffer.trim().to_string();
                if !name.is_empty() {
                    if state.input_mode == InputMode::RenameHost {
                        let old = std::env::var("ZELLIJ_SESSION_NAME").unwrap_or_default();
                        if let Err(e) = crate::session::rename_host(&old, &name) {
                            state.error = Some(format!("Rename error: {e}"));
                        }
                        state.cancel_new_session_prompt();
                        return Ok(false);
                    }
                    state.creation_pending = Some(name);
                }
                state.cancel_new_session_prompt();
            }
            KeyCode::Char(c) => {
                state.handle_input_char(c);
            }
            _ => {}
        }
        return Ok(false);
    }

    match key.code {
        // Quit
        KeyCode::Char('q') | KeyCode::Esc => return Ok(true),
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => return Ok(true),

        // New inner session prompt
        KeyCode::Char('n') | KeyCode::Char('c') | KeyCode::Char('+') => {
            state.start_new_session_prompt();
        }

        KeyCode::Char('R') => state.start_rename_host_prompt(),

        // Help bar toggle
        KeyCode::Char('?') => {
            state.toggle_help();
        }

        // Navigation (Vim & Arrow keys)
        KeyCode::Char('J') => state.next_session(),
        KeyCode::Char('K') => state.previous_session(),
        KeyCode::Char('j') if key.modifiers == KeyModifiers::SHIFT => state.next_session(),
        KeyCode::Char('k') if key.modifiers == KeyModifiers::SHIFT => state.previous_session(),
        KeyCode::Char('j') | KeyCode::Down => state.cursor_down(),
        KeyCode::Char('k') | KeyCode::Up => state.cursor_up(),

        // Page navigation
        KeyCode::PageDown => state.page_down(10),
        KeyCode::PageUp => state.page_up(10),
        KeyCode::Char('d') if key.modifiers.contains(KeyModifiers::CONTROL) => state.page_down(10),
        KeyCode::Char('u') if key.modifiers.contains(KeyModifiers::CONTROL) => state.page_up(10),

        // Jump to top / bottom
        KeyCode::Home | KeyCode::Char('g') => state.cursor_home(),
        KeyCode::End | KeyCode::Char('G') => state.cursor_end(),

        // Tree folding
        KeyCode::Char(' ') | KeyCode::Tab => state.toggle_collapse(),
        KeyCode::Left | KeyCode::Char('h') => state.collapse_selected(),
        KeyCode::Right | KeyCode::Char('l') => state.expand_selected(),

        // Action: The Inception Switch
        KeyCode::Enter => {
            state.pending_action = Some(PendingAction::Activate);
        }

        KeyCode::Char('d') if key.modifiers.is_empty() && key.kind == KeyEventKind::Press => {
            if state.active_session.is_some() {
                state.pending_action = Some(PendingAction::DetachSession);
            }
        }
        KeyCode::Char('x') if key.modifiers.is_empty() && key.kind == KeyEventKind::Press => {
            state.pending_action = match state.selected_node() {
                Some(TreeNode::Session { needs_resurrection: false, .. }) => Some(PendingAction::KillSession),
                Some(TreeNode::Tab { .. }) => Some(PendingAction::CloseTab),
                _ => None,
            };
        }

        _ => {}
    }

    Ok(false)
}

/// Process mouse events (click, scroll wheel).
fn handle_mouse(
    state: &mut AppState,
    mouse: MouseEvent,
    last_click: &mut Option<(Instant, usize)>,
    _monitor: Option<&mut visits::Monitor>,
) -> Result<()> {
    match mouse.kind {
        MouseEventKind::ScrollDown => {
            state.cursor_down();
        }
        MouseEventKind::ScrollUp => {
            state.cursor_up();
        }
        // With borders removed, items start directly on row 0
        MouseEventKind::Down(MouseButton::Left) => {
            let visual_index = mouse.row as usize;
            let offset = state.list_state.offset();
            let target_index = offset + visual_index;

            if target_index < state.nodes.len() {
                let clicked_caret = state
                    .fold_hitboxes
                    .get(target_index)
                    .and_then(|range| range.as_ref())
                    .is_some_and(|range| range.contains(&mouse.column));
                state.select_index(target_index);

                if clicked_caret {
                    state.toggle_collapse();
                    *last_click = None;
                    return Ok(());
                }

                if state.config.tui.single_click_action {
                    state.pending_action = Some(PendingAction::Activate);
                    *last_click = None;
                    return Ok(());
                }

                // Check for double click within 400ms
                let now = Instant::now();
                if let Some((prev_time, prev_idx)) = *last_click {
                    if prev_idx == target_index
                        && now.duration_since(prev_time) < Duration::from_millis(400)
                    {
                        state.pending_action = Some(PendingAction::Activate);
                        *last_click = None;
                        return Ok(());
                    }
                }

                *last_click = Some((now, target_index));
            }
        }
        _ => {}
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Action dispatch: The Inception Switch
// ---------------------------------------------------------------------------

fn action_progress(state: &AppState, action: PendingAction) -> Option<String> {
    if action == PendingAction::DetachSession {
        return state.active_session.as_ref().map(|session| format!("Detaching session '{session}'…"));
    }
    let node = state.selected_node()?;
    Some(match action {
        PendingAction::Activate => match node {
            TreeNode::Session { needs_resurrection: true, name, .. } => format!("Restoring session '{name}'…"),
            TreeNode::Tab { session_name, name, .. }
                if state.active_session.as_deref() == Some(session_name) => format!("Switching tab '{name}'…"),
            TreeNode::AgentPane { session_name, view, .. }
                if state.active_session.as_deref() == Some(session_name) => format!("Focusing agent '{}'…", view.title),
            TreeNode::Session { name, .. }
                if state.active_session.as_deref() == Some(name) => format!("Focusing session '{name}'…"),
            _ => format!("Switching to session '{}'…", node.session_name()),
        },
        PendingAction::KillSession => format!("Killing session '{}'…", node.session_name()),
        PendingAction::CloseTab => match node {
            TreeNode::Tab { name, .. } => format!("Closing tab '{name}'…"),
            _ => return None,
        },
        PendingAction::DetachSession => unreachable!(),
    })
}

/// Empty the host attachment without leaving a stale command, title, runtime
/// marker or durable last-session record that could reconnect it on restart.
fn empty_workspace(state: &mut AppState) -> Result<()> {
    let name = state.config.workspace.pane_default.clone();
    actions::reset_workspace(&name)?;
    state.active_session = None;
    state.workspace_focus = None;
    state.workspace_pane_name = Some(name);
    // Invalidate remembered native focus until a fresh attachment is registered.
    state.apply_focus(None);
    actions::forget_workspace_session()?;
    // Persist the replacement immediately so an old serialized nested attach
    // cannot reconnect a deliberately emptied Workspace on host resurrection.
    crate::session::zellij_action(&["action", "save-session"])?;
    state.rebuild_nodes();
    Ok(())
}

fn follow_session_action(state: &mut AppState, removed: &str, behavior: AfterSessionAction) -> Result<()> {
    if behavior == AfterSessionAction::Empty {
        return Ok(());
    }
    let statuses = crate::session::checked_session_statuses()?;
    if let Some(target) = state.nearest_live_session(removed, &statuses) {
        let name = state.format_workspace_pane_name(&target);
        actions::switch_session(None, &target, None, Some(&name))?;
        state.active_session = Some(target.clone());
        state.workspace_pane_name = Some(name);
        state.rebuild_nodes();
        if let Some(index) = state.nodes.iter().position(|node| matches!(node, TreeNode::Session { name, .. } if name == &target)) {
            state.select_index(index);
        }
    }
    Ok(())
}

fn invalidate_navigation(state: &mut AppState, monitor: Option<&mut visits::Monitor>) {
    if let Some(monitor) = monitor {
        monitor.invalidate();
    }
    if let Some(queue) = state.activation.as_mut() {
        queue.invalidate();
    }
}

fn dispatch_pending_action(
    state: &mut AppState,
    action: PendingAction,
    monitor: Option<&mut visits::Monitor>,
) -> Result<()> {
    if action == PendingAction::Activate {
        return dispatch_action(state, monitor);
    }
    invalidate_navigation(state, monitor);
    let result = (|| -> Result<()> {
        if action == PendingAction::DetachSession {
            let Some(target) = state.active_session.clone() else { return Ok(()); };
            empty_workspace(state)?;
            return follow_session_action(state, &target, state.config.tui.after_detach);
        }
        match state.selected_node().cloned() {
            Some(TreeNode::Session { name, needs_resurrection: false, .. }) if action == PendingAction::KillSession => {
                let attached = state.active_session.as_deref() == Some(&name);
                actions::kill_session(&name)?;
                if let Some(session) = state.sessions.iter_mut().find(|session| session.name == name) {
                    session.needs_resurrection = true;
                }
                if attached {
                    empty_workspace(state)?;
                    follow_session_action(state, &name, state.config.tui.after_kill)?;
                }
            }
            Some(TreeNode::Tab { session_name, name, position, .. }) if action == PendingAction::CloseTab => {
                let last_tab = actions::close_tab(&session_name, position, &name)?;
                if let Some(session) = state.sessions.iter_mut().find(|session| session.name == session_name) {
                    session.tabs.retain(|tab| tab.position != position);
                    session.needs_resurrection = last_tab;
                }
                if last_tab && state.active_session.as_deref() == Some(&session_name) {
                    empty_workspace(state)?;
                    follow_session_action(state, &session_name, state.config.tui.after_kill)?;
                }
            }
            _ => {}
        }
        Ok(())
    })();
    let focus_result = actions::focus_sidebar();
    state.error = result.and(focus_result).err().map(|error| format!("{error:#}"));
    state.rebuild_nodes();
    state.cursor = state.cursor.min(state.nodes.len().saturating_sub(1));
    state.sync_list_state();
    Ok(())
}

/// Translate the currently selected `TreeNode` into session/tab switch action.
fn dispatch_action(state: &mut AppState, monitor: Option<&mut visits::Monitor>) -> Result<()> {
    let Some(node) = state.selected_node().cloned() else {
        return Ok(());
    };
    // New navigation intent invalidates older passive visit work before either
    // queue can dispatch a follow-up or publish a stale focus/ack snapshot.
    if let Some(monitor) = monitor {
        monitor.invalidate();
    }
    if let state::TreeNode::AgentPane {
        view, session_name, ..
    } = &node
    {
        if !view.navigable {
            state.error =
                Some("Agent ownership is unavailable; no activation or acknowledgement".into());
            return Ok(());
        }
        let Some(host) = std::env::var("VERIJ_HOST_MARKER_KEY")
            .ok()
            .filter(|key| verij_types::identity::valid_record_key(key))
        else {
            state.error = Some("Agent navigation requires a registered host binding".into());
            return Ok(());
        };
        let target = crate::activation::AgentTarget {
            host,
            instance: view.instance.clone(),
            pane: view.pane.clone(),
            revision: (view.status == verij_types::agent::AgentStatus::Done)
                .then_some(view.completion_revision),
        };
        let queue = state
            .activation
            .get_or_insert_with(crate::activation::Queue::start);
        if let Err(error) = queue.enqueue_agent(session_name.clone(), target) {
            state.error = Some(error.to_string());
        }
        return Ok(());
    }

    let target_session = node.session_name().to_string();
    let tab_position = node.tab_position();

    let old_active = state.active_session.clone();

    // Session switches can use current metadata immediately. Tab switches update
    // the name from the next filesystem snapshot after Zellij changes focus.
    let workspace_pane_name = if tab_position.is_none() {
        Some(state.format_workspace_pane_name(&target_session))
    } else {
        None
    };

    let queue = state
        .activation
        .get_or_insert_with(crate::activation::Queue::start);
    if let Err(error) = queue.enqueue(
        old_active,
        target_session,
        tab_position,
        workspace_pane_name,
    ) {
        state.error = Some(error.to_string());
    }

    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::widgets::ListState;
    use verij_types::{SessionSnapshot, TabSnapshot};

    fn action_state() -> AppState {
        let mut state = AppState::default();
        state.reconcile(["alpha", "beta", "gamma"].into_iter().map(|name| SessionSnapshot {
            name: name.into(), is_current: false, tabs: vec![
                TabSnapshot { name: "first".into(), position: 0, is_active: true },
                TabSnapshot { name: "second".into(), position: 1, is_active: false },
             ], active_pane: None, connected_clients: Some(1), needs_resurrection: false,
             inventory: None,
        }).collect());
        state.active_session = Some("beta".into());
        state.rebuild_nodes();
        state
    }

    #[test]
    fn shortcuts_keep_vim_navigation_and_scope_lifecycle_actions() {
        let mut state = action_state();
        let key = |code| KeyEvent::new(KeyCode::Char(code), KeyModifiers::empty());
        handle_key(&mut state, key('J'), None).unwrap();
        assert_eq!(state.cursor, 3);
        handle_key(&mut state, key('K'), None).unwrap();
        assert_eq!(state.cursor, 0);
        handle_key(&mut state, key('j'), None).unwrap();
        assert_eq!(state.cursor, 1);
        handle_key(&mut state, key('k'), None).unwrap();
        assert_eq!(state.cursor, 0);
        handle_key(&mut state, key('x'), None).unwrap();
        assert_eq!(state.pending_action.take(), Some(PendingAction::KillSession));
        handle_key(&mut state, key('j'), None).unwrap();
        handle_key(&mut state, key('x'), None).unwrap();
        assert_eq!(state.pending_action.take(), Some(PendingAction::CloseTab));
        handle_key(&mut state, key('d'), None).unwrap();
        assert_eq!(state.pending_action.take(), Some(PendingAction::DetachSession));
        assert_eq!(action_progress(&state, PendingAction::DetachSession).unwrap(), "Detaching session 'beta'…");
        assert_eq!(state.selected_node().unwrap().session_name(), "alpha");

        handle_key(&mut state, KeyEvent::new(KeyCode::Char('d'), KeyModifiers::CONTROL), None).unwrap();
        assert!(state.pending_action.is_none());
        let mut repeated = key('x');
        repeated.kind = KeyEventKind::Repeat;
        handle_key(&mut state, repeated, None).unwrap();
        assert!(state.pending_action.is_none());

        state.active_session = None;
        handle_key(&mut state, key('d'), None).unwrap();
        assert!(state.pending_action.is_none());
        state.sessions[0].needs_resurrection = true;
        state.rebuild_nodes();
        state.select_index(0);
        handle_key(&mut state, key('x'), None).unwrap();
        assert!(state.pending_action.is_none());
        assert!(handle_key(&mut state, key('q'), None).unwrap());
    }

    #[cfg(unix)]
    #[test]
    fn lifecycle_actions_target_selected_objects_and_preserve_host_state_on_failure() {
        use std::fs;
        use std::os::unix::fs::PermissionsExt;
        use std::path::PathBuf;
        use std::process::Command;

        let Some(root) = std::env::var_os("VERIJ_LIFECYCLE_FIXTURE").map(PathBuf::from) else {
            // Run mocked process/state tests in a separate test process. Other
            // parallel tests must never inherit its PATH or host environment.
            let root = std::env::temp_dir().join(format!("verij-lifecycle-{}-{}", std::process::id(),
                std::time::SystemTime::now().duration_since(std::time::UNIX_EPOCH).unwrap().as_nanos()));
            fs::create_dir_all(root.join("bin")).unwrap();
            fs::create_dir_all(root.join("state/verij")).unwrap();
            fs::create_dir_all(root.join("runtime/verij")).unwrap();
            let script = root.join("bin/zellij");
            fs::write(&script, r#"#!/bin/sh
printf '%s\n' "$*" >> "$VERIJ_LIFECYCLE_FIXTURE/commands"
case "$1" in
  list-sessions) cat "$VERIJ_LIFECYCLE_FIXTURE/sessions"; exit 0 ;;
  kill-session)
    if [ "${VERIJ_MUTATION_FAIL:-}" = "1" ]; then echo 'kill failed diagnostic' >&2; exit 1; fi
    if [ "$2" = "alpha" ]; then
      printf '%s\n' 'alpha [Created 1m ago] (EXITED)' 'beta [Created 1m ago]' 'gamma [Created 1m ago]' > "$VERIJ_LIFECYCLE_FIXTURE/sessions"
    else
      printf '%s\n' 'alpha [Created 1m ago]' 'beta [Created 1m ago] (EXITED)' 'gamma [Created 1m ago]' > "$VERIJ_LIFECYCLE_FIXTURE/sessions"
    fi
    exit 0 ;;
  --session|-s) shift 2 ;;
esac
case "$2" in
  list-panes)
    id=$(cat "$VERIJ_LIFECYCLE_FIXTURE/pane-id")
    printf '[{"id":0,"is_plugin":false,"pane_x":0,"tab_id":0,"title":"Verij"},{"id":%s,"is_plugin":false,"pane_x":30,"tab_id":0,"title":"Workspace"}]\n' "$id" ;;
  new-pane)
    id=$(cat "$VERIJ_LIFECYCLE_FIXTURE/pane-id")
    id=$((id+1))
    printf '%s\n' "$id" > "$VERIJ_LIFECYCLE_FIXTURE/pane-id"
    printf 'terminal_%s\n' "$id" ;;
  list-tabs) cat "$VERIJ_LIFECYCLE_FIXTURE/tabs" ;;
  focus-pane-id) echo 'Pane Terminal(0) is already focused' >&2; exit 2 ;;
  *) echo 'captured action diagnostic'; echo 'captured error diagnostic' >&2 ;;
esac
"#).unwrap();
            fs::set_permissions(&script, fs::Permissions::from_mode(0o755)).unwrap();
            let output = Command::new(std::env::current_exe().unwrap())
                .args(["--exact", "tui::tests::lifecycle_actions_target_selected_objects_and_preserve_host_state_on_failure", "--nocapture"])
                .env("VERIJ_LIFECYCLE_FIXTURE", &root)
                .env("XDG_STATE_HOME", root.join("state"))
                .env("XDG_RUNTIME_DIR", root.join("runtime"))
                .env("VERIJ_CONTROL_DIR", root.join("control"))
                .env("ZELLIJ_BIN", &script)
                .env_remove("ZELLIJ_CONFIG_FILE")
                .env("ZELLIJ_SESSION_NAME", "host")
                .env("VERIJ_HOST_NAME", "host")
                .env("VERIJ_HOST_MARKER_KEY", "stable")
                .env("ZELLIJ_PANE_ID", "0")
                .env("PATH", format!("{}:{}", root.join("bin").display(), std::env::var("PATH").unwrap()))
                .output().unwrap();
            fs::remove_dir_all(root).unwrap();
            assert!(output.status.success(), "{}{}", String::from_utf8_lossy(&output.stdout), String::from_utf8_lossy(&output.stderr));
            assert!(!String::from_utf8_lossy(&output.stdout).contains("captured action diagnostic"));
            assert!(!String::from_utf8_lossy(&output.stderr).contains("captured error diagnostic"));
            return;
        };

        let reset = || {
            fs::write(root.join("commands"), "").unwrap();
            fs::write(root.join("pane-id"), "1").unwrap();
            fs::write(root.join("sessions"), "alpha [Created 1m ago]\nbeta [Created 1m ago]\ngamma [Created 1m ago]\n").unwrap();
            fs::write(root.join("tabs"), r#"[{"tab_id":90,"position":0,"name":"first"},{"tab_id":91,"position":1,"name":"second"}]"#).unwrap();
            fs::write(root.join("state/verij/hosts.toml"), "[hosts.host]\nlast_session = 'beta'\n[hosts.other]\nlast_session = 'alpha'\n").unwrap();
            fs::write(root.join("runtime/verij/workspace-stable.session"), "beta").unwrap();
            action_state()
        };
        let commands = || fs::read_to_string(root.join("commands")).unwrap();

        // Global d ignores selected alpha tab; empty policy forgets beta durably.
        let mut state = reset();
        state.select_index(2);
        state.config.tui.after_detach = AfterSessionAction::Empty;
        dispatch_pending_action(&mut state, PendingAction::DetachSession, None).unwrap();
        assert!(state.error.is_none(), "{:?}", state.error);
        assert!(state.active_session.is_none());
        assert!(actions::workspace_session().is_none());
        assert!(crate::config::last_host_session("host").is_none());
        assert_eq!(crate::config::last_host_session("other").as_deref(), Some("alpha"));
        assert!(commands().contains("--pane-id terminal_1 --no-focus"));
        assert!(commands().contains("action save-session"));
        assert!(!commands().contains("action detach"), "must not detach other clients");
        assert!(commands().ends_with("action focus-pane-id terminal_0\n"));

        // Default policy opens nearest live gamma through the replacement pane.
        let mut state = reset();
        dispatch_pending_action(&mut state, PendingAction::DetachSession, None).unwrap();
        assert!(state.error.is_none(), "{:?}", state.error);
        assert_eq!(state.active_session.as_deref(), Some("gamma"));
        assert_eq!(crate::config::last_host_session("host").as_deref(), Some("gamma"));
        assert!(commands().contains("action write-chars --pane-id terminal_2"));
        assert!(commands().contains(" workspace 'gamma' --host 'stable'"));

        // Killing unattached alpha leaves current beta Workspace intact.
        let mut state = reset();
        dispatch_pending_action(&mut state, PendingAction::KillSession, None).unwrap();
        assert!(state.error.is_none(), "{:?}", state.error);
        assert_eq!(state.active_session.as_deref(), Some("beta"));
        assert!(commands().contains("kill-session alpha\n"));
        assert!(!commands().contains("new-pane"));

        // Killing attached beta triggers configured nearest replacement.
        let mut state = reset();
        state.select_index(3);
        dispatch_pending_action(&mut state, PendingAction::KillSession, None).unwrap();
        assert!(state.error.is_none(), "{:?}", state.error);
        assert_eq!(state.active_session.as_deref(), Some("gamma"));
        assert!(commands().contains("kill-session beta\n"));

        // Failed kill must not erase attachment or replace any host pane.
        let mut state = reset();
        state.select_index(3);
        std::env::set_var("VERIJ_MUTATION_FAIL", "1");
        dispatch_pending_action(&mut state, PendingAction::KillSession, None).unwrap();
        std::env::remove_var("VERIJ_MUTATION_FAIL");
        assert!(state.error.as_deref().unwrap().contains("kill failed diagnostic"));
        assert_eq!(state.active_session.as_deref(), Some("beta"));
        assert_eq!(actions::workspace_session().as_deref(), Some("beta"));
        assert_eq!(crate::config::last_host_session("host").as_deref(), Some("beta"));
        assert!(!commands().contains("new-pane"));

        // Closing alpha's inactive second tab resolves its stable ID, not beta's active tab.
        let mut state = reset();
        state.select_index(2);
        dispatch_pending_action(&mut state, PendingAction::CloseTab, None).unwrap();
        assert!(state.error.is_none(), "{:?}", state.error);
        assert_eq!(state.active_session.as_deref(), Some("beta"));
        assert!(commands().contains("--session alpha action close-tab-by-id 91\n"));
        assert!(!commands().contains("go-to-tab"));
        assert!(!commands().contains("new-pane"));

        // Stale tab position/name must not close a different tab.
        let mut state = reset();
        state.select_index(1);
        fs::write(root.join("tabs"), r#"[{"tab_id":91,"position":0,"name":"second"}]"#).unwrap();
        dispatch_pending_action(&mut state, PendingAction::CloseTab, None).unwrap();
        assert!(state.error.as_deref().unwrap().contains("Selected tab changed"));
        assert!(!commands().contains("close-tab-by-id"));

        // Closing attached session's last tab follows after_kill=empty.
        let mut state = reset();
        state.config.tui.after_kill = AfterSessionAction::Empty;
        state.select_index(4);
        fs::write(root.join("tabs"), r#"[{"tab_id":7,"position":0,"name":"first"}]"#).unwrap();
        dispatch_pending_action(&mut state, PendingAction::CloseTab, None).unwrap();
        assert!(state.error.is_none(), "{:?}", state.error);
        assert!(state.active_session.is_none());
        assert!(crate::config::last_host_session("host").is_none());
        assert!(commands().contains("--session beta action close-tab-by-id 7\n"));

        // Lifecycle mutation supersedes an already-running activation before
        // that worker can publish focus, acknowledgement or an old result.
        let mut state = reset();
        let entered = std::sync::Arc::new(std::sync::Barrier::new(2));
        let release = std::sync::Arc::new(std::sync::Barrier::new(2));
        let stale_followup = std::sync::Arc::new(std::sync::atomic::AtomicBool::new(false));
        let worker_entered = entered.clone();
        let worker_release = release.clone();
        let worker_marked = stale_followup.clone();
        let mut queue = crate::activation::Queue::start_with_executor(move |_, current| {
            worker_entered.wait();
            worker_release.wait();
            if current() { worker_marked.store(true, std::sync::atomic::Ordering::Release); }
            Ok(crate::activation::Outcome::Legacy)
        });
        queue.enqueue(Some("beta".into()), "alpha".into(), None, None).unwrap();
        entered.wait();
        state.activation = Some(queue);
        dispatch_pending_action(&mut state, PendingAction::KillSession, None).unwrap();
        release.wait();
        let queue = state.activation.as_ref().unwrap();
        assert!(!queue.pending);
        assert!(queue.receiver.recv_timeout(Duration::from_millis(100)).is_err());
        assert!(!stale_followup.load(std::sync::atomic::Ordering::Acquire));
        assert_eq!(state.active_session.as_deref(), Some("beta"));
    }

    #[test]
    fn confirming_new_session_queues_creation_before_progress_draw() {
        let mut state = AppState::default();
        state.start_new_session_prompt();
        state.input_buffer = "o24".into();
        handle_key(&mut state, KeyEvent::new(KeyCode::Enter, KeyModifiers::empty()), None).unwrap();
        assert_eq!(state.creation_pending.as_deref(), Some("o24"));
        assert_eq!(state.input_mode, InputMode::Normal);
        assert!(state.active_session.is_none());
    }

    #[test]
    fn exited_session_click_queues_action_so_progress_can_render_first() {
        let mut state = AppState::default();
        state.reconcile(vec![SessionSnapshot {
            name: "exited".into(), is_current: false, tabs: vec![],
            active_pane: None, connected_clients: None, needs_resurrection: true,
            inventory: None,
        }]);
        let click = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 5, row: 0, modifiers: KeyModifiers::empty(),
        };
        handle_mouse(&mut state, click, &mut None, None).unwrap();
        assert_eq!(state.pending_action, Some(PendingAction::Activate));
        assert_eq!(state.selected_node().unwrap().session_name(), "exited");
        assert!(state.active_session.is_none(), "switch must wait until after progress draw");

        state.pending_action = None;
        handle_key(&mut state, KeyEvent::new(KeyCode::Enter, KeyModifiers::empty()), None).unwrap();
        assert_eq!(state.pending_action, Some(PendingAction::Activate));
    }

    #[test]
    fn delayed_monitor_snapshot_cannot_regress_activation_acknowledgement() {
        use verij_types::agent::InstanceAck;
        let ack = |revision, time| InstanceAck {
            acknowledged_revision: revision,
            acknowledged_at_ms: time,
            turn_id: None,
            request_id: format!("request-{revision}"),
        };
        let mut current = std::collections::BTreeMap::from([("agent-a".into(), ack(2, 20))]);
        merge_acknowledgements(
            &mut current,
            [
                ("agent-a".into(), ack(1, 30)),
                ("agent-b".into(), ack(1, 10)),
            ],
        );
        assert_eq!(current["agent-a"].acknowledged_revision, 2);
        assert_eq!(current["agent-b"].acknowledged_revision, 1);
        merge_acknowledgements(&mut current, []);
        assert_eq!(current.len(), 2);
        merge_acknowledgements(&mut current, [("agent-a".into(), ack(3, 40))]);
        assert_eq!(current["agent-a"].acknowledged_revision, 3);
    }

    #[test]
    fn mouse_fold_uses_scrolled_row_and_rendered_marker_columns() {
        let mut state = AppState::default();
        state.reconcile(
            ["first", "second"]
                .into_iter()
                .map(|name| SessionSnapshot {
                    name: name.into(),
                    is_current: false,
                    tabs: vec![],
                    active_pane: None,
                    connected_clients: None,
                    needs_resurrection: false,
                    inventory: None,
                })
                .collect(),
        );
        state.list_state = ListState::default().with_offset(1);
        state.fold_hitboxes = vec![Some(0..2), Some(4..7)];
        let click = MouseEvent {
            kind: MouseEventKind::Down(MouseButton::Left),
            column: 5,
            row: 0,
            modifiers: KeyModifiers::empty(),
        };
        handle_mouse(&mut state, click, &mut None, None).unwrap();
        assert!(state.collapsed.contains("second"));
        assert!(!state.collapsed.contains("first"));
    }
}
