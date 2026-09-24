/// verij-cli/src/tui/render.rs
///
/// Ratatui rendering: draws the 2-level session/tab tree with folding, badges,
/// scrolling viewport, animated spinner empty state, and status bar.
///
/// Color design:
///   - ANSI 0-15 palette to honor user terminal themes (configurable via config.toml).
///   - Selected cursor row uses theme-neutral pair `bg=243, fg=0`.
///   - Attached/active Workspace session row uses `bg=1 (Red), fg=255` (configurable).
///   - Active tab of attached session uses same red badge.
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
    let colors = state.config.colors.clone();
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
        state
            .nodes
            .iter()
            .enumerate()
            .map(|(i, node)| {
                // Determine if this Tab node is the last sibling within its session
                let is_last_tab = if let TreeNode::Tab { session_name, .. } = node {
                    // Look ahead to next node; if next is Tab with same session, not last
                    let next_is_same_session = state.nodes.get(i + 1).map(|next| matches!(next, TreeNode::Tab { session_name: ns, .. } if ns == session_name)).unwrap_or(false);
                    !next_is_same_session
                } else {
                    false
                };
                node_to_list_item(node, i == state.cursor, is_last_tab, &colors)
            })
            .collect()
    };

    // Ensure list state selects current cursor
    state.sync_list_state();

    // highlight_style is intentionally left at default (no-op) so that span-level
    // background colours (e.g. bg=1 on the active-tab name) are not clobbered by
    // the list widget's selection overlay.  Selection styling is applied at the
    // ListItem level inside node_to_list_item instead.
    let list = List::new(items);

    // Stateful rendering handles viewport scrolling
    frame.render_stateful_widget(list, area, &mut state.list_state);
}

/// Convert a `TreeNode` into a styled `ListItem`.
fn node_to_list_item(
    node: &TreeNode,
    is_selected: bool,
    is_last_tab: bool,
    colors: &crate::config::ColorConfig,
) -> ListItem<'static> {
    let mut spans = Vec::new();

    let is_workspace_active_tab = node.is_workspace_active_tab();

    let (fold_fg, session_fg, muted_fg) = if is_selected {
        (
            c(colors.selected_fg),
            c(colors.selected_fg),
            c(colors.selected_fg),
        )
    } else {
        (c(colors.muted), c(colors.session), c(colors.muted))
    };

    match node {
        TreeNode::Session {
            name,
            is_current: _,
            is_attached,
            needs_resurrection,
            is_collapsed,
            active_tab,
            tab_count,
            ..
        } => {
            // Fold marker
            let fold_icon = if *is_collapsed { "▷ " } else { "▽ " };
            spans.push(Span::styled(
                fold_icon,
                Style::default().fg(fold_fg).add_modifier(Modifier::BOLD),
            ));

            // Exited sessions require resurrection before they can be attached.
            let name_fg = if *needs_resurrection && !is_selected {
                c(colors.muted)
            } else if *is_attached {
                c(colors.attached_fg)
            } else {
                session_fg
            };
            spans.push(Span::styled(
                name.clone(),
                Style::default().fg(name_fg).add_modifier(Modifier::BOLD),
            ));

            // Tab activity indicator on session row (when collapsed)
            if *is_collapsed {
                if let Some(tab) = active_tab {
                    spans.push(Span::raw(" "));
                    if *is_attached {
                        // Attached in workspace pane: highlight active tab badge in active colors
                        spans.push(Span::styled(
                            format!(" [{tab}] "),
                            Style::default()
                                .bg(c(colors.active_bg))
                                .fg(c(colors.active_fg))
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
            // Indent using box drawing characters
            let indent = if is_last_tab { "└ " } else { "├ " };
            spans.push(Span::raw(indent));

            if *is_workspace_active {
                if is_selected {
                    // Line is selected (bg=selected), but tab name specifically gets active colors
                    spans.push(Span::styled(
                        format!(" {name} "),
                        Style::default()
                            .bg(c(colors.active_bg))
                            .fg(c(colors.active_fg))
                            .add_modifier(Modifier::BOLD),
                    ));
                } else {
                    // Active tab of Workspace session, not selected: active fg, bold
                    spans.push(Span::styled(
                        name.clone(),
                        Style::default()
                            .fg(c(colors.active_fg))
                            .add_modifier(Modifier::BOLD),
                    ));
                }
            } else {
                let text_fg = if is_selected {
                    c(colors.selected_fg)
                } else {
                    Color::Reset
                };
                spans.push(Span::styled(name.clone(), Style::default().fg(text_fg)));
            }
        }
    }

    let line = Line::from(spans);
    let mut item = ListItem::new(line);
    if is_selected {
        item = item.style(
            Style::default()
                .bg(c(colors.selected_bg))
                .fg(c(colors.selected_fg)),
        );
    } else if is_workspace_active_tab {
        item = item.style(
            Style::default()
                .bg(c(colors.active_bg))
                .fg(c(colors.active_fg)),
        );
    }
    item
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
    } else {
        Span::styled(
            " j/k: nav  Enter: attach  n: new  R: rename host  ?: hide  q: quit",
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
