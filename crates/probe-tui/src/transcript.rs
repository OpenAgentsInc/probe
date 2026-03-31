use ratatui::text::{Line, Text};

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

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TranscriptEntry {
    role: TranscriptRole,
    title: String,
    body: Vec<String>,
}

impl TranscriptEntry {
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
        let mut lines = vec![Line::from(format!(
            "[{}] {}",
            self.role.label(),
            self.title
        ))];
        for line in &self.body {
            lines.push(Line::from(format!("  {line}")));
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
        let mut lines = vec![Line::from(format!(
            "[active {}] {}",
            self.role.label(),
            self.title
        ))];
        for line in &self.body {
            lines.push(Line::from(format!("  {line}")));
        }
        lines
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct RetainedTranscript {
    entries: Vec<TranscriptEntry>,
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
        if self.entries.is_empty() && self.active_turn.is_none() {
            lines.push(Line::from("Transcript is empty."));
            lines.push(Line::from(""));
            lines.push(Line::from(
                "Type in the composer to start a real chat turn.",
            ));
            lines.push(Line::from(
                "Committed turns stay in app state while one explicit active turn renders live work.",
            ));
            return Text::from(lines);
        }

        for (index, entry) in self.entries.iter().enumerate() {
            if index > 0 {
                lines.push(Line::from(""));
            }
            lines.extend(entry.render_lines());
        }

        if let Some(active_turn) = &self.active_turn {
            if !lines.is_empty() {
                lines.push(Line::from(""));
            }
            lines.extend(active_turn.render_lines());
        }

        Text::from(lines)
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
            vec![String::from("Press r to start the Apple FM prove-out.")],
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
    fn retained_transcript_has_explicit_empty_state() {
        let transcript = RetainedTranscript::new();
        let rendered = lines_to_plain_text(&transcript);
        assert!(rendered.contains("Transcript is empty."));
        assert!(rendered.contains("Type in the composer"));
    }
}
