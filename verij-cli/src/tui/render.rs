/// verij-cli/src/tui/render.rs
///
/// Ratatui rendering: draws the 2-level session/tab tree with folding, badges,
/// scrolling viewport, animated spinner empty state, and status bar.
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, Paragraph},
};

use super::state::{AppState, TreeNode};

// ---------------------------------------------------------------------------
// Colour palette
// ---------------------------------------------------------------------------

const COLOR_SESSION: Color = Color::Cyan;
const COLOR_CURRENT_MARKER: Color = Color::Yellow;
const COLOR_TAB_ACTIVE: Color = Color::Green;
const COLOR_TAB_NORMAL: Color = Color::Gray;
const COLOR_SELECTED_BG: Color = Color::Rgb(40, 44, 52); // Soft dark gray highlight
const COLOR_TITLE: Color = Color::Yellow;
const COLOR_STATUS: Color = Color::DarkGray;
const COLOR_ERROR: Color = Color::Red;
const COLOR_MUTED: Color = Color::DarkGray;

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
        .border_style(Style::default().fg(Color::DarkGray));

    // Build list items
    let items: Vec<ListItem> = if state.nodes.is_empty() {
        // 4.4 Animated empty state
        let spinner_char = SPINNER_FRAMES[(state.tick / 2) % SPINNER_FRAMES.len()];
        vec![ListItem::new(Line::from(vec![
            Span::styled(format!("  {spinner_char} "), Style::default().fg(Color::Yellow)),
            Span::styled("Connecting to verij-plugin...", Style::default().fg(COLOR_MUTED)),
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
        .highlight_style(Style::default().bg(COLOR_SELECTED_BG));

    // 4.1 Stateful rendering handles viewport scrolling
    frame.render_stateful_widget(list, area, &mut state.list_state);
}

/// Convert a `TreeNode` into a styled `ListItem`.
fn node_to_list_item(node: &TreeNode, is_selected: bool) -> ListItem<'static> {
    let mut spans = Vec::new();

    match node {
        TreeNode::Session {
            name,
            is_current,
            is_collapsed,
            active_tab,
            tab_count,
            ..
        } => {
            // Fold marker (4.7)
            let fold_icon = if *is_collapsed { "▸ " } else { "▾ " };
            spans.push(Span::styled(
                fold_icon,
                Style::default().fg(Color::DarkGray).add_modifier(Modifier::BOLD),
            ));

            // Current session indicator
            if *is_current {
                spans.push(Span::styled(
                    "* ",
                    Style::default().fg(COLOR_CURRENT_MARKER).add_modifier(Modifier::BOLD),
                ));
            }

            // Session name
            spans.push(Span::styled(
                name.clone(),
                Style::default().fg(COLOR_SESSION).add_modifier(Modifier::BOLD),
            ));

            // 4.3 Tab activity indicator on session row
            if *is_collapsed {
                if let Some(tab) = active_tab {
                    spans.push(Span::raw(" "));
                    spans.push(Span::styled(
                        format!("[{tab}]"),
                        Style::default().fg(COLOR_TAB_ACTIVE),
                    ));
                }
                if *tab_count > 0 {
                    spans.push(Span::raw(" "));
                    spans.push(Span::styled(
                        format!("({tab_count} tabs)"),
                        Style::default().fg(COLOR_MUTED),
                    ));
                }
            } else if *tab_count > 0 {
                spans.push(Span::raw(" "));
                spans.push(Span::styled(
                    format!("({tab_count})"),
                    Style::default().fg(COLOR_MUTED),
                ));
            }
        }
        TreeNode::Tab {
            name,
            is_active,
            ..
        } => {
            // Indentation
            spans.push(Span::raw("    "));

            // Tab active dot
            if *is_active {
                spans.push(Span::styled("● ", Style::default().fg(COLOR_TAB_ACTIVE)));
                spans.push(Span::styled(
                    name.clone(),
                    Style::default().fg(COLOR_TAB_ACTIVE).add_modifier(Modifier::BOLD),
                ));
            } else {
                spans.push(Span::styled("○ ", Style::default().fg(COLOR_MUTED)));
                spans.push(Span::styled(
                    name.clone(),
                    Style::default().fg(COLOR_TAB_NORMAL),
                ));
            }
        }
    }

    let line = Line::from(spans);
    let mut item = ListItem::new(line);
    if is_selected {
        item = item.style(Style::default().bg(COLOR_SELECTED_BG));
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
            Style::default().fg(COLOR_ERROR).add_modifier(Modifier::BOLD),
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
