use wisp_host_tests::storage_policy::{automatic_selection, Candidate};

const FOREIGN: Candidate = Candidate {
    mounted: false,
    format_allowed: false,
    failed: false,
};
const BLANK: Candidate = Candidate {
    mounted: false,
    format_allowed: true,
    failed: false,
};
const MOUNTED: Candidate = Candidate {
    mounted: true,
    format_allowed: true,
    failed: false,
};

#[test]
fn existing_store_wins_over_other_transports_and_foreign_disks() {
    for index in 0..3 {
        let mut disks = [FOREIGN, BLANK, FOREIGN];
        disks[index] = MOUNTED;
        assert_eq!(automatic_selection(&disks), Some(index));
    }
}

#[test]
fn ambiguous_stores_or_format_targets_require_an_explicit_choice() {
    assert_eq!(automatic_selection(&[MOUNTED, MOUNTED]), None);
    assert_eq!(automatic_selection(&[BLANK, FOREIGN, BLANK]), None);
    assert_eq!(automatic_selection(&[FOREIGN, BLANK]), Some(1));
    assert_eq!(automatic_selection(&[FOREIGN]), None);
    assert_eq!(automatic_selection(&[]), None);
}

#[test]
fn failed_devices_are_never_selected() {
    let failed = Candidate {
        failed: true,
        ..MOUNTED
    };
    assert_eq!(automatic_selection(&[failed, MOUNTED]), Some(1));
    assert_eq!(automatic_selection(&[failed]), None);
    assert_eq!(automatic_selection(&[failed, BLANK]), Some(1));
}
