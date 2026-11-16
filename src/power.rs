//! System reset, power-off, and halt primitives (`reboot`, `poweroff`, `halt`).
//!
//! Primary mechanism: UEFI Runtime Services `ResetSystem`, whose table pointer
//! is preserved across `exit_boot_services` and whose code (`RUNTIME_SERVICES_CODE`)
//! and data (`RUNTIME_SERVICES_DATA`) regions are identity-mapped with W^X
//! permissions (`RX` and `RW+NX`) in `paging::init`.
//!
//! Fallbacks for `reboot` if `ResetSystem` is unavailable or returns:
//! 1. ACPI FADT reset register (`RESET_REG` / `RESET_VALUE`), if advertised;
//! 2. 8042 PS/2 controller reset line pulse (`0xFE` to port `0x64`);
//! 3. Deliberate triple fault via a zero-length IDT and `int3`.

use core::arch::asm;
use core::sync::atomic::{AtomicBool, AtomicU64, AtomicU8, Ordering};

use uefi::table::runtime::ResetType;
use uefi::table::{Runtime, SystemTable};
use uefi::Status;

use crate::{debugcon, sync};

static RUNTIME_ST: sync::Locked<Option<SystemTable<Runtime>>> = sync::Locked::new(None);

static FADT_RESET_VALID: AtomicBool = AtomicBool::new(false);
static FADT_RESET_IS_IO: AtomicBool = AtomicBool::new(false);
static FADT_RESET_ADDR: AtomicU64 = AtomicU64::new(0);
static FADT_RESET_VAL: AtomicU8 = AtomicU8::new(0);

/// Preserve the post-`exit_boot_services` UEFI Runtime System Table.
pub fn init(runtime_st: SystemTable<Runtime>) {
    RUNTIME_ST.with(|slot| {
        *slot = Some(runtime_st);
    });
}

/// Record an AML-free ACPI FADT reset register discovered during `acpi::parse`.
pub fn register_fadt_reset(is_io: bool, address: u64, value: u8) {
    FADT_RESET_IS_IO.store(is_io, Ordering::Relaxed);
    FADT_RESET_ADDR.store(address, Ordering::Relaxed);
    FADT_RESET_VAL.store(value, Ordering::Relaxed);
    FADT_RESET_VALID.store(true, Ordering::Release);
}

#[inline]
fn out8(port: u16, val: u8) {
    unsafe {
        asm!(
            "out dx, al",
            in("dx") port,
            in("al") val,
            options(nomem, nostack, preserves_flags),
        );
    }
}

#[inline]
fn in8(port: u16) -> u8 {
    let val: u8;
    unsafe {
        asm!(
            "in al, dx",
            in("dx") port,
            out("al") val,
            options(nomem, nostack, preserves_flags),
        );
    }
    val
}

fn try_uefi_reset(reset_type: ResetType) {
    let st_opt = RUNTIME_ST.with(|slot| slot.take());
    if let Some(st) = st_opt {
        let rt = unsafe { st.runtime_services() };
        rt.reset(reset_type, Status::SUCCESS, None);
    }
}

fn try_fadt_reset() {
    if !FADT_RESET_VALID.load(Ordering::Acquire) {
        return;
    }
    let addr = FADT_RESET_ADDR.load(Ordering::Relaxed);
    let val = FADT_RESET_VAL.load(Ordering::Relaxed);
    if FADT_RESET_IS_IO.load(Ordering::Relaxed) {
        if let Ok(port) = u16::try_from(addr) {
            debugcon::write_str("[power] attempting ACPI FADT I/O reset\n");
            out8(port, val);
        }
    } else if addr != 0 {
        debugcon::write_str("[power] attempting ACPI FADT MMIO reset\n");
        unsafe {
            (addr as *mut u8).write_volatile(val);
        }
    }
}

fn try_8042_reset() {
    debugcon::write_str("[power] attempting 8042 keyboard-controller reset (0xFE -> 0x64)\n");
    for _ in 0..10_000 {
        if in8(0x64) & 0x02 == 0 {
            break;
        }
        core::hint::spin_loop();
    }
    out8(0x64, 0xFE);
    for _ in 0..100_000 {
        core::hint::spin_loop();
    }
}

#[repr(C, packed)]
struct NullIdtr {
    limit: u16,
    base: u64,
}

fn triple_fault() -> ! {
    debugcon::write_str("[power] forcing triple-fault reset\n");
    let null_idtr = NullIdtr { limit: 0, base: 0 };
    unsafe {
        asm!(
            "cli",
            "lidt [{}]",
            "int3",
            in(reg) &null_idtr,
            options(noreturn, nostack),
        );
    }
}

/// Cold-reset the machine (`ResetSystem(Cold)` -> FADT -> 8042 -> triple fault).
pub fn reboot() -> ! {
    debugcon::write_str("[power] reboot requested\n");
    try_uefi_reset(ResetType::COLD);
    try_fadt_reset();
    try_8042_reset();
    triple_fault()
}

/// Request a firmware power-off via `ResetSystem(Shutdown)`.
/// Returns if the firmware does not honour the shutdown request so the shell
/// can report that the machine remains running.
pub fn poweroff() {
    debugcon::write_str("[power] poweroff requested\n");
    try_uefi_reset(ResetType::SHUTDOWN);
    debugcon::write_str("[power] firmware ResetSystem(Shutdown) returned\n");
}

/// Disable interrupts and halt the CPU forever.
pub fn halt() -> ! {
    debugcon::write_str("[power] system halted\n");
    loop {
        unsafe {
            asm!("cli", "hlt", options(nomem, nostack));
        }
    }
}
