use std::collections::VecDeque;
use std::sync::{Arc, Mutex};

use crate::frames::FrameSlot;

use super::BrowserCommand;

pub(super) const COMMAND_CAPACITY: usize = 512;

struct QueueState {
    commands: VecDeque<BrowserCommand>,
    sender_open: bool,
}

pub(super) struct CommandSender {
    state: Arc<Mutex<QueueState>>,
    frame_slot: Arc<FrameSlot>,
}

pub(crate) struct CommandReceiver {
    state: Arc<Mutex<QueueState>>,
}

pub(crate) struct CommandBatch {
    pub(crate) commands: Vec<BrowserCommand>,
    pub(crate) disconnected: bool,
}

pub(super) fn channel(frame_slot: Arc<FrameSlot>) -> (CommandSender, CommandReceiver) {
    let state = Arc::new(Mutex::new(QueueState {
        commands: VecDeque::with_capacity(COMMAND_CAPACITY),
        sender_open: true,
    }));
    (
        CommandSender {
            state: Arc::clone(&state),
            frame_slot,
        },
        CommandReceiver { state },
    )
}

impl CommandSender {
    pub(super) fn send(&self, command: BrowserCommand) -> bool {
        let mut state = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        if !state.sender_open {
            return false;
        }
        if coalesce_tail(&mut state.commands, &command) {
            self.frame_slot.record_command_coalesced();
            return true;
        }
        if state.commands.len() == COMMAND_CAPACITY {
            let eviction = state.commands.iter().position(is_discardable).or_else(|| {
                if is_state_closing(&command) {
                    state.commands.iter().position(|queued| !is_state_closing(queued))
                } else {
                    None
                }
            });
            if let Some(index) = eviction {
                state.commands.remove(index);
                self.frame_slot.record_command_coalesced();
            } else {
                self.frame_slot.record_command_rejected();
                return false;
            }
        }
        state.commands.push_back(command);
        true
    }
}

impl Drop for CommandSender {
    fn drop(&mut self) {
        self.state
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
            .sender_open = false;
    }
}

impl CommandReceiver {
    pub(crate) fn drain(&self, limit: usize) -> CommandBatch {
        let mut state = self.state.lock().unwrap_or_else(std::sync::PoisonError::into_inner);
        let take = limit.min(state.commands.len());
        let commands = state.commands.drain(..take).collect();
        CommandBatch {
            commands,
            disconnected: !state.sender_open && state.commands.is_empty(),
        }
    }
}

fn coalesce_tail(commands: &mut VecDeque<BrowserCommand>, incoming: &BrowserCommand) -> bool {
    let Some(last) = commands.back_mut() else {
        return false;
    };
    if matches!(incoming, BrowserCommand::SetViewport { .. }) && matches!(last, BrowserCommand::SetViewport { .. })
        || matches!(incoming, BrowserCommand::Input(crate::BrowserInput::MouseMove { .. }))
            && matches!(last, BrowserCommand::Input(crate::BrowserInput::MouseMove { .. }))
    {
        *last = incoming.clone();
        true
    } else {
        coalesce_hover_across_wheels(commands, incoming) || coalesce_wheel_burst(commands, incoming)
    }
}

fn coalesce_hover_across_wheels(commands: &mut VecDeque<BrowserCommand>, incoming: &BrowserCommand) -> bool {
    let BrowserCommand::Input(crate::BrowserInput::MouseMove { buttons: 0, .. }) = incoming else {
        return false;
    };
    let Some(index) = commands.iter().rposition(|command| {
        matches!(
            command,
            BrowserCommand::Input(crate::BrowserInput::MouseMove { buttons: 0, .. })
        )
    }) else {
        return false;
    };
    if !commands
        .iter()
        .skip(index + 1)
        .all(|command| matches!(command, BrowserCommand::Input(crate::BrowserInput::Wheel { .. })))
    {
        return false;
    }
    commands.remove(index);
    commands.push_back(incoming.clone());
    true
}

fn coalesce_wheel_burst(commands: &mut VecDeque<BrowserCommand>, incoming: &BrowserCommand) -> bool {
    let BrowserCommand::Input(crate::BrowserInput::Wheel {
        x,
        y,
        delta_x,
        delta_y,
        modifiers,
    }) = incoming
    else {
        return false;
    };
    let Some(index) = commands.iter().rposition(|command| {
        if matches!(
            command,
            BrowserCommand::Input(crate::BrowserInput::MouseMove { buttons: 0, .. })
        ) {
            return false;
        }
        matches!(
            command,
            BrowserCommand::Input(crate::BrowserInput::Wheel {
                delta_x: queued_x,
                delta_y: queued_y,
                modifiers: queued_modifiers,
                ..
            }) if queued_modifiers == modifiers
                && same_direction(*queued_x, *delta_x)
                && same_direction(*queued_y, *delta_y)
                && (*queued_x + *delta_x).is_finite()
                && (*queued_y + *delta_y).is_finite()
        )
    }) else {
        return false;
    };
    if !commands.iter().skip(index + 1).all(|command| {
        matches!(
            command,
            BrowserCommand::Input(crate::BrowserInput::MouseMove { buttons: 0, .. })
        )
    }) {
        return false;
    }
    let Some(BrowserCommand::Input(crate::BrowserInput::Wheel {
        delta_x: queued_x,
        delta_y: queued_y,
        ..
    })) = commands.remove(index)
    else {
        return false;
    };
    commands.push_back(BrowserCommand::Input(crate::BrowserInput::Wheel {
        x: *x,
        y: *y,
        delta_x: queued_x + delta_x,
        delta_y: queued_y + delta_y,
        modifiers: *modifiers,
    }));
    true
}

fn same_direction(left: f64, right: f64) -> bool {
    left == 0.0 || right == 0.0 || left.is_sign_positive() == right.is_sign_positive()
}

fn is_discardable(command: &BrowserCommand) -> bool {
    matches!(
        command,
        BrowserCommand::SetViewport { .. }
            | BrowserCommand::Input(
                crate::BrowserInput::MouseMove { .. }
                    | crate::BrowserInput::Wheel { .. }
                    | crate::BrowserInput::KeyDown { repeat: true, .. }
            )
    )
}

fn is_state_closing(command: &BrowserCommand) -> bool {
    matches!(
        command,
        BrowserCommand::Input(crate::BrowserInput::MouseRelease { .. } | crate::BrowserInput::KeyUp { .. })
    )
}

#[cfg(test)]
mod tests {
    use std::sync::Arc;

    use crate::{BrowserButton, BrowserInput, BrowserModifiers, FrameSlot};

    use super::{BrowserCommand, COMMAND_CAPACITY, channel};

    #[test]
    fn consecutive_motion_and_resize_keep_only_the_newest_command() {
        let frame_slot = Arc::new(FrameSlot::new());
        let (sender, receiver) = channel(Arc::clone(&frame_slot));
        assert!(sender.send(BrowserCommand::SetViewport {
            width: 800,
            height: 600,
        }));
        assert!(sender.send(BrowserCommand::SetViewport {
            width: 900,
            height: 700,
        }));
        let move_command = |x| {
            BrowserCommand::Input(BrowserInput::MouseMove {
                x,
                y: 2.0,
                buttons: 0,
                modifiers: BrowserModifiers::none(),
            })
        };
        assert!(sender.send(move_command(1.0)));
        assert!(sender.send(move_command(3.0)));

        let batch = receiver.drain(10);
        assert_eq!(batch.commands.len(), 2);
        assert!(matches!(
            batch.commands[0],
            BrowserCommand::SetViewport {
                width: 900,
                height: 700
            }
        ));
        assert!(matches!(
            batch.commands[1],
            BrowserCommand::Input(BrowserInput::MouseMove { x: 3.0, .. })
        ));
        assert_eq!(frame_slot.metrics().commands_coalesced, 2);
    }

    #[test]
    fn hover_and_same_direction_wheel_bursts_stay_bounded() {
        let frame_slot = Arc::new(FrameSlot::new());
        let (sender, receiver) = channel(Arc::clone(&frame_slot));
        let modifiers = BrowserModifiers::none();
        for index in 0..20 {
            assert!(sender.send(BrowserCommand::Input(BrowserInput::MouseMove {
                x: f64::from(index),
                y: 2.0,
                buttons: 0,
                modifiers,
            })));
            assert!(sender.send(BrowserCommand::Input(BrowserInput::Wheel {
                x: f64::from(index),
                y: 2.0,
                delta_x: 0.0,
                delta_y: 16.0,
                modifiers,
            })));
        }
        assert!(sender.send(BrowserCommand::Input(BrowserInput::Wheel {
            x: 20.0,
            y: 2.0,
            delta_x: 0.0,
            delta_y: -16.0,
            modifiers,
        })));

        let batch = receiver.drain(10);
        assert_eq!(batch.commands.len(), 3);
        assert!(matches!(
            batch.commands[0],
            BrowserCommand::Input(BrowserInput::MouseMove { x: 19.0, .. })
        ));
        assert!(matches!(
            batch.commands[1],
            BrowserCommand::Input(BrowserInput::Wheel { delta_y: 320.0, .. })
        ));
        assert!(matches!(
            batch.commands[2],
            BrowserCommand::Input(BrowserInput::Wheel { delta_y: -16.0, .. })
        ));
        assert_eq!(frame_slot.metrics().commands_coalesced, 38);
    }

    #[test]
    fn closed_sender_drains_queued_commands_before_disconnect() {
        let frame_slot = Arc::new(FrameSlot::new());
        let (sender, receiver) = channel(frame_slot);
        assert!(sender.send(BrowserCommand::Back));
        assert!(sender.send(BrowserCommand::Forward));
        drop(sender);

        let first = receiver.drain(1);
        assert_eq!(first.commands.len(), 1);
        assert!(!first.disconnected);
        let second = receiver.drain(1);
        assert_eq!(second.commands.len(), 1);
        assert!(second.disconnected);
    }

    #[test]
    fn bounded_queue_evicts_stale_motion_before_rejecting_control_commands() {
        let frame_slot = Arc::new(FrameSlot::new());
        let (sender, receiver) = channel(Arc::clone(&frame_slot));
        assert!(sender.send(BrowserCommand::Input(BrowserInput::MouseMove {
            x: 1.0,
            y: 1.0,
            buttons: 0,
            modifiers: BrowserModifiers::none(),
        })));
        for index in 1..COMMAND_CAPACITY {
            assert!(sender.send(BrowserCommand::Navigate(format!("https://example.test/{index}"))));
        }
        assert!(sender.send(BrowserCommand::Reload));

        let batch = receiver.drain(COMMAND_CAPACITY + 1);
        assert_eq!(batch.commands.len(), COMMAND_CAPACITY);
        assert!(matches!(batch.commands.last(), Some(BrowserCommand::Reload)));
        assert_eq!(frame_slot.metrics().commands_coalesced, 1);
        assert_eq!(frame_slot.metrics().commands_rejected, 0);
    }

    #[test]
    fn bounded_queue_never_drops_a_pointer_release() {
        let frame_slot = Arc::new(FrameSlot::new());
        let (sender, receiver) = channel(Arc::clone(&frame_slot));
        let modifiers = BrowserModifiers::none();
        assert!(sender.send(BrowserCommand::Input(BrowserInput::MousePress {
            x: 1.0,
            y: 1.0,
            button: BrowserButton::Left,
            click_count: 1,
            buttons: 1,
            modifiers,
        })));
        for _ in 1..COMMAND_CAPACITY {
            assert!(sender.send(BrowserCommand::Input(BrowserInput::Wheel {
                x: 1.0,
                y: 1.0,
                delta_x: 0.0,
                delta_y: 1.0,
                modifiers,
            })));
        }
        assert!(sender.send(BrowserCommand::Input(BrowserInput::MouseRelease {
            x: 1.0,
            y: 1.0,
            button: BrowserButton::Left,
            click_count: 1,
            buttons: 0,
            modifiers,
        })));

        let batch = receiver.drain(COMMAND_CAPACITY);
        assert!(matches!(
            batch.commands.last(),
            Some(BrowserCommand::Input(BrowserInput::MouseRelease { .. }))
        ));
        assert_eq!(frame_slot.metrics().commands_rejected, 0);
    }
}
