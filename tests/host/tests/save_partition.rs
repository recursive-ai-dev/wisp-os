//! The installer and persistence share a physical disk. Every save-sector
//! operation must stay inside the dedicated GPT partition.
use wisp_host_tests::ahci::Hba;
use wisp_host_tests::disk_layout::{self, InstallFile, SAVE_START_LBA};
use wisp_host_tests::save_format::{self, Probe, SaveDisk, REGION_SECTORS};
use wisp_host_tests::wisp::value::Value;

const DISK_SECTORS: usize = 80 * 1024 * 1024 / 512;

fn installed_disk() -> Hba {
    let hba = Hba::new(DISK_SECTORS);
    disk_layout::install(
        DISK_SECTORS as u64,
        &[
            InstallFile { name: "BOOTX64.EFI", data: b"MZ installer fixture" },
            InstallFile { name: "hello.wisp", data: b"print(\"hello\");" },
        ],
        [0x57; 16],
        |lba, sector| {
            hba.seed(lba, sector);
            Ok(())
        },
    ).unwrap();
    hba
}

#[test]
fn installed_store_mounts_read_only_and_all_operations_stay_in_its_partition() {
    let hba = installed_disk();
    assert_eq!(unsafe { save_format::probe(&hba, hba.total_sectors()) }, Ok(Probe::Formatted));
    assert_eq!(hba.writes.get(), 0, "boot probe must never write");
    let before = hba.media();
    let mut disk = SaveDisk::new(hba, DISK_SECTORS as u64, true);
    assert_eq!(disk.format_range().unwrap(), (SAVE_START_LBA, SAVE_START_LBA + REGION_SECTORS - 1));
    disk.save("count", &Value::Int(41)).unwrap();
    disk.save("count", &Value::Int(42)).unwrap();
    assert!(matches!(disk.load("count").unwrap(), Some(Value::Int(42))));
    assert_eq!(disk.keys().unwrap().len(), 1);
    assert!(disk.delete("count").unwrap());
    disk.save("before_format", &Value::Bool(true)).unwrap();
    disk.format().unwrap();
    assert!(disk.is_mounted());
    assert!(disk.keys().unwrap().is_empty());
    disk.save("after_format", &Value::Int(7)).unwrap();
    assert!(matches!(disk.load("after_format").unwrap(), Some(Value::Int(7))));
    let after = disk.hba().media();
    for (lba, (old, new)) in before.iter().zip(&after).enumerate() {
        if !(SAVE_START_LBA..SAVE_START_LBA + REGION_SECTORS).contains(&(lba as u64)) {
            assert_eq!(old, new, "save operation changed sector {lba} outside the store");
        }
    }
}

#[test]
fn raw_api_also_discovers_the_installed_save_partition() {
    let hba = installed_disk();
    let first_sector = hba.media()[0];
    unsafe { save_format::save(&hba, "raw_api", &Value::Int(99)) }.unwrap();
    assert!(matches!(unsafe { save_format::load(&hba, "raw_api") }.unwrap(), Some(Value::Int(99))));
    assert_eq!(hba.media()[0], first_sector, "raw API overwrote protective MBR");
}

#[test]
fn foreign_mbr_disk_cannot_be_formatted_as_a_raw_store() {
    let hba = Hba::new(2048);
    let mut mbr = [0x37; 512];
    mbr[510..].copy_from_slice(&[0x55, 0xaa]);
    hba.seed(0, &mbr);
    let before = hba.media();
    assert_eq!(unsafe { save_format::probe(&hba, hba.total_sectors()) }, Ok(Probe::Foreign));
    assert!(unsafe { save_format::provision(&hba, hba.total_sectors()) }.is_err());
    let mut disk = SaveDisk::new(hba, 2048, false);
    assert!(disk.format_range().is_err());
    assert!(disk.format().is_err());
    assert_eq!(disk.hba().writes.get(), 0);
    assert_eq!(disk.hba().media(), before);
}

#[test]
fn malformed_gpt_is_never_treated_as_a_raw_disk_even_without_a_valid_mbr() {
    let hba = Hba::new(2048);
    let mut header = [0u8; 512];
    header[..8].copy_from_slice(b"EFI PART");
    // Deliberately invalid revision, size, LBAs and CRC.
    hba.seed(1, &header);
    let before = hba.media();
    assert!(unsafe { save_format::probe(&hba, hba.total_sectors()) }.is_err());
    assert!(unsafe { save_format::provision(&hba, hba.total_sectors()) }.is_err());
    let mut disk = SaveDisk::new(hba, 2048, false);
    assert!(disk.format_range().is_err());
    assert!(disk.format().is_err());
    assert_eq!(disk.hba().writes.get(), 0);
    assert_eq!(disk.hba().media(), before);
}
