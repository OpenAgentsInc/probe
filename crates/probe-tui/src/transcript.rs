use ratatui::style::Style;
use ratatui::text::{Line, Span, Text};

use crate::{rich_text, theme};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TranscriptRole {
    System,
    Status,
    Tool,
    Assistant,
    User,
}

impl TranscriptRole {
    pub(crate) fn label(self) -> &'static str {
        match self {
            Self::System => "system",
            Self::Status => "status",
            Self::Tool => "tool",
            Self::Assistant => "assistant",
            Self::User => "user",
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TranscriptEntryKind {
    Generic,
    ToolCall,
    ToolResult,
    ToolRefused,
    ApprovalPending,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranscriptEntry {
    role: TranscriptRole,
    kind: TranscriptEntryKind,
    title: String,
    body: Vec<String>,
}

impl TranscriptEntry {
    #[must_use]
    pub fn new(role: TranscriptRole, title: impl Into<String>, body: Vec<String>) -> Self {
        Self {
            role,
            kind: TranscriptEntryKind::Generic,
            title: title.into(),
            body,
        }
    }

    #[must_use]
    pub fn tool_call(title: impl Into<String>, body: Vec<String>) -> Self {
        Self {
            role: TranscriptRole::Tool,
            kind: TranscriptEntryKind::ToolCall,
            title: title.into(),
            body,
        }
    }

    #[must_use]
    pub fn tool_result(title: impl Into<String>, body: Vec<String>) -> Self {
        Self {
            role: TranscriptRole::Tool,
            kind: TranscriptEntryKind::ToolResult,
            title: title.into(),
            body,
        }
    }

    #[must_use]
    pub fn tool_refused(title: impl Into<String>, body: Vec<String>) -> Self {
        Self {
            role: TranscriptRole::Status,
            kind: TranscriptEntryKind::ToolRefused,
            title: title.into(),
            body,
        }
    }

    #[must_use]
    pub fn approval_pending(title: impl Into<String>, body: Vec<String>) -> Self {
        Self {
            role: TranscriptRole::Status,
            kind: TranscriptEntryKind::ApprovalPending,
            title: title.into(),
            body,
        }
    }

    #[must_use]
    pub const fn role(&self) -> TranscriptRole {
        self.role
    }

    #[must_use]
    pub const fn kind(&self) -> TranscriptEntryKind {
        self.kind
    }

    #[must_use]
    pub fn title(&self) -> &str {
        self.title.as_str()
    }

    #[must_use]
    pub fn body(&self) -> &[String] {
        self.body.as_slice()
    }

    #[must_use]
    pub fn label(&self) -> &'static str {
        match self.kind {
            TranscriptEntryKind::Generic => self.role.label(),
            TranscriptEntryKind::ToolCall => "tool call",
            TranscriptEntryKind::ToolResult => "tool result",
            TranscriptEntryKind::ToolRefused => "tool refused",
            TranscriptEntryKind::ApprovalPending => "approval pending",
        }
    }

    fn render_lines(&self) -> Vec<Line<'static>> {
        let mut lines = vec![header_line(
            self.label(),
            theme::transcript_label(),
            entry_title_style(self.role, self.kind),
            self.title.as_str(),
        )];
        lines.extend(indented_body_lines(self.body.as_slice()));
        lines
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ActiveTurn {
    role: TranscriptRole,
    title: String,
    body: Vec<String>,
}

impl ActiveTurn {
    #[must_use]
    pub fn new(role: TranscriptRole, title: impl Into<String>, body: Vec<String>) -> Self {
        Self {
            role,
            title: title.into(),
            body,
        }
    }

    #[must_use]
    pub const fn role(&self) -> TranscriptRole {
        self.role
    }

    #[must_use]
    pub fn title(&self) -> &str {
        self.title.as_str()
    }

    #[must_use]
    pub fn body(&self) -> &[String] {
        self.body.as_slice()
    }

    fn render_lines(&self) -> Vec<Line<'static>> {
        let mut lines = vec![header_line(
            format!("active {}", self.role.label()).as_str(),
            theme::active_label(),
            entry_title_style(self.role, TranscriptEntryKind::Generic),
            self.title.as_str(),
        )];
        lines.extend(indented_body_lines(self.body.as_slice()));
        lines
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RetainedTranscript {
    entries: Vec<TranscriptEntry>,
    live_entries: Vec<TranscriptEntry>,
    active_turn: Option<ActiveTurn>,
}

impl RetainedTranscript {
    #[must_use]
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push_entry(&mut self, entry: TranscriptEntry) {
        self.entries.push(entry);
    }

    pub fn push_live_entry(&mut self, entry: TranscriptEntry) {
        self.live_entries.push(entry);
    }

    pub fn clear_live_entries(&mut self) {
        self.live_entries.clear();
    }

    pub fn commit_live_entries(&mut self) {
        self.entries.append(&mut self.live_entries);
    }

    pub fn set_active_turn(&mut self, turn: ActiveTurn) {
        self.active_turn = Some(turn);
    }

    pub fn clear_active_turn(&mut self) {
        self.active_turn = None;
    }

    #[must_use]
    pub fn entries(&self) -> &[TranscriptEntry] {
        self.entries.as_slice()
    }

    #[must_use]
    pub fn active_turn(&self) -> Option<&ActiveTurn> {
        self.active_turn.as_ref()
    }

    #[must_use]
    pub fn as_text(&self) -> Text<'static> {
        let mut lines = Vec::new();
        if self.entries.is_empty() && self.live_entries.is_empty() && self.active_turn.is_none() {
            return Text::from(lines);
        }

        append_entry_lines(&mut lines, &self.entries);
        if !self.entries.is_empty() && !self.live_entries.is_empty() {
            lines.push(Line::from(""));
        }
        append_entry_lines(&mut lines, &self.live_entries);

        if let Some(active_turn) = &self.active_turn {
            if !lines.is_empty() {
                lines.push(Line::from(""));
            }
            lines.extend(active_turn.render_lines());
        }

        Text::from(lines)
    }
}

fn append_entry_lines(lines: &mut Vec<Line<'static>>, entries: &[TranscriptEntry]) {
    for (index, entry) in entries.iter().enumerate() {
        if index > 0 {
            lines.push(Line::from(""));
        }
        lines.extend(entry.render_lines());
    }
}

fn entry_title_style(role: TranscriptRole, kind: TranscriptEntryKind) -> Style {
    match kind {
        TranscriptEntryKind::ToolCall | TranscriptEntryKind::ToolResult => theme::tool_title(),
        TranscriptEntryKind::ToolRefused | TranscriptEntryKind::ApprovalPending => {
            theme::status_title()
        }
        TranscriptEntryKind::Generic => match role {
            TranscriptRole::System => theme::system_title(),
            TranscriptRole::Status => theme::status_title(),
            TranscriptRole::Tool => theme::tool_title(),
            TranscriptRole::Assistant => theme::assistant_title(),
            TranscriptRole::User => theme::user_title(),
        },
    }
}

fn header_line(label: &str, label_style: Style, title_style: Style, title: &str) -> Line<'static> {
    let mut spans = vec![
        Span::styled(format!("[{label}]"), label_style),
        Span::raw(" ".to_string()),
    ];
    spans.extend(rich_text::highlight_inline_spans(title, title_style));
    Line::from(spans)
}

fn indented_body_lines(body: &[String]) -> Vec<Line<'static>> {
    if body.is_empty() {
        return Vec::new();
    }
    let rendered = rich_text::render_markdownish_lines(body.join("\n").as_str());
    rendered
        .into_iter()
        .map(|line| {
            let mut spans = vec![Span::styled("  ".to_string(), theme::subtle())];
            spans.extend(line.spans);
            Line::from(spans)
        })
        .collect()
}

#[cfg(test)]
mod tests {
    use ratatui::style::Color;

    use super::{ActiveTurn, RetainedTranscript, TranscriptEntry, TranscriptRole};

    fn lines_to_plain_text(transcript: &RetainedTranscript) -> String {
        transcript
            .as_text()
            .lines
            .iter()
            .map(|line| line.to_string())
            .collect::<Vec<_>>()
            .join("\n")
    }

    #[test]
    fn retained_transcript_renders_committed_entries_and_active_turn() {
        let mut transcript = RetainedTranscript::new();
        transcript.push_entry(TranscriptEntry::new(
            TranscriptRole::System,
            "Shell Ready",
            vec![String::from(
                "Press Ctrl+R to start the Apple FM setup check.",
            )],
        ));
        transcript.set_active_turn(ActiveTurn::new(
            TranscriptRole::Tool,
            "Sanity Check",
            vec![String::from("Reply with exactly READY.")],
        ));

        let rendered = lines_to_plain_text(&transcript);
        assert!(rendered.contains("[system] Shell Ready"));
        assert!(rendered.contains("[active tool] Sanity Check"));
    }

    #[test]
    fn retained_transcript_is_blank_when_empty() {
        let transcript = RetainedTranscript::new();
        let rendered = lines_to_plain_text(&transcript);
        assert!(rendered.is_empty());
    }

    #[test]
    fn retained_transcript_keeps_live_entries_before_active_turn() {
        let mut transcript = RetainedTranscript::new();
        transcript.push_live_entry(TranscriptEntry::tool_call(
            "read_file",
            vec![String::from("README.md")],
        ));
        transcript.push_live_entry(TranscriptEntry::tool_result(
            "read_file",
            vec![String::from("README.md:1-2")],
        ));
        transcript.set_active_turn(ActiveTurn::new(
            TranscriptRole::Tool,
            "Running Tool: list_files",
            vec![String::from("risk: read")],
        ));

        let rendered = lines_to_plain_text(&transcript);
        assert!(rendered.contains("[tool call] read_file"));
        assert!(rendered.contains("[tool result] read_file"));
        assert!(rendered.contains("[active tool] Running Tool: list_files"));
    }

    #[test]
    fn transcript_header_and_body_pick_up_codex_style_colors() {
        let entry = TranscriptEntry::new(
            TranscriptRole::User,
            "You",
            vec![String::from(
                "/plan see probe#119 in crates/probe-tui/src/lib.rs",
            )],
        );
        let lines = entry.render_lines();
        assert_eq!(lines[0].spans[0].style.fg, Some(Color::DarkGray));
        assert_eq!(lines[0].spans[2].style.fg, Some(Color::Cyan));
        assert!(
            lines[1]
                .spans
                .iter()
                .any(|span| span.style.fg == Some(Color::Magenta))
        );
        assert!(
            lines[1]
                .spans
                .iter()
                .any(|span| span.style.fg == Some(Color::Cyan))
        );
    }
}
