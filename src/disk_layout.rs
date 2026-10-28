//! Streaming on-disk installer layout, independent of firmware and drivers.
//!
//! A 64 MiB FAT32 EFI System Partition contains the live kernel and apps;
//! a second GPT partition owns the WISP save store. Installation is
//! destructive and is not power-fail atomic. Validate everything before
//! the first write, invalidate old partition headers first, and publish
//! the new GPT only after the files and save store have been written.

use alloc::vec::Vec;

pub const ESP_START_LBA: u64 = 2048;
pub const ESP_SECTORS: u64 = 131_072;
pub const SAVE_START_LBA: u64 = ESP_START_LBA + ESP_SECTORS;
pub const MIN_DISK_SECTORS: u64 = 80 * 1024 * 1024 / 512;
/// GPT type GUID 7c2b7e91-51d0-4a2e-916e-574953505356, on-disk byte order.
pub const SAVE_TYPE_GUID: [u8; 16] = [
    0x91, 0x7e, 0x2b, 0x7c, 0xd0, 0x51, 0x2e, 0x4a, 0x91, 0x6e, 0x57, 0x49, 0x53, 0x50, 0x53, 0x56,
];
const ESP_TYPE_GUID: [u8; 16] = [
    0x28, 0x73, 0x2a, 0xc1, 0x1f, 0xf8, 0xd2, 0x11, 0xba, 0x4b, 0x00, 0xa0, 0xc9, 0x3e, 0xc9, 0x3b,
];
const RESERVED_SECTORS: u64 = 32;
const FAT_SECTORS: u64 = 1024;
const DATA_START_LBA: u64 = ESP_START_LBA + RESERVED_SECTORS + 2 * FAT_SECTORS;
const DATA_CLUSTERS: u32 = (ESP_START_LBA + ESP_SECTORS - DATA_START_LBA) as u32;
const SAVE_SECTORS_USED: u64 = 513;
const GPT_ENTRY_COUNT: usize = 128;
const GPT_ENTRY_SIZE: usize = 128;
const GPT_TABLE_SECTORS: u64 = 32;
const EOC: u32 = 0x0fff_ffff;

pub struct InstallFile<'a> {
    /// BOOTX64.EFI goes in EFI/BOOT; every other name must end in .wisp.
    pub name: &'a str,
    pub data: &'a [u8],
}

struct Allocation {
    first: u32,
    count: u32,
}

struct Plan {
    allocations: Vec<Allocation>,
    apps_entries: Vec<[u8; 32]>,
    apps_clusters: u32,
    next_cluster: u32,
    efi_index: usize,
}

fn put16(bytes: &mut [u8], offset: usize, value: u16) {
    bytes[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
}

fn put32(bytes: &mut [u8], offset: usize, value: u32) {
    bytes[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

fn put64(bytes: &mut [u8], offset: usize, value: u64) {
    bytes[offset..offset + 8].copy_from_slice(&value.to_le_bytes());
}

fn get32(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().expect("four bytes"))
}

fn get64(bytes: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes(bytes[offset..offset + 8].try_into().expect("eight bytes"))
}

fn crc_update(mut crc: u32, bytes: &[u8]) -> u32 {
    for &byte in bytes {
        crc ^= u32::from(byte);
        for _ in 0..8 {
            crc = (crc >> 1) ^ (0xedb8_8320u32.wrapping_mul(crc & 1));
        }
    }
    crc
}

fn crc32(bytes: &[u8]) -> u32 {
    !crc_update(!0, bytes)
}

fn directory_entry(name: &[u8; 11], attributes: u8, cluster: u32, size: u32) -> [u8; 32] {
    let mut entry = [0u8; 32];
    entry[..11].copy_from_slice(name);
    entry[11] = attributes;
    // Fixed valid FAT date: 1980-01-01. Builds remain reproducible.
    put16(&mut entry, 16, 0x0021);
    put16(&mut entry, 18, 0x0021);
    put16(&mut entry, 24, 0x0021);
    put16(&mut entry, 20, (cluster >> 16) as u16);
    put16(&mut entry, 26, cluster as u16);
    put32(&mut entry, 28, size);
    entry
}

fn alias(index: usize) -> [u8; 11] {
    let mut name = *b"W0000000WIS";
    for position in 0..7 {
        name[7 - position] = b"0123456789ABCDEF"[(index >> (position * 4)) & 15];
    }
    name
}

fn short_checksum(short: &[u8; 11]) -> u8 {
    short
        .iter()
        .fold(0u8, |sum, &byte| sum.rotate_right(1).wrapping_add(byte))
}

fn push_long_name(entries: &mut Vec<[u8; 32]>, name: &str, short: &[u8; 11]) {
    const OFFSETS: [usize; 13] = [1, 3, 5, 7, 9, 14, 16, 18, 20, 22, 24, 28, 30];
    let count = name.len().div_ceil(13);
    for sequence in (1..=count).rev() {
        let mut entry = [0u8; 32];
        entry[0] = sequence as u8 | if sequence == count { 0x40 } else { 0 };
        entry[11] = 0x0f;
        entry[13] = short_checksum(short);
        for (i, offset) in OFFSETS.into_iter().enumerate() {
            let position = (sequence - 1) * 13 + i;
            let value = match position.cmp(&name.len()) {
                core::cmp::Ordering::Less => u16::from(name.as_bytes()[position]),
                core::cmp::Ordering::Equal => 0,
                core::cmp::Ordering::Greater => 0xffff,
            };
            put16(&mut entry, offset, value);
        }
        entries.push(entry);
    }
}

fn validate_app_name(name: &str) -> bool {
    let Some((stem, extension)) = name.rsplit_once('.') else {
        return false;
    };
    !stem.is_empty()
        && extension.eq_ignore_ascii_case("wisp")
        && name.len() <= 255
        && !name.ends_with([' ', '.'])
        && name
            .bytes()
            .all(|b| (0x20..=0x7e).contains(&b) && !b"\"*/:<>?\\|".contains(&b))
}

fn plan(total_sectors: u64, files: &[InstallFile<'_>]) -> Result<Plan, &'static str> {
    if total_sectors < MIN_DISK_SECTORS || total_sectors >= 1u64 << 48 {
        return Err("installer: disk must be at least 80 MiB and fit LBA48 addressing");
    }
    if files.is_empty() || files.len() > 129 {
        return Err("installer: expected one EFI kernel and at most 128 apps");
    }
    let mut efi_index = None;
    let mut directory_entries = 2usize;
    for (index, file) in files.iter().enumerate() {
        if files[..index]
            .iter()
            .any(|other| other.name.eq_ignore_ascii_case(file.name))
        {
            return Err("installer: duplicate filename");
        }
        if file.name == "BOOTX64.EFI" {
            if file.data.is_empty() {
                return Err("installer: empty EFI kernel");
            }
            efi_index = Some(index);
        } else if validate_app_name(file.name) {
            directory_entries += file.name.len().div_ceil(13) + 1;
        } else {
            return Err(
                "installer: invalid filename (expected BOOTX64.EFI or an ASCII .wisp name)",
            );
        }
        if file.data.len() > u32::MAX as usize {
            return Err("installer: file exceeds the FAT32 size limit");
        }
    }
    let efi_index = efi_index.ok_or("installer: BOOTX64.EFI is missing")?;
    // Include an end-of-directory entry, even if the last file lands
    // exactly at a sector boundary.
    let apps_clusters = ((directory_entries + 1) * 32).div_ceil(512) as u32;
    let mut next_cluster = 5 + apps_clusters;
    let mut allocations = Vec::with_capacity(files.len());
    for file in files {
        let count = file.data.len().div_ceil(512) as u32;
        let first = if count == 0 { 0 } else { next_cluster };
        next_cluster = next_cluster
            .checked_add(count)
            .ok_or("installer: files exceed the ESP capacity")?;
        if next_cluster > DATA_CLUSTERS + 2 {
            return Err("installer: files exceed the ESP capacity");
        }
        allocations.push(Allocation { first, count });
    }
    let mut apps_entries = Vec::with_capacity(directory_entries + 1);
    apps_entries.push(directory_entry(b".          ", 0x10, 5, 0));
    // FAT specifies cluster 0 for a parent that is the root directory.
    apps_entries.push(directory_entry(b"..         ", 0x10, 0, 0));
    for (index, file) in files.iter().enumerate() {
        if index == efi_index {
            continue;
        }
        let short = alias(index);
        push_long_name(&mut apps_entries, file.name, &short);
        apps_entries.push(directory_entry(
            &short,
            0x20,
            allocations[index].first,
            file.data.len() as u32,
        ));
    }
    apps_entries.push([0u8; 32]);
    Ok(Plan {
        allocations,
        apps_entries,
        apps_clusters,
        next_cluster,
        efi_index,
    })
}

fn volume_boot_sector() -> [u8; 512] {
    let mut sector = [0u8; 512];
    sector[..3].copy_from_slice(&[0xeb, 0x58, 0x90]);
    sector[3..11].copy_from_slice(b"WISP OS ");
    put16(&mut sector, 11, 512);
    sector[13] = 1; // one sector per cluster
    put16(&mut sector, 14, RESERVED_SECTORS as u16);
    sector[16] = 2; // redundant FATs
    sector[21] = 0xf8;
    put16(&mut sector, 24, 63);
    put16(&mut sector, 26, 255);
    put32(&mut sector, 28, ESP_START_LBA as u32);
    put32(&mut sector, 32, ESP_SECTORS as u32);
    put32(&mut sector, 36, FAT_SECTORS as u32);
    put32(&mut sector, 44, 2); // root cluster
    put16(&mut sector, 48, 1); // FSInfo
    put16(&mut sector, 50, 6); // backup boot sector
    sector[64] = 0x80;
    sector[66] = 0x29;
    put32(&mut sector, 67, 0x5753_5031);
    sector[71..82].copy_from_slice(b"WISP SYSTEM");
    sector[82..90].copy_from_slice(b"FAT32   ");
    sector[510..].copy_from_slice(&[0x55, 0xaa]);
    sector
}

fn fs_info(next_cluster: u32) -> [u8; 512] {
    let mut sector = [0u8; 512];
    put32(&mut sector, 0, 0x4161_5252);
    put32(&mut sector, 484, 0x6141_7272);
    put32(&mut sector, 488, DATA_CLUSTERS - (next_cluster - 2));
    put32(
        &mut sector,
        492,
        if next_cluster < DATA_CLUSTERS + 2 {
            next_cluster
        } else {
            0xffff_ffff
        },
    );
    put32(&mut sector, 508, 0xaa55_0000);
    sector
}

fn guid(mut installation_id: [u8; 16], discriminator: u8) -> [u8; 16] {
    // GPT stores the first three UUID fields little endian. Preserve the
    // caller's per-installation identity while setting UUID v4/variant
    // bits and assigning distinct IDs to the disk and both partitions.
    installation_id[7] = (installation_id[7] & 0x0f) | 0x40;
    installation_id[8] = (installation_id[8] & 0x3f) | 0x80;
    installation_id[15] ^= discriminator;
    installation_id
}

fn gpt_entry(
    kind: &[u8; 16],
    installation_id: [u8; 16],
    id: u8,
    first: u64,
    last: u64,
    name: &str,
) -> [u8; 128] {
    let mut entry = [0u8; 128];
    entry[..16].copy_from_slice(kind);
    entry[16..32].copy_from_slice(&guid(installation_id, id));
    put64(&mut entry, 32, first);
    put64(&mut entry, 40, last);
    for (i, byte) in name.bytes().take(36).enumerate() {
        put16(&mut entry, 56 + i * 2, u16::from(byte));
    }
    entry
}

fn gpt_table_sector(index: u64, total_sectors: u64, installation_id: [u8; 16]) -> [u8; 512] {
    let mut sector = [0u8; 512];
    if index == 0 {
        sector[..128].copy_from_slice(&gpt_entry(
            &ESP_TYPE_GUID,
            installation_id,
            1,
            ESP_START_LBA,
            SAVE_START_LBA - 1,
            "WISP System",
        ));
        sector[128..256].copy_from_slice(&gpt_entry(
            &SAVE_TYPE_GUID,
            installation_id,
            2,
            SAVE_START_LBA,
            total_sectors - 34,
            "WISP Saves",
        ));
    }
    sector
}

fn gpt_header(
    total_sectors: u64,
    installation_id: [u8; 16],
    backup: bool,
    table_crc: u32,
) -> [u8; 512] {
    let mut sector = [0u8; 512];
    sector[..8].copy_from_slice(b"EFI PART");
    put32(&mut sector, 8, 0x0001_0000);
    put32(&mut sector, 12, 92);
    put64(&mut sector, 24, if backup { total_sectors - 1 } else { 1 });
    put64(&mut sector, 32, if backup { 1 } else { total_sectors - 1 });
    put64(&mut sector, 40, 34);
    put64(&mut sector, 48, total_sectors - 34);
    sector[56..72].copy_from_slice(&guid(installation_id, 0));
    put64(&mut sector, 72, if backup { total_sectors - 33 } else { 2 });
    put32(&mut sector, 80, GPT_ENTRY_COUNT as u32);
    put32(&mut sector, 84, GPT_ENTRY_SIZE as u32);
    put32(&mut sector, 88, table_crc);
    let header_crc = crc32(&sector[..92]);
    put32(&mut sector, 16, header_crc);
    sector
}

fn protective_mbr(total_sectors: u64) -> [u8; 512] {
    let mut sector = [0u8; 512];
    sector[447..450].copy_from_slice(&[0, 2, 0]);
    sector[450] = 0xee;
    sector[451..454].copy_from_slice(&[0xff; 3]);
    put32(&mut sector, 454, 1);
    put32(
        &mut sector,
        458,
        (total_sectors - 1).min(u64::from(u32::MAX)) as u32,
    );
    sector[510..].copy_from_slice(&[0x55, 0xaa]);
    sector
}

fn cluster_lba(cluster: u32) -> u64 {
    DATA_START_LBA + u64::from(cluster - 2)
}

/// Install the live files to an explicitly selected disk. This function
/// performs no device selection or confirmation; its caller owns both.
/// The callback must write exactly one 512-byte logical sector. A
/// callback error stops installation immediately and is returned intact.
/// The caller must flush the device before reporting success.
/// Supply a fresh installation_id for each new disk installation and
/// retain it for verify; repeating the same ID produces identical bytes.
pub fn install<F>(
    total_sectors: u64,
    files: &[InstallFile<'_>],
    installation_id: [u8; 16],
    mut write: F,
) -> Result<(), &'static str>
where
    F: FnMut(u64, &[u8; 512]) -> Result<(), &'static str>,
{
    let plan = plan(total_sectors, files)?;
    let zero = [0u8; 512];

    // Unpublish the old tables before making changes. If power fails
    // now, this is an interrupted install, not a valid new system.
    write(0, &zero)?;
    write(1, &zero)?;
    write(total_sectors - 1, &zero)?;
    for offset in 0..RESERVED_SECTORS {
        write(ESP_START_LBA + offset, &zero)?;
    }
    let boot = volume_boot_sector();
    write(ESP_START_LBA, &boot)?;
    write(ESP_START_LBA + 6, &boot)?;
    let info = fs_info(plan.next_cluster);
    write(ESP_START_LBA + 1, &info)?;
    write(ESP_START_LBA + 7, &info)?;

    // Every FAT sector is initialized, including unallocated entries;
    // untouched free data clusters remain unreferenced by the new volume.
    for offset in 0..FAT_SECTORS {
        let mut sector = [0u8; 512];
        for position in 0..128 {
            let cluster = (offset * 128 + position as u64) as u32;
            let value = match cluster {
                0 => 0x0fff_fff8,
                1..=4 => EOC,
                c if c >= 5 && c < 5 + plan.apps_clusters => {
                    if c + 1 == 5 + plan.apps_clusters {
                        EOC
                    } else {
                        c + 1
                    }
                }
                c => plan
                    .allocations
                    .iter()
                    .find_map(|allocation| {
                        if c >= allocation.first && c < allocation.first + allocation.count {
                            Some(if c + 1 == allocation.first + allocation.count {
                                EOC
                            } else {
                                c + 1
                            })
                        } else {
                            None
                        }
                    })
                    .unwrap_or(0),
            };
            put32(&mut sector, position * 4, value);
        }
        write(ESP_START_LBA + RESERVED_SECTORS + offset, &sector)?;
        write(
            ESP_START_LBA + RESERVED_SECTORS + FAT_SECTORS + offset,
            &sector,
        )?;
    }

    let mut root = [0u8; 512];
    root[..32].copy_from_slice(&directory_entry(b"WISP SYSTEM", 0x08, 0, 0));
    root[32..64].copy_from_slice(&directory_entry(b"EFI        ", 0x10, 3, 0));
    root[64..96].copy_from_slice(&directory_entry(b"APPS       ", 0x10, 5, 0));
    write(cluster_lba(2), &root)?;
    let mut efi = [0u8; 512];
    efi[..32].copy_from_slice(&directory_entry(b".          ", 0x10, 3, 0));
    efi[32..64].copy_from_slice(&directory_entry(b"..         ", 0x10, 0, 0));
    efi[64..96].copy_from_slice(&directory_entry(b"BOOT       ", 0x10, 4, 0));
    write(cluster_lba(3), &efi)?;
    let mut boot_dir = [0u8; 512];
    boot_dir[..32].copy_from_slice(&directory_entry(b".          ", 0x10, 4, 0));
    boot_dir[32..64].copy_from_slice(&directory_entry(b"..         ", 0x10, 3, 0));
    boot_dir[64..96].copy_from_slice(&directory_entry(
        b"BOOTX64 EFI",
        0x20,
        plan.allocations[plan.efi_index].first,
        files[plan.efi_index].data.len() as u32,
    ));
    write(cluster_lba(4), &boot_dir)?;
    for offset in 0..plan.apps_clusters {
        let mut sector = [0u8; 512];
        for position in 0..16 {
            if let Some(entry) = plan.apps_entries.get(offset as usize * 16 + position) {
                sector[position * 32..position * 32 + 32].copy_from_slice(entry);
            }
        }
        write(cluster_lba(5 + offset), &sector)?;
    }
    for (file, allocation) in files.iter().zip(&plan.allocations) {
        for (offset, bytes) in file.data.chunks(512).enumerate() {
            let mut sector = [0u8; 512];
            sector[..bytes.len()].copy_from_slice(bytes);
            write(cluster_lba(allocation.first) + offset as u64, &sector)?;
        }
    }
    for offset in 0..SAVE_SECTORS_USED {
        write(SAVE_START_LBA + offset, &zero)?;
    }
    let mut save = [0u8; 512];
    save[..9].copy_from_slice(b"WISPSAVE1");
    save[9] = 2;
    put16(&mut save, 10, 64);
    put32(&mut save, 12, 4096);
    write(SAVE_START_LBA, &save)?;

    let mut table_crc = !0;
    for offset in 0..GPT_TABLE_SECTORS {
        let sector = gpt_table_sector(offset, total_sectors, installation_id);
        table_crc = crc_update(table_crc, &sector);
        write(2 + offset, &sector)?;
        write(total_sectors - 33 + offset, &sector)?;
    }
    write(
        total_sectors - 1,
        &gpt_header(total_sectors, installation_id, true, !table_crc),
    )?;
    write(
        1,
        &gpt_header(total_sectors, installation_id, false, !table_crc),
    )?;
    write(0, &protective_mbr(total_sectors))?;
    Ok(())
}

/// Read back every sector that belongs to the completed installation.
/// Uses the same deterministic generator as installation, but omits its
/// temporary invalidations and reserved-sector zeros that are superseded
/// by boot records. Free data clusters are intentionally not examined.
pub fn verify<F>(
    total_sectors: u64,
    files: &[InstallFile<'_>],
    installation_id: [u8; 16],
    mut read: F,
) -> Result<(), &'static str>
where
    F: FnMut(u64, &mut [u8; 512]) -> Result<(), &'static str>,
{
    install(total_sectors, files, installation_id, |lba, expected| {
        let overwritten = lba == 0
            || lba == 1
            || lba == total_sectors - 1
            || lba == ESP_START_LBA
            || lba == ESP_START_LBA + 1
            || lba == ESP_START_LBA + 6
            || lba == ESP_START_LBA + 7
            || lba == SAVE_START_LBA;
        if overwritten && expected.iter().all(|&byte| byte == 0) {
            return Ok(());
        }
        let mut actual = [0u8; 512];
        read(lba, &mut actual)?;
        if actual != *expected {
            return Err("installer: installed sector failed read-back verification");
        }
        Ok(())
    })
}

/// Locate a WISP save partition without modifying the disk. Legacy raw
/// stores and non-GPT disks return None. Recognized but malformed GPTs
/// return an error; callers must not guess at a save offset afterward.
/// Both the primary header and complete partition table are CRC checked,
/// and every partition is checked for bounds and overlap.
pub fn find_save_region<F>(
    total_sectors: u64,
    mut read: F,
) -> Result<Option<(u64, u64)>, &'static str>
where
    F: FnMut(u64, &mut [u8; 512]) -> Result<(), &'static str>,
{
    if total_sectors < 2 {
        return Ok(None);
    }
    let mut header = [0u8; 512];
    read(1, &mut header)?;
    if &header[..8] != b"EFI PART" {
        return Ok(None);
    }
    let size = get32(&header, 12) as usize;
    if total_sectors < 68
        || !(92..=512).contains(&size)
        || get32(&header, 8) != 0x0001_0000
        || get32(&header, 20) != 0
        || get64(&header, 24) != 1
        || get64(&header, 32) != total_sectors - 1
    {
        return Err("installer: invalid GPT header geometry");
    }
    let recorded_crc = get32(&header, 16);
    put32(&mut header, 16, 0);
    if crc32(&header[..size]) != recorded_crc {
        return Err("installer: GPT header checksum mismatch");
    }
    let first_usable = get64(&header, 40);
    let last_usable = get64(&header, 48);
    let table_start = get64(&header, 72);
    let count = get32(&header, 80) as usize;
    let entry_size = get32(&header, 84) as usize;
    if count == 0 || count > GPT_ENTRY_COUNT || entry_size != GPT_ENTRY_SIZE {
        return Err("installer: unsupported GPT partition table geometry");
    }
    let table_bytes = count * entry_size;
    let sectors = table_bytes.div_ceil(512) as u64;
    if first_usable < 34
        || first_usable > last_usable
        || last_usable >= total_sectors - 33
        || table_start < 2
        || table_start
            .checked_add(sectors)
            .is_none_or(|end| end > first_usable)
    {
        return Err("installer: GPT partition table is outside the usable disk bounds");
    }
    let mut table = Vec::with_capacity(sectors as usize * 512);
    for offset in 0..sectors {
        let mut sector = [0u8; 512];
        read(table_start + offset, &mut sector)?;
        table.extend_from_slice(&sector);
    }
    if crc32(&table[..table_bytes]) != get32(&header, 88) {
        return Err("installer: GPT partition table checksum mismatch");
    }
    let mut regions: Vec<(u64, u64)> = Vec::new();
    let mut save_region = None;
    for entry in table[..table_bytes].chunks_exact(GPT_ENTRY_SIZE) {
        if entry[..16].iter().all(|&b| b == 0) {
            continue;
        }
        let first = get64(entry, 32);
        let last = get64(entry, 40);
        if first < first_usable
            || last > last_usable
            || first > last
            || regions
                .iter()
                .any(|&(other_first, other_last)| first <= other_last && last >= other_first)
        {
            return Err("installer: GPT partition bounds overlap or exceed the disk");
        }
        regions.push((first, last));
        if entry[..16] == SAVE_TYPE_GUID {
            if save_region.is_some() || last - first + 1 < SAVE_SECTORS_USED {
                return Err("installer: duplicate or undersized WISP save partition");
            }
            save_region = Some((first, last - first + 1));
        }
    }
    Ok(save_region)
}
