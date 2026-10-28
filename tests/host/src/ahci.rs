//! Stand-in for the kernel's AHCI driver: an in-memory disk with the one
//! property that matters for durability tests -- a volatile write cache.
//! Writes land in the cache; only `flush_cache` moves them to "media";
//! `power_loss` throws the cache away, and `pending` exposes it so a test
//! can build the disk image for *every* subset of it reaching the media --
//! which is what a real write-back cache may do: anything not yet flushed
//! can be persisted, or not, in any order. Faults can be injected per write, per flush, or as a device
//! that stops responding after a given number of operations.

use std::cell::{Cell, RefCell};
use std::collections::BTreeMap;

pub const SECTOR_SIZE: u32 = 512;
type Sector = [u8; SECTOR_SIZE as usize];

pub struct DiskInfo {
    pub total_sectors: u64,
}

pub struct Hba {
    media: RefCell<Vec<Sector>>,
    cache: RefCell<BTreeMap<u64, Sector>>,
    /// Fail the write with this 0-based index (counting every write).
    pub fail_write_at: Cell<Option<usize>>,
    /// Fail every flush while set.
    pub fail_flush: Cell<bool>,
    /// Fail every write and flush once this many of them (counted together
    /// in `ops`) have been issued: a device that dies mid-operation.
    pub fail_after_ops: Cell<Option<usize>>,
    pub writes: Cell<usize>,
    pub reads: Cell<usize>,
    pub flushes: Cell<usize>,
    /// Writes and flushes issued, in order -- the clock `fail_after_ops`
    /// runs on.
    pub ops: Cell<usize>,
}

impl Hba {
    pub fn new(sectors: usize) -> Self {
        Self::from_media(vec![[0u8; SECTOR_SIZE as usize]; sectors])
    }

    /// A disk whose media already holds `media` (and whose cache is empty).
    pub fn from_media(media: Vec<Sector>) -> Self {
        Self {
            media: RefCell::new(media),
            cache: RefCell::new(BTreeMap::new()),
            fail_write_at: Cell::new(None),
            fail_flush: Cell::new(false),
            fail_after_ops: Cell::new(None),
            writes: Cell::new(0),
            reads: Cell::new(0),
            flushes: Cell::new(0),
            ops: Cell::new(0),
        }
    }

    pub fn info(&self) -> Option<DiskInfo> {
        Some(DiskInfo { total_sectors: self.total_sectors() })
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

    /// Every write in the cache that has not been flushed, by LBA: what a
    /// power loss right now may or may not persist, in any combination.
    pub fn pending(&self) -> Vec<(u64, Sector)> {
        self.cache.borrow().iter().map(|(&lba, data)| (lba, *data)).collect()
    }

    /// Lose every write that was not flushed.
    pub fn power_loss(&self) {
        self.cache.borrow_mut().clear();
    }

    pub fn is_failed(&self) -> bool {
        false
    }

    /// Count one write/flush; `Err` if the device has died.
    fn op(&self) -> Result<(), &'static str> {
        let n = self.ops.get();
        self.ops.set(n + 1);
        if self.fail_after_ops.get().is_some_and(|limit| n >= limit) {
            return Err("mock: device stopped responding");
        }
        Ok(())
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
        self.op()?;
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
        self.op()?;
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
