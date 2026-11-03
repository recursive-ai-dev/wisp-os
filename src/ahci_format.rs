//! ATA geometry checks shared by IDENTIFY and sector submission.

const LBA48_SECTORS: u64 = 1 << 48;

pub fn capacity(words: &[u16; 256]) -> Result<u64, &'static str> {
    let sectors = u64::from(words[100])
        | (u64::from(words[101]) << 16)
        | (u64::from(words[102]) << 32)
        | (u64::from(words[103]) << 48);
    if sectors == 0 || sectors > LBA48_SECTORS {
        return Err("ahci: disk capacity is zero or exceeds LBA48 addressing");
    }
    Ok(sectors)
}

pub fn validate_sector(total_sectors: u64, lba: u64) -> Result<(), &'static str> {
    if total_sectors == 0 || total_sectors > LBA48_SECTORS || lba >= total_sectors {
        return Err("ahci: sector is outside the disk's LBA48 address range");
    }
    Ok(())
}
