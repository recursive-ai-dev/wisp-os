//! A hand-designed, fixed-slot key/value store on top of `storage.rs`'s raw
//! sector read/write -- Build Order Step 8 of the Phase 2 plan, and the
//! only way a WISP program's `save`/`load` data survives a reboot.
//!
//! Apps are served from the boot volume's read-only in-memory snapshot.
//! Saves use a small copy-on-write store, either on a legacy raw disk
//! or inside the dedicated WISP GPT save partition created by the installer.
//! Partition discovery validates the GPT before constructing a bounded view;
//! the boot partition and unrelated partitions are never save-write targets.
//!
//! # On-disk layout (format version 2)
//!
//! ```text
//! Relative LBA 0:           superblock (this sector only)
//! Relative LBA 1 ..= 512:   64 fixed-size slots, 8 sectors (4 KiB) each
//! ```
//!
//! LBAs are relative to the selected save region: sector zero of a raw
//! store, or the first sector of a validated WISP save partition.
//!
//! **Superblock** (512 bytes, relative LBA 0):
//! - bytes 0..9:   magic, [`MAGIC`] (`"WISPSAVE1"`)
//! - byte  9:      format version ([`FORMAT_VERSION`]; 1 is still read)
//! - bytes 10..12: slot count, `u16` little-endian ([`SLOT_COUNT`])
//! - bytes 12..16: slot size in bytes, `u32` little-endian ([`SLOT_SIZE`])
//! - remaining bytes: reserved, zeroed
//!
//! **Slot** (4096 bytes = 8 sectors). Its whole header sits in the first
//! sector, so a scan reads one sector per slot:
//! - byte  0:        state: `0` free, `2` a committed copy (`1`: a legacy
//!                   version-1 copy; see "Version 1" below)
//! - bytes 1..33:    key, zero-padded UTF-8, up to [`KEY_MAX_LEN`] bytes
//! - bytes 33..37:   value length, `u32` little-endian
//! - bytes 37..41:   checksum (FNV-1a/32) of bytes 0..37 and 41..49+length
//!                   -- the whole header except this field, and the value
//! - bytes 41..49:   generation, `u64` little-endian
//! - bytes 49..4096: the serialized value (see below), zero-padded
//!
//! See `docs/SAVE_FORMAT.md` for the full spec this module implements.
//!
//! # Updates are copy-on-write, so a crash never loses the old value
//!
//! A key may briefly have two *copies* on disk. Readers always take the
//! copy with the highest generation (ties: the lowest slot index) -- the
//! key's *winner* -- and ignore every other. [`save`] never overwrites the
//! winner in place; it writes the new value into a free slot with the next
//! generation, in this order:
//!
//! 1. the new copy's value sectors past the first, then a cache flush;
//! 2. the new copy's first sector (header, checksum, generation), then a
//!    flush -- **the commit point**: from here the new copy is the winner;
//! 3. free the old copy (zero its first sector), then a flush.
//!
//! A power loss can persist any subset of the writes not yet flushed, in
//! any order. Before step 2's flush, the new copy's header was either never
//! written or not yet durable, so its slot still reads as free and the old
//! copy wins. After it, the new copy is complete (its value was made
//! durable first) and has the higher generation, so it wins whether or not
//! step 3 survived. Every other key is untouched throughout. So after a
//! crash at any point, `load` returns the old value or the new one -- never
//! an error, never nothing. One slot is always kept free to make this
//! possible, which is why the store holds [`MAX_KEYS`] keys, not 64.
//!
//! A crash between steps 2 and 3 leaves a *stale* copy. The next [`save`]
//! or [`delete`] frees every stale copy, and flushes, before writing
//! anything else -- so a later delete can never let an older generation
//! resurface.
//!
//! # Version 1
//!
//! Version-1 stores (no generation field; checksum over the value only;
//! state `1`; values updated in place) are still mounted and read: their
//! copies count as generation 0. The first [`save`] to one upgrades it in
//! place by rewriting the superblock's version byte -- a single sector,
//! flushed before any version-2 copy is written -- so a version-1 kernel
//! afterwards reports the disk as unsupported and leaves it alone, instead
//! of misreading copies it does not understand.
//!
//! # Ownership: probe at boot, provision only on request
//!
//! Boot only reads metadata ([`probe`]). A region is used for saves only
//! if it already carries a complete, valid superblock. Blank and foreign
//! disks are left byte-for-byte untouched. The shell's `format` command
//! shows the device identity and exact physical sector range, then asks
//! for confirmation before calling [`provision`]. Partitioned disks with
//! no valid WISP save partition are refused. Every save-sector access
//! goes through [`region_lba`] before translation into a physical LBA,
//! rejecting anything outside the [`REGION_SECTORS`] this format owns.
//!
//! # Value encoding
//!
//! A minimal type-tagged binary format: one tag byte, then a type-specific
//! payload. `Nil`/`Bool`/`Int`/`Str` encode directly; `List` encodes its
//! element count followed by each element's own tagged encoding, and `Map`
//! its entry count followed by each entry's key and then value, in key
//! order -- recursively, bounded by `wisp::value`'s own [`wisp::value::MAX_VALUE_DEPTH`]
//! -- the same recursion guard the interpreter itself already uses for
//! nested-list equality/display, not a separately-invented limit. [`save`]
//! returns an explicit error (never silently truncates) if a value's
//! encoded form is longer than [`VALUE_MAX_LEN`] bytes -- checked before
//! every append, so a small but heavily shared list graph is rejected
//! after at most `VALUE_MAX_LEN` bytes of work instead of being expanded
//! in full first. Saving `nil` deletes the key: `load` already reports a
//! missing key as `nil`, so the two were never distinguishable, and this
//! way the slot is freed instead of spent on a value that says "nothing".

use alloc::collections::BTreeMap;
use alloc::rc::Rc;
use alloc::string::{String, ToString};
use alloc::vec::Vec;

use crate::storage::Disk as Hba;
use crate::wisp::value::{ListCell, MapCell, MapKey, Value, MAX_VALUE_DEPTH};

/// On-disk magic identifying a disk formatted for this store. It names the
/// store, not the layout -- that is the version byte's job -- so it stays
/// `"WISPSAVE1"` across format versions.
pub const MAGIC: &[u8; 9] = b"WISPSAVE1";
/// The layout this driver writes: copy-on-write slots with generations.
/// A disk whose superblock reports any version other than this or
/// [`LEGACY_VERSION`] is treated as unreadable (see [`probe`]) rather than
/// misinterpreted.
const FORMAT_VERSION: u8 = 2;
/// The original layout: no generation, values updated in place. Still
/// mounted and read; upgraded to [`FORMAT_VERSION`] by the first [`save`].
const LEGACY_VERSION: u8 = 1;
/// Number of fixed-size slots -- deliberately small and fixed rather than
/// derived from disk size, which keeps every slot's LBA a simple, constant
/// calculation.
const SLOT_COUNT: u16 = 64;
/// Most distinct keys the store holds: every slot but one, which is always
/// left free so an update can be written copy-on-write (see the module doc
/// comment).
pub const MAX_KEYS: usize = SLOT_COUNT as usize - 1;
/// Sectors per slot: 8 * 512 = 4096 bytes, a whole number of sectors so
/// every slot read/write is a small loop over [`Hba::read_sector`]/
/// [`Hba::write_sector`], never a partial-sector operation.
const SECTORS_PER_SLOT: u32 = 8;
const SECTOR: usize = crate::storage::SECTOR_SIZE as usize;
const SLOT_SIZE: u32 = SECTORS_PER_SLOT * crate::storage::SECTOR_SIZE;
/// Maximum key length, chosen to comfortably fit any identifier-shaped
/// name (WISP identifiers are short ASCII words) while keeping the slot
/// header itself small and fixed-size.
const KEY_MAX_LEN: usize = 32;

/// Slot states (byte 0 of a slot).
const STATE_FREE: u8 = 0;
const STATE_LEGACY: u8 = 1;
const STATE_COMMITTED: u8 = 2;
const STATE_CHAIN_HEAD: u8 = 3;
const STATE_CHAIN_DATA: u8 = 4;
const CHAIN_NONE: u16 = 0xFFFF;

/// Header field offsets within a slot (and so within its first sector).
const OFF_KEY: usize = 1;
const OFF_LEN: usize = OFF_KEY + KEY_MAX_LEN;
const OFF_CHECKSUM: usize = OFF_LEN + 4;
const OFF_GENERATION: usize = OFF_CHECKSUM + 4;
/// A version-1 header ends where the generation field begins.
const LEGACY_HEADER_LEN: usize = OFF_GENERATION;
/// state (1) + key (`KEY_MAX_LEN`) + value length (4) + checksum (4) +
/// generation (8).
const SLOT_HEADER_LEN: usize = OFF_GENERATION + 8;
/// Chained head header adds a 2-byte `first_data_slot` index after `generation`.
const CHAIN_HEAD_HEADER_LEN: usize = SLOT_HEADER_LEN + 2;
const CHAIN_HEAD_PAYLOAD_MAX: usize = SLOT_SIZE as usize - CHAIN_HEAD_HEADER_LEN;
/// Chained data slot header: state (1) + head_generation (8) + chunk_index (2)
/// + next_slot (2) + chunk_len (2) = 15 bytes.
const CHAIN_DATA_HEADER_LEN: usize = 15;
const CHAIN_DATA_PAYLOAD_MAX: usize = SLOT_SIZE as usize - CHAIN_DATA_HEADER_LEN;
/// The most a single-slot value's serialized encoding can occupy: whatever's
/// left in a slot after its fixed header.
pub const VALUE_MAX_LEN: usize = SLOT_SIZE as usize - SLOT_HEADER_LEN;
/// Maximum serialized length of a multi-slot chained value: 64 KiB payload
/// plus 5-byte string tag/length header (at most 17 slots out of 64, ensuring
/// copy-on-write updates of maximum-size values always have room to commit
/// before retiring the previous chain).
pub const MAX_CHAINED_VALUE_LEN: usize = 64 * 1024 + 5;
/// The same for a version-1 copy, whose header is 8 bytes shorter.
const LEGACY_VALUE_MAX_LEN: usize = SLOT_SIZE as usize - LEGACY_HEADER_LEN;

const SUPERBLOCK_LBA: u64 = 0;
const SLOTS_START_LBA: u64 = 1;
/// Every sector this format owns: the superblock plus all slots
/// (relative LBA 0 ..= 512). Metadata discovery may read the partition table;
/// save data operations never leave this region.
pub const REGION_SECTORS: u64 = SLOTS_START_LBA + SLOT_COUNT as u64 * SECTORS_PER_SLOT as u64;

const TAG_NIL: u8 = 0;
const TAG_BOOL: u8 = 1;
const TAG_INT: u8 = 2;
const TAG_STR: u8 = 3;
const TAG_LIST: u8 = 4;
/// Added with WISP maps, without a format version change: the slot layout
/// is the same, and a kernel that predates maps reports a value holding
/// one as unreadable (an unrecognized tag) rather than misreading it.
const TAG_MAP: u8 = 5;

const CORRUPT_LENGTH: &str = "save_format: corrupt slot -- stored value length exceeds the slot's capacity";
const CORRUPT_CHECKSUM: &str = "save_format: corrupt slot -- checksum mismatch";

fn slot_lba(slot_index: u16) -> u64 {
    SLOTS_START_LBA + (slot_index as u64) * (SECTORS_PER_SLOT as u64)
}

/// The single gate every sector access in this module passes through:
/// `lba` must fall inside the region this format owns.
fn region_lba(lba: u64) -> Result<u64, &'static str> {
    if lba < REGION_SECTORS {
        Ok(lba)
    } else {
        Err("save_format: internal error -- sector outside the save region")
    }
}

unsafe fn read_region_sector(hba: &Region<'_>, lba: u64, out: &mut [u8; SECTOR]) -> Result<(), &'static str> {
    unsafe { hba.read_sector(lba, out) }
}

unsafe fn write_region_sector(hba: &Region<'_>, lba: u64, data: &[u8; SECTOR]) -> Result<(), &'static str> {
    unsafe { hba.write_sector(lba, data) }
}

/// A view that maps relative save sectors onto one validated disk region.
/// Internal operations cannot address the GPT, ESP or adjacent partitions.
struct Region<'a> {
    hba: &'a Hba,
    start: u64,
}

impl<'a> Region<'a> {
    unsafe fn discover(hba: &'a Hba, total_sectors: u64) -> Result<Option<Self>, &'static str> {
        if total_sectors < REGION_SECTORS {
            return Err("save_format: disk is too small to hold the save-format region");
        }
        let mut sector0 = [0u8; SECTOR];
        unsafe { hba.read_sector(0, &mut sector0)? };
        // Existing raw stores take precedence, including unsupported versions.
        if &sector0[..MAGIC.len()] == MAGIC {
            return Ok(Some(Self { hba, start: 0 }));
        }
        let mut sector1 = [0u8; SECTOR];
        unsafe { hba.read_sector(1, &mut sector1)? };
        // The current GPT reader uses 512-byte LBAs. A native 4Kn GPT
        // puts its header at byte 4096, even if its protective MBR is
        // missing. Refuse that geometry before any format can touch it.
        if hba.native_sector_size() > 512 {
            let mut native_header = [0u8; SECTOR];
            unsafe { hba.read_sector(u64::from(hba.native_sector_size() / 512), &mut native_header)? };
            if &native_header[..8] == b"EFI PART" {
                return Err("save_format: native 4Kn GPT partitions are not supported; disk left untouched");
            }
        }
        if sector0[510..512] != [0x55, 0xaa] && &sector1[..8] != b"EFI PART" {
            // An unpartitioned disk may be provisioned only after consent.
            return Ok(Some(Self { hba, start: 0 }));
        }
        let found = crate::disk_layout::find_save_region(total_sectors, |lba, out| unsafe {
            hba.read_sector(lba, out)
        })?;
        match found {
            Some((start, len))
                if len >= REGION_SECTORS
                    && start.checked_add(len).is_some_and(|end| end <= total_sectors) =>
            {
                Ok(Some(Self { hba, start }))
            }
            Some(_) => Err("save_format: invalid or undersized WISP save partition"),
            None => Ok(None),
        }
    }

    unsafe fn identified(hba: &'a Hba) -> Result<Self, &'static str> {
        let sectors = hba.info().ok_or("save_format: disk has not been identified")?.total_sectors;
        let region = unsafe { Self::discover(hba, sectors)? }
            .ok_or("save_format: partitioned disk has no WISP save partition")?;
        // Mount state is only a boot-time observation. Revalidate before
        // scanning slots, especially before any stale-copy reclamation.
        let mut sb = [0u8; SECTOR];
        unsafe { region.read_sector(SUPERBLOCK_LBA, &mut sb)? };
        superblock_version(&sb)
            .map_err(|_| "save_format: the superblock is no longer valid -- the disk changed since it was mounted")?;
        Ok(region)
    }

    fn physical_lba(&self, lba: u64) -> Result<u64, &'static str> {
        self.start.checked_add(region_lba(lba)?)
            .ok_or("save_format: sector address overflow")
    }

    unsafe fn read_sector(&self, lba: u64, out: &mut [u8; SECTOR]) -> Result<(), &'static str> {
        unsafe { self.hba.read_sector(self.physical_lba(lba)?, out) }
    }

    unsafe fn write_sector(&self, lba: u64, data: &[u8; SECTOR]) -> Result<(), &'static str> {
        if self.hba.native_sector_size() != 512 {
            return Err("save_format: writes need 512-byte native blocks; 4Kn stores require an aligned future format");
        }
        unsafe { self.hba.write_sector(self.physical_lba(lba)?, data) }
    }

    unsafe fn flush_cache(&self) -> Result<(), &'static str> {
        unsafe { self.hba.flush_cache() }
    }
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
    // The key field is zero-padded, so "a" and "a\0" would be the same key.
    if key.as_bytes().contains(&0) {
        return Err("save_format: key must not contain NUL bytes");
    }
    Ok(())
}

/// FNV-1a, 32-bit, over the concatenation of `parts`: a small, fast,
/// dependency-free checksum -- not cryptographic, just enough to catch
/// accidental corruption (a torn write, a stray bit flip) the way
/// `docs/SAVE_FORMAT.md`'s design calls for, the same "good enough for its
/// actual job" standard as `shell.rs`'s xorshift64 `rand()` PRNG (not
/// cryptographically secure either, and explicitly documented as such).
fn fnv1a32(parts: &[&[u8]]) -> u32 {
    let mut hash: u32 = 0x811c_9dc5;
    for part in parts {
        for &b in *part {
            hash ^= u32::from(b);
            hash = hash.wrapping_mul(0x0100_0193);
        }
    }
    hash
}

/// Append `bytes` to `out`, or fail if that would take the encoding past
/// [`VALUE_MAX_LEN`]. Checked *before* appending, so no encoding ever
/// grows beyond one slot's capacity no matter how large the value's
/// expansion would be.
fn put_bounded(out: &mut Vec<u8>, bytes: &[u8], max_len: usize) -> Result<(), &'static str> {
    if bytes.len() > max_len.saturating_sub(out.len()) {
        return Err("save_format: serialized value is too large to fit in one slot");
    }
    out.try_reserve(bytes.len())
        .map_err(|_| "save_format: out of memory preparing the value for saving")?;
    out.extend_from_slice(bytes);
    Ok(())
}

/// Serialize `value` into `out` using the type-tagged binary format this
/// module's doc comment describes.
fn serialize_value(
    value: &Value,
    depth: u32,
    out: &mut Vec<u8>,
    max_len: usize,
) -> Result<(), &'static str> {
    if depth >= MAX_VALUE_DEPTH {
        return Err("save_format: value nesting is too deep to serialize");
    }
    match value {
        Value::Nil => put_bounded(out, &[TAG_NIL], max_len)?,
        // A function is a name, meaningful only to the program that holds
        // it; stored, it would read back after a reboot as a reference to
        // whatever happens to carry that name then.
        Value::Func(_) | Value::Closure(_) => return Err("save_format: a function cannot be saved"),
        Value::Bool(b) => put_bounded(out, &[TAG_BOOL, u8::from(*b)], max_len)?,
        Value::Int(i) => {
            put_bounded(out, &[TAG_INT], max_len)?;
            put_bounded(out, &i.to_le_bytes(), max_len)?;
        }
        Value::Str(s) => {
            let bytes = s.as_bytes();
            let len =
                u32::try_from(bytes.len()).map_err(|_| "save_format: string is too long to serialize")?;
            put_bounded(out, &[TAG_STR], max_len)?;
            put_bounded(out, &len.to_le_bytes(), max_len)?;
            put_bounded(out, bytes, max_len)?;
        }
        Value::List(items) => {
            let items = items.borrow();
            let count =
                u32::try_from(items.len()).map_err(|_| "save_format: list is too long to serialize")?;
            put_bounded(out, &[TAG_LIST], max_len)?;
            put_bounded(out, &count.to_le_bytes(), max_len)?;
            for item in items.iter() {
                serialize_value(item, depth + 1, out, max_len)?;
            }
        }
        Value::Map(entries) => {
            let entries = entries.borrow();
            let count =
                u32::try_from(entries.len()).map_err(|_| "save_format: map is too large to serialize")?;
            put_bounded(out, &[TAG_MAP], max_len)?;
            put_bounded(out, &count.to_le_bytes(), max_len)?;
            for (key, item) in entries.iter() {
                serialize_value(&key.to_value(), depth + 1, out, max_len)?;
                serialize_value(item, depth + 1, out, max_len)?;
            }
        }
    }
    Ok(())
}

/// Encode `value` for single-slot storage, failing (without allocating beyond
/// one slot's worth) if it does not fit in [`VALUE_MAX_LEN`] bytes.
pub fn encode(value: &Value) -> Result<Vec<u8>, &'static str> {
    let mut out = Vec::new();
    out.try_reserve_exact(VALUE_MAX_LEN)
        .map_err(|_| "save_format: out of memory preparing the value for saving")?;
    serialize_value(value, 0, &mut out, VALUE_MAX_LEN)?;
    Ok(out)
}

/// Encode `value` for multi-slot chained storage (up to [`MAX_CHAINED_VALUE_LEN`]).
pub fn encode_chained(value: &Value) -> Result<Vec<u8>, &'static str> {
    let mut out = Vec::new();
    serialize_value(value, 0, &mut out, MAX_CHAINED_VALUE_LEN)
        .map_err(|e| {
            if e == "save_format: serialized value is too large to fit in one slot" {
                "save_format: serialized value exceeds the multi-slot store capacity"
            } else {
                e
            }
        })?;
    Ok(out)
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
            if count as usize > rest.len() - 4 {
                return Err("save_format: stored list length is implausibly large -- corrupt data");
            }
            let mut consumed = 1 + 4;
            let mut items = Vec::new();
            for _ in 0..count {
                let (item, item_len) = deserialize_value(&bytes[consumed..], depth + 1)?;
                // Reserve only for values that were actually decoded. Even
                // individually plausible nested counts can collectively
                // describe far more items than the slot contains.
                items.try_reserve(1)
                    .map_err(|_| "save_format: out of memory decoding a stored list")?;
                items.push(item);
                consumed += item_len;
            }
            Ok((Value::List(Rc::new(ListCell::new(items))), consumed))
        }
        TAG_MAP => {
            let count_slice = rest
                .get(0..4)
                .ok_or("save_format: truncated value data (missing map length)")?;
            let count = u32::from_le_bytes(count_slice.try_into().expect("slice is exactly 4 bytes long"));
            // The shortest valid key is an empty string (five bytes), and
            // every value uses at least one byte.
            if count as usize > (rest.len() - 4) / 6 {
                return Err("save_format: stored map length is implausibly large -- corrupt data");
            }
            let mut consumed = 1 + 4;
            let mut entries = BTreeMap::new();
            for _ in 0..count {
                let (key, key_len) = deserialize_value(&bytes[consumed..], depth + 1)?;
                consumed += key_len;
                let key = MapKey::from_value(&key)
                    .ok_or("save_format: stored map key is not an int or a string -- corrupt data")?;
                // `serialize_value` writes entries in key order, so a key
                // that is not greater than the one before it (including a
                // repeated key) is not something it could have written.
                if entries.last_key_value().is_some_and(|(last, _)| *last >= key) {
                    return Err("save_format: stored map keys are out of order -- corrupt data");
                }
                let (item, item_len) = deserialize_value(&bytes[consumed..], depth + 1)?;
                consumed += item_len;
                entries.insert(key, item);
            }
            Ok((Value::Map(Rc::new(MapCell::new(entries))), consumed))
        }
        _ => Err("save_format: unrecognized value type tag -- corrupt data"),
    }
}

/// What [`probe`] found in the selected save region.
#[derive(Clone, Copy, PartialEq, Eq, Debug)]
pub enum Probe {
    /// A complete superblock this driver understands (version 2, or
    /// version 1, which the first save upgrades): save/load may use the
    /// disk.
    Formatted,
    /// The region's superblock is all zeros, including an interrupted
    /// earlier `format` (the superblock is written last).
    Blank,
    /// The region holds non-WISP data, or a partitioned disk has no WISP
    /// save partition. Foreign partitions are never reformatted.
    Foreign,
    /// A WISP superblock whose version or geometry this driver does not
    /// understand.
    Unsupported,
}

/// Whether `sb` is a complete superblock of a version this driver reads,
/// and if so which version. `Err` carries the [`Probe`] verdict otherwise.
fn superblock_version(sb: &[u8; SECTOR]) -> Result<u8, Probe> {
    if &sb[0..9] != MAGIC {
        return Err(if sb.iter().all(|&b| b == 0) { Probe::Blank } else { Probe::Foreign });
    }
    let version = sb[9];
    let slot_count = u16::from_le_bytes([sb[10], sb[11]]);
    let slot_size = u32::from_le_bytes([sb[12], sb[13], sb[14], sb[15]]);
    if !matches!(version, FORMAT_VERSION | LEGACY_VERSION) || slot_count != SLOT_COUNT || slot_size != SLOT_SIZE {
        return Err(Probe::Unsupported);
    }
    Ok(version)
}

/// Read-only check of whether `hba`'s disk already carries a valid,
/// complete save-format superblock, either on a legacy raw disk or inside a
/// validated WISP GPT partition. Never writes -- boot calls this, never
/// [`provision`].
///
/// # Safety
/// Same contract as [`Hba::read_sector`].
pub unsafe fn probe(hba: &Hba, total_sectors: u64) -> Result<Probe, &'static str> {
    let Some(region) = (unsafe { Region::discover(hba, total_sectors)? }) else {
        return Ok(Probe::Foreign);
    };
    let mut sb = [0u8; SECTOR];
    unsafe { read_region_sector(&region, SUPERBLOCK_LBA, &mut sb)? };
    Ok(match superblock_version(&sb) {
        Ok(_) => Probe::Formatted,
        Err(verdict) => verdict,
    })
}

/// Format `hba`'s selected save region, destroying exactly
/// [`REGION_SECTORS`] sectors starting at the raw store or WISP partition.
/// Only ever called after the user explicitly confirmed it (the shell's `format` command) -- never from boot.
///
/// Ordered so that an interruption at any point leaves a disk [`probe`]
/// does not accept as formatted:
/// 1. invalidate any existing superblock and flush,
/// 2. clear every slot and flush,
/// 3. publish the new superblock and flush.
///
/// # Safety
/// Same contract as [`Hba::write_sector`]/[`Hba::flush_cache`].
pub unsafe fn provision(hba: &Hba, total_sectors: u64) -> Result<(), &'static str> {
    if hba.native_sector_size() != 512 {
        return Err("save_format: formatting needs 512-byte native blocks; 4Kn disks are read-only");
    }
    let region = unsafe { Region::discover(hba, total_sectors)? }
        .ok_or("save_format: partitioned disk has no WISP save partition; format refused")?;
    let hba = &region;
    let zero = [0u8; SECTOR];
    unsafe {
        write_region_sector(hba, SUPERBLOCK_LBA, &zero)?;
        hba.flush_cache()?;
    }

    // Every sector of every slot, not just the headers: a fresh store
    // holds no bytes of whatever the disk held before.
    for lba in SLOTS_START_LBA..REGION_SECTORS {
        unsafe { write_region_sector(hba, lba, &zero)? };
    }
    unsafe { hba.flush_cache()? };

    let mut new_sb = [0u8; SECTOR];
    new_sb[0..9].copy_from_slice(MAGIC);
    new_sb[9] = FORMAT_VERSION;
    new_sb[10..12].copy_from_slice(&SLOT_COUNT.to_le_bytes());
    new_sb[12..16].copy_from_slice(&SLOT_SIZE.to_le_bytes());
    unsafe {
        write_region_sector(hba, SUPERBLOCK_LBA, &new_sb)?;
        hba.flush_cache()?;
    }
    Ok(())
}

/// One slot's decoded header.
#[derive(Clone, Copy, PartialEq, Eq)]
struct Header {
    /// A version-1 copy: generation 0, checksum over the value only.
    legacy: bool,
    /// A version-3 multi-slot chain head (`STATE_CHAIN_HEAD`).
    chained: bool,
    first_data_slot: u16,
    key: [u8; KEY_MAX_LEN],
    len: usize,
    checksum: u32,
    generation: u64,
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct ChainDataHeader {
    head_generation: u64,
    chunk_index: u16,
    next_slot: u16,
    chunk_len: u16,
}

impl Header {
    /// Where the value starts in the slot, and the most a single slot holds.
    fn value_start_and_max(&self) -> (usize, usize) {
        if self.legacy {
            (LEGACY_HEADER_LEN, LEGACY_VALUE_MAX_LEN)
        } else if self.chained {
            (CHAIN_HEAD_HEADER_LEN, MAX_CHAINED_VALUE_LEN)
        } else {
            (SLOT_HEADER_LEN, VALUE_MAX_LEN)
        }
    }

    /// The slot bytes the copy occupies in the head slot, if its recorded length fits.
    fn used_len(&self) -> Result<usize, &'static str> {
        let (start, max) = self.value_start_and_max();
        if self.len > max || (self.chained && self.len <= VALUE_MAX_LEN) {
            return Err(CORRUPT_LENGTH);
        }
        let inline = if self.chained {
            self.len.min(CHAIN_HEAD_PAYLOAD_MAX)
        } else {
            self.len
        };
        Ok(start + inline)
    }

    fn key_string(&self) -> String {
        let end = self.key.iter().position(|&b| b == 0).unwrap_or(KEY_MAX_LEN);
        String::from_utf8_lossy(&self.key[..end]).into_owned()
    }
}

/// What a slot's first sector says about it.
#[derive(Clone, Copy)]
enum Slot {
    Free,
    Copy(Header),
    ChainData(ChainDataHeader),
    /// A state byte this driver does not know. Only corruption produces
    /// one; the slot belongs to no key and is never written, so whatever
    /// it holds is left for a human to look at.
    Unknown,
}

fn parse_header(sector: &[u8; SECTOR]) -> Slot {
    let (legacy, chained) = match sector[0] {
        STATE_FREE => return Slot::Free,
        STATE_LEGACY => (true, false),
        STATE_COMMITTED => (false, false),
        STATE_CHAIN_HEAD => (false, true),
        STATE_CHAIN_DATA => {
            let head_generation =
                u64::from_le_bytes(sector[1..9].try_into().expect("8 bytes"));
            let chunk_index =
                u16::from_le_bytes(sector[9..11].try_into().expect("2 bytes"));
            let next_slot =
                u16::from_le_bytes(sector[11..13].try_into().expect("2 bytes"));
            let chunk_len =
                u16::from_le_bytes(sector[13..15].try_into().expect("2 bytes"));
            return Slot::ChainData(ChainDataHeader {
                head_generation,
                chunk_index,
                next_slot,
                chunk_len,
            });
        }
        _ => return Slot::Unknown,
    };
    let mut key = [0u8; KEY_MAX_LEN];
    key.copy_from_slice(&sector[OFF_KEY..OFF_LEN]);
    let len = u32::from_le_bytes(sector[OFF_LEN..OFF_CHECKSUM].try_into().expect("4 bytes")) as usize;
    let checksum = u32::from_le_bytes(sector[OFF_CHECKSUM..OFF_GENERATION].try_into().expect("4 bytes"));
    let generation = if legacy {
        0
    } else {
        u64::from_le_bytes(sector[OFF_GENERATION..SLOT_HEADER_LEN].try_into().expect("8 bytes"))
    };
    let first_data_slot = if chained {
        u16::from_le_bytes(sector[SLOT_HEADER_LEN..CHAIN_HEAD_HEADER_LEN].try_into().expect("2 bytes"))
    } else {
        CHAIN_NONE
    };
    Slot::Copy(Header {
        legacy,
        chained,
        first_data_slot,
        key,
        len,
        checksum,
        generation,
    })
}

/// Every slot's header, read from the first sector of each: one sector per
/// slot, never the whole slot.
struct Scan {
    slots: Vec<Slot>,
}

impl Scan {
    /// # Safety
    /// Same contract as [`Hba::read_sector`].
    unsafe fn read(hba: &Region<'_>) -> Result<Self, &'static str> {
        let mut slots = Vec::with_capacity(SLOT_COUNT as usize);
        let mut sector = [0u8; SECTOR];
        for slot_index in 0..SLOT_COUNT {
            unsafe { read_region_sector(hba, slot_lba(slot_index), &mut sector)? };
            slots.push(parse_header(&sector));
        }
        Ok(Self { slots })
    }

    /// The copy readers use for `key`: highest generation, ties broken by
    /// the lowest slot index. `None` if the key has no copy at all.
    fn winner(&self, key: &[u8; KEY_MAX_LEN]) -> Option<(u16, Header)> {
        let mut best: Option<(u16, Header)> = None;
        for (index, slot) in self.slots.iter().enumerate() {
            if let Slot::Copy(h) = slot {
                if h.key == *key && best.is_none_or(|(_, b)| h.generation > b.generation) {
                    best = Some((index as u16, *h));
                }
            }
        }
        best
    }

    /// Every copy that is not its key's header-selected winner: invisible
    /// to readers, but reclaimable only after that winner is validated.
    fn stale(&self) -> Vec<u16> {
        let mut out = Vec::new();
        for (index, slot) in self.slots.iter().enumerate() {
            if let Slot::Copy(h) = slot {
                if self.winner(&h.key).is_some_and(|(w, _)| w as usize != index) {
                    out.push(index as u16);
                }
            }
        }
        out
    }

    fn first_free(&self) -> Option<u16> {
        self.slots.iter().position(|s| matches!(s, Slot::Free)).map(|i| i as u16)
    }

    fn free_slots(&self, count: usize) -> Option<Vec<u16>> {
        let mut out = Vec::with_capacity(count);
        for (i, s) in self.slots.iter().enumerate() {
            if matches!(s, Slot::Free) {
                out.push(i as u16);
                if out.len() == count {
                    return Some(out);
                }
            }
        }
        None
    }

    fn has_chain_data(&self) -> bool {
        self.slots.iter().any(|s| matches!(s, Slot::ChainData(_)))
    }

    /// Collect the continuation slot indices linked from `h.first_data_slot`
    /// using the already-scanned headers when they match `h.generation`.
    fn chain_slots_for(&self, h: &Header) -> Vec<u16> {
        if !h.chained {
            return Vec::new();
        }
        let mut out = Vec::new();
        let mut cur = h.first_data_slot;
        let mut expected_chunk = 0u16;
        while cur != CHAIN_NONE && (cur as usize) < self.slots.len() && out.len() < self.slots.len() {
            if out.contains(&cur) {
                break;
            }
            if let Slot::ChainData(cd) = self.slots[cur as usize] {
                if cd.head_generation == h.generation && cd.chunk_index == expected_chunk {
                    out.push(cur);
                    cur = cd.next_slot;
                    expected_chunk = expected_chunk.saturating_add(1);
                    continue;
                }
            }
            break;
        }
        out
    }

    fn occupied(&self) -> usize {
        self.slots.iter().filter(|s| !matches!(s, Slot::Free)).count()
    }
}

/// Mark `slot_index` free: its first sector becomes all zeros. The rest of
/// the slot is left as it was; nothing reads past a free header.
///
/// # Safety
/// Same contract as [`Hba::write_sector`].
unsafe fn free_slot(hba: &Region<'_>, slot_index: u16) -> Result<(), &'static str> {
    unsafe { write_region_sector(hba, slot_lba(slot_index), &[0u8; SECTOR]) }
}

/// Validate winners before freeing their stale copies or orphaned chain data
/// slots. A forged generation must never authorize deletion of the only intact
/// copy. An explicit replacement can repair a corrupt winner, but retains that
/// key's copies until the new value commits. Transport errors always propagate.
///
/// # Safety
/// Same contract as [`Hba::write_sector`]/[`Hba::flush_cache`].
unsafe fn reclaim_stale(
    hba: &Region<'_>,
    scan: &mut Scan,
    replacing: Option<&[u8; KEY_MAX_LEN]>,
) -> Result<(), &'static str> {
    let stale = scan.stale();
    let has_chain_data = scan.has_chain_data();
    if stale.is_empty() && !has_chain_data {
        return Ok(());
    }
    let mut verified = [None; SLOT_COUNT as usize];
    let mut reclaim = Vec::new();
    for &slot_index in &stale {
        let Slot::Copy(header) = scan.slots[slot_index as usize] else {
            continue;
        };
        let (winner, winner_header) = scan.winner(&header.key)
            .ok_or("save_format: internal error -- stale copy has no winner")?;
        let safe = match verified[winner as usize] {
            Some(safe) => safe,
            None => {
                let safe = match unsafe { read_copy(hba, winner, &winner_header) } {
                    Ok(_) => true,
                    Err(CORRUPT_CHECKSUM | CORRUPT_LENGTH) if replacing == Some(&winner_header.key) => false,
                    Err(error) => return Err(error),
                };
                verified[winner as usize] = Some(safe);
                safe
            }
        };
        if safe {
            reclaim.push(slot_index);
            for data_slot in scan.chain_slots_for(&header) {
                if !reclaim.contains(&data_slot) {
                    reclaim.push(data_slot);
                }
            }
        }
    }

    let mut reclaim_data = Vec::new();
    if has_chain_data {
        let mut active_chain_slots = [false; SLOT_COUNT as usize];
        for index in 0..scan.slots.len() {
            let Slot::Copy(h) = scan.slots[index] else {
                continue;
            };
            if !h.chained {
                continue;
            }
            let is_winner = scan.winner(&h.key).is_some_and(|(w, _)| w as usize == index);
            if !is_winner {
                if !reclaim.contains(&(index as u16)) {
                    for s in scan.chain_slots_for(&h) {
                        active_chain_slots[s as usize] = true;
                    }
                }
                continue;
            }
            let safe = match verified[index] {
                Some(safe) => safe,
                None => {
                    let safe = match unsafe { read_copy(hba, index as u16, &h) } {
                        Ok(_) => true,
                        Err(CORRUPT_CHECKSUM | CORRUPT_LENGTH) if replacing == Some(&h.key) => false,
                        Err(error) => return Err(error),
                    };
                    verified[index] = Some(safe);
                    safe
                }
            };
            let _ = safe;
            for s in scan.chain_slots_for(&h) {
                active_chain_slots[s as usize] = true;
            }
        }
        for (idx, slot) in scan.slots.iter().enumerate() {
            if matches!(slot, Slot::ChainData(_)) && !active_chain_slots[idx] {
                let slot_u16 = idx as u16;
                if !reclaim.contains(&slot_u16) && !reclaim_data.contains(&slot_u16) {
                    reclaim_data.push(slot_u16);
                }
            }
        }
    }

    // Finish every validation before the first write so a corrupt winner
    // discovered later cannot leave a partially reclaimed store.
    if reclaim.is_empty() && reclaim_data.is_empty() {
        return Ok(());
    }
    if !reclaim.is_empty() {
        for slot_index in reclaim {
            unsafe { free_slot(hba, slot_index)? };
            scan.slots[slot_index as usize] = Slot::Free;
        }
        unsafe { hba.flush_cache()? };
    }
    if !reclaim_data.is_empty() {
        for slot_index in reclaim_data {
            unsafe { free_slot(hba, slot_index)? };
            scan.slots[slot_index as usize] = Slot::Free;
        }
        unsafe { hba.flush_cache()? };
    }
    Ok(())
}

/// Read the sectors of `slot_index` that the copy described by `h` covers
/// (including continuation slots when `h.chained` is true) and verify its
/// checksum. Returns the value's bytes.
///
/// # Safety
/// Same contract as [`Hba::read_sector`].
unsafe fn read_copy(hba: &Region<'_>, slot_index: u16, h: &Header) -> Result<Vec<u8>, &'static str> {
    let used = h.used_len()?;
    let mut raw = Vec::new();
    raw.try_reserve_exact(used.div_ceil(SECTOR) * SECTOR)
        .map_err(|_| "save_format: out of memory reading a slot")?;
    let mut sector = [0u8; SECTOR];
    for i in 0..used.div_ceil(SECTOR) {
        unsafe { read_region_sector(hba, slot_lba(slot_index) + i as u64, &mut sector)? };
        raw.extend_from_slice(&sector);
    }
    // The first sector was read again here, after the scan: what it says
    // now is what the checksum below is checked against.
    let Slot::Copy(now) = parse_header(raw[..SECTOR].try_into().expect("one sector")) else {
        return Err("save_format: slot changed while it was being read");
    };
    if now != *h {
        return Err("save_format: slot changed while it was being read");
    }
    let (start, _) = h.value_start_and_max();
    if !h.chained {
        let checksum = if h.legacy {
            fnv1a32(&[&raw[start..used]])
        } else {
            fnv1a32(&[&raw[..OFF_CHECKSUM], &raw[OFF_GENERATION..used]])
        };
        if checksum != h.checksum {
            return Err(CORRUPT_CHECKSUM);
        }
        raw.truncate(used);
        raw.drain(..start);
        return Ok(raw);
    }

    let mut payload = Vec::new();
    payload
        .try_reserve_exact(h.len)
        .map_err(|_| "save_format: out of memory reading a multi-slot value")?;
    payload.extend_from_slice(&raw[start..used]);

    let mut cur_slot = h.first_data_slot;
    let mut expected_chunk = 0u16;
    let mut visited = [false; SLOT_COUNT as usize];
    visited[slot_index as usize] = true;

    while cur_slot != CHAIN_NONE {
        if cur_slot >= SLOT_COUNT || visited[cur_slot as usize] {
            return Err(CORRUPT_LENGTH);
        }
        visited[cur_slot as usize] = true;

        let mut sec0 = [0u8; SECTOR];
        unsafe { read_region_sector(hba, slot_lba(cur_slot), &mut sec0)? };
        let Slot::ChainData(cd) = parse_header(&sec0) else {
            return Err(CORRUPT_CHECKSUM);
        };
        if cd.head_generation != h.generation || cd.chunk_index != expected_chunk {
            return Err(CORRUPT_CHECKSUM);
        }
        let chunk_len = cd.chunk_len as usize;
        if chunk_len == 0
            || chunk_len > CHAIN_DATA_PAYLOAD_MAX
            || payload.len().saturating_add(chunk_len) > h.len
        {
            return Err(CORRUPT_LENGTH);
        }
        let data_used = CHAIN_DATA_HEADER_LEN + chunk_len;
        let sectors = data_used.div_ceil(SECTOR);
        let mut slot_raw = [0u8; SLOT_SIZE as usize];
        slot_raw[..SECTOR].copy_from_slice(&sec0);
        for i in 1..sectors {
            unsafe {
                read_region_sector(
                    hba,
                    slot_lba(cur_slot) + i as u64,
                    (&mut slot_raw[i * SECTOR..(i + 1) * SECTOR]).try_into().expect("one sector"),
                )?;
            }
        }
        payload.extend_from_slice(&slot_raw[CHAIN_DATA_HEADER_LEN..data_used]);
        cur_slot = cd.next_slot;
        expected_chunk = expected_chunk.saturating_add(1);
    }

    if payload.len() != h.len {
        return Err(CORRUPT_LENGTH);
    }
    let checksum = fnv1a32(&[
        &raw[..OFF_CHECKSUM],
        &raw[OFF_GENERATION..CHAIN_HEAD_HEADER_LEN],
        &payload,
    ]);
    if checksum != h.checksum {
        return Err(CORRUPT_CHECKSUM);
    }
    Ok(payload)
}

/// Write a complete copy of `key` = `encoded` at `generation` into the free
/// slot `slot_index`, in commit order: value sectors past the first, flush,
/// then the first sector (header and checksum), flush. When this returns
/// `Ok`, the copy is durable; until its last flush has completed, the slot
/// still reads as free (see the module doc comment).
///
/// # Safety
/// Same contract as [`Hba::write_sector`]/[`Hba::flush_cache`].
unsafe fn write_copy(
    hba: &Region<'_>,
    slot_index: u16,
    key: &str,
    encoded: &[u8],
    generation: u64,
) -> Result<(), &'static str> {
    let used = SLOT_HEADER_LEN + encoded.len();
    let mut raw = [0u8; SLOT_SIZE as usize];
    raw[0] = STATE_COMMITTED;
    raw[OFF_KEY..OFF_LEN].copy_from_slice(&key_field(key));
    raw[OFF_LEN..OFF_CHECKSUM].copy_from_slice(&(encoded.len() as u32).to_le_bytes());
    raw[OFF_GENERATION..SLOT_HEADER_LEN].copy_from_slice(&generation.to_le_bytes());
    raw[SLOT_HEADER_LEN..used].copy_from_slice(encoded);
    let checksum = fnv1a32(&[&raw[..OFF_CHECKSUM], &raw[OFF_GENERATION..used]]);
    raw[OFF_CHECKSUM..OFF_GENERATION].copy_from_slice(&checksum.to_le_bytes());

    let sectors = used.div_ceil(SECTOR);
    let base = slot_lba(slot_index);
    let sector_at = |i: usize| -> [u8; SECTOR] { raw[i * SECTOR..(i + 1) * SECTOR].try_into().expect("one sector") };
    // A value that fits in the first sector needs no first barrier: there
    // is nothing for the header to get ahead of.
    if sectors > 1 {
        for i in 1..sectors {
            unsafe { write_region_sector(hba, base + i as u64, &sector_at(i))? };
        }
        unsafe { hba.flush_cache()? };
    }
    unsafe {
        write_region_sector(hba, base, &sector_at(0))?;
        hba.flush_cache()
    }
}

/// Write a multi-slot copy of `key` = `encoded` (`encoded.len() > VALUE_MAX_LEN`)
/// at `generation` across `head_slot` and `data_slots` in crash-atomic order:
/// 1. Write all `STATE_CHAIN_DATA` continuation slots (`data_slots`) and
///    sectors `1..8` of `head_slot`, then `flush_cache()`. Until `head_slot`
///    sector `0` is written, `head_slot` remains `STATE_FREE` and the
///    continuation slots are unreferenced orphans (reclaimed on recovery).
/// 2. Write sector `0` of `head_slot` (`STATE_CHAIN_HEAD = 3`, whole-chain
///    checksum, `generation`, `first_data_slot`), then `flush_cache()`.
///
/// # Safety
/// Same contract as [`Hba::write_sector`]/[`Hba::flush_cache`].
unsafe fn write_chained_copy(
    hba: &Region<'_>,
    head_slot: u16,
    data_slots: &[u16],
    key: &str,
    encoded: &[u8],
    generation: u64,
) -> Result<(), &'static str> {
    if encoded.len() <= VALUE_MAX_LEN || data_slots.is_empty() {
        return Err("save_format: internal error -- invalid multi-slot write parameters");
    }
    let rem = &encoded[CHAIN_HEAD_PAYLOAD_MAX..];
    for (chunk_idx, &data_slot) in data_slots.iter().enumerate() {
        let start = chunk_idx * CHAIN_DATA_PAYLOAD_MAX;
        let end = ((chunk_idx + 1) * CHAIN_DATA_PAYLOAD_MAX).min(rem.len());
        let chunk = &rem[start..end];
        let next_slot = if chunk_idx + 1 < data_slots.len() {
            data_slots[chunk_idx + 1]
        } else {
            CHAIN_NONE
        };
        let mut slot_raw = [0u8; SLOT_SIZE as usize];
        slot_raw[0] = STATE_CHAIN_DATA;
        slot_raw[1..9].copy_from_slice(&generation.to_le_bytes());
        slot_raw[9..11].copy_from_slice(&(chunk_idx as u16).to_le_bytes());
        slot_raw[11..13].copy_from_slice(&next_slot.to_le_bytes());
        slot_raw[13..15].copy_from_slice(&(chunk.len() as u16).to_le_bytes());
        let used = CHAIN_DATA_HEADER_LEN + chunk.len();
        slot_raw[CHAIN_DATA_HEADER_LEN..used].copy_from_slice(chunk);

        let sectors = used.div_ceil(SECTOR);
        let base = slot_lba(data_slot);
        for i in 0..sectors {
            let sec: [u8; SECTOR] = slot_raw[i * SECTOR..(i + 1) * SECTOR]
                .try_into()
                .expect("one sector");
            unsafe { write_region_sector(hba, base + i as u64, &sec)? };
        }
    }

    let mut head_raw = [0u8; SLOT_SIZE as usize];
    head_raw[0] = STATE_CHAIN_HEAD;
    head_raw[OFF_KEY..OFF_LEN].copy_from_slice(&key_field(key));
    head_raw[OFF_LEN..OFF_CHECKSUM].copy_from_slice(&(encoded.len() as u32).to_le_bytes());
    head_raw[OFF_GENERATION..SLOT_HEADER_LEN].copy_from_slice(&generation.to_le_bytes());
    head_raw[SLOT_HEADER_LEN..CHAIN_HEAD_HEADER_LEN]
        .copy_from_slice(&data_slots[0].to_le_bytes());
    head_raw[CHAIN_HEAD_HEADER_LEN..]
        .copy_from_slice(&encoded[..CHAIN_HEAD_PAYLOAD_MAX]);
    let checksum = fnv1a32(&[
        &head_raw[..OFF_CHECKSUM],
        &head_raw[OFF_GENERATION..CHAIN_HEAD_HEADER_LEN],
        encoded,
    ]);
    head_raw[OFF_CHECKSUM..OFF_GENERATION].copy_from_slice(&checksum.to_le_bytes());

    let head_base = slot_lba(head_slot);
    for i in 1..(SECTORS_PER_SLOT as usize) {
        let sec: [u8; SECTOR] = head_raw[i * SECTOR..(i + 1) * SECTOR]
            .try_into()
            .expect("one sector");
        unsafe { write_region_sector(hba, head_base + i as u64, &sec)? };
    }
    unsafe { hba.flush_cache()? };

    let sec0: [u8; SECTOR] = head_raw[..SECTOR].try_into().expect("one sector");
    unsafe {
        write_region_sector(hba, head_base, &sec0)?;
        hba.flush_cache()
    }
}

/// Make sure the superblock says [`FORMAT_VERSION`] before any copy in
/// that format is written, upgrading a [`LEGACY_VERSION`] store in place
/// (one sector, flushed). Also re-checks that the superblock is still the
/// one that was mounted.
///
/// # Safety
/// Same contract as [`Hba::read_sector`]/[`Hba::write_sector`]/
/// [`Hba::flush_cache`].
unsafe fn ensure_current_version(hba: &Region<'_>) -> Result<(), &'static str> {
    let mut sb = [0u8; SECTOR];
    unsafe { read_region_sector(hba, SUPERBLOCK_LBA, &mut sb)? };
    match superblock_version(&sb) {
        Ok(FORMAT_VERSION) => Ok(()),
        Ok(_) => {
            sb[9] = FORMAT_VERSION;
            unsafe {
                write_region_sector(hba, SUPERBLOCK_LBA, &sb)?;
                hba.flush_cache()
            }
        }
        Err(_) => Err("save_format: the superblock is no longer valid -- the disk changed since it was mounted"),
    }
}

/// Store `value` under `key`, replacing any value already saved under it.
/// Crash-atomic: `Ok` means the new value is durable, and a crash or power
/// loss at any point before that leaves `load(key)` returning either the
/// old value or the new one (see the module doc comment for why). Saving
/// [`Value::Nil`] deletes the key instead.
///
/// Returns `Err` -- never silently truncates -- if `key` is empty, longer
/// than 32 bytes or contains a NUL byte, `value`'s serialized encoding is
/// longer than [`VALUE_MAX_LEN`] bytes, the key is new and [`MAX_KEYS`]
/// keys are already stored, or the device reports an error.
///
/// # Safety
/// Same contract as [`Hba::read_sector`]/[`Hba::write_sector`]. The caller
/// must have confirmed [`probe`] returned [`Probe::Formatted`] (or run
/// [`provision`]) on this same `Hba`.
unsafe fn save_in(hba: &Region<'_>, key: &str, value: &Value) -> Result<(), &'static str> {
    validate_key(key)?;
    if matches!(value, Value::Nil) {
        return unsafe { delete_in(hba, key) }.map(|_| ());
    }
    let encoded = encode(value)?;
    let field = key_field(key);

    // Freeing stale copies is valid in either version (a free slot is the
    // same in both), so it may run before the upgrade; the upgrade itself
    // waits until the save is known to be possible, so a save that fails
    // for lack of room leaves a version-1 store a version-1 store.
    let mut scan = unsafe { Scan::read(hba)? };
    unsafe { reclaim_stale(hba, &mut scan, Some(&field))? };

    let old = scan.winner(&field);
    if old.is_none() && scan.occupied() >= MAX_KEYS {
        return Err("save_format: the save disk is full (63 keys is the limit) -- delete a key first");
    }
    let Some(target) = scan.first_free() else {
        // Only reachable on a version-1 store that already held 64 keys:
        // nothing this driver writes ever fills the last slot.
        return Err("save_format: no free slot to write the new value into (64 keys stored; the limit is 63) -- delete a key first");
    };
    let generation = match old {
        Some((_, h)) => h
            .generation
            .checked_add(1)
            .ok_or("save_format: corrupt slot -- generation counter exhausted")?,
        None => 1,
    };

    unsafe {
        ensure_current_version(hba)?;
        write_copy(hba, target, key, &encoded, generation)?;
    }

    // Committed. Freeing the old copy is housekeeping: if it fails, or
    // never reaches the media, the old copy is merely stale -- invisible
    // to readers, and reclaimed by the next save or delete. Reporting an
    // error here would claim a save failed that in fact took effect.
    if old.is_some() {
        // Include any copies preserved while explicitly replacing a corrupt
        // winner. The new target was free in this scan and cannot be retired.
        let _ = (|| -> Result<(), &'static str> {
            let mut data_to_free = Vec::new();
            for (slot, copy) in scan.slots.iter().enumerate() {
                if let Slot::Copy(header) = copy {
                    if header.key == field {
                        unsafe { free_slot(hba, slot as u16)? };
                        if header.chained {
                            data_to_free.extend(scan.chain_slots_for(header));
                        }
                    }
                }
            }
            unsafe { hba.flush_cache()? };
            if !data_to_free.is_empty() {
                for data_slot in data_to_free {
                    unsafe { free_slot(hba, data_slot)? };
                }
                unsafe { hba.flush_cache()? };
            }
            Ok(())
        })();
    }
    Ok(())
}

/// Store `value` under `key`, allowing multi-slot chained storage up to
/// [`MAX_CHAINED_VALUE_LEN`] bytes (`STATE_CHAIN_HEAD` + `STATE_CHAIN_DATA`)
/// when the encoded value exceeds [`VALUE_MAX_LEN`], or single-slot
/// `STATE_COMMITTED` when it fits in one slot.
///
/// # Safety
/// Same contract as [`save_in`].
unsafe fn save_chained_in(
    hba: &Region<'_>,
    key: &str,
    value: &Value,
) -> Result<(), &'static str> {
    validate_key(key)?;
    if matches!(value, Value::Nil) {
        return unsafe { delete_in(hba, key) }.map(|_| ());
    }
    let encoded = encode_chained(value)?;
    if encoded.len() <= VALUE_MAX_LEN {
        return unsafe { save_in(hba, key, value) };
    }
    let field = key_field(key);
    let mut scan = unsafe { Scan::read(hba)? };
    unsafe { reclaim_stale(hba, &mut scan, Some(&field))? };

    let old = scan.winner(&field);
    if old.is_none() && scan.occupied() >= MAX_KEYS {
        return Err("save_format: the save disk is full (63 keys is the limit) -- delete a key first");
    }
    let rem_len = encoded.len() - CHAIN_HEAD_PAYLOAD_MAX;
    let data_slots_needed = rem_len.div_ceil(CHAIN_DATA_PAYLOAD_MAX);
    let total_slots_needed = 1 + data_slots_needed;
    let Some(free) = scan.free_slots(total_slots_needed) else {
        return Err("save_format: not enough free slots on the save disk for this multi-slot value -- delete a key first");
    };
    let head_slot = free[0];
    let data_slots = &free[1..];
    let generation = match old {
        Some((_, h)) => h
            .generation
            .checked_add(1)
            .ok_or("save_format: corrupt slot -- generation counter exhausted")?,
        None => 1,
    };

    unsafe {
        ensure_current_version(hba)?;
        write_chained_copy(hba, head_slot, data_slots, key, &encoded, generation)?;
    }

    if old.is_some() {
        let _ = (|| -> Result<(), &'static str> {
            let mut data_to_free = Vec::new();
            for (slot, copy) in scan.slots.iter().enumerate() {
                if let Slot::Copy(header) = copy {
                    if header.key == field {
                        unsafe { free_slot(hba, slot as u16)? };
                        if header.chained {
                            data_to_free.extend(scan.chain_slots_for(header));
                        }
                    }
                }
            }
            unsafe { hba.flush_cache()? };
            if !data_to_free.is_empty() {
                for data_slot in data_to_free {
                    unsafe { free_slot(hba, data_slot)? };
                }
                unsafe { hba.flush_cache()? };
            }
            Ok(())
        })();
    }
    Ok(())
}

/// Delete `key`, freeing its slot (and any chained continuation slots).
/// `Ok(true)` if it was stored, `Ok(false)` if there was nothing to delete.
/// Crash-atomic in the same sense as [`save`]: stale copies are freed and
/// flushed first, so a crash leaves either the current value or nothing --
/// never an older generation.
///
/// # Safety
/// Same contract as [`save`].
unsafe fn delete_in(hba: &Region<'_>, key: &str) -> Result<bool, &'static str> {
    validate_key(key)?;
    let mut scan = unsafe { Scan::read(hba)? };
    unsafe { reclaim_stale(hba, &mut scan, None)? };
    match scan.winner(&key_field(key)) {
        None => Ok(false),
        Some((slot_index, header)) => {
            unsafe {
                free_slot(hba, slot_index)?;
                hba.flush_cache()?;
                if header.chained {
                    for data_slot in scan.chain_slots_for(&header) {
                        free_slot(hba, data_slot)?;
                    }
                    hba.flush_cache()?;
                }
            }
            Ok(true)
        }
    }
}

/// Look up `key`, returning `Ok(None)` if it was never saved (a first-class
/// outcome, not an error) -- Build Order Step 8 of the Phase 2 plan.
///
/// Returns `Err` if `key` is invalid (matching [`save`]'s own validation --
/// an over-length key is a caller mistake worth reporting clearly, not
/// silent "not found"), or if the key's winning copy fails its checksum,
/// has an implausible stored length, or decodes to malformed value data --
/// real corruption, never silently swallowed into a wrong answer, and
/// never papered over with an older copy.
///
/// # Safety
/// Same contract as [`Hba::read_sector`]. The caller must have already
/// confirmed [`probe`] returned [`Probe::Formatted`] (or run
/// [`provision`]) on this same `Hba`.
unsafe fn load_in(hba: &Region<'_>, key: &str) -> Result<Option<Value>, &'static str> {
    validate_key(key)?;
    let scan = unsafe { Scan::read(hba)? };
    let Some((slot_index, header)) = scan.winner(&key_field(key)) else {
        return Ok(None);
    };
    let value_bytes = unsafe { read_copy(hba, slot_index, &header)? };
    let (value, consumed) = deserialize_value(&value_bytes, 0)?;
    if consumed != value_bytes.len() {
        return Err("save_format: corrupt slot -- trailing bytes after a fully-decoded value");
    }
    Ok(Some(value))
}

/// Every stored key with the length of its encoded value, in slot order.
/// Reads headers only; a key whose value is corrupt is still listed (the
/// corruption is reported when it is loaded).
///
/// # Safety
/// Same contract as [`load`].
unsafe fn keys_in(hba: &Region<'_>) -> Result<Vec<(String, usize)>, &'static str> {
    let scan = unsafe { Scan::read(hba)? };
    let mut out = Vec::new();
    for (index, slot) in scan.slots.iter().enumerate() {
        if let Slot::Copy(h) = slot {
            if scan.winner(&h.key).is_some_and(|(w, _)| w as usize == index) {
                out.push((h.key_string(), h.len));
            }
        }
    }
    Ok(out)
}

/// Store a value in the discovered raw store or WISP GPT save partition.
///
/// # Safety
/// The HBA must be identified and the selected region must have been mounted.
pub unsafe fn save(hba: &Hba, key: &str, value: &Value) -> Result<(), &'static str> {
    unsafe { save_in(&Region::identified(hba)?, key, value) }
}

/// Store a value (up to [`MAX_CHAINED_VALUE_LEN`] bytes, using multi-slot
/// chaining when needed) in the discovered raw store or WISP GPT save partition.
///
/// # Safety
/// Same contract as [`save`].
pub unsafe fn save_chained(hba: &Hba, key: &str, value: &Value) -> Result<(), &'static str> {
    unsafe { save_chained_in(&Region::identified(hba)?, key, value) }
}

/// Delete a saved value from the discovered store.
///
/// # Safety
/// Same contract as [`save`].
pub unsafe fn delete(hba: &Hba, key: &str) -> Result<bool, &'static str> {
    unsafe { delete_in(&Region::identified(hba)?, key) }
}

/// Read a value from the discovered store.
///
/// # Safety
/// Same contract as [`save`].
pub unsafe fn load(hba: &Hba, key: &str) -> Result<Option<Value>, &'static str> {
    unsafe { load_in(&Region::identified(hba)?, key) }
}

/// List the discovered store's saved values.
///
/// # Safety
/// Same contract as [`save`].
pub unsafe fn keys(hba: &Hba) -> Result<Vec<(String, usize)>, &'static str> {
    unsafe { keys_in(&Region::identified(hba)?) }
}

/// The save disk as the shell sees it: the native device, its size, and
/// whether it currently carries a valid store. Owning all three together
/// is what lets `save`/`load` refuse an unformatted disk, and lets the
/// `format` command mount the disk the moment provisioning succeeds.
pub struct SaveDisk {
    hba: Hba,
    total_sectors: u64,
    mounted: bool,
}

impl SaveDisk {
    /// Wrap an identified `hba`. `mounted` must be `true` only if [`probe`]
    /// returned [`Probe::Formatted`] for it.
    pub fn new(hba: Hba, total_sectors: u64, mounted: bool) -> Self {
        Self { hba, total_sectors, mounted }
    }

    pub fn hba(&self) -> &Hba {
        &self.hba
    }

    pub fn total_sectors(&self) -> u64 {
        self.total_sectors
    }

    /// Inclusive physical sector range that an explicit format may overwrite.
    /// Foreign or malformed partition tables are rejected before confirmation.
    pub fn format_range(&self) -> Result<(u64, u64), &'static str> {
        if self.hba.native_sector_size() != 512 {
            return Err("save_format: formatting needs 512-byte native blocks; 4Kn disks are read-only");
        }
        let region = unsafe { Region::discover(&self.hba, self.total_sectors)? }
            .ok_or("save_format: partitioned disk has no WISP save partition; format refused")?;
        Ok((region.start, region.physical_lba(REGION_SECTORS - 1)?))
    }

    pub fn is_mounted(&self) -> bool {
        self.mounted
    }

    fn usable(&self) -> Result<(), &'static str> {
        if self.hba.is_failed() {
            Err("the save disk was taken offline after an unrecoverable error (reboot to retry)")
        } else if !self.mounted {
            Err("the save disk is not formatted for WISP -- run 'format' at the shell prompt to set it up")
        } else {
            Ok(())
        }
    }

    /// [`save`] against this disk, if it is mounted.
    pub fn save(&self, key: &str, value: &Value) -> Result<(), &'static str> {
        self.usable()?;
        // SAFETY: the native driver was initialized and identified, and
        // `mounted` proves a valid superblock -- `save`'s contract.
        unsafe { save(&self.hba, key, value) }
    }

    /// [`save_chained`] against this disk, if it is mounted.
    pub fn save_chained(&self, key: &str, value: &Value) -> Result<(), &'static str> {
        self.usable()?;
        // SAFETY: same reasoning as `SaveDisk::save`.
        unsafe { save_chained(&self.hba, key, value) }
    }

    /// [`load`] against this disk, if it is mounted.
    pub fn load(&self, key: &str) -> Result<Option<Value>, &'static str> {
        self.usable()?;
        // SAFETY: same reasoning as `SaveDisk::save`.
        unsafe { load(&self.hba, key) }
    }

    /// [`delete`] against this disk, if it is mounted.
    pub fn delete(&self, key: &str) -> Result<bool, &'static str> {
        self.usable()?;
        // SAFETY: same reasoning as `SaveDisk::save`.
        unsafe { delete(&self.hba, key) }
    }

    /// [`keys`] against this disk, if it is mounted.
    pub fn keys(&self) -> Result<Vec<(String, usize)>, &'static str> {
        self.usable()?;
        // SAFETY: same reasoning as `SaveDisk::save`.
        unsafe { keys(&self.hba) }
    }

    /// [`provision`] this disk and mount it on success. The caller is
    /// responsible for having obtained the user's explicit consent.
    pub fn format(&mut self) -> Result<(), &'static str> {
        if self.hba.is_failed() {
            return Err("the save disk was taken offline after an unrecoverable error (reboot to retry)");
        }
        self.mounted = false;
        // SAFETY: the native driver was initialized and identified.
        unsafe { provision(&self.hba, self.total_sectors)? };
        self.mounted = true;
        Ok(())
    }
}
