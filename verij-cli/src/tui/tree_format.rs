//! Compiled tmux-like formats for the navigable session/tab tree.
use std::ops::Range;

use ratatui::{
    style::{Color, Modifier, Style},
    text::{Line, Span},
    widgets::ListItem,
};
use verij_types::SessionSnapshot;

use crate::config::{
    ColorConfig, TreeConfig, TreeRowStyles, DEFAULT_AGENT_FORMAT, DEFAULT_SESSION_FORMAT,
    DEFAULT_TAB_FORMAT,
};

use super::state::{RowMeta, TreeNode};
use verij_types::agent::AgentStatus;

#[derive(Debug, Clone)]
enum Token {
    Text(String),
    Variable(String),
    Conditional {
        flag: String,
        yes: Vec<Token>,
        no: Vec<Token>,
    },
    Style(Style),
    Reset,
}

#[derive(Debug, Clone)]
struct CompiledStyles {
    normal: Style,
    selected: Style,
    active: Style,
    both: Style,
}

impl CompiledStyles {
    fn compile(
        config: &TreeRowStyles,
        defaults: &TreeRowStyles,
        colors: &ColorConfig,
        name: &str,
    ) -> Self {
        let parse = |field: &str, value: &str, fallback: &str| {
            parse_style(value, colors).unwrap_or_else(|error| {
                eprintln!("[verij] Invalid {name}.{field}: {error}; using default");
                parse_style(fallback, colors).expect("valid default tree style")
            })
        };
        Self {
            normal: parse("normal", &config.normal, &defaults.normal),
            selected: parse("selected", &config.selected, &defaults.selected),
            active: parse("active", &config.active, &defaults.active),
            both: parse("both", &config.both, &defaults.both),
        }
    }

    fn for_state(&self, selected: bool, active: bool) -> Style {
        match (selected, active) {
            (false, false) => self.normal,
            (true, false) => self.selected,
            (false, true) => self.active,
            (true, true) => self.both,
        }
    }
}

#[derive(Debug, Clone)]
pub struct TreeFormatter {
    session: Vec<Token>,
    tab: Vec<Token>,
    agent: Vec<Token>,
    session_styles: CompiledStyles,
    tab_styles: CompiledStyles,
    agent_styles: CompiledStyles,
    fold_collapsed: String,
    fold_expanded: String,
    branch_first: String,
    branch_middle: String,
    branch_last: String,
}

impl Default for TreeFormatter {
    fn default() -> Self {
        Self::new(&TreeConfig::default(), &ColorConfig::default())
    }
}

impl TreeFormatter {
    pub fn new(config: &TreeConfig, colors: &ColorConfig) -> Self {
        let defaults = TreeConfig::default();
        let compile = |value: &str, fallback: &str, name: &str| {
            parse_template(value, colors).unwrap_or_else(|error| {
                eprintln!("[verij] Invalid [tui.tree].{name}: {error}; using default");
                parse_template(fallback, colors).expect("valid default tree format")
            })
        };
        let glyph = |value: &str, fallback: &str, name: &str| {
            if value.contains(['\n', '\r']) {
                eprintln!(
                    "[verij] Invalid [tui.tree].{name}: glyph must be one line; using default"
                );
                fallback.to_string()
            } else {
                value.to_string()
            }
        };
        Self {
            session: compile(
                &config.session_format,
                DEFAULT_SESSION_FORMAT,
                "session_format",
            ),
            tab: compile(&config.tab_format, DEFAULT_TAB_FORMAT, "tab_format"),
            agent: compile(&config.agent_format, DEFAULT_AGENT_FORMAT, "agent_format"),
            session_styles: CompiledStyles::compile(
                &config.session_styles,
                &defaults.session_styles,
                colors,
                "session_styles",
            ),
            tab_styles: CompiledStyles::compile(
                &config.tab_styles,
                &defaults.tab_styles,
                colors,
                "tab_styles",
            ),
            agent_styles: CompiledStyles::compile(
                &config.agent_styles,
                &defaults.agent_styles,
                colors,
                "agent_styles",
            ),
            fold_collapsed: glyph(
                &config.fold_collapsed,
                &defaults.fold_collapsed,
                "fold_collapsed",
            ),
            fold_expanded: glyph(
                &config.fold_expanded,
                &defaults.fold_expanded,
                "fold_expanded",
            ),
            branch_first: glyph(&config.branch_first, &defaults.branch_first, "branch_first"),
            branch_middle: glyph(
                &config.branch_middle,
                &defaults.branch_middle,
                "branch_middle",
            ),
            branch_last: glyph(&config.branch_last, &defaults.branch_last, "branch_last"),
        }
    }

    pub fn render_at_tick(
        &self,
        node: &TreeNode,
        snapshot: Option<&SessionSnapshot>,
        selected: bool,
        attached: bool,
        first_tab: bool,
        last_tab: bool,
        tick: usize,
    ) -> (ListItem<'static>, Option<Range<u16>>) {
        let active = match node {
            TreeNode::Session { is_attached, .. } => *is_attached,
            TreeNode::Tab {
                is_workspace_active,
                ..
            } => *is_workspace_active,
            TreeNode::AgentPane { view, .. } => view.is_active,
        };
        let (tokens, styles) = match node {
            TreeNode::Session { .. } => (&self.session, &self.session_styles),
            TreeNode::Tab { .. } => (&self.tab, &self.tab_styles),
            TreeNode::AgentPane { .. } => (&self.agent, &self.agent_styles),
        };
        let base = styles.for_state(selected, active);
        let context = RowContext {
            node,
            snapshot,
            selected,
            active,
            attached,
            first_tab,
            last_tab,
            tick,
            formatter: self,
        };
        let mut output = RowOutput {
            spans: Vec::new(),
            style: base,
            base,
            column: 0,
            fold_range: None,
            after_fold: false,
        };
        output.emit(tokens, &context);
        (
            ListItem::new(Line::from(output.spans)).style(base),
            output.fold_range,
        )
    }

    #[allow(dead_code)]
    pub fn render(
        &self,
        node: &TreeNode,
        snapshot: Option<&SessionSnapshot>,
        selected: bool,
        attached: bool,
        first_tab: bool,
        last_tab: bool,
    ) -> (ListItem<'static>, Option<Range<u16>>) {
        self.render_at_tick(node, snapshot, selected, attached, first_tab, last_tab, 0)
    }
}

struct RowContext<'a> {
    node: &'a TreeNode,
    snapshot: Option<&'a SessionSnapshot>,
    selected: bool,
    active: bool,
    attached: bool,
    first_tab: bool,
    last_tab: bool,
    tick: usize,
    formatter: &'a TreeFormatter,
}

impl RowContext<'_> {
    fn spinner_char(&self) -> &'static str {
        const SPINNER_FRAMES: &[&str] = &["⠋", "⠙", "⠹", "⠸", "⠼", "⠴", "⠦", "⠧", "⠇", "⠏"];
        SPINNER_FRAMES[(self.tick / 2) % SPINNER_FRAMES.len()]
    }

    fn summary_string(&self) -> String {
        let summary = self.node.summary();
        let mut parts = Vec::new();
        if summary.error > 0 {
            parts.push(format!("×{}", summary.error));
        }
        if summary.needs_input > 0 {
            parts.push(format!("!{}", summary.needs_input));
        }
        if summary.done > 0 {
            parts.push(format!("✓{}", summary.done));
        }
        if summary.working > 0 {
            parts.push(format!("{}{}", self.spinner_char(), summary.working));
        }
        if summary.unknown > 0 {
            parts.push(format!("?{}", summary.unknown));
        }
        parts.join(" ")
    }

    fn compute_tree_prefix(&self, meta: &RowMeta) -> String {
        let mut prefix = String::new();
        if meta.depth >= 2 {
            let continuations = if meta.ancestor_continuations.len() >= meta.depth {
                &meta.ancestor_continuations[1..meta.depth]
            } else {
                &meta.ancestor_continuations[..]
            };
            for &cont in continuations {
                if cont {
                    prefix.push_str("│  ");
                } else {
                    prefix.push_str("   ");
                }
            }
            let needed = (meta.depth - 1).saturating_sub(continuations.len());
            for _ in 0..needed {
                prefix.push_str("   ");
            }
        }
        prefix
    }

    fn flag(&self, flag: &str) -> bool {
        match flag {
            "selected" => self.selected,
            "active" => self.active,
            "attached" => self.attached,
            "collapsed" => match self.node {
                TreeNode::Session { is_collapsed, .. } => *is_collapsed,
                TreeNode::Tab {
                    is_collapsed,
                    has_children,
                    ..
                } => *has_children && *is_collapsed,
                TreeNode::AgentPane { .. } => false,
            },
            "expanded" => match self.node {
                TreeNode::Session { is_collapsed, .. } => !*is_collapsed,
                TreeNode::Tab {
                    is_collapsed,
                    has_children,
                    ..
                } => *has_children && !*is_collapsed,
                TreeNode::AgentPane { .. } => false,
            },
            "first_tab" => match self.node {
                TreeNode::Tab { meta, .. } => {
                    if meta.sibling_count > 0 {
                        meta.sibling_index == 0
                    } else {
                        self.first_tab
                    }
                }
                _ => false,
            },
            "last_tab" => match self.node {
                TreeNode::Tab { meta, .. } => {
                    if meta.sibling_count > 0 {
                        meta.sibling_index + 1 == meta.sibling_count
                    } else {
                        self.last_tab
                    }
                }
                _ => false,
            },
            "first_agent" => match self.node {
                TreeNode::AgentPane { meta, .. } => {
                    if meta.sibling_count > 0 {
                        meta.sibling_index == 0
                    } else {
                        true
                    }
                }
                _ => false,
            },
            "last_agent" => match self.node {
                TreeNode::AgentPane { meta, .. } => {
                    if meta.sibling_count > 0 {
                        meta.sibling_index + 1 == meta.sibling_count
                    } else {
                        true
                    }
                }
                _ => false,
            },
            "exited" => self.snapshot.is_some_and(|s| s.needs_resurrection),
            "has_tabs" => self.snapshot.is_some_and(|s| !s.tabs.is_empty()),
            "has_active_tab" => self
                .snapshot
                .and_then(SessionSnapshot::active_tab)
                .is_some(),
            "has_agents" => match self.node {
                TreeNode::Session { summary, .. } => summary.total > 0,
                TreeNode::Tab {
                    has_children,
                    summary,
                    ..
                } => *has_children || summary.total > 0,
                TreeNode::AgentPane { .. } => false,
            },
            "agent_summary" => !self.summary_string().is_empty(),
            "working" => match self.node {
                TreeNode::AgentPane { view, .. } => view.status == AgentStatus::Working,
                _ => false,
            },
            "needs_input" => match self.node {
                TreeNode::AgentPane { view, .. } => view.status == AgentStatus::NeedsInput,
                _ => false,
            },
            "done" => match self.node {
                TreeNode::AgentPane { view, .. } => view.status == AgentStatus::Done,
                _ => false,
            },
            "idle" => match self.node {
                TreeNode::AgentPane { view, .. } => view.status == AgentStatus::Idle,
                _ => false,
            },
            "error" => match self.node {
                TreeNode::AgentPane { view, .. } => view.status == AgentStatus::Error,
                _ => false,
            },
            "unknown" => match self.node {
                TreeNode::AgentPane { view, .. } => view.status == AgentStatus::Unknown,
                _ => false,
            },
            "floating" => match self.node {
                TreeNode::AgentPane { view, .. } => view.is_floating,
                _ => false,
            },
            "stacked" => match self.node {
                TreeNode::AgentPane { view, .. } => view.stacked == Some(true),
                _ => false,
            },
            _ => false,
        }
    }

    fn value(&self, name: &str) -> String {
        match name {
            "session_name" => self.node.session_name().to_string(),
            "tab_name" => match self.node {
                TreeNode::Tab { name, .. } => name.clone(),
                TreeNode::AgentPane { tab_name, .. } => tab_name.clone(),
                _ => String::new(),
            },
            "tab_position" => self
                .node
                .tab_position()
                .map(|i| i.to_string())
                .unwrap_or_default(),
            "tab_number" => match self.node {
                TreeNode::Tab { position, .. } => self
                    .snapshot
                    .and_then(|s| s.tabs.iter().position(|t| t.position == *position))
                    .map(|i| i + 1)
                    .unwrap_or(position + 1)
                    .to_string(),
                TreeNode::AgentPane { tab_position, .. } => (tab_position + 1).to_string(),
                _ => String::new(),
            },
            "tab_count" => self
                .snapshot
                .map(|s| s.tabs.len().to_string())
                .unwrap_or_default(),
            "active_tab_name" => self
                .snapshot
                .and_then(SessionSnapshot::active_tab)
                .map(|t| t.name.clone())
                .unwrap_or_default(),
            "fold_marker" => match self.node {
                TreeNode::Session {
                    is_collapsed: true, ..
                } => self.formatter.fold_collapsed.clone(),
                TreeNode::Session { .. } => self.formatter.fold_expanded.clone(),
                TreeNode::Tab {
                    has_children: true,
                    is_collapsed: true,
                    ..
                } => self.formatter.fold_collapsed.clone(),
                TreeNode::Tab {
                    has_children: true,
                    is_collapsed: false,
                    ..
                } => self.formatter.fold_expanded.clone(),
                _ => String::new(),
            },
            "branch" => match self.node {
                TreeNode::Tab { meta, .. } => {
                    let is_last = if meta.sibling_count > 0 {
                        meta.sibling_index + 1 == meta.sibling_count
                    } else {
                        self.last_tab
                    };
                    let is_first = if meta.sibling_count > 0 {
                        meta.sibling_index == 0
                    } else {
                        self.first_tab
                    };
                    if is_last {
                        self.formatter.branch_last.clone()
                    } else if is_first {
                        self.formatter.branch_first.clone()
                    } else {
                        self.formatter.branch_middle.clone()
                    }
                }
                TreeNode::AgentPane { meta, .. } => {
                    let is_last =
                        meta.sibling_count > 0 && meta.sibling_index + 1 == meta.sibling_count;
                    let is_first = meta.sibling_count > 0 && meta.sibling_index == 0;
                    if is_last {
                        self.formatter.branch_last.clone()
                    } else if is_first {
                        self.formatter.branch_first.clone()
                    } else {
                        self.formatter.branch_middle.clone()
                    }
                }
                _ => String::new(),
            },
            "tree_prefix" => match self.node {
                TreeNode::AgentPane { meta, .. } => self.compute_tree_prefix(meta),
                _ => String::new(),
            },
            "agent_name" => match self.node {
                TreeNode::AgentPane { view, .. } => {
                    let name = view.kind.to_string();
                    name
                }
                _ => String::new(),
            },
            "agent_title" => match self.node {
                TreeNode::AgentPane { view, .. } => {
                    let title = sanitize_display_text(&view.title);
                    if view.is_synthetic && !title.contains("[fixture]") {
                        format!("{title} [fixture]")
                    } else {
                        title
                    }
                }
                _ => String::new(),
            },
            "pane_title" => match self.node {
                TreeNode::AgentPane { view, .. } => sanitize_display_text(&view.pane_title),
                _ => String::new(),
            },
            "conversation_title" => match self.node {
                TreeNode::AgentPane { view, .. } => sanitize_display_text(&view.conversation_title),
                _ => String::new(),
            },
            "pane_id" => match self.node {
                TreeNode::AgentPane { view, .. } => view.pane.terminal.0.to_string(),
                _ => String::new(),
            },
            "agent_status" => match self.node {
                TreeNode::AgentPane { view, .. } => match view.status {
                    AgentStatus::Working => "working",
                    AgentStatus::NeedsInput => "needs_input",
                    AgentStatus::Done => "done",
                    AgentStatus::Idle => "idle",
                    AgentStatus::Error => "error",
                    AgentStatus::Unknown => "unknown",
                }
                .to_string(),
                _ => String::new(),
            },
            "status_icon" => match self.node {
                TreeNode::AgentPane { view, .. } => match view.status {
                    AgentStatus::Working => self.spinner_char().to_string(),
                    AgentStatus::NeedsInput => "!".to_string(),
                    AgentStatus::Done => "✓".to_string(),
                    AgentStatus::Idle => "○".to_string(),
                    AgentStatus::Error => "×".to_string(),
                    AgentStatus::Unknown => "?".to_string(),
                },
                _ => String::new(),
            },
            "status_detail" => match self.node {
                TreeNode::AgentPane { view, .. } => sanitize_display_text(&view.detail),
                _ => String::new(),
            },
            "agent_summary" => self.summary_string(),
            "agent_count" => self.node.summary().total.to_string(),
            "working_count" => self.node.summary().working.to_string(),
            "needs_input_count" => self.node.summary().needs_input.to_string(),
            "done_count" => self.node.summary().done.to_string(),
            "error_count" => self.node.summary().error.to_string(),
            "unknown_count" => self.node.summary().unknown.to_string(),
            "idle_count" => self.node.summary().idle.to_string(),
            _ => String::new(),
        }
    }
}

fn sanitize_display_text(s: &str) -> String {
    let sanitized: String = s
        .chars()
        .map(|c| if c.is_control() { ' ' } else { c })
        .collect();
    let trimmed = sanitized.trim();
    if trimmed.chars().count() > 256 {
        trimmed.chars().take(256).collect()
    } else {
        trimmed.to_string()
    }
}

struct RowOutput {
    spans: Vec<Span<'static>>,
    style: Style,
    base: Style,
    column: usize,
    fold_range: Option<Range<u16>>,
    after_fold: bool,
}

impl RowOutput {
    fn text(&mut self, text: String) {
        // Keep each navigable node on exactly one rendered line.
        let text = text.replace(['\r', '\n'], " ");
        let width = Line::raw(text.as_str()).width();
        if self.after_fold && text.starts_with(' ') {
            if let Some(range) = &mut self.fold_range {
                range.end = range.end.saturating_add(1);
            }
        }
        if !text.is_empty() {
            self.after_fold = false;
            self.spans.push(Span::styled(text, self.style));
            self.column += width;
        }
    }

    fn emit(&mut self, tokens: &[Token], context: &RowContext<'_>) {
        for token in tokens {
            match token {
                Token::Text(text) => self.text(text.clone()),
                Token::Variable(name) => {
                    let value = context.value(name);
                    let is_fold_target = match context.node {
                        TreeNode::Session { .. } => name == "fold_marker",
                        TreeNode::Tab { has_children, .. } => {
                            name == "fold_marker" && *has_children
                        }
                        TreeNode::AgentPane { .. } => false,
                    };
                    if is_fold_target && !value.is_empty() {
                        let width = Line::raw(value.as_str()).width();
                        if width > 0 {
                            let start = self.column.min(u16::MAX as usize) as u16;
                            self.fold_range = Some(
                                start..start.saturating_add(width.min(u16::MAX as usize) as u16),
                            );
                            self.after_fold = true;
                        }
                    }
                    self.text(value);
                    if is_fold_target {
                        self.after_fold = true;
                    }
                }
                Token::Conditional { flag, yes, no } => {
                    self.emit(if context.flag(flag) { yes } else { no }, context)
                }
                Token::Style(style) => self.style = self.style.patch(*style),
                Token::Reset => self.style = self.base,
            }
        }
    }
}

const VARIABLES: &[&str] = &[
    "session_name",
    "tab_name",
    "tab_position",
    "tab_number",
    "tab_count",
    "active_tab_name",
    "fold_marker",
    "branch",
    "tree_prefix",
    "agent_name",
    "agent_title",
    "pane_title",
    "conversation_title",
    "pane_id",
    "agent_status",
    "status_icon",
    "status_detail",
    "agent_summary",
    "agent_count",
    "working_count",
    "needs_input_count",
    "done_count",
    "error_count",
    "unknown_count",
    "idle_count",
];

const FLAGS: &[&str] = &[
    "selected",
    "active",
    "attached",
    "collapsed",
    "expanded",
    "first_tab",
    "last_tab",
    "exited",
    "has_tabs",
    "has_active_tab",
    "has_agents",
    "agent_summary",
    "working",
    "needs_input",
    "done",
    "idle",
    "error",
    "unknown",
    "floating",
    "stacked",
    "first_agent",
    "last_agent",
];

fn parse_template(template: &str, colors: &ColorConfig) -> Result<Vec<Token>, String> {
    if template.contains(['\n', '\r']) {
        return Err("formats must be one line".into());
    }
    let mut parser = Parser {
        input: template,
        offset: 0,
        colors,
    };
    let tokens = parser.sequence(false)?;
    Ok(tokens)
}

struct Parser<'a> {
    input: &'a str,
    offset: usize,
    colors: &'a ColorConfig,
}

impl Parser<'_> {
    fn sequence(&mut self, branch: bool) -> Result<Vec<Token>, String> {
        let mut tokens = Vec::new();
        let mut text = String::new();
        while let Some(rest) = self.input.get(self.offset..) {
            if rest.is_empty() || (branch && (rest.starts_with(',') || rest.starts_with('}'))) {
                break;
            }
            if rest.starts_with("#{") || rest.starts_with("#[") {
                if !text.is_empty() {
                    tokens.push(Token::Text(std::mem::take(&mut text)));
                }
                if rest.starts_with("#[") {
                    self.offset += 2;
                    let end = self.input[self.offset..]
                        .find(']')
                        .ok_or("unclosed style directive")?
                        + self.offset;
                    let content = &self.input[self.offset..end];
                    tokens.push(if content == "default" {
                        Token::Reset
                    } else {
                        Token::Style(parse_style(content, self.colors)?)
                    });
                    self.offset = end + 1;
                } else {
                    self.offset += 2;
                    if self.input[self.offset..].starts_with('?') {
                        self.offset += 1;
                        let end = self.input[self.offset..]
                            .find(',')
                            .ok_or("conditional missing first comma")?
                            + self.offset;
                        let flag = self.input[self.offset..end].trim();
                        if !FLAGS.contains(&flag) {
                            return Err(format!("unknown condition '{flag}'"));
                        }
                        self.offset = end + 1;
                        let yes = self.sequence(true)?;
                        if !self.input[self.offset..].starts_with(',') {
                            return Err("conditional missing else branch".into());
                        }
                        self.offset += 1;
                        let no = self.sequence(true)?;
                        if !self.input[self.offset..].starts_with('}') {
                            return Err("unclosed conditional".into());
                        }
                        self.offset += 1;
                        tokens.push(Token::Conditional {
                            flag: flag.into(),
                            yes,
                            no,
                        });
                    } else {
                        let end = self.input[self.offset..]
                            .find('}')
                            .ok_or("unclosed variable")?
                            + self.offset;
                        let name = &self.input[self.offset..end];
                        if !VARIABLES.contains(&name) {
                            return Err(format!("unknown variable '{name}'"));
                        }
                        tokens.push(Token::Variable(name.into()));
                        self.offset = end + 1;
                    }
                }
            } else if rest.starts_with("##") {
                text.push('#');
                self.offset += 2;
            } else if rest.starts_with('\\')
                && rest[1..]
                    .chars()
                    .next()
                    .is_some_and(|c| matches!(c, ',' | '}' | '\\' | '#'))
            {
                let c = rest[1..].chars().next().unwrap();
                text.push(c);
                self.offset += 1 + c.len_utf8();
            } else {
                let c = rest.chars().next().unwrap();
                text.push(c);
                self.offset += c.len_utf8();
            }
        }
        if !text.is_empty() {
            tokens.push(Token::Text(text));
        }
        Ok(tokens)
    }
}

fn parse_style(input: &str, colors: &ColorConfig) -> Result<Style, String> {
    let mut style = Style::default();
    for attr in input.split(',').map(str::trim).filter(|s| !s.is_empty()) {
        if let Some(value) = attr.strip_prefix("fg=") {
            style = style.fg(parse_color(value, colors)?);
        } else if let Some(value) = attr.strip_prefix("bg=") {
            style = style.bg(parse_color(value, colors)?);
        } else {
            let modifier = match attr {
                "bold" => Modifier::BOLD,
                "dim" => Modifier::DIM,
                "italic" => Modifier::ITALIC,
                "underline" => Modifier::UNDERLINED,
                "reverse" => Modifier::REVERSED,
                _ => return Err(format!("unknown style attribute '{attr}'")),
            };
            style = style.add_modifier(modifier);
        }
    }
    Ok(style)
}

fn parse_color(value: &str, colors: &ColorConfig) -> Result<Color, String> {
    let index = match value {
        "default" => return Ok(Color::Reset),
        "title" => colors.title,
        "session" => colors.session,
        "attached_fg" => colors.attached_fg,
        "active_bg" => colors.active_bg,
        "active_fg" => colors.active_fg,
        "selected_bg" => colors.selected_bg,
        "selected_fg" => colors.selected_fg,
        "muted" => colors.muted,
        "error" => colors.error,
        "spinner" => colors.spinner,
        "current_mark" => colors.current_mark,
        "needs_input" => colors.needs_input,
        "done" => colors.done,
        "idle" => colors.idle,
        "unknown" => colors.unknown,
        _ => value
            .parse::<u8>()
            .map_err(|_| format!("unknown color '{value}'"))?,
    };
    Ok(Color::Indexed(index))
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::tui::agent_view::{AgentSummary, AgentView};
    use crate::tui::state::NodeKey;
    use verij_types::agent::AgentKind;
    use verij_types::TabSnapshot;

    fn snapshot() -> SessionSnapshot {
        SessionSnapshot {
            name: "backend".into(),
            is_current: false,
            needs_resurrection: false,
            active_pane: None,
            connected_clients: None,
            inventory: None,
            tabs: vec![
                TabSnapshot {
                    name: "editor".into(),
                    position: 0,
                    is_active: true,
                },
                TabSnapshot {
                    name: "logs".into(),
                    position: 2,
                    is_active: false,
                },
                TabSnapshot {
                    name: "build".into(),
                    position: 3,
                    is_active: false,
                },
            ],
        }
    }

    fn session(collapsed: bool, attached: bool) -> TreeNode {
        TreeNode::Session {
            name: "backend".into(),
            is_current: false,
            is_attached: attached,
            needs_resurrection: false,
            is_collapsed: collapsed,
            active_tab: Some("editor".into()),
            tab_count: 3,
            session_index: 0,
            meta: Default::default(),
            summary: Default::default(),
        }
    }

    fn output(
        formatter: &TreeFormatter,
        node: &TreeNode,
        snap: &SessionSnapshot,
        selected: bool,
        attached: bool,
        first: bool,
        last: bool,
    ) -> RowOutput {
        output_at_tick(formatter, node, snap, selected, attached, first, last, 0)
    }

    fn output_at_tick(
        formatter: &TreeFormatter,
        node: &TreeNode,
        snap: &SessionSnapshot,
        selected: bool,
        attached: bool,
        first: bool,
        last: bool,
        tick: usize,
    ) -> RowOutput {
        let active = match node {
            TreeNode::Session { is_attached, .. } => *is_attached,
            TreeNode::Tab {
                is_workspace_active,
                ..
            } => *is_workspace_active,
            TreeNode::AgentPane { view, .. } => view.is_active,
        };
        let (tokens, styles) = match node {
            TreeNode::Session { .. } => (&formatter.session, &formatter.session_styles),
            TreeNode::Tab { .. } => (&formatter.tab, &formatter.tab_styles),
            TreeNode::AgentPane { .. } => (&formatter.agent, &formatter.agent_styles),
        };
        let base = styles.for_state(selected, active);
        let context = RowContext {
            node,
            snapshot: Some(snap),
            selected,
            active,
            attached,
            first_tab: first,
            last_tab: last,
            tick,
            formatter,
        };
        let mut output = RowOutput {
            spans: Vec::new(),
            style: base,
            base,
            column: 0,
            fold_range: None,
            after_fold: false,
        };
        output.emit(tokens, &context);
        output
    }

    fn content(row: &RowOutput) -> String {
        row.spans.iter().map(|span| span.content.as_ref()).collect()
    }

    #[test]
    fn nested_conditional_and_style_commas() {
        let tokens = parse_template(
            "#{?collapsed,#{?selected,#[fg=selected_fg,bold]a,b},c}",
            &ColorConfig::default(),
        )
        .unwrap();
        assert_eq!(tokens.len(), 1);
        assert!(parse_template("#{?bad,x,y}", &ColorConfig::default()).is_err());
        assert!(parse_template("#[fg=not_a_color]", &ColorConfig::default()).is_err());
        assert!(parse_template("#{missing}", &ColorConfig::default()).is_err());
    }

    #[test]
    fn escapes_and_default_formats_parse() {
        assert!(parse_template(DEFAULT_SESSION_FORMAT, &ColorConfig::default()).is_ok());
        assert!(parse_template(DEFAULT_TAB_FORMAT, &ColorConfig::default()).is_ok());
        let tokens =
            parse_template(r"##\,\}\\#{?active,yes\,ok,no}", &ColorConfig::default()).unwrap();
        assert!(matches!(&tokens[0], Token::Text(value) if value == "#,}\\"));
    }

    #[test]
    fn defaults_reproduce_session_states_and_badge() {
        let formatter = TreeFormatter::default();
        let snap = snapshot();
        let expanded = session(false, false);
        assert_eq!(
            content(&output(
                &formatter, &expanded, &snap, false, false, false, false
            )),
            "▽ backend (3)"
        );
        let selected = output(&formatter, &expanded, &snap, true, false, false, false);
        assert_eq!(selected.base.bg, Some(Color::Indexed(15)));
        assert_eq!(selected.spans[0].style.fg, Some(Color::Indexed(8)));
        assert_eq!(selected.spans[0].style.bg, Some(Color::Indexed(15)));
        assert_eq!(
            selected
                .spans
                .iter()
                .find(|span| span.content == "backend")
                .unwrap()
                .style
                .fg,
            Some(Color::Indexed(8))
        );

        let collapsed = session(true, false);
        let collapsed_row = output(&formatter, &collapsed, &snap, false, false, false, false);
        assert_eq!(content(&collapsed_row), "▷ backend [editor] (3 tabs)");
        assert_eq!(collapsed_row.spans[0].style.fg, None);
        assert_eq!(collapsed_row.spans[0].style.bg, None);
        let attached = session(true, true);
        let attached_row = output(&formatter, &attached, &snap, false, true, false, false);
        assert_eq!(content(&attached_row), "▷ backend  [editor]  (3 tabs)");
        assert_eq!(
            attached_row
                .spans
                .iter()
                .find(|span| span.content == "backend")
                .unwrap()
                .style
                .fg,
            Some(Color::Indexed(1))
        );
        assert_eq!(
            attached_row
                .spans
                .iter()
                .find(|span| span.content == "editor")
                .unwrap()
                .style
                .bg,
            Some(Color::Indexed(1))
        );
        assert_eq!(attached_row.fold_range, Some(0..2));

        let both = output(&formatter, &attached, &snap, true, true, false, false);
        assert_eq!(both.base.bg, Some(Color::Indexed(15)));
        assert_eq!(
            both.spans
                .iter()
                .find(|span| span.content == "editor")
                .unwrap()
                .style
                .bg,
            Some(Color::Indexed(1))
        );

        let mut empty = snap.clone();
        empty.tabs.clear();
        assert_eq!(
            content(&output(
                &formatter, &collapsed, &empty, false, false, false, false
            )),
            "▷ backend"
        );
        empty.tabs.push(TabSnapshot {
            name: "logs".into(),
            position: 0,
            is_active: false,
        });
        assert_eq!(
            content(&output(
                &formatter, &collapsed, &empty, false, false, false, false
            )),
            "▷ backend (1 tabs)"
        );

        empty.needs_resurrection = true;
        let exited = output(&formatter, &expanded, &empty, false, false, false, false);
        assert_eq!(
            exited
                .spans
                .iter()
                .find(|span| span.content == "backend")
                .unwrap()
                .style
                .fg,
            Some(Color::Indexed(8))
        );
        let exited_attached = output(
            &formatter,
            &session(false, true),
            &empty,
            false,
            true,
            false,
            false,
        );
        assert_eq!(
            exited_attached
                .spans
                .iter()
                .find(|span| span.content == "backend")
                .unwrap()
                .style
                .fg,
            Some(Color::Indexed(8))
        );
        let selected_exited_attached = output(
            &formatter,
            &session(false, true),
            &empty,
            true,
            true,
            false,
            false,
        );
        assert_eq!(
            selected_exited_attached
                .spans
                .iter()
                .find(|span| span.content == "backend")
                .unwrap()
                .style
                .fg,
            Some(Color::Indexed(1))
        );
    }

    #[test]
    fn defaults_reproduce_tab_states_and_first_branch_override() {
        let mut config = TreeConfig::default();
        config.branch_first = "╞".into();
        let formatter = TreeFormatter::new(&config, &ColorConfig::default());
        let snap = snapshot();
        let tab = TreeNode::Tab {
            session_name: "backend".into(),
            session_index: 0,
            name: "editor".into(),
            position: 0,
            is_workspace_active: false,
            meta: Default::default(),
            summary: Default::default(),
            is_collapsed: false,
            has_children: false,
        };
        assert_eq!(
            content(&output(&formatter, &tab, &snap, false, false, true, false)),
            "╞ editor"
        );
        assert_eq!(
            content(&output(&formatter, &tab, &snap, false, false, false, false)),
            "├ editor"
        );
        assert_eq!(
            content(&output(&formatter, &tab, &snap, false, false, true, true)),
            "└ editor"
        );
        let selected = output(&formatter, &tab, &snap, true, false, true, false);
        assert_eq!(selected.base.bg, Some(Color::Indexed(15)));

        let mut active = tab.clone();
        if let TreeNode::Tab {
            is_workspace_active,
            ..
        } = &mut active
        {
            *is_workspace_active = true;
        }
        let active_row = output(&formatter, &active, &snap, false, true, true, false);
        assert_eq!(content(&active_row), "╞  editor ");
        assert_eq!(active_row.base.bg, None);
        assert_eq!(active_row.spans[0].style.bg, None);
        assert_eq!(active_row.spans[0].style.fg, None);
        let badge = active_row
            .spans
            .iter()
            .find(|span| span.content == "editor")
            .unwrap();
        assert_eq!(badge.style.fg, Some(Color::Indexed(255)));
        assert_eq!(badge.style.bg, Some(Color::Indexed(1)));
        assert!(badge.style.add_modifier.contains(Modifier::BOLD));
        let both = output(&formatter, &active, &snap, true, true, true, false);
        assert_eq!(content(&both), "╞  editor ");
        assert_eq!(both.base.bg, Some(Color::Indexed(15)));
        assert_eq!(
            both.spans
                .iter()
                .find(|span| span.content == "editor")
                .unwrap()
                .style
                .bg,
            Some(Color::Indexed(1))
        );
    }

    #[test]
    fn custom_format_hitbox_literal_names_colors_and_reset() {
        let mut config = TreeConfig::default();
        config.session_format =
            "🔻  #{fold_marker} #[fg=session,bold]#{session_name}#[default]".into();
        config.fold_expanded = "🔸".into();
        let mut colors = ColorConfig::default();
        colors.session = 12;
        let formatter = TreeFormatter::new(&config, &colors);
        let mut snap = snapshot();
        snap.name = "#[fg=evil]#{active}".into();
        let mut node = session(false, false);
        if let TreeNode::Session { name, .. } = &mut node {
            *name = snap.name.clone();
        }
        let row = output(&formatter, &node, &snap, true, false, false, false);
        assert_eq!(content(&row), "🔻  🔸 #[fg=evil]#{active}");
        assert_eq!(row.fold_range, Some(4..7));
        assert_eq!(
            row.spans
                .iter()
                .find(|span| span.content == snap.name)
                .unwrap()
                .style
                .fg,
            Some(Color::Indexed(12))
        );
        assert_eq!(row.style.bg, row.base.bg);

        config.session_format = "literal text".into();
        let formatter = TreeFormatter::new(&config, &colors);
        assert_eq!(
            output(&formatter, &node, &snap, false, false, false, false).fold_range,
            None
        );
    }

    #[test]
    fn invalid_custom_format_and_style_use_defaults() {
        let mut config = TreeConfig::default();
        config.session_format = "#{unknown}".into();
        config.tab_styles.selected = "fg=missing".into();
        let formatter = TreeFormatter::new(&config, &ColorConfig::default());
        let snap = snapshot();
        assert_eq!(
            content(&output(
                &formatter,
                &session(false, false),
                &snap,
                false,
                false,
                false,
                false
            )),
            "▽ backend (3)"
        );
        assert_eq!(formatter.tab_styles.selected.fg, Some(Color::Indexed(8)));
    }

    #[test]
    fn inline_reset_restores_selected_row_base() {
        let mut config = TreeConfig::default();
        config.session_format = "#[fg=12]X#[default]Y".into();
        let formatter = TreeFormatter::new(&config, &ColorConfig::default());
        let snap = snapshot();
        let row = output(
            &formatter,
            &session(false, false),
            &snap,
            true,
            false,
            false,
            false,
        );
        assert_eq!(row.spans[0].style.fg, Some(Color::Indexed(12)));
        assert_eq!(row.spans[0].style.bg, Some(Color::Indexed(15)));
        assert_eq!(row.spans[1].style.fg, Some(Color::Indexed(8)));
        assert_eq!(row.spans[1].style.bg, Some(Color::Indexed(15)));
    }

    #[test]
    fn selected_style_changes_default_session_text() {
        let mut config = TreeConfig::default();
        config.session_styles.selected = "fg=12,bg=selected_bg".into();
        let formatter = TreeFormatter::new(&config, &ColorConfig::default());
        let snap = snapshot();
        let row = output(
            &formatter,
            &session(false, false),
            &snap,
            true,
            false,
            false,
            false,
        );
        for span in row
            .spans
            .iter()
            .filter(|span| !span.content.trim().is_empty())
        {
            assert_eq!(span.style.fg, Some(Color::Indexed(12)));
        }
    }

    use verij_types::identity::{AgentInstanceId, PaneKey, SessionInstanceId, TerminalPaneId};

    fn make_agent_view(
        agent_id: &str,
        kind: AgentKind,
        status: AgentStatus,
        active: bool,
        synthetic: bool,
    ) -> AgentView {
        AgentView {
            location: None,
            instance: AgentInstanceId(agent_id.to_string()),
            pane: PaneKey {
                session: SessionInstanceId("backend".to_string()),
                terminal: TerminalPaneId(42),
            },
            kind,
            title: format!("{kind}-task"),
            pane_title: "pane-42".to_string(),
            conversation_title: "conv-1".to_string(),
            status,
            detail: "permission".to_string(),
            is_floating: false,
            stacked: None,
            completion_revision: 1,
            is_active: active,
            is_synthetic: synthetic,
            navigable: true,
        }
    }

    #[test]
    fn formatting_nested_branches_with_agent_children() {
        let formatter = TreeFormatter::default();
        let snap = snapshot();

        // Agent 1 under non-last tab (tab has more siblings -> tab_has_more = true)
        let agent1_meta = RowMeta {
            key: NodeKey::Agent {
                pane: PaneKey {
                    session: SessionInstanceId("backend".to_string()),
                    terminal: TerminalPaneId(1),
                },
                instance: AgentInstanceId("ag-1".to_string()),
            },
            parent: None,
            depth: 2,
            sibling_index: 0,
            sibling_count: 2,
            ancestor_continuations: vec![false, true],
        };
        let agent1 = TreeNode::AgentPane {
            session_name: "backend".into(),
            session_index: 0,
            tab_name: "editor".into(),
            tab_position: 0,
            view: make_agent_view(
                "ag-1",
                AgentKind::Opencode,
                AgentStatus::Working,
                false,
                false,
            ),
            meta: agent1_meta,
        };
        let row1 = output(&formatter, &agent1, &snap, false, false, true, false);
        assert_eq!(content(&row1), "│  ├ ⠋ opencode opencode-task");

        // Agent 2 (last agent under non-last tab)
        let agent2_meta = RowMeta {
            key: NodeKey::Agent {
                pane: PaneKey {
                    session: SessionInstanceId("backend".to_string()),
                    terminal: TerminalPaneId(2),
                },
                instance: AgentInstanceId("ag-2".to_string()),
            },
            parent: None,
            depth: 2,
            sibling_index: 1,
            sibling_count: 2,
            ancestor_continuations: vec![false, true],
        };
        let agent2 = TreeNode::AgentPane {
            session_name: "backend".into(),
            session_index: 0,
            tab_name: "editor".into(),
            tab_position: 0,
            view: make_agent_view(
                "ag-2",
                AgentKind::Agy,
                AgentStatus::NeedsInput,
                false,
                false,
            ),
            meta: agent2_meta,
        };
        let row2 = output(&formatter, &agent2, &snap, false, false, false, true);
        assert_eq!(content(&row2), "│  └ ! agy agy-task");

        // Agent 3 under last tab (tab has NO more siblings -> tab_has_more = false)
        let agent3_meta = RowMeta {
            key: NodeKey::Agent {
                pane: PaneKey {
                    session: SessionInstanceId("backend".to_string()),
                    terminal: TerminalPaneId(3),
                },
                instance: AgentInstanceId("ag-3".to_string()),
            },
            parent: None,
            depth: 2,
            sibling_index: 0,
            sibling_count: 1,
            ancestor_continuations: vec![false, false],
        };
        let agent3 = TreeNode::AgentPane {
            session_name: "backend".into(),
            session_index: 0,
            tab_name: "build".into(),
            tab_position: 3,
            view: make_agent_view("ag-3", AgentKind::Opencode, AgentStatus::Done, false, false),
            meta: agent3_meta,
        };
        let row3 = output(&formatter, &agent3, &snap, false, false, true, true);
        assert_eq!(content(&row3), "   └ ✓ opencode opencode-task");
    }

    #[test]
    fn first_last_metadata() {
        let mut config = TreeConfig::default();
        config.agent_format = "#{?first_agent,F,f}#{?last_agent,LAST,last} #{branch}".into();
        let formatter = TreeFormatter::new(&config, &ColorConfig::default());
        let snap = snapshot();

        // First of two
        let agent_first = TreeNode::AgentPane {
            session_name: "backend".into(),
            session_index: 0,
            tab_name: "editor".into(),
            tab_position: 0,
            view: make_agent_view("ag-1", AgentKind::Opencode, AgentStatus::Idle, false, false),
            meta: RowMeta {
                key: NodeKey::Agent {
                    pane: PaneKey {
                        session: SessionInstanceId("backend".into()),
                        terminal: TerminalPaneId(1),
                    },
                    instance: AgentInstanceId("ag-1".into()),
                },
                parent: None,
                depth: 2,
                sibling_index: 0,
                sibling_count: 2,
                ancestor_continuations: vec![false, true],
            },
        };
        assert_eq!(
            content(&output(
                &formatter,
                &agent_first,
                &snap,
                false,
                false,
                false,
                false
            )),
            "Flast ├"
        );

        // Last of two
        let agent_last = TreeNode::AgentPane {
            session_name: "backend".into(),
            session_index: 0,
            tab_name: "editor".into(),
            tab_position: 0,
            view: make_agent_view("ag-2", AgentKind::Opencode, AgentStatus::Idle, false, false),
            meta: RowMeta {
                key: NodeKey::Agent {
                    pane: PaneKey {
                        session: SessionInstanceId("backend".into()),
                        terminal: TerminalPaneId(2),
                    },
                    instance: AgentInstanceId("ag-2".into()),
                },
                parent: None,
                depth: 2,
                sibling_index: 1,
                sibling_count: 2,
                ancestor_continuations: vec![false, true],
            },
        };
        assert_eq!(
            content(&output(
                &formatter,
                &agent_last,
                &snap,
                false,
                false,
                false,
                false
            )),
            "fLAST └"
        );

        // Single child is both first and last
        let agent_single = TreeNode::AgentPane {
            session_name: "backend".into(),
            session_index: 0,
            tab_name: "editor".into(),
            tab_position: 0,
            view: make_agent_view("ag-3", AgentKind::Opencode, AgentStatus::Idle, false, false),
            meta: RowMeta {
                key: NodeKey::Agent {
                    pane: PaneKey {
                        session: SessionInstanceId("backend".into()),
                        terminal: TerminalPaneId(3),
                    },
                    instance: AgentInstanceId("ag-3".into()),
                },
                parent: None,
                depth: 2,
                sibling_index: 0,
                sibling_count: 1,
                ancestor_continuations: vec![false, true],
            },
        };
        assert_eq!(
            content(&output(
                &formatter,
                &agent_single,
                &snap,
                false,
                false,
                false,
                false
            )),
            "FLAST └"
        );
    }

    #[test]
    fn tick_spinner() {
        let formatter = TreeFormatter::default();
        let snap = snapshot();

        let working_agent = TreeNode::AgentPane {
            session_name: "backend".into(),
            session_index: 0,
            tab_name: "editor".into(),
            tab_position: 0,
            view: make_agent_view(
                "ag-1",
                AgentKind::Opencode,
                AgentStatus::Working,
                false,
                false,
            ),
            meta: Default::default(),
        };

        // Frame changes with tick
        let tick0 = content(&output_at_tick(
            &formatter,
            &working_agent,
            &snap,
            false,
            false,
            false,
            false,
            0,
        ));
        let tick2 = content(&output_at_tick(
            &formatter,
            &working_agent,
            &snap,
            false,
            false,
            false,
            false,
            2,
        ));
        let tick4 = content(&output_at_tick(
            &formatter,
            &working_agent,
            &snap,
            false,
            false,
            false,
            false,
            4,
        ));
        assert!(tick0.contains('⠋'));
        assert!(tick2.contains('⠙'));
        assert!(tick4.contains('⠹'));

        // Done status does not change with tick
        let done_agent = TreeNode::AgentPane {
            session_name: "backend".into(),
            session_index: 0,
            tab_name: "editor".into(),
            tab_position: 0,
            view: make_agent_view("ag-2", AgentKind::Opencode, AgentStatus::Done, false, false),
            meta: Default::default(),
        };
        let done0 = content(&output_at_tick(
            &formatter,
            &done_agent,
            &snap,
            false,
            false,
            false,
            false,
            0,
        ));
        let done2 = content(&output_at_tick(
            &formatter,
            &done_agent,
            &snap,
            false,
            false,
            false,
            false,
            2,
        ));
        assert_eq!(done0, done2);
        assert!(done0.contains('✓'));
    }

    #[test]
    fn summaries_folded() {
        let formatter = TreeFormatter::default();
        let snap = snapshot();

        let mut summary = AgentSummary::default();
        summary.error = 1;
        summary.needs_input = 2;
        summary.done = 3;
        summary.working = 4;
        summary.unknown = 5;
        summary.idle = 6;
        summary.total = 21;

        let session_node = TreeNode::Session {
            name: "backend".into(),
            is_current: false,
            is_attached: false,
            needs_resurrection: false,
            is_collapsed: true,
            active_tab: Some("editor".into()),
            tab_count: 3,
            session_index: 0,
            meta: Default::default(),
            summary: summary.clone(),
        };

        // Summary order: Error NeedsInput Done Working Unknown, non-zero only
        let res = content(&output_at_tick(
            &formatter,
            &session_node,
            &snap,
            false,
            false,
            false,
            false,
            0,
        ));
        assert!(res.contains("×1 !2 ✓3 ⠋4 ?5"));

        // Tab with children and summary
        let tab_node = TreeNode::Tab {
            session_name: "backend".into(),
            session_index: 0,
            name: "editor".into(),
            position: 0,
            is_workspace_active: false,
            meta: Default::default(),
            summary,
            is_collapsed: true,
            has_children: true,
        };
        let tab_res = content(&output_at_tick(
            &formatter, &tab_node, &snap, false, false, false, false, 0,
        ));
        assert!(tab_res.contains("▷"));
        assert!(tab_res.contains("×1 !2 ✓3 ⠋4 ?5"));
    }

    #[test]
    fn selected_active_independent() {
        let formatter = TreeFormatter::default();
        let snap = snapshot();

        let view_inactive =
            make_agent_view("ag-1", AgentKind::Opencode, AgentStatus::Idle, false, false);
        let view_active =
            make_agent_view("ag-1", AgentKind::Opencode, AgentStatus::Idle, true, false);

        let node_inactive = TreeNode::AgentPane {
            session_name: "backend".into(),
            session_index: 0,
            tab_name: "editor".into(),
            tab_position: 0,
            view: view_inactive,
            meta: Default::default(),
        };
        let node_active = TreeNode::AgentPane {
            session_name: "backend".into(),
            session_index: 0,
            tab_name: "editor".into(),
            tab_position: 0,
            view: view_active,
            meta: Default::default(),
        };

        let normal = output(
            &formatter,
            &node_inactive,
            &snap,
            false,
            false,
            false,
            false,
        );
        assert_eq!(normal.base.bg, None);

        let selected_only = output(&formatter, &node_inactive, &snap, true, false, false, false);
        assert_eq!(selected_only.base.bg, Some(Color::Indexed(15)));

        let active_only = output(&formatter, &node_active, &snap, false, false, false, false);
        assert_eq!(active_only.base.bg, Some(Color::Indexed(1)));

        let both = output(&formatter, &node_active, &snap, true, false, false, false);
        assert_eq!(both.base.bg, Some(Color::Indexed(15)));
    }

    #[test]
    fn narrow_buffer_and_wide_hitboxes() {
        let formatter = TreeFormatter::default();
        let snap = snapshot();

        let session_node = session(false, false);
        let session_row = output(&formatter, &session_node, &snap, false, false, false, false);
        assert!(session_row.fold_range.is_some());

        let tab_with_children = TreeNode::Tab {
            session_name: "backend".into(),
            session_index: 0,
            name: "editor".into(),
            position: 0,
            is_workspace_active: false,
            meta: Default::default(),
            summary: AgentSummary {
                total: 1,
                working: 1,
                ..Default::default()
            },
            is_collapsed: false,
            has_children: true,
        };
        let tab_row = output(
            &formatter,
            &tab_with_children,
            &snap,
            false,
            false,
            false,
            false,
        );
        assert!(tab_row.fold_range.is_some());

        // Leaf nodes do NOT create fold targets
        let tab_leaf = TreeNode::Tab {
            session_name: "backend".into(),
            session_index: 0,
            name: "editor".into(),
            position: 0,
            is_workspace_active: false,
            meta: Default::default(),
            summary: Default::default(),
            is_collapsed: false,
            has_children: false,
        };
        let tab_leaf_row = output(&formatter, &tab_leaf, &snap, false, false, false, false);
        assert_eq!(tab_leaf_row.fold_range, None);

        let agent_leaf = TreeNode::AgentPane {
            session_name: "backend".into(),
            session_index: 0,
            tab_name: "editor".into(),
            tab_position: 0,
            view: make_agent_view("ag-1", AgentKind::Opencode, AgentStatus::Done, false, false),
            meta: Default::default(),
        };
        let agent_row = output(&formatter, &agent_leaf, &snap, false, false, false, false);
        assert_eq!(agent_row.fold_range, None);
    }

    #[test]
    fn custom_formats_and_old_default_outputs_where_zero_agents() {
        let formatter = TreeFormatter::default();
        let snap = snapshot();

        // Session with 0 agents produces exact legacy output
        let session_zero = session(false, false);
        assert_eq!(
            content(&output(
                &formatter,
                &session_zero,
                &snap,
                false,
                false,
                false,
                false
            )),
            "▽ backend (3)"
        );

        // Tab with 0 agents produces exact legacy output
        let tab_zero = TreeNode::Tab {
            session_name: "backend".into(),
            session_index: 0,
            name: "editor".into(),
            position: 0,
            is_workspace_active: false,
            meta: Default::default(),
            summary: Default::default(),
            is_collapsed: false,
            has_children: false,
        };
        assert_eq!(
            content(&output(
                &formatter, &tab_zero, &snap, false, false, false, false
            )),
            "├ editor"
        );

        // Synthetic fixtures are visibly marked [fixture]
        let synthetic_agent = TreeNode::AgentPane {
            session_name: "backend".into(),
            session_index: 0,
            tab_name: "editor".into(),
            tab_position: 0,
            view: make_agent_view(
                "ag-synth",
                AgentKind::Opencode,
                AgentStatus::Idle,
                false,
                true,
            ),
            meta: Default::default(),
        };
        let synth_row = content(&output(
            &formatter,
            &synthetic_agent,
            &snap,
            false,
            false,
            false,
            false,
        ));
        assert!(synth_row.contains("[fixture]"));
    }
}
