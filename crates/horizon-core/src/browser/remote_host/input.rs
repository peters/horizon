//! What a panel command becomes on the channel of a browser host.
//!
//! The channel is the MCP contract of that host, so a command reaches the page
//! as the call of one tool. The contract is semantic: it takes a click on an
//! element, never a pointer position. A keyboard key, an untargeted page scroll
//! and a navigation cross it, and a pointer position does not, so this module
//! names that difference instead of dropping the command.

use horizon_browser::{
    BrowserCommand, BrowserEditCommand, BrowserInput, BrowserKey, BrowserKeyChord, BrowserModifiers,
};

use super::RemoteHostRefusal;

/// One call of the contract that carries a panel command to the page.
pub(super) struct HostAction {
    pub(super) tool: &'static str,
    pub(super) arguments: serde_json::Value,
}

impl HostAction {
    fn act(panel_id: &str, arguments: serde_json::Value) -> Self {
        let mut arguments = arguments;
        if let Some(object) = arguments.as_object_mut() {
            object.insert("panel_id".to_string(), serde_json::Value::String(panel_id.to_string()));
        }
        Self {
            tool: "browser_act",
            arguments,
        }
    }
}

/// The calls that carry `command` to the page of `panel_id`, or the reason this
/// channel cannot carry it.
///
/// # Errors
/// Returns [`RemoteHostRefusal::Pointer`] for a pointer position and
/// [`RemoteHostRefusal::Unsupported`] for a command that a page of a host does
/// not take from this panel.
pub(super) fn translate(command: &BrowserCommand, panel_id: &str) -> Result<Vec<HostAction>, RemoteHostRefusal> {
    let actions = match command {
        BrowserCommand::Navigate(url) => vec![HostAction {
            tool: "browser_navigate",
            arguments: serde_json::json!({ "panel_id": panel_id, "url": url }),
        }],
        BrowserCommand::Reload => vec![HostAction::act(panel_id, serde_json::json!({ "action": "reload" }))],
        BrowserCommand::Back => vec![HostAction::act(panel_id, serde_json::json!({ "action": "back" }))],
        BrowserCommand::Forward => vec![HostAction::act(panel_id, serde_json::json!({ "action": "forward" }))],
        BrowserCommand::SetViewport { width, height } => vec![HostAction {
            tool: "browser_resize",
            arguments: serde_json::json!({ "panel_id": panel_id, "width": width, "height": height }),
        }],
        BrowserCommand::Input(input) => input_actions(input, panel_id)?,
        BrowserCommand::Stop => Vec::new(),
        BrowserCommand::DropFiles { .. }
        | BrowserCommand::Video { .. }
        | BrowserCommand::Orientation { .. }
        | BrowserCommand::HandoffDone
        | BrowserCommand::NativeSelectChoose { .. }
        | BrowserCommand::NativeSelectDismiss => return Err(RemoteHostRefusal::Unsupported),
    };
    Ok(actions)
}

/// A press of one key or chord on the focused element.
fn press(panel_id: &str, key: &str) -> HostAction {
    HostAction::act(
        panel_id,
        serde_json::json!({ "action": "press", "key": key, "count": 1 }),
    )
}

fn input_actions(input: &BrowserInput, panel_id: &str) -> Result<Vec<HostAction>, RemoteHostRefusal> {
    Ok(match input {
        // The contract carries no pointer position, so a move changes nothing on
        // the page that this panel can see.
        BrowserInput::MouseMove { .. } | BrowserInput::KeyUp { .. } => Vec::new(),
        BrowserInput::MousePress { .. } | BrowserInput::MouseRelease { .. } => return Err(RemoteHostRefusal::Pointer),
        // A wheel over the page is a page scroll. The contract scrolls the page
        // and takes no point, so the deltas cross and the point does not.
        BrowserInput::Wheel { delta_x, delta_y, .. } => vec![HostAction::act(
            panel_id,
            serde_json::json!({ "action": "scroll", "delta_x": delta_x, "delta_y": delta_y }),
        )],
        BrowserInput::KeyDown {
            key,
            modifiers,
            edit_command,
            ..
        } => {
            let key = match edit_command {
                Some(edit) => chord_text(
                    BrowserKey::Char(edit_key(*edit)),
                    BrowserModifiers {
                        ctrl: true,
                        ..BrowserModifiers::none()
                    },
                ),
                None => chord_text(*key, *modifiers),
            };
            vec![press(panel_id, &key)]
        }
        // Text committed by an input method arrives as characters that the
        // contract takes one press each when the US layout holds them.
        BrowserInput::InsertText { text } => text
            .chars()
            .map(|character| {
                BrowserKeyChord::parse(character.to_string().as_str())
                    .map(|chord| press(panel_id, &chord_text(chord.key, chord.modifiers)))
                    .map_err(|_| RemoteHostRefusal::Text)
            })
            .collect::<Result<Vec<_>, _>>()?,
    })
}

const fn edit_key(edit: BrowserEditCommand) -> char {
    match edit {
        BrowserEditCommand::Copy => 'c',
        BrowserEditCommand::Cut => 'x',
        BrowserEditCommand::SelectAll => 'a',
    }
}

/// The text of one press, as `press` reads it: the `+`-joined modifiers of the
/// chord, then the key. A printable key keeps its US-layout base character, so
/// `Shift` travels as a modifier and never as the character itself.
pub(super) fn chord_text(key: BrowserKey, modifiers: BrowserModifiers) -> String {
    let mut text = String::new();
    for (held, name) in [
        (modifiers.ctrl, "Control"),
        (modifiers.alt, "Alt"),
        (modifiers.meta, "Meta"),
        (modifiers.shift, "Shift"),
    ] {
        if held {
            text.push_str(name);
            text.push('+');
        }
    }
    match key {
        BrowserKey::Char(character) => text.push(character),
        BrowserKey::ArrowUp => text.push_str("ArrowUp"),
        BrowserKey::ArrowDown => text.push_str("ArrowDown"),
        BrowserKey::ArrowLeft => text.push_str("ArrowLeft"),
        BrowserKey::ArrowRight => text.push_str("ArrowRight"),
        BrowserKey::Enter => text.push_str("Enter"),
        BrowserKey::Tab => text.push_str("Tab"),
        BrowserKey::Escape => text.push_str("Escape"),
        BrowserKey::Space => text.push_str("Space"),
        BrowserKey::Backspace => text.push_str("Backspace"),
        BrowserKey::Delete => text.push_str("Delete"),
        BrowserKey::Home => text.push_str("Home"),
        BrowserKey::End => text.push_str("End"),
        BrowserKey::PageUp => text.push_str("PageUp"),
        BrowserKey::PageDown => text.push_str("PageDown"),
        BrowserKey::Insert => text.push_str("Insert"),
        BrowserKey::F1 => text.push_str("F1"),
        BrowserKey::F2 => text.push_str("F2"),
        BrowserKey::F3 => text.push_str("F3"),
        BrowserKey::F4 => text.push_str("F4"),
        BrowserKey::F5 => text.push_str("F5"),
        BrowserKey::F6 => text.push_str("F6"),
        BrowserKey::F7 => text.push_str("F7"),
        BrowserKey::F8 => text.push_str("F8"),
        BrowserKey::F9 => text.push_str("F9"),
        BrowserKey::F10 => text.push_str("F10"),
        BrowserKey::F11 => text.push_str("F11"),
        BrowserKey::F12 => text.push_str("F12"),
        BrowserKey::F13 => text.push_str("F13"),
        BrowserKey::F14 => text.push_str("F14"),
        BrowserKey::F15 => text.push_str("F15"),
    }
    text
}
