//! Persistent disk handles backed by xHCI bulk-only SCSI devices.
//! Controller access is mainline-only and never waits with IRQs disabled.

use crate::usb_hid;
use alloc::string::String;
use alloc::vec::Vec;

#[derive(Clone)]
pub struct DiskInfo {
    pub total_sectors: u64,
    pub(crate) block_size: u32,
    pub(crate) model: String,
    pub(crate) serial: String,
}

impl DiskInfo {
    pub fn model(&self) -> &str {
        &self.model
    }
    pub fn serial(&self) -> &str {
        &self.serial
    }
}

pub struct UsbDisk {
    index: usize,
    info: DiskInfo,
}

pub fn disks() -> Vec<UsbDisk> {
    usb_hid::with_controller(|controller| {
        controller
            .storage_infos()
            .into_iter()
            .enumerate()
            .map(|(index, info)| UsbDisk { index, info })
            .collect()
    })
    .unwrap_or_default()
}

impl UsbDisk {
    pub fn info(&self) -> Option<&DiskInfo> {
        Some(&self.info)
    }
    pub fn index(&self) -> usize {
        self.index
    }
    pub fn logical_block_size(&self) -> u32 {
        self.info.block_size
    }
    pub fn is_failed(&self) -> bool {
        usb_hid::with_controller(|controller| controller.storage_failed(self.index)).unwrap_or(true)
    }
    pub unsafe fn read_sector(&self, lba: u64, out: &mut [u8; 512]) -> Result<(), &'static str> {
        usb_hid::with_controller(|controller| unsafe {
            controller.storage_read(self.index, lba, out)
        })?
    }
    pub unsafe fn write_sector(&self, lba: u64, data: &[u8; 512]) -> Result<(), &'static str> {
        usb_hid::with_controller(|controller| unsafe {
            controller.storage_write(self.index, lba, data)
        })?
    }
    pub unsafe fn flush_cache(&self) -> Result<(), &'static str> {
        usb_hid::with_controller(|controller| unsafe { controller.storage_flush(self.index) })?
    }
}
