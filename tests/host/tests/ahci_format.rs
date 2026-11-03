#[path = "../../../src/ahci_format.rs"]
mod ahci_format;

fn identify(sectors: u64) -> [u16; 256] {
    let mut words = [0; 256];
    for index in 0..4 {
        words[100 + index] = (sectors >> (16 * index)) as u16;
    }
    words
}

#[test]
fn capacity_accepts_only_nonempty_lba48_geometry() {
    for sectors in [1, 2048, u64::from(u32::MAX), (1 << 48) - 1, 1 << 48] {
        assert_eq!(ahci_format::capacity(&identify(sectors)), Ok(sectors));
        assert!(sectors.checked_mul(512).is_some());
    }
    for sectors in [0, (1 << 48) + 1, 1 << 55, 1 << 63, u64::MAX] {
        assert!(ahci_format::capacity(&identify(sectors)).is_err());
    }
}

#[test]
fn requests_at_capacity_or_past_the_fis_address_limit_are_rejected() {
    for sectors in [1, 2048, 1 << 48] {
        assert_eq!(ahci_format::validate_sector(sectors, 0), Ok(()));
        assert_eq!(ahci_format::validate_sector(sectors, sectors - 1), Ok(()));
        for lba in [sectors, 1 << 48, u64::MAX] {
            assert!(ahci_format::validate_sector(sectors, lba).is_err());
        }
    }
    for sectors in [0, (1 << 48) + 1, u64::MAX] {
        assert!(ahci_format::validate_sector(sectors, 0).is_err());
    }
}

#[test]
fn deterministic_capacity_samples_preserve_the_addressing_invariant() {
    let mut seed = 0xa13c_4800_1937_5badu64;
    for _ in 0..10_000 {
        seed = seed.wrapping_mul(6364136223846793005).wrapping_add(1);
        let sectors = seed >> (seed & 31);
        let result = ahci_format::capacity(&identify(sectors));
        assert_eq!(result.is_ok(), sectors > 0 && sectors <= (1 << 48));
        if result.is_ok() {
            assert!(ahci_format::validate_sector(sectors, sectors - 1).is_ok());
            assert!(ahci_format::validate_sector(sectors, sectors).is_err());
        }
    }
}
