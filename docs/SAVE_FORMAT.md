# Save Disk Format (v2)

This is the on-disk layout `src/save_format.rs` reads and writes on the
second, AHCI-attached disk this kernel uses purely for persistent WISP
program state (Phase 2, Part B -- see `CHANGES.md`). It exists because
nothing else in this kernel can survive a reboot: `src/fs.rs` only ever
reads `APPS/*.wisp` from the boot USB stick, once, before Boot Services
are exited, and that disk is deliberately never written to (see
`CHANGES.md`'s vvfat-corruption postmortem). `save()`/`load()` -- both
the shell commands and the WISP builtins -- plus the shell's `keys` and
`delete` are the only way anything in this project ever reaches this
format, and this document is the spec `src/save_format.rs`'s own module
doc comment implements.

If you only need to *use* persistence from a WISP script, you don't
need this document -- see `save(key, value)`/`load(key)` in
`docs/WISP_LANGUAGE.md`. This document is for anyone reading or
modifying the raw bytes: `src/save_format.rs` itself, `tests/
ahci_test.py`'s direct on-disk assertions, or a hex-editor session
against a `save.img` file.

Format version 2 replaced version 1 in the release after v1.0.0. The
difference, in one sentence: version 1 overwrote a value in place, so a
power cut in the middle of a save could destroy the old value without
writing the new one; version 2 never overwrites a live value, so a save
interrupted at any point leaves either the old value or the new one.
Version-1 disks keep working -- see "Version 1" below.

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
fragmentation, no partial update that has to be recovered.

## Sector and slot layout

All addressing is in 512-byte logical sectors (LBAs), matching
`src/ahci.rs`'s `SECTOR_SIZE`.

```
LBA 0:              superblock (this one sector only)
LBA 1 .. 1+64*8-1:   64 fixed-size slots, 8 sectors (4096 bytes) each
```

64 slots is deliberately small and fixed rather than derived from disk
size, which keeps every slot's LBA a simple constant calculation: slot
`i`'s first sector is `LBA (1 + i * 8)`. The store holds at most **63
keys**: one slot is always kept free so that an update can be written
copy-on-write (see "Saving" below).

### Superblock (LBA 0, 512 bytes)

| Offset | Size | Field | Value |
| --- | --- | --- | --- |
| 0 | 9 bytes | magic | ASCII `"WISPSAVE1"` |
| 9 | 1 byte | format version | `2` (`1` is still read; see "Version 1") |
| 10 | 2 bytes | slot count | `u16`, little-endian; currently `64` |
| 12 | 4 bytes | slot size | `u32`, little-endian; currently `4096` |
| 16 | 496 bytes | reserved | zeroed |

The magic names the store, not its layout -- that is the version
byte's job -- so it stays `"WISPSAVE1"` in version 2.

At boot the kernel only *reads* LBA 0 (`save_format::probe`). A disk is
mounted for `save`/`load` only if its superblock matches completely:
magic, a version this driver reads (1 or 2), slot count and slot size.
Anything else is left byte-for-byte untouched and persistence stays
unavailable for that boot:

- all zeros -- a blank disk, or an interrupted `format` (see below);
- any other bytes -- a partition table, another filesystem, a damaged
  superblock: treated as someone else's data;
- the WISP magic with any other version or geometry -- see
  "Versioning" below; never silently reinterpreted.

### Provisioning (`format`)

The shell's `format` command is the only thing that ever writes outside
an existing store. It prints the device (AHCI port, model, serial,
size) and the exact range it will overwrite -- LBA 0-512, 256.5 KiB,
including any partition table -- and proceeds only if the user types
`ERASE`. `save_format::provision` then:

1. zeroes LBA 0 (invalidating any previous store) and flushes;
2. zeroes every sector of all 64 slots and flushes;
3. writes a version-2 superblock and flushes.

The superblock is published last, so a power loss or write error at any
earlier point leaves a disk that `probe` reports as blank or foreign --
never one it accepts with stale slot contents.

Everything outside LBA 0-512 is never read or written; every sector
access goes through a range check against that region.

### Slot (4096 bytes = 8 sectors)

A slot is either free or holds one *copy* of one key's value. The whole
header fits in the slot's first sector, so scanning the store reads 64
sectors, not 512.

| Offset | Size | Field |
| --- | --- | --- |
| 0 | 1 byte | state: `0` = free, `2` = a committed copy (`1` = a version-1 copy; any other value is corruption -- such a slot is left alone and belongs to no key) |
| 1 | 32 bytes | key: zero-padded UTF-8, 1 to 32 bytes, no NUL bytes |
| 33 | 4 bytes | value length, `u32` little-endian |
| 37 | 4 bytes | checksum: FNV-1a/32 (see below) |
| 41 | 8 bytes | generation, `u64` little-endian |
| 49 | up to 4047 bytes | the encoded value (see "Value encoding") |

A free slot's first sector is all zeros; nothing past a free header is
ever read, so the rest of a freed slot may still hold old bytes.

### Generations and winners

A key may briefly have more than one copy on disk (see "Saving"). Every
reader -- `load`, `keys`, and the writers' own scans -- uses exactly one
of them, the key's **winner**: the copy with the highest generation,
ties broken by the lowest slot index. Every other copy of the key is
*stale*: invisible, and freed by the next `save` or `delete`.

If the winner fails its checksum, `load` reports corruption. It never
falls back to an older copy: that would silently hand back a value the
program had already replaced.

### Checksum

`fnv1a32`, the standard 32-bit FNV-1a algorithm (offset basis
`0x811c_9dc5`, prime `0x0100_0193`), computed over the slot's bytes
0..37 (state, key, value length) followed by bytes 41..49+length
(generation, then the value) -- everything the copy says, except the
checksum field itself. Not the zero padding after the value.

Covering the header matters as much as covering the value: a bit flip
that raised a stale copy's *generation* would otherwise make it the
winner, and `load` would return an old value as if it were current.

This is not a cryptographic checksum -- it exists only to catch
accidental corruption (a torn write, a stray bit flip), the same "good
enough for its actual job, explicitly not cryptographically secure"
standard `src/shell.rs`'s xorshift64 `rand()` already uses. A mismatch
is a hard error, never a silent "not found" or a silently wrong value.

## Saving: copy-on-write

`save(key, value)` never overwrites a key's winner. It:

1. scans every slot's header; frees every stale copy of *any* key and
   flushes (so a later step can never let an older generation
   resurface);
2. refuses if the key is new and 63 keys are already stored (one slot
   must stay free for the next update);
3. if the store is still version 1, upgrades it (see "Version 1");
4. writes the new copy -- generation = the old winner's + 1, or 1 for a
   new key -- into the first free slot: first the value sectors past the
   slot's first sector, then a flush; then the first sector (header,
   generation, checksum), then a flush. **That flush is the commit
   point**: from here the new copy is the winner;
5. frees the old winner and flushes.

A device's write cache may persist any of the writes not yet flushed,
in any order, when power is lost. Walking through the steps under that
model:

- before step 4's last flush, the new copy's first sector either never
  reached the media or is not yet guaranteed to -- if it did not, the
  slot still reads as free, and the old copy is still the winner; if it
  did, the value sectors it covers were flushed *before* it was even
  written, so it is complete;
- after it, the new copy is complete and has the higher generation, so
  it is the winner whether or not step 5 survived;
- nothing else on the disk is written, apart from freeing copies no
  reader uses.

So after a crash at any point, `load(key)` returns the old value or the
new one -- never an error, never nothing -- and every other key is
unchanged. `save` reports success only once step 4's commit flush has
completed; if only step 5 fails, the save has still taken effect, and
it says so (the old copy is merely stale until the next save).

A value small enough to fit in the first sector beside the header (up
to 463 encoded bytes) skips step 4's first flush: there is nothing for
the header to get ahead of.

`src/save_format.rs`'s host tests check this exhaustively: for every
operation they stop the device after each possible number of writes and
flushes, build the disk image for every subset of the unflushed writes
reaching the media, and check every key on every image.

### Deleting

`delete(key)` -- the shell's `delete` command, or saving `nil` from a
script -- frees stale copies and flushes, then frees the winner and
flushes. A crash leaves the current value or nothing, never an older
one. Saving `nil` deletes because `load` already reports a missing key
as `nil`: the two were never distinguishable, and this way the slot is
freed instead of spent on a value that means "nothing".

## Value encoding

A minimal type-tagged binary format: one tag byte, then a type-specific
payload. All multi-byte integers are little-endian. Unchanged since
version 1.

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
be walked as if it were trusted, well-formed input. An encoding longer
than 4047 bytes (`slot size - header size`) is rejected by `save`
outright, as soon as the encoder reaches that size -- never truncated.

### Worked example

`save("greeting", "hello world")` on a freshly formatted disk lands in
slot 0 (the first free slot) at generation 1, and slot 0's bytes look
like:

```
offset 0:        02                                   (state: committed copy)
offset 1..33:    "greeting" + 24 zero bytes            (key, zero-padded)
offset 33..37:   10 00 00 00                           (value length = 16)
offset 37..41:   45 13 c2 97                           (checksum, LE: FNV-1a/32 of bytes 0..37 and 41..65)
offset 41..49:   01 00 00 00 00 00 00 00               (generation = 1)
offset 49..65:   03 0b 00 00 00 68 65 6c 6c 6f 20 77 6f 72 6c 64
                 ^^ ^^^^^^^^^^^ ^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^^
                 |  |           "hello world" (11 UTF-8 bytes)
                 |  string length = 11 (u32 LE)
                 tag 3 = Str
offset 65..4096: zero padding
```

`save("greeting", "hello again")` next writes generation 2 into slot 1
(the first free slot), commits it, and then zeroes slot 0's first
sector. `tests/ahci_test.py` checks exactly that against a real
(emulated) AHCI disk, and the host tests pin the bytes above.

## Version 1

The v1.0.0 kernel wrote format version 1. Its slots differ from
version 2's in three ways: the state byte of an occupied slot is `1`;
there is no generation field, so the value starts at offset 41 (and may
be up to 4055 bytes); and the checksum covers the value bytes only. It
updated a key by overwriting its slot in place.

This driver mounts version-1 stores and reads them unchanged: a
version-1 copy counts as generation 0. The first `save` to one --
once it is known to have room -- rewrites the superblock's version byte
to `2` and flushes, before any version-2 copy exists; that single
sector is the whole upgrade. Version-1 copies already on the disk stay
where they are and stay readable, and are replaced copy-on-write like
any other copy when their key is next saved. `load`, `keys` and
`delete` never upgrade (a free slot is the same in both versions).

A version-1 kernel shown an upgraded disk reports it as unsupported and
leaves it alone, rather than misreading copies it does not understand.

Version 1 allowed all 64 slots to be used. On such a store there is no
free slot to write an update into, so `save` fails with an explanation
instead of falling back to an unsafe in-place write; deleting one key
makes room.

## Versioning

`FORMAT_VERSION` (currently `2`) is bumped only if this on-disk layout
ever changes in a way that isn't backward-compatible. `probe` treats a
superblock whose magic matches but whose version (or slot geometry) it
does not know as unsupported -- a disk it refuses to guess about, and
never writes to -- rather than attempting to reinterpret its bytes
under its own rules, which risks silently misreading real data as
something else.

- **Version 1** (v1.0.0): in-place updates, value-only checksum.
- **Version 2**: copy-on-write updates with generations; whole-copy
  checksum; header-only scans; 63-key capacity; `nil` deletes.

## What this format deliberately does not do

- **No wear leveling, no garbage collection beyond stale copies.** 64
  fixed slots in 64 fixed locations is the entire allocation story; a
  value moves between slots on every update only because that is what
  copy-on-write needs.
- **Atomic per key, not across keys.** Each `save` or `delete` is
  atomic on its own. A script that saves two related keys can lose
  power between the two saves and find one updated and not the other;
  there are no multi-key transactions. (Put related values in one list
  under one key if they must change together.)
- **Durability needs a flush.** `save` reports success only after its
  commit flush (FLUSH CACHE EXT, or FLUSH CACHE). A device that has a
  write cache but supports neither command makes `save` fail rather
  than report a success it cannot back.
- **Torn sectors.** The analysis above assumes the device writes each
  512-byte sector all-or-nothing -- the property filesystems and
  databases conventionally rely on, though ATA itself does not spell
  out a guarantee for power loss. A sector torn in spite of that is
  caught by the checksum and reported as corruption.
- **No directory structure, no key namespacing.** Every key lives in
  one flat space, shared by every WISP script that ever runs
  `save()`/`load()` on a given machine's save disk.
