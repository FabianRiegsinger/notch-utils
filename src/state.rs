//! Notch state machine. Pure logic: no UI, no timers. The app layer feeds events in
//! and applies the resulting state to the window.

use crate::config::OpenMode;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum State {
    Collapsed,
    Peek,
    Expanded,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Event {
    PointerEnter,
    /// Fired by the app after the pointer stayed outside for `collapse_delay_ms`.
    LeaveTimeout,
    Click,
    /// Global hotkey, tray action or `--toggle` IPC.
    Toggle,
    Escape,
    FocusLost,
    /// A module wants attention (e.g. timer finished): peek if collapsed.
    Alert,
}

pub fn next(state: State, event: Event, mode: OpenMode) -> State {
    use Event::*;
    use State::*;
    match (event, mode) {
        (PointerEnter, OpenMode::PeekThenClick) if state == Collapsed => Peek,
        (PointerEnter, OpenMode::HoverFull) => Expanded,
        (PointerEnter, _) => state,
        // Click-only stays open until Esc, focus loss or toggle.
        (LeaveTimeout, OpenMode::ClickOnly) => state,
        (LeaveTimeout, _) => Collapsed,
        (Click, _) => Expanded,
        (Toggle, _) if state == Expanded => Collapsed,
        (Toggle, _) => Expanded,
        (Escape, _) => Collapsed,
        (FocusLost, _) if state == Expanded => Collapsed,
        (FocusLost, _) => state,
        (Alert, _) if state == Collapsed => Peek,
        (Alert, _) => state,
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use Event::*;
    use OpenMode::*;
    use State::*;

    #[test]
    fn peek_then_click() {
        assert_eq!(next(Collapsed, PointerEnter, PeekThenClick), Peek);
        assert_eq!(next(Peek, Click, PeekThenClick), Expanded);
        assert_eq!(next(Expanded, PointerEnter, PeekThenClick), Expanded);
        assert_eq!(next(Expanded, LeaveTimeout, PeekThenClick), Collapsed);
        assert_eq!(next(Peek, LeaveTimeout, PeekThenClick), Collapsed);
    }

    #[test]
    fn hover_full() {
        assert_eq!(next(Collapsed, PointerEnter, HoverFull), Expanded);
        assert_eq!(next(Expanded, LeaveTimeout, HoverFull), Collapsed);
    }

    #[test]
    fn click_only_ignores_hover() {
        assert_eq!(next(Collapsed, PointerEnter, ClickOnly), Collapsed);
        assert_eq!(next(Collapsed, Click, ClickOnly), Expanded);
        assert_eq!(next(Expanded, LeaveTimeout, ClickOnly), Expanded);
        assert_eq!(next(Expanded, FocusLost, ClickOnly), Collapsed);
    }

    #[test]
    fn toggle_and_escape_any_mode() {
        for m in [PeekThenClick, HoverFull, ClickOnly] {
            assert_eq!(next(Collapsed, Toggle, m), Expanded);
            assert_eq!(next(Peek, Toggle, m), Expanded);
            assert_eq!(next(Expanded, Toggle, m), Collapsed);
            assert_eq!(next(Expanded, Escape, m), Collapsed);
            assert_eq!(next(Peek, FocusLost, m), Peek);
            assert_eq!(next(Collapsed, Alert, m), Peek);
            assert_eq!(next(Expanded, Alert, m), Expanded);
        }
    }
}
