//! Minimal freestanding implementations of the handful of C library
//! symbols that LLVM's optimizer can emit calls to even in a `no_std`,
//! `no_main`, no-libc UEFI binary.
//!
//! At `opt-level = "s"` and above, LLVM's loop-idiom-recognition pass will
//! rewrite a hand-written "scan forward until a zero code unit" loop --
//! which is exactly what `uefi::CStr16` and several protocols in the
//! `uefi` crate use internally to find a UCS-2 string's length -- into a
//! call to the platform's `wcslen`. That is a correct and desirable
//! optimization on a hosted target where libc provides `wcslen`, but this
//! is a freestanding UEFI application with no libc linked in at all, so
//! the call is left dangling and the linker fails with "undefined symbol:
//! wcslen". This is not a bug in our code or in the `uefi` crate; it is an
//! inherent gap in bare-metal no_std targets that don't opt out of this
//! class of optimization. See CHANGES.md for the full account.
//!
//! The fix used here -- providing our own trivial definition of the
//! missing symbol -- is the standard remedy for this class of problem in
//! bare-metal Rust projects, and keeps the release profile's normal
//! optimization settings (including size optimization) intact rather than
//! papering over the symptom by disabling optimization wholesale.

/// Count the number of `u16` code units before a null terminator, matching
/// the C `wchar_t` semantics LLVM assumes when it recognizes this idiom for
/// UCS-2/UTF-16-ish null-terminated strings.
///
/// # Safety
/// `s` must point to a valid, null-terminated array of `u16` values.
#[no_mangle]
pub unsafe extern "C" fn wcslen(mut s: *const u16) -> usize {
    let start = s;
    while *s != 0 {
        s = s.add(1);
    }
    s.offset_from(start) as usize
}
