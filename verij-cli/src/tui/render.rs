/// verij-cli/src/tui/render.rs
///
/// Ratatui rendering: draws the 2-level session/tab tree from compiled user
/// templates, plus the scrolling viewport, animated empty state, and status bar.
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, Clear, List, ListItem, Paragraph, Wrap},
    Frame,
};

use super::state::{AppState, TreeNode};

// ---------------------------------------------------------------------------
// Color helpers
// ---------------------------------------------------------------------------

/// Convert a config u8 ANSI index into a ratatui `Color`.
#[inline]
fn c(idx: u8) -> Color {
    Color::Indexed(idx)
}

// ---------------------------------------------------------------------------
// Main render function
// ---------------------------------------------------------------------------

/// Draw the entire TUI frame from the mutable `AppState`.
pub fn render(frame: &mut Frame, state: &mut AppState) {
    let area = frame.area();

    // Allocate status rows for help, an error, or an in-progress action.
    let show_bottom_bar = state.show_help || state.error.is_some() || state.progress.is_some();
    let (list_area, status_area) = if show_bottom_bar {
        let status_height = status_bar_height(area.width, state).min(area.height);
        let [top, bottom] =
            split_vertical(area, Constraint::Min(0), Constraint::Length(status_height));
        (top, Some(bottom))
    } else {
        (area, None)
    };

    render_session_tree(frame, list_area, state);

    if let Some(status_rect) = status_area {
        render_status_bar(frame, status_rect, state);
    }

    if state.input_mode != super::state::InputMode::Normal {
        render_new_session_dialog(frame, area, state);
    }
}

// ---------------------------------------------------------------------------
// Session / tab tree
// ---------------------------------------------------------------------------

fn render_session_tree(frame: &mut Frame, area: Rect, state: &mut AppState) {
    let colors = &state.config.colors;
    let spinner_frames = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

    // Build list items (edge-to-edge compact UI without wasted border space)
    let items: Vec<ListItem> = if state.nodes.is_empty() {
        // Animated empty state
        let spinner_char = spinner_frames[(state.tick / 2) % spinner_frames.len()];
        vec![ListItem::new(Line::from(vec![
            Span::styled(
                format!("{spinner_char} "),
                Style::default().fg(c(colors.spinner)),
            ),
            Span::styled(
                "Connecting to verij-plugin...",
                Style::default().fg(c(colors.muted)),
            ),
        ]))]
    } else {
        let mut hitboxes = Vec::with_capacity(state.nodes.len());
        let items = state
            .nodes
            .iter()
            .enumerate()
            .map(|(i, node)| {
                // Determine first and last tab in display order (position need not be dense).
                let is_first_tab = matches!(node, TreeNode::Tab { .. })
                    && i > 0
                    && matches!(state.nodes.get(i - 1), Some(TreeNode::Session { .. }));
                let is_last_tab = if let TreeNode::Tab { session_name, .. } = node {
                    // Look ahead to next node; if next is Tab with same session, not last
                    let next_is_same_session = state.nodes.get(i + 1).map(|next| matches!(next, TreeNode::Tab { session_name: ns, .. } if ns == session_name)).unwrap_or(false);
                    !next_is_same_session
                } else {
                    false
                };
                let session_index = match node {
                    TreeNode::Session { session_index, .. } | TreeNode::Tab { session_index, .. } => *session_index,
                };
                let (item, hitbox) = state.tree_formatter.render(
                    node,
                    state.sessions.get(session_index),
                    i == state.cursor,
                    state.active_session.as_deref() == Some(node.session_name()),
                    is_first_tab,
                    is_last_tab,
                );
                hitboxes.push(hitbox);
                item
            })
            .collect();
        state.fold_hitboxes = hitboxes;
        items
    };

    // Ensure list state selects current cursor
    state.sync_list_state();

    // No List selection overlay: each templated row supplies its own base and spans.
    let list = List::new(items);

    // Stateful rendering handles viewport scrolling
    frame.render_stateful_widget(list, area, &mut state.list_state);
}

// ---------------------------------------------------------------------------
// Status bar
// ---------------------------------------------------------------------------

fn render_status_bar(frame: &mut Frame, area: Rect, state: &AppState) {
    let bar = Paragraph::new(Line::from(status_bar_content(state))).wrap(Wrap { trim: true });
    frame.render_widget(bar, area);
}

fn status_bar_height(width: u16, state: &AppState) -> u16 {
    Paragraph::new(Line::from(status_bar_content(state)))
        .wrap(Wrap { trim: true })
        .line_count(width)
        .max(1) as u16
}

fn status_bar_content(state: &AppState) -> Span<'static> {
    let colors = &state.config.colors;
    if let Some(progress) = &state.progress {
        Span::styled(
            format!(" ⠋ {progress}"),
            Style::default().fg(c(colors.spinner)),
        )
    } else if let Some(err) = &state.error {
        Span::styled(
            format!(" ✗ {err}"),
            Style::default()
                .fg(c(colors.error))
                .add_modifier(Modifier::BOLD),
        )
    } else if state.input_mode != super::state::InputMode::Normal {
        Span::styled(
            format!(
                " {}: {}█  (Enter: confirm, Esc: cancel)",
                if state.input_mode == super::state::InputMode::RenameHost {
                    "Rename Host"
                } else {
                    "New Session"
                },
                state.input_buffer
            ),
            Style::default()
                .fg(c(colors.title))
                .add_modifier(Modifier::BOLD),
        )
    } else {
        Span::styled(
            " j/k: nav  J/K: sessions  Enter: attach  d: detach active  x: kill session/close tab  n: new  R: rename host  ?: hide  q: quit",
            Style::default().fg(c(colors.muted)),
        )
    }
}

/// Renders a centered modal dialog for typing a new inner session name.
fn render_new_session_dialog(frame: &mut Frame, area: Rect, state: &AppState) {
    let colors = &state.config.colors;
    let dialog_width = area.width.saturating_sub(4).min(45);
    if dialog_width < 3 || area.height < 3 {
        return;
    }

    let cursor_char = if (state.tick / 3) % 2 == 0 {
        "█"
    } else {
        " "
    };
    let text = vec![
        Line::from(vec![
            Span::styled(" Name: ", Style::default().fg(c(colors.muted))),
            Span::styled(
                state.input_buffer.clone(),
                Style::default()
                    .fg(c(colors.title))
                    .add_modifier(Modifier::BOLD),
            ),
            Span::styled(cursor_char, Style::default().fg(c(colors.current_mark))),
        ]),
        Line::from(Span::styled(
            " Enter: confirm   Esc: cancel",
            Style::default().fg(c(colors.muted)),
        )),
    ];
    let dialog_height = (Paragraph::new(text.clone())
        .wrap(Wrap { trim: true })
        .line_count(dialog_width.saturating_sub(2)) as u16)
        .saturating_add(2)
        .min(area.height);
    let x = (area.width.saturating_sub(dialog_width)) / 2;
    let y = (area.height.saturating_sub(dialog_height)) / 2;
    let dialog_area = Rect::new(x, y, dialog_width, dialog_height);

    // Clear background beneath dialog
    frame.render_widget(Clear, dialog_area);

    let block = Block::default()
        .title(Span::styled(
            if state.input_mode == super::state::InputMode::RenameHost {
                " Rename Host "
            } else {
                " New Workspace Session "
            },
            Style::default()
                .fg(c(colors.title))
                .add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(c(colors.current_mark)));

    let paragraph = Paragraph::new(text).wrap(Wrap { trim: true }).block(block);
    frame.render_widget(paragraph, dialog_area);
}

// ---------------------------------------------------------------------------
// Layout helpers
// ---------------------------------------------------------------------------

fn split_vertical(area: Rect, top: Constraint, bottom: Constraint) -> [Rect; 2] {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([top, bottom])
        .split(area);

    [chunks[0], chunks[1]]
}

#[cfg(test)]
mod tests {
    use super::*;
    use ratatui::{backend::TestBackend, Terminal};
    use verij_types::SessionSnapshot;

    #[test]
    fn wrapped_progress_status_is_removed_without_leaving_text_in_tree() {
        for width in [22, 40] {
            let mut state = AppState::default();
            state.reconcile(vec![SessionSnapshot {
                name: "exited-session".into(), is_current: false, tabs: vec![],
                active_pane: None, connected_clients: None, needs_resurrection: true,
            }]);
            let mut terminal = Terminal::new(TestBackend::new(width, 8)).unwrap();
            terminal.draw(|frame| render(frame, &mut state)).unwrap();
            let original = terminal.backend().buffer().clone();

            state.progress = Some("Restoring session 'exited-session'…".into());
            terminal.draw(|frame| render(frame, &mut state)).unwrap();
            let screen: String = terminal.backend().buffer().content.iter()
                .map(|cell| cell.symbol()).collect();
            assert!(screen.contains("Restoring"), "{screen}");
            assert!(screen.contains("exited-session"), "{screen}");

            state.progress = None;
            terminal.draw(|frame| render(frame, &mut state)).unwrap();
            assert_eq!(terminal.backend().buffer(), &original);
        }
    }
}
