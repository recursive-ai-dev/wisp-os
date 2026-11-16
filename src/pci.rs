//! PCI discovery through legacy configuration ports and Intel client VMD.
//!
//! Scan firmware-assigned buses without changing bridge numbering or resource
//! assignments. VMD devices expose another configuration domain through BAR0;
//! its ECAM address uses a bus number relative to VMD's configured bus start.
//! Device drivers use `memory_bar` so passthrough shadow translations are also
//! respected. Interrupt-disabled accesses serialize CF8/CFC and VMD's posted
//! configuration writes are read back before returning.
//!
//! Register references:
//! https://edc.intel.com/content/www/us/en/design/publications/12th-generation-core-processor-datasheet-volume-2-of-2/vmd-configuration-base-address-cfgbar-0-14-0-pci-offset-10/
//! https://edc.intel.com/content/www/us/en/design/publications/core-ultra-p200s-series-processors-cfg-mem-registers/cfgbarsz-cfgbarsz-0-14-0-pci-offset-4c/
//! https://codebrowser.dev/linux/linux/drivers/pci/controller/vmd.c.html

use alloc::vec::Vec;
use core::arch::asm;

use crate::{paging, sync};

const CONFIG_ADDRESS: u16 = 0xcf8;
const CONFIG_DATA: u16 = 0xcfc;
const MAX_DEVICES: usize = 512;
const MAX_VMD_DOMAINS: usize = 4;
static INVENTORY: sync::Locked<Option<Vec<PciDevice>>> = sync::Locked::new(None);

fn out32(port: u16, value: u32) {
    unsafe {
        asm!("out dx, eax", in("dx") port, in("eax") value, options(nomem, nostack, preserves_flags));
    }
}

fn in32(port: u16) -> u32 {
    let value: u32;
    unsafe {
        asm!("in eax, dx", in("dx") port, out("eax") value, options(nomem, nostack, preserves_flags));
    }
    value
}

fn out16(port: u16, value: u16) {
    unsafe {
        asm!("out dx, ax", in("dx") port, in("ax") value, options(nomem, nostack, preserves_flags));
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct MemoryWindow {
    cpu: u64,
    bus: u64,
    bytes: u64,
}

impl MemoryWindow {
    fn translate(self, address: u64) -> Option<u64> {
        let offset = address.checked_sub(self.bus)?;
        (offset < self.bytes)
            .then(|| self.cpu.checked_add(offset))
            .flatten()
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
struct VmdConfig {
    base: u64,
    bus_start: u8,
    bus_count: u16,
    domain: u8,
    memory: [MemoryWindow; 2],
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ConfigSpace {
    Legacy,
    Vmd(VmdConfig),
}

/// PCI location plus its configuration domain. Domain metadata is private so
/// MMIO configuration accesses only target apertures mapped during discovery.
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct PciAddress {
    pub bus: u8,
    pub device: u8,
    pub function: u8,
    space: ConfigSpace,
}

fn config_address(addr: PciAddress, offset: u8) -> u32 {
    (1u32 << 31)
        | ((addr.bus as u32) << 16)
        | ((addr.device as u32) << 11)
        | ((addr.function as u32) << 8)
        | ((offset as u32) & 0xfc)
}

fn ecam_address(addr: PciAddress, offset: u8) -> Option<u64> {
    let ConfigSpace::Vmd(config) = addr.space else {
        return None;
    };
    let relative_bus = addr.bus.checked_sub(config.bus_start)? as u16;
    if relative_bus >= config.bus_count || addr.device >= 32 || addr.function >= 8 {
        return None;
    }
    config.base.checked_add(
        ((relative_bus as u64) << 20)
            | ((addr.device as u64) << 15)
            | ((addr.function as u64) << 12)
            | offset as u64,
    )
}

pub fn config_read32(addr: PciAddress, offset: u8) -> u32 {
    sync::without_interrupts(|| match addr.space {
        ConfigSpace::Legacy => {
            out32(CONFIG_ADDRESS, config_address(addr, offset));
            in32(CONFIG_DATA)
        }
        ConfigSpace::Vmd(_) => ecam_address(addr, offset & !3)
            .map_or(u32::MAX, |physical| unsafe {
                (physical as *const u32).read_volatile()
            }),
    })
}

#[allow(dead_code)] // Kept beside the halfword API for device register access.
pub fn config_write32(addr: PciAddress, offset: u8, value: u32) {
    sync::without_interrupts(|| match addr.space {
        ConfigSpace::Legacy => {
            out32(CONFIG_ADDRESS, config_address(addr, offset));
            out32(CONFIG_DATA, value);
        }
        ConfigSpace::Vmd(_) => {
            if let Some(physical) = ecam_address(addr, offset & !3) {
                unsafe {
                    (physical as *mut u32).write_volatile(value);
                    let _ = (physical as *const u32).read_volatile();
                }
            }
        }
    });
}

pub fn config_read16(addr: PciAddress, offset: u8) -> u16 {
    ((config_read32(addr, offset) >> ((offset & 2) * 8)) & 0xffff) as u16
}

/// Use an actual halfword write. A dword read/modify/write of Command would
/// replay Status's write-one-to-clear bits in its upper half.
pub fn config_write16(addr: PciAddress, offset: u8, value: u16) {
    sync::without_interrupts(|| match addr.space {
        ConfigSpace::Legacy => {
            out32(CONFIG_ADDRESS, config_address(addr, offset));
            out16(CONFIG_DATA + (offset as u16 & 2), value);
        }
        ConfigSpace::Vmd(_) => {
            if let Some(physical) = ecam_address(addr, offset & !1) {
                unsafe {
                    (physical as *mut u16).write_volatile(value);
                    let _ = (physical as *const u16).read_volatile();
                }
            }
        }
    });
}

#[derive(Clone, Copy)]
pub struct PciDevice {
    pub address: PciAddress,
    pub vendor_id: u16,
    pub device_id: u16,
    pub class: u8,
    pub subclass: u8,
    pub prog_if: u8,
    pub header_type: u8,
}

fn probe_function(
    addr: PciAddress,
    read: &mut impl FnMut(PciAddress, u8) -> u32,
) -> Option<(PciDevice, bool)> {
    let identity = read(addr, 0);
    let vendor_id = identity as u16;
    if vendor_id == 0xffff || vendor_id == 0 {
        return None;
    }
    let class = read(addr, 8);
    let header = (read(addr, 0x0c) >> 16) as u8;
    Some((
        PciDevice {
            address: addr,
            vendor_id,
            device_id: (identity >> 16) as u16,
            class: (class >> 24) as u8,
            subclass: (class >> 16) as u8,
            prog_if: (class >> 8) as u8,
            header_type: header & 0x7f,
        },
        header & 0x80 != 0,
    ))
}

fn scan_space(
    found: &mut Vec<PciDevice>,
    space: ConfigSpace,
    first: u8,
    count: u16,
    read: &mut impl FnMut(PciAddress, u8) -> u32,
) {
    for number in first as u16..(first as u16 + count).min(256) {
        for device in 0..32 {
            if found.len() == MAX_DEVICES {
                return;
            }
            let address = PciAddress {
                bus: number as u8,
                device,
                function: 0,
                space,
            };
            let Some((dev, multiple)) = probe_function(address, read) else {
                continue;
            };
            found.push(dev);
            if multiple {
                for function in 1..8 {
                    if found.len() == MAX_DEVICES {
                        return;
                    }
                    if let Some((dev, _)) = probe_function(
                        PciAddress {
                            function,
                            ..address
                        },
                        read,
                    ) {
                        found.push(dev);
                    }
                }
            }
        }
    }
}

/// Kept for callers explicitly interested in the host root bus alone.
pub fn enumerate_bus0() -> Vec<PciDevice> {
    let mut devices = Vec::new();
    scan_space(&mut devices, ConfigSpace::Legacy, 0, 1, &mut config_read32);
    devices
}

fn is_client_vmd(dev: PciDevice) -> bool {
    dev.vendor_id == 0x8086
        && dev.class == 1
        && dev.subclass == 4
        && matches!(
            dev.device_id,
            0x467f | 0x4c3d | 0xa77f | 0x7d0b | 0xad0b | 0x9a0b | 0xb60b | 0xb06f | 0xb07f
        )
}

fn vmd_bus_start(capability: u32, configuration: u32) -> Result<u8, &'static str> {
    if capability & 1 == 0 {
        return Ok(0);
    }
    match (configuration >> 8) & 3 {
        0 => Ok(0),
        1 => Ok(128),
        2 => Ok(224),
        _ => Err("pci: unsupported VMD bus restriction"),
    }
}

fn vmd_sizes(value: u32) -> Result<(u64, [u64; 2]), &'static str> {
    let cfg = value as u8;
    let mem1 = (value >> 8) as u8;
    let mem2 = (value >> 16) as u8;
    // BIOS programs the lowest writable address bit, so the aperture size
    // is 2^value. Reject disabled/uninitialized and implausible apertures.
    if !(20..=28).contains(&cfg) || !(12..=47).contains(&mem1) || !(12..=47).contains(&mem2) {
        return Err("pci: VMD aperture sizes are not configured");
    }
    Ok((1u64 << cfg, [1u64 << mem1, 1u64 << mem2]))
}

fn raw_memory_bar(addr: PciAddress, index: u8) -> Result<u64, &'static str> {
    if index >= 6 {
        return Err("pci: invalid BAR index");
    }
    let low = read_bar(addr, index);
    let high = if low & 6 == 4 {
        if index == 5 {
            return Err("pci: incomplete 64-bit BAR");
        }
        read_bar(addr, index + 1)
    } else {
        0
    };
    decode_memory_bar(low, high)
}

fn decode_memory_bar(low: u32, high: u32) -> Result<u64, &'static str> {
    if low & 1 != 0 {
        return Err("pci: BAR describes I/O ports rather than memory");
    }
    let address = match (low >> 1) & 3 {
        0 => (low & !15) as u64,
        2 => ((high as u64) << 32) | ((low & !15) as u64),
        _ => return Err("pci: unsupported memory BAR type"),
    };
    if address == 0 || low == u32::MAX {
        return Err("pci: memory BAR is unassigned");
    }
    Ok(address)
}

/// Resolve a firmware-assigned memory BAR to a CPU physical address.
/// Supports ordinary 32-/64-bit BARs and VMD's translated memory windows.
pub fn memory_bar(addr: PciAddress, index: u8) -> Result<u64, &'static str> {
    let physical = raw_memory_bar(addr, index)?;
    match addr.space {
        ConfigSpace::Legacy => Ok(physical),
        ConfigSpace::Vmd(config) => config
            .memory
            .iter()
            .find_map(|window| window.translate(physical))
            .ok_or("pci: BAR lies outside the VMD memory apertures"),
    }
}

fn vmd_shadow_bases(addr: PciAddress) -> Option<[u64; 2]> {
    if config_read16(addr, 6) & (1 << 4) == 0 {
        return None;
    }
    let mut pos = (config_read32(addr, 0x34) & 0xfc) as u8;
    // A bounded walk also rejects cycles in a malformed capability list.
    for _ in 0..48 {
        if !(0x40..=0xfc).contains(&pos) {
            return None;
        }
        let header = config_read32(addr, pos);
        if header as u8 == 9 && pos <= 0xe8 && config_read32(addr, pos + 4) == 0x5348_4457 {
            let first = (config_read32(addr, pos + 8) as u64)
                | ((config_read32(addr, pos + 12) as u64) << 32);
            let second = (config_read32(addr, pos + 16) as u64)
                | ((config_read32(addr, pos + 20) as u64) << 32);
            return Some([first & !15, second & !15]);
        }
        pos = ((header >> 8) as u8) & !3;
    }
    None
}

fn open_vmd(dev: PciDevice, domain: u8) -> Result<VmdConfig, &'static str> {
    let addr = dev.address;
    let base = raw_memory_bar(addr, 0)?;
    let (cfg_bytes, memory_bytes) = vmd_sizes(config_read32(addr, 0x4c))?;
    let bus_start = vmd_bus_start(config_read32(addr, 0x40), config_read32(addr, 0x44))?;
    let buses = cfg_bytes >> 20;
    if bus_start as u64 + buses > 256 || base & (cfg_bytes - 1) != 0 {
        return Err("pci: invalid VMD configuration aperture");
    }
    let cpu = [raw_memory_bar(addr, 2)?, raw_memory_bar(addr, 4)?];
    let bus = vmd_shadow_bases(addr).unwrap_or(cpu);
    let memory = [
        MemoryWindow {
            cpu: cpu[0],
            bus: bus[0],
            bytes: memory_bytes[0],
        },
        MemoryWindow {
            cpu: cpu[1],
            bus: bus[1],
            bytes: memory_bytes[1],
        },
    ];
    for window in memory {
        if window
            .cpu
            .checked_add(window.bytes)
            .is_none_or(|end| end > (1u64 << 47))
            || window.bus.checked_add(window.bytes).is_none()
        {
            return Err("pci: VMD memory aperture overflows address space");
        }
    }
    if base
        .checked_add(cfg_bytes)
        .is_none_or(|end| end > (1u64 << 47))
    {
        return Err("pci: VMD configuration aperture overflows address space");
    }
    // Firmware assigned the windows; enable decode without relocating them,
    // resetting bridges or probing BAR sizes with temporary all-ones writes.
    enable_bus_master_and_memory_space(addr);
    unsafe {
        paging::map(base, base, cfg_bytes, paging::PageFlags::Mmio)?;
    }
    let _ = crate::debugcon::write_fmt(format_args!(
        "[vmd] domain {} cfg=0x{:x} buses={:02x}-{:02x} ({} MiB)\n",
        domain,
        base,
        bus_start,
        bus_start as u16 + buses as u16 - 1,
        cfg_bytes >> 20
    ));
    Ok(VmdConfig {
        base,
        bus_start,
        bus_count: buses as u16,
        domain,
        memory,
    })
}

/// Discover all conventional buses and supported Intel VMD domains. Assigned
/// downstream devices remain visible even when the firmware chose sparse bus
/// numbering. Devices beyond the bounded inventory are reported and skipped.
/// Call only after the kernel's page tables have been loaded.
pub fn enumerate_devices() -> Vec<PciDevice> {
    if let Some(devices) = INVENTORY.with(|cached| cached.clone()) {
        return devices;
    }
    let mut devices = enumerate_bus0();
    scan_space(
        &mut devices,
        ConfigSpace::Legacy,
        1,
        255,
        &mut config_read32,
    );
    let roots: Vec<_> = devices
        .iter()
        .copied()
        .filter(|dev| is_client_vmd(*dev))
        .take(MAX_VMD_DOMAINS)
        .collect();
    for (index, root) in roots.into_iter().enumerate() {
        match open_vmd(root, (index + 1) as u8) {
            Ok(config) => scan_space(
                &mut devices,
                ConfigSpace::Vmd(config),
                config.bus_start,
                config.bus_count,
                &mut config_read32,
            ),
            Err(message) => {
                let _ = crate::debugcon::write_fmt(format_args!(
                    "[vmd] {:02x}:{:02x}.{} unavailable: {}\n",
                    root.address.bus, root.address.device, root.address.function, message
                ));
            }
        }
    }
    if devices.len() == MAX_DEVICES {
        crate::debugcon::write_str("[pci] inventory limit reached; further devices skipped\n");
    }
    INVENTORY.with(|cached| *cached = Some(devices.clone()));
    devices
}

pub fn dump_devices(devices: &[PciDevice]) {
    crate::debugcon::write_str("[pci] devices found:\n");
    for dev in devices {
        let _ = crate::debugcon::write_fmt(format_args!(
            "[pci]   {:02x}:{:02x}.{:x}  vendor:device={:04x}:{:04x}  class:subclass:prog_if={:02x}:{:02x}:{:02x}  header_type={:02x}\n",
            dev.address.bus, dev.address.device, dev.address.function, dev.vendor_id, dev.device_id,
            dev.class, dev.subclass, dev.prog_if, dev.header_type));
        if let ConfigSpace::Vmd(config) = dev.address.space {
            let _ = crate::debugcon::write_fmt(format_args!(
                "[pci]     via VMD domain {}\n",
                config.domain
            ));
        }
    }
}

/// Snapshot the discovered PCI inventory for `lspci`.
pub fn cached_devices() -> Vec<PciDevice> {
    INVENTORY.with(|cached| cached.clone()).unwrap_or_default()
}

/// Return the Intel VMD domain index if `addr` sits behind a VMD controller.
pub fn vmd_domain(addr: PciAddress) -> Option<u8> {
    match addr.space {
        ConfigSpace::Legacy => None,
        ConfigSpace::Vmd(cfg) => Some(cfg.domain),
    }
}

/// Human-readable description for common PCI class/subclass/prog_if codes.
pub const fn class_description(class: u8, subclass: u8, prog_if: u8) -> &'static str {
    match (class, subclass, prog_if) {
        (0x01, 0x01, _) => "IDE storage controller",
        (0x01, 0x04, _) => "RAID / VMD storage controller",
        (0x01, 0x06, 0x01) => "SATA AHCI controller",
        (0x01, 0x06, _) => "SATA controller",
        (0x01, 0x08, 0x02) => "NVMe solid-state controller",
        (0x01, 0x08, _) => "Non-Volatile Memory controller",
        (0x01, _, _) => "Mass storage controller",
        (0x02, _, _) => "Network controller (unclaimed by design)",
        (0x03, 0x00, _) => "VGA compatible display controller",
        (0x03, 0x80, _) => "Display controller",
        (0x03, _, _) => "Graphics controller",
        (0x04, 0x03, _) => "HD Audio controller",
        (0x04, _, _) => "Multimedia controller",
        (0x06, 0x00, _) => "Host bridge",
        (0x06, 0x01, _) => "ISA bridge",
        (0x06, 0x04, _) => "PCI-to-PCI bridge",
        (0x06, _, _) => "Bridge device",
        (0x0c, 0x03, 0x00) => "USB UHCI controller",
        (0x0c, 0x03, 0x10) => "USB OHCI controller",
        (0x0c, 0x03, 0x20) => "USB EHCI controller",
        (0x0c, 0x03, 0x30) => "USB xHCI controller",
        (0x0c, 0x03, _) => "USB controller",
        (0x0c, 0x05, _) => "SMBus controller",
        _ => "PCI device",
    }
}

pub fn find_ahci_controller(devices: &[PciDevice]) -> Option<PciDevice> {
    devices
        .iter()
        .copied()
        .find(|d| d.class == 1 && d.subclass == 6 && d.prog_if == 1)
}

pub fn enable_bus_master_and_memory_space(addr: PciAddress) {
    let command = config_read16(addr, 4);
    config_write16(addr, 4, command | 2 | 4);
}

fn interrupt_changes(mut read: impl FnMut(u8) -> u32) -> Result<Vec<(u8, u16)>, &'static str> {
    let command_status = read(4);
    let mut changes = Vec::new();
    if command_status & (1 << 20) != 0 {
        let mut position = (read(0x34) & 0xff) as u8;
        let mut visited = 0u64;
        while position != 0 {
            if position < 0x40 || position & 3 != 0 {
                return Err("pci: invalid interrupt capability pointer");
            }
            let bit = 1u64 << (position / 4);
            if visited & bit != 0 {
                return Err("pci: interrupt capability list contains a cycle");
            }
            visited |= bit;
            let header = read(position);
            let control = (header >> 16) as u16;
            match header as u8 {
                // PCI MSI Message Control: Enable is bit 0.
                5 => changes.push((position + 2, control & !1)),
                // PCI MSI-X Message Control: Function Mask is bit 14,
                // Enable is bit 15. Preserve table size and other fields.
                0x11 => changes.push((position + 2, (control | (1 << 14)) & !(1 << 15))),
                _ => {}
            }
            position = (header >> 8) as u8;
        }
    }
    changes.push((4, command_status as u16 | (1 << 10)));
    Ok(changes)
}

/// Quiesce firmware interrupt routing before a polling driver takes over.
/// Mask MSI/MSI-X and legacy INTx; per-device interrupt mask registers alone
/// do not necessarily affect MSI-X. Validate the whole bounded capability
/// chain before changing any register, and use halfword writes to preserve
/// adjacent write-one-to-clear PCI Status fields.
pub fn disable_interrupts(addr: PciAddress) -> Result<(), &'static str> {
    for (offset, value) in interrupt_changes(|offset| config_read32(addr, offset))? {
        config_write16(addr, offset, value);
    }
    Ok(())
}

pub fn read_bar(addr: PciAddress, index: u8) -> u32 {
    debug_assert!(index < 6, "PCI BAR index must be 0-5");
    config_read32(addr, 0x10 + index * 4)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn address(bus: u8, device: u8, function: u8) -> PciAddress {
        PciAddress {
            bus,
            device,
            function,
            space: ConfigSpace::Legacy,
        }
    }

    fn vmd() -> VmdConfig {
        VmdConfig {
            base: 0x6202_000000,
            bus_start: 224,
            bus_count: 32,
            domain: 1,
            memory: [
                MemoryWindow {
                    cpu: 0x5200_0000,
                    bus: 0x5200_0000,
                    bytes: 32 << 20,
                },
                MemoryWindow {
                    cpu: 0x6205_100000,
                    bus: 0x6205_100000,
                    bytes: 1 << 20,
                },
            ],
        }
    }

    #[test]
    fn sparse_downstream_and_multifunction_devices_are_found_without_bus_writes() {
        let mut reads = Vec::new();
        let mut read = |addr: PciAddress, offset| {
            reads.push((addr.bus, addr.device, addr.function, offset));
            match (addr.bus, addr.device, addr.function, offset) {
                (0, 2, 0, 0) => 0x1234_8086,
                (0, 2, 0, 0x0c) => 0x0080_0000,
                (0, 2, 4, 0) => 0x5678_8086,
                (200, 5, 0, 0) => 0x0010_1b36,
                (200, 5, 0, 8) => 0x0108_0200,
                (_, _, _, 0) => u32::MAX,
                _ => 0,
            }
        };
        let mut devices = Vec::new();
        scan_space(&mut devices, ConfigSpace::Legacy, 0, 256, &mut read);
        assert_eq!(devices.len(), 3);
        assert_eq!(devices[1].address.function, 4);
        assert_eq!(devices[2].address.bus, 200);
        assert_eq!(
            (devices[2].class, devices[2].subclass, devices[2].prog_if),
            (1, 8, 2)
        );
        assert!(!reads
            .iter()
            .any(|&(bus, dev, function, _)| bus == 200 && dev == 5 && function != 0));
    }

    #[test]
    fn vmd_ecam_uses_relative_bus_and_checks_aperture_bounds() {
        let mut addr = address(225, 3, 2);
        addr.space = ConfigSpace::Vmd(vmd());
        assert_eq!(
            ecam_address(addr, 0x44),
            Some(0x6202_000000 + (1 << 20) + (3 << 15) + (2 << 12) + 0x44)
        );
        addr.bus = 223;
        assert_eq!(ecam_address(addr, 0), None);
        addr.bus = 255;
        assert!(ecam_address(addr, 0xfc).is_some());
        let ConfigSpace::Vmd(mut cfg) = addr.space else {
            unreachable!()
        };
        cfg.bus_count = 31;
        addr.space = ConfigSpace::Vmd(cfg);
        assert_eq!(ecam_address(addr, 0), None);
        addr.bus = 224;
        addr.device = 32;
        assert_eq!(ecam_address(addr, 0), None);
    }

    #[test]
    fn vmd_bus_offsets_and_firmware_sizes_are_validated() {
        assert_eq!(vmd_bus_start(0, 0x300), Ok(0));
        assert_eq!(vmd_bus_start(1, 0), Ok(0));
        assert_eq!(vmd_bus_start(1, 0x100), Ok(128));
        assert_eq!(vmd_bus_start(1, 0x200), Ok(224));
        assert!(vmd_bus_start(1, 0x300).is_err());
        assert_eq!(
            vmd_sizes(25 | (25 << 8) | (20 << 16)),
            Ok((32 << 20, [32 << 20, 1 << 20]))
        );
        assert!(vmd_sizes(0).is_err());
        assert!(vmd_sizes(29 | (25 << 8) | (20 << 16)).is_err());
    }

    #[test]
    fn bar_decoding_keeps_all_64_bits_and_rejects_unsupported_layouts() {
        assert_eq!(decode_memory_bar(0x8100_0000, 0), Ok(0x8100_0000));
        assert_eq!(decode_memory_bar(0x0200_000c, 0x62), Ok(0x6202_000000));
        assert!(decode_memory_bar(0x81, 0).is_err());
        assert!(decode_memory_bar(2, 0).is_err());
        assert!(decode_memory_bar(6, 0).is_err());
        assert!(decode_memory_bar(0, 0).is_err());
    }

    #[test]
    fn vmd_shadow_translation_preserves_offset_and_rejects_outside_addresses() {
        let window = MemoryWindow {
            cpu: 0x6200_000000,
            bus: 0x1000_0000,
            bytes: 1 << 20,
        };
        assert_eq!(window.translate(0x1000_4000), Some(0x6200_004000));
        assert_eq!(window.translate(0x100f_ffff), Some(0x6200_0fffff));
        assert_eq!(window.translate(0x0fff_ffff), None);
        assert_eq!(window.translate(0x1010_0000), None);
    }

    #[test]
    fn configuration_address_cannot_overlap_device_bits() {
        assert_eq!(config_address(address(200, 31, 7), 0x13), 0x80c8_ff10);
    }

    #[test]
    fn polled_takeover_masks_msi_msix_and_intx_without_replaying_status() {
        let changes = interrupt_changes(|offset| match offset {
            4 => (1 << 20) | 7,
            0x34 => 0x50,
            0x50 => 0x0081_6015, // unrelated capability, next 0x60
            0x60 => 0x0181_7005, // MSI enabled, preserve other control bits
            0x70 => 0x8007_0011, // MSI-X enabled, table size 7
            _ => panic!("unexpected configuration offset {offset:x}"),
        })
        .unwrap();
        assert_eq!(changes, [(0x62, 0x180), (0x72, 0x4007), (4, 0x407)]);
    }

    #[test]
    fn malformed_interrupt_capability_chains_are_rejected_before_writes() {
        assert!(interrupt_changes(|offset| match offset {
            4 => 1 << 20,
            0x34 => 0x40,
            0x40 => 0x0001_4005,
            _ => unreachable!(),
        })
        .is_err());
        assert!(interrupt_changes(|offset| match offset {
            4 => 1 << 20,
            0x34 => 0x41,
            _ => unreachable!(),
        })
        .is_err());
        assert_eq!(
            interrupt_changes(|offset| {
                assert_eq!(offset, 4);
                3
            })
            .unwrap(),
            [(4, 0x403)]
        );
    }
}
