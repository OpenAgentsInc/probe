use ratatui::Frame;
use ratatui::layout::{Alignment, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Text};
use ratatui::widgets::{Block, Borders, Padding, Paragraph, Wrap};

use crate::event::UiEvent;

const PLACEHOLDER: &str = "Type a Probe message. Enter submits. Shift+Enter inserts a newline.";
const MAX_VISIBLE_COMPOSER_LINES: usize = 4;
const MAX_HISTORY_ENTRIES: usize = 24;
const ATTACHMENT_LIBRARY: [&str; 3] = ["README.md", "Cargo.toml", "docs/README.md"];

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

    fn title_state_label(&self) -> Option<&'static str> {
        match self {
            Self::Active => None,
            Self::Busy(_) => Some("busy"),
            Self::Disabled(_) => Some("locked"),
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
    history: Vec<DraftSnapshot>,
    history_index: Option<usize>,
    stashed_snapshot: Option<DraftSnapshot>,
}

impl ComposerState {
    fn insert_char(&mut self, ch: char) {
        self.prepare_for_edit();
        self.text.insert(self.cursor, ch);
        self.cursor += ch.len_utf8();
    }

    fn insert_text(&mut self, value: &str) {
        self.prepare_for_edit();
        self.text.insert_str(self.cursor, value);
        self.cursor += value.len();
        if value.contains('\n') || value.chars().count() > 24 {
            self.pasted_multiline = true;
        }
    }

    fn insert_newline(&mut self) {
        self.prepare_for_edit();
        self.text.insert(self.cursor, '\n');
        self.cursor += 1;
    }

    fn backspace(&mut self) {
        self.prepare_for_edit();
        if let Some((start, end)) = previous_grapheme_bounds(self.text.as_str(), self.cursor) {
            self.text.replace_range(start..end, "");
            self.cursor = start;
        }
    }

    fn delete(&mut self) {
        self.prepare_for_edit();
        if let Some((start, end)) = next_grapheme_bounds(self.text.as_str(), self.cursor) {
            self.text.replace_range(start..end, "");
        }
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
    }

    fn title_segments(&self) -> Vec<String> {
        let mut parts = vec![match slash_command(self.text.as_str()) {
            Some(command) => format!("/{command}"),
            None => String::from("plain"),
        }];
        if !self.attachments.is_empty() {
            parts.push(format!("attach {}", self.attachments.len()));
        }
        if self.pasted_multiline {
            parts.push(String::from("paste"));
        }
        if !self.history.is_empty() {
            parts.push(format!("hist {}", self.history.len()));
        }
        parts
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
            self.composer.line_count().min(MAX_VISIBLE_COMPOSER_LINES) as u16
        };
        content_lines + 2
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
                self.composer.recall_previous();
                None
            }
            UiEvent::ComposerHistoryNext => {
                self.composer.recall_next();
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
            UiEvent::ComposerSubmit => self.composer.submit(),
            _ => None,
        }
    }

    fn title_text(&self, state: &BottomPaneState, runtime_status: &str) -> String {
        let mut parts = Vec::new();
        if let Some(label) = state.title_state_label() {
            parts.push(label.to_string());
        }
        parts.extend(self.composer.title_segments());
        if !runtime_status.is_empty() {
            parts.push(runtime_status.to_string());
        }
        parts.join(" | ")
    }

    pub fn render(
        &self,
        frame: &mut Frame<'_>,
        area: Rect,
        runtime_status: &str,
        state: &BottomPaneState,
    ) {
        let title = self.title_text(state, runtime_status);
        let composer_block = Block::default()
            .borders(Borders::ALL)
            .padding(Padding::horizontal(1))
            .title(Line::from(format!(" {title} ")).alignment(Alignment::Right))
            .style(shell_border());
        let composer_inner = composer_block.inner(area);
        frame.render_widget(composer_block, area);

        let content = if let Some(helper_copy) = state.helper_copy() {
            Text::from(Line::styled(
                helper_copy.to_string(),
                Style::default()
                    .fg(Color::Rgb(0x91, 0xae, 0xc4))
                    .add_modifier(Modifier::DIM),
            ))
        } else {
            let lines = if self.composer.text.is_empty() {
                vec![Line::styled(
                    PLACEHOLDER,
                    Style::default()
                        .fg(Color::Rgb(0x91, 0xae, 0xc4))
                        .add_modifier(Modifier::DIM),
                )]
            } else {
                self.composer
                    .visible_lines()
                    .into_iter()
                    .map(Line::from)
                    .collect()
            };
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
        let composer_inner = Block::default()
            .borders(Borders::ALL)
            .padding(Padding::horizontal(1))
            .inner(area);
        let visible_lines = self.composer.visible_lines();
        let visible_start_row = self
            .composer
            .line_count()
            .saturating_sub(visible_lines.len());
        let (row, col) = self.composer.cursor_row_col();
        let row = row.saturating_sub(visible_start_row);
        Some((
            composer_inner.x.saturating_add(col as u16),
            composer_inner.y.saturating_add(row as u16),
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
}
