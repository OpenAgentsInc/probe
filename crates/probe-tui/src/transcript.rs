use ratatui::style::{Color, Modifier, Style};
use ratatui::text::{Line, Span, Text};

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
        let styles = transcript_entry_styles(self.role, self.kind);
        let mut lines = vec![Line::from(vec![
            Span::styled(
                format!("[{}] ", self.label()),
                styles.label_style.add_modifier(Modifier::BOLD),
            ),
            Span::styled(self.title.clone(), styles.title_style),
        ])];
        for line in &self.body {
            lines.push(Line::from(vec![
                Span::raw("  "),
                Span::styled(line.clone(), styles.body_style),
            ]));
        }
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
        let styles = active_turn_styles(self.role);
        let mut lines = vec![Line::from(vec![
            Span::styled(
                format!("[active {}] ", self.role.label()),
                styles.label_style.add_modifier(Modifier::BOLD),
            ),
            Span::styled(self.title.clone(), styles.title_style),
        ])];
        for line in &self.body {
            lines.push(Line::from(vec![
                Span::raw("  "),
                Span::styled(line.clone(), styles.body_style),
            ]));
        }
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
    pub fn is_empty(&self) -> bool {
        self.entries.is_empty() && self.live_entries.is_empty() && self.active_turn.is_none()
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

    #[must_use]
    pub fn as_conversation_text(&self) -> Text<'static> {
        let mut lines = Vec::new();
        if self.entries.is_empty() && self.live_entries.is_empty() && self.active_turn.is_none() {
            return Text::from(lines);
        }

        append_conversation_entry_lines(&mut lines, &self.entries);
        if !self.entries.is_empty() && !self.live_entries.is_empty() {
            lines.push(Line::from(""));
        }
        append_conversation_entry_lines(&mut lines, &self.live_entries);

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

fn append_conversation_entry_lines(lines: &mut Vec<Line<'static>>, entries: &[TranscriptEntry]) {
    if !entries.is_empty()
        && entries.iter().all(|entry| {
            matches!(
                entry.kind(),
                TranscriptEntryKind::ToolCall | TranscriptEntryKind::ToolResult
            )
        })
    {
        append_entry_lines(lines, entries);
        return;
    }

    let mut hidden_edit_paths = Vec::new();
    let mut appended_any = !lines.is_empty();

    for entry in entries {
        if matches!(
            entry.kind(),
            TranscriptEntryKind::ToolCall | TranscriptEntryKind::ToolResult
        ) {
            append_unique_hidden_edit_paths(&mut hidden_edit_paths, entry);
            continue;
        }

        if !hidden_edit_paths.is_empty() {
            if appended_any {
                lines.push(Line::from(""));
            }
            lines.extend(render_hidden_edit_summary_lines(
                hidden_edit_paths.as_slice(),
            ));
            hidden_edit_paths.clear();
            appended_any = true;
        }

        if appended_any {
            lines.push(Line::from(""));
        }
        lines.extend(entry.render_lines());
        appended_any = true;
    }

    if !hidden_edit_paths.is_empty() {
        if appended_any {
            lines.push(Line::from(""));
        }
        lines.extend(render_hidden_edit_summary_lines(
            hidden_edit_paths.as_slice(),
        ));
    }
}

fn render_hidden_edit_summary_lines(edit_paths: &[String]) -> Vec<Line<'static>> {
    let styles = transcript_entry_styles(TranscriptRole::Status, TranscriptEntryKind::Generic);
    vec![Line::from(vec![
        Span::styled(
            "[edited] ".to_string(),
            styles.label_style.add_modifier(Modifier::BOLD),
        ),
        Span::styled(
            summarize_tool_titles(edit_paths, 2),
            styles.body_style.add_modifier(Modifier::DIM),
        ),
    ])]
}

fn append_unique_hidden_edit_paths(target: &mut Vec<String>, entry: &TranscriptEntry) {
    for path in hidden_edit_paths(entry) {
        if !target.iter().any(|existing| existing == &path) {
            target.push(path);
        }
    }
}

fn hidden_edit_paths(entry: &TranscriptEntry) -> Vec<String> {
    if entry.title() != "apply_patch" {
        return Vec::new();
    }

    let mut paths = Vec::new();
    for line in entry.body() {
        if let Some(value) = line.strip_prefix("updated: ") {
            for path in value
                .split(',')
                .map(str::trim)
                .filter(|value| !value.is_empty())
            {
                paths.push(path.to_string());
            }
            continue;
        }
        if is_relative_file_path_candidate(line) {
            paths.push(line.trim().to_string());
            break;
        }
    }
    paths
}

fn is_relative_file_path_candidate(value: &str) -> bool {
    let value = value.trim();
    !value.is_empty()
        && !value.contains(' ')
        && value.contains('.')
        && !value.starts_with('#')
        && !value.ends_with(':')
        && !value.contains('{')
        && !value.contains('}')
}

fn summarize_tool_titles(tool_titles: &[String], max_items: usize) -> String {
    let mut items = tool_titles
        .iter()
        .take(max_items)
        .cloned()
        .collect::<Vec<_>>();
    let remaining = tool_titles.len().saturating_sub(items.len());
    if remaining > 0 {
        items.push(format!("+{remaining} more"));
    }
    items.join(", ")
}

#[derive(Clone, Copy)]
struct TranscriptVisualStyles {
    label_style: Style,
    title_style: Style,
    body_style: Style,
}

fn transcript_entry_styles(
    role: TranscriptRole,
    kind: TranscriptEntryKind,
) -> TranscriptVisualStyles {
    match kind {
        TranscriptEntryKind::ToolCall | TranscriptEntryKind::ToolResult => TranscriptVisualStyles {
            label_style: Style::default()
                .fg(Color::Rgb(0x7f, 0x93, 0xa6))
                .add_modifier(Modifier::DIM),
            title_style: Style::default()
                .fg(Color::Rgb(0x9b, 0xae, 0xc0))
                .add_modifier(Modifier::DIM),
            body_style: Style::default()
                .fg(Color::Rgb(0x88, 0x9a, 0xab))
                .add_modifier(Modifier::DIM),
        },
        TranscriptEntryKind::ToolRefused | TranscriptEntryKind::ApprovalPending => {
            TranscriptVisualStyles {
                label_style: Style::default()
                    .fg(Color::Rgb(0xf1, 0xc4, 0x53))
                    .add_modifier(Modifier::DIM),
                title_style: Style::default().fg(Color::Rgb(0xff, 0xe2, 0x92)),
                body_style: Style::default().fg(Color::Rgb(0xe3, 0xca, 0x88)),
            }
        }
        TranscriptEntryKind::Generic => match role {
            TranscriptRole::Assistant => TranscriptVisualStyles {
                label_style: Style::default().fg(Color::Rgb(0xf8, 0xf4, 0xe3)),
                title_style: Style::default()
                    .fg(Color::Rgb(0xff, 0xf1, 0xd0))
                    .add_modifier(Modifier::BOLD),
                body_style: Style::default().fg(Color::Rgb(0xff, 0xfb, 0xef)),
            },
            TranscriptRole::User => TranscriptVisualStyles {
                label_style: Style::default().fg(Color::Rgb(0x9f, 0xd7, 0xff)),
                title_style: Style::default()
                    .fg(Color::Rgb(0xc7, 0xe9, 0xff))
                    .add_modifier(Modifier::BOLD),
                body_style: Style::default().fg(Color::Rgb(0xe0, 0xf1, 0xff)),
            },
            TranscriptRole::Status => TranscriptVisualStyles {
                label_style: Style::default()
                    .fg(Color::Rgb(0xa7, 0xb8, 0xc8))
                    .add_modifier(Modifier::DIM),
                title_style: Style::default().fg(Color::Rgb(0xc5, 0xd1, 0xdb)),
                body_style: Style::default().fg(Color::Rgb(0xb7, 0xc3, 0xcf)),
            },
            TranscriptRole::System | TranscriptRole::Tool => TranscriptVisualStyles {
                label_style: Style::default()
                    .fg(Color::Rgb(0x91, 0xae, 0xc4))
                    .add_modifier(Modifier::DIM),
                title_style: Style::default().fg(Color::Rgb(0xc2, 0xd2, 0xe0)),
                body_style: Style::default().fg(Color::Rgb(0xad, 0xbe, 0xcc)),
            },
        },
    }
}

fn active_turn_styles(role: TranscriptRole) -> TranscriptVisualStyles {
    match role {
        TranscriptRole::Assistant => TranscriptVisualStyles {
            label_style: Style::default().fg(Color::Rgb(0xff, 0xf1, 0xd0)),
            title_style: Style::default()
                .fg(Color::Rgb(0xff, 0xfb, 0xef))
                .add_modifier(Modifier::BOLD),
            body_style: Style::default().fg(Color::Rgb(0xff, 0xfb, 0xef)),
        },
        TranscriptRole::Tool => TranscriptVisualStyles {
            label_style: Style::default()
                .fg(Color::Rgb(0x7f, 0x93, 0xa6))
                .add_modifier(Modifier::DIM),
            title_style: Style::default()
                .fg(Color::Rgb(0xb0, 0xc0, 0xce))
                .add_modifier(Modifier::DIM),
            body_style: Style::default()
                .fg(Color::Rgb(0x95, 0xa6, 0xb6))
                .add_modifier(Modifier::DIM),
        },
        TranscriptRole::Status | TranscriptRole::System => TranscriptVisualStyles {
            label_style: Style::default().fg(Color::Rgb(0xf1, 0xc4, 0x53)),
            title_style: Style::default()
                .fg(Color::Rgb(0xff, 0xe2, 0x92))
                .add_modifier(Modifier::BOLD),
            body_style: Style::default().fg(Color::Rgb(0xf1, 0xde, 0xb0)),
        },
        TranscriptRole::User => TranscriptVisualStyles {
            label_style: Style::default().fg(Color::Rgb(0x9f, 0xd7, 0xff)),
            title_style: Style::default()
                .fg(Color::Rgb(0xe0, 0xf1, 0xff))
                .add_modifier(Modifier::BOLD),
            body_style: Style::default().fg(Color::Rgb(0xe0, 0xf1, 0xff)),
        },
    }
}

#[cfg(test)]
mod tests {
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
}
