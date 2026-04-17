use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseEvent, MouseEventKind};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UiEvent {
    NextView,
    PreviousView,
    ToggleBody,
    RunBackgroundTask,
    OpenHelp,
    OpenSetupOverlay,
    OpenApprovalOverlay,
    ScrollUp,
    ScrollDown,
    PageUp,
    PageDown,
    Dismiss,
    Quit,
    ComposerInsert(char),
    ComposerBackspace,
    ComposerDelete,
    ComposerMoveLeft,
    ComposerMoveRight,
    ComposerHistoryPrevious,
    ComposerHistoryNext,
    ComposerAddAttachment,
    ComposerPaste(String),
    ComposerMoveHome,
    ComposerMoveEnd,
    ComposerNewline,
    ComposerSubmit,
    Tick,
}

pub fn event_from_key(key: KeyEvent) -> Option<UiEvent> {
    if key.kind == KeyEventKind::Release {
        return None;
    }

    let modifiers = key.modifiers;
    match key.code {
        KeyCode::Tab => Some(UiEvent::NextView),
        KeyCode::BackTab => Some(UiEvent::PreviousView),
        KeyCode::Esc => Some(UiEvent::Dismiss),
        KeyCode::F(1) => Some(UiEvent::OpenHelp),
        KeyCode::PageUp => Some(UiEvent::PageUp),
        KeyCode::PageDown => Some(UiEvent::PageDown),
        // Match OpenAI Codex TUI (`codex-rs/tui`): `ChatComposer` submits only on Enter with
        // **no** modifiers; any other Enter (Shift/Ctrl/Alt/Cmd/…) goes to `TextArea::input`, which
        // inserts `\n` for Enter with any modifiers. See `chat_composer.rs` (Enter+NONE submit)
        // and `textarea.rs` (`KeyCode::Enter, ..` => insert newline).
        KeyCode::Enter if modifiers.is_empty() => Some(UiEvent::ComposerSubmit),
        KeyCode::Enter => Some(UiEvent::ComposerNewline),
        // Some terminals surface modified Return as a raw CR/LF character instead of `KeyCode::Enter`.
        // Normalize those to the same newline path Codex's textarea takes for modified Enter.
        KeyCode::Char('\n') | KeyCode::Char('\r') => Some(UiEvent::ComposerNewline),
        // Codex textarea also maps ^J / ^M to newline for terminals that send C0 controls.
        KeyCode::Char('j') | KeyCode::Char('J') if modifiers.contains(KeyModifiers::CONTROL) => {
            Some(UiEvent::ComposerNewline)
        }
        KeyCode::Char('m') | KeyCode::Char('M') if modifiers.contains(KeyModifiers::CONTROL) => {
            Some(UiEvent::ComposerNewline)
        }
        KeyCode::Backspace => Some(UiEvent::ComposerBackspace),
        KeyCode::Delete => Some(UiEvent::ComposerDelete),
        KeyCode::Left => Some(UiEvent::ComposerMoveLeft),
        KeyCode::Right => Some(UiEvent::ComposerMoveRight),
        KeyCode::Up => Some(UiEvent::ComposerHistoryPrevious),
        KeyCode::Down => Some(UiEvent::ComposerHistoryNext),
        KeyCode::Home => Some(UiEvent::ComposerMoveHome),
        KeyCode::End => Some(UiEvent::ComposerMoveEnd),
        KeyCode::Char('a') if modifiers.contains(KeyModifiers::CONTROL) => {
            Some(UiEvent::OpenApprovalOverlay)
        }
        KeyCode::Char('c') if modifiers.contains(KeyModifiers::CONTROL) => Some(UiEvent::Quit),
        KeyCode::Char('o') if modifiers.contains(KeyModifiers::CONTROL) => {
            Some(UiEvent::ComposerAddAttachment)
        }
        KeyCode::Char('r') if modifiers.contains(KeyModifiers::CONTROL) => {
            Some(UiEvent::RunBackgroundTask)
        }
        KeyCode::Char('s') if modifiers.contains(KeyModifiers::CONTROL) => {
            Some(UiEvent::OpenSetupOverlay)
        }
        KeyCode::Char('t') if modifiers.contains(KeyModifiers::CONTROL) => {
            Some(UiEvent::ToggleBody)
        }
        KeyCode::Char(character)
            if !modifiers.contains(KeyModifiers::CONTROL)
                && !modifiers.contains(KeyModifiers::ALT) =>
        {
            Some(UiEvent::ComposerInsert(character))
        }
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    use super::{UiEvent, event_from_key};

    #[test]
    fn plain_enter_submits() {
        let event = event_from_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::NONE));
        assert_eq!(event, Some(UiEvent::ComposerSubmit));
    }

    #[test]
    fn shift_enter_inserts_newline() {
        let event = event_from_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::SHIFT));
        assert_eq!(event, Some(UiEvent::ComposerNewline));
    }

    #[test]
    fn ctrl_enter_inserts_newline() {
        let event = event_from_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::CONTROL));
        assert_eq!(event, Some(UiEvent::ComposerNewline));
    }

    #[test]
    fn alt_enter_inserts_newline() {
        let event = event_from_key(KeyEvent::new(KeyCode::Enter, KeyModifiers::ALT));
        assert_eq!(event, Some(UiEvent::ComposerNewline));
    }

    #[test]
    fn ctrl_j_inserts_newline() {
        let event = event_from_key(KeyEvent::new(KeyCode::Char('j'), KeyModifiers::CONTROL));
        assert_eq!(event, Some(UiEvent::ComposerNewline));
    }

    #[test]
    fn ctrl_m_inserts_newline() {
        let event = event_from_key(KeyEvent::new(KeyCode::Char('m'), KeyModifiers::CONTROL));
        assert_eq!(event, Some(UiEvent::ComposerNewline));
    }

    #[test]
    fn carriage_return_char_inserts_newline() {
        let event = event_from_key(KeyEvent::new(KeyCode::Char('\r'), KeyModifiers::SHIFT));
        assert_eq!(event, Some(UiEvent::ComposerNewline));
    }

    #[test]
    fn line_feed_char_inserts_newline() {
        let event = event_from_key(KeyEvent::new(KeyCode::Char('\n'), KeyModifiers::SHIFT));
        assert_eq!(event, Some(UiEvent::ComposerNewline));
    }
}

pub fn event_from_mouse(mouse: MouseEvent) -> Option<UiEvent> {
    match mouse.kind {
        MouseEventKind::ScrollUp => Some(UiEvent::ScrollUp),
        MouseEventKind::ScrollDown => Some(UiEvent::ScrollDown),
        _ => None,
    }
}
