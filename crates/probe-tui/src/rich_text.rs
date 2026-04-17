use std::mem;

use pulldown_cmark::{CodeBlockKind, Event, HeadingLevel, Options, Parser, Tag, TagEnd};
use ratatui::style::{Modifier, Style};
use ratatui::text::{Line, Span};

use crate::highlight::highlight_code_to_lines;
use crate::theme;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum ListKind {
    Ordered(usize),
    Unordered,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct CodeBlockState {
    language: Option<String>,
    content: String,
}

#[derive(Debug, Clone)]
struct MarkdownWriter {
    lines: Vec<Line<'static>>,
    current: Vec<Span<'static>>,
    styles: Vec<Style>,
    lists: Vec<ListKind>,
    pending_marker: Option<Vec<Span<'static>>>,
    blockquote_depth: usize,
    line_has_content: bool,
    code_block: Option<CodeBlockState>,
}

impl MarkdownWriter {
    fn new() -> Self {
        Self {
            lines: Vec::new(),
            current: Vec::new(),
            styles: vec![Style::default()],
            lists: Vec::new(),
            pending_marker: None,
            blockquote_depth: 0,
            line_has_content: false,
            code_block: None,
        }
    }

    fn run(mut self, input: &str) -> Vec<Line<'static>> {
        let mut options = Options::empty();
        options.insert(Options::ENABLE_STRIKETHROUGH);
        let parser = Parser::new_ext(input, options);

        for event in parser {
            match event {
                Event::Start(tag) => self.start(tag),
                Event::End(tag) => self.end(tag),
                Event::Text(text) => self.text(text.as_ref()),
                Event::Code(text) => self.inline_code(text.as_ref()),
                Event::SoftBreak | Event::HardBreak => self.newline(),
                Event::Rule => {
                    self.finish_line_if_needed();
                    self.lines.push(Line::from("───".to_string()));
                }
                Event::Html(_) | Event::InlineHtml(_) | Event::FootnoteReference(_) => {}
                Event::TaskListMarker(checked) => {
                    self.push_spans(vec![Span::styled(
                        if checked { "✔ " } else { "□ " },
                        if checked {
                            theme::success_icon()
                        } else {
                            theme::warning_icon()
                        },
                    )]);
                }
            }
        }

        if let Some(code_block) = self.code_block.take() {
            self.flush_code_block(code_block);
        }
        self.finish_line_if_needed();
        trim_empty_lines(self.lines)
    }

    fn start(&mut self, tag: Tag<'_>) {
        match tag {
            Tag::Paragraph => {}
            Tag::Heading { level, .. } => {
                self.finish_line_if_needed();
                self.styles.push(theme::heading(heading_level(level)));
            }
            Tag::BlockQuote => {
                self.finish_line_if_needed();
                self.blockquote_depth += 1;
            }
            Tag::CodeBlock(kind) => {
                self.finish_line_if_needed();
                let language = match kind {
                    CodeBlockKind::Fenced(language) => {
                        let language = language.trim();
                        (!language.is_empty()).then(|| language.to_string())
                    }
                    CodeBlockKind::Indented => None,
                };
                self.code_block = Some(CodeBlockState {
                    language,
                    content: String::new(),
                });
            }
            Tag::Emphasis => self.push_style(Style::default().add_modifier(Modifier::ITALIC)),
            Tag::Strong => self.push_style(Style::default().add_modifier(Modifier::BOLD)),
            Tag::Strikethrough => {
                self.push_style(Style::default().add_modifier(Modifier::CROSSED_OUT))
            }
            Tag::Link { .. } => self.push_style(theme::link()),
            Tag::List(start) => {
                self.finish_line_if_needed();
                self.lists.push(match start {
                    Some(start) => ListKind::Ordered(start as usize),
                    None => ListKind::Unordered,
                });
            }
            Tag::Item => {
                self.finish_line_if_needed();
                self.pending_marker = Some(self.list_marker());
            }
            _ => {}
        }
    }

    fn end(&mut self, tag: TagEnd) {
        match tag {
            TagEnd::Paragraph => self.blank_line(),
            TagEnd::Heading(_) => {
                self.pop_style();
                self.blank_line();
            }
            TagEnd::BlockQuote => {
                self.finish_line_if_needed();
                self.blockquote_depth = self.blockquote_depth.saturating_sub(1);
                self.blank_line();
            }
            TagEnd::CodeBlock => {
                if let Some(code_block) = self.code_block.take() {
                    self.flush_code_block(code_block);
                    self.blank_line();
                }
            }
            TagEnd::Emphasis | TagEnd::Strong | TagEnd::Strikethrough | TagEnd::Link => {
                self.pop_style();
            }
            TagEnd::List(_) => {
                self.finish_line_if_needed();
                self.lists.pop();
                self.blank_line();
            }
            TagEnd::Item => self.finish_line_if_needed(),
            _ => {}
        }
    }

    fn text(&mut self, value: &str) {
        if let Some(code_block) = self.code_block.as_mut() {
            code_block.content.push_str(value);
            return;
        }
        for (index, segment) in value.split('\n').enumerate() {
            if index > 0 {
                self.newline();
            }
            if !segment.is_empty() {
                self.push_spans(highlight_inline_spans(segment, self.current_style()));
            }
        }
    }

    fn inline_code(&mut self, value: &str) {
        self.push_spans(vec![Span::styled(
            value.to_string(),
            self.current_style().patch(theme::inline_code()),
        )]);
    }

    fn push_style(&mut self, style: Style) {
        self.styles.push(self.current_style().patch(style));
    }

    fn pop_style(&mut self) {
        if self.styles.len() > 1 {
            self.styles.pop();
        }
    }

    fn current_style(&self) -> Style {
        self.styles.last().copied().unwrap_or_default()
    }

    fn push_spans(&mut self, spans: Vec<Span<'static>>) {
        self.ensure_prefix();
        self.current.extend(spans);
        self.line_has_content = true;
    }

    fn ensure_prefix(&mut self) {
        if self.line_has_content {
            return;
        }
        if self.blockquote_depth > 0 {
            self.current.push(Span::styled(
                "> ".repeat(self.blockquote_depth),
                theme::blockquote(),
            ));
        }
        if let Some(marker) = self.pending_marker.take() {
            self.current.extend(marker);
        }
    }

    fn list_marker(&mut self) -> Vec<Span<'static>> {
        match self.lists.last_mut() {
            Some(ListKind::Ordered(next)) => {
                let marker = format!("{next}. ");
                *next += 1;
                vec![Span::styled(marker, theme::ordered_list_marker())]
            }
            Some(ListKind::Unordered) => vec![Span::raw("• ".to_string())],
            None => Vec::new(),
        }
    }

    fn newline(&mut self) {
        self.lines.push(Line::from(mem::take(&mut self.current)));
        self.line_has_content = false;
    }

    fn finish_line_if_needed(&mut self) {
        if self.line_has_content || !self.current.is_empty() {
            self.newline();
        }
    }

    fn blank_line(&mut self) {
        self.finish_line_if_needed();
        if self.lines.last().is_none_or(|line| !line.spans.is_empty()) {
            self.lines.push(Line::from(String::new()));
        }
    }

    fn flush_code_block(&mut self, code_block: CodeBlockState) {
        let language = code_block.language.as_deref().unwrap_or("text");
        for line in highlight_code_to_lines(code_block.content.as_str(), language) {
            self.lines.push(line);
        }
    }
}

pub(crate) fn render_markdownish_lines(input: &str) -> Vec<Line<'static>> {
    if input.trim().is_empty() {
        return Vec::new();
    }
    MarkdownWriter::new().run(input)
}

pub(crate) fn highlight_plain_lines(input: &str) -> Vec<Line<'static>> {
    let mut lines = input
        .split('\n')
        .map(|line| highlight_inline_line(line, Style::default()))
        .collect::<Vec<_>>();
    if lines.is_empty() {
        lines.push(Line::from(String::new()));
    }
    lines
}

pub(crate) fn highlight_inline_line(input: &str, base: Style) -> Line<'static> {
    Line::from(highlight_inline_spans(input, base))
}

pub(crate) fn highlight_inline_spans(input: &str, base: Style) -> Vec<Span<'static>> {
    let mut spans = Vec::new();
    let mut rest = input;
    loop {
        let Some(start) = rest.find('`') else {
            tokenize_plain_segment(&mut spans, rest, base);
            break;
        };
        let (before, after_tick) = rest.split_at(start);
        tokenize_plain_segment(&mut spans, before, base);
        let after_tick = &after_tick[1..];
        let Some(end) = after_tick.find('`') else {
            spans.push(Span::styled("`".to_string(), base));
            tokenize_plain_segment(&mut spans, after_tick, base);
            break;
        };
        let (code, remainder) = after_tick.split_at(end);
        spans.push(Span::styled(
            code.to_string(),
            base.patch(theme::inline_code()),
        ));
        rest = &remainder[1..];
    }
    spans
}

fn tokenize_plain_segment(spans: &mut Vec<Span<'static>>, segment: &str, base: Style) {
    let mut token = String::new();
    for ch in segment.chars() {
        if ch.is_whitespace() {
            flush_token(spans, &mut token, base);
            spans.push(Span::styled(ch.to_string(), base));
        } else {
            token.push(ch);
        }
    }
    flush_token(spans, &mut token, base);
}

fn flush_token(spans: &mut Vec<Span<'static>>, token: &mut String, base: Style) {
    if token.is_empty() {
        return;
    }

    let owned = mem::take(token);
    // Use character boundaries only. The previous logic used `rfind(..) + 1` as an exclusive
    // end byte index, which is only valid for ASCII; multi-byte punctuation (e.g. U+201C “)
    // would slice inside a UTF-8 character and panic.
    let start_byte = owned
        .char_indices()
        .find(|&(_, ch)| !leading_punctuation(ch))
        .map(|(bi, _)| bi)
        .unwrap_or(owned.len());
    let end_byte = owned
        .char_indices()
        .rev()
        .find(|&(_, ch)| !trailing_punctuation(ch))
        .map(|(bi, ch)| bi + ch.len_utf8())
        .unwrap_or(0);

    if start_byte >= end_byte {
        spans.push(Span::styled(owned, base));
        return;
    }

    let prefix = &owned[..start_byte];
    let core = &owned[start_byte..end_byte];
    let suffix = &owned[end_byte..];

    if !prefix.is_empty() {
        spans.push(Span::styled(prefix.to_string(), base));
    }
    spans.push(Span::styled(
        core.to_string(),
        base.patch(classify_token(core)),
    ));
    if !suffix.is_empty() {
        spans.push(Span::styled(suffix.to_string(), base));
    }
}

fn leading_punctuation(ch: char) -> bool {
    ch.is_ascii_punctuation() && !matches!(ch, '/' | '@' | '#' | '.' | '~')
}

fn trailing_punctuation(ch: char) -> bool {
    ch.is_ascii_punctuation() && !matches!(ch, '/' | '@' | '#' | '.' | '_' | '-')
}

fn classify_token(token: &str) -> Style {
    if is_link_like(token) {
        return theme::link();
    }
    if is_slash_command(token) {
        return theme::slash_command();
    }
    if is_mention(token) {
        return theme::mention();
    }
    if is_issue_ref(token) {
        return theme::issue_ref();
    }
    if is_path_like(token) {
        return theme::path();
    }
    Style::default()
}

fn is_link_like(token: &str) -> bool {
    token.starts_with("http://") || token.starts_with("https://") || token.starts_with("www.")
}

fn is_slash_command(token: &str) -> bool {
    let Some(rest) = token.strip_prefix('/') else {
        return false;
    };
    !rest.is_empty()
        && rest
            .chars()
            .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_'))
}

fn is_mention(token: &str) -> bool {
    token.starts_with("@skill:") || token.starts_with("@app:") || token.starts_with("@runtime:")
}

fn is_issue_ref(token: &str) -> bool {
    let Some(index) = token.rfind('#') else {
        return false;
    };
    let (head, tail) = token.split_at(index);
    let digits = &tail[1..];
    !digits.is_empty()
        && digits.chars().all(|ch| ch.is_ascii_digit())
        && (head.is_empty()
            || head
                .chars()
                .all(|ch| ch.is_ascii_alphanumeric() || matches!(ch, '-' | '_' | '.')))
}

fn is_path_like(token: &str) -> bool {
    token.starts_with("./")
        || token.starts_with("../")
        || token.starts_with("~/")
        || token.contains('/')
        || matches!(
            token.rsplit_once('.').map(|(_, ext)| ext),
            Some(
                "rs" | "md"
                    | "toml"
                    | "json"
                    | "yaml"
                    | "yml"
                    | "txt"
                    | "lock"
                    | "tsx"
                    | "ts"
                    | "js"
                    | "jsx"
            )
        )
}

fn heading_level(level: HeadingLevel) -> u8 {
    match level {
        HeadingLevel::H1 => 1,
        HeadingLevel::H2 => 2,
        HeadingLevel::H3 => 3,
        HeadingLevel::H4 => 4,
        HeadingLevel::H5 => 5,
        HeadingLevel::H6 => 6,
    }
}

fn trim_empty_lines(mut lines: Vec<Line<'static>>) -> Vec<Line<'static>> {
    while lines.first().is_some_and(|line| line.spans.is_empty()) {
        lines.remove(0);
    }
    while lines.last().is_some_and(|line| line.spans.is_empty()) {
        lines.pop();
    }
    lines
}

#[cfg(test)]
mod tests {
    use ratatui::style::{Color, Modifier, Style};

    use super::{highlight_inline_line, render_markdownish_lines};

    #[test]
    fn inline_highlight_does_not_panic_on_unicode_quote_tokens() {
        let line = highlight_inline_line("\u{201C}panic?\u{201D}", Style::default());
        assert!(
            !line.spans.is_empty(),
            "expected spans for curly-quote token"
        );
    }

    #[test]
    fn inline_highlighting_styles_commands_mentions_paths_and_issues() {
        let line = highlight_inline_line(
            "/plan @skill:rust probe#119 crates/probe-tui/src/lib.rs",
            Style::default(),
        );
        assert_eq!(line.spans[0].style.fg, Some(Color::Magenta));
        assert_eq!(line.spans[2].style.fg, Some(Color::Magenta));
        assert_eq!(line.spans[4].style.fg, Some(Color::Cyan));
        assert_eq!(line.spans[6].style.fg, Some(Color::Cyan));
    }

    #[test]
    fn markdown_render_highlights_links_quotes_and_code_blocks() {
        let lines = render_markdownish_lines(
            "> quoted\n\nSee [README](https://example.com).\n\n```rust\nfn main() {}\n```",
        );
        assert_eq!(lines[0].spans[0].style.fg, Some(Color::Green));
        assert!(
            lines[2]
                .spans
                .iter()
                .any(|span| span.style.fg == Some(Color::Cyan))
        );
        assert!(lines[4].spans.iter().any(
            |span| span.style.fg.is_some() || span.style.add_modifier.contains(Modifier::BOLD)
        ));
    }
}
