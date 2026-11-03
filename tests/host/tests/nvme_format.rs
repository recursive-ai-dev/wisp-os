#[path = "../../../src/nvme_format.rs"]
mod nvme_format;

fn identify(blocks: u64, exponent: u8) -> [u8; 4096] {
    let mut page = [0; 4096];
    page[0..8].copy_from_slice(&blocks.to_le_bytes());
    page[8..16].copy_from_slice(&blocks.to_le_bytes());
    page[130] = exponent;
    page
}

#[test]
fn capacity_is_translated_to_512_byte_sectors_without_truncation() {
    let native = nvme_format::namespace(&identify(1234, 9)).unwrap();
    assert_eq!(native.blocks, 1234);
    assert_eq!(native.block_size, 512);
    assert_eq!(native.total_sectors, 1234);
    let native = nvme_format::namespace(&identify(1234, 12)).unwrap();
    assert_eq!(native.block_size, 4096);
    assert_eq!(native.total_sectors, 9872);
    assert!(nvme_format::namespace(&identify(u64::MAX, 12)).is_err());
}

#[test]
fn selected_format_is_validated_instead_of_assuming_format_zero() {
    let mut page = identify(100, 10);
    page[25] = 1;
    page[26] = 1;
    page[134] = 12;
    assert_eq!(nvme_format::namespace(&page).unwrap().block_size, 4096);
    page[25] = 0;
    assert!(nvme_format::namespace(&page).is_err());
    page[26] = 0x20;
    assert!(nvme_format::namespace(&page).is_err());
}

#[test]
fn inactive_and_unsupported_namespace_layouts_are_rejected() {
    assert!(nvme_format::namespace(&identify(0, 9)).is_err());
    for exponent in [0, 8, 10, 11, 13, 63] {
        assert!(nvme_format::namespace(&identify(100, exponent)).is_err());
    }
    let mut page = identify(100, 9);
    page[8..16].fill(0);
    assert!(nvme_format::namespace(&page).is_err());
    page[8..16].copy_from_slice(&101u64.to_le_bytes());
    assert!(nvme_format::namespace(&page).is_err());
    for protected in [false, true] {
        let mut page = identify(100, 9);
        if protected {
            page[29] = 1;
        } else {
            page[128] = 8;
        }
        assert!(nvme_format::namespace(&page).is_err());
    }
}
