use super::{
    Borrowed, Keyboard, Layout, ModifierError, PlanError, RECLAIM_QUIET, Stroke, TIMELINE_NOW, from_timeline, keyboard,
    keysym, plan, quiet_after, runs, spare_candidate, to_timeline,
};

const SHIFT: u8 = 50;
const SHIFTED: Keyboard = Keyboard {
    shift_keycode: Some(SHIFT),
    caps_lock: false,
};
const NO_SHIFT: Keyboard = Keyboard {
    shift_keycode: None,
    caps_lock: false,
};
const FIRST_FREE: u8 = 100;
const FREE: usize = 5;

/// A small US-like layout: letters, digits, minus/underscore and space, plus
/// `FREE` empty keycodes starting at `FIRST_FREE`.
fn layout() -> Vec<u32> {
    let mut keysyms = vec![0; usize::from(u8::MAX - 8 + 1) * 2];
    let mut set = |keycode: u8, lower: u32, upper: u32| {
        let index = usize::from(keycode - 8) * 2;
        keysyms[index] = lower;
        keysyms[index + 1] = upper;
    };
    for keycode in 9..=u8::MAX {
        set(keycode, 0xfe00 | u32::from(keycode), 0);
    }
    for (offset, letter) in (b'a'..=b'z').enumerate() {
        let keycode = 10 + u8::try_from(offset).unwrap_or_default();
        set(keycode, u32::from(letter), u32::from(letter.to_ascii_uppercase()));
    }
    for (offset, digit) in (b'0'..=b'9').enumerate() {
        set(40 + u8::try_from(offset).unwrap_or_default(), u32::from(digit), 0);
    }
    set(SHIFT, 0xffe1, 0);
    set(51, u32::from(b'-'), u32::from(b'_'));
    set(52, u32::from(b' '), 0);
    // Keycode 8 must never be used, even when it holds a matching keysym.
    set(8, u32::from(b'#'), 0);
    for keycode in FIRST_FREE..FIRST_FREE + u8::try_from(FREE).unwrap_or_default() {
        set(keycode, 0, 0);
    }
    keysyms
}

fn view3(keysyms: &[u32]) -> Layout<'_> {
    Layout {
        min_keycode: 8,
        keysyms_per_keycode: 3,
        keysyms,
    }
}

fn view(keysyms: &[u32]) -> Layout<'_> {
    Layout {
        min_keycode: 8,
        keysyms_per_keycode: 2,
        keysyms,
    }
}

fn apply(keysyms: &mut [u32], bindings: &[(u8, u32)]) {
    for &(keycode, keysym) in bindings {
        let index = usize::from(keycode - 8) * 2;
        keysyms[index] = keysym;
        keysyms[index + 1] = keysym;
    }
}

#[test]
fn layout_characters_use_existing_keys_and_shift_without_mapping() -> Result<(), PlanError> {
    let keysyms = layout();
    let text = "Synthetic_KEY-0123456789abcdefghijklmnopqrstuvwxyzABCDEFGHIJKLMNOPQRSTUVWXYZ z";
    let plan = plan(&view(&keysyms), &[], SHIFTED, text)?;
    assert!(plan.bindings.is_empty(), "no keycode is remapped");
    assert_eq!(plan.not_before_ms, 0);
    assert_eq!(plan.strokes.len(), text.chars().count());
    assert_eq!(
        plan.strokes[0],
        Stroke {
            keycode: 10 + 18,
            shift: true
        }
    );
    assert_eq!(
        plan.strokes[1],
        Stroke {
            keycode: 10 + 24,
            shift: false
        }
    );
    assert_eq!(
        plan.strokes[9],
        Stroke {
            keycode: 51,
            shift: true
        }
    );
    assert_eq!(
        plan.strokes[13],
        Stroke {
            keycode: 51,
            shift: false
        }
    );
    assert!(plan.record(5).is_empty());
    Ok(())
}

#[test]
fn shifted_characters_need_a_mapping_without_a_shift_key() -> Result<(), PlanError> {
    let keysyms = layout();
    let plan = plan(&view(&keysyms), &[], NO_SHIFT, "aA_")?;
    assert_eq!(
        plan.strokes[0],
        Stroke {
            keycode: 10,
            shift: false
        }
    );
    assert_eq!(
        plan.bindings,
        vec![(FIRST_FREE + 3, u32::from(b'_')), (FIRST_FREE + 4, u32::from(b'A'))]
    );
    Ok(())
}

#[test]
fn reserved_keycode_is_never_used() -> Result<(), PlanError> {
    let mut keysyms = layout();
    let plan_hash = plan(&view(&keysyms), &[], SHIFTED, "#")?;
    assert_eq!(plan_hash.bindings, vec![(FIRST_FREE + 4, u32::from(b'#'))]);
    keysyms[0] = 0;
    let previous = [Borrowed {
        keycode: 8,
        keysym: 0,
        last_used_ms: 0,
    }];
    let plan = plan(&view(&keysyms), &previous, SHIFTED, "æ")?;
    assert_eq!(plan.bindings, vec![(FIRST_FREE + 4, 0xe6)]);
    assert!(plan.record(1).iter().all(|record| record.keycode != 8));
    Ok(())
}

#[test]
fn temporary_mappings_are_reused_unchanged_across_actions() -> Result<(), PlanError> {
    let mut keysyms = layout();
    let first = plan(&view(&keysyms), &[], SHIFTED, "æø🦀")?;
    let crab = keysym('🦀').ok_or(PlanError::NoKeysym)?;
    assert_eq!(
        first.bindings,
        vec![(FIRST_FREE + 2, crab), (FIRST_FREE + 3, 0xf8), (FIRST_FREE + 4, 0xe6)]
    );
    apply(&mut keysyms, &first.bindings);
    let record = first.record(1_000);
    assert_eq!(record.len(), 3);
    assert!(record.iter().all(|entry| entry.last_used_ms == 1_000));

    let second = plan(&view(&keysyms), &record, SHIFTED, "øæå")?;
    assert_eq!(
        second.bindings,
        vec![(FIRST_FREE + 1, 0xe5)],
        "only new symbols are mapped"
    );
    assert_eq!(second.not_before_ms, 0, "unused keycodes need no quiet interval");
    assert_eq!(
        second.strokes[0],
        Stroke {
            keycode: FIRST_FREE + 3,
            shift: false
        }
    );
    assert_eq!(
        second.strokes[1],
        Stroke {
            keycode: FIRST_FREE + 4,
            shift: false
        }
    );
    let record = second.record(2_000);
    assert_eq!(
        record
            .iter()
            .map(|entry| (entry.keycode, entry.last_used_ms))
            .collect::<Vec<_>>(),
        vec![
            (FIRST_FREE + 1, 2_000),
            (FIRST_FREE + 2, 1_000),
            (FIRST_FREE + 3, 2_000),
            (FIRST_FREE + 4, 2_000)
        ]
    );
    Ok(())
}

#[test]
fn reclaim_takes_least_recently_used_keycodes_after_quiet_interval() -> Result<(), PlanError> {
    let mut keysyms = layout();
    let quiet = u64::try_from(RECLAIM_QUIET.as_millis()).unwrap_or(u64::MAX);
    let previous: Vec<Borrowed> = (0..FREE)
        .map(|index| {
            let keycode = FIRST_FREE + u8::try_from(index).unwrap_or_default();
            let keysym = 0x0100_e000 + u32::from(keycode);
            Borrowed {
                keycode,
                keysym,
                last_used_ms: 10_000 - 1_000 * u64::from(keycode - FIRST_FREE),
            }
        })
        .collect();
    let previous_bindings: Vec<(u8, u32)> = previous.iter().map(|record| (record.keycode, record.keysym)).collect();
    apply(&mut keysyms, &previous_bindings);
    let reused = char::from_u32(0xe000 + u32::from(FIRST_FREE)).ok_or(PlanError::NoKeysym)?;
    let text = format!("{reused}æø");
    let plan = plan(&view(&keysyms), &previous, SHIFTED, &text)?;
    // Keycode FIRST_FREE stays because this text uses it; the two least
    // recently used of the others are reassigned.
    assert_eq!(plan.bindings, vec![(FIRST_FREE + 3, 0xf8), (FIRST_FREE + 4, 0xe6)]);
    assert_eq!(plan.not_before_ms, 7_000 + quiet);
    assert_eq!(
        plan.strokes[0],
        Stroke {
            keycode: FIRST_FREE,
            shift: false
        }
    );
    Ok(())
}

#[test]
fn capacity_counts_unused_and_reclaimable_keycodes() {
    let keysyms = layout();
    let fits: String = (0..FREE - 1)
        .filter_map(|index| char::from_u32(0xe000 + u32::try_from(index).unwrap_or_default()))
        .collect();
    assert!(plan(&view(&keysyms), &[], SHIFTED, &fits).is_ok());
    let too_many = format!("{fits}\u{f000}");
    assert_eq!(
        plan(&view(&keysyms), &[], SHIFTED, &too_many).err(),
        Some(PlanError::Capacity)
    );
}

#[test]
fn foreign_changes_invalidate_borrowed_records() -> Result<(), PlanError> {
    let mut keysyms = layout();
    apply(&mut keysyms, &[(FIRST_FREE, 0x1234)]);
    let previous = [Borrowed {
        keycode: FIRST_FREE,
        keysym: 0xe6,
        last_used_ms: 1,
    }];
    let plan = plan(&view(&keysyms), &previous, SHIFTED, "æ")?;
    assert_eq!(plan.bindings, vec![(FIRST_FREE + 4, 0xe6)]);
    assert_eq!(plan.not_before_ms, 0);
    assert_eq!(plan.record(3).len(), 1, "foreign keycode is no longer recorded");

    // A third level that another client added also ends ownership.
    let mut wide = vec![0; keysyms.len() / 2 * 3];
    for (keycode, symbols) in keysyms.chunks(2).enumerate() {
        wide[keycode * 3..keycode * 3 + 2].copy_from_slice(symbols);
    }
    let index = usize::from(FIRST_FREE + 3 - 8) * 3;
    wide[index..index + 3].copy_from_slice(&[0xe6, 0xe6, 0xe6]);
    let owned = [Borrowed {
        keycode: FIRST_FREE + 3,
        keysym: 0xe6,
        last_used_ms: 1,
    }];
    assert_eq!(super::owned(&view3(&wide), &owned), owned.to_vec());
    wide[index + 2] = 0xfe03;
    assert!(super::owned(&view3(&wide), &owned).is_empty(), "foreign third level");
    Ok(())
}

#[test]
fn lowest_unused_keycode_stays_unused_for_other_actions() -> Result<(), PlanError> {
    let mut keysyms = layout();
    let text: String = (0..FREE - 1)
        .filter_map(|index| char::from_u32(0xe000 + u32::try_from(index).unwrap_or_default()))
        .collect();
    let plan = plan(&view(&keysyms), &[], SHIFTED, &text)?;
    apply(&mut keysyms, &plan.bindings);
    let unused: Vec<u8> = (FIRST_FREE..FIRST_FREE + u8::try_from(FREE).unwrap_or_default())
        .filter(|keycode| keysyms[usize::from(keycode - 8) * 2] == 0)
        .collect();
    assert_eq!(unused, vec![FIRST_FREE]);
    Ok(())
}

#[test]
fn characters_without_keysym_are_rejected() {
    let keysyms = layout();
    assert_eq!(
        plan(&view(&keysyms), &[], SHIFTED, "a\u{fffe}").err(),
        Some(PlanError::NoKeysym)
    );
}

#[test]
fn records_round_trip_and_bindings_group_into_runs() {
    let records = vec![
        Borrowed {
            keycode: 97,
            keysym: 0x0101_f980,
            last_used_ms: (7 << 32) | 9,
        },
        Borrowed {
            keycode: 103,
            keysym: 0xe6,
            last_used_ms: 0,
        },
    ];
    let words = Borrowed::encode(&records);
    assert_eq!(words.len(), 8);
    assert_eq!(Borrowed::decode(&words), records);
    assert_eq!(Borrowed::decode(&[300, 1, 0, 0, 97]), Vec::new());
    assert_eq!(
        runs(&[(97, 1), (98, 2), (100, 3)]),
        vec![(97, vec![1, 1, 2, 2]), (100, vec![3, 3])]
    );
}

#[test]
fn a_full_keymap_releases_the_least_recently_used_temporary_keycode() {
    let mut keysyms = layout();
    let records: Vec<Borrowed> = (0..FREE)
        .map(|index| {
            let keycode = FIRST_FREE + u8::try_from(index).unwrap_or_default();
            Borrowed {
                keycode,
                keysym: 0x0100_e000 + u32::from(keycode),
                last_used_ms: 50 + u64::from(keycode % 3),
            }
        })
        .collect();
    let all: Vec<(u8, u32)> = records.iter().map(|record| (record.keycode, record.keysym)).collect();
    apply(&mut keysyms, &all[1..]);
    assert_eq!(
        spare_candidate(&view(&keysyms), &records),
        None,
        "one keycode is still unused"
    );
    apply(&mut keysyms, &all[..1]);
    let candidate = spare_candidate(&view(&keysyms), &records);
    assert_eq!(
        candidate.map(|record| record.keycode),
        Some(102),
        "oldest use, then lowest keycode"
    );
    assert_eq!(
        quiet_after(50),
        50 + u64::try_from(RECLAIM_QUIET.as_millis()).unwrap_or_default()
    );
    assert_eq!(
        spare_candidate(&view(&keysyms), &[]),
        None,
        "foreign mappings are never cleared"
    );
}

#[test]
fn caps_lock_inverts_shift_only_for_letter_keys() -> Result<(), PlanError> {
    let mut keysyms = layout();
    apply(&mut keysyms, &[(FIRST_FREE + 4, 0xe6)]);
    let locked = Keyboard {
        caps_lock: true,
        ..SHIFTED
    };
    let plan = plan(&view(&keysyms), &[], locked, "aA_-æ")?;
    let shifts: Vec<bool> = plan.strokes.iter().map(|stroke| stroke.shift).collect();
    assert_eq!(shifts, vec![true, false, true, false, false]);
    assert!(plan.bindings.is_empty());
    let unshifted = Keyboard {
        shift_keycode: None,
        caps_lock: true,
    };
    let plan = super::plan(&view(&keysyms), &[], unshifted, "aA")?;
    assert_eq!(
        plan.strokes[1],
        Stroke {
            keycode: 10,
            shift: false
        },
        "Caps Lock alone gives the uppercase letter"
    );
    assert_eq!(plan.bindings, vec![(FIRST_FREE + 3, u32::from(b'a'))]);
    Ok(())
}

#[test]
fn server_times_convert_to_a_timeline_across_the_32_bit_wrap() {
    let now = 5_u32;
    let stored = u64::from(u32::MAX - 994);
    assert_eq!(to_timeline(stored, now), TIMELINE_NOW - 1_000);
    assert_eq!(from_timeline(TIMELINE_NOW - 1_000, now), stored);
    assert_eq!(from_timeline(TIMELINE_NOW + 20, now), 25);
    assert_eq!(to_timeline(25, 25), TIMELINE_NOW);
    // A lease ahead of the server time stays in the future.
    assert_eq!(to_timeline(1_005, 5), TIMELINE_NOW + 1_000);
    assert_eq!(to_timeline(3, u32::MAX - 6), TIMELINE_NOW + 10);
    assert_eq!(from_timeline(TIMELINE_NOW + 10, u32::MAX - 6), 3);
    assert_eq!(super::lease_ms(256), 11_240);
    assert_eq!(
        super::lease_ms(10_000),
        11_240,
        "a lease never exceeds the longest action"
    );
    // A time further ahead than the longest lease is old, not a lease.
    assert_eq!(to_timeline(11_245, 5), TIMELINE_NOW + 11_240);
    assert_eq!(
        to_timeline(11_246, 5),
        TIMELINE_NOW - u64::from(5_u32.wrapping_sub(11_246))
    );
    let month_old = 5_u32.wrapping_sub(30 * 24 * 3_600 * 1_000);
    assert_eq!(to_timeline(u64::from(month_old), 5), TIMELINE_NOW - 2_592_000_000);
    // An old record with a 64-bit time keeps only its low word.
    assert_eq!(to_timeline((7 << 32) | 0x14, 25), TIMELINE_NOW - 5);
}

#[test]
fn modifier_rows_decide_which_state_bits_block_text_input() {
    let mut keysyms = layout();
    let mut set = |keycode: u8, keysym: u32| keysyms[usize::from(keycode - 8) * 2] = keysym;
    set(54, 0xffe5); // Caps_Lock
    set(55, 0xffe6); // Shift_Lock
    set(56, 0xff7f); // Num_Lock
    set(57, 0xffe9); // Alt_L
    let shift_row: &[u8] = &[SHIFT, 0];
    let rows = |lock: u8, mod1: u8, mod2: u8| -> Vec<Vec<u8>> {
        vec![
            shift_row.to_vec(),
            vec![lock],
            vec![0],
            vec![mod1],
            vec![mod2],
            vec![0],
            vec![0],
            vec![0],
        ]
    };
    let check = |rows: &[Vec<u8>], state: u16| {
        let rows: Vec<&[u8]> = rows.iter().map(Vec::as_slice).collect();
        keyboard(&view(&keysyms), &rows, state)
    };
    let usual = rows(54, 57, 56);
    assert_eq!(
        check(&usual, 0).map(|k| (k.shift_keycode, k.caps_lock)),
        Ok((Some(SHIFT), false))
    );
    assert!(check(&usual, 0x10).is_ok(), "Num Lock on Mod2");
    assert_eq!(check(&usual, 0x08).err(), Some(ModifierError::Held), "Alt on Mod1");
    assert_eq!(check(&usual, 0x01).err(), Some(ModifierError::Held), "Shift");
    assert_eq!(check(&usual, 0x04).err(), Some(ModifierError::Held), "Control");
    assert_eq!(check(&usual, 0x02).map(|k| k.caps_lock), Ok(true), "Caps Lock");
    let swapped = rows(54, 56, 57);
    assert!(check(&swapped, 0x08).is_ok(), "Num Lock on Mod1");
    assert_eq!(check(&swapped, 0x10).err(), Some(ModifierError::Held), "Alt on Mod2");
    let shift_lock = rows(55, 57, 56);
    assert_eq!(check(&shift_lock, 0x02).err(), Some(ModifierError::Lock));
    assert!(check(&shift_lock, 0).is_ok(), "an inactive Shift Lock is harmless");

    // A row that mixes the exempt key with another key is not exempt.
    let mut mixed = rows(54, 0, 56);
    mixed[4].push(57);
    assert_eq!(check(&mixed, 0x10).err(), Some(ModifierError::Held), "Num Lock and Alt");
    mixed[1].push(55);
    assert_eq!(
        check(&mixed, 0x02).err(),
        Some(ModifierError::Lock),
        "Caps Lock and Shift Lock"
    );
    // Keycode 8 and keys other than Shift are never the Shift key.
    let mut reserved = rows(54, 57, 56);
    reserved[0] = vec![8, 57, SHIFT];
    assert_eq!(check(&reserved, 0).map(|k| k.shift_keycode), Ok(Some(SHIFT)));
    reserved[0] = vec![57];
    assert_eq!(check(&reserved, 0).map(|k| k.shift_keycode), Ok(None));
}
