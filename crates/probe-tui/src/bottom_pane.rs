use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Text};
use ratatui::widgets::{Block, Borders, Padding, Paragraph, Wrap};

use crate::event::UiEvent;

const PLACEHOLDER: &str = "Type a Probe message. Enter submits. Ctrl+J inserts a newline.";
const MAX_VISIBLE_COMPOSER_LINES: usize = 4;

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
            Self::Active => None,
            Self::Busy(copy) | Self::Disabled(copy) => Some(copy.as_str()),
        }
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
struct ComposerState {
    text: String,
    cursor: usize,
}

impl ComposerState {
    fn insert_char(&mut self, ch: char) {
        self.text.insert(self.cursor, ch);
        self.cursor += ch.len_utf8();
    }

    fn insert_newline(&mut self) {
        self.text.insert(self.cursor, '\n');
        self.cursor += 1;
    }

    fn backspace(&mut self) {
        if let Some((start, end)) = previous_grapheme_bounds(self.text.as_str(), self.cursor) {
            self.text.replace_range(start..end, "");
            self.cursor = start;
        }
    }

    fn delete(&mut self) {
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

    fn submit(&mut self) -> Option<String> {
        let submitted = self.text.trim().to_string();
        if submitted.is_empty() {
            return None;
        }
        self.text.clear();
        self.cursor = 0;
        Some(submitted)
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
    pub fn desired_height(&self) -> u16 {
        let visible_lines = self.composer.line_count().min(MAX_VISIBLE_COMPOSER_LINES) as u16;
        3 + 1 + visible_lines + 2
    }

    pub fn handle_event(&mut self, event: UiEvent, state: &BottomPaneState) -> Option<String> {
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
            UiEvent::ComposerNewline => {
                self.composer.insert_newline();
                None
            }
            UiEvent::ComposerSubmit => self.composer.submit(),
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
                "status: {status} | Tab/Shift+Tab | Ctrl+R | F1 | Ctrl+C"
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
        } else if self.composer.text.is_empty() {
            Text::from(Line::styled(
                PLACEHOLDER,
                Style::default()
                    .fg(Color::Rgb(0x91, 0xae, 0xc4))
                    .add_modifier(Modifier::DIM),
            ))
        } else {
            Text::from(
                self.composer
                    .visible_lines()
                    .into_iter()
                    .map(Line::from)
                    .collect::<Vec<_>>(),
            )
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
        assert_eq!(submitted.as_deref(), Some("a\nb"));
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
}
