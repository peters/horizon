use std::sync::mpsc;

use alacritty_terminal::event::Event;

use super::RuntimeTitle;

#[derive(Default)]
pub(super) struct ReplayRestoreState {
    pub(super) title: RuntimeTitle,
}

pub(super) fn drain_replay_events(event_rx: &mpsc::Receiver<Event>) -> ReplayRestoreState {
    let mut state = ReplayRestoreState::default();

    while let Ok(event) = event_rx.try_recv() {
        match event {
            Event::Title(title) => {
                let _ = state.title.apply_incoming(&title);
            }
            Event::ResetTitle => state.title.reset(),
            Event::ClipboardStore(_, _)
            | Event::ClipboardLoad(_, _)
            | Event::ColorRequest(_, _)
            | Event::PtyWrite(_)
            | Event::TextAreaSizeRequest(_)
            | Event::MouseCursorDirty
            | Event::CursorBlinkingChange
            | Event::Wakeup
            | Event::Bell
            | Event::Exit
            | Event::ChildExit(_) => {}
        }
    }

    state
}

#[cfg(test)]
mod tests {
    use std::sync::{Arc, mpsc};

    use alacritty_terminal::event::Event;
    use alacritty_terminal::sync::FairMutex;
    use alacritty_terminal::term::{self, Term};
    use alacritty_terminal::vte::ansi::Rgb;

    use super::drain_replay_events;
    use crate::terminal::support::replay_terminal_bytes;
    use crate::terminal::{TerminalDimensions, TerminalEventProxy};

    #[test]
    fn replaying_device_status_queries_emits_side_effect_events() {
        let (term, event_rx) = replay_test_term();

        replay_terminal_bytes(&term, b"\x1b[6n");

        let mut saw_cursor_report = false;
        while let Ok(event) = event_rx.try_recv() {
            if matches!(event, Event::PtyWrite(ref text) if text == "\x1b[1;1R") {
                saw_cursor_report = true;
                break;
            }
        }

        assert!(saw_cursor_report);
    }

    #[test]
    fn drain_replay_events_preserves_title_and_discards_side_effect_requests() {
        let formatter = |color: Rgb| format!("rgb:{:02x}/{:02x}/{:02x}", color.r, color.g, color.b);
        let (tx, rx) = mpsc::channel();

        // Populate the replay queue with the side-effect requests we need to drop.
        tx.send(Event::PtyWrite("\x1b[1;1R".to_string()))
            .expect("pty write event");
        tx.send(Event::ColorRequest(11, std::sync::Arc::new(formatter)))
            .expect("color request event");
        tx.send(Event::Title("restored title".to_string()))
            .expect("title event");

        let state = drain_replay_events(&rx);

        assert_eq!(state.title, super::RuntimeTitle::Open("restored title".to_string()));
        assert!(rx.try_recv().is_err());
    }

    #[test]
    fn drain_replay_events_keeps_horizon_title_over_later_ordinary_titles() {
        let (tx, rx) = mpsc::channel();

        tx.send(Event::Title("HORIZON_TITLE:set:PR comments".to_string()))
            .expect("horizon title event");
        tx.send(Event::Title(": horizon".to_string()))
            .expect("ordinary title event");
        tx.send(Event::ResetTitle).expect("reset title event");

        let state = drain_replay_events(&rx);

        assert_eq!(state.title, super::RuntimeTitle::Pinned("PR comments".to_string()));
        assert!(rx.try_recv().is_err());
    }

    fn replay_test_term() -> (Arc<FairMutex<Term<TerminalEventProxy>>>, mpsc::Receiver<Event>) {
        let (event_tx, event_rx) = mpsc::channel();
        let dimensions = TerminalDimensions::new(24, 80);
        let config = term::Config {
            scrolling_history: 256,
            kitty_keyboard: true,
            ..term::Config::default()
        };

        let term = Arc::new(FairMutex::new(Term::new(
            config,
            &dimensions,
            TerminalEventProxy { event_tx },
        )));

        (term, event_rx)
    }
}
