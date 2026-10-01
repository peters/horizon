use super::{ColorLookup, Event, HorizonOscTitle, Rgb, TermMode, Terminal, term};

impl Terminal {
    /// Drain pending PTY events. Returns `true` if any events were processed.
    #[profiling::function]
    pub fn process_events(&mut self) -> bool {
        let mut had_events = false;
        while let Ok(event) = self.event_rx.try_recv() {
            self.handle_event(event);
            had_events = true;
        }
        self.flush_pending_pty_resize();
        had_events
    }

    pub(super) fn parse_horizon_title(title: &str) -> Option<HorizonOscTitle> {
        // Retired protocol: older `horizon-notify` skills may still emit it,
        // and it must not surface as the panel title.
        if title.starts_with("HORIZON_NOTIFY:") {
            return Some(HorizonOscTitle::Ignore);
        }

        let payload = title.strip_prefix("HORIZON_TITLE:")?;

        if payload == "clear" {
            return Some(HorizonOscTitle::ClearTitle);
        }

        if let Some(next_title) = payload.strip_prefix("set:") {
            return Some(HorizonOscTitle::SetTitle(next_title.to_string()));
        }

        Some(HorizonOscTitle::Ignore)
    }

    #[must_use]
    pub fn title(&self) -> &str {
        self.title.as_str()
    }

    #[must_use]
    pub fn mode(&self) -> TermMode {
        *self.term.lock().mode()
    }

    pub fn set_focused(&mut self, focused: bool) {
        let mode = {
            let mut term = self.term.lock();
            if term.is_focused == focused {
                return;
            }

            term.is_focused = focused;
            *term.mode()
        };

        if mode.contains(TermMode::FOCUS_IN_OUT) {
            let sequence = if focused { b"\x1b[I" } else { b"\x1b[O" };
            self.write_protocol(sequence);
        }
    }

    pub(crate) fn handle_event(&mut self, event: Event) {
        match event {
            Event::Title(title) => self.title.apply_incoming(&title),
            Event::ResetTitle => self.title.reset(),
            Event::ClipboardStore(clipboard, contents) => match clipboard {
                term::ClipboardType::Clipboard => self.clipboard_contents = contents,
                term::ClipboardType::Selection => self.selection_contents = contents,
            },
            Event::ClipboardLoad(clipboard, formatter) => {
                let contents = match clipboard {
                    term::ClipboardType::Clipboard => self.clipboard_contents.as_str(),
                    term::ClipboardType::Selection => self.selection_contents.as_str(),
                };
                self.write_protocol(formatter(contents).as_bytes());
            }
            Event::ColorRequest(index, formatter) => {
                let color = self.color_for_request(index);
                self.write_protocol(formatter(color).as_bytes());
            }
            Event::PtyWrite(text) => {
                self.write_protocol(text.as_bytes());
            }
            Event::TextAreaSizeRequest(formatter) => {
                self.write_protocol(formatter(self.window_size()).as_bytes());
            }
            Event::Exit => {
                self.child_exited = true;
            }
            Event::ChildExit(status) => {
                self.child_exited = true;
                self.child_exit_status = Some(status);
            }
            Event::Bell | Event::MouseCursorDirty | Event::CursorBlinkingChange | Event::Wakeup => {}
        }
    }

    fn color_for_request(&self, index: usize) -> Rgb {
        self.term.lock().colors().lookup(index)
    }
}
