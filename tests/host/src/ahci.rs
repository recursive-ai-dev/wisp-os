//! Stand-in for the kernel's AHCI driver: an in-memory disk with the one
//! property that matters for durability tests -- a volatile write cache.
//! Writes land in the cache; only `flush_cache` moves them to "media";
//! `power_loss` throws the cache away. Faults can be injected per write
//! and per flush.

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;

pub const SECTOR_SIZE: u32 = 512;
type Sector = [u8; SECTOR_SIZE as usize];

pub struct Hba {
    media: RefCell<Vec<Sector>>,
    cache: RefCell<BTreeMap<u64, Sector>>,
    /// Fail the write with this 0-based index (counting every write).
    pub fail_write_at: Cell<Option<usize>>,
    /// Fail every flush while set.
    pub fail_flush: Cell<bool>,
    pub writes: Cell<usize>,
    pub reads: Cell<usize>,
    pub flushes: Cell<usize>,
}

impl Hba {
    pub fn new(sectors: usize) -> Self {
        Self {
            media: RefCell::new(vec![[0u8; SECTOR_SIZE as usize]; sectors]),
            cache: RefCell::new(BTreeMap::new()),
            fail_write_at: Cell::new(None),
            fail_flush: Cell::new(false),
            writes: Cell::new(0),
            reads: Cell::new(0),
            flushes: Cell::new(0),
        }
    }

    pub fn total_sectors(&self) -> u64 {
        self.media.borrow().len() as u64
    }

    /// Write straight to media, bypassing the cache and counters -- for
    /// setting up a disk's initial contents.
    pub fn seed(&self, lba: u64, data: &Sector) {
        self.media.borrow_mut()[lba as usize] = *data;
    }

    /// Everything durably on media.
    pub fn media(&self) -> Vec<Sector> {
        self.media.borrow().clone()
    }

    /// Lose every write that was not flushed.
    pub fn power_loss(&self) {
        self.cache.borrow_mut().clear();
    }

    pub fn is_failed(&self) -> bool {
        false
    }

    pub unsafe fn read_sector(&self, lba: u64, out: &mut Sector) -> Result<(), &'static str> {
        self.reads.set(self.reads.get() + 1);
        let media = self.media.borrow();
        let sector = self
            .cache
            .borrow()
            .get(&lba)
            .copied()
            .or_else(|| media.get(lba as usize).copied())
            .ok_or("mock: read past end of disk")?;
        *out = sector;
        Ok(())
    }

    pub unsafe fn write_sector(&self, lba: u64, data: &Sector) -> Result<(), &'static str> {
        let n = self.writes.get();
        self.writes.set(n + 1);
        if self.fail_write_at.get() == Some(n) {
            return Err("mock: injected write failure");
        }
        if lba >= self.total_sectors() {
            return Err("mock: write past end of disk");
        }
        self.cache.borrow_mut().insert(lba, *data);
        Ok(())
    }

    pub unsafe fn flush_cache(&self) -> Result<(), &'static str> {
        self.flushes.set(self.flushes.get() + 1);
        if self.fail_flush.get() {
            return Err("mock: injected flush failure");
        }
        let mut media = self.media.borrow_mut();
        for (lba, data) in std::mem::take(&mut *self.cache.borrow_mut()) {
            media[lba as usize] = data;
        }
        Ok(())
    }
}
