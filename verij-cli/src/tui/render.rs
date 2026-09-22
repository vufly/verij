/// verij-cli/src/tui/render.rs
///
/// Ratatui rendering: draws the 2-level session/tab tree and a status bar.
///
/// Layout:
///   ┌─ block (full terminal area) ──────────────────────────┐
///   │ Title: "Verij"                                        │
///   │                                                       │
///   │  (session/tab list — scrollable List widget)          │
///   │                                                       │
///   │ Status bar: keybindings or error message              │
///   └───────────────────────────────────────────────────────┘
use ratatui::{
    Frame,
    layout::{Constraint, Direction, Layout, Rect},
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::{Block, Borders, List, ListItem, ListState, Paragraph},
};

use super::state::{AppState, TreeNode};

// ---------------------------------------------------------------------------
// Colour palette (centralised for easy theming)
// ---------------------------------------------------------------------------

const COLOR_SESSION: Color = Color::Cyan;
const COLOR_TAB_ACTIVE: Color = Color::Green;
const COLOR_TAB_NORMAL: Color = Color::Gray;
const COLOR_SELECTED_BG: Color = Color::DarkGray;
const COLOR_TITLE: Color = Color::Yellow;
const COLOR_STATUS: Color = Color::DarkGray;
const COLOR_ERROR: Color = Color::Red;

// ---------------------------------------------------------------------------
// Main render function
// ---------------------------------------------------------------------------

/// Draw the entire TUI frame from the current `AppState`.
///
/// Called by the Ratatui event loop on every state change or terminal resize.
pub fn render(frame: &mut Frame, state: &AppState) {
    let area = frame.area();
    let [list_area, status_area] = split_vertical(area, Constraint::Min(0), Constraint::Length(1));

    render_session_tree(frame, list_area, state);
    render_status_bar(frame, status_area, state);
}

// ---------------------------------------------------------------------------
// Session / tab tree
// ---------------------------------------------------------------------------

fn render_session_tree(frame: &mut Frame, area: Rect, state: &AppState) {
    let block = Block::default()
        .title(Span::styled(
            " Verij ",
            Style::default()
                .fg(COLOR_TITLE)
                .add_modifier(Modifier::BOLD),
        ))
        .borders(Borders::ALL)
        .border_style(Style::default().fg(Color::DarkGray));

    let _inner = block.inner(area);

    // Build list items from the flattened node list.
    let items: Vec<ListItem> = if state.nodes.is_empty() {
        vec![ListItem::new(Line::from(Span::styled(
            "  Connecting to verij-plugin...",
            Style::default().fg(Color::DarkGray),
        )))]
    } else {
        state
            .nodes
            .iter()
            .enumerate()
            .map(|(i, node)| node_to_list_item(node, i == state.cursor))
            .collect()
    };

    // ListState tracks the highlighted row for the built-in scroll mechanism.
    let mut list_state = ListState::default();
    if !state.nodes.is_empty() {
        list_state.select(Some(state.cursor));
    }

    let list = List::new(items)
        .block(block)
        .highlight_style(Style::default().bg(COLOR_SELECTED_BG));

    frame.render_stateful_widget(list, area, &mut list_state);
}

/// Convert a `TreeNode` into a styled `ListItem`.
fn node_to_list_item(node: &TreeNode, is_selected: bool) -> ListItem<'static> {
    let label = node.display_label();

    let style = match node {
        TreeNode::Session { .. } => Style::default()
            .fg(COLOR_SESSION)
            .add_modifier(Modifier::BOLD),
        TreeNode::Tab { is_active, .. } => {
            if *is_active {
                Style::default().fg(COLOR_TAB_ACTIVE)
            } else {
                Style::default().fg(COLOR_TAB_NORMAL)
            }
        }
    };

    // Apply selection highlight on top of node-level style.
    let final_style = if is_selected {
        style.bg(COLOR_SELECTED_BG)
    } else {
        style
    };

    ListItem::new(Line::from(Span::styled(label, final_style)))
}

// ---------------------------------------------------------------------------
// Status bar
// ---------------------------------------------------------------------------

fn render_status_bar(frame: &mut Frame, area: Rect, state: &AppState) {
    let content = if let Some(err) = &state.error {
        Span::styled(
            format!(" ✗ {err}"),
            Style::default().fg(COLOR_ERROR),
        )
    } else {
        Span::styled(
            " j/k: navigate   Enter: attach   q: quit",
            Style::default().fg(COLOR_STATUS),
        )
    };

    let bar = Paragraph::new(Line::from(content));
    frame.render_widget(bar, area);
}

// ---------------------------------------------------------------------------
// Layout helpers
// ---------------------------------------------------------------------------

/// Split `area` vertically into two chunks with the given constraints.
fn split_vertical(area: Rect, top: Constraint, bottom: Constraint) -> [Rect; 2] {
    let chunks = Layout::default()
        .direction(Direction::Vertical)
        .constraints([top, bottom])
        .split(area);

    [chunks[0], chunks[1]]
}
