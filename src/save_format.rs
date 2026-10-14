//! A hand-designed, fixed-slot key/value store on top of `ahci.rs`'s raw
//! sector read/write -- Build Order Step 8 of the Phase 2 plan, and the
//! only way a WISP program's `save`/`load` data survives a reboot.
//!
//! # Why not a real filesystem
//!
//! Writable FAT32 needs synchronized dual FAT tables, cluster-chain
//! allocation, and long-filename directory entries -- real engineering
//! effort with no user-visible payoff yet: nothing needs a hierarchical
//! filesystem, and apps are still served from `fs::snapshot_apps`'s
//! read-only RAM snapshot of the boot USB stick (which this format never
//! touches -- it lives entirely on the second, AHCI-attached disk). A flat
//! key/value store of fixed-size slots is the simplest thing that can
//! actually hold a script's save data across a reboot, and is trivial to
//! reason about: no allocation graph, no fragmentation, just a linear scan.
//!
//! # On-disk layout
//!
//! ```text
//! LBA 0:                superblock (this sector only)
//! LBA 1 .. 1+64*8:       64 fixed-size slots, 8 sectors (4 KiB) each
//! ```
//!
//! **Superblock** (512 bytes, LBA 0):
//! - bytes 0..9:   magic, [`MAGIC`] (`"WISPSAVE1"`)
//! - byte  9:      format version ([`FORMAT_VERSION`])
//! - bytes 10..12: slot count, `u16` little-endian ([`SLOT_COUNT`])
//! - bytes 12..16: slot size in bytes, `u32` little-endian ([`SLOT_SIZE`])
//! - remaining bytes: reserved, zeroed
//!
//! **Slot** (4096 bytes = 8 sectors):
//! - byte  0:        in-use flag (`0` = free, `1` = occupied)
//! - bytes 1..33:    key, zero-padded UTF-8, up to [`KEY_MAX_LEN`] bytes
//! - bytes 33..37:   value length, `u32` little-endian
//! - bytes 37..41:   checksum (FNV-1a/32) of the value bytes
//! - bytes 41..4096: the serialized value (see below), zero-padded
//!
//! `save`/`load` both do a linear scan over all `SLOT_COUNT` slots --
//! adequate at this scale (64 slots), no index needed. See
//! `docs/SAVE_FORMAT.md` for the full spec this module implements.
//!
//! # Value encoding
//!
//! A minimal type-tagged binary format: one tag byte, then a type-specific
//! payload. `Nil`/`Bool`/`Int`/`Str` encode directly; `List` encodes its
//! element count followed by each element's own tagged encoding,
//! recursively, bounded by `wisp::value`'s own [`wisp::value::MAX_VALUE_DEPTH`]
//! -- the same recursion guard the interpreter itself already uses for
//! nested-list equality/display, not a separately-invented limit. [`save`]
//! returns an explicit error (never silently truncates) if a value's
//! encoded form is longer than [`VALUE_MAX_LEN`] bytes.

use alloc::rc::Rc;
use alloc::string::ToString;
use alloc::vec::Vec;
use core::cell::RefCell;

use crate::ahci::Hba;
use crate::wisp::value::{Value, MAX_VALUE_DEPTH};

/// On-disk magic identifying a disk formatted for this store.
pub const MAGIC: &[u8; 9] = b"WISPSAVE1";
/// Bumped only if the on-disk layout ever changes incompatibly; a disk
/// whose superblock reports a different version is treated as unreadable
/// (see [`ensure_formatted`]) rather than misinterpreted.
const FORMAT_VERSION: u8 = 1;
/// Number of fixed-size slots -- deliberately small and fixed rather than
/// derived from disk size: 64 key/value pairs is far more than any WISP
/// script built so far (or plausibly ever) needs, and a fixed count keeps
/// every slot's LBA a simple, constant calculation.
const SLOT_COUNT: u16 = 64;
/// Sectors per slot: 8 * 512 = 4096 bytes, a whole number of sectors so
/// every slot read/write is just a small loop over [`Hba::read_sector`]/
/// [`Hba::write_sector`], never a partial-sector operation.
const SECTORS_PER_SLOT: u32 = 8;
const SLOT_SIZE: u32 = SECTORS_PER_SLOT * crate::ahci::SECTOR_SIZE;
/// Maximum key length, chosen to comfortably fit any identifier-shaped
/// name (WISP identifiers are short ASCII words) while keeping the slot
/// header itself small and fixed-size.
const KEY_MAX_LEN: usize = 32;
/// in-use flag (1) + key (`KEY_MAX_LEN`) + value length (4) + checksum (4).
const SLOT_HEADER_LEN: usize = 1 + KEY_MAX_LEN + 4 + 4;
/// The most a single value's serialized encoding can occupy: whatever's
/// left in a slot after its fixed header.
pub const VALUE_MAX_LEN: usize = SLOT_SIZE as usize - SLOT_HEADER_LEN;

const SUPERBLOCK_LBA: u64 = 0;
const SLOTS_START_LBA: u64 = 1;

const TAG_NIL: u8 = 0;
const TAG_BOOL: u8 = 1;
const TAG_INT: u8 = 2;
const TAG_STR: u8 = 3;
const TAG_LIST: u8 = 4;

fn slot_lba(slot_index: u16) -> u64 {
    SLOTS_START_LBA + (slot_index as u64) * (SECTORS_PER_SLOT as u64)
}

/// Read one whole slot ([`SECTORS_PER_SLOT`] sectors) into a single
/// contiguous buffer.
///
/// # Safety
/// Same contract as [`Hba::read_sector`].
unsafe fn read_slot_raw(hba: &Hba, slot_index: u16) -> Result<[u8; SLOT_SIZE as usize], &'static str> {
    let mut buf = [0u8; SLOT_SIZE as usize];
    let base_lba = slot_lba(slot_index);
    for i in 0..SECTORS_PER_SLOT {
        let mut sector = [0u8; crate::ahci::SECTOR_SIZE as usize];
        unsafe { hba.read_sector(base_lba + u64::from(i), &mut sector)? };
        let off = (i as usize) * crate::ahci::SECTOR_SIZE as usize;
        buf[off..off + crate::ahci::SECTOR_SIZE as usize].copy_from_slice(&sector);
    }
    Ok(buf)
}

/// Write one whole slot, sector by sector.
///
/// # Safety
/// Same contract as [`Hba::write_sector`].
unsafe fn write_slot_raw(hba: &Hba, slot_index: u16, data: &[u8; SLOT_SIZE as usize]) -> Result<(), &'static str> {
    let base_lba = slot_lba(slot_index);
    for i in 0..SECTORS_PER_SLOT {
        let off = (i as usize) * crate::ahci::SECTOR_SIZE as usize;
        let mut sector = [0u8; crate::ahci::SECTOR_SIZE as usize];
        sector.copy_from_slice(&data[off..off + crate::ahci::SECTOR_SIZE as usize]);
        unsafe { hba.write_sector(base_lba + u64::from(i), &sector)? };
    }
    Ok(())
}

fn key_field(key: &str) -> [u8; KEY_MAX_LEN] {
    let mut buf = [0u8; KEY_MAX_LEN];
    buf[..key.len()].copy_from_slice(key.as_bytes());
    buf
}

fn validate_key(key: &str) -> Result<(), &'static str> {
    if key.is_empty() {
        return Err("save_format: key must not be empty");
    }
    if key.len() > KEY_MAX_LEN {
        return Err("save_format: key is longer than the 32-byte maximum");
    }
    Ok(())
}

/// FNV-1a, 32-bit: a small, fast, dependency-free checksum -- not
/// cryptographic, just enough to catch accidental corruption (a torn
/// write, a stray bit flip) the way `docs/SAVE_FORMAT.md`'s design calls
/// for, the same "good enough for its actual job" standard as `shell.rs`'s
/// xorshift64 `rand()` PRNG (not cryptographically secure either, and
/// explicitly documented as such).
fn fnv1a32(data: &[u8]) -> u32 {
    let mut hash: u32 = 0x811c_9dc5;
    for &b in data {
        hash ^= u32::from(b);
        hash = hash.wrapping_mul(0x0100_0193);
    }
    hash
}

/// Serialize `value` into `out` using the type-tagged binary format this
/// module's doc comment describes. `depth` guards against unbounded
/// recursion on a self-referential or pathologically deep list, mirroring
/// `wisp::value::Value`'s own `MAX_VALUE_DEPTH`-bounded structural
/// operations.
fn serialize_value(value: &Value, depth: u32, out: &mut Vec<u8>) -> Result<(), &'static str> {
    if depth >= MAX_VALUE_DEPTH {
        return Err("save_format: value nesting is too deep to serialize");
    }
    match value {
        Value::Nil => out.push(TAG_NIL),
        Value::Bool(b) => {
            out.push(TAG_BOOL);
            out.push(u8::from(*b));
        }
        Value::Int(i) => {
            out.push(TAG_INT);
            out.extend_from_slice(&i.to_le_bytes());
        }
        Value::Str(s) => {
            out.push(TAG_STR);
            let bytes = s.as_bytes();
            let len =
                u32::try_from(bytes.len()).map_err(|_| "save_format: string is too long to serialize")?;
            out.extend_from_slice(&len.to_le_bytes());
            out.extend_from_slice(bytes);
        }
        Value::List(items) => {
            out.push(TAG_LIST);
            let items = items.borrow();
            let count =
                u32::try_from(items.len()).map_err(|_| "save_format: list is too long to serialize")?;
            out.extend_from_slice(&count.to_le_bytes());
            for item in items.iter() {
                serialize_value(item, depth + 1, out)?;
            }
        }
    }
    Ok(())
}

/// Decode one value from the front of `bytes`, returning it along with the
/// number of bytes consumed (so a caller decoding a list's elements can
/// advance through a shared buffer without re-slicing at every step).
/// `depth` mirrors [`serialize_value`]'s own recursion guard -- corrupt
/// data claiming impossible nesting is rejected with an error rather than
/// exhausting this kernel's real call stack.
fn deserialize_value(bytes: &[u8], depth: u32) -> Result<(Value, usize), &'static str> {
    if depth >= MAX_VALUE_DEPTH {
        return Err("save_format: stored value nesting is too deep -- corrupt data");
    }
    let (&tag, rest) = bytes
        .split_first()
        .ok_or("save_format: truncated value data (missing type tag)")?;
    match tag {
        TAG_NIL => Ok((Value::Nil, 1)),
        TAG_BOOL => {
            let b = *rest
                .first()
                .ok_or("save_format: truncated value data (missing bool payload)")?;
            Ok((Value::Bool(b != 0), 2))
        }
        TAG_INT => {
            let slice = rest
                .get(0..8)
                .ok_or("save_format: truncated value data (missing int payload)")?;
            let arr: [u8; 8] = slice.try_into().expect("slice is exactly 8 bytes long");
            Ok((Value::Int(i64::from_le_bytes(arr)), 9))
        }
        TAG_STR => {
            let len_slice = rest
                .get(0..4)
                .ok_or("save_format: truncated value data (missing string length)")?;
            let len = u32::from_le_bytes(len_slice.try_into().expect("slice is exactly 4 bytes long")) as usize;
            let str_bytes = rest
                .get(4..4 + len)
                .ok_or("save_format: truncated value data (string shorter than its declared length)")?;
            let s = core::str::from_utf8(str_bytes)
                .map_err(|_| "save_format: stored string is not valid UTF-8 -- corrupt data")?;
            Ok((Value::Str(Rc::new(s.to_string())), 1 + 4 + len))
        }
        TAG_LIST => {
            let count_slice = rest
                .get(0..4)
                .ok_or("save_format: truncated value data (missing list length)")?;
            let count = u32::from_le_bytes(count_slice.try_into().expect("slice is exactly 4 bytes long"));
            // A well-formed list can never legitimately claim more elements
            // than there are bytes left to hold them (every element is at
            // least 1 byte) -- reject an implausible count up front rather
            // than looping `count` times against corrupt data.
            if count as usize > VALUE_MAX_LEN {
                return Err("save_format: stored list length is implausibly large -- corrupt data");
            }
            let mut consumed = 1 + 4;
            let mut items = Vec::with_capacity(count as usize);
            for _ in 0..count {
                let (item, item_len) = deserialize_value(&bytes[consumed..], depth + 1)?;
                items.push(item);
                consumed += item_len;
            }
            Ok((Value::List(Rc::new(RefCell::new(items))), consumed))
        }
        _ => Err("save_format: unrecognized value type tag -- corrupt data"),
    }
}

/// Confirm `hba`'s disk already has a valid save-format superblock (magic
/// and version both match), or -- on a disk that's never been formatted
/// for this purpose (a fresh/zeroed disk, or any disk whose LBA 0 simply
/// doesn't start with [`MAGIC`]) -- write a fresh superblock and zero every
/// slot's in-use byte, so leftover bytes from whatever the disk previously
/// held can never be mistaken for a real save slot.
///
/// Idempotent: calling this again against an already-formatted disk is a
/// cheap no-op (one sector read, no writes). Must be called once before
/// [`save`]/[`load`] against a given `Hba`.
///
/// # Safety
/// Same contract as [`Hba::read_sector`]/[`Hba::write_sector`].
pub unsafe fn ensure_formatted(hba: &Hba, total_sectors: u64) -> Result<(), &'static str> {
    let required_sectors = SLOTS_START_LBA + u64::from(SLOT_COUNT) * u64::from(SECTORS_PER_SLOT);
    if total_sectors < required_sectors {
        return Err("save_format: disk is too small to hold the save-format region");
    }

    let mut sb = [0u8; crate::ahci::SECTOR_SIZE as usize];
    unsafe { hba.read_sector(SUPERBLOCK_LBA, &mut sb)? };

    if &sb[0..9] == MAGIC {
        let version = sb[9];
        if version != FORMAT_VERSION {
            return Err("save_format: on-disk superblock version is not one this driver understands");
        }
        return Ok(());
    }

    let mut new_sb = [0u8; crate::ahci::SECTOR_SIZE as usize];
    new_sb[0..9].copy_from_slice(MAGIC);
    new_sb[9] = FORMAT_VERSION;
    new_sb[10..12].copy_from_slice(&SLOT_COUNT.to_le_bytes());
    new_sb[12..16].copy_from_slice(&SLOT_SIZE.to_le_bytes());
    unsafe { hba.write_sector(SUPERBLOCK_LBA, &new_sb)? };

    let empty_slot = [0u8; SLOT_SIZE as usize]; // byte 0 (in-use flag) is 0 = free.
    for slot_index in 0..SLOT_COUNT {
        unsafe { write_slot_raw(hba, slot_index, &empty_slot)? };
    }

    Ok(())
}

fn write_value_into_slot(
    hba: &Hba,
    slot_index: u16,
    key: &str,
    encoded: &[u8],
) -> Result<(), &'static str> {
    let mut raw = [0u8; SLOT_SIZE as usize];
    raw[0] = 1;
    raw[1..1 + KEY_MAX_LEN].copy_from_slice(&key_field(key));
    let len = encoded.len() as u32;
    raw[1 + KEY_MAX_LEN..1 + KEY_MAX_LEN + 4].copy_from_slice(&len.to_le_bytes());
    let checksum = fnv1a32(encoded);
    raw[1 + KEY_MAX_LEN + 4..SLOT_HEADER_LEN].copy_from_slice(&checksum.to_le_bytes());
    raw[SLOT_HEADER_LEN..SLOT_HEADER_LEN + encoded.len()].copy_from_slice(encoded);
    // SAFETY: same contract as this module's own `write_slot_raw`.
    unsafe { write_slot_raw(hba, slot_index, &raw) }
}

/// Store `value` under `key`, overwriting any existing value already saved
/// under that same key -- Build Order Step 8 of the Phase 2 plan.
///
/// Does a full linear scan over all `SLOT_COUNT` slots: to find either an
/// existing slot for `key` (overwrite in place) or, failing that, the
/// first free slot -- adequate at this scale (64 slots), no index needed.
///
/// Returns `Err` -- never silently truncates -- if `key` is empty or
/// longer than 32 bytes, `value`'s serialized encoding is longer than
/// [`VALUE_MAX_LEN`] bytes, or every slot is already occupied by a
/// different key.
///
/// # Safety
/// Same contract as [`Hba::read_sector`]/[`Hba::write_sector`]. The caller
/// must have already called [`ensure_formatted`] on this same `Hba`.
pub unsafe fn save(hba: &Hba, key: &str, value: &Value) -> Result<(), &'static str> {
    validate_key(key)?;

    let mut encoded = Vec::new();
    serialize_value(value, 0, &mut encoded)?;
    if encoded.len() > VALUE_MAX_LEN {
        return Err("save_format: serialized value is too large to fit in one slot");
    }

    let wanted_key = key_field(key);
    let mut free_slot: Option<u16> = None;
    for slot_index in 0..SLOT_COUNT {
        // SAFETY: caller's contract (see this function's own doc comment).
        let raw = unsafe { read_slot_raw(hba, slot_index)? };
        if raw[0] == 0 {
            if free_slot.is_none() {
                free_slot = Some(slot_index);
            }
            continue;
        }
        if raw[1..1 + KEY_MAX_LEN] == wanted_key {
            return write_value_into_slot(hba, slot_index, key, &encoded);
        }
    }

    match free_slot {
        Some(slot_index) => write_value_into_slot(hba, slot_index, key, &encoded),
        None => Err("save_format: no free slot available (all 64 slots are in use)"),
    }
}

/// Look up `key`, returning `Ok(None)` if it was never saved (a first-class
/// outcome, not an error) -- Build Order Step 8 of the Phase 2 plan.
///
/// Returns `Err` if `key` is empty or too long (matching [`save`]'s own
/// validation -- an over-length key is a caller mistake worth reporting
/// clearly, not silent "not found"), or if the slot that matched `key`
/// fails its stored checksum, has an implausible stored length, or decodes
/// to malformed value data -- real corruption, never silently swallowed
/// into a wrong answer.
///
/// # Safety
/// Same contract as [`Hba::read_sector`]. The caller must have already
/// called [`ensure_formatted`] on this same `Hba`.
pub unsafe fn load(hba: &Hba, key: &str) -> Result<Option<Value>, &'static str> {
    validate_key(key)?;
    let wanted_key = key_field(key);

    for slot_index in 0..SLOT_COUNT {
        // SAFETY: caller's contract (see this function's own doc comment).
        let raw = unsafe { read_slot_raw(hba, slot_index)? };
        if raw[0] == 0 || raw[1..1 + KEY_MAX_LEN] != wanted_key {
            continue;
        }

        let value_len = u32::from_le_bytes(
            raw[1 + KEY_MAX_LEN..1 + KEY_MAX_LEN + 4]
                .try_into()
                .expect("slice is exactly 4 bytes long"),
        ) as usize;
        let stored_checksum = u32::from_le_bytes(
            raw[1 + KEY_MAX_LEN + 4..SLOT_HEADER_LEN]
                .try_into()
                .expect("slice is exactly 4 bytes long"),
        );
        if value_len > VALUE_MAX_LEN {
            return Err("save_format: corrupt slot -- stored value length exceeds the slot's capacity");
        }

        let value_bytes = &raw[SLOT_HEADER_LEN..SLOT_HEADER_LEN + value_len];
        if fnv1a32(value_bytes) != stored_checksum {
            return Err("save_format: corrupt slot -- checksum mismatch");
        }

        let (value, consumed) = deserialize_value(value_bytes, 0)?;
        if consumed != value_bytes.len() {
            return Err("save_format: corrupt slot -- trailing bytes after a fully-decoded value");
        }
        return Ok(Some(value));
    }

    Ok(None)
}
