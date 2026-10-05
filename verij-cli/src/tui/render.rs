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

    // Only allocate status bar row if user toggled help with '?' or an error exists
    let show_bottom_bar = state.show_help || state.error.is_some();
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
                // Determine first and last tab using explicit RowMeta if available, fallback to lookahead.
                let is_first_tab = match node {
                    TreeNode::Tab { meta, .. } => {
                        if meta.sibling_count > 0 {
                            meta.sibling_index == 0
                        } else {
                            i > 0 && matches!(state.nodes.get(i - 1), Some(TreeNode::Session { .. }))
                        }
                    }
                    _ => false,
                };
                let is_last_tab = match node {
                    TreeNode::Tab { session_name, meta, .. } => {
                        if meta.sibling_count > 0 {
                            meta.sibling_index + 1 == meta.sibling_count
                        } else {
                            let next_is_same_session = state.nodes.get(i + 1).map(|next| matches!(next, TreeNode::Tab { session_name: ns, .. } if ns == session_name)).unwrap_or(false);
                            !next_is_same_session
                        }
                    }
                    _ => false,
                };
                let session_index = match node {
                    TreeNode::Session { session_index, .. }
                    | TreeNode::Tab { session_index, .. }
                    | TreeNode::AgentPane { session_index, .. } => *session_index,
                };
                let (item, hitbox) = state.tree_formatter.render_at_tick(
                    node,
                    state.sessions.get(session_index),
                    i == state.cursor,
                    state.active_session.as_deref() == Some(node.session_name()),
                    is_first_tab,
                    is_last_tab,
                    state.tick,
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
    if let Some(err) = &state.error {
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
    } else if state.show_help {
        Span::styled(
            " j/k: nav  h/l: fold/expand  Space: toggle (parent tab on agent)  Enter: activate  ?: hide  q: quit",
            Style::default().fg(c(colors.muted)),
        )
    } else if let Some(TreeNode::AgentPane { view, .. }) = state.selected_node() {
        let placement = if view.is_floating {
            " [floating]"
        } else if view.stacked == Some(true) {
            " [stacked]"
        } else {
            ""
        };
        let fixture = if view.is_synthetic { " [fixture]" } else { "" };
        let detail = if !view.detail.is_empty() {
            format!(" · {}", view.detail)
        } else {
            String::new()
        };
        Span::styled(
            format!(
                " {} · pane {}{}{}{} (Enter: activate, Space: parent tab)",
                view.title, view.pane.terminal.0, placement, fixture, detail
            ),
            Style::default().fg(c(colors.muted)),
        )
    } else {
        Span::styled(
            " j/k: nav  h/l: fold  Space: toggle  Enter: attach  n: new  R: rename host  ?: help  q: quit",
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
