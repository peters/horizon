use super::{AgentNotification, ColorLookup, ContextEvent, Event, HorizonOscTitle, Rgb, TermMode, Terminal, term};

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

    /// Returns `true` if a bell has fired since the last call, then clears.
    pub fn take_bell(&mut self) -> bool {
        std::mem::take(&mut self.bell_pending)
    }

    pub fn take_notification(&mut self) -> Option<AgentNotification> {
        self.pending_notification.take()
    }

    /// Drain pending context events published by agents via OSC sequences.
    pub fn take_context_events(&mut self) -> Vec<ContextEvent> {
        std::mem::take(&mut self.pending_context_events)
    }

    pub(super) fn parse_horizon_title(title: &str) -> Option<HorizonOscTitle> {
        if let Some(payload) = title.strip_prefix("HORIZON_NOTIFY:") {
            let Some((severity, message)) = payload.split_once(':') else {
                return Some(HorizonOscTitle::Ignore);
            };
            if severity.is_empty() || message.is_empty() {
                return Some(HorizonOscTitle::Ignore);
            }
            return Some(HorizonOscTitle::Notification(AgentNotification {
                severity: severity.to_string(),
                message: message.to_string(),
            }));
        }

        if let Some(payload) = title.strip_prefix("HORIZON_CONTEXT:") {
            return Some(Self::parse_context_command(payload));
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

    /// Parse a `HORIZON_CONTEXT:publish:<key>:<value>` command.
    ///
    /// Values may contain colons, so we split on the first two colons only:
    /// `publish` `:` `key` `:` `everything-else-is-value`.
    fn parse_context_command(payload: &str) -> HorizonOscTitle {
        let Some(rest) = payload.strip_prefix("publish:") else {
            return HorizonOscTitle::Ignore;
        };
        let Some((key, value)) = rest.split_once(':') else {
            return HorizonOscTitle::Ignore;
        };
        if key.is_empty() || value.is_empty() {
            return HorizonOscTitle::Ignore;
        }
        HorizonOscTitle::ContextPublish {
            key: key.to_string(),
            value: value.to_string(),
        }
    }

    #[must_use]
    pub fn title(&self) -> &str {
        &self.title
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
            self.write_input(sequence);
        }
    }

    pub(super) fn handle_event(&mut self, event: Event) {
        match event {
            Event::Title(title) => match Self::parse_horizon_title(&title) {
                Some(HorizonOscTitle::Notification(notification)) => {
                    self.pending_notification = Some(notification);
                }
                Some(HorizonOscTitle::SetTitle(next_title)) => {
                    self.title = next_title;
                }
                Some(HorizonOscTitle::ClearTitle) => {
                    self.title.clear();
                }
                Some(HorizonOscTitle::ContextPublish { key, value }) => {
                    self.pending_context_events.push(ContextEvent { key, value });
                }
                Some(HorizonOscTitle::Ignore) => {}
                None => {
                    self.title = title;
                }
            },
            Event::ResetTitle => {
                self.title.clear();
            }
            Event::ClipboardStore(clipboard, contents) => match clipboard {
                term::ClipboardType::Clipboard => self.clipboard_contents = contents,
                term::ClipboardType::Selection => self.selection_contents = contents,
            },
            Event::ClipboardLoad(clipboard, formatter) => {
                let contents = match clipboard {
                    term::ClipboardType::Clipboard => self.clipboard_contents.as_str(),
                    term::ClipboardType::Selection => self.selection_contents.as_str(),
                };
                self.write_input(formatter(contents).as_bytes());
            }
            Event::ColorRequest(index, formatter) => {
                let color = self.color_for_request(index);
                self.write_input(formatter(color).as_bytes());
            }
            Event::PtyWrite(text) => {
                self.write_input(text.as_bytes());
            }
            Event::TextAreaSizeRequest(formatter) => {
                self.write_input(formatter(self.window_size()).as_bytes());
            }
            Event::Exit | Event::ChildExit(_) => {
                self.child_exited = true;
            }
            Event::Bell => {
                self.bell_pending = true;
            }
            Event::MouseCursorDirty | Event::CursorBlinkingChange | Event::Wakeup => {}
        }
    }

    fn color_for_request(&self, index: usize) -> Rgb {
        self.term.lock().colors().lookup(index)
    }
}
