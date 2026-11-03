//! Common sector interface for the native storage transports. All save
//! operations use 512-byte sectors; drivers translate native block sizes.

use alloc::format;
use alloc::string::{String, ToString};

use crate::{ahci, nvme, usb_storage};

pub const SECTOR_SIZE: u32 = 512;

pub struct DiskInfo {
    pub total_sectors: u64,
    model: String,
    serial: String,
}

impl DiskInfo {
    pub fn model(&self) -> &str {
        &self.model
    }
    pub fn serial(&self) -> &str {
        &self.serial
    }
}

enum Backend {
    Ahci(ahci::Hba),
    Nvme(nvme::Nvme),
    Usb(usb_storage::UsbDisk),
}

pub struct Disk {
    backend: Backend,
    info: DiskInfo,
    location: String,
}

impl Disk {
    pub fn from_ahci(hba: ahci::Hba) -> Result<Self, &'static str> {
        let info = hba.info().ok_or("AHCI disk has not been identified")?;
        let info = DiskInfo {
            total_sectors: info.total_sectors,
            model: info.model().to_string(),
            serial: info.serial().to_string(),
        };
        let location = format!("AHCI port {}", hba.port());
        Ok(Self {
            backend: Backend::Ahci(hba),
            info,
            location,
        })
    }

    pub fn from_nvme(disk: nvme::Nvme) -> Result<Self, &'static str> {
        let info = disk.info().ok_or("NVMe disk has not been identified")?;
        let info = DiskInfo {
            total_sectors: info.total_sectors,
            model: info.model().to_string(),
            serial: info.serial().to_string(),
        };
        let location = format!("NVMe namespace {}", disk.namespace_id());
        Ok(Self {
            backend: Backend::Nvme(disk),
            info,
            location,
        })
    }

    pub fn from_usb(disk: usb_storage::UsbDisk) -> Result<Self, &'static str> {
        let info = disk.info().ok_or("USB disk has not been identified")?;
        let info = DiskInfo {
            total_sectors: info.total_sectors,
            model: info.model().to_string(),
            serial: info.serial().to_string(),
        };
        let location = format!("USB mass storage {}", disk.index());
        Ok(Self {
            backend: Backend::Usb(disk),
            info,
            location,
        })
    }

    pub fn info(&self) -> Option<&DiskInfo> {
        Some(&self.info)
    }
    pub fn location(&self) -> &str {
        &self.location
    }

    pub fn transport(&self) -> &'static str {
        match self.backend {
            Backend::Ahci(_) => "AHCI",
            Backend::Nvme(_) => "NVMe",
            Backend::Usb(_) => "USB",
        }
    }

    pub fn native_sector_size(&self) -> u32 {
        match &self.backend {
            Backend::Ahci(_) => 512,
            Backend::Nvme(disk) => disk.logical_block_size(),
            Backend::Usb(disk) => disk.logical_block_size(),
        }
    }

    pub fn is_failed(&self) -> bool {
        match &self.backend {
            Backend::Ahci(disk) => disk.is_failed(),
            Backend::Nvme(disk) => disk.is_failed(),
            Backend::Usb(disk) => disk.is_failed(),
        }
    }

    /// The driver is initialized and exclusively owned by this disk.
    pub unsafe fn read_sector(&self, lba: u64, out: &mut [u8; 512]) -> Result<(), &'static str> {
        match &self.backend {
            Backend::Ahci(disk) => unsafe { disk.read_sector(lba, out) },
            Backend::Nvme(disk) => unsafe { disk.read_sector(lba, out) },
            Backend::Usb(disk) => unsafe { disk.read_sector(lba, out) },
        }
    }

    /// Writes require an explicit save/format operation; discovery only reads.
    pub unsafe fn write_sector(&self, lba: u64, data: &[u8; 512]) -> Result<(), &'static str> {
        match &self.backend {
            Backend::Ahci(disk) => unsafe { disk.write_sector(lba, data) },
            Backend::Nvme(disk) => unsafe { disk.write_sector(lba, data) },
            Backend::Usb(disk) => unsafe { disk.write_sector(lba, data) },
        }
    }

    pub unsafe fn flush_cache(&self) -> Result<(), &'static str> {
        match &self.backend {
            Backend::Ahci(disk) => unsafe { disk.flush_cache() },
            Backend::Nvme(disk) => unsafe { disk.flush_cache() },
            Backend::Usb(disk) => unsafe { disk.flush_cache() },
        }
    }
}
