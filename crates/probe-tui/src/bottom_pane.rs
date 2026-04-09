use ratatui::Frame;
use ratatui::layout::{Constraint, Layout, Rect};
use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Text};
use ratatui::widgets::{Block, Borders, Padding, Paragraph, Wrap};

use crate::event::UiEvent;

const PLACEHOLDER: &str = "Type a Probe message. Enter submits. Ctrl+J inserts a newline.";
const MAX_VISIBLE_COMPOSER_LINES: usize = 6;
const MAX_HISTORY_ENTRIES: usize = 24;
const ATTACHMENT_LIBRARY: [&str; 3] = ["README.md", "Cargo.toml", "docs/README.md"];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct SlashCommandSpec {
    name: &'static str,
    description: &'static str,
    shortcut: Option<&'static str>,
    submit_on_select: bool,
}

const SLASH_COMMANDS: [SlashCommandSpec; 33] = [
    SlashCommandSpec {
        name: "help",
        description: "Show keyboard help and shortcuts",
        shortcut: Some("F1"),
        submit_on_select: true,
    },
    SlashCommandSpec {
        name: "status",
        description: "Inspect the active lane state at a glance",
        shortcut: Some("F2"),
        submit_on_select: true,
    },
    SlashCommandSpec {
        name: "doctor",
        description: "Run a quick health check for the active lane",
        shortcut: Some("F3"),
        submit_on_select: true,
    },
    SlashCommandSpec {
        name: "recipes",
        description: "Open guided workflows for common Probe tasks",
        shortcut: Some("F5"),
        submit_on_select: true,
    },
    SlashCommandSpec {
        name: "git",
        description: "Inspect branch, repo, and delivery state",
        shortcut: Some("Ctrl+G"),
        submit_on_select: true,
    },
    SlashCommandSpec {
        name: "branch",
        description: "Create or switch the current git branch",
        shortcut: None,
        submit_on_select: true,
    },
    SlashCommandSpec {
        name: "stage",
        description: "Stage the current repo changes",
        shortcut: None,
        submit_on_select: true,
    },
    SlashCommandSpec {
        name: "backend",
        description: "Inspect or switch the active backend lane",
        shortcut: Some("Ctrl+S"),
        submit_on_select: true,
    },
    SlashCommandSpec {
        name: "model",
        description: "Choose the model for the active backend",
        shortcut: None,
        submit_on_select: true,
    },
    SlashCommandSpec {
        name: "memory",
        description: "Inspect active user, repo, and directory memory",
        shortcut: None,
        submit_on_select: true,
    },
    SlashCommandSpec {
        name: "mcp",
        description: "Inspect integrations and MCP status",
        shortcut: None,
        submit_on_select: true,
    },
    SlashCommandSpec {
        name: "reasoning",
        description: "Adjust Codex reasoning effort",
        shortcut: None,
        submit_on_select: true,
    },
    SlashCommandSpec {
        name: "plan",
        description: "Toggle plan mode for the active lane",
        shortcut: None,
        submit_on_select: true,
    },
    SlashCommandSpec {
        name: "review_mode",
        description: "Choose how write-capable work should be reviewed",
        shortcut: None,
        submit_on_select: true,
    },
    SlashCommandSpec {
        name: "background",
        description: "Choose whether the next turn runs foreground or background",
        shortcut: None,
        submit_on_select: true,
    },
    SlashCommandSpec {
        name: "delegate",
        description: "Queue the next turn as a delegated child task",
        shortcut: None,
        submit_on_select: true,
    },
    SlashCommandSpec {
        name: "cwd",
        description: "Inspect or change the current workspace",
        shortcut: None,
        submit_on_select: true,
    },
    SlashCommandSpec {
        name: "approvals",
        description: "Review pending approval requests",
        shortcut: Some("Ctrl+A"),
        submit_on_select: true,
    },
    SlashCommandSpec {
        name: "new",
        description: "Start a fresh task or session",
        shortcut: None,
        submit_on_select: true,
    },
    SlashCommandSpec {
        name: "clear",
        description: "Start a fresh context on the active lane",
        shortcut: None,
        submit_on_select: true,
    },
    SlashCommandSpec {
        name: "compact",
        description: "Carry forward a compact summary",
        shortcut: None,
        submit_on_select: true,
    },
    SlashCommandSpec {
        name: "commit",
        description: "Create a git commit from staged changes",
        shortcut: None,
        submit_on_select: true,
    },
    SlashCommandSpec {
        name: "push",
        description: "Push the current branch to its remote",
        shortcut: None,
        submit_on_select: true,
    },
    SlashCommandSpec {
        name: "pr",
        description: "Create a draft pull request for the current branch",
        shortcut: None,
        submit_on_select: true,
    },
    SlashCommandSpec {
        name: "pr_comments",
        description: "Inspect the current PR's review comments and feedback",
        shortcut: None,
        submit_on_select: true,
    },
    SlashCommandSpec {
        name: "conversation",
        description: "Show the calmer conversation-first transcript view",
        shortcut: None,
        submit_on_select: true,
    },
    SlashCommandSpec {
        name: "trace",
        description: "Show the full tool and runtime transcript trace",
        shortcut: None,
        submit_on_select: true,
    },
    SlashCommandSpec {
        name: "usage",
        description: "Inspect token usage for this lane",
        shortcut: None,
        submit_on_select: true,
    },
    SlashCommandSpec {
        name: "diff",
        description: "Inspect the latest task diff for this lane",
        shortcut: None,
        submit_on_select: true,
    },
    SlashCommandSpec {
        name: "checkpoint",
        description: "Inspect latest checkpoint coverage for this lane",
        shortcut: None,
        submit_on_select: true,
    },
    SlashCommandSpec {
        name: "revert",
        description: "Inspect revert readiness for the latest task",
        shortcut: None,
        submit_on_select: true,
    },
    SlashCommandSpec {
        name: "tasks",
        description: "Inspect detached tasks and reopen one",
        shortcut: Some("F4"),
        submit_on_select: true,
    },
    SlashCommandSpec {
        name: "resume",
        description: "Reopen a previous Probe task or session",
        shortcut: None,
        submit_on_select: true,
    },
];

impl SlashCommandSpec {
    fn display_description(self) -> String {
        match self.shortcut {
            Some(shortcut) => format!("{} ({shortcut})", self.description),
            None => self.description.to_string(),
        }
    }
}

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
    slash_palette_dismissed: bool,
    slash_selection: usize,
    history: Vec<DraftSnapshot>,
    history_index: Option<usize>,
    stashed_snapshot: Option<DraftSnapshot>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct WrappedComposerView {
    visible_lines: Vec<String>,
    total_lines: usize,
    start_row: usize,
    cursor_row: usize,
    cursor_col: usize,
}

impl ComposerState {
    fn insert_char(&mut self, ch: char) {
        self.prepare_for_edit();
        self.slash_palette_dismissed = false;
        self.text.insert(self.cursor, ch);
        self.cursor += ch.len_utf8();
        self.clamp_slash_selection();
    }

    fn insert_text(&mut self, value: &str) {
        self.prepare_for_edit();
        self.slash_palette_dismissed = false;
        self.text.insert_str(self.cursor, value);
        self.cursor += value.len();
        if value.contains('\n') || value.chars().count() > 24 {
            self.pasted_multiline = true;
        }
        self.clamp_slash_selection();
    }

    fn insert_newline(&mut self) {
        self.prepare_for_edit();
        self.slash_palette_dismissed = false;
        self.text.insert(self.cursor, '\n');
        self.cursor += 1;
        self.clamp_slash_selection();
    }

    fn backspace(&mut self) {
        self.prepare_for_edit();
        self.slash_palette_dismissed = false;
        if let Some((start, end)) = previous_grapheme_bounds(self.text.as_str(), self.cursor) {
            self.text.replace_range(start..end, "");
            self.cursor = start;
        }
        self.clamp_slash_selection();
    }

    fn delete(&mut self) {
        self.prepare_for_edit();
        self.slash_palette_dismissed = false;
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
        self.slash_palette_dismissed = false;
        let label = ATTACHMENT_LIBRARY[self.attachments.len() % ATTACHMENT_LIBRARY.len()];
        self.attachments.push(DraftAttachment {
            label: label.to_string(),
            source: String::from("local-placeholder"),
        });
        self.clamp_slash_selection();
    }

    fn dismiss_slash_palette(&mut self) -> bool {
        if self.slash_palette().is_some() {
            self.slash_palette_dismissed = true;
            return true;
        }
        false
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
        self.slash_palette_dismissed = false;
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
        self.slash_palette_dismissed = false;
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
        if self.slash_palette_dismissed {
            return None;
        }
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

    fn wrapped_view(&self, width: usize, max_visible_lines: usize) -> WrappedComposerView {
        let width = width.max(1);
        let wrapped_lines = wrap_composer_text(self.text.as_str(), width);
        let (cursor_row, cursor_col) =
            wrapped_cursor_row_col(self.text.as_str(), self.cursor, width);
        let total_lines = wrapped_lines.len().max(1);
        let visible_count = total_lines.min(max_visible_lines.max(1));
        let start_row = cursor_row.saturating_add(1).saturating_sub(visible_count);
        let end_row = (start_row + visible_count).min(total_lines);
        WrappedComposerView {
            visible_lines: wrapped_lines[start_row..end_row].to_vec(),
            total_lines,
            start_row,
            cursor_row,
            cursor_col,
        }
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

    pub fn replace_draft(&mut self, text: impl Into<String>) {
        let text = text.into();
        self.composer.clear();
        self.composer.text = text;
        self.composer.cursor = self.composer.text.len();
        self.composer.clamp_slash_selection();
    }

    pub fn dismiss_slash_palette(&mut self, state: &BottomPaneState) -> bool {
        if !state.input_enabled() {
            return false;
        }
        self.composer.dismiss_slash_palette()
    }

    #[must_use]
    pub fn desired_height(&self, width: u16, state: &BottomPaneState) -> u16 {
        let content_lines = if state.helper_copy().is_some() {
            1u16
        } else {
            let content_width = width.saturating_sub(4).max(1) as usize;
            let wrapped_view = self
                .composer
                .wrapped_view(content_width, MAX_VISIBLE_COMPOSER_LINES);
            1 + self.composer.visible_slash_palette().len() as u16
                + wrapped_view.visible_lines.len() as u16
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
            UiEvent::Dismiss => {
                let _ = self.composer.dismiss_slash_palette();
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
                "status: {status} | F2/F3/F4/F5 status/doctor/tasks/recipes | Ctrl+G git | Ctrl+R/S/A/O | F1 | Ctrl+C"
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
            let wrapped_view = self.composer.wrapped_view(
                composer_inner.width.max(1) as usize,
                MAX_VISIBLE_COMPOSER_LINES,
            );
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
                    format!(
                        "{prefix} /{}  {}",
                        command.name,
                        command.display_description()
                    ),
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
                if wrapped_view.start_row > 0 {
                    lines.push(Line::styled(
                        format!("... {} earlier lines", wrapped_view.start_row),
                        Style::default()
                            .fg(Color::Rgb(0x91, 0xae, 0xc4))
                            .add_modifier(Modifier::DIM),
                    ));
                }
                lines.extend(wrapped_view.visible_lines.into_iter().map(Line::from));
                let hidden_tail = wrapped_view
                    .total_lines
                    .saturating_sub(wrapped_view.start_row + MAX_VISIBLE_COMPOSER_LINES);
                if hidden_tail > 0 {
                    lines.push(Line::styled(
                        format!("... {} more lines", hidden_tail),
                        Style::default()
                            .fg(Color::Rgb(0x91, 0xae, 0xc4))
                            .add_modifier(Modifier::DIM),
                    ));
                }
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
        let slash_palette_rows = self.composer.visible_slash_palette().len();
        let wrapped_view = self.composer.wrapped_view(
            composer_inner.width.max(1) as usize,
            MAX_VISIBLE_COMPOSER_LINES,
        );
        let row = wrapped_view
            .cursor_row
            .saturating_sub(wrapped_view.start_row);
        let col = wrapped_view.cursor_col;
        Some((
            composer_inner
                .x
                .saturating_add(col.min(composer_inner.width.saturating_sub(1) as usize) as u16),
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

fn wrap_composer_text(value: &str, width: usize) -> Vec<String> {
    let width = width.max(1);
    let mut wrapped = Vec::new();
    for logical_line in value.split('\n') {
        if logical_line.is_empty() {
            wrapped.push(String::new());
            continue;
        }

        let mut current = String::new();
        let mut col = 0usize;
        for ch in logical_line.chars() {
            if col >= width {
                wrapped.push(current);
                current = String::new();
                col = 0;
            }
            current.push(ch);
            col += 1;
        }
        wrapped.push(current);
    }
    if wrapped.is_empty() {
        wrapped.push(String::new());
    }
    wrapped
}

fn wrapped_cursor_row_col(value: &str, cursor: usize, width: usize) -> (usize, usize) {
    let width = width.max(1);
    let mut row = 0usize;
    let mut col = 0usize;
    for ch in value[..cursor].chars() {
        if ch == '\n' {
            row += 1;
            col = 0;
            continue;
        }
        if col >= width {
            row += 1;
            col = 0;
        }
        col += 1;
        if col == width {
            row += 1;
            col = 0;
        }
    }
    (row, col)
}

#[cfg(test)]
mod tests {
    use super::{BottomPane, BottomPaneState};
    use crate::event::UiEvent;
    use ratatui::layout::Rect;

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
        let _ = pane.handle_event(UiEvent::ComposerInsert('e'), &state);
        let _ = pane.handle_event(UiEvent::ComposerInsert('v'), &state);
        let _ = pane.handle_event(UiEvent::ComposerHistoryNext, &state);
        let submitted = pane
            .handle_event(UiEvent::ComposerSubmit, &state)
            .expect("revert should submit immediately");

        assert_eq!(submitted.text, "/revert");
        assert_eq!(submitted.slash_command.as_deref(), Some("revert"));
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
                "status",
                "doctor",
                "recipes",
                "git",
                "branch",
                "stage",
                "backend",
                "model",
                "memory",
                "mcp",
                "reasoning",
                "plan",
                "review_mode",
                "background",
                "delegate",
                "cwd",
                "approvals",
                "new",
                "clear",
                "compact",
                "commit",
                "push",
                "pr",
                "pr_comments",
                "conversation",
                "trace",
                "usage",
                "diff",
                "checkpoint",
                "revert",
                "tasks",
                "resume",
            ]
        );
    }

    #[test]
    fn slash_palette_includes_hotkey_hints_for_popular_commands() {
        let mut pane = BottomPane::new();
        let state = BottomPaneState::Active;

        let _ = pane.handle_event(UiEvent::ComposerInsert('/'), &state);

        let visible = pane.composer.visible_slash_palette();
        let status = visible
            .iter()
            .find(|command| command.name == "status")
            .expect("status command should be visible");
        let git = visible
            .iter()
            .find(|command| command.name == "git")
            .expect("git command should be visible");
        let tasks = visible
            .iter()
            .find(|command| command.name == "tasks")
            .expect("tasks command should be visible");

        assert_eq!(
            status.display_description(),
            "Inspect the active lane state at a glance (F2)"
        );
        assert_eq!(
            git.display_description(),
            "Inspect branch, repo, and delivery state (Ctrl+G)"
        );
        assert_eq!(
            tasks.display_description(),
            "Inspect detached tasks and reopen one (F4)"
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

    #[test]
    fn dismiss_hides_slash_palette_without_clearing_the_draft() {
        let mut pane = BottomPane::new();
        let state = BottomPaneState::Active;

        let _ = pane.handle_event(UiEvent::ComposerInsert('/'), &state);
        let _ = pane.handle_event(UiEvent::ComposerInsert('m'), &state);

        assert!(!pane.composer.visible_slash_palette().is_empty());

        let _ = pane.handle_event(UiEvent::Dismiss, &state);

        assert_eq!(pane.current_text(), "/m");
        assert!(pane.composer.visible_slash_palette().is_empty());
    }

    #[test]
    fn editing_after_dismiss_reopens_the_matching_slash_palette() {
        let mut pane = BottomPane::new();
        let state = BottomPaneState::Active;

        let _ = pane.handle_event(UiEvent::ComposerInsert('/'), &state);
        let _ = pane.handle_event(UiEvent::ComposerInsert('m'), &state);
        let _ = pane.handle_event(UiEvent::Dismiss, &state);

        let _ = pane.handle_event(UiEvent::ComposerInsert('o'), &state);

        let visible = pane.composer.visible_slash_palette();
        assert!(!visible.is_empty());
        assert!(visible.iter().any(|command| command.name == "model"));
    }

    #[test]
    fn desired_height_grows_for_long_wrapped_drafts() {
        let mut pane = BottomPane::new();
        let state = BottomPaneState::Active;

        let _ = pane.handle_event(
            UiEvent::ComposerPaste(String::from(
                "This is a long prompt that should wrap across multiple rows in the composer pane.",
            )),
            &state,
        );

        let short = pane.desired_height(120, &state);
        let narrow = pane.desired_height(40, &state);

        assert!(narrow > short, "short={short} narrow={narrow}");
    }

    #[test]
    fn cursor_tracks_wrapped_tail_for_long_drafts() {
        let mut pane = BottomPane::new();
        let state = BottomPaneState::Active;

        let _ = pane.handle_event(
            UiEvent::ComposerPaste(String::from(
                "alpha beta gamma delta epsilon zeta eta theta iota kappa lambda mu",
            )),
            &state,
        );

        let area = Rect::new(0, 0, 40, pane.desired_height(40, &state));
        let cursor = pane
            .cursor_position(area, &state)
            .expect("active composer should have a cursor");

        assert!(cursor.0 > 0);
        assert!(cursor.1 > 0);
    }
}
