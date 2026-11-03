use wisp_host_tests::ahci::Hba;
use wisp_host_tests::save_format::{self, SaveDisk};
use wisp_host_tests::wisp::value::Value;

#[test]
fn native_4kn_store_can_be_read_but_never_rewritten_with_512_byte_slots() {
    let hba = Hba::new(2048);
    unsafe { save_format::provision(&hba, 2048) }.unwrap();
    unsafe { save_format::save(&hba, "existing", &Value::Int(42)) }.unwrap();
    let writes = hba.writes.get();
    let before = hba.media();
    hba.native_block_size.set(4096);
    assert!(matches!(
        unsafe { save_format::load(&hba, "existing") }.unwrap(),
        Some(Value::Int(42))
    ));
    assert!(unsafe { save_format::save(&hba, "existing", &Value::Int(43)) }.is_err());
    assert!(unsafe { save_format::delete(&hba, "existing") }.is_err());
    assert!(unsafe { save_format::provision(&hba, 2048) }.is_err());
    assert_eq!(hba.writes.get(), writes);
    assert_eq!(hba.media(), before);
    let mut disk = SaveDisk::new(hba, 2048, true);
    assert!(disk.format_range().is_err());
    assert!(disk.format().is_err());
    assert_eq!(disk.hba().writes.get(), writes);
}

#[test]
fn native_gpt_header_is_detected_even_without_its_protective_mbr() {
    let hba = Hba::new(2048);
    let mut sector = [0; 512];
    sector[..8].copy_from_slice(b"EFI PART");
    hba.seed(8, &sector);
    hba.native_block_size.set(4096);
    let before = hba.media();
    assert!(unsafe { save_format::probe(&hba, 2048) }
        .unwrap_err()
        .contains("4Kn GPT"));
    assert!(unsafe { save_format::provision(&hba, 2048) }.is_err());
    assert_eq!(hba.writes.get(), 0);
    assert_eq!(hba.media(), before);
}
