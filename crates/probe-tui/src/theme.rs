use ratatui::style::{Color, Modifier, Style};

pub(crate) fn shell_border() -> Style {
    Style::default().fg(Color::Cyan)
}

pub(crate) fn shell_accent() -> Style {
    Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::BOLD)
}

pub(crate) fn subtle() -> Style {
    Style::default()
        .fg(Color::DarkGray)
        .add_modifier(Modifier::DIM)
}

pub(crate) fn placeholder() -> Style {
    subtle().add_modifier(Modifier::ITALIC)
}

pub(crate) fn helper() -> Style {
    subtle()
}

pub(crate) fn transcript_label() -> Style {
    subtle()
}

pub(crate) fn user_title() -> Style {
    Style::default().fg(Color::Cyan)
}

pub(crate) fn assistant_title() -> Style {
    Style::default()
}

pub(crate) fn tool_title() -> Style {
    Style::default().fg(Color::Magenta)
}

pub(crate) fn system_title() -> Style {
    Style::default().fg(Color::Yellow)
}

pub(crate) fn status_title() -> Style {
    Style::default().fg(Color::Yellow)
}

pub(crate) fn active_label() -> Style {
    Style::default()
        .fg(Color::Yellow)
        .add_modifier(Modifier::DIM)
}

pub(crate) fn inline_code() -> Style {
    Style::default().fg(Color::Cyan)
}

pub(crate) fn link() -> Style {
    Style::default()
        .fg(Color::Cyan)
        .add_modifier(Modifier::UNDERLINED)
}

pub(crate) fn blockquote() -> Style {
    Style::default().fg(Color::Green)
}

pub(crate) fn ordered_list_marker() -> Style {
    Style::default().fg(Color::LightBlue)
}

pub(crate) fn heading(level: u8) -> Style {
    match level {
        1 => Style::default().add_modifier(Modifier::BOLD | Modifier::UNDERLINED),
        2 => Style::default().add_modifier(Modifier::BOLD),
        3 => Style::default().add_modifier(Modifier::BOLD | Modifier::ITALIC),
        _ => Style::default().add_modifier(Modifier::ITALIC),
    }
}

pub(crate) fn slash_command() -> Style {
    Style::default().fg(Color::Magenta)
}

pub(crate) fn mention() -> Style {
    Style::default().fg(Color::Magenta)
}

pub(crate) fn issue_ref() -> Style {
    Style::default().fg(Color::Cyan)
}

pub(crate) fn path() -> Style {
    Style::default().fg(Color::Cyan)
}

pub(crate) fn model() -> Style {
    Style::default().fg(Color::Cyan)
}

pub(crate) fn reasoning() -> Style {
    Style::default().fg(Color::Magenta)
}

pub(crate) fn draft_meta() -> Style {
    subtle()
}

pub(crate) fn state_busy() -> Style {
    Style::default().fg(Color::Yellow)
}

pub(crate) fn state_locked() -> Style {
    Style::default().fg(Color::Red)
}

pub(crate) fn footer_separator() -> Style {
    subtle()
}

pub(crate) fn status_icon(icon: char) -> Style {
    match icon {
        '!' => Style::default()
            .fg(Color::Yellow)
            .add_modifier(Modifier::BOLD),
        'x' | '✗' => Style::default().fg(Color::Red).add_modifier(Modifier::BOLD),
        '*' | '•' => Style::default()
            .fg(Color::Cyan)
            .add_modifier(Modifier::BOLD),
        '.' | '·' => Style::default()
            .fg(Color::Green)
            .add_modifier(Modifier::BOLD),
        _ => subtle(),
    }
}

pub(crate) fn success_icon() -> Style {
    Style::default()
        .fg(Color::Green)
        .add_modifier(Modifier::BOLD)
}

pub(crate) fn warning_icon() -> Style {
    Style::default()
        .fg(Color::Yellow)
        .add_modifier(Modifier::BOLD)
}
