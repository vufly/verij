/// verij-cli/src/tui/mod.rs
///
/// The Ratatui event loop for `verij ui`.
///
/// Concurrently handles two event sources:
///   1. Terminal keyboard input (via crossterm).
///   2. Session snapshot messages from the pipe reader (via mpsc channel).
///
/// On every event it mutates `AppState`, then redraws the frame.
///
/// # Termination
///
/// The loop exits cleanly on `q`, `Escape`, or `Ctrl+C`. The terminal is
/// always restored to its original state on exit (raw mode + alternate screen
/// are torn down in a `finally`-style drop via `RestoreTerminal`).
pub mod render;
pub mod state;

use anyhow::{Context, Result};
use crossterm::{
    event::{
        self, DisableMouseCapture, EnableMouseCapture, Event as CEvent, KeyCode, KeyEvent,
        KeyModifiers,
    },
    execute,
    terminal::{
        EnterAlternateScreen, LeaveAlternateScreen, disable_raw_mode, enable_raw_mode,
    },
};
use ratatui::{Terminal, backend::CrosstermBackend};
use std::io;
use std::time::Duration;
use tokio::sync::mpsc;

use crate::actions;
use crate::pipe_reader;
use state::AppState;

// ---------------------------------------------------------------------------
// Public entry point
// ---------------------------------------------------------------------------

/// Launch the Verij TUI.
///
/// Sets up the terminal, spawns the pipe reader task, then runs the event loop
/// until the user quits. Always restores the terminal on exit.
pub async fn run() -> Result<()> {
    // Set up the crossterm terminal.
    enable_raw_mode().context("Failed to enable raw mode")?;
    let mut stdout = io::stdout();
    execute!(stdout, EnterAlternateScreen, EnableMouseCapture)
        .context("Failed to enter alternate screen")?;

    let backend = CrosstermBackend::new(stdout);
    let mut terminal = Terminal::new(backend).context("Failed to create terminal")?;
    terminal.clear()?;

    // Run the event loop; capture the result so we can restore the terminal
    // even if an error occurs.
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

/// Internal event loop body.
///
/// Drives the TUI until the user quits or a fatal error occurs.
async fn event_loop(terminal: &mut Terminal<CrosstermBackend<io::Stdout>>) -> Result<()> {
    // Channel for session snapshot updates from the pipe reader.
    // Bounded at 16: if the TUI can't keep up, we drop old snapshots rather
    // than accumulate unbounded memory.
    let (tx, mut rx) = mpsc::channel(16);

    let mut state = AppState::default();

    // Spawn the pipe reader background task.
    match pipe_reader::spawn_pipe_reader(tx) {
        Ok(_handle) => {}
        Err(e) => {
            // Pipe startup failure is non-fatal at this stage: the TUI can run
            // in a "waiting" mode displaying an error, and retry logic can be
            // added later. For now we set the error message and continue.
            state.error = Some(format!("Pipe error: {e}"));
        }
    }

    // Initial render.
    terminal.draw(|frame| render::render(frame, &state))?;

    // Event loop: select on terminal input and pipe channel messages.
    loop {
        // Poll for terminal events with a 100 ms timeout so the mpsc receiver
        // is checked at least 10 times per second even with no keyboard input.
        if event::poll(Duration::from_millis(100))
            .context("Failed to poll terminal events")?
        {
            if let CEvent::Key(key) = event::read().context("Failed to read terminal event")? {
                if handle_key(&mut state, key)? {
                    // `true` → user requested quit.
                    break;
                }
                terminal.draw(|frame| render::render(frame, &state))?;
            }
        }

        // Drain all pending snapshot messages from the pipe reader.
        // Using `try_recv` in a loop ensures we process all queued messages
        // before re-rendering, avoiding one render per message when updates
        // arrive in bursts.
        let mut got_update = false;
        loop {
            match rx.try_recv() {
                Ok(snapshot) => {
                    state.error = None; // clear any previous pipe error
                    state.reconcile(snapshot);
                    got_update = true;
                }
                Err(mpsc::error::TryRecvError::Empty) => break,
                Err(mpsc::error::TryRecvError::Disconnected) => {
                    // Pipe reader task exited — update the error display but
                    // keep the TUI alive so the user can quit gracefully.
                    state.error = Some("Pipe disconnected. Is verij-plugin loaded?".into());
                    got_update = true;
                    break;
                }
            }
        }

        if got_update {
            terminal.draw(|frame| render::render(frame, &state))?;
        }
    }

    Ok(())
}

// ---------------------------------------------------------------------------
// Key handler
// ---------------------------------------------------------------------------

/// Process a single key event.
///
/// Returns `true` if the event loop should exit.
fn handle_key(state: &mut AppState, key: KeyEvent) -> Result<bool> {
    match key.code {
        // Quit
        KeyCode::Char('q') | KeyCode::Esc => return Ok(true),
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => return Ok(true),

        // Navigation
        KeyCode::Char('j') | KeyCode::Down => state.cursor_down(),
        KeyCode::Char('k') | KeyCode::Up => state.cursor_up(),

        // Action: attach selected session / tab
        KeyCode::Enter => {
            dispatch_action(state)?;
        }

        _ => {}
    }

    Ok(false)
}

// ---------------------------------------------------------------------------
// Action dispatch
// ---------------------------------------------------------------------------

/// Translate the currently selected `TreeNode` into a Zellij action.
///
/// Errors are displayed in the TUI status bar rather than crashing the loop.
fn dispatch_action(state: &mut AppState) -> Result<()> {
    let Some(node) = state.selected_node() else {
        return Ok(());
    };

    let result = match node.tab_position() {
        None => {
            // Session node selected — switch to its first tab.
            actions::switch_session(node.session_name())
        }
        Some(position) => {
            // Tab node selected — switch session and navigate to the tab.
            actions::switch_session_tab(node.session_name(), position)
        }
    };

    if let Err(e) = result {
        state.error = Some(e.to_string());
    } else {
        state.error = None;
    }

    Ok(())
}
