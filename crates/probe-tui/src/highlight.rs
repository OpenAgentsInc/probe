use std::sync::OnceLock;

use ratatui::style::{Color as RtColor, Modifier, Style};
use ratatui::text::{Line, Span};
use syntect::easy::HighlightLines;
use syntect::highlighting::{Color as SyntectColor, FontStyle, Style as SyntectStyle, Theme};
use syntect::parsing::{SyntaxReference, SyntaxSet};
use syntect::util::LinesWithEndings;
use two_face::theme::EmbeddedThemeName;

const MAX_HIGHLIGHT_BYTES: usize = 512 * 1024;
const MAX_HIGHLIGHT_LINES: usize = 10_000;

static SYNTAX_SET: OnceLock<SyntaxSet> = OnceLock::new();
static THEME: OnceLock<Theme> = OnceLock::new();

fn syntax_set() -> &'static SyntaxSet {
    SYNTAX_SET.get_or_init(two_face::syntax::extra_newlines)
}

fn theme() -> &'static Theme {
    THEME.get_or_init(|| {
        two_face::theme::extra()
            .get(EmbeddedThemeName::CatppuccinMocha)
            .clone()
    })
}

#[allow(clippy::disallowed_methods)]
fn ansi_palette_color(index: u8) -> RtColor {
    match index {
        0x00 => RtColor::Black,
        0x01 => RtColor::Red,
        0x02 => RtColor::Green,
        0x03 => RtColor::Yellow,
        0x04 => RtColor::Blue,
        0x05 => RtColor::Magenta,
        0x06 => RtColor::Cyan,
        0x07 => RtColor::Gray,
        n => RtColor::Indexed(n),
    }
}

#[allow(clippy::disallowed_methods)]
fn convert_syntect_color(color: SyntectColor) -> Option<RtColor> {
    match color.a {
        0x00 => Some(ansi_palette_color(color.r)),
        0x01 => None,
        0xFF => Some(RtColor::Rgb(color.r, color.g, color.b)),
        _ => Some(RtColor::Rgb(color.r, color.g, color.b)),
    }
}

fn convert_style(syn_style: SyntectStyle) -> Style {
    let mut style = Style::default();
    if let Some(fg) = convert_syntect_color(syn_style.foreground) {
        style = style.fg(fg);
    }
    if syn_style.font_style.contains(FontStyle::BOLD) {
        style = style.add_modifier(Modifier::BOLD);
    }
    style
}

fn find_syntax(lang: &str) -> Option<&'static SyntaxReference> {
    let patched = match lang {
        "csharp" | "c-sharp" => "c#",
        "golang" => "go",
        "python3" => "python",
        "shell" => "bash",
        _ => lang,
    };
    let ss = syntax_set();
    ss.find_syntax_by_token(patched)
        .or_else(|| ss.find_syntax_by_name(patched))
        .or_else(|| {
            let lower = patched.to_ascii_lowercase();
            ss.syntaxes()
                .iter()
                .find(|syntax| syntax.name.to_ascii_lowercase() == lower)
        })
        .or_else(|| ss.find_syntax_by_extension(lang))
}

fn highlight_to_line_spans(code: &str, lang: &str) -> Option<Vec<Vec<Span<'static>>>> {
    if code.is_empty()
        || code.len() > MAX_HIGHLIGHT_BYTES
        || code.lines().count() > MAX_HIGHLIGHT_LINES
    {
        return None;
    }

    let syntax = find_syntax(lang)?;
    let mut highlighter = HighlightLines::new(syntax, theme());
    let mut lines = Vec::new();

    for raw_line in LinesWithEndings::from(code) {
        let ranges = highlighter.highlight_line(raw_line, syntax_set()).ok()?;
        let mut spans = Vec::new();
        for (style, text) in ranges {
            let text = text.trim_end_matches(['\n', '\r']);
            if text.is_empty() {
                continue;
            }
            spans.push(Span::styled(text.to_string(), convert_style(style)));
        }
        if spans.is_empty() {
            spans.push(Span::raw(String::new()));
        }
        lines.push(spans);
    }

    Some(lines)
}

pub(crate) fn highlight_code_to_lines(code: &str, lang: &str) -> Vec<Line<'static>> {
    if let Some(lines) = highlight_to_line_spans(code, lang) {
        lines.into_iter().map(Line::from).collect()
    } else {
        let mut lines = code
            .lines()
            .map(|line| Line::from(line.to_string()))
            .collect::<Vec<_>>();
        if lines.is_empty() {
            lines.push(Line::from(String::new()));
        }
        lines
    }
}

#[cfg(test)]
mod tests {
    use ratatui::style::Color;

    use super::highlight_code_to_lines;

    #[test]
    fn rust_highlighting_uses_theme_colors() {
        let lines = highlight_code_to_lines("fn main() { let answer = 42; }\n", "rust");
        let colors = lines[0]
            .spans
            .iter()
            .filter_map(|span| span.style.fg)
            .collect::<Vec<_>>();
        assert!(
            colors
                .iter()
                .any(|color| matches!(color, Color::Rgb(_, _, _) | Color::Cyan)),
            "expected syntax-highlighted spans, got {colors:?}"
        );
    }
}
