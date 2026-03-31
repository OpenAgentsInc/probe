use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UiEvent {
    NextView,
    PreviousView,
    ToggleBody,
    RunBackgroundTask,
    OpenHelp,
    OpenSetupOverlay,
    OpenApprovalOverlay,
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
    if key.kind != KeyEventKind::Press {
        return None;
    }

    let modifiers = key.modifiers;
    match key.code {
        KeyCode::Tab => Some(UiEvent::NextView),
        KeyCode::BackTab => Some(UiEvent::PreviousView),
        KeyCode::Esc => Some(UiEvent::Dismiss),
        KeyCode::F(1) => Some(UiEvent::OpenHelp),
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
