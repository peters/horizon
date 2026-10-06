//! A minimal X11 text receiver for live input tests.
//!
//! It models a client that reads key events late and translates each key with
//! the keymap that the server holds at translation time, as clients that fetch
//! the keymap after a mapping notification do.

use std::time::{Duration, Instant};
use x11rb::{
    connection::Connection,
    protocol::{
        Event,
        xproto::{ConnectionExt, CreateWindowAux, EventMask, InputFocus, KeyButMask, KeyPressEvent, WindowClass},
    },
    rust_connection::RustConnection,
};

pub type TestResult<T> = std::result::Result<T, Box<dyn std::error::Error + Send + Sync>>;

pub struct Receiver {
    pub connection: RustConnection,
    pending: Vec<(Instant, KeyPressEvent)>,
}

impl Receiver {
    /// Opens a focused window on `display` that receives key presses.
    pub fn open(display: &str) -> TestResult<Self> {
        let (connection, screen) = x11rb::connect(Some(display))?;
        let root = connection.setup().roots[screen].root;
        let window = connection.generate_id()?;
        connection
            .create_window(
                x11rb::COPY_DEPTH_FROM_PARENT,
                window,
                root,
                0,
                0,
                100,
                100,
                0,
                WindowClass::INPUT_OUTPUT,
                0,
                &CreateWindowAux::new()
                    .override_redirect(1)
                    .event_mask(EventMask::KEY_PRESS),
            )?
            .check()?;
        connection.map_window(window)?.check()?;
        connection
            .set_input_focus(InputFocus::PARENT, window, x11rb::CURRENT_TIME)?
            .check()?;
        Ok(Self {
            connection,
            pending: Vec::new(),
        })
    }

    /// Reads queued key presses and translates those older than `lag`.
    pub fn pump(&mut self, lag: Duration, text: &mut String) -> TestResult<()> {
        while let Some(event) = self.connection.poll_for_event()? {
            if let Event::KeyPress(event) = event {
                self.pending.push((Instant::now(), event));
            }
        }
        let ready = self
            .pending
            .iter()
            .take_while(|(received, _)| received.elapsed() >= lag)
            .count();
        for (_, event) in self.pending.drain(..ready).collect::<Vec<_>>() {
            if let Some(character) = self.translate(&event)? {
                text.push(character);
            }
        }
        Ok(())
    }

    /// Pumps until `done` is true and every received key has been translated.
    pub fn collect_while(&mut self, lag: Duration, done: impl Fn() -> bool) -> TestResult<String> {
        let deadline = Instant::now() + Duration::from_secs(120);
        let mut text = String::new();
        let mut finished: Option<Instant> = None;
        while Instant::now() < deadline {
            std::thread::sleep(Duration::from_millis(10));
            self.pump(lag, &mut text)?;
            if finished.is_none() && done() {
                finished = Some(Instant::now());
            }
            if finished.is_some_and(|at| at.elapsed() > lag * 2) && self.pending.is_empty() {
                return Ok(text);
            }
        }
        Err("receiver timed out".into())
    }

    fn translate(&self, event: &KeyPressEvent) -> TestResult<Option<char>> {
        let mapping = self.connection.get_keyboard_mapping(event.detail, 1)?.reply()?;
        let state = u16::from(event.state);
        let level = |index: usize| mapping.keysyms.get(index).copied().unwrap_or_default();
        let character = |index: usize| xkeysym::Keysym::new(level(index)).key_char();
        // Caps Lock inverts Shift on a key with a lowercase and an uppercase letter.
        let letters = matches!((character(0), character(1)),
            (Some(lower), Some(upper)) if lower != upper && lower.to_uppercase().eq(std::iter::once(upper)));
        let shifted =
            (state & u16::from(KeyButMask::SHIFT) != 0) != (letters && state & u16::from(KeyButMask::LOCK) != 0);
        let symbol = if shifted && level(1) != 0 { level(1) } else { level(0) };
        let symbol = xkeysym::Keysym::new(symbol);
        if symbol.is_modifier_key() {
            return Ok(None);
        }
        Ok(Some(symbol.key_char().unwrap_or('\u{fffd}')))
    }
}

/// Distinct characters that can get a temporary mapping now: unused keycodes,
/// less the lowest one, plus keycodes still holding a mapping recorded by
/// `horizon-device`. Keycodes of the modifier mapping never count.
// Each test crate includes this module; not every crate uses every helper.
#[allow(dead_code)]
pub fn mapping_capacity(connection: &RustConnection) -> TestResult<usize> {
    use x11rb::protocol::xproto::AtomEnum;
    let setup = connection.setup();
    let root = setup.roots[0].root;
    let mapping = connection
        .get_keyboard_mapping(setup.min_keycode, setup.max_keycode - setup.min_keycode + 1)?
        .reply()?;
    let width = usize::from(mapping.keysyms_per_keycode);
    let symbols = |keycode: u8| {
        let start = usize::from(keycode - setup.min_keycode) * width;
        &mapping.keysyms[start..start + width]
    };
    let modifiers = connection.get_modifier_mapping()?.reply()?.keycodes;
    let is_modifier = |keycode: u8| modifiers.contains(&keycode);
    // `horizon-device` keeps the lowest unused keycode free for other actions,
    // also when it is a modifier keycode, and then skips each modifier keycode.
    let unused = (setup.min_keycode..=setup.max_keycode)
        .filter(|keycode| *keycode != 8 && symbols(*keycode).iter().all(|symbol| *symbol == 0))
        .skip(1)
        .filter(|keycode| !is_modifier(*keycode))
        .count();
    let atom = connection.intern_atom(false, b"_HORIZON_DEVICE_KEYMAP")?.reply()?.atom;
    let record = connection
        .get_property(false, root, atom, AtomEnum::CARDINAL, 0, 1024)?
        .reply()?;
    let words: Vec<u32> = record.value32().map(Iterator::collect).unwrap_or_default();
    let borrowed = words
        .as_chunks::<4>()
        .0
        .iter()
        .filter(|entry| {
            u8::try_from(entry[0]).is_ok_and(|keycode| {
                (setup.min_keycode..=setup.max_keycode).contains(&keycode)
                    && !is_modifier(keycode)
                    && symbols(keycode).get(..2) == Some(&[entry[1], entry[1]][..])
                    && symbols(keycode)
                        .iter()
                        .all(|symbol| *symbol == 0 || *symbol == entry[1])
            })
        })
        .count();
    Ok(unused + borrowed)
}
