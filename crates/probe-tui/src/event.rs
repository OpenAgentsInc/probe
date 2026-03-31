use crossterm::event::{KeyCode, KeyEvent, KeyEventKind, KeyModifiers};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UiEvent {
    NextView,
    PreviousView,
    ToggleBody,
    RunBackgroundTask,
    OpenHelp,
    Dismiss,
    Quit,
    Tick,
}

pub fn event_from_key(key: KeyEvent) -> Option<UiEvent> {
    if key.kind != KeyEventKind::Press {
        return None;
    }

    match key.code {
        KeyCode::Tab | KeyCode::Right => Some(UiEvent::NextView),
        KeyCode::BackTab | KeyCode::Left => Some(UiEvent::PreviousView),
        KeyCode::Esc => Some(UiEvent::Dismiss),
        KeyCode::F(1) => Some(UiEvent::OpenHelp),
        KeyCode::Char('?') => Some(UiEvent::OpenHelp),
        KeyCode::Char('q') => Some(UiEvent::Quit),
        KeyCode::Char('r') => Some(UiEvent::RunBackgroundTask),
        KeyCode::Char('t') => Some(UiEvent::ToggleBody),
        KeyCode::Char('c') if key.modifiers.contains(KeyModifiers::CONTROL) => Some(UiEvent::Quit),
        KeyCode::Char(character) => match character.to_ascii_lowercase() {
            'q' => Some(UiEvent::Quit),
            'r' => Some(UiEvent::RunBackgroundTask),
            't' => Some(UiEvent::ToggleBody),
            _ => None,
        },
        _ => None,
    }
}
