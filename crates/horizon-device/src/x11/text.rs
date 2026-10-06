//! Text entry through XTEST with stable keycode mappings.

use super::keymap::{self, Borrowed, Layout, Plan, PlanError};
use super::{X11, unavailable};
use crate::{DeviceError, Result};
use std::time::{Duration, SystemTime, UNIX_EPOCH};
use x11rb::{
    connection::Connection,
    protocol::{
        xproto::{AtomEnum, ConnectionExt as _, GetKeyboardMappingReply, KEY_PRESS_EVENT, KEY_RELEASE_EVENT, PropMode},
        xtest::ConnectionExt as _,
    },
    wrapper::ConnectionExt as _,
};

/// Root window property that records the keycodes this tool mapped.
const RECORD_PROPERTY: &[u8] = b"_HORIZON_DEVICE_KEYMAP";
const KEY_INTERVAL: Duration = Duration::from_millis(20);
const FINAL_DRAIN: Duration = Duration::from_millis(100);

fn now_ms() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |elapsed| u64::try_from(elapsed.as_millis()).unwrap_or(u64::MAX))
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
        let mapping = self.keyboard_mapping()?;
        let layout = self.layout(&mapping);
        let modifiers = self
            .connection
            .get_modifier_mapping()
            .map_err(unavailable)?
            .reply()
            .map_err(unavailable)?;
        // The first modifier row is Shift.
        let shift_keycode = modifiers
            .keycodes
            .iter()
            .take(usize::from(modifiers.keycodes_per_modifier()))
            .copied()
            .find(|keycode| *keycode != 0);
        let record_atom = self.record_atom()?;
        let previous = self.read_record(record_atom)?;
        let plan = keymap::plan(&layout, &previous, shift_keycode, text).map_err(|e| match e {
            PlanError::Capacity => DeviceError::Invalid("text exceeds available X11 Unicode key mappings".into()),
            PlanError::NoKeysym => DeviceError::Invalid("text contains a character without an X11 keysym".into()),
        })?;
        self.apply_bindings(&plan, record_atom)?;
        let typed = self.send_strokes(&plan, shift_keycode);
        let recorded = self.write_record(record_atom, &plan.record(now_ms()));
        std::thread::sleep(FINAL_DRAIN);
        typed.and(recorded.map_err(indeterminate))
    }

    /// Clears the least recently used temporary keycode when no keycode is
    /// unused. The input backend of the other actions refuses to start without
    /// one, for example after text from an older version used every keycode.
    pub(super) fn ensure_unused_keycode(&self) -> Result<()> {
        let mapping = self.keyboard_mapping()?;
        let record_atom = self.record_atom()?;
        let mut record = self.read_record(record_atom)?;
        let Some(candidate) = keymap::spare_candidate(&self.layout(&mapping), &record) else {
            return Ok(());
        };
        let wait = Duration::from_millis(keymap::quiet_after(candidate.last_used_ms).saturating_sub(now_ms()));
        std::thread::sleep(wait.min(keymap::RECLAIM_QUIET));
        record.retain(|entry| entry.keycode != candidate.keycode);
        self.connection
            .change_keyboard_mapping(1, candidate.keycode, 2, &[0, 0])
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
        Ok(Borrowed::decode(&words))
    }

    fn apply_bindings(&self, plan: &Plan, record_atom: u32) -> Result<()> {
        if plan.bindings.is_empty() {
            return Ok(());
        }
        let wait = Duration::from_millis(plan.not_before_ms.saturating_sub(now_ms()));
        std::thread::sleep(wait.min(keymap::RECLAIM_QUIET));
        // Record ownership before the change so a failure cannot leak keycodes.
        self.write_record(record_atom, &plan.record(now_ms()))
            .map_err(unavailable)?;
        for (first, keysyms) in keymap::runs(&plan.bindings) {
            let count = u8::try_from(keysyms.len() / 2).map_err(unavailable)?;
            self.connection
                .change_keyboard_mapping(count, first, 2, &keysyms)
                .map_err(unavailable)?;
        }
        // A round trip proves that the server applied every mapping before the
        // first key event; clients receive the notifications ahead of the keys.
        self.connection.sync().map_err(unavailable)
    }

    fn send_strokes(&self, plan: &Plan, shift_keycode: Option<u8>) -> Result<()> {
        let fake = |kind: u8, keycode: u8| {
            self.connection
                .xtest_fake_input(kind, keycode, x11rb::CURRENT_TIME, self.root, 0, 0, 0)
                .map(drop)
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
            self.connection.sync().map_err(indeterminate)?;
            std::thread::sleep(KEY_INTERVAL);
        }
        Ok(())
    }

    fn write_record(
        &self,
        record_atom: u32,
        record: &[Borrowed],
    ) -> std::result::Result<(), x11rb::errors::ReplyError> {
        let words = Borrowed::encode(record);
        if words.is_empty() {
            self.connection.delete_property(self.root, record_atom)?;
        } else {
            self.connection
                .change_property32(PropMode::REPLACE, self.root, record_atom, AtomEnum::CARDINAL, &words)?;
        }
        self.connection.sync()
    }
}
