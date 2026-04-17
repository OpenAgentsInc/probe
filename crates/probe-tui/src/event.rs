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

fn enter_inserts_newline(modifiers: KeyModifiers) -> bool {
    // Shift+Enter is the primary binding, but many terminal emulators (notably macOS Terminal.app)
    // report Shift+Enter as plain Enter with no modifier bits. Ctrl+Enter, Option/Alt+Enter, and
    // Ctrl+J (Unix line-feed) are reliable fallbacks that still map to "newline in composer".
    modifiers.intersects(KeyModifiers::SHIFT | KeyModifiers::CONTROL | KeyModifiers::ALT)
}

pub fn event_from_key(key: KeyEvent) -> Option<UiEvent> {
    if key.kind != KeyEventKind::Press {
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
        KeyCode::Enter if enter_inserts_newline(modifiers) => Some(UiEvent::ComposerNewline),
        KeyCode::Enter => Some(UiEvent::ComposerSubmit),
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
        KeyCode::Char('j') | KeyCode::Char('J') if modifiers.contains(KeyModifiers::CONTROL) => {
            Some(UiEvent::ComposerNewline)
        }
        // Ctrl+M (carriage return) — some terminals send this instead of Enter+Ctrl for "newline".
        KeyCode::Char('m') | KeyCode::Char('M') if modifiers.contains(KeyModifiers::CONTROL) => {
            Some(UiEvent::ComposerNewline)
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
}

pub fn event_from_mouse(mouse: MouseEvent) -> Option<UiEvent> {
    match mouse.kind {
        MouseEventKind::ScrollUp => Some(UiEvent::ScrollUp),
        MouseEventKind::ScrollDown => Some(UiEvent::ScrollDown),
        _ => None,
    }
}
