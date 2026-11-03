#[path = "../../../src/usb_bot.rs"]
mod usb_bot;

fn config() -> Vec<u8> {
    vec![
        9, 2, 44, 0, 1, 1, 0, 0x80, 50, 9, 4, 0, 0, 2, 8, 6, 0x50, 0, 7, 5, 0x81, 2, 0, 4, 0, 6,
        0x30, 3, 0, 0, 0, 7, 5, 2, 2, 0, 4, 0, 6, 0x30, 7, 0, 0, 0,
    ]
}

#[test]
fn descriptors_match_only_lun_zero_bot_interfaces_and_bulk_endpoints() {
    let config = config();
    let interface = usb_bot::interface(&config).unwrap();
    assert_eq!(interface.configuration, 1);
    assert_eq!(interface.number, 0);
    assert_eq!(interface.input.address, 0x81);
    assert_eq!(interface.input.packet_size, 1024);
    assert_eq!(interface.input.max_burst, 3);
    assert_eq!(interface.output.address, 2);
    assert_eq!(interface.output.max_burst, 7);
    for (offset, value) in [(12, 1), (14, 3), (15, 1), (16, 0x62), (20, 0x80), (21, 3)] {
        let mut unsupported = config.clone();
        unsupported[offset] = value;
        assert!(
            usb_bot::interface(&unsupported).is_none(),
            "accepted {offset}:{value}"
        );
    }
}

#[test]
fn malformed_and_truncated_descriptors_are_rejected_without_partial_endpoint_pairs() {
    let config = config();
    for len in 0..config.len() {
        assert!(
            usb_bot::interface(&config[..len]).is_none(),
            "accepted prefix {len}"
        );
    }
    for (offset, value) in [(0, 0), (2, 200), (9, 0), (18, 99), (27, 31)] {
        let mut broken = config.clone();
        broken[offset] = value;
        assert!(usb_bot::interface(&broken).is_none());
    }
}

#[test]
fn command_wrapper_has_exact_wire_size_endianness_and_reserved_bytes() {
    let cbw = usb_bot::cbw(0x1234_abcd, true, 4096, &[0x88; 16]).unwrap();
    assert_eq!(&cbw[..4], b"USBC");
    assert_eq!(&cbw[4..8], &[0xcd, 0xab, 0x34, 0x12]);
    assert_eq!(&cbw[8..12], &[0, 0x10, 0, 0]);
    assert_eq!(&cbw[12..15], &[0x80, 0, 16]);
    assert_eq!(&cbw[15..], &[0x88; 16]);
    assert!(usb_bot::cbw(0, false, 0, &[]).is_err());
    assert!(usb_bot::cbw(0, false, 0, &[0; 17]).is_err());
}

fn status() -> [u8; 13] {
    let mut out = [0; 13];
    out[..4].copy_from_slice(b"USBS");
    out[4..8].copy_from_slice(&17u32.to_le_bytes());
    out
}

#[test]
fn status_never_accepts_stale_tags_residue_partial_data_or_phase_errors() {
    let status = status();
    assert_eq!(usb_bot::csw(&status, 17, 512, 512), Ok(()));
    assert!(usb_bot::csw(&status[..12], 17, 512, 512).is_err());
    assert!(usb_bot::csw(&status, 18, 512, 512).is_err());
    assert!(usb_bot::csw(&status, 17, 512, 511).is_err());
    let mut residue = status;
    residue[8..12].copy_from_slice(&513u32.to_le_bytes());
    assert!(usb_bot::csw(&residue, 17, 512, 512).is_err());
    residue[8..12].copy_from_slice(&1u32.to_le_bytes());
    assert!(usb_bot::csw(&residue, 17, 512, 512).is_err());
    let mut failed = status;
    failed[12] = 1;
    assert_eq!(
        usb_bot::csw(&failed, 17, 512, 512),
        Err(usb_bot::COMMAND_FAILED)
    );
    for code in 2..=255 {
        failed[12] = code;
        assert!(usb_bot::csw(&failed, 17, 512, 512).is_err());
    }
}

#[test]
fn capacity_conversion_is_checked_and_preserves_native_block_geometry() {
    assert_eq!(usb_bot::capacity(127, 512), Ok(128));
    assert_eq!(usb_bot::capacity(127, 4096), Ok(1024));
    assert!(usb_bot::capacity(u64::MAX, 512).is_err());
    assert!(usb_bot::capacity(u64::MAX / 8, 4096).is_err());
    for size in [0, 256, 520, 1024, 2048, 8192] {
        assert!(usb_bot::capacity(100, size).is_err());
    }
}

#[test]
fn read_write_commands_use_ten_byte_scsi_until_the_lba_requires_sixteen() {
    let (read, len) = usb_bot::rw_cdb(u64::from(u32::MAX), false);
    assert_eq!(len, 10);
    assert_eq!(read[0], 0x28);
    assert_eq!(&read[2..6], &[255; 4]);
    assert_eq!(&read[7..9], &[0, 1]);
    let (write, len) = usb_bot::rw_cdb(0x1_0000_0000, true);
    assert_eq!(len, 16);
    assert_eq!(write[0], 0x8a);
    assert_eq!(&write[2..10], &0x1_0000_0000u64.to_be_bytes());
    assert_eq!(&write[10..14], &[0, 0, 0, 1]);
}
