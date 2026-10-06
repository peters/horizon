//! Text entry through XTEST with stable keycode mappings.

use super::keymap::{self, Borrowed, Keyboard, Layout, Plan, PlanError};
use super::{X11, unavailable};
use crate::{DeviceError, Result};
use std::time::Duration;
use x11rb::{
    connection::{Connection, RequestConnection as _},
    errors::ReplyError,
    protocol::{
        xproto::{
            AtomEnum, ConnectionExt as _, GetKeyboardMappingReply, KEY_PRESS_EVENT, KEY_RELEASE_EVENT, KeyButMask,
            PropMode,
        },
        xtest::{self, ConnectionExt as _},
    },
    wrapper::ConnectionExt as _,
};

/// Root window property that records the keycodes this tool mapped.
const RECORD_PROPERTY: &[u8] = b"_HORIZON_DEVICE_KEYMAP";
const KEY_INTERVAL: Duration = Duration::from_millis(20);
const FINAL_DRAIN: Duration = Duration::from_millis(100);

/// Milliseconds since boot, from `/proc/uptime` (`CLOCK_BOOTTIME`). Unlike the
/// wall clock, it does not step, and all local processes share it.
fn now_ms() -> std::io::Result<u64> {
    keymap::uptime_ms(&std::fs::read_to_string("/proc/uptime")?)
        .ok_or_else(|| std::io::Error::other("unexpected /proc/uptime format"))
}

/// Sleeps until the boot clock reaches `not_before_ms`, at most the quiet
/// interval at a time. Returns false when no wait is necessary, so that the
/// caller uses its current plan. A caller that must wait a third time gets
/// `unavailable`, because another client keeps the keymap busy.
fn wait_until(not_before_ms: u64, attempts: &mut u8) -> Result<bool> {
    let remaining = not_before_ms.saturating_sub(now_ms().map_err(unavailable)?);
    if remaining == 0 {
        return Ok(false);
    }
    *attempts += 1;
    if *attempts > 2 {
        return Err(DeviceError::Unavailable(
            "X11 keymap changed during the quiet interval; observe and retry".into(),
        ));
    }
    std::thread::sleep(Duration::from_millis(remaining).min(keymap::RECLAIM_QUIET));
    Ok(true)
}

fn indeterminate(e: impl std::fmt::Display) -> DeviceError {
    DeviceError::Indeterminate(e.to_string())
}

impl X11 {
    /// Types `text` with one key click for each character.
    ///
    /// Invalid text fails before any keymap change or input. Each keycode
    /// keeps its keysym while a queued event can still refer to it, so a
    /// client that reads events late still translates every key correctly.
    pub(super) fn type_text(&self, text: &str) -> Result<()> {
        if self
            .connection
            .extension_information(xtest::X11_EXTENSION_NAME)
            .map_err(unavailable)?
            .is_none()
        {
            return Err(DeviceError::Unsupported(
                "X11 text input requires the XTEST extension".into(),
            ));
        }
        let state = self
            .connection
            .query_pointer(self.root)
            .map_err(unavailable)?
            .reply()
            .map_err(unavailable)?
            .mask;
        // Core state bits 13 and 14 hold the XKB group. Other groups have other levels.
        if u16::from(state) & 0x6000 != 0 {
            return Err(DeviceError::Unsupported(
                "X11 text input requires the first keyboard group".into(),
            ));
        }
        // A held Shift, Control, Alt or Super key would change each typed key.
        // Lock is planned for, and Mod2 is usually Num Lock.
        let held = [
            KeyButMask::SHIFT,
            KeyButMask::CONTROL,
            KeyButMask::MOD1,
            KeyButMask::MOD3,
            KeyButMask::MOD4,
            KeyButMask::MOD5,
        ];
        if held
            .into_iter()
            .any(|modifier| u16::from(state) & u16::from(modifier) != 0)
        {
            return Err(DeviceError::Unsupported(
                "X11 text input requires released modifier keys".into(),
            ));
        }
        let modifiers = self
            .connection
            .get_modifier_mapping()
            .map_err(unavailable)?
            .reply()
            .map_err(unavailable)?;
        // The first modifier row is Shift.
        let keyboard = Keyboard {
            shift_keycode: modifiers
                .keycodes
                .iter()
                .take(usize::from(modifiers.keycodes_per_modifier()))
                .copied()
                .find(|keycode| *keycode != 0),
            caps_lock: u16::from(state) & u16::from(KeyButMask::LOCK) != 0,
        };
        let record_atom = self.record_atom()?;
        // A reassignment waits for the quiet interval. Plan again after the wait,
        // because another client can change the keymap in the meantime.
        let mut attempts = 0;
        let plan = loop {
            let mapping = self.keyboard_mapping()?;
            let previous = self.read_record(record_atom)?;
            let plan = keymap::plan(&self.layout(&mapping), &previous, keyboard, text).map_err(|e| match e {
                PlanError::Capacity => DeviceError::Invalid("text exceeds available X11 Unicode key mappings".into()),
                PlanError::NoKeysym => DeviceError::Invalid("text contains a character without an X11 keysym".into()),
            })?;
            if !wait_until(plan.not_before_ms, &mut attempts)? {
                break plan;
            }
        };
        self.apply_bindings(&plan, record_atom)?;
        let typed = self.send_strokes(&plan, keyboard.shift_keycode);
        let recorded = now_ms()
            .map_err(indeterminate)
            .and_then(|now| self.write_record(record_atom, &plan.record(now)).map_err(indeterminate));
        std::thread::sleep(FINAL_DRAIN);
        typed.and(recorded)
    }

    /// Clears the least recently used temporary keycode when no keycode is
    /// unused. The input backend of the other actions refuses to start without
    /// one, for example after another client used the keycode that `type` keeps.
    pub(super) fn ensure_unused_keycode(&self) -> Result<()> {
        let record_atom = self.record_atom()?;
        // Choose again after the wait, because another client can change the keymap.
        let mut attempts = 0;
        let (candidate, mut record) = loop {
            let mapping = self.keyboard_mapping()?;
            let record = self.read_record(record_atom)?;
            let Some(candidate) = keymap::spare_candidate(&self.layout(&mapping), &record) else {
                return Ok(());
            };
            if !wait_until(keymap::quiet_after(candidate.last_used_ms), &mut attempts)? {
                break (candidate, record);
            }
        };
        record.retain(|entry| entry.keycode != candidate.keycode);
        self.connection
            .change_keyboard_mapping(1, candidate.keycode, 2, &[0, 0])
            .map_err(unavailable)?
            .check()
            .map_err(unavailable)?;
        self.write_record(record_atom, &record).map_err(unavailable)
    }

    fn keyboard_mapping(&self) -> Result<GetKeyboardMappingReply> {
        let setup = self.connection.setup();
        self.connection
            .get_keyboard_mapping(setup.min_keycode, setup.max_keycode - setup.min_keycode + 1)
            .map_err(unavailable)?
            .reply()
            .map_err(unavailable)
    }

    fn layout<'a>(&self, mapping: &'a GetKeyboardMappingReply) -> Layout<'a> {
        Layout {
            min_keycode: self.connection.setup().min_keycode,
            keysyms_per_keycode: mapping.keysyms_per_keycode,
            keysyms: &mapping.keysyms,
        }
    }

    fn record_atom(&self) -> Result<u32> {
        Ok(self
            .connection
            .intern_atom(false, RECORD_PROPERTY)
            .map_err(unavailable)?
            .reply()
            .map_err(unavailable)?
            .atom)
    }

    fn read_record(&self, record_atom: u32) -> Result<Vec<Borrowed>> {
        let reply = self
            .connection
            .get_property(false, self.root, record_atom, AtomEnum::CARDINAL, 0, 1024)
            .map_err(unavailable)?
            .reply()
            .map_err(unavailable)?;
        let words: Vec<u32> = reply.value32().map(Iterator::collect).unwrap_or_default();
        // A time after now comes from another clock, for example before a
        // reboot of a persistent server. Such a record has no recent use.
        let now = now_ms().map_err(unavailable)?;
        Ok(Borrowed::decode(&words)
            .into_iter()
            .map(|record| Borrowed {
                last_used_ms: if record.last_used_ms > now {
                    0
                } else {
                    record.last_used_ms
                },
                ..record
            })
            .collect())
    }

    fn apply_bindings(&self, plan: &Plan, record_atom: u32) -> Result<()> {
        if plan.bindings.is_empty() {
            return Ok(());
        }
        // Record ownership before the change so a failure cannot leak keycodes.
        self.write_record(record_atom, &plan.record(now_ms().map_err(unavailable)?))
            .map_err(unavailable)?;
        for (first, keysyms) in keymap::runs(&plan.bindings) {
            let count = u8::try_from(keysyms.len() / 2).map_err(unavailable)?;
            // A checked request is a round trip: the server applied the mapping
            // before the first key event, and clients get the notification first.
            self.connection
                .change_keyboard_mapping(count, first, 2, &keysyms)
                .map_err(unavailable)?
                .check()
                .map_err(unavailable)?;
        }
        Ok(())
    }

    fn send_strokes(&self, plan: &Plan, shift_keycode: Option<u8>) -> Result<()> {
        let fake = |kind: u8, keycode: u8| -> std::result::Result<(), ReplyError> {
            self.connection
                .xtest_fake_input(kind, keycode, x11rb::CURRENT_TIME, self.root, 0, 0, 0)?
                .check()
        };
        for stroke in &plan.strokes {
            let shift = shift_keycode.filter(|_| stroke.shift);
            let result = (|| {
                if let Some(shift) = shift {
                    fake(KEY_PRESS_EVENT, shift)?;
                }
                fake(KEY_PRESS_EVENT, stroke.keycode)?;
                fake(KEY_RELEASE_EVENT, stroke.keycode)
            })();
            let release = shift.map_or(Ok(()), |shift| fake(KEY_RELEASE_EVENT, shift));
            result.and(release).map_err(indeterminate)?;
            std::thread::sleep(KEY_INTERVAL);
        }
        Ok(())
    }

    fn write_record(&self, record_atom: u32, record: &[Borrowed]) -> std::result::Result<(), ReplyError> {
        let words = Borrowed::encode(record);
        if words.is_empty() {
            self.connection.delete_property(self.root, record_atom)?.check()
        } else {
            self.connection
                .change_property32(PropMode::REPLACE, self.root, record_atom, AtomEnum::CARDINAL, &words)?
                .check()
        }
    }
}
