//! Pure USB Bulk-Only Transport wire-format helpers.
//! Reference: https://www.usb.org/sites/default/files/usbmassbulk_10.pdf
//! Sections 3.2, 4.3, 5.1 and 5.2. All BOT integer fields are little endian;
//! SCSI CDBs and capacity responses use big endian.

#[derive(Clone, Copy, Debug)]
pub struct BulkEndpoint {
    pub address: u8,
    pub packet_size: u16,
    pub max_burst: u8,
}

#[derive(Clone, Copy, Debug)]
pub struct Interface {
    pub configuration: u8,
    pub number: u8,
    pub input: BulkEndpoint,
    pub output: BulkEndpoint,
}

pub fn interface(config: &[u8]) -> Option<Interface> {
    if config.len() < 9 || config[0] < 9 || config[1] != 2 {
        return None;
    }
    let total = usize::from(u16::from_le_bytes([config[2], config[3]]));
    if total > config.len() || total < 9 {
        return None;
    }
    let mut offset = 0;
    let mut number = None;
    let mut input = None;
    let mut output = None;
    let mut last_input = None;
    while offset + 2 <= total {
        let len = usize::from(config[offset]);
        if len < 2 || offset + len > total {
            return None;
        }
        let d = &config[offset..offset + len];
        match d[1] {
            4 if len >= 9 => {
                if let (Some(number), Some(input), Some(output)) = (number, input, output) {
                    return Some(Interface {
                        configuration: config[5],
                        number,
                        input,
                        output,
                    });
                }
                number = (d[3] == 0 && d[5] == 8 && d[6] == 6 && d[7] == 0x50).then_some(d[2]);
                input = None;
                output = None;
                last_input = None;
            }
            5 if len >= 7 && number.is_some() && d[3] & 3 == 2 => {
                let packet_size = u16::from_le_bytes([d[4], d[5]]) & 0x7ff;
                if d[2] & 0xf == 0 || packet_size == 0 || packet_size > 1024 {
                    return None;
                }
                let endpoint = BulkEndpoint {
                    address: d[2],
                    packet_size,
                    max_burst: 0,
                };
                last_input = Some(d[2] & 0x80 != 0);
                if d[2] & 0x80 != 0 {
                    input = Some(endpoint);
                } else {
                    output = Some(endpoint);
                }
            }
            0x30 if len >= 6 => {
                // SuperSpeed streams are deliberately disabled; only burst size
                // is required for a normal bulk endpoint context.
                if d[2] > 15 {
                    return None;
                }
                match last_input {
                    Some(true) => {
                        if let Some(ref mut ep) = input {
                            ep.max_burst = d[2];
                        }
                    }
                    Some(false) => {
                        if let Some(ref mut ep) = output {
                            ep.max_burst = d[2];
                        }
                    }
                    None => {}
                }
            }
            _ => {
                last_input = None;
            }
        }
        offset += len;
    }
    match (number, input, output) {
        (Some(number), Some(input), Some(output)) => Some(Interface {
            configuration: config[5],
            number,
            input,
            output,
        }),
        _ => None,
    }
}

pub fn cbw(tag: u32, input: bool, length: u32, cdb: &[u8]) -> Result<[u8; 31], &'static str> {
    if cdb.is_empty() || cdb.len() > 16 {
        return Err("usb-storage: invalid SCSI command length");
    }
    let mut out = [0; 31];
    out[..4].copy_from_slice(&0x4342_5355u32.to_le_bytes());
    out[4..8].copy_from_slice(&tag.to_le_bytes());
    out[8..12].copy_from_slice(&length.to_le_bytes());
    out[12] = if input { 0x80 } else { 0 };
    out[14] = cdb.len() as u8;
    out[15..15 + cdb.len()].copy_from_slice(cdb);
    Ok(out)
}

pub const COMMAND_FAILED: &str = "usb-storage: SCSI command failed";

pub fn csw(bytes: &[u8], tag: u32, requested: u32, transferred: u32) -> Result<(), &'static str> {
    if bytes.len() != 13 || bytes[..4] != 0x5342_5355u32.to_le_bytes() {
        return Err("usb-storage: invalid command status wrapper");
    }
    if u32::from_le_bytes(bytes[4..8].try_into().unwrap()) != tag {
        return Err("usb-storage: command status tag does not match");
    }
    let residue = u32::from_le_bytes(bytes[8..12].try_into().unwrap());
    if residue > requested || transferred > requested {
        return Err("usb-storage: invalid command residue");
    }
    match bytes[12] {
        0 if residue == 0 && transferred == requested => Ok(()),
        0 => Err("usb-storage: command completed with incomplete data"),
        1 => Err(COMMAND_FAILED),
        _ => Err("usb-storage: command phase error"),
    }
}

pub fn capacity(last_block: u64, block_size: u32) -> Result<u64, &'static str> {
    if !matches!(block_size, 512 | 4096) {
        return Err("usb-storage: only 512-byte and 4096-byte blocks are supported");
    }
    last_block
        .checked_add(1)
        .and_then(|n| n.checked_mul(u64::from(block_size / 512)))
        .ok_or("usb-storage: capacity overflow")
}

pub fn rw_cdb(lba: u64, write: bool) -> ([u8; 16], usize) {
    let mut cdb = [0; 16];
    if let Ok(lba) = u32::try_from(lba) {
        cdb[0] = if write { 0x2a } else { 0x28 };
        cdb[2..6].copy_from_slice(&lba.to_be_bytes());
        cdb[7..9].copy_from_slice(&1u16.to_be_bytes());
        (cdb, 10)
    } else {
        cdb[0] = if write { 0x8a } else { 0x88 };
        cdb[2..10].copy_from_slice(&lba.to_be_bytes());
        cdb[10..14].copy_from_slice(&1u32.to_be_bytes());
        (cdb, 16)
    }
}
