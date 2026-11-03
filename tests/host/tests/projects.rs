//! Project naming and source persistence through the real save format.

use std::rc::Rc;

use wisp_host_tests::ahci::Hba;
use wisp_host_tests::projects::{self, MAX_SAVED_SOURCE_BYTES};
use wisp_host_tests::save_format::{self, SaveDisk, MAX_KEYS};
use wisp_host_tests::wisp::value::Value;

fn disk() -> SaveDisk {
    let hba = Hba::new(2048);
    let total = hba.total_sectors();
    unsafe { save_format::provision(&hba, total) }.unwrap();
    SaveDisk::new(hba, total, true)
}

fn templates() -> Vec<(String, String)> {
    vec![("HELLO.WISP".into(), "print(\"bundled\");\n".into())]
}

#[test]
fn names_are_canonical_and_fit_the_store_key() {
    assert_eq!(
        projects::normalize_name(" Hello-2_test.WiSp ").unwrap(),
        "hello-2_test"
    );
    let longest = "a".repeat(27);
    assert_eq!(
        projects::normalize_name(&format!("{longest}.wisp")).unwrap(),
        longest
    );
    for invalid in [
        "",
        ".wisp",
        "hello world",
        "APPS/hello",
        "..",
        "a.b",
        "ééé",
        "λtest",
        "a\0b",
    ] {
        assert!(
            projects::normalize_name(invalid).is_err(),
            "accepted {invalid:?}"
        );
    }
    assert!(projects::normalize_name(&"a".repeat(28)).is_err());
}

#[test]
fn saved_source_wins_and_survives_a_reboot() {
    let disk = disk();
    let source = "# exact bytes, including tabs and CRLF\r\n\tprint(\"saved\");\r\n";
    assert_eq!(
        projects::load_source("Hello", &templates(), Some(&disk)).unwrap(),
        Some(templates()[0].1.clone())
    );
    projects::save_source("Hello.WISP", source, Some(&disk)).unwrap();
    assert_eq!(
        projects::load_source("HELLO", &templates(), Some(&disk))
            .unwrap()
            .as_deref(),
        Some(source)
    );
    assert!(disk.hba().pending().is_empty(), "save was not flushed");
    let hba = Hba::from_media(disk.hba().media());
    let total = hba.total_sectors();
    assert_eq!(
        unsafe { save_format::probe(&hba, total) }.unwrap(),
        save_format::Probe::Formatted
    );
    let reopened = SaveDisk::new(hba, total, true);
    assert_eq!(
        projects::load_source("hello", &[], Some(&reopened))
            .unwrap()
            .as_deref(),
        Some(source)
    );
}

#[test]
fn missing_projects_are_distinct_from_store_errors() {
    assert_eq!(
        projects::load_source("new", &templates(), None).unwrap(),
        None
    );
    assert_eq!(
        projects::load_source("hello", &templates(), None).unwrap(),
        Some(templates()[0].1.clone())
    );
    let unmounted = SaveDisk::new(Hba::new(2048), 2048, false);
    assert_eq!(
        projects::load_source("hello", &templates(), Some(&unmounted)).unwrap(),
        Some(templates()[0].1.clone())
    );
    assert!(projects::save_source("new", "", Some(&unmounted))
        .unwrap_err()
        .contains("not formatted"));
    assert!(projects::save_source("new", "", None)
        .unwrap_err()
        .contains("no save disk selected"));
    // This intentionally invalid mounted handle makes the real format reader fail.
    let broken = SaveDisk::new(Hba::new(1), 1, true);
    assert!(projects::load_source("hello", &templates(), Some(&broken)).is_err());
    assert!(projects::list(Some(&broken)).is_err());
}

#[test]
fn exact_slot_limit_is_in_bytes_and_oversize_never_writes() {
    let disk = disk();
    assert_eq!(MAX_SAVED_SOURCE_BYTES, 4042);
    let exact = "é".repeat(MAX_SAVED_SOURCE_BYTES / 2);
    projects::save_source("limit", &exact, Some(&disk)).unwrap();
    assert_eq!(
        projects::load_source("limit", &[], Some(&disk))
            .unwrap()
            .as_deref(),
        Some(exact.as_str())
    );
    let writes = disk.hba().writes.get();
    assert!(projects::save_source("limit", &(exact.clone() + "x"), Some(&disk)).is_err());
    assert_eq!(disk.hba().writes.get(), writes);
    assert_eq!(
        projects::load_source("limit", &[], Some(&disk))
            .unwrap()
            .as_deref(),
        Some(exact.as_str())
    );
}

#[test]
fn empty_source_is_a_real_project() {
    let disk = disk();
    projects::save_source("hello", "", Some(&disk)).unwrap();
    assert_eq!(
        projects::load_source("hello", &templates(), Some(&disk)).unwrap(),
        Some(String::new())
    );
    assert_eq!(projects::list(Some(&disk)).unwrap(), ["hello"]);
}

#[test]
fn wrong_value_type_does_not_fall_back_to_the_template() {
    let disk = disk();
    disk.save("prog.hello", &Value::Int(42)).unwrap();
    assert!(projects::load_source("hello", &templates(), Some(&disk))
        .unwrap_err()
        .contains("non-string"));
}

#[test]
fn listing_filters_nonproject_and_noncanonical_keys() {
    let disk = disk();
    projects::save_source("zeta", "", Some(&disk)).unwrap();
    projects::save_source("Alpha", "", Some(&disk)).unwrap();
    for key in [
        "score",
        "prog.",
        "prog.a.b",
        "prog.UPPER",
        "prog.name.wisp",
        "progs.hello",
    ] {
        disk.save(key, &Value::Str(Rc::new("other".into())))
            .unwrap();
    }
    assert_eq!(projects::list(Some(&disk)).unwrap(), ["alpha", "zeta"]);
    assert!(projects::list(None).unwrap().is_empty());
    assert!(
        projects::list(Some(&SaveDisk::new(Hba::new(2048), 2048, false)))
            .unwrap()
            .is_empty()
    );
}

#[test]
fn shared_store_capacity_errors_leave_other_values_intact() {
    let disk = disk();
    for i in 0..MAX_KEYS {
        disk.save(&format!("data{i}"), &Value::Int(i as i64))
            .unwrap();
    }
    assert!(projects::save_source("hello", "print(1);", Some(&disk))
        .unwrap_err()
        .contains("full"));
    assert_eq!(disk.keys().unwrap().len(), MAX_KEYS);
    assert_eq!(
        projects::load_source("hello", &[], Some(&disk)).unwrap(),
        None
    );
    assert_eq!(
        disk.load("data0").unwrap().unwrap().to_display_string(),
        "0"
    );
}

#[test]
fn invalid_names_cannot_be_used_to_write_arbitrary_store_keys() {
    let disk = disk();
    let writes = disk.hba().writes.get();
    assert!(projects::save_source("../score", "", Some(&disk)).is_err());
    assert!(projects::load_source("../score", &[], Some(&disk)).is_err());
    assert_eq!(disk.hba().writes.get(), writes);
}
