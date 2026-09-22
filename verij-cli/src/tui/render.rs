/// verij-cli/src/tui/render.rs
///
/// Ratatui rendering: draws the 2-level session/tab tree with folding, badges,
/// scrolling viewport, animated spinner empty state, and status bar.
///
/// Color design:
///   - ANSI 0-15 palette to honor user terminal themes.
///   - Selected cursor row uses theme-neutral pair `bg=243, fg=0`.
///   - Active tab of attached Workspace session uses `bg=1, fg=255`.
use ratatui::{
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph},
    Frame,
};

use super::state::{AppState, TreeNode};

// ---------------------------------------------------------------------------
// Colour palette (ANSI 0-15 & theme-neutral 256 pair)
// ---------------------------------------------------------------------------

const COLOR_TITLE: Color = Color::Indexed(6); // Cyan
const COLOR_BORDER: Color = Color::Indexed(8); // Muted Gray / Bright Black
const COLOR_SESSION: Color = Color::Indexed(4); // Blue
const COLOR_CURRENT_MARKER: Color = Color::Indexed(2); // Green
const COLOR_TAB_NORMAL: Color = Color::Reset; // Terminal default foreground
const COLOR_FOLD_ICON: Color = Color::Indexed(8); // Muted Gray
const COLOR_MUTED: Color = Color::Indexed(8); // Muted Gray
const COLOR_STATUS: Color = Color::Indexed(8); // Muted Gray
const COLOR_ERROR: Color = Color::Indexed(1); // Red
const COLOR_SPINNER: Color = Color::Indexed(6); // Cyan

// Selected row pair: neutral medium-gray with black text (dark & light compatible)
const COLOR_SELECTED_BG: Color = Color::Indexed(243);
const COLOR_SELECTED_FG: Color = Color::Indexed(0);

// Active tab pair: Red background with bright white text (active tab of session attached in Workspace pane)
const COLOR_ACTIVE_TAB_BG: Color = Color::Indexed(1);
const COLOR_ACTIVE_TAB_FG: Color = Color::Indexed(255);

const SPINNER_FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];

// ---------------------------------------------------------------------------
// Main render function
// ---------------------------------------------------------------------------

/// Draw the entire TUI frame from the mutable `AppState`.
pub fn render(frame: &mut Frame, state: &mut AppState) {
    let area = frame.area();
    let [list_area, status_area] = split_vertical(area, Constraint::Min(0), Constraint::Length(1));

    render_session_tree(frame, list_area, state);
    render_status_bar(frame, status_area, state);
}

// ---------------------------------------------------------------------------
// Session / tab tree
// ---------------------------------------------------------------------------

fn render_session_tree(frame: &mut Frame, area: Rect, state: &mut AppState) {
    // 4.2 Session count badge in title
    let session_count = state.sessions.len();
    let title_text = match session_count {
        0 => " Verij ".to_string(),
        1 => " Verij (1 session) ".to_string(),
        n => format!(" Verij ({n} sessions) "),
    };

    let block = Block::default()
        .title(Span::styled(
            title_text,
            Style::default()
                .fg(COLOR_TITLE)
                .add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(COLOR_BORDER));

    // Build list items
    let items: Vec<ListItem> = if state.nodes.is_empty() {
        // 4.4 Animated empty state
        let spinner_char = SPINNER_FRAMES[(state.tick / 2) % SPINNER_FRAMES.len()];
        vec![ListItem::new(Line::from(vec![
            Span::styled(
                format!("  {spinner_char} "),
                Style::default().fg(COLOR_SPINNER),
            ),
            Span::styled(
                "Connecting to verij-plugin...",
                Style::default().fg(COLOR_MUTED),
            ),
        ]))]
    } else {
        state
            .nodes
            .iter()
            .enumerate()
            .map(|(i, node)| node_to_list_item(node, i == state.cursor))
            .collect()
    };

    // Ensure list state selects current cursor
    state.sync_list_state();

    let list = List::new(items)
        .block(block)
        .highlight_style(Style::default().bg(COLOR_SELECTED_BG).fg(COLOR_SELECTED_FG));

    // 4.1 Stateful rendering handles viewport scrolling
    frame.render_stateful_widget(list, area, &mut state.list_state);
}

/// Convert a `TreeNode` into a styled `ListItem`.
fn node_to_list_item(node: &TreeNode, is_selected: bool) -> ListItem<'static> {
    let mut spans = Vec::new();

    let is_workspace_active_tab = node.is_workspace_active_tab();

    let (fold_fg, current_fg, session_fg, muted_fg) = if is_selected {
        (
            COLOR_SELECTED_FG,
            COLOR_SELECTED_FG,
            COLOR_SELECTED_FG,
            COLOR_SELECTED_FG,
        )
    } else {
        (
            COLOR_FOLD_ICON,
            COLOR_CURRENT_MARKER,
            COLOR_SESSION,
            COLOR_MUTED,
        )
    };

    match node {
        TreeNode::Session {
            name,
            is_current,
            is_attached,
            is_collapsed,
            active_tab,
            tab_count,
            ..
        } => {
            // Fold marker (4.7)
            let fold_icon = if *is_collapsed { "▸ " } else { "▾ " };
            spans.push(Span::styled(
                fold_icon,
                Style::default().fg(fold_fg).add_modifier(Modifier::BOLD),
            ));

            // Current session indicator (* for current host session)
            if *is_current {
                spans.push(Span::styled(
                    "* ",
                    Style::default().fg(current_fg).add_modifier(Modifier::BOLD),
                ));
            }

            // Session name
            spans.push(Span::styled(
                name.clone(),
                Style::default().fg(session_fg).add_modifier(Modifier::BOLD),
            ));

            // 4.3 Tab activity indicator on session row
            if *is_collapsed {
                if let Some(tab) = active_tab {
                    spans.push(Span::raw(" "));
                    if *is_attached {
                        // Attached in workspace pane: highlight active tab badge in bg=1, fg=255
                        spans.push(Span::styled(
                            format!(" [{tab}] "),
                            Style::default()
                                .bg(COLOR_ACTIVE_TAB_BG)
                                .fg(COLOR_ACTIVE_TAB_FG)
                                .add_modifier(Modifier::BOLD),
                        ));
                    } else {
                        // Background session: muted active tab name
                        spans.push(Span::styled(
                            format!("[{tab}]"),
                            Style::default().fg(muted_fg),
                        ));
                    }
                }
                if *tab_count > 0 {
                    spans.push(Span::raw(" "));
                    spans.push(Span::styled(
                        format!("({tab_count} tabs)"),
                        Style::default().fg(muted_fg),
                    ));
                }
            } else if *tab_count > 0 {
                spans.push(Span::raw(" "));
                spans.push(Span::styled(
                    format!("({tab_count})"),
                    Style::default().fg(muted_fg),
                ));
            }
        }
        TreeNode::Tab {
            name,
            is_workspace_active,
            ..
        } => {
            spans.push(Span::raw("    "));

            if *is_workspace_active {
                if is_selected {
                    // Line is selected (bg=243, fg=0), but tab name specifically gets bg=1, fg=255
                    spans.push(Span::styled(
                        "● ",
                        Style::default()
                            .fg(COLOR_SELECTED_FG)
                            .add_modifier(Modifier::BOLD),
                    ));
                    spans.push(Span::styled(
                        format!(" {name} "),
                        Style::default()
                            .bg(COLOR_ACTIVE_TAB_BG)
                            .fg(COLOR_ACTIVE_TAB_FG)
                            .add_modifier(Modifier::BOLD),
                    ));
                } else {
                    // Active tab of Workspace session, not selected: entire line styled with bg=1, fg=255
                    spans.push(Span::styled(
                        "● ",
                        Style::default()
                            .fg(COLOR_ACTIVE_TAB_FG)
                            .add_modifier(Modifier::BOLD),
                    ));
                    spans.push(Span::styled(
                        name.clone(),
                        Style::default()
                            .fg(COLOR_ACTIVE_TAB_FG)
                            .add_modifier(Modifier::BOLD),
                    ));
                }
            } else {
                let dot_fg = if is_selected {
                    COLOR_SELECTED_FG
                } else {
                    COLOR_MUTED
                };
                let text_fg = if is_selected {
                    COLOR_SELECTED_FG
                } else {
                    COLOR_TAB_NORMAL
                };
                spans.push(Span::styled("○ ", Style::default().fg(dot_fg)));
                spans.push(Span::styled(name.clone(), Style::default().fg(text_fg)));
            }
        }
    }

    let line = Line::from(spans);
    let mut item = ListItem::new(line);
    if is_selected {
        item = item.style(Style::default().bg(COLOR_SELECTED_BG).fg(COLOR_SELECTED_FG));
    } else if is_workspace_active_tab {
        item = item.style(
            Style::default()
                .bg(COLOR_ACTIVE_TAB_BG)
                .fg(COLOR_ACTIVE_TAB_FG),
        );
    }
    item
}

// ---------------------------------------------------------------------------
// Status bar
// ---------------------------------------------------------------------------

fn render_status_bar(frame: &mut Frame, area: Rect, state: &AppState) {
    let content = if let Some(err) = &state.error {
        Span::styled(
            format!(" ✗ {err}"),
            Style::default()
                .fg(COLOR_ERROR)
                .add_modifier(Modifier::BOLD),
        )
    } else {
        Span::styled(
            " j/k: nav  Space: fold  Enter: attach  q: quit",
            Style::default().fg(COLOR_STATUS),
        )
    };

    let bar = Paragraph::new(Line::from(content));
    frame.render_widget(bar, area);
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
