use std::collections::BTreeMap;

use wisp_host_tests::disk_layout::{
    self, InstallFile, ESP_SECTORS, ESP_START_LBA, MIN_DISK_SECTORS, SAVE_START_LBA, SAVE_TYPE_GUID,
};

const TOTAL: u64 = MIN_DISK_SECTORS;
const INSTALLATION_ID: [u8; 16] = [0x57; 16];
type Sector = [u8; 512];

#[derive(Clone, Default)]
struct Disk {
    sectors: BTreeMap<u64, Sector>,
    writes: Vec<u64>,
}

impl Disk {
    fn read(&self, lba: u64) -> Sector {
        assert!(lba < TOTAL);
        self.sectors.get(&lba).copied().unwrap_or([0xa5; 512])
    }

    fn install(files: &[InstallFile<'_>]) -> Self {
        let mut disk = Self::default();
        disk_layout::install(TOTAL, files, INSTALLATION_ID, |lba, sector| {
            assert!(lba < TOTAL, "write outside disk at {lba}");
            disk.sectors.insert(lba, *sector);
            disk.writes.push(lba);
            Ok(())
        })
        .unwrap();
        disk
    }

    fn verify(&self, files: &[InstallFile<'_>]) -> Result<(), &'static str> {
        disk_layout::verify(TOTAL, files, INSTALLATION_ID, |lba, sector| {
            *sector = self.read(lba);
            Ok(())
        })
    }
}

fn u16_at(bytes: &[u8], offset: usize) -> u16 {
    u16::from_le_bytes(bytes[offset..offset + 2].try_into().unwrap())
}
fn u32_at(bytes: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes(bytes[offset..offset + 4].try_into().unwrap())
}
fn u64_at(bytes: &[u8], offset: usize) -> u64 {
    u64::from_le_bytes(bytes[offset..offset + 8].try_into().unwrap())
}

// A deliberately separate implementation, with the standard check value.
fn crc(bytes: &[u8]) -> u32 {
    let mut state = u32::MAX;
    for byte in bytes {
        state ^= u32::from(*byte);
        for _ in 0..8 {
            state = if state & 1 == 1 {
                0xedb8_8320 ^ (state >> 1)
            } else {
                state >> 1
            };
        }
    }
    !state
}

fn header_crc(header: &Sector) -> u32 {
    let mut data = *header;
    data[16..20].fill(0);
    crc(&data[..u32_at(header, 12) as usize])
}

fn repair_gpt(disk: &mut Disk) {
    let mut table = Vec::new();
    for lba in 2..34 {
        table.extend_from_slice(&disk.read(lba));
    }
    let table_crc = crc(&table);
    for (lba, backup) in [(1, false), (TOTAL - 1, true)] {
        let mut header = disk.read(lba);
        header[88..92].copy_from_slice(&table_crc.to_le_bytes());
        let checksum = header_crc(&header);
        header[16..20].copy_from_slice(&checksum.to_le_bytes());
        disk.sectors.insert(lba, header);
        if backup {
            for offset in 0..32 {
                disk.sectors
                    .insert(TOTAL - 33 + offset, disk.read(2 + offset));
            }
        }
    }
}

struct Fat<'a> {
    disk: &'a Disk,
    first_fat: u64,
    second_fat: u64,
    data: u64,
    root: u32,
    max_cluster: u32,
}

impl<'a> Fat<'a> {
    fn open(disk: &'a Disk) -> Self {
        let boot = disk.read(ESP_START_LBA);
        assert_eq!(u16_at(&boot, 11), 512);
        assert_eq!(boot[13], 1);
        assert_eq!(boot[16], 2);
        assert_eq!(u16_at(&boot, 17), 0, "FAT32 has no fixed root entries");
        assert_eq!(u16_at(&boot, 22), 0, "FAT32 uses the 32-bit FAT size");
        assert_eq!(u32_at(&boot, 28) as u64, ESP_START_LBA);
        assert_eq!(u32_at(&boot, 32) as u64, ESP_SECTORS);
        assert_eq!(boot[510..], [0x55, 0xaa]);
        let reserved = u16_at(&boot, 14) as u64;
        let fat_size = u32_at(&boot, 36) as u64;
        let data = ESP_START_LBA + reserved + fat_size * 2;
        let clusters = ESP_SECTORS - reserved - fat_size * 2;
        assert!(
            clusters >= 65_525,
            "volume must classify as FAT32 by cluster count"
        );
        assert!(fat_size * 512 / 4 >= clusters + 2);
        assert_eq!(disk.read(ESP_START_LBA + u16_at(&boot, 50) as u64), boot);
        let info = disk.read(ESP_START_LBA + u16_at(&boot, 48) as u64);
        assert_eq!(u32_at(&info, 0), 0x4161_5252);
        assert_eq!(u32_at(&info, 484), 0x6141_7272);
        assert_eq!(u32_at(&info, 508), 0xaa55_0000);
        Self {
            disk,
            first_fat: ESP_START_LBA + reserved,
            second_fat: ESP_START_LBA + reserved + fat_size,
            data,
            root: u32_at(&boot, 44),
            max_cluster: clusters as u32 + 1,
        }
    }

    fn chain(&self, mut cluster: u32) -> Vec<u8> {
        let mut bytes = Vec::new();
        let mut seen = Vec::new();
        while cluster >= 2 {
            assert!(cluster <= self.max_cluster, "bad data cluster {cluster}");
            assert!(!seen.contains(&cluster), "FAT cycle");
            seen.push(cluster);
            bytes.extend_from_slice(&self.disk.read(self.data + cluster as u64 - 2));
            let byte_offset = cluster as u64 * 4;
            let fat1 = self.disk.read(self.first_fat + byte_offset / 512);
            let fat2 = self.disk.read(self.second_fat + byte_offset / 512);
            assert_eq!(fat1, fat2);
            cluster = u32_at(&fat1, (byte_offset % 512) as usize) & 0x0fff_ffff;
            if cluster >= 0x0fff_fff8 {
                break;
            }
        }
        bytes
    }

    fn directory(&self, cluster: u32) -> Vec<(String, u32, u32)> {
        let bytes = self.chain(cluster);
        let mut result = Vec::new();
        let mut long_parts = BTreeMap::new();
        let mut long_checksum = None;
        for entry in bytes.chunks_exact(32) {
            if entry[0] == 0 {
                break;
            }
            if entry[11] == 0x0f {
                if entry[0] & 0x40 != 0 {
                    long_parts.clear();
                    long_checksum = Some(entry[13]);
                }
                assert_eq!(long_checksum, Some(entry[13]));
                let text: Vec<u16> = [1, 3, 5, 7, 9, 14, 16, 18, 20, 22, 24, 28, 30]
                    .into_iter()
                    .map(|offset| u16_at(entry, offset))
                    .collect();
                long_parts.insert(entry[0] & 0x1f, text);
                continue;
            }
            if entry[11] & 8 != 0 {
                continue;
            } // volume label
            let name = if long_parts.is_empty() {
                let base = std::str::from_utf8(&entry[..8]).unwrap().trim_end();
                let ext = std::str::from_utf8(&entry[8..11]).unwrap().trim_end();
                if ext.is_empty() {
                    base.to_string()
                } else {
                    format!("{base}.{ext}")
                }
            } else {
                let sum = entry[..11]
                    .iter()
                    .fold(0u8, |sum, &byte| sum.rotate_right(1).wrapping_add(byte));
                assert_eq!(long_checksum, Some(sum), "LFN belongs to another alias");
                let chars: Vec<u16> = long_parts
                    .values()
                    .flatten()
                    .copied()
                    .take_while(|&c| c != 0 && c != 0xffff)
                    .collect();
                long_parts.clear();
                String::from_utf16(&chars).unwrap()
            };
            let first = u32::from(u16_at(entry, 26)) | (u32::from(u16_at(entry, 20)) << 16);
            result.push((name, first, u32_at(entry, 28)));
        }
        result
    }

    fn lookup(&self, parent: u32, name: &str) -> (u32, u32) {
        self.directory(parent)
            .into_iter()
            .find(|entry| entry.0 == name)
            .map(|(_, first, size)| (first, size))
            .unwrap_or_else(|| panic!("missing {name}"))
    }

    fn file(&self, parent: u32, name: &str) -> Vec<u8> {
        let (first, size) = self.lookup(parent, name);
        let mut data = if first == 0 {
            Vec::new()
        } else {
            self.chain(first)
        };
        assert!(data.len() >= size as usize);
        assert!(
            data[size as usize..].iter().all(|&byte| byte == 0),
            "file tail leaks old contents"
        );
        data.truncate(size as usize);
        data
    }
}

#[test]
fn gpt_has_valid_primary_backup_and_nonoverlapping_partition_geometry() {
    assert_eq!(crc(b"123456789"), 0xcbf4_3926);
    let disk = Disk::install(&[InstallFile {
        name: "BOOTX64.EFI",
        data: b"MZkernel",
    }]);
    let mbr = disk.read(0);
    assert_eq!(mbr[510..], [0x55, 0xaa]);
    assert_eq!(mbr[450], 0xee);
    assert_eq!(u32_at(&mbr, 454), 1);
    assert_eq!(u32_at(&mbr, 458) as u64, TOTAL - 1);
    let mut tables = Vec::new();
    for offset in 0..32 {
        let primary = disk.read(2 + offset);
        assert_eq!(disk.read(TOTAL - 33 + offset), primary);
        tables.extend_from_slice(&primary);
    }
    for (header_lba, other_lba, table_lba) in [(1, TOTAL - 1, 2), (TOTAL - 1, 1, TOTAL - 33)] {
        let header = disk.read(header_lba);
        assert_eq!(&header[..8], b"EFI PART");
        assert_eq!(header_crc(&header), u32_at(&header, 16));
        assert_eq!(u64_at(&header, 24), header_lba);
        assert_eq!(u64_at(&header, 32), other_lba);
        assert_eq!(u64_at(&header, 72), table_lba);
        assert_eq!(u32_at(&header, 80), 128);
        assert_eq!(u32_at(&header, 84), 128);
        assert_eq!(u32_at(&header, 88), crc(&tables));
        assert_eq!(u64_at(&header, 48), TOTAL - 34);
    }
    assert_eq!(u64_at(&tables, 32), ESP_START_LBA);
    assert_eq!(u64_at(&tables, 40), ESP_START_LBA + ESP_SECTORS - 1);
    assert_eq!(tables[128..144], SAVE_TYPE_GUID);
    assert_eq!(u64_at(&tables, 128 + 32), SAVE_START_LBA);
    assert_eq!(u64_at(&tables, 128 + 40), TOTAL - 34);
    let found = disk_layout::find_save_region(TOTAL, |lba, sector| {
        *sector = disk.read(lba);
        Ok(())
    })
    .unwrap();
    assert_eq!(found, Some((SAVE_START_LBA, TOTAL - 33 - SAVE_START_LBA)));
    assert_eq!(
        &disk.read(SAVE_START_LBA)[..16],
        b"WISPSAVE1\x02\x40\x00\x00\x10\x00\x00"
    );
    for lba in SAVE_START_LBA + 1..SAVE_START_LBA + 513 {
        assert_eq!(disk.read(lba), [0u8; 512]);
    }
    assert_eq!(disk.read(SAVE_START_LBA + 513), [0xa5; 512]);
    assert!(
        disk.writes.len() < 5000,
        "installer should initialize metadata and files, not erase 64 MiB"
    );
}

#[test]
fn fat_directories_reconstruct_all_live_files_and_long_names_across_clusters() {
    let kernel: Vec<u8> = (0..6000).map(|i| (i % 251) as u8).collect();
    let names: Vec<String> = (0..40)
        .map(|i| format!("a_very_long_program_name_{i:03}.wisp"))
        .chain([
            "xor_brain.wisp".to_string(),
            "exactly8.wisp".to_string(),
            format!("{}.wisp", "x".repeat(250)),
        ])
        .collect();
    let payloads: Vec<Vec<u8>> = names
        .iter()
        .enumerate()
        .map(|(i, _)| vec![i as u8; if i == 0 { 0 } else { 1 + i * 37 }])
        .collect();
    let mut files = vec![InstallFile {
        name: "BOOTX64.EFI",
        data: &kernel,
    }];
    files.extend(
        names
            .iter()
            .zip(&payloads)
            .map(|(name, data)| InstallFile { name, data }),
    );
    let disk = Disk::install(&files);
    let fat = Fat::open(&disk);
    let (efi_dir, _) = fat.lookup(fat.root, "EFI");
    let (boot_dir, _) = fat.lookup(efi_dir, "BOOT");
    assert_eq!(fat.file(boot_dir, "BOOTX64.EFI"), kernel);
    let (apps_dir, _) = fat.lookup(fat.root, "APPS");
    assert!(
        fat.chain(apps_dir).len() > 512,
        "fixture must cover directory chaining"
    );
    for (name, data) in names.iter().zip(&payloads) {
        assert_eq!(fat.file(apps_dir, name), *data, "wrong {name}");
    }
    assert_eq!(fat.directory(apps_dir).len(), names.len() + 2); // includes . and ..
    disk.verify(&files).unwrap();
}

#[test]
fn malformed_install_inputs_never_write_any_sector() {
    let kernel = InstallFile {
        name: "BOOTX64.EFI",
        data: b"MZ",
    };
    let invalid_names = [
        "../oops.wisp",
        "path\\oops.wisp",
        "bad?.wisp",
        "bad:name.wisp",
        ".wisp",
        "é.wisp",
        "hello.txt",
        "BOOTX64.efi",
        "test.wisp ",
    ];
    for name in invalid_names {
        let mut writes = 0;
        let files = [
            InstallFile {
                name: kernel.name,
                data: kernel.data,
            },
            InstallFile { name, data: b"x" },
        ];
        assert!(
            disk_layout::install(TOTAL, &files, INSTALLATION_ID, |_, _| {
                writes += 1;
                Ok(())
            })
            .is_err(),
            "accepted {name}"
        );
        assert_eq!(writes, 0, "wrote before rejecting {name}");
    }
    let oversized = vec![0u8; ESP_SECTORS as usize * 512];
    let long_name = format!("{}.wisp", "x".repeat(251));
    let cases = vec![
        vec![],
        vec![InstallFile {
            name: "BOOTX64.EFI",
            data: b"",
        }],
        vec![InstallFile {
            name: "hello.wisp",
            data: b"x",
        }],
        vec![
            InstallFile {
                name: kernel.name,
                data: kernel.data,
            },
            InstallFile {
                name: "one.wisp",
                data: b"x",
            },
            InstallFile {
                name: "ONE.WISP",
                data: b"y",
            },
        ],
        vec![InstallFile {
            name: "BOOTX64.EFI",
            data: &oversized,
        }],
        vec![
            InstallFile {
                name: kernel.name,
                data: kernel.data,
            },
            InstallFile {
                name: &long_name,
                data: b"x",
            },
        ],
    ];
    for files in cases {
        let mut writes = 0;
        assert!(
            disk_layout::install(TOTAL, &files, INSTALLATION_ID, |_, _| {
                writes += 1;
                Ok(())
            })
            .is_err()
        );
        assert_eq!(writes, 0);
    }
    for total in [0, 1, MIN_DISK_SECTORS - 1, 1u64 << 48, u64::MAX] {
        let mut writes = 0;
        assert!(disk_layout::install(
            total,
            &[InstallFile {
                name: kernel.name,
                data: kernel.data
            }],
            INSTALLATION_ID,
            |_, _| {
                writes += 1;
                Ok(())
            }
        )
        .is_err());
        assert_eq!(writes, 0);
    }
}

#[test]
fn readback_verifies_final_sectors_and_detects_corruption() {
    let files = [
        InstallFile {
            name: "BOOTX64.EFI",
            data: b"MZkernel",
        },
        InstallFile {
            name: "hello.wisp",
            data: b"print(42);",
        },
    ];
    let disk = Disk::install(&files);
    disk.verify(&files).unwrap();
    let again = Disk::install(&files);
    assert_eq!(disk.sectors, again.sectors);
    let fat = Fat::open(&disk);
    let data_sector = fat.data
        + u64::from(
            fat.lookup(
                fat.lookup(fat.lookup(fat.root, "EFI").0, "BOOT").0,
                "BOOTX64.EFI",
            )
            .0,
        )
        - 2;
    for lba in [
        0,
        1,
        2,
        TOTAL - 1,
        TOTAL - 33,
        ESP_START_LBA,
        ESP_START_LBA + 1,
        fat.first_fat,
        fat.second_fat,
        fat.data,
        data_sector,
        SAVE_START_LBA,
        SAVE_START_LBA + 512,
    ] {
        let mut bad = disk.clone();
        bad.sectors.get_mut(&lba).unwrap()[100] ^= 1;
        assert!(
            bad.verify(&files).is_err(),
            "ignored corruption at sector {lba}"
        );
    }
}

#[test]
fn installation_identity_makes_disk_and_partition_guids_unique() {
    let files = [InstallFile {
        name: "BOOTX64.EFI",
        data: b"MZkernel",
    }];
    let disk = Disk::install(&files);
    let other_id = [0xab; 16];
    let mut other = Disk::default();
    disk_layout::install(TOTAL, &files, other_id, |lba, sector| {
        other.sectors.insert(lba, *sector);
        Ok(())
    })
    .unwrap();
    let disk_guid = &disk.read(1)[56..72];
    let esp_guid = &disk.read(2)[16..32];
    let save_guid = &disk.read(2)[144..160];
    assert_ne!(disk_guid, &other.read(1)[56..72]);
    assert_ne!(esp_guid, &other.read(2)[16..32]);
    assert_ne!(save_guid, &other.read(2)[144..160]);
    assert_ne!(disk_guid, esp_guid);
    assert_ne!(disk_guid, save_guid);
    assert_ne!(esp_guid, save_guid);
    for guid in [disk_guid, esp_guid, save_guid] {
        assert_eq!(guid[7] >> 4, 4);
        assert_eq!(guid[8] >> 6, 2);
    }
    disk_layout::verify(TOTAL, &files, other_id, |lba, sector| {
        *sector = other.read(lba);
        Ok(())
    })
    .unwrap();
    assert!(
        disk_layout::verify(TOTAL, &files, INSTALLATION_ID, |lba, sector| {
            *sector = other.read(lba);
            Ok(())
        })
        .is_err()
    );
}

#[test]
fn interrupted_writes_stop_immediately_and_do_not_publish_partial_tables() {
    let files = [InstallFile {
        name: "BOOTX64.EFI",
        data: b"MZkernel",
    }];
    let baseline = Disk::install(&files);
    assert_eq!(&baseline.writes[..3], &[0, 1, TOTAL - 1]);
    assert_eq!(
        &baseline.writes[baseline.writes.len() - 3..],
        &[TOTAL - 1, 1, 0]
    );
    for fail_at in [
        0,
        1,
        2,
        30,
        500,
        baseline.writes.len() - 4,
        baseline.writes.len() - 1,
    ] {
        let mut attempts = 0;
        let mut disk = Disk::default();
        let result = disk_layout::install(TOTAL, &files, INSTALLATION_ID, |lba, sector| {
            let index = attempts;
            attempts += 1;
            if index == fail_at {
                return Err("injected I/O failure");
            }
            disk.sectors.insert(lba, *sector);
            Ok(())
        });
        assert_eq!(result, Err("injected I/O failure"));
        assert_eq!(attempts, fail_at + 1);
        assert_ne!(
            disk.read(0)[510..],
            [0x55, 0xaa],
            "published final MBR despite failed install"
        );
    }
}

#[test]
fn gpt_discovery_rejects_bad_checksums_bounds_overlaps_and_duplicate_stores() {
    let baseline = Disk::install(&[InstallFile {
        name: "BOOTX64.EFI",
        data: b"MZkernel",
    }]);
    let find = |disk: &Disk| {
        disk_layout::find_save_region(TOTAL, |lba, sector| {
            *sector = disk.read(lba);
            Ok(())
        })
    };
    let mut bad_header = baseline.clone();
    bad_header.sectors.get_mut(&1).unwrap()[56] ^= 1;
    assert!(find(&bad_header).is_err());
    let mut bad_table = baseline.clone();
    bad_table.sectors.get_mut(&2).unwrap()[200] ^= 1;
    assert!(find(&bad_table).is_err());
    for (first, last) in [
        (ESP_START_LBA, SAVE_START_LBA),
        (SAVE_START_LBA, TOTAL - 1),
        (SAVE_START_LBA, SAVE_START_LBA + 511),
        (SAVE_START_LBA + 100, SAVE_START_LBA),
    ] {
        let mut bad = baseline.clone();
        let entry = bad.sectors.get_mut(&2).unwrap();
        entry[160..168].copy_from_slice(&first.to_le_bytes());
        entry[168..176].copy_from_slice(&last.to_le_bytes());
        repair_gpt(&mut bad);
        assert!(
            find(&bad).is_err(),
            "accepted invalid save bounds {first}..{last}"
        );
    }
    let mut duplicate = baseline.clone();
    let mut entry = duplicate.read(2)[128..256].to_vec();
    // Make two disjoint valid-sized save partitions, so only duplication fails.
    duplicate.sectors.get_mut(&2).unwrap()[168..176]
        .copy_from_slice(&(SAVE_START_LBA + 512).to_le_bytes());
    entry[32..40].copy_from_slice(&(SAVE_START_LBA + 1024).to_le_bytes());
    duplicate.sectors.get_mut(&2).unwrap()[256..384].copy_from_slice(&entry);
    repair_gpt(&mut duplicate);
    assert!(find(&duplicate).is_err());
    let mut no_store = baseline.clone();
    no_store.sectors.get_mut(&2).unwrap()[128..256].fill(0);
    repair_gpt(&mut no_store);
    assert_eq!(find(&no_store), Ok(None));
    let blank = Disk::default();
    assert_eq!(find(&blank), Ok(None));
    assert_eq!(
        disk_layout::find_save_region(1, |_, _| panic!("too-small disk read")),
        Ok(None)
    );
    assert_eq!(
        disk_layout::find_save_region(TOTAL, |_, _| Err("read failure")),
        Err("read failure")
    );
}
