//! Hardware-independent validation of the NVM namespace's Identify page.

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Namespace {
    pub blocks: u64,
    pub block_size: u32,
    pub total_sectors: u64,
}

pub fn namespace(page: &[u8; 4096]) -> Result<Namespace, &'static str> {
    let blocks = u64::from_le_bytes(page[0..8].try_into().unwrap());
    let capacity = u64::from_le_bytes(page[8..16].try_into().unwrap());
    if blocks == 0 || capacity == 0 || capacity > blocks {
        return Err("nvme: namespace has no usable capacity");
    }
    // This driver implements the 16 LBA formats defined by NVMe 1.4.
    // Reject extended format selectors instead of interpreting the wrong one.
    if page[26] & 0xe0 != 0 {
        return Err("nvme: extended LBA format selectors are unsupported");
    }
    let format = (page[26] & 0x0f) as usize;
    if format > usize::from(page[25]) {
        return Err("nvme: namespace selects an invalid LBA format");
    }
    let offset = 128 + format * 4;
    if u16::from_le_bytes([page[offset], page[offset + 1]]) != 0 || page[29] & 7 != 0 {
        return Err("nvme: namespace metadata/protection information is unsupported");
    }
    let block_size = match page[offset + 2] {
        9 => 512,
        12 => 4096,
        _ => return Err("nvme: namespace requires 512-byte or 4096-byte logical blocks"),
    };
    let total_sectors = blocks
        .checked_mul(u64::from(block_size / 512))
        .ok_or("nvme: namespace capacity overflows the sector address space")?;
    Ok(Namespace {
        blocks,
        block_size,
        total_sectors,
    })
}
