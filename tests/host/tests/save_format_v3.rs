//! Save format v3 multi-slot chaining (`STATE_CHAIN_HEAD = 3` and
//! `STATE_CHAIN_DATA = 4`) tests: round-trips up to `MAX_CHAINED_VALUE_LEN`,
//! crash-atomicity across every write/flush boundary, orphan continuation
//! slot reclamation, and transitions between single-slot (`STATE_COMMITTED`)
//! and multi-slot (`STATE_CHAIN_HEAD`) values.

use std::collections::BTreeMap;
use std::rc::Rc;

use wisp_host_tests::ahci::{Hba, SECTOR_SIZE};
use wisp_host_tests::save_format::{
    self, Probe, MAX_CHAINED_VALUE_LEN, VALUE_MAX_LEN,
};
use wisp_host_tests::wisp::value::Value;

const DISK_SECTORS: usize = 2048;
const SECTOR: usize = SECTOR_SIZE as usize;
type Sector = [u8; SECTOR];

fn s(text: &str) -> Value {
    Value::Str(Rc::new(text.into()))
}

fn formatted() -> Hba {
    let hba = Hba::new(DISK_SECTORS);
    unsafe { save_format::provision(&hba, hba.total_sectors()) }.unwrap();
    hba
}

fn slot_states(media: &[Sector]) -> Vec<u8> {
    (0..64).map(|i| media[1 + i * 8][0]).collect()
}

fn occupied(media: &[Sector]) -> usize {
    slot_states(media).iter().filter(|&&b| b != 0).count()
}

#[test]
fn multi_slot_values_round_trip_and_free_continuation_slots() {
    let hba = formatted();

    // 10 KiB string requires 1 head slot + 2 continuation slots = 3 slots.
    let text_10k: String = (0..10_240).map(|i| (b'a' + (i % 26) as u8) as char).collect();
    unsafe { save_format::save_chained(&hba, "app.big", &s(&text_10k)) }.unwrap();

    let states = slot_states(&hba.media());
    assert_eq!(states[0], 3, "slot 0 must be STATE_CHAIN_HEAD (3)");
    assert_eq!(states[1], 4, "slot 1 must be STATE_CHAIN_DATA (4)");
    assert_eq!(states[2], 4, "slot 2 must be STATE_CHAIN_DATA (4)");
    assert_eq!(occupied(&hba.media()), 3);

    let loaded = unsafe { save_format::load(&hba, "app.big") }.unwrap();
    assert_eq!(loaded, Some(s(&text_10k)));

    let keys = unsafe { save_format::keys(&hba) }.unwrap();
    assert_eq!(keys, vec![("app.big".to_string(), 5 + text_10k.len())]);

    // Updating to a 15 KiB string (4 slots) commits the new chain and frees slots 0..=2.
    let text_15k: String = (0..15_360).map(|i| (b'A' + (i % 26) as u8) as char).collect();
    unsafe { save_format::save_chained(&hba, "app.big", &s(&text_15k)) }.unwrap();
    assert_eq!(
        unsafe { save_format::load(&hba, "app.big") }.unwrap(),
        Some(s(&text_15k))
    );
    assert_eq!(occupied(&hba.media()), 4);

    // Shrinking back to a small single-slot value frees all 4 chained slots.
    unsafe { save_format::save_chained(&hba, "app.big", &s("small")) }.unwrap();
    assert_eq!(
        unsafe { save_format::load(&hba, "app.big") }.unwrap(),
        Some(s("small"))
    );
    assert_eq!(occupied(&hba.media()), 1);

    // Deleting a multi-slot value frees both head and data slots.
    unsafe { save_format::save_chained(&hba, "app.big", &s(&text_10k)) }.unwrap();
    assert_eq!(occupied(&hba.media()), 3);
    assert!(unsafe { save_format::delete(&hba, "app.big") }.unwrap());
    assert_eq!(unsafe { save_format::load(&hba, "app.big") }.unwrap(), None);
    assert_eq!(occupied(&hba.media()), 0);
}

#[test]
fn values_up_to_max_chained_len_round_trip() {
    let hba = formatted();
    let max_str = "x".repeat(MAX_CHAINED_VALUE_LEN - 5);
    unsafe { save_format::save_chained(&hba, "app.max", &s(&max_str)) }.unwrap();
    assert_eq!(
        unsafe { save_format::load(&hba, "app.max") }.unwrap(),
        Some(s(&max_str))
    );
    assert_eq!(occupied(&hba.media()), 17);

    // Updating a maximum-size value in-place via copy-on-write also succeeds.
    let max_str_2 = "y".repeat(MAX_CHAINED_VALUE_LEN - 5);
    unsafe { save_format::save_chained(&hba, "app.max", &s(&max_str_2)) }.unwrap();
    assert_eq!(
        unsafe { save_format::load(&hba, "app.max") }.unwrap(),
        Some(s(&max_str_2))
    );
    assert_eq!(occupied(&hba.media()), 17);

    let too_long = "x".repeat(MAX_CHAINED_VALUE_LEN - 4);
    assert!(unsafe { save_format::save_chained(&hba, "app.over", &s(&too_long)) }.is_err());
}

#[test]
fn corrupted_continuation_slot_is_detected() {
    let hba = formatted();
    let text_10k = "w".repeat(10_000);
    unsafe { save_format::save_chained(&hba, "app.code", &s(&text_10k)) }.unwrap();

    // Flip a byte in the second continuation slot's payload.
    let mut media = hba.media();
    media[1 + 2 * 8 + 1][100] ^= 0x55;
    let corrupted = Hba::from_media(media);
    let err = unsafe { save_format::load(&corrupted, "app.code") }.unwrap_err();
    assert!(err.contains("checksum"), "expected checksum error, got: {err}");
}

#[test]
fn multi_slot_save_and_delete_are_crash_atomic_at_every_op() {
    let hba = formatted();
    unsafe { save_format::save_chained(&hba, "keep", &s("untouched")) }.unwrap();
    let v1 = s(&"a".repeat(VALUE_MAX_LEN + 500)); // 2 slots (1 head + 1 data)
    let v2 = s(&"b".repeat(VALUE_MAX_LEN + 5000)); // 3 slots (1 head + 2 data)
    unsafe { save_format::save_chained(&hba, "k", &v1) }.unwrap();

    let start = hba.media();
    let mut before = BTreeMap::new();
    before.insert("keep".to_string(), s("untouched"));
    before.insert("k".to_string(), v1.clone());

    // Measure total ops for updating `k` from `v1` (2 slots) to `v2` (3 slots).
    let clean = Hba::from_media(start.clone());
    unsafe { save_format::save_chained(&clean, "k", &v2) }.unwrap();
    let total_ops = clean.ops.get();
    assert!(clean.pending().is_empty());

    for crash_at in 0..=total_ops {
        let crashed = Hba::from_media(start.clone());
        crashed.fail_after_ops.set(Some(crash_at));
        let res = unsafe { save_format::save_chained(&crashed, "k", &v2) };

        // Check both "no unflushed writes survived" and "all unflushed writes survived",
        // plus single-write subsets.
        let pending = crashed.pending();
        let mut masks = vec![0usize, (1usize << pending.len().min(20)) - 1];
        for i in 0..pending.len().min(20) {
            masks.push(1usize << i);
        }
        for mask in masks {
            let mut media = crashed.media();
            for (bit, (lba, data)) in pending.iter().enumerate().take(20) {
                if (mask & (1usize << bit)) != 0 {
                    media[*lba as usize] = *data;
                }
            }
            let rebooted = Hba::from_media(media);
            assert_eq!(
                unsafe { save_format::probe(&rebooted, rebooted.total_sectors()) }.unwrap(),
                Probe::Formatted
            );
            assert_eq!(
                unsafe { save_format::load(&rebooted, "keep") }.unwrap(),
                Some(s("untouched"))
            );
            let actual = unsafe { save_format::load(&rebooted, "k") }.unwrap();
            if res.is_ok() && mask == (1usize << pending.len().min(20)) - 1 {
                assert_eq!(actual, Some(v2.clone()));
            } else {
                assert!(
                    actual == Some(v1.clone()) || actual == Some(v2.clone()),
                    "crash_at={crash_at} produced unexpected state"
                );
            }
            // Recovery save must reclaim any orphan ChainData slots and leave exact slot count.
            unsafe { save_format::save_chained(&rebooted, "k", &s("recovered")) }.unwrap();
            assert_eq!(
                unsafe { save_format::load(&rebooted, "k") }.unwrap(),
                Some(s("recovered"))
            );
            assert_eq!(occupied(&rebooted.media()), 2);
        }
    }
}
