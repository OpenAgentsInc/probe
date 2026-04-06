use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Text};
use ratatui::widgets::{Block, Borders, Padding, Paragraph, Wrap};

use crate::event::UiEvent;

const PLACEHOLDER: &str = "Type a Probe message. Enter submits. Ctrl+J inserts a newline.";
const MAX_VISIBLE_COMPOSER_LINES: usize = 4;
const MAX_HISTORY_ENTRIES: usize = 24;
const ATTACHMENT_LIBRARY: [&str; 3] = ["README.md", "Cargo.toml", "docs/README.md"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SlashCommandSpec {
    name: &'static str,
    description: &'static str,
    submit_on_select: bool,
}

const SLASH_COMMANDS: [SlashCommandSpec; 13] = [
    SlashCommandSpec {
        name: "help",
        description: "Show keyboard help and shortcuts",
        submit_on_select: true,
    },
    SlashCommandSpec {
        name: "backend",
        description: "Inspect or switch the active backend lane",
        submit_on_select: true,
    },
    SlashCommandSpec {
        name: "model",
        description: "Choose the model for the active backend",
        submit_on_select: true,
    },
    SlashCommandSpec {
        name: "mcp",
        description: "Inspect integrations and MCP status",
        submit_on_select: true,
    },
    SlashCommandSpec {
        name: "reasoning",
        description: "Adjust Codex reasoning effort",
        submit_on_select: true,
    },
    SlashCommandSpec {
        name: "plan",
        description: "Toggle plan mode for the active lane",
        submit_on_select: true,
    },
    SlashCommandSpec {
        name: "cwd",
        description: "Inspect or change the current workspace",
        submit_on_select: true,
    },
    SlashCommandSpec {
        name: "approvals",
        description: "Review pending approval requests",
        submit_on_select: true,
    },
    SlashCommandSpec {
        name: "new",
        description: "Start a fresh task or session",
        submit_on_select: true,
    },
    SlashCommandSpec {
        name: "clear",
        description: "Start a fresh context on the active lane",
        submit_on_select: true,
    },
    SlashCommandSpec {
        name: "compact",
        description: "Carry forward a compact summary",
        submit_on_select: true,
    },
    SlashCommandSpec {
        name: "usage",
        description: "Inspect token usage for this lane",
        submit_on_select: true,
    },
    SlashCommandSpec {
        name: "resume",
        description: "Resume a previous Probe session",
        submit_on_select: true,
    },
];

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum BottomPaneState {
    Active,
    Busy(String),
    Disabled(String),
}

impl BottomPaneState {
    fn input_enabled(&self) -> bool {
        matches!(self, Self::Active | Self::Busy(_))
    }

    fn title(&self) -> String {
        match self {
            Self::Active => String::from("─ Composer "),
            Self::Busy(_) => String::from("─ Composer (busy) "),
            Self::Disabled(_) => String::from("─ Composer (disabled) "),
        }
    }

    fn helper_copy(&self) -> Option<&str> {
        match self {
            Self::Disabled(copy) => Some(copy.as_str()),
            Self::Active | Self::Busy(_) => None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MentionKind {
    Skill,
    App,
    RuntimeObject,
}

impl MentionKind {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::Skill => "skill",
            Self::App => "app",
            Self::RuntimeObject => "runtime",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DraftMention {
    pub kind: MentionKind,
    pub value: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DraftAttachment {
    pub label: String,
    pub source: String,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ComposerSubmission {
    pub text: String,
    pub slash_command: Option<String>,
    pub mentions: Vec<DraftMention>,
    pub attachments: Vec<DraftAttachment>,
    pub pasted_multiline: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct DraftSnapshot {
    text: String,
    attachments: Vec<DraftAttachment>,
    pasted_multiline: bool,
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct ComposerState {
    text: String,
    cursor: usize,
    attachments: Vec<DraftAttachment>,
    pasted_multiline: bool,
    slash_selection: usize,
    history: Vec<DraftSnapshot>,
    history_index: Option<usize>,
    stashed_snapshot: Option<DraftSnapshot>,
}

impl ComposerState {
    fn insert_char(&mut self, ch: char) {
        self.prepare_for_edit();
        self.text.insert(self.cursor, ch);
        self.cursor += ch.len_utf8();
        self.clamp_slash_selection();
    }

    fn insert_text(&mut self, value: &str) {
        self.prepare_for_edit();
        self.text.insert_str(self.cursor, value);
        self.cursor += value.len();
        if value.contains('\n') || value.chars().count() > 24 {
            self.pasted_multiline = true;
        }
        self.clamp_slash_selection();
    }

    fn insert_newline(&mut self) {
        self.prepare_for_edit();
        self.text.insert(self.cursor, '\n');
        self.cursor += 1;
        self.clamp_slash_selection();
    }

    fn backspace(&mut self) {
        self.prepare_for_edit();
        if let Some((start, end)) = previous_grapheme_bounds(self.text.as_str(), self.cursor) {
            self.text.replace_range(start..end, "");
            self.cursor = start;
        }
        self.clamp_slash_selection();
    }

    fn delete(&mut self) {
        self.prepare_for_edit();
        if let Some((start, end)) = next_grapheme_bounds(self.text.as_str(), self.cursor) {
            self.text.replace_range(start..end, "");
        }
        self.clamp_slash_selection();
    }

    fn move_left(&mut self) {
        if let Some((start, _)) = previous_grapheme_bounds(self.text.as_str(), self.cursor) {
            self.cursor = start;
        }
    }

    fn move_right(&mut self) {
        if let Some((_, end)) = next_grapheme_bounds(self.text.as_str(), self.cursor) {
            self.cursor = end;
        }
    }

    fn move_home(&mut self) {
        self.cursor = line_start(self.text.as_str(), self.cursor);
    }

    fn move_end(&mut self) {
        self.cursor = line_end(self.text.as_str(), self.cursor);
    }

    fn add_attachment(&mut self) {
        self.prepare_for_edit();
        let label = ATTACHMENT_LIBRARY[self.attachments.len() % ATTACHMENT_LIBRARY.len()];
        self.attachments.push(DraftAttachment {
            label: label.to_string(),
            source: String::from("local-placeholder"),
        });
        self.clamp_slash_selection();
    }

    fn recall_previous(&mut self) {
        if self.history.is_empty() {
            return;
        }
        match self.history_index {
            None => {
                self.stashed_snapshot = Some(self.snapshot());
                self.history_index = Some(self.history.len() - 1);
            }
            Some(0) => {}
            Some(index) => self.history_index = Some(index.saturating_sub(1)),
        }
        if let Some(index) = self.history_index {
            let snapshot = self.history[index].clone();
            self.restore(snapshot);
        }
    }

    fn recall_next(&mut self) {
        let Some(index) = self.history_index else {
            return;
        };
        if index + 1 < self.history.len() {
            let next_index = index + 1;
            self.history_index = Some(next_index);
            let snapshot = self.history[next_index].clone();
            self.restore(snapshot);
            return;
        }

        self.history_index = None;
        if let Some(snapshot) = self.stashed_snapshot.take() {
            self.restore(snapshot);
        }
    }

    fn submit(&mut self) -> Option<ComposerSubmission> {
        let text = self.text.trim().to_string();
        if text.is_empty() && self.attachments.is_empty() {
            return None;
        }

        let submission = ComposerSubmission {
            slash_command: slash_command(text.as_str()),
            mentions: parse_mentions(text.as_str()),
            attachments: self.attachments.clone(),
            pasted_multiline: self.pasted_multiline,
            text,
        };
        let snapshot = DraftSnapshot {
            text: submission.text.clone(),
            attachments: submission.attachments.clone(),
            pasted_multiline: submission.pasted_multiline,
        };
        self.history.push(snapshot);
        while self.history.len() > MAX_HISTORY_ENTRIES {
            self.history.remove(0);
        }

        self.clear();
        Some(submission)
    }

    fn clear(&mut self) {
        self.text.clear();
        self.cursor = 0;
        self.attachments.clear();
        self.pasted_multiline = false;
        self.slash_selection = 0;
        self.history_index = None;
        self.stashed_snapshot = None;
    }

    fn prepare_for_edit(&mut self) {
        if self.history_index.is_some() {
            self.history_index = None;
            self.stashed_snapshot = None;
        }
    }

    fn snapshot(&self) -> DraftSnapshot {
        DraftSnapshot {
            text: self.text.clone(),
            attachments: self.attachments.clone(),
            pasted_multiline: self.pasted_multiline,
        }
    }

    fn restore(&mut self, snapshot: DraftSnapshot) {
        self.text = snapshot.text;
        self.cursor = self.text.len();
        self.attachments = snapshot.attachments;
        self.pasted_multiline = snapshot.pasted_multiline;
        self.clamp_slash_selection();
    }

    fn metadata_line(&self) -> String {
        let mut parts = vec![match slash_command(self.text.as_str()) {
            Some(command) => format!("cmd: /{command}"),
            None => String::from("cmd: plain"),
        }];

        let mentions = parse_mentions(self.text.as_str());
        if !mentions.is_empty() {
            parts.push(format!(
                "mentions: {}",
                render_mentions(mentions.as_slice())
            ));
        }
        if !self.attachments.is_empty() {
            parts.push(format!(
                "attach: {}",
                self.attachments
                    .iter()
                    .map(|attachment| attachment.label.as_str())
                    .collect::<Vec<_>>()
                    .join(", ")
            ));
        }
        if self.pasted_multiline {
            parts.push(String::from("paste: multiline"));
        }
        if !self.history.is_empty() {
            parts.push(format!("hist: {}", self.history.len()));
        }

        parts.join(" | ")
    }

    fn slash_palette(&self) -> Option<Vec<SlashCommandSpec>> {
        let query = slash_command_query(self.text.as_str())?;
        let mut matches = SLASH_COMMANDS
            .iter()
            .copied()
            .filter(|command| command.name.starts_with(query))
            .collect::<Vec<_>>();
        if matches.is_empty() && !query.is_empty() {
            matches = SLASH_COMMANDS
                .iter()
                .copied()
                .filter(|command| command.name.contains(query))
                .collect::<Vec<_>>();
        }
        (!matches.is_empty()).then_some(matches)
    }

    fn clamp_slash_selection(&mut self) {
        if let Some(commands) = self.slash_palette() {
            self.slash_selection = self.slash_selection.min(commands.len().saturating_sub(1));
        } else {
            self.slash_selection = 0;
        }
    }

    fn select_previous_slash_command(&mut self) -> bool {
        let Some(commands) = self.slash_palette() else {
            return false;
        };
        if self.slash_selection == 0 {
            self.slash_selection = commands.len().saturating_sub(1);
        } else {
            self.slash_selection = self.slash_selection.saturating_sub(1);
        }
        true
    }

    fn select_next_slash_command(&mut self) -> bool {
        let Some(commands) = self.slash_palette() else {
            return false;
        };
        self.slash_selection = (self.slash_selection + 1) % commands.len();
        true
    }

    fn complete_selected_slash_command(&mut self) -> bool {
        let Some(commands) = self.slash_palette() else {
            return false;
        };
        let Some(selected) = commands.get(self.slash_selection).copied() else {
            return false;
        };
        self.text = format!("/{} ", selected.name);
        self.cursor = self.text.len();
        self.clamp_slash_selection();
        true
    }

    fn submit_selected_slash_command(&mut self) -> Option<ComposerSubmission> {
        let commands = self.slash_palette()?;
        let selected = commands.get(self.slash_selection).copied()?;
        if !selected.submit_on_select {
            return None;
        }

        let submission = ComposerSubmission {
            text: format!("/{}", selected.name),
            slash_command: Some(selected.name.to_string()),
            mentions: Vec::new(),
            attachments: Vec::new(),
            pasted_multiline: false,
        };
        let snapshot = DraftSnapshot {
            text: submission.text.clone(),
            attachments: Vec::new(),
            pasted_multiline: false,
        };
        self.history.push(snapshot);
        while self.history.len() > MAX_HISTORY_ENTRIES {
            self.history.remove(0);
        }
        self.clear();
        Some(submission)
    }

    fn visible_slash_palette(&self) -> Vec<SlashCommandSpec> {
        self.slash_palette().unwrap_or_default()
    }

    fn visible_lines(&self) -> Vec<String> {
        let mut lines = self
            .text
            .split('\n')
            .map(ToOwned::to_owned)
            .collect::<Vec<_>>();
        if lines.is_empty() {
            lines.push(String::new());
        }
        if lines.len() > MAX_VISIBLE_COMPOSER_LINES {
            lines.split_off(lines.len() - MAX_VISIBLE_COMPOSER_LINES)
        } else {
            lines
        }
    }

    fn line_count(&self) -> usize {
        self.text.split('\n').count().max(1)
    }

    fn cursor_row_col(&self) -> (usize, usize) {
        let mut row = 0usize;
        let mut col = 0usize;
        for ch in self.text[..self.cursor].chars() {
            if ch == '\n' {
                row += 1;
                col = 0;
            } else {
                col += 1;
            }
        }
        (row, col)
    }
}

#[derive(Debug, Clone, Default)]
pub struct BottomPane {
    composer: ComposerState,
}

impl BottomPane {
    #[must_use]
    pub fn new() -> Self {
        Self {
            composer: ComposerState::default(),
        }
    }

    #[must_use]
    pub fn desired_height(&self, state: &BottomPaneState) -> u16 {
        let content_lines = if state.helper_copy().is_some() {
            1u16
        } else {
            1 + self.composer.visible_slash_palette().len() as u16
                + self.composer.line_count().min(MAX_VISIBLE_COMPOSER_LINES) as u16
        };
        3 + 1 + content_lines + 2
    }

    pub fn handle_event(
        &mut self,
        event: UiEvent,
        state: &BottomPaneState,
    ) -> Option<ComposerSubmission> {
        if !state.input_enabled() {
            return None;
        }

        match event {
            UiEvent::ComposerInsert(ch) => {
                self.composer.insert_char(ch);
                None
            }
            UiEvent::ComposerBackspace => {
                self.composer.backspace();
                None
            }
            UiEvent::ComposerDelete => {
                self.composer.delete();
                None
            }
            UiEvent::ComposerMoveLeft => {
                self.composer.move_left();
                None
            }
            UiEvent::ComposerMoveRight => {
                self.composer.move_right();
                None
            }
            UiEvent::ComposerMoveHome => {
                self.composer.move_home();
                None
            }
            UiEvent::ComposerMoveEnd => {
                self.composer.move_end();
                None
            }
            UiEvent::ComposerHistoryPrevious => {
                if !self.composer.select_previous_slash_command() {
                    self.composer.recall_previous();
                }
                None
            }
            UiEvent::ComposerHistoryNext => {
                if !self.composer.select_next_slash_command() {
                    self.composer.recall_next();
                }
                None
            }
            UiEvent::ComposerAddAttachment => {
                self.composer.add_attachment();
                None
            }
            UiEvent::ComposerPaste(payload) => {
                self.composer.insert_text(payload.as_str());
                None
            }
            UiEvent::ComposerNewline => {
                self.composer.insert_newline();
                None
            }
            UiEvent::ComposerSubmit => {
                if let Some(submission) = self.composer.submit_selected_slash_command() {
                    Some(submission)
                } else if self.composer.complete_selected_slash_command() {
                    None
                } else {
                    self.composer.submit()
                }
            }
            _ => None,
        }
    }

    pub fn render(&self, frame: &mut Frame<'_>, area: Rect, status: &str, state: &BottomPaneState) {
        let rows = Layout::vertical([Constraint::Length(3), Constraint::Min(0)])
            .spacing(1)
            .split(area);

        let status_block = Block::default()
            .borders(Borders::ALL)
            .padding(Padding::horizontal(1))
            .style(shell_border());
        let status_inner = status_block.inner(rows[0]);
        frame.render_widget(status_block, rows[0]);
        frame.render_widget(
            Paragraph::new(Line::from(format!(
                "status: {status} | Tab backend | Shift+Tab Codex/back | Ctrl+R/S/A/O | F1 | Ctrl+C"
            )))
            .wrap(Wrap { trim: false }),
            status_inner,
        );

        let composer_block = Block::default()
            .borders(Borders::ALL)
            .padding(Padding::horizontal(1))
            .title(state.title())
            .style(shell_border());
        let composer_inner = composer_block.inner(rows[1]);
        frame.render_widget(composer_block, rows[1]);

        let content = if let Some(helper_copy) = state.helper_copy() {
            Text::from(Line::styled(
                helper_copy.to_string(),
                Style::default()
                    .fg(Color::Rgb(0x91, 0xae, 0xc4))
                    .add_modifier(Modifier::DIM),
            ))
        } else {
            let mut lines = vec![Line::styled(
                self.composer.metadata_line(),
                Style::default()
                    .fg(Color::Rgb(0xf1, 0xc4, 0x53))
                    .add_modifier(Modifier::DIM),
            )];
            let slash_commands = self.composer.visible_slash_palette();
            let selected_index = self.composer.slash_selection;
            lines.extend(slash_commands.iter().enumerate().map(|(index, command)| {
                let selected = index == selected_index;
                let prefix = if selected { ">" } else { " " };
                let style = if selected {
                    Style::default()
                        .fg(Color::Rgb(0x73, 0xc2, 0xfb))
                        .add_modifier(Modifier::BOLD)
                } else {
                    Style::default().fg(Color::Rgb(0x91, 0xae, 0xc4))
                };
                Line::styled(
                    format!("{prefix} /{}  {}", command.name, command.description),
                    style,
                )
            }));
            if self.composer.text.is_empty() {
                lines.push(Line::styled(
                    PLACEHOLDER,
                    Style::default()
                        .fg(Color::Rgb(0x91, 0xae, 0xc4))
                        .add_modifier(Modifier::DIM),
                ));
            } else {
                lines.extend(self.composer.visible_lines().into_iter().map(Line::from));
            }
            Text::from(lines)
        };
        frame.render_widget(
            Paragraph::new(content).wrap(Wrap { trim: false }),
            composer_inner,
        );
    }

    #[must_use]
    pub fn cursor_position(&self, area: Rect, state: &BottomPaneState) -> Option<(u16, u16)> {
        if !state.input_enabled() {
            return None;
        }
        let rows = Layout::vertical([Constraint::Length(3), Constraint::Min(0)])
            .spacing(1)
            .split(area);
        let composer_inner = Block::default()
            .borders(Borders::ALL)
            .padding(Padding::horizontal(1))
            .inner(rows[1]);
        let visible_lines = self.composer.visible_lines();
        let slash_palette_rows = self.composer.visible_slash_palette().len();
        let visible_start_row = self
            .composer
            .line_count()
            .saturating_sub(visible_lines.len());
        let (row, col) = self.composer.cursor_row_col();
        let row = row.saturating_sub(visible_start_row);
        Some((
            composer_inner.x.saturating_add(col as u16),
            composer_inner
                .y
                .saturating_add(1 + slash_palette_rows as u16 + row as u16),
        ))
    }

    #[must_use]
    pub fn current_text(&self) -> &str {
        self.composer.text.as_str()
    }
}

fn shell_border() -> Style {
    Style::default().fg(Color::Rgb(0x73, 0xc2, 0xfb))
}

fn slash_command(value: &str) -> Option<String> {
    let trimmed = value.trim_start();
    let command = trimmed.strip_prefix('/')?.split_whitespace().next()?;
    if command.is_empty() {
        None
    } else {
        Some(command.to_string())
    }
}

fn slash_command_query(value: &str) -> Option<&str> {
    let trimmed = value.trim_start();
    let remainder = trimmed.strip_prefix('/')?;
    if remainder.contains(char::is_whitespace) {
        return None;
    }
    Some(remainder)
}

fn parse_mentions(value: &str) -> Vec<DraftMention> {
    let mut mentions = Vec::new();
    for token in value.split_whitespace() {
        let normalized = token.trim_end_matches(|ch: char| {
            !ch.is_alphanumeric() && ch != ':' && ch != '-' && ch != '_'
        });
        if let Some(value) = normalized.strip_prefix("@skill:") {
            if !value.is_empty() {
                mentions.push(DraftMention {
                    kind: MentionKind::Skill,
                    value: value.to_string(),
                });
            }
        } else if let Some(value) = normalized.strip_prefix("@app:") {
            if !value.is_empty() {
                mentions.push(DraftMention {
                    kind: MentionKind::App,
                    value: value.to_string(),
                });
            }
        } else if let Some(value) = normalized.strip_prefix("@runtime:") {
            if !value.is_empty() {
                mentions.push(DraftMention {
                    kind: MentionKind::RuntimeObject,
                    value: value.to_string(),
                });
            }
        }
    }
    mentions
}

fn render_mentions(mentions: &[DraftMention]) -> String {
    mentions
        .iter()
        .map(|mention| format!("{}:{}", mention.kind.label(), mention.value))
        .collect::<Vec<_>>()
        .join(", ")
}

fn previous_grapheme_bounds(value: &str, cursor: usize) -> Option<(usize, usize)> {
    let mut last = None;
    for (index, ch) in value[..cursor].char_indices() {
        last = Some((index, index + ch.len_utf8()));
    }
    last
}

fn next_grapheme_bounds(value: &str, cursor: usize) -> Option<(usize, usize)> {
    value[cursor..]
        .char_indices()
        .next()
        .map(|(offset, ch)| (cursor + offset, cursor + offset + ch.len_utf8()))
}

fn line_start(value: &str, cursor: usize) -> usize {
    value[..cursor].rfind('\n').map_or(0, |index| index + 1)
}

fn line_end(value: &str, cursor: usize) -> usize {
    value[cursor..]
        .find('\n')
        .map_or(value.len(), |offset| cursor + offset)
}

#[cfg(test)]
mod tests {
    use super::{BottomPane, BottomPaneState};
    use crate::event::UiEvent;

    #[test]
    fn composer_handles_insert_move_and_backspace() {
        let mut pane = BottomPane::new();
        let state = BottomPaneState::Active;
        let _ = pane.handle_event(UiEvent::ComposerInsert('h'), &state);
        let _ = pane.handle_event(UiEvent::ComposerInsert('i'), &state);
        let _ = pane.handle_event(UiEvent::ComposerMoveLeft, &state);
        let _ = pane.handle_event(UiEvent::ComposerInsert('!'), &state);
        let _ = pane.handle_event(UiEvent::ComposerBackspace, &state);
        assert_eq!(pane.current_text(), "hi");
    }

    #[test]
    fn composer_supports_newline_and_submit() {
        let mut pane = BottomPane::new();
        let state = BottomPaneState::Active;
        let _ = pane.handle_event(UiEvent::ComposerInsert('a'), &state);
        let _ = pane.handle_event(UiEvent::ComposerNewline, &state);
        let _ = pane.handle_event(UiEvent::ComposerInsert('b'), &state);
        let submitted = pane.handle_event(UiEvent::ComposerSubmit, &state);
        assert_eq!(
            submitted.as_ref().map(|value| value.text.as_str()),
            Some("a\nb")
        );
        assert_eq!(pane.current_text(), "");
    }

    #[test]
    fn composer_ignores_input_when_disabled() {
        let mut pane = BottomPane::new();
        let state = BottomPaneState::Disabled(String::from("composer disabled"));
        let submitted = pane.handle_event(UiEvent::ComposerInsert('x'), &state);
        assert!(submitted.is_none());
        assert_eq!(pane.current_text(), "");
    }

    #[test]
    fn composer_history_restores_previous_submission() {
        let mut pane = BottomPane::new();
        let state = BottomPaneState::Active;

        let _ = pane.handle_event(UiEvent::ComposerInsert('f'), &state);
        let _ = pane.handle_event(UiEvent::ComposerInsert('i'), &state);
        let _ = pane.handle_event(UiEvent::ComposerInsert('r'), &state);
        let _ = pane.handle_event(UiEvent::ComposerInsert('s'), &state);
        let _ = pane.handle_event(UiEvent::ComposerInsert('t'), &state);
        let _ = pane.handle_event(UiEvent::ComposerSubmit, &state);

        let _ = pane.handle_event(UiEvent::ComposerInsert('n'), &state);
        let _ = pane.handle_event(UiEvent::ComposerInsert('e'), &state);
        let _ = pane.handle_event(UiEvent::ComposerInsert('x'), &state);
        let _ = pane.handle_event(UiEvent::ComposerInsert('t'), &state);
        let _ = pane.handle_event(UiEvent::ComposerHistoryPrevious, &state);
        assert_eq!(pane.current_text(), "first");
        let _ = pane.handle_event(UiEvent::ComposerHistoryNext, &state);
        assert_eq!(pane.current_text(), "next");
    }

    #[test]
    fn composer_submission_tracks_command_mentions_attachments_and_paste() {
        let mut pane = BottomPane::new();
        let state = BottomPaneState::Active;

        let _ = pane.handle_event(UiEvent::ComposerInsert('/'), &state);
        for ch in "plan @skill:rust @app:github".chars() {
            let _ = pane.handle_event(UiEvent::ComposerInsert(ch), &state);
        }
        let _ = pane.handle_event(UiEvent::ComposerAddAttachment, &state);
        let _ = pane.handle_event(
            UiEvent::ComposerPaste(String::from("\nalpha\nbeta")),
            &state,
        );
        let submitted = pane
            .handle_event(UiEvent::ComposerSubmit, &state)
            .expect("submission should exist");

        assert_eq!(submitted.slash_command.as_deref(), Some("plan"));
        assert_eq!(submitted.mentions.len(), 2);
        assert_eq!(submitted.attachments.len(), 1);
        assert!(submitted.pasted_multiline);
    }

    #[test]
    fn slash_palette_navigation_completes_the_selected_command_before_submit() {
        let mut pane = BottomPane::new();
        let state = BottomPaneState::Active;

        let _ = pane.handle_event(UiEvent::ComposerInsert('/'), &state);
        let _ = pane.handle_event(UiEvent::ComposerInsert('r'), &state);
        let _ = pane.handle_event(UiEvent::ComposerHistoryNext, &state);
        let submitted = pane
            .handle_event(UiEvent::ComposerSubmit, &state)
            .expect("resume should submit immediately");

        assert_eq!(submitted.text, "/resume");
        assert_eq!(submitted.slash_command.as_deref(), Some("resume"));
        assert_eq!(pane.current_text(), "");
    }

    #[test]
    fn slash_palette_can_submit_immediate_action_commands() {
        let mut pane = BottomPane::new();
        let state = BottomPaneState::Active;

        let _ = pane.handle_event(UiEvent::ComposerInsert('/'), &state);
        let _ = pane.handle_event(UiEvent::ComposerInsert('m'), &state);
        let submitted = pane
            .handle_event(UiEvent::ComposerSubmit, &state)
            .expect("model should submit immediately");

        assert_eq!(submitted.text, "/model");
        assert_eq!(submitted.slash_command.as_deref(), Some("model"));
        assert_eq!(pane.current_text(), "");
    }

    #[test]
    fn slash_palette_shows_all_available_commands_after_typing_slash() {
        let mut pane = BottomPane::new();
        let state = BottomPaneState::Active;

        let _ = pane.handle_event(UiEvent::ComposerInsert('/'), &state);

        let visible = pane.composer.visible_slash_palette();
        let names = visible
            .into_iter()
            .map(|command| command.name)
            .collect::<Vec<_>>();

        assert_eq!(
            names,
            vec![
                "help",
                "backend",
                "model",
                "mcp",
                "reasoning",
                "plan",
                "cwd",
                "approvals",
                "new",
                "clear",
                "compact",
                "usage",
                "resume",
            ]
        );
    }

    #[test]
    fn slash_palette_defers_to_history_when_no_command_list_is_open() {
        let mut pane = BottomPane::new();
        let state = BottomPaneState::Active;

        for ch in "first".chars() {
            let _ = pane.handle_event(UiEvent::ComposerInsert(ch), &state);
        }
        let _ = pane.handle_event(UiEvent::ComposerSubmit, &state);

        for ch in "next".chars() {
            let _ = pane.handle_event(UiEvent::ComposerInsert(ch), &state);
        }
        let _ = pane.handle_event(UiEvent::ComposerHistoryPrevious, &state);

        assert_eq!(pane.current_text(), "first");
    }
}
