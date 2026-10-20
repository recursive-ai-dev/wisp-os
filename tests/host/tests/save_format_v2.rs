//! Save format version 2: copy-on-write slots with generations.
//!
//! The central claim (`src/save_format.rs`'s module doc comment) is that a
//! crash or power loss at *any* point during a save or delete leaves the
//! key holding either its old value or its new one, and every other key
//! untouched. These tests check it the hard way: for each operation, stop
//! the device after every possible number of writes/flushes, then build
//! the disk image for *every* subset of the still-unflushed writes having
//! reached the media (a write-back cache may persist any of them, in any
//! order), mount it, and check every key. Then they check the store
//! recovers: a further save succeeds and leaves no stale copies behind.
//!
//! Version-1 disks are built here by an independent encoder written from
//! `docs/SAVE_FORMAT.md`, not by the code under test.

use std::collections::BTreeMap;
use std::rc::Rc;

use wisp_host_tests::ahci::{Hba, SECTOR_SIZE};
use wisp_host_tests::save_format::{self, Probe, MAX_KEYS, REGION_SECTORS, VALUE_MAX_LEN};
use wisp_host_tests::wisp::value::{ListCell, Value};

const DISK_SECTORS: usize = 2048;
const SECTOR: usize = SECTOR_SIZE as usize;
type Sector = [u8; SECTOR];

fn s(text: &str) -> Value {
    Value::Str(Rc::new(text.into()))
}

fn list(items: Vec<Value>) -> Value {
    Value::List(Rc::new(ListCell::new(items)))
}

fn show(v: &Option<Value>) -> String {
    match v {
        Some(v) => format!("{:.40}", v.to_display_string()),
        None => "<absent>".into(),
    }
}

fn formatted() -> Hba {
    let hba = Hba::new(DISK_SECTORS);
    unsafe { save_format::provision(&hba, hba.total_sectors()) }.unwrap();
    hba
}

fn put(hba: &Hba, key: &str, value: Value) {
    unsafe { save_format::save(hba, key, &value) }.unwrap_or_else(|e| panic!("save({key}) failed: {e}"));
}

fn get(hba: &Hba, key: &str) -> Option<Value> {
    unsafe { save_format::load(hba, key) }.unwrap_or_else(|e| panic!("load({key}) failed: {e}"))
}

/// State byte of every slot, straight from the media.
fn slot_states(media: &[Sector]) -> Vec<u8> {
    (0..64).map(|i| media[1 + i * 8][0]).collect()
}

fn occupied(media: &[Sector]) -> usize {
    slot_states(media).iter().filter(|&&b| b != 0).count()
}

// ---- an independent version-1 encoder (docs/SAVE_FORMAT.md, v1) ---------

fn fnv1a32(data: &[u8]) -> u32 {
    let mut h: u32 = 0x811c_9dc5;
    for &b in data {
        h ^= u32::from(b);
        h = h.wrapping_mul(0x0100_0193);
    }
    h
}

/// A version-1 store holding `entries`, one per slot from slot 0, exactly
/// as the v1.0.0 kernel wrote it.
fn legacy_disk(entries: &[(String, Value)]) -> Hba {
    let hba = Hba::new(DISK_SECTORS);
    let mut sb = [0u8; SECTOR];
    sb[..9].copy_from_slice(b"WISPSAVE1");
    sb[9] = 1;
    sb[10..12].copy_from_slice(&64u16.to_le_bytes());
    sb[12..16].copy_from_slice(&4096u32.to_le_bytes());
    hba.seed(0, &sb);
    for (slot, (key, value)) in entries.iter().enumerate() {
        let encoded = save_format::encode(value).unwrap();
        let mut raw = vec![0u8; 4096];
        raw[0] = 1;
        raw[1..1 + key.len()].copy_from_slice(key.as_bytes());
        raw[33..37].copy_from_slice(&(encoded.len() as u32).to_le_bytes());
        raw[37..41].copy_from_slice(&fnv1a32(&encoded).to_le_bytes());
        raw[41..41 + encoded.len()].copy_from_slice(&encoded);
        for i in 0..8 {
            hba.seed(1 + slot as u64 * 8 + i as u64, raw[i * SECTOR..(i + 1) * SECTOR].try_into().unwrap());
        }
    }
    hba
}

// ---- the exhaustive crash harness ----------------------------------------

#[derive(Clone)]
enum Op {
    Save(&'static str, Value),
    Delete(&'static str),
}

impl Op {
    fn key(&self) -> &'static str {
        match self {
            Op::Save(k, _) | Op::Delete(k) => k,
        }
    }

    fn apply(&self, hba: &Hba) -> Result<(), &'static str> {
        match self {
            Op::Save(k, v) => unsafe { save_format::save(hba, k, v) },
            Op::Delete(k) => unsafe { save_format::delete(hba, k) }.map(|_| ()),
        }
    }

    /// The op's key's value once the op has completed.
    fn new_value(&self) -> Option<Value> {
        match self {
            Op::Save(_, Value::Nil) | Op::Delete(_) => None,
            Op::Save(_, v) => Some(v.clone()),
        }
    }
}

/// Run `op` against `start` crashed after every possible number of device
/// operations, under every possible subset of unflushed writes surviving,
/// and check the old-or-new invariant and recovery on each resulting disk.
/// `before` is what `start` holds. Returns how many disk images were
/// checked.
fn check_crash_atomic(start: &[Sector], before: &BTreeMap<String, Value>, op: Op) -> usize {
    let clean = Hba::from_media(start.to_vec());
    op.apply(&clean).expect("the operation must succeed on a healthy device");
    let total_ops = clean.ops.get();
    assert!(clean.pending().is_empty(), "a completed operation left unflushed writes");

    let key = op.key();
    let old = before.get(key).cloned();
    let new = op.new_value();
    let mut images = 0;

    for crash_at in 0..=total_ops {
        let crashed = Hba::from_media(start.to_vec());
        crashed.fail_after_ops.set(Some(crash_at));
        let result = op.apply(&crashed);
        let pending = crashed.pending();
        assert!(pending.len() <= 10, "more unflushed writes than this harness enumerates");

        for mask in 0u32..(1 << pending.len()) {
            let mut media = crashed.media();
            for (bit, (lba, data)) in pending.iter().enumerate() {
                if mask & (1 << bit) != 0 {
                    media[*lba as usize] = *data;
                }
            }
            let disk = Hba::from_media(media);
            images += 1;
            let ctx = format!("{key}: crash after {crash_at}/{total_ops} ops, persisted-pending mask {mask:#b}");

            assert_eq!(
                unsafe { save_format::probe(&disk, disk.total_sectors()) },
                Ok(Probe::Formatted),
                "{ctx}: store no longer mounts"
            );
            // A store still labelled version 1 must hold nothing a
            // version-1 kernel would misread.
            let media = disk.media();
            if media[0][9] == 1 {
                assert!(
                    slot_states(&media).iter().all(|&b| b <= 1),
                    "{ctx}: a version-2 copy is on a disk still labelled version 1"
                );
            }
            let got = unsafe { save_format::load(&disk, key) }
                .unwrap_or_else(|e| panic!("{ctx}: load failed: {e}"));
            assert!(
                got == old || got == new,
                "{ctx}: got {} -- neither old {} nor new {}",
                show(&got),
                show(&old),
                show(&new)
            );
            if result.is_ok() {
                assert!(got == new, "{ctx}: the operation reported success but the old value survived");
            }
            for (other, value) in before.iter().filter(|(k, _)| k.as_str() != key) {
                assert!(get(&disk, other).as_ref() == Some(value), "{ctx}: unrelated key '{other}' changed");
            }

            // Recovery: the store keeps working, and a successful save
            // leaves exactly one copy per key.
            put(&disk, key, s("recovered"));
            put(&disk, "zz_after", Value::Int(7));
            assert!(get(&disk, key) == Some(s("recovered")), "{ctx}: recovery save lost");
            let keys = unsafe { save_format::keys(&disk) }.unwrap();
            assert_eq!(
                occupied(&disk.media()),
                keys.len(),
                "{ctx}: stale copies left behind after a successful save ({keys:?})"
            );
        }
    }
    images
}

fn store_with(entries: &[(&str, Value)]) -> (Vec<Sector>, BTreeMap<String, Value>) {
    let hba = formatted();
    for (k, v) in entries {
        put(&hba, k, v.clone());
    }
    (hba.media(), entries.iter().map(|(k, v)| (k.to_string(), v.clone())).collect())
}

#[test]
fn small_update_is_crash_atomic() {
    let (start, before) = store_with(&[("a", Value::Int(1)), ("b", s("two"))]);
    check_crash_atomic(&start, &before, Op::Save("a", Value::Int(2)));
}

#[test]
fn multi_sector_update_is_crash_atomic() {
    let (start, before) = store_with(&[("big", s(&"x".repeat(3000))), ("a", Value::Int(1))]);
    let images = check_crash_atomic(&start, &before, Op::Save("big", s(&"y".repeat(4000))));
    // Seven value sectors were in flight at once: every subset was tried.
    assert!(images > 128, "only {images} disk images checked");
}

#[test]
fn shrinking_and_growing_updates_are_crash_atomic() {
    let (start, before) = store_with(&[("big", s(&"x".repeat(4000)))]);
    check_crash_atomic(&start, &before, Op::Save("big", Value::Bool(true)));
    let (start, before) = store_with(&[("small", Value::Bool(false))]);
    check_crash_atomic(&start, &before, Op::Save("small", list(vec![s(&"z".repeat(2000)), Value::Int(3)])));
}

#[test]
fn new_key_is_crash_atomic() {
    let (start, before) = store_with(&[("a", Value::Int(1))]);
    check_crash_atomic(&start, &before, Op::Save("c", list(vec![Value::Int(1), s("x")])));
}

#[test]
fn delete_and_nil_save_are_crash_atomic() {
    let (start, before) = store_with(&[("a", Value::Int(1)), ("b", Value::Int(2))]);
    check_crash_atomic(&start, &before, Op::Delete("b"));
    check_crash_atomic(&start, &before, Op::Save("a", Value::Nil));
}

/// A save that crashed after its commit but before freeing the old copy
/// leaves a stale copy behind. Returns that disk: "a" = 2 committed, and a
/// stale copy of "a" = 1 still on the media.
fn store_with_stale_copy() -> Hba {
    let hba = formatted();
    put(&hba, "a", Value::Int(1));
    put(&hba, "b", s("bee"));
    // A small value: header write, flush (the commit), then the cleanup's
    // write and flush. Kill the device right at the cleanup.
    hba.ops.set(0);
    hba.fail_after_ops.set(Some(2));
    put(&hba, "a", Value::Int(2));
    hba.fail_after_ops.set(None);
    hba.power_loss();
    assert_eq!(occupied(&hba.media()), 3, "expected a stale copy of 'a' on the media");
    hba
}

#[test]
fn a_save_whose_cleanup_fails_still_reports_success() {
    let hba = store_with_stale_copy();
    assert!(get(&hba, "a") == Some(Value::Int(2)));
    let keys = unsafe { save_format::keys(&hba) }.unwrap();
    assert_eq!(keys.iter().filter(|(k, _)| k == "a").count(), 1, "{keys:?}");
}

#[test]
fn stale_copies_never_resurface() {
    // Deleting a key that has a stale older copy: the older value must
    // never come back, whatever the crash point.
    let hba = store_with_stale_copy();
    let before: BTreeMap<String, Value> = [("a".into(), Value::Int(2)), ("b".into(), s("bee"))].into_iter().collect();
    check_crash_atomic(&hba.media(), &before, Op::Delete("a"));
    check_crash_atomic(&hba.media(), &before, Op::Save("a", Value::Int(3)));
    check_crash_atomic(&hba.media(), &before, Op::Save("b", Value::Int(4)));
}

#[test]
fn a_corrupt_winner_is_reported_not_papered_over() {
    let hba = store_with_stale_copy();
    // Find the winning copy of "a" (generation 2) and flip a value byte.
    let media = hba.media();
    let slot = (0..64)
        .find(|&i| media[1 + i * 8][0] == 2 && &media[1 + i * 8][1..3] == b"a\0" && media[1 + i * 8][41] == 2)
        .expect("winner of 'a'");
    let mut sector = media[1 + slot * 8];
    sector[49 + 1] ^= 0xFF;
    hba.seed(1 + slot as u64 * 8, &sector);
    let err = unsafe { save_format::load(&hba, "a") }.unwrap_err();
    assert!(err.contains("checksum"), "{err}");
    // Saving over it works, and the new value wins.
    put(&hba, "a", Value::Int(5));
    assert!(get(&hba, "a") == Some(Value::Int(5)));
}

#[test]
fn a_corrupted_generation_cannot_promote_a_stale_copy() {
    // The checksum covers the generation. Without that, a bit flip that
    // raised a stale copy's generation would make it the winner, and
    // `load` would silently return the older value.
    let hba = store_with_stale_copy();
    let media = hba.media();
    let stale = (0..64)
        .find(|&i| media[1 + i * 8][0] == 2 && &media[1 + i * 8][1..3] == b"a\0" && media[1 + i * 8][41] == 1)
        .expect("stale copy of 'a'");
    let mut sector = media[1 + stale * 8];
    sector[41 + 7] ^= 0x40; // generation's top byte
    hba.seed(1 + stale as u64 * 8, &sector);
    match unsafe { save_format::load(&hba, "a") } {
        Err(e) => assert!(e.contains("checksum"), "{e}"),
        Ok(v) => assert!(v == Some(Value::Int(2)), "stale value {} served as current", show(&v)),
    }
}

#[test]
fn the_documented_worked_example_is_byte_exact() {
    // docs/SAVE_FORMAT.md, "Worked example".
    let hba = formatted();
    put(&hba, "greeting", s("hello world"));
    let slot0 = hba.media()[1];
    let mut expected = [0u8; 65];
    expected[0] = 0x02;
    expected[1..9].copy_from_slice(b"greeting");
    expected[33..37].copy_from_slice(&[0x10, 0, 0, 0]);
    expected[37..41].copy_from_slice(&[0x45, 0x13, 0xc2, 0x97]);
    expected[41..49].copy_from_slice(&[1, 0, 0, 0, 0, 0, 0, 0]);
    expected[49..65].copy_from_slice(&[0x03, 0x0b, 0, 0, 0, b'h', b'e', b'l', b'l', b'o', b' ', b'w', b'o', b'r', b'l', b'd']);
    assert_eq!(&slot0[..65], &expected[..]);
    assert!(slot0[65..].iter().all(|&b| b == 0));

    put(&hba, "greeting", s("hello again"));
    let media = hba.media();
    assert_eq!(slot_states(&media)[..3], [0, 2, 0], "update must go to slot 1 and free slot 0");
    assert_eq!(media[1 + 8][41], 2, "generation 2");
}

// ---- capacity, sizes, listing --------------------------------------------

#[test]
fn the_store_holds_63_keys_and_keeps_one_slot_free() {
    let hba = formatted();
    for i in 0..MAX_KEYS {
        put(&hba, &format!("k{i}"), Value::Int(i as i64));
    }
    let err = unsafe { save_format::save(&hba, "one_too_many", &Value::Int(0)) }.unwrap_err();
    assert!(err.contains("full"), "{err}");
    // Updating a stored key still works at capacity -- that is what the
    // free slot is for.
    put(&hba, "k5", s("updated"));
    assert!(get(&hba, "k5") == Some(s("updated")));
    assert_eq!(occupied(&hba.media()), MAX_KEYS);
    // Deleting makes room.
    assert!(unsafe { save_format::delete(&hba, "k0") }.unwrap());
    assert!(!unsafe { save_format::delete(&hba, "k0") }.unwrap());
    put(&hba, "one_too_many", Value::Int(0));
    assert_eq!(unsafe { save_format::keys(&hba) }.unwrap().len(), MAX_KEYS);
}

#[test]
fn values_up_to_the_maximum_round_trip() {
    let hba = formatted();
    for len in [0usize, 1, 455, 456, 457, 968, 969, VALUE_MAX_LEN - 5] {
        let v = s(&"q".repeat(len));
        put(&hba, "v", v.clone());
        assert!(get(&hba, "v") == Some(v), "string of {len} bytes did not round-trip");
    }
    let too_big = s(&"q".repeat(VALUE_MAX_LEN - 4));
    assert!(unsafe { save_format::save(&hba, "v", &too_big) }.is_err());
}

#[test]
fn keys_lists_each_stored_key_once() {
    let hba = store_with_stale_copy();
    put(&hba, "list", list(vec![Value::Int(1), Value::Bool(true)]));
    let mut keys = unsafe { save_format::keys(&hba) }.unwrap();
    keys.sort();
    let names: Vec<&str> = keys.iter().map(|(k, _)| k.as_str()).collect();
    assert_eq!(names, ["a", "b", "list"]);
}

#[test]
fn invalid_keys_are_rejected() {
    let hba = formatted();
    for key in ["", &"k".repeat(33), "a\0"] {
        assert!(unsafe { save_format::save(&hba, key, &Value::Int(1)) }.is_err(), "{key:?}");
        assert!(unsafe { save_format::load(&hba, key) }.is_err(), "{key:?}");
    }
}

#[test]
fn lookups_read_one_sector_per_slot() {
    let hba = formatted();
    put(&hba, "big", s(&"x".repeat(4000)));
    hba.reads.set(0);
    get(&hba, "big");
    // 64 header sectors, then the 8 sectors of the one matching copy.
    assert_eq!(hba.reads.get(), 64 + 8);
    hba.reads.set(0);
    assert!(get(&hba, "missing").is_none());
    assert_eq!(hba.reads.get(), 64);
}

#[test]
fn nothing_outside_the_region_is_ever_written() {
    let hba = Hba::new(DISK_SECTORS);
    for lba in 0..DISK_SECTORS as u64 {
        hba.seed(lba, &[0xEE; SECTOR]);
    }
    unsafe { save_format::provision(&hba, hba.total_sectors()) }.unwrap();
    for i in 0..40 {
        put(&hba, &format!("k{}", i % 7), s(&"v".repeat(i * 97)));
        if i % 5 == 0 {
            unsafe { save_format::delete(&hba, &format!("k{}", i % 3)) }.unwrap();
        }
    }
    for (lba, sector) in hba.media().iter().enumerate().skip(REGION_SECTORS as usize) {
        assert_eq!(*sector, [0xEE; SECTOR], "sector {lba} outside the region was modified");
    }
}

// ---- version 1 stores ----------------------------------------------------

fn legacy_entries(n: usize) -> Vec<(String, Value)> {
    (0..n)
        .map(|i| (format!("old{i}"), list(vec![Value::Int(i as i64), s(&"p".repeat(i * 60))])))
        .collect()
}

#[test]
fn a_version_1_store_is_read_and_upgraded_by_its_first_save() {
    let entries = legacy_entries(5);
    let hba = legacy_disk(&entries);
    assert_eq!(unsafe { save_format::probe(&hba, hba.total_sectors()) }, Ok(Probe::Formatted));
    for (k, v) in &entries {
        assert!(get(&hba, k).as_ref() == Some(v), "legacy key {k} unreadable");
    }
    // Reads never write, and do not upgrade.
    assert_eq!(hba.writes.get(), 0);
    assert_eq!(hba.media()[0][9], 1);

    put(&hba, "new", s("fresh"));
    assert_eq!(hba.media()[0][9], 2, "superblock not upgraded");
    put(&hba, "old2", s("replaced"));
    assert!(get(&hba, "old2") == Some(s("replaced")));
    for (k, v) in entries.iter().filter(|(k, _)| k != "old2") {
        assert!(get(&hba, k).as_ref() == Some(v), "legacy key {k} lost by the upgrade");
    }
    assert_eq!(occupied(&hba.media()), 6);
}

#[test]
fn the_upgrading_save_is_crash_atomic() {
    let entries = legacy_entries(3);
    let hba = legacy_disk(&entries);
    let before: BTreeMap<String, Value> = entries.iter().cloned().collect();
    check_crash_atomic(&hba.media(), &before, Op::Save("old1", s(&"u".repeat(1500))));
    check_crash_atomic(&hba.media(), &before, Op::Save("brand_new", Value::Int(9)));
    check_crash_atomic(&hba.media(), &before, Op::Delete("old0"));
}

#[test]
fn a_full_version_1_store_explains_itself() {
    // v1.0.0 allowed all 64 slots to be used; with no free slot, an update
    // cannot be written copy-on-write. It must fail clearly, not fall back
    // to an in-place write, and deleting a key must fix it.
    let entries = legacy_entries(64);
    let hba = legacy_disk(&entries);
    let err = unsafe { save_format::save(&hba, "old3", &Value::Int(1)) }.unwrap_err();
    assert!(err.contains("no free slot") && err.contains("delete"), "{err}");
    let err = unsafe { save_format::save(&hba, "another", &Value::Int(1)) }.unwrap_err();
    assert!(err.contains("full"), "{err}");
    assert!(get(&hba, "old3").as_ref() == Some(&entries[3].1), "failed save changed the value");
    assert_eq!(hba.media()[0][9], 1, "a save that failed must not have upgraded the store");

    assert!(unsafe { save_format::delete(&hba, "old10") }.unwrap());
    put(&hba, "old3", Value::Int(1));
    assert!(get(&hba, "old3") == Some(Value::Int(1)));
}

#[test]
fn a_torn_version_1_slot_is_reported_and_can_be_overwritten() {
    let entries = legacy_entries(2);
    let hba = legacy_disk(&entries);
    let mut sector = hba.media()[1 + 8];
    sector[45] ^= 0x01;
    hba.seed(1 + 8, &sector);
    let err = unsafe { save_format::load(&hba, "old1") }.unwrap_err();
    assert!(err.contains("checksum"), "{err}");
    put(&hba, "old1", Value::Int(3));
    assert!(get(&hba, "old1") == Some(Value::Int(3)));
    assert!(get(&hba, "old0").as_ref() == Some(&entries[0].1));
}
