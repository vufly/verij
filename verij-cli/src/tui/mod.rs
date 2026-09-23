/// verij-cli/src/tui/mod.rs
///
/// The Ratatui event loop for `verij ui`.
///
/// Handles:
///   1. Keyboard input (navigation, fold/unfold, actions, shortcuts).
///   2. Mouse input (click to select, double-click to attach, wheel scroll).
///   3. Session snapshot messages from the filesystem watcher (`/tmp/verij/states/`).
///   4. Tick-based spinner animation for empty/connecting states.
///   5. Dynamic terminal resize handling.
pub mod render;
pub mod state;

use anyhow::{Context, Result};
use crossterm::{
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event as CEvent, KeyCode, KeyEvent,
        KeyModifiers, MouseButton, MouseEvent, MouseEventKind,
    },
    execute,
    terminal::{disable_raw_mode, enable_raw_mode, EnterAlternateScreen, LeaveAlternateScreen},
};
use ratatui::{backend::CrosstermBackend, Terminal};
use std::io;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

use crate::actions;
use crate::config::Config;
use crate::fs_watcher;
use state::{AppState, InputMode};

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

/// Launch the Verij TUI.
pub async fn run(config: Config) -> Result<()> {
    // Ensure host session pane frame style is titles
    let _ = std::process::Command::new("zellij")
        .args(["action", "set-pane-frame-style", "titles"])
        .stdin(std::process::Stdio::null())
        .status();

    enable_raw_mode().context("Failed to enable raw mode")?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)
        .context("Failed to enter alternate screen")?;

    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend).context("Failed to create terminal")?;
    terminal.clear()?;

    let result = event_loop(&mut terminal, config).await;

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

async fn event_loop(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>, config: Config) -> Result<()> {
    let (tx, mut rx) = mpsc::channel(16);
    let mut state = AppState::with_config(config);
    state.plugin_path = crate::layout::resolve_plugin_path(None).ok();

    // Spawn filesystem watcher background task targeting /tmp/verij/states/
    let prefix = state.config.prefix().to_string();
    match fs_watcher::spawn_fs_watcher(tx, prefix) {
        Ok(_handle) => {}
        Err(e) => {
            state.error = Some(format!("FS watcher error: {e}"));
        }
    }

    // Initial render
    terminal.draw(|frame| render::render(frame, &mut state))?;

    let mut last_click: Option<(Instant, usize)> = None;

    loop {
        let mut needs_render = false;

        // Poll for terminal events (100 ms timeout for animation & responsiveness)
        if event::poll(Duration::from_millis(100)).context("Failed to poll terminal events")? {
            match event::read().context("Failed to read terminal event")? {
                CEvent::Key(key) => {
                    if handle_key(&mut state, key)? {
                        break;
                    }
                    needs_render = true;
                }
                CEvent::Mouse(mouse) => {
                    handle_mouse(&mut state, mouse, &mut last_click)?;
                    needs_render = true;
                }
                CEvent::Resize(_cols, _rows) => {
                    terminal.clear()?;
                    needs_render = true;
                }
                _ => {}
            }
        }

        // Drain pending session updates from the filesystem watcher
        loop {
            match rx.try_recv() {
                Ok(snapshot) => {
                    state.error = None;
                    state.reconcile(snapshot);
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

        // Tick counter for animations (e.g. connecting spinner)
        state.tick = state.tick.wrapping_add(1);
        if state.nodes.is_empty() {
            needs_render = true;
        }

        if needs_render {
            terminal.draw(|frame| render::render(frame, &mut state))?;
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Input handlers
// ---------------------------------------------------------------------------

/// Process keyboard events. Returns `true` if TUI should quit.
fn handle_key(state: &mut AppState, key: KeyEvent) -> Result<bool> {
    if state.input_mode == InputMode::NewSession {
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
                    let old_active = state.active_session.clone();
                    let res = actions::create_inner_session(
                        old_active.as_deref(),
                        &name,
                        state.plugin_path.as_deref(),
                    );
                    match res {
                        Ok(()) => {
                            state.active_session = Some(name);
                            state.error = None;
                        }
                        Err(e) => {
                            state.error = Some(format!("Create error: {e}"));
                        }
                    }
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

        // Help bar toggle
        KeyCode::Char('?') => {
            state.toggle_help();
        }

        // Navigation (Vim & Arrow keys)
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
            dispatch_action(state)?;
        }

        _ => {}
    }

    Ok(false)
}

/// Process mouse events (click, double click, scroll wheel).
fn handle_mouse(
    state: &mut AppState,
    mouse: MouseEvent,
    last_click: &mut Option<(Instant, usize)>,
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
                state.select_index(target_index);

                // Check for double click within 400ms
                let now = Instant::now();
                if let Some((prev_time, prev_idx)) = *last_click {
                    if prev_idx == target_index
                        && now.duration_since(prev_time) < Duration::from_millis(400)
                    {
                        dispatch_action(state)?;
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

/// Translate the currently selected `TreeNode` into session/tab switch action.
fn dispatch_action(state: &mut AppState) -> Result<()> {
    let Some(node) = state.selected_node() else {
        return Ok(());
    };

    let target_session = node.session_name().to_string();
    let tab_position = node.tab_position();

    let old_active = state.active_session.clone();
    state.active_session = Some(target_session.clone());

    // Only rename the Workspace tab when switching at session level (not tab navigation).
    // For tab clicks, the session is already attached — no rename needed.
    let workspace_tab_name: Option<String> = if tab_position.is_none() {
        Some(state.config.workspace.format_tab_name(&target_session))
    } else {
        None
    };

    let result = actions::switch_session(
        old_active.as_deref(),
        &target_session,
        tab_position,
        workspace_tab_name.as_deref(),
    );

    if let Err(e) = result {
        state.error = Some(e.to_string());
    } else {
        state.error = None;
    }

    state.rebuild_nodes();
    state.sync_list_state();

    Ok(())
}
