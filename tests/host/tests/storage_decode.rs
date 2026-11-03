//! Untrusted saved-value lengths must not amplify allocation before rejection.
use std::alloc::{GlobalAlloc, Layout, System};
use std::cell::Cell;
use wisp_host_tests::{ahci::Hba, save_format};

struct Counting;
thread_local! {
    static TRACK: Cell<bool> = const { Cell::new(false) };
    static ALLOCATED: Cell<usize> = const { Cell::new(0) };
    static FAIL_AT_SIZE: Cell<usize> = const { Cell::new(0) };
    static FAILED: Cell<bool> = const { Cell::new(false) };
}
unsafe impl GlobalAlloc for Counting {
    unsafe fn alloc(&self, layout: Layout) -> *mut u8 {
        let reject = FAIL_AT_SIZE
            .try_with(|limit| {
                if limit.get() != 0 && layout.size() >= limit.get() {
                    limit.set(0);
                    let _ = FAILED.try_with(|failed| failed.set(true));
                    true
                } else {
                    false
                }
            })
            .unwrap_or(false);
        if reject {
            return std::ptr::null_mut();
        }
        let pointer = unsafe { System.alloc(layout) };
        if !pointer.is_null() && TRACK.try_with(Cell::get).unwrap_or(false) {
            let _ = ALLOCATED.try_with(|bytes| bytes.set(bytes.get() + layout.size()));
        }
        pointer
    }
    unsafe fn dealloc(&self, pointer: *mut u8, layout: Layout) {
        unsafe { System.dealloc(pointer, layout) };
    }
}
#[global_allocator]
static ALLOCATOR: Counting = Counting;

fn encoded_disk(encoded: &[u8]) -> Hba {
    let hba = Hba::new(2048);
    unsafe { save_format::provision(&hba, 2048) }.unwrap();
    let mut slot = [0u8; 4096];
    slot[0] = 2;
    slot[1..5].copy_from_slice(b"test");
    slot[33..37].copy_from_slice(&(encoded.len() as u32).to_le_bytes());
    slot[41..49].copy_from_slice(&1u64.to_le_bytes());
    slot[49..49 + encoded.len()].copy_from_slice(encoded);
    let checksum = slot[..37]
        .iter()
        .chain(&slot[41..49 + encoded.len()])
        .fold(0x811c_9dc5u32, |hash, byte| {
            (hash ^ u32::from(*byte)).wrapping_mul(0x0100_0193)
        });
    slot[37..41].copy_from_slice(&checksum.to_le_bytes());
    for (offset, sector) in slot.chunks_exact(512).enumerate() {
        hba.seed(1 + offset as u64, sector.try_into().unwrap());
    }
    hba
}

fn rejected_with_allocation_budget(encoded: &[u8], budget: usize) {
    let hba = encoded_disk(encoded);
    ALLOCATED.with(|bytes| bytes.set(0));
    TRACK.with(|track| track.set(true));
    let result = unsafe { save_format::load(&hba, "test") };
    TRACK.with(|track| track.set(false));
    let allocated = ALLOCATED.with(Cell::get);
    eprintln!(
        "decode allocation: {} encoded bytes, {allocated} allocated bytes",
        encoded.len()
    );
    assert!(result.is_err(), "malformed length was accepted");
    assert!(
        allocated <= budget,
        "{allocated} bytes allocated for {} input bytes; budget {budget}",
        encoded.len()
    );
}

#[test]
fn tiny_truncated_containers_do_not_reserve_their_claimed_capacity() {
    for tag in [4, 5] {
        for count in [1u32, 4000, u32::MAX] {
            let mut encoded = vec![tag];
            encoded.extend_from_slice(&count.to_le_bytes());
            rejected_with_allocation_budget(&encoded, 32 * 1024);
        }
    }
}

#[test]
fn nested_claims_are_allocated_only_as_elements_are_decoded() {
    let mut encoded = Vec::new();
    for _ in 0..63 {
        encoded.push(4);
        encoded.extend_from_slice(&256u32.to_le_bytes());
    }
    // Each count individually fits the remaining byte count. Only the
    // innermost list has its promised elements; outer lists are truncated.
    encoded.extend_from_slice(&[0; 256]);
    rejected_with_allocation_budget(&encoded, 64 * 1024);
}

#[test]
fn valid_empty_and_dense_containers_remain_readable() {
    for encoded in [vec![4, 0, 0, 0, 0], vec![5, 0, 0, 0, 0], {
        let mut bytes = vec![4];
        bytes.extend_from_slice(&4000u32.to_le_bytes());
        bytes.resize(4005, 0);
        bytes
    }] {
        let hba = encoded_disk(&encoded);
        let loaded = unsafe { save_format::load(&hba, "test") }.unwrap().unwrap();
        assert_eq!(save_format::encode(&loaded).unwrap(), encoded);
    }
}

#[test]
fn list_allocation_failure_returns_an_error_and_allows_a_later_read() {
    let mut encoded = vec![4];
    encoded.extend_from_slice(&4000u32.to_le_bytes());
    encoded.resize(4005, 0);
    let hba = encoded_disk(&encoded);
    for _ in 0..4 {
        FAILED.with(|failed| failed.set(false));
        // Slot/header allocations fit below this threshold; fail only a
        // decoded list's growth, one time, to model heap fragmentation.
        FAIL_AT_SIZE.with(|limit| limit.set(8192));
        let result = unsafe { save_format::load(&hba, "test") };
        FAIL_AT_SIZE.with(|limit| limit.set(0));
        assert!(FAILED.with(Cell::get), "allocator fault did not fire");
        assert_eq!(
            result.unwrap_err(),
            "save_format: out of memory decoding a stored list"
        );
        let loaded = unsafe { save_format::load(&hba, "test") }.unwrap().unwrap();
        assert_eq!(save_format::encode(&loaded).unwrap(), encoded);
    }
}
