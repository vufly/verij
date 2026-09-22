/// verij-cli/src/tui/mod.rs
///
/// The Ratatui event loop for `verij ui`.
///
/// Handles:
///   1. Keyboard input (navigation, fold/unfold, actions, shortcuts).
///   2. Mouse input (click to select, double-click to attach, wheel scroll).
///   3. Session snapshot messages from the pipe reader.
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
    terminal::{
        EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
    },
};
use ratatui::{Terminal, backend::CrosstermBackend};
use std::io;
use std::time::{Duration, Instant};
use tokio::sync::mpsc;

use crate::actions;
use crate::pipe_reader;
use state::AppState;

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

/// Launch the Verij TUI.
pub async fn run() -> Result<()> {
    enable_raw_mode().context("Failed to enable raw mode")?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)
        .context("Failed to enter alternate screen")?;

    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend).context("Failed to create terminal")?;
    terminal.clear()?;

    let result = event_loop(&mut terminal).await;

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

async fn event_loop(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) -> Result<()> {
    let (tx, mut rx) = mpsc::channel(16);
    let mut state = AppState::default();

    // Spawn pipe reader background task
    match pipe_reader::spawn_pipe_reader(tx) {
        Ok(_handle) => {}
        Err(e) => {
            state.error = Some(format!("Pipe error: {e}"));
        }
    }

    // Initial render
    terminal.draw(|frame| render::render(frame, &mut state))?;

    let mut last_click: Option<(Instant, usize)> = None;

    loop {
        let mut needs_render = false;

        // Poll for terminal events (100 ms timeout for animation & responsiveness)
        if event::poll(Duration::from_millis(100))
            .context("Failed to poll terminal events")?
        {
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

        // Drain pending session updates from pipe reader
        loop {
            match rx.try_recv() {
                Ok(snapshot) => {
                    state.error = None;
                    state.reconcile(snapshot);
                    needs_render = true;
                }
                Err(mpsc::error::TryRecvError::Empty) => break,
                Err(mpsc::error::TryRecvError::Disconnected) => {
                    state.error = Some("Pipe disconnected. Is verij-plugin running?".into());
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
    match key.code {
        // Quit
        KeyCode::Char('q') | KeyCode::Esc => return Ok(true),
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => return Ok(true),

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

        // 4.7 Tree folding
        KeyCode::Char(' ') | KeyCode::Tab => state.toggle_collapse(),
        KeyCode::Left | KeyCode::Char('h') => state.collapse_selected(),
        KeyCode::Right | KeyCode::Char('l') => state.expand_selected(),

        // Action: attach or jump to selected session/tab
        KeyCode::Enter => {
            dispatch_action(state)?;
        }

        _ => {}
    }

    Ok(false)
}

/// 4.5 Process mouse events (click, double click, scroll wheel).
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
        MouseEventKind::Down(MouseButton::Left) => {
            // In render.rs, row 0 is top border, items start on row 1
            if mouse.row >= 1 {
                let visual_index = (mouse.row - 1) as usize;
                let offset = state.list_state.offset();
                let target_index = offset + visual_index;

                if target_index < state.nodes.len() {
                    state.select_index(target_index);

                    // Check for double click within 400ms
                    let now = Instant::now();
                    if let Some((prev_time, prev_idx)) = *last_click {
                        if prev_idx == target_index && now.duration_since(prev_time) < Duration::from_millis(400) {
                            dispatch_action(state)?;
                            *last_click = None;
                            return Ok(());
                        }
                    }

                    *last_click = Some((now, target_index));
                }
            }
        }
        _ => {}
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Action dispatch
// ---------------------------------------------------------------------------

/// Translate the currently selected `TreeNode` into a Zellij action.
fn dispatch_action(state: &mut AppState) -> Result<()> {
    let Some(node) = state.selected_node() else {
        return Ok(());
    };

    let result = match node.tab_position() {
        None => actions::switch_session(node.session_name()),
        Some(position) => actions::switch_session_tab(node.session_name(), position),
    };

    if let Err(e) = result {
        state.error = Some(e.to_string());
    } else {
        state.error = None;
        state.update_attached_session();
    }

    Ok(())
}
