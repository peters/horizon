//! Keystroke planning for X11 text entry.
//!
//! X11 clients translate a keycode with the keymap they hold when they process
//! the event, and many clients (for example xkbcommon users such as winit)
//! fetch the server keymap only after they read the mapping notification.
//! A keycode that is remapped or cleared before a slow client reads its last
//! key event is therefore translated with the wrong mapping, or not at all.
//!
//! The planner keeps every keycode a pending event can use stable:
//! characters on the first two shift levels of the current keymap need no
//! mapping; temporary mappings stay on their keycodes after an action and later
//! actions reuse them; a temporary mapping is reassigned only when no unused
//! keycode remains, least recently used first, after a quiet interval. The
//! lowest unused keycode is never taken, because the input backend of the other
//! actions requires one.

use std::collections::{BTreeMap, BTreeSet};
use std::time::Duration;

/// Minimum idle time before a temporary mapping may get a different keysym.
pub(super) const RECLAIM_QUIET: Duration = Duration::from_secs(2);

/// Keycode 8 becomes evdev code 0, which clients treat as no key.
const RESERVED_KEYCODE: u8 = 8;

/// The core keyboard mapping as returned by `GetKeyboardMapping`, with the
/// keycodes of the modifier mapping from `GetModifierMapping`.
pub(super) struct Layout<'a> {
    pub min_keycode: u8,
    pub keysyms_per_keycode: u8,
    pub keysyms: &'a [u32],
    /// A key in a modifier row changes the modifier state when it is pressed,
    /// whatever its keysyms are. The planner never types with such a key and
    /// never maps it.
    pub modifier_keycodes: &'a [u8],
    /// For each keycode, the keysyms that the key gives without Shift and with
    /// Shift in the current modifier state, from its XKB key type. Without
    /// XKB, the core protocol rules apply to the first two keysyms.
    pub typed: Option<&'a [[u32; 2]]>,
}

/// An XKB key type: the modifiers that it reads, and the level for each
/// combination of them in its active map entries. Other combinations give
/// the first level.
#[derive(Clone, Debug, PartialEq, Eq)]
pub(super) struct KeyType {
    pub mods_mask: u16,
    pub map: Vec<(u16, u8)>,
}

impl KeyType {
    fn level(&self, mods: u16) -> u8 {
        let mods = mods & self.mods_mask;
        self.map
            .iter()
            .find(|(entry, _)| *entry == mods)
            .map_or(0, |(_, level)| *level)
    }
}

const SHIFT_MASK: u16 = 1;

/// The keysyms that a key with `key_type` and the first-group keysyms `syms`
/// gives without Shift and with Shift, in the modifier state `mods`.
pub(super) fn typed_symbols(key_type: &KeyType, syms: &[u32], mods: u16) -> [u32; 2] {
    let at = |level: u8| syms.get(usize::from(level)).copied().unwrap_or(0);
    [
        at(key_type.level(mods & !SHIFT_MASK)),
        at(key_type.level(mods | SHIFT_MASK)),
    ]
}

/// Keypad keysyms. Without XKB, Num Lock changes the level of such a key.
fn is_keypad(symbol: u32) -> bool {
    (0xff80..=0xffbd).contains(&symbol)
}

impl Layout<'_> {
    fn keycodes(&self) -> impl Iterator<Item = (u8, &[u32])> {
        let width = usize::from(self.keysyms_per_keycode.max(1));
        (self.min_keycode..=u8::MAX).zip(self.keysyms.chunks(width))
    }

    fn is_modifier(&self, keycode: u8) -> bool {
        self.modifier_keycodes.contains(&keycode)
    }

    /// The keysym that `keycode` gives without Shift or with Shift, or none
    /// when the level cannot be established.
    fn typed_symbol(&self, keycode: u8, shift: bool) -> Option<u32> {
        let symbol = if let Some(typed) = self.typed {
            typed.get(usize::from(keycode))?[usize::from(shift)]
        } else {
            let symbols = self.symbols(keycode);
            if symbols.iter().take(2).any(|symbol| is_keypad(*symbol)) {
                return None;
            }
            *symbols.get(usize::from(shift))?
        };
        (symbol != 0).then_some(symbol)
    }

    fn symbols(&self, keycode: u8) -> &[u32] {
        self.keycodes()
            .find(|(code, _)| *code == keycode)
            .map_or(&[], |(_, symbols)| symbols)
    }
}

/// A keycode that this tool mapped to one keysym on both shift levels.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Borrowed {
    pub keycode: u8,
    pub keysym: u32,
    pub last_used_ms: u64,
}

impl Borrowed {
    const WORDS: usize = 4;

    /// Encodes records as `[keycode, keysym, used_ms_high, used_ms_low]` words.
    pub(super) fn encode(records: &[Self]) -> Vec<u32> {
        records
            .iter()
            .flat_map(|record| {
                let [high, low] = split_ms(record.last_used_ms);
                [u32::from(record.keycode), record.keysym, high, low]
            })
            .collect()
    }

    /// Decodes records; malformed words are ignored because the property is advisory.
    pub(super) fn decode(words: &[u32]) -> Vec<Self> {
        words
            .as_chunks::<{ Self::WORDS }>()
            .0
            .iter()
            .filter_map(|word| {
                Some(Self {
                    keycode: u8::try_from(word[0]).ok()?,
                    keysym: word[1],
                    last_used_ms: (u64::from(word[2]) << 32) | u64::from(word[3]),
                })
            })
            .collect()
    }
}

fn split_ms(ms: u64) -> [u32; 2] {
    let high = u32::try_from(ms >> 32).unwrap_or(u32::MAX);
    let low = u32::try_from(ms & u64::from(u32::MAX)).unwrap_or_default();
    [high, low]
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub(super) struct Stroke {
    pub keycode: u8,
    pub shift: bool,
}

#[derive(Debug, PartialEq, Eq)]
pub(super) enum PlanError {
    NoKeysym,
    Capacity,
    /// Caps Lock is on, and the text has a letter with case. The case that a
    /// client gives depends on the XKB key type, which the planner does not read.
    CapsLock,
}

#[derive(Debug)]
pub(super) struct Plan {
    /// One stroke for each character of the text, in order.
    pub strokes: Vec<Stroke>,
    /// Keycodes to map before the first stroke, in ascending keycode order.
    pub bindings: Vec<(u8, u32)>,
    /// Earliest timeline time in milliseconds at which `bindings` may be applied.
    pub not_before_ms: u64,
    borrowed: Vec<Borrowed>,
    used: BTreeSet<u8>,
}

impl Plan {
    /// The borrowed-keycode record after the strokes were sent at `now_ms`.
    pub(super) fn record(&self, now_ms: u64) -> Vec<Borrowed> {
        self.borrowed
            .iter()
            .map(|record| Borrowed {
                last_used_ms: if self.used.contains(&record.keycode) {
                    now_ms
                } else {
                    record.last_used_ms
                },
                ..*record
            })
            .collect()
    }
}

pub(super) fn keysym(character: char) -> Option<u32> {
    let keysym = xkeysym::Keysym::from_char(character);
    (keysym != xkeysym::Keysym::NoSymbol).then(|| keysym.raw())
}

/// The records that still describe the server keymap, one for each keycode.
/// A record is ours only while the server holds exactly what we mapped: the
/// keysym on the first two levels, and on each other level the keysym or none.
fn owned(layout: &Layout<'_>, previous: &[Borrowed]) -> Vec<Borrowed> {
    let mut borrowed: Vec<Borrowed> = previous
        .iter()
        .filter(|record| {
            let symbols = layout.symbols(record.keycode);
            record.keycode != RESERVED_KEYCODE
                && !layout.is_modifier(record.keycode)
                && record.keysym != 0
                && symbols.get(..2) == Some(&[record.keysym, record.keysym][..])
                && symbols.iter().all(|symbol| *symbol == 0 || *symbol == record.keysym)
        })
        .copied()
        .collect();
    borrowed.sort_by_key(|record| record.keycode);
    borrowed.dedup_by_key(|record| record.keycode);
    borrowed
}

/// When no keycode is unused, picks the temporary keycode with the oldest last
/// use to clear, so that the input backend of the other actions can start.
pub(super) fn spare_candidate(layout: &Layout<'_>, previous: &[Borrowed]) -> Option<Borrowed> {
    let unused = layout
        .keycodes()
        .any(|(keycode, symbols)| keycode != RESERVED_KEYCODE && symbols.iter().all(|symbol| *symbol == 0));
    if unused {
        return None;
    }
    owned(layout, previous)
        .into_iter()
        .min_by_key(|record| (record.last_used_ms, record.keycode))
}

/// The present on the planning timeline. Records store 32-bit X server
/// times, which wrap after about 49 days. The planner instead uses a 64-bit
/// timeline on which "now" is this constant and every record is in the past.
pub(super) const TIMELINE_NOW: u64 = 1 << 40;

/// The pacing between two strokes.
pub(super) const KEY_INTERVAL: Duration = Duration::from_millis(20);

/// The longest lease: the strokes of a 256-character action, twice the paced
/// time, plus one second.
pub(super) const MAX_LEASE: Duration = KEY_INTERVAL
    .saturating_mul(2 * 256)
    .saturating_add(Duration::from_secs(1));

/// Converts a stored X server time to the planning timeline at `server_now`.
/// Only a lease is in the future, and a lease is at most `MAX_LEASE` ahead.
/// Any other time is in the past, up to the full 32-bit range of about 49 days.
pub(super) fn to_timeline(stored: u64, server_now: u32) -> u64 {
    let ahead = low_word(stored).wrapping_sub(server_now);
    if u128::from(ahead) <= MAX_LEASE.as_millis() {
        TIMELINE_NOW + u64::from(ahead)
    } else {
        TIMELINE_NOW - u64::from(server_now.wrapping_sub(low_word(stored)))
    }
}

/// How long the strokes of one action can keep their keycodes busy, in
/// milliseconds: twice the paced duration, plus one second.
pub(super) fn lease_ms(strokes: usize) -> u64 {
    let paced = u64::try_from(KEY_INTERVAL.as_millis()).unwrap_or(u64::MAX);
    let lease = u64::try_from(strokes)
        .unwrap_or(u64::MAX)
        .saturating_mul(paced)
        .saturating_mul(2)
        .saturating_add(1_000);
    lease.min(u64::try_from(MAX_LEASE.as_millis()).unwrap_or(u64::MAX))
}

/// Converts a planning timeline time back to an X server time at `server_now`.
pub(super) fn from_timeline(timeline: u64, server_now: u32) -> u64 {
    u64::from(server_now.wrapping_add(low_word(timeline.wrapping_sub(TIMELINE_NOW))))
}

fn low_word(value: u64) -> u32 {
    u32::try_from(value & u64::from(u32::MAX)).unwrap_or_default()
}

/// The earliest timeline time in milliseconds at which a keycode last used at
/// `last_used_ms` may get a different keysym.
pub(super) fn quiet_after(last_used_ms: u64) -> u64 {
    last_used_ms.saturating_add(u64::try_from(RECLAIM_QUIET.as_millis()).unwrap_or(u64::MAX))
}

/// The modifier keys and state that select a level of the current keymap.
#[derive(Clone, Copy, Debug)]
pub(super) struct Keyboard {
    /// A keycode bound to Shift; without one, only first-level keysyms are used.
    pub shift_keycode: Option<u8>,
    /// Caps Lock is on, so Shift selects the lowercase level of a letter key.
    pub caps_lock: bool,
}

/// Why the current modifier state does not allow text input.
#[derive(Debug, PartialEq, Eq)]
pub(super) enum ModifierError {
    /// A modifier other than Lock and Num Lock is active.
    Held,
    /// Lock is active, but its row has no Caps Lock key, for example Shift Lock.
    Lock,
}

const CAPS_LOCK: u32 = 0xffe5;
const NUM_LOCK: u32 = 0xff7f;

/// Derives the keyboard for the planner from the eight core modifier rows
/// (Shift, Lock, Control, Mod1 to Mod5) and the core state mask. Any slot can
/// hold Alt, Super or Num Lock, so the rows decide which bits are allowed.
pub(super) fn keyboard(layout: &Layout<'_>, rows: &[&[u8]], state: u16) -> Result<Keyboard, ModifierError> {
    let keycodes = |row: usize| {
        rows.get(row)
            .into_iter()
            .flat_map(|keycodes| keycodes.iter().copied())
            .filter(|keycode| *keycode != 0 && *keycode != RESERVED_KEYCODE)
    };
    // The state bit cannot tell which key of a row is down. Thus, a row is
    // exempt only when each of its keys is the exempt key. A key is the
    // exempt key only with the exempt keysym on its first level, because the
    // key acts with the symbol of its first level.
    let only = |row: usize, keysym: u32| {
        keycodes(row).next().is_some() && keycodes(row).all(|keycode| layout.symbols(keycode).first() == Some(&keysym))
    };
    let active = |row: usize| state & (1_u16 << row) != 0;
    // Shift (0), Control (2) and Mod1 to Mod5 (3 to 7), but not a Num Lock row.
    if [0, 2, 3, 4, 5, 6, 7]
        .into_iter()
        .any(|row| active(row) && !(row >= 3 && only(row, NUM_LOCK)))
    {
        return Err(ModifierError::Held);
    }
    let caps_lock = active(1);
    if caps_lock && !only(1, CAPS_LOCK) {
        return Err(ModifierError::Lock);
    }
    Ok(Keyboard {
        // XTEST presses the key without a modifier, so it types its first-level
        // symbol. Any other key in the Shift row would type that symbol with
        // each stroke.
        shift_keycode: keycodes(0).find(|keycode| {
            layout
                .symbols(*keycode)
                .first()
                .is_some_and(|symbol| matches!(symbol, 0xffe1 | 0xffe2))
        }),
        caps_lock,
    })
}

/// Plans the strokes for `text` without changing any existing keycode that a
/// queued event could still use.
pub(super) fn plan(
    layout: &Layout<'_>,
    previous: &[Borrowed],
    keyboard: Keyboard,
    text: &str,
) -> Result<Plan, PlanError> {
    let mut borrowed = owned(layout, previous);
    let mut located: BTreeMap<u32, Stroke> = BTreeMap::new();
    let mut missing: Vec<u32> = Vec::new();
    for character in text.chars() {
        let symbol = keysym(character).ok_or(PlanError::NoKeysym)?;
        if located.contains_key(&symbol) || missing.contains(&symbol) {
            continue;
        }
        if keyboard.caps_lock && is_cased(character) {
            return Err(PlanError::CapsLock);
        }
        match locate(layout, keyboard, symbol) {
            Some(stroke) => {
                located.insert(symbol, stroke);
            }
            None => missing.push(symbol),
        }
    }

    let needed: BTreeSet<u8> = located.values().map(|stroke| stroke.keycode).collect();
    // The lowest unused keycode stays unused: the input backend for the other
    // actions refuses a keymap without one and maps its own keysyms there.
    let free = layout
        .keycodes()
        .filter(|(keycode, symbols)| *keycode != RESERVED_KEYCODE && symbols.iter().all(|symbol| *symbol == 0))
        .map(|(keycode, _)| (keycode, None))
        .skip(1)
        .filter(|(keycode, _)| !layout.is_modifier(*keycode))
        .collect::<Vec<_>>()
        .into_iter()
        .rev();
    let mut reclaimable: Vec<&Borrowed> = borrowed
        .iter()
        .filter(|record| !needed.contains(&record.keycode))
        .collect();
    reclaimable.sort_by_key(|record| (record.last_used_ms, record.keycode));
    let mut slots = free.chain(
        reclaimable
            .into_iter()
            .map(|record| (record.keycode, Some(record.last_used_ms))),
    );

    let mut bindings = Vec::with_capacity(missing.len());
    let mut not_before_ms = 0;
    for symbol in missing {
        let (keycode, last_used_ms) = slots.next().ok_or(PlanError::Capacity)?;
        if let Some(last_used_ms) = last_used_ms {
            not_before_ms = not_before_ms.max(quiet_after(last_used_ms));
        }
        located.insert(symbol, Stroke { keycode, shift: false });
        bindings.push((keycode, symbol));
    }
    bindings.sort_unstable();

    borrowed.retain(|record| !bindings.iter().any(|(keycode, _)| *keycode == record.keycode));
    borrowed.extend(bindings.iter().map(|&(keycode, keysym)| Borrowed {
        keycode,
        keysym,
        last_used_ms: 0,
    }));
    borrowed.sort_by_key(|record| record.keycode);

    let strokes: Vec<Stroke> = text
        .chars()
        .filter_map(|character| keysym(character).and_then(|symbol| located.get(&symbol).copied()))
        .collect();
    let used = strokes.iter().map(|stroke| stroke.keycode).collect();
    Ok(Plan {
        strokes,
        bindings,
        not_before_ms,
        borrowed,
        used,
    })
}

/// Finds a key that gives `symbol` without Shift first, then with Shift.
fn locate(layout: &Layout<'_>, keyboard: Keyboard, symbol: u32) -> Option<Stroke> {
    [false, true]
        .into_iter()
        .filter(|shift| !shift || keyboard.shift_keycode.is_some())
        .find_map(|shift| {
            layout.keycodes().find_map(|(keycode, _)| {
                (keycode != RESERVED_KEYCODE
                    && !layout.is_modifier(keycode)
                    && layout.typed_symbol(keycode, shift) == Some(symbol))
                .then_some(Stroke { keycode, shift })
            })
        })
}

/// A character that a case conversion changes. With Caps Lock on, the key type
/// decides whether a client gives the lowercase or the uppercase form: Lock
/// selects the level on an alphabetic type, and on another type a client can
/// convert the keysym to uppercase.
fn is_cased(character: char) -> bool {
    character.to_lowercase().ne(std::iter::once(character)) || character.to_uppercase().ne(std::iter::once(character))
}

/// Groups bindings into runs of consecutive keycodes for one request each.
pub(super) fn runs(bindings: &[(u8, u32)]) -> Vec<(u8, Vec<u32>)> {
    let mut runs: Vec<(u8, Vec<u32>)> = Vec::new();
    for &(keycode, keysym) in bindings {
        match runs.last_mut() {
            Some((first, symbols)) if usize::from(*first) + symbols.len() / 2 == usize::from(keycode) => {
                symbols.extend([keysym, keysym]);
            }
            _ => runs.push((keycode, vec![keysym, keysym])),
        }
    }
    runs
}

#[cfg(test)]
mod tests;
