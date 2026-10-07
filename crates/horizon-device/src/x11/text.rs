//! Text entry through XTEST with stable keycode mappings.

use super::keymap::{self, Borrowed, Keyboard, Layout, ModifierError, Plan, PlanError};
use super::stroke;
use super::{X11, unavailable};
use crate::{DeviceError, Result};
use std::time::{Duration, Instant};
use x11rb::{
    connection::{Connection, RequestConnection as _},
    errors::{ConnectionError, ReplyError},
    protocol::{
        Event,
        xkb::{self, ConnectionExt as _},
        xproto::{
            AtomEnum, ChangeWindowAttributesAux, ConnectionExt as _, EventMask, GetKeyboardMappingReply,
            GetModifierMappingReply, KEY_PRESS_EVENT, KEY_RELEASE_EVENT, PropMode,
        },
        xtest::{self, ConnectionExt as _},
    },
    wrapper::ConnectionExt as _,
};

/// Root window property that records the keycodes this tool mapped.
const RECORD_PROPERTY: &[u8] = b"_HORIZON_DEVICE_KEYMAP";
const FINAL_DRAIN: Duration = Duration::from_millis(100);
/// The longest wait for one keycode: the longest lease, then the quiet interval.
const MAX_WAIT: Duration = keymap::MAX_LEASE.saturating_add(keymap::RECLAIM_QUIET);

/// Sleeps until the timeline reaches `not_before_ms`, at most the quiet interval
/// at a time. Returns false when no wait is necessary, so that the caller uses
/// its current plan. A caller that must wait a third time gets `unavailable`,
/// because another client keeps the keymap busy.
fn wait_until(not_before_ms: u64, attempts: &mut u8) -> Result<bool> {
    let remaining = not_before_ms.saturating_sub(keymap::TIMELINE_NOW);
    if remaining == 0 {
        return Ok(false);
    }
    *attempts += 1;
    if *attempts > 2 {
        return Err(DeviceError::Unavailable(
            "X11 keymap changed during the quiet interval; observe and retry".into(),
        ));
    }
    std::thread::sleep(Duration::from_millis(remaining).min(MAX_WAIT));
    Ok(true)
}

/// The Shift key and Caps Lock state for `layout`. Fails with
/// `unsupported` for a keyboard state that would change the typed keys.
fn keyboard(layout: &Layout<'_>, modifiers: &GetModifierMappingReply, state: u16, group: u8) -> Result<Keyboard> {
    // Other groups have other levels.
    if group != 0 {
        return Err(DeviceError::Unsupported(
            "X11 text input requires the first keyboard group".into(),
        ));
    }
    let per_row = usize::from(modifiers.keycodes_per_modifier()).max(1);
    let rows: Vec<&[u8]> = modifiers.keycodes.chunks(per_row).collect();
    // A held Shift, Control, Alt or Super key would change each typed key.
    keymap::keyboard(layout, &rows, state).map_err(|e| {
        DeviceError::Unsupported(match e {
            ModifierError::Held => "X11 text input requires released modifier keys".into(),
            ModifierError::Lock => "X11 text input supports Caps Lock but no other Lock modifier".into(),
        })
    })
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
        let record_atom = self.record_atom()?;
        // A reassignment waits for the quiet interval. Read the keyboard and
        // plan again after the wait, because another client can change the
        // keymap, the modifier mapping or the keyboard state in the meantime.
        let mut attempts = 0;
        let (plan, server_now, observed_at, keyboard) = loop {
            let mapping = self.keyboard_mapping()?;
            let modifiers = self.modifier_mapping()?;
            let (state, group) = self.keyboard_state()?;
            let typed = self.typed_symbols(state)?;
            let layout = Layout {
                typed: typed.as_deref(),
                ..self.layout(&mapping, &modifiers)
            };
            let keyboard = keyboard(&layout, &modifiers, state, group)?;
            let (previous, server_now) = self.read_record(record_atom)?;
            let observed_at = Instant::now();
            let plan = keymap::plan(&layout, &previous, keyboard, text).map_err(|e| match e {
                PlanError::Capacity => DeviceError::Invalid("text exceeds available X11 Unicode key mappings".into()),
                PlanError::NoKeysym => DeviceError::Invalid("text contains a character without an X11 keysym".into()),
                PlanError::CapsLock => {
                    DeviceError::Unsupported("X11 text input with Caps Lock on supports no letter with case".into())
                }
            })?;
            if !wait_until(plan.not_before_ms, &mut attempts)? {
                break (plan, server_now, observed_at, keyboard);
            }
        };
        self.apply_bindings(&plan, record_atom, server_now)?;
        let typed = self.send_strokes(&plan, keyboard.shift_keycode, record_atom, server_now, observed_at);
        let recorded = self.server_time(record_atom).map_err(indeterminate).and_then(|end| {
            let used = keymap::TIMELINE_NOW + u64::from(end.wrapping_sub(server_now));
            self.write_record(record_atom, &plan.record(used), server_now)
                .map_err(indeterminate)
        });
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
        let (candidate, mut record, server_now) = loop {
            let mapping = self.keyboard_mapping()?;
            let modifiers = self.modifier_mapping()?;
            let (record, server_now) = self.read_record(record_atom)?;
            let Some(candidate) = keymap::spare_candidate(&self.layout(&mapping, &modifiers), &record) else {
                return Ok(());
            };
            if !wait_until(keymap::quiet_after(candidate.last_used_ms), &mut attempts)? {
                break (candidate, record, server_now);
            }
        };
        record.retain(|entry| entry.keycode != candidate.keycode);
        self.connection
            .change_keyboard_mapping(1, candidate.keycode, 2, &[0, 0])
            .map_err(unavailable)?
            .check()
            .map_err(unavailable)?;
        self.write_record(record_atom, &record, server_now).map_err(unavailable)
    }

    /// The effective modifier mask and keyboard group. XKB reports the group
    /// only to a client that uses the extension. Without XKB, the core state
    /// holds the group in bits 13 and 14.
    fn keyboard_state(&self) -> Result<(u16, u8)> {
        if self.uses_xkb()? {
            let state = self
                .connection
                .xkb_get_state(xkb::ID::USE_CORE_KBD.into())
                .map_err(unavailable)?
                .reply()
                .map_err(unavailable)?;
            return Ok((u16::from(state.mods), u8::from(state.group)));
        }
        let mask = u16::from(
            self.connection
                .query_pointer(self.root)
                .map_err(unavailable)?
                .reply()
                .map_err(unavailable)?
                .mask,
        );
        Ok((mask & 0xff, u8::try_from((mask >> 13) & 3).unwrap_or_default()))
    }

    fn keyboard_mapping(&self) -> Result<GetKeyboardMappingReply> {
        let setup = self.connection.setup();
        self.connection
            .get_keyboard_mapping(setup.min_keycode, setup.max_keycode - setup.min_keycode + 1)
            .map_err(unavailable)?
            .reply()
            .map_err(unavailable)
    }

    fn modifier_mapping(&self) -> Result<GetModifierMappingReply> {
        self.connection
            .get_modifier_mapping()
            .map_err(unavailable)?
            .reply()
            .map_err(unavailable)
    }

    fn layout<'a>(&self, mapping: &'a GetKeyboardMappingReply, modifiers: &'a GetModifierMappingReply) -> Layout<'a> {
        Layout {
            min_keycode: self.connection.setup().min_keycode,
            keysyms_per_keycode: mapping.keysyms_per_keycode,
            keysyms: &mapping.keysyms,
            modifier_keycodes: &modifiers.keycodes,
            typed: None,
        }
    }

    /// For each keycode, the keysyms of the first group that the key gives
    /// without Shift and with Shift in the modifier state `mods`, from the XKB
    /// key types. `None` without XKB.
    fn typed_symbols(&self, mods: u16) -> Result<Option<Vec<[u32; 2]>>> {
        if !self.uses_xkb()? {
            return Ok(None);
        }
        let reply = self
            .connection
            .xkb_get_map(
                xkb::ID::USE_CORE_KBD.into(),
                xkb::MapPart::KEY_TYPES | xkb::MapPart::KEY_SYMS,
                xkb::MapPart::from(0_u16),
                0,
                0,
                0,
                0,
                0,
                0,
                0,
                0,
                xkb::VMod::from(0_u16),
                0,
                0,
                0,
                0,
                0,
                0,
            )
            .map_err(unavailable)?
            .reply()
            .map_err(unavailable)?;
        let types: Vec<keymap::KeyType> = reply
            .map
            .types_rtrn
            .unwrap_or_default()
            .iter()
            .map(|key_type| keymap::KeyType {
                mods_mask: u16::from(key_type.mods_mask),
                map: key_type
                    .map
                    .iter()
                    .filter(|entry| entry.active)
                    .map(|entry| (u16::from(entry.mods_mask), entry.level))
                    .collect(),
            })
            .collect();
        let mut per_key = vec![[0; 2]; 256];
        let keys = (reply.first_key_sym..=u8::MAX).zip(reply.map.syms_rtrn.unwrap_or_default());
        for (keycode, key) in keys {
            // The first group: `width` keysyms at the start, if the key has a group.
            let Some(key_type) = types.get(usize::from(key.kt_index[0])) else {
                continue;
            };
            let groups = key.group_info & 0x0f;
            if groups == 0 {
                continue;
            }
            let width = usize::from(key.width).min(key.syms.len());
            let syms: Vec<u32> = key.syms[..width].to_vec();
            per_key[usize::from(keycode)] = keymap::typed_symbols(key_type, &syms, mods);
        }
        Ok(Some(per_key))
    }

    fn uses_xkb(&self) -> Result<bool> {
        Ok(self
            .connection
            .extension_information(xkb::X11_EXTENSION_NAME)
            .map_err(unavailable)?
            .is_some()
            && self
                .connection
                .xkb_use_extension(1, 0)
                .map_err(unavailable)?
                .reply()
                .map_err(unavailable)?
                .supported)
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

    /// The current X server time in milliseconds. The server clock is
    /// monotonic, excludes suspend, and is the same for every client of the
    /// display. An empty append to the record property makes the server send
    /// a property notification with its time.
    fn server_time(&self, record_atom: u32) -> std::result::Result<u32, ReplyError> {
        let select = |mask: EventMask| {
            self.connection
                .change_window_attributes(self.root, &ChangeWindowAttributesAux::new().event_mask(mask))?
                .check()
        };
        select(EventMask::PROPERTY_CHANGE)?;
        let appended = self
            .connection
            .change_property32(PropMode::APPEND, self.root, record_atom, AtomEnum::CARDINAL, &[])
            .map_err(ReplyError::from)
            .and_then(x11rb::cookie::VoidCookie::check);
        let deselected = select(EventMask::NO_EVENT);
        appended.and(deselected)?;
        // The checked requests were round trips, so the notification is queued.
        let mut time = None;
        while let Some(event) = self.connection.poll_for_event()? {
            if let Event::PropertyNotify(event) = event
                && event.window == self.root
                && event.atom == record_atom
            {
                time = Some(event.time);
            }
        }
        time.ok_or(ReplyError::ConnectionError(ConnectionError::UnknownError))
    }

    /// The records on the planning timeline, and the server time of the read.
    fn read_record(&self, record_atom: u32) -> Result<(Vec<Borrowed>, u32)> {
        let server_now = self.server_time(record_atom).map_err(unavailable)?;
        let reply = self
            .connection
            .get_property(false, self.root, record_atom, AtomEnum::CARDINAL, 0, 1024)
            .map_err(unavailable)?
            .reply()
            .map_err(unavailable)?;
        let words: Vec<u32> = reply.value32().map(Iterator::collect).unwrap_or_default();
        let records = Borrowed::decode(&words)
            .into_iter()
            .map(|record| Borrowed {
                last_used_ms: keymap::to_timeline(record.last_used_ms, server_now),
                ..record
            })
            .collect();
        Ok((records, server_now))
    }

    fn apply_bindings(&self, plan: &Plan, record_atom: u32, server_now: u32) -> Result<()> {
        // Before any input, record the keycodes with a lease that covers the
        // last possible stroke. If this process stops early, a later action
        // still waits for the strokes that it sent.
        let record = plan.record(stroke::Lease::start(plan.strokes.len()).until);
        if !record.is_empty() {
            self.write_record(record_atom, &record, server_now)
                .map_err(unavailable)?;
        }
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

    /// Sends the strokes of `plan`. If the strokes are slower than the lease
    /// expects, for example on a stalled server, the record gets a new lease
    /// from the observed server time before the next stroke.
    fn send_strokes(
        &self,
        plan: &Plan,
        shift_keycode: Option<u8>,
        record_atom: u32,
        server_now: u32,
        mut observed_at: Instant,
    ) -> Result<()> {
        let fake = |kind: u8, keycode: u8| -> std::result::Result<(), ReplyError> {
            self.connection
                .xtest_fake_input(kind, keycode, x11rb::CURRENT_TIME, self.root, 0, 0, 0)?
                .check()
        };
        let mut lease = stroke::Lease::start(plan.strokes.len());
        for (index, next) in plan.strokes.iter().enumerate() {
            let elapsed = u64::try_from(observed_at.elapsed().as_millis()).unwrap_or(u64::MAX);
            if lease.needs_extension(elapsed) {
                let now = self.server_time(record_atom).map_err(indeterminate)?;
                observed_at = Instant::now();
                let observed = keymap::TIMELINE_NOW + u64::from(now.wrapping_sub(server_now));
                lease = stroke::Lease::extended(observed, plan.strokes.len() - index);
                self.write_record(record_atom, &plan.record(lease.until), server_now)
                    .map_err(indeterminate)?;
            }
            let shift = shift_keycode.filter(|_| next.shift);
            stroke::send(next.keycode, shift, KEY_PRESS_EVENT, KEY_RELEASE_EVENT, fake).map_err(indeterminate)?;
            std::thread::sleep(keymap::KEY_INTERVAL);
        }
        Ok(())
    }

    /// Writes timeline records as X server times relative to `server_now`.
    fn write_record(
        &self,
        record_atom: u32,
        record: &[Borrowed],
        server_now: u32,
    ) -> std::result::Result<(), ReplyError> {
        let stored: Vec<Borrowed> = record
            .iter()
            .map(|entry| Borrowed {
                last_used_ms: keymap::from_timeline(entry.last_used_ms, server_now),
                ..*entry
            })
            .collect();
        let words = Borrowed::encode(&stored);
        if words.is_empty() {
            self.connection.delete_property(self.root, record_atom)?.check()
        } else {
            self.connection
                .change_property32(PropMode::REPLACE, self.root, record_atom, AtomEnum::CARDINAL, &words)?
                .check()
        }
    }
}
