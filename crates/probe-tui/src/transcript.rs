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
        match self.kind {
            TranscriptEntryKind::Generic => match self.role {
                TranscriptRole::User => {
                    render_user_message(message_lines(self.title.as_str(), self.body.as_slice()))
                }
                TranscriptRole::Assistant => render_assistant_message(message_lines(
                    self.title.as_str(),
                    self.body.as_slice(),
                )),
                TranscriptRole::System | TranscriptRole::Status | TranscriptRole::Tool => {
                    render_history_cell(
                        "•",
                        theme::history_bullet(),
                        None,
                        self.title.as_str(),
                        entry_title_style(self.role, self.kind),
                        self.body.as_slice(),
                        theme::history_detail(),
                    )
                }
            },
            TranscriptEntryKind::ToolCall => render_history_cell(
                "•",
                theme::history_bullet(),
                Some("Calling"),
                self.title.as_str(),
                theme::tool_title(),
                self.body.as_slice(),
                theme::history_detail(),
            ),
            TranscriptEntryKind::ToolResult => render_history_cell(
                "•",
                theme::history_success_bullet(),
                Some("Called"),
                self.title.as_str(),
                theme::tool_title(),
                self.body.as_slice(),
                theme::history_detail(),
            ),
            TranscriptEntryKind::ToolRefused => render_history_cell(
                "✖",
                theme::history_error_bullet(),
                Some("Blocked"),
                self.title.as_str(),
                theme::tool_title(),
                self.body.as_slice(),
                theme::history_detail(),
            ),
            TranscriptEntryKind::ApprovalPending => render_history_cell(
                "⚠",
                theme::history_warning_bullet(),
                Some("Approval needed for"),
                self.title.as_str(),
                theme::tool_title(),
                self.body.as_slice(),
                theme::history_detail(),
            ),
        }
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
        match self.role {
            TranscriptRole::User => {
                render_user_message(message_lines(self.title.as_str(), self.body.as_slice()))
            }
            TranscriptRole::Assistant => {
                if self.body.is_empty() {
                    render_history_cell(
                        "•",
                        theme::history_bullet(),
                        None,
                        self.title.as_str(),
                        theme::history_header(),
                        &[],
                        theme::history_detail(),
                    )
                } else {
                    render_assistant_message(self.body.clone())
                }
            }
            TranscriptRole::System => render_history_cell(
                "•",
                theme::history_bullet(),
                Some("Active"),
                self.title.as_str(),
                entry_title_style(self.role, TranscriptEntryKind::Generic),
                self.body.as_slice(),
                theme::history_detail(),
            ),
            TranscriptRole::Status | TranscriptRole::Tool => {
                if let Some(tool_name) = self.title.strip_prefix("Running Tool: ") {
                    render_history_cell(
                        "•",
                        theme::history_bullet(),
                        Some("Running"),
                        tool_name,
                        theme::tool_title(),
                        self.body.as_slice(),
                        theme::history_detail(),
                    )
                } else {
                    render_history_cell(
                        "•",
                        theme::history_bullet(),
                        Some("Active"),
                        self.title.as_str(),
                        entry_title_style(self.role, TranscriptEntryKind::Generic),
                        self.body.as_slice(),
                        theme::history_detail(),
                    )
                }
            }
        }
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

fn message_lines(title: &str, body: &[String]) -> Vec<String> {
    if body.is_empty() {
        vec![title.to_string()]
    } else {
        body.to_vec()
    }
}

fn render_user_message(body: Vec<String>) -> Vec<Line<'static>> {
    let rendered = patch_lines(
        rich_text::render_markdownish_lines(body.join("\n").as_str()),
        theme::user_message(),
    );
    prefixed_lines(
        rendered,
        vec![Span::styled("› ".to_string(), theme::history_bullet())],
        vec![Span::styled("  ".to_string(), theme::history_detail())],
    )
}

fn render_assistant_message(body: Vec<String>) -> Vec<Line<'static>> {
    prefixed_body_lines(
        body.as_slice(),
        vec![Span::styled("• ".to_string(), theme::history_bullet())],
        vec![Span::styled("  ".to_string(), theme::history_detail())],
        Style::default(),
    )
}

fn render_history_cell(
    symbol: &str,
    bullet_style: Style,
    verb: Option<&str>,
    title: &str,
    title_style: Style,
    body: &[String],
    body_style: Style,
) -> Vec<Line<'static>> {
    let mut header_spans = vec![
        Span::styled(symbol.to_string(), bullet_style),
        Span::raw(" ".to_string()),
    ];
    if let Some(verb) = verb {
        header_spans.push(Span::styled(verb.to_string(), theme::history_header()));
        if !title.is_empty() {
            header_spans.push(Span::raw(" ".to_string()));
        }
    }
    if !title.is_empty() {
        header_spans.extend(rich_text::highlight_inline_spans(
            title,
            theme::history_header().patch(title_style),
        ));
    }

    let mut lines = vec![Line::from(header_spans)];
    lines.extend(prefixed_body_lines(
        body,
        vec![Span::styled("  └ ".to_string(), theme::history_detail())],
        vec![Span::styled("    ".to_string(), theme::history_detail())],
        body_style,
    ));
    lines
}

fn prefixed_body_lines(
    body: &[String],
    initial_prefix: Vec<Span<'static>>,
    subsequent_prefix: Vec<Span<'static>>,
    base_style: Style,
) -> Vec<Line<'static>> {
    if body.is_empty() {
        return Vec::new();
    }
    let rendered = patch_lines(
        rich_text::render_markdownish_lines(body.join("\n").as_str()),
        base_style,
    );
    prefixed_lines(rendered, initial_prefix, subsequent_prefix)
}

fn prefixed_lines(
    lines: Vec<Line<'static>>,
    initial_prefix: Vec<Span<'static>>,
    subsequent_prefix: Vec<Span<'static>>,
) -> Vec<Line<'static>> {
    lines
        .into_iter()
        .enumerate()
        .map(|(index, line)| {
            let mut spans = if index == 0 {
                initial_prefix.clone()
            } else {
                subsequent_prefix.clone()
            };
            spans.extend(line.spans);
            Line::from(spans)
        })
        .collect()
}

fn patch_lines(lines: Vec<Line<'static>>, base_style: Style) -> Vec<Line<'static>> {
    lines
        .into_iter()
        .map(|mut line| {
            line.spans = line
                .spans
                .into_iter()
                .map(|mut span| {
                    span.style = base_style.patch(span.style);
                    span
                })
                .collect();
            line
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
        assert!(rendered.contains("• Shell Ready"));
        assert!(rendered.contains("• Active Sanity Check"));
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
        assert!(rendered.contains("• Calling read_file"));
        assert!(rendered.contains("• Called read_file"));
        assert!(rendered.contains("• Running list_files"));
    }

    #[test]
    fn transcript_tool_rows_pick_up_codex_style_colors() {
        let entry = TranscriptEntry::tool_result(
            "read_file",
            vec![String::from("crates/probe-tui/src/lib.rs")],
        );
        let lines = entry.render_lines();
        assert_eq!(lines[0].spans[0].style.fg, Some(Color::Green));
        assert!(
            lines[0]
                .spans
                .iter()
                .any(|span| span.style.fg == Some(Color::Cyan))
        );
        assert!(
            lines[1]
                .spans
                .iter()
                .any(|span| span.style.fg == Some(Color::Cyan))
        );
    }

    #[test]
    fn transcript_user_rows_pick_up_codex_style_prefix_and_inline_styles() {
        let entry = TranscriptEntry::new(
            TranscriptRole::User,
            "You",
            vec![String::from(
                "/plan see probe#119 in crates/probe-tui/src/lib.rs",
            )],
        );
        let lines = entry.render_lines();
        assert_eq!(lines[0].spans[0].content.as_ref(), "› ");
        assert_eq!(lines[0].spans[0].style.fg, Some(Color::DarkGray));
        assert!(
            lines[0]
                .spans
                .iter()
                .any(|span| span.style.fg == Some(Color::Magenta))
        );
        assert!(
            lines[0]
                .spans
                .iter()
                .any(|span| span.style.fg == Some(Color::Cyan))
        );
    }
}
