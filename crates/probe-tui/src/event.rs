use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers, MouseEvent, MouseEventKind};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UiEvent {
    NextView,
    PreviousView,
    ToggleBody,
    RunBackgroundTask,
    OpenHelp,
    OpenStatusOverlay,
    OpenDoctorOverlay,
    OpenSetupOverlay,
    OpenApprovalOverlay,
    OpenGitOverlay,
    OpenRecipesOverlay,
    OpenTasksOverlay,
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
    PasteSystemClipboard,
    ComposerMoveHome,
    ComposerMoveEnd,
    ComposerNewline,
    ComposerSubmit,
    Tick,
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
        KeyCode::F(2) => Some(UiEvent::OpenStatusOverlay),
        KeyCode::F(3) => Some(UiEvent::OpenDoctorOverlay),
        KeyCode::F(4) => Some(UiEvent::OpenTasksOverlay),
        KeyCode::F(5) => Some(UiEvent::OpenRecipesOverlay),
        KeyCode::PageUp => Some(UiEvent::PageUp),
        KeyCode::PageDown => Some(UiEvent::PageDown),
        KeyCode::Enter if modifiers.contains(KeyModifiers::CONTROL) => {
            Some(UiEvent::ComposerNewline)
        }
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
        KeyCode::Char('g') if modifiers.contains(KeyModifiers::CONTROL) => {
            Some(UiEvent::OpenGitOverlay)
        }
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
        KeyCode::Char('v') if modifiers.contains(KeyModifiers::CONTROL) => {
            Some(UiEvent::PasteSystemClipboard)
        }
        KeyCode::Char('j') if modifiers.contains(KeyModifiers::CONTROL) => {
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

pub fn event_from_mouse(mouse: MouseEvent) -> Option<UiEvent> {
    match mouse.kind {
        MouseEventKind::ScrollUp => Some(UiEvent::ScrollUp),
        MouseEventKind::ScrollDown => Some(UiEvent::ScrollDown),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::{UiEvent, event_from_key};
    use crossterm::event::{KeyCode, KeyEvent, KeyModifiers};

    #[test]
    fn function_keys_map_to_operator_overlays() {
        assert_eq!(
            event_from_key(KeyEvent::new(KeyCode::F(2), KeyModifiers::NONE)),
            Some(UiEvent::OpenStatusOverlay)
        );
        assert_eq!(
            event_from_key(KeyEvent::new(KeyCode::F(3), KeyModifiers::NONE)),
            Some(UiEvent::OpenDoctorOverlay)
        );
        assert_eq!(
            event_from_key(KeyEvent::new(KeyCode::F(4), KeyModifiers::NONE)),
            Some(UiEvent::OpenTasksOverlay)
        );
        assert_eq!(
            event_from_key(KeyEvent::new(KeyCode::F(5), KeyModifiers::NONE)),
            Some(UiEvent::OpenRecipesOverlay)
        );
    }

    #[test]
    fn ctrl_g_opens_git_overlay() {
        assert_eq!(
            event_from_key(KeyEvent::new(KeyCode::Char('g'), KeyModifiers::CONTROL)),
            Some(UiEvent::OpenGitOverlay)
        );
    }
}
