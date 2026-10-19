# Save Disk Format (v1)

This is the on-disk layout `src/save_format.rs` reads and writes on the
second, AHCI-attached disk this kernel uses purely for persistent WISP
program state (Phase 2, Part B -- see `CHANGES.md`). It exists because
nothing else in this kernel can survive a reboot: `src/fs.rs` only ever
reads `APPS/*.wisp` from the boot USB stick, once, before Boot Services
are exited, and that disk is deliberately never written to (see
`CHANGES.md`'s vvfat-corruption postmortem). `save()`/`load()` -- both
the shell commands and the WISP builtins -- are the only way anything
in this project ever reaches this format, and this document is the
spec `src/save_format.rs`'s own module doc comment implements.

If you only need to *use* persistence from a WISP script, you don't
need this document -- see `save(key, value)`/`load(key)` in
`docs/WISP_LANGUAGE.md`. This document is for anyone reading or
modifying the raw bytes: `src/save_format.rs` itself, `tests/
ahci_test.py`'s direct on-disk assertions, or a hex-editor session
against a `save.img` file.

## Why not a real filesystem

Writable FAT32 needs synchronized dual FAT tables, cluster-chain
allocation, and long-filename directory entries -- real engineering
effort with no user-visible payoff. Nothing in this project needs a
hierarchical filesystem: `APPS/*.wisp` is served from a read-only RAM
snapshot of the boot stick that this format never touches (it lives
entirely on the second, AHCI-attached disk), and a WISP script's saved
state is just a handful of named values. A flat, fixed-slot key/value
store is the simplest thing that can actually hold that across a
reboot, and it's trivial to reason about: no allocation graph, no
fragmentation, no partial writes to recover from -- every slot is
written in one shot as a whole 4 KiB unit.

## Sector and slot layout

All addressing is in 512-byte logical sectors (LBAs), matching
`src/ahci.rs`'s `SECTOR_SIZE`.

```
LBA 0:              superblock (this one sector only)
LBA 1 .. 1+64*8-1:   64 fixed-size slots, 8 sectors (4096 bytes) each
```

64 slots is deliberately small and fixed rather than derived from disk
size -- far more than any WISP script built so far (or plausibly ever)
needs, and a fixed count keeps every slot's LBA a simple constant
calculation: slot `i`'s first sector is `LBA (1 + i * 8)`.

### Superblock (LBA 0, 512 bytes)

| Offset | Size | Field | Value |
| --- | --- | --- | --- |
| 0 | 9 bytes | magic | ASCII `"WISPSAVE1"` |
| 9 | 1 byte | format version | `1` (see "Versioning" below) |
| 10 | 2 bytes | slot count | `u16`, little-endian; currently `64` |
| 12 | 4 bytes | slot size | `u32`, little-endian; currently `4096` |
| 16 | 496 bytes | reserved | zeroed |

At boot the kernel only *reads* LBA 0 (`save_format::probe`). A disk is
mounted for `save`/`load` only if its superblock matches completely:
magic, version, slot count and slot size. Anything else is left
byte-for-byte untouched and persistence stays unavailable for that boot:

- all zeros -- a blank disk, or an interrupted `format` (see below);
- any other bytes -- a partition table, another filesystem, a damaged
  superblock: treated as someone else's data;
- the WISP magic with a different version or geometry -- see
  "Versioning" below; never silently reinterpreted.

### Provisioning (`format`)

The shell's `format` command is the only thing that ever writes outside
an existing store. It prints the device (AHCI port, model, serial,
size) and the exact range it will overwrite -- LBA 0-512, 256.5 KiB,
including any partition table -- and proceeds only if the user types
`ERASE`. `save_format::provision` then:

1. zeroes LBA 0 (invalidating any previous store) and flushes;
2. zeroes all 64 slots and flushes;
3. writes the superblock and flushes.

The superblock is published last, so a power loss or write error at any
earlier point leaves a disk that `probe` reports as blank or foreign --
never one it accepts with stale slot contents.

Everything outside LBA 0-512 is never read or written; every sector
access goes through a range check against that region.

### Slot (4096 bytes = 8 sectors)

| Offset | Size | Field |
| --- | --- | --- |
| 0 | 1 byte | in-use flag (`0` = free, `1` = occupied) |
| 1 | 32 bytes | key: zero-padded UTF-8, up to 32 bytes |
| 33 | 4 bytes | value length, `u32` little-endian |
| 37 | 4 bytes | checksum: FNV-1a/32 of the value bytes (see below) |
| 41 | up to 4055 bytes | the encoded value (see "Value encoding"), zero-padded to fill the slot |

`save(key, value)` does a full linear scan over all 64 slots: if `key`
already occupies a slot, that slot is overwritten in place; otherwise
the value is written into the first free slot found. `load(key)` scans
the same way and returns the decoded value, or "not found" if no
occupied slot's key matches. At 64 slots this is fast enough that no
index is worth building. `save` never truncates: a key longer than 32
bytes, an empty key, or a value whose encoded form is longer than
4055 bytes (`slot size - header size`) is rejected outright with an
error, and "all 64 slots are already occupied by other keys" is its
own explicit error too.

### Checksum

`fnv1a32`, the standard 32-bit FNV-1a algorithm (offset basis
`0x811c_9dc5`, prime `0x0100_0193`), computed over exactly the encoded
value bytes (not the key, not the length field, not the zero padding
after the value). This is not a cryptographic checksum -- it exists
only to catch accidental corruption (a torn write, a stray bit flip),
the same "good enough for its actual job, explicitly not
cryptographically secure" standard `src/shell.rs`'s xorshift64 `rand()`
already uses. `load` recomputes this checksum against what it reads
back and treats a mismatch as a hard error (never a silent "not
found" or a silently wrong value) -- real corruption is always
reported, never swallowed.

## Value encoding

A minimal type-tagged binary format: one tag byte, then a type-specific
payload. All multi-byte integers are little-endian.

| Tag | WISP type | Payload |
| --- | --- | --- |
| `0` | `Nil` | (none) |
| `1` | `Bool` | 1 byte: `0` or `1` |
| `2` | `Int` | 8 bytes: `i64` |
| `3` | `Str` | 4 bytes (`u32` byte length) + that many UTF-8 bytes |
| `4` | `List` | 4 bytes (`u32` element count) + each element, recursively encoded the same way |

`List` nesting is bounded by `wisp::value::MAX_VALUE_DEPTH` -- the same
recursion guard `wisp::value::Value` already uses for structural
equality and display, reused here rather than inventing a second,
separate depth limit. Decoding rejects a list whose claimed element
count is larger than could possibly fit in the remaining bytes
*before* looping over it, so a corrupt or truncated stored value can't
be walked as if it were trusted, well-formed input.

### Worked example

`save("greeting", "hello world")` on an empty disk lands in slot 0
(the first free slot), and slot 0's bytes look like:

```
offset 0:        01                                   (in-use)
offset 1..33:    "greeting" + 24 zero bytes            (key, zero-padded)
offset 33..37:   10 00 00 00                           (value length = 16)
offset 37..41:   b3 d6 0f 14                            (FNV-1a/32 of the 16 value bytes below, LE)
offset 41..57:   03 0b 00 00 00 68 65 6c 6c 6f 20 77 6f 72 6c 64
                 ^^ ^^^^^^^^^^^ ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
                 |  |           "hello world" (11 UTF-8 bytes)
                 |  string length = 11 (u32 LE)
                 tag 3 = Str
offset 57..4096: zero padding
```

16 total encoded bytes (1 tag + 4 length + 11 payload) is what's
checksummed and what `value length` at offset 33 records -- not the
11-byte string length alone.

## Versioning

`FORMAT_VERSION` (currently `1`) is bumped only if this on-disk layout
ever changes in a way that isn't backward-compatible. `probe` treats a
superblock whose magic matches but whose version (or slot geometry)
doesn't as unsupported -- a disk it refuses to guess about, and never
writes to -- rather than
attempting to reinterpret old-format bytes under new-format rules,
which risks silently misreading real data as something else. There is
currently only one version; this section exists so a future version
bump has an obvious place to record what changed and why.

## What this format deliberately does not do

- **No wear leveling, no fragmentation handling, no garbage
  collection.** 64 fixed slots, always in the same 64 fixed locations,
  is the entire allocation story.
- **Durability, not atomicity.** `save()` reports success only after
  the slot's sectors are written *and* the device's write cache has been
  flushed (FLUSH CACHE EXT, or FLUSH CACHE). A device that has a write
  cache but supports neither command makes `save()` fail rather than
  report a success it cannot back.
- **No transactions or atomicity across multiple keys.** Each `save()`
  call writes one slot's 8 sectors in a simple sequential loop
  (`src/ahci.rs`'s `write_sector`, one at a time, no batching) -- a
  power loss mid-write could in principle leave a single slot with a
  torn write, which the checksum in that same slot exists to detect
  on the next `load()`. There is no journal and no way to recover a
  torn slot's previous contents; this is an accepted, documented
  limitation given this format's actual job (a small hobby/demo
  language's save state), not an oversight.
- **No directory structure, no key namespacing.** Every key lives in
  one flat 64-slot space, shared by every WISP script that ever runs
  `save()`/`load()` on a given machine's save disk.
