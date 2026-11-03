//! Compile real PCI discovery helpers without performing host PCI I/O.
extern crate alloc;

#[allow(dead_code)]
mod paging {
    pub enum PageFlags {
        Mmio,
    }
    pub unsafe fn map(_: u64, _: u64, _: u64, _: PageFlags) -> Result<(), &'static str> {
        panic!("hardware mapping must not run in a PCI parser test")
    }
}

#[allow(dead_code)]
mod sync {
    pub struct Locked<T>(core::cell::UnsafeCell<T>);
    unsafe impl<T> Sync for Locked<T> {}
    impl<T> Locked<T> {
        pub const fn new(value: T) -> Self {
            Self(core::cell::UnsafeCell::new(value))
        }
        pub fn with<R>(&self, f: impl FnOnce(&mut T) -> R) -> R {
            unsafe { f(&mut *self.0.get()) }
        }
    }
    pub fn without_interrupts<R>(f: impl FnOnce() -> R) -> R {
        f()
    }
}

#[allow(dead_code)]
mod debugcon {
    pub fn write_str(_: &str) {}
    pub fn write_fmt(_: core::fmt::Arguments<'_>) -> Result<(), core::fmt::Error> {
        Ok(())
    }
}

#[allow(dead_code)]
#[path = "../../../src/pci.rs"]
mod pci;
