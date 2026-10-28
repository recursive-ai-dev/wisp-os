//! Interactive live installer. Every firmware handle is dropped before
//! ExitBootServices; the running kernel never borrows a firmware protocol.
//! Only an explicitly confirmed whole disk is written. Its device path is
//! checked against the boot volume both when listed and immediately before
//! installation, and every generated sector is flushed and read back.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;
use core::fmt::Write;

use uefi::prelude::*;
use uefi::proto::console::text::{Key, ScanCode};
use uefi::proto::device_path::text::{AllowShortcuts, DisplayOnly};
use uefi::proto::device_path::DevicePath;
use uefi::proto::loaded_image::LoadedImage;
use uefi::proto::media::block::BlockIO;
use uefi::proto::rng::Rng;
use uefi::proto::ProtocolPointer;
use uefi::table::boot::{BootServices, OpenProtocolAttributes, OpenProtocolParams, ScopedProtocol};
use uefi::table::runtime::ResetType;

use crate::disk_layout::{self, InstallFile};
use crate::gfx::{self, Console};
use crate::line_editor::{self, Outcome};
use crate::{debugcon, fs};

const MAX_EFI_BYTES: usize = 2 * 1024 * 1024;
const MAX_TARGETS: usize = 32;

struct Target {
    handle: Handle,
    media_id: u32,
    sectors: u64,
    path: Vec<u8>,
    description: String,
}

// BlockIO can require alignment beyond a Rust byte array's alignment.
#[repr(align(4096))]
struct IoBuffer([u8; 512]);

fn inspect_protocol<P: ProtocolPointer + ?Sized>(
    bs: &BootServices,
    handle: Handle,
) -> uefi::Result<ScopedProtocol<'_, P>> {
    // SAFETY: queries are single-threaded, references stay local, and all
    // returned guards drop before any exclusive BlockIO open can detach
    // filesystem drivers. Enumeration must not disconnect those drivers:
    // an exclusive open just to read media metadata can remove child
    // handles, including the boot filesystem.
    unsafe {
        bs.open_protocol::<P>(
            OpenProtocolParams {
                handle,
                agent: bs.image_handle(),
                controller: None,
            },
            OpenProtocolAttributes::GetProtocol,
        )
    }
}

fn device_path(bs: &BootServices, handle: Handle) -> Option<Vec<u8>> {
    let path = inspect_protocol::<DevicePath>(bs, handle).ok()?;
    let bytes = path.as_bytes();
    // DevicePath ends with a four-byte End Entire node. An empty or
    // multi-instance path cannot establish a unique physical boot disk.
    if bytes.len() <= 4
        || bytes[bytes.len() - 4..] != [0x7f, 0xff, 4, 0]
        || path.instance_iter().count() != 1
    {
        return None;
    }
    Some(bytes[..bytes.len() - 4].to_vec())
}

fn boot_path(bs: &BootServices, image: Handle) -> Option<Vec<u8>> {
    let source = {
        let loaded = inspect_protocol::<LoadedImage>(bs, image).ok()?;
        loaded.device()?
    };
    device_path(bs, source)
}

fn is_source(source: &[u8], target: &[u8]) -> bool {
    // A partition's path extends its parent whole disk's path. Check both
    // directions so ambiguous parent handles are also excluded.
    source.starts_with(target) || target.starts_with(source)
}

fn installation_id(st: &SystemTable<Boot>, bs: &BootServices, target: &Target) -> [u8; 16] {
    let mut id = [0u8; 16];
    if let Ok(handle) = bs.get_handle_for_protocol::<Rng>() {
        if let Ok(mut rng) = bs.open_protocol_exclusive::<Rng>(handle) {
            if rng.get_rng(None, &mut id).is_ok() {
                return id;
            }
        }
    }
    // GUID uniqueness, not a security token. Mix firmware wall time,
    // physical device identity and the CPU counter when EFI RNG is absent.
    let counter = unsafe { core::arch::x86_64::_rdtsc() };
    let time = st.runtime_services().get_time().ok();
    let stamp = format!("{time:?}:{counter}:{}", target.sectors);
    for half in 0..2 {
        let mut hash = 0xcbf2_9ce4_8422_2325u64 ^ (half as u64).wrapping_mul(0x9e37_79b9_7f4a_7c15);
        for &byte in target.path.iter().chain(stamp.as_bytes()) {
            hash = (hash ^ u64::from(byte)).wrapping_mul(0x100_0000_01b3);
        }
        id[half * 8..half * 8 + 8].copy_from_slice(&hash.to_le_bytes());
    }
    id
}

fn targets(bs: &BootServices, source: &[u8]) -> Vec<Target> {
    let mut targets = Vec::new();
    let Ok(handles) = bs.find_handles::<BlockIO>() else {
        return targets;
    };
    for handle in handles {
        if targets.len() >= MAX_TARGETS {
            break;
        }
        let Some(path) = device_path(bs, handle) else {
            continue;
        };
        if is_source(source, &path) {
            continue;
        }
        let Ok(block) = inspect_protocol::<BlockIO>(bs, handle) else {
            continue;
        };
        let media = block.media();
        let Some(sectors) = media.last_block().checked_add(1) else {
            continue;
        };
        let align = media.io_align();
        if !media.is_media_present()
            || media.is_logical_partition()
            || media.is_read_only()
            || media.block_size() != 512
            || sectors < disk_layout::MIN_DISK_SECTORS
            || (align > 1 && (!align.is_power_of_two() || align > 4096))
        {
            continue;
        }
        let media_id = media.media_id();
        drop(block);
        let description = inspect_protocol::<DevicePath>(bs, handle)
            .ok()
            .and_then(|p| {
                p.to_string(bs, DisplayOnly(true), AllowShortcuts(false))
                    .ok()
            })
            .map(|p| format!("{p}"))
            .unwrap_or_else(|| String::from("UEFI block device"));
        targets.push(Target {
            handle,
            media_id,
            sectors,
            path,
            description,
        });
    }
    targets
}

/// Poll firmware input only while Boot Services is alive. Errors abort
/// the installer instead of leaving an erase confirmation loop spinning.
fn key(st: &mut SystemTable<Boot>) -> Result<Option<Key>, &'static str> {
    st.stdin()
        .read_key()
        .map_err(|_| "firmware keyboard unavailable")
}

fn read_line(
    st: &mut SystemTable<Boot>,
    bs: &BootServices,
    console: &mut Console,
) -> Option<String> {
    let outcome = line_editor::read_line(
        console,
        &mut || loop {
            let byte = match key(st) {
                Err(_) | Ok(Some(Key::Special(ScanCode::ESCAPE))) => 0x1b,
                Ok(Some(Key::Special(code))) => match code {
                    ScanCode::LEFT => line_editor::KEY_LEFT,
                    ScanCode::RIGHT => line_editor::KEY_RIGHT,
                    ScanCode::HOME => line_editor::KEY_HOME,
                    ScanCode::END => line_editor::KEY_END,
                    ScanCode::DELETE => line_editor::KEY_DELETE,
                    _ => 0,
                },
                Ok(Some(Key::Printable(ch))) => {
                    let ch = char::from(ch);
                    if ch == '\n' {
                        0x0d
                    } else if ch.is_ascii() {
                        ch as u8
                    } else {
                        0
                    }
                }
                _ => 0,
            };
            if byte != 0 {
                return byte;
            }
            bs.stall(10_000);
        },
        None,
        false,
    );
    console.newline();
    match outcome {
        Outcome::Line(line) => Some(line),
        Outcome::Escaped(_) => None,
    }
}

/// Show the startup choice for four seconds, defaulting to the live
/// environment. Selecting the installer disables the timer entirely.
pub fn boot_menu(
    st: &mut SystemTable<Boot>,
    bs: &BootServices,
    image: Handle,
    console: &mut Console,
    apps: &[(String, String)],
) {
    console.clear();
    console.set_fg(gfx::ACCENT);
    let _ = writeln!(console, "WISP OS {}", env!("CARGO_PKG_VERSION"));
    console.set_fg(gfx::FOREGROUND);
    console
        .write_str("\n[T] Try Wisp OS    [I] Install to disk\n\nStarting Wisp in 4 seconds...\n");
    debugcon::write_str("[installer] boot menu ready\n");
    for _ in 0..400 {
        match key(st) {
            Ok(Some(Key::Printable(ch))) => match char::from(ch) {
                'i' | 'I' => {
                    run(st, bs, image, console, apps);
                    break;
                }
                't' | 'T' | '\r' | '\n' => break,
                _ => {}
            },
            Ok(Some(Key::Special(ScanCode::ESCAPE))) | Err(_) => break,
            _ => {}
        }
        bs.stall(10_000);
    }
    // Do not leak firmware menu keystrokes into the native PS/2 path.
    let _ = st.stdin().reset(false);
    console.clear();
}

fn run(
    st: &mut SystemTable<Boot>,
    bs: &BootServices,
    image: Handle,
    console: &mut Console,
    apps: &[(String, String)],
) {
    console.clear();
    console.write_str("Install the running Wisp system\n\n");
    // Finish all source filesystem reads before enumerating targets or
    // opening a disk exclusively. Keep only owned bytes across that step.
    let efi = match fs::read_file_to_bytes(bs, image, "EFI/BOOT/BOOTX64.EFI", MAX_EFI_BYTES) {
        Ok(bytes) if bytes.len() >= 64 && bytes.starts_with(b"MZ") => bytes,
        result => {
            if let Err(error) = result {
                let _ = debugcon::write_fmt(format_args!(
                    "[installer] EFI snapshot error: {:?}\n",
                    error.status()
                ));
            }
            console.write_str("Cannot read the live EFI executable (maximum 2 MiB). No writes.\n");
            debugcon::write_str("[installer] source snapshot unavailable; no writes\n");
            bs.stall(3_000_000);
            return;
        }
    };
    let Some(source) = boot_path(bs, image) else {
        console.write_str("Cannot identify the boot media safely. Installation unavailable.\n");
        debugcon::write_str("[installer] source identity unavailable; no writes\n");
        bs.stall(3_000_000);
        return;
    };
    let available = targets(bs, &source);
    console.write_str("The boot media is excluded. Supported targets: whole disks,\n512-byte sectors, at least 80 MiB. NVMe/USB installation uses firmware.\n\n");
    for (index, disk) in available.iter().enumerate() {
        let _ = writeln!(
            console,
            "  {}: {} MiB  {}",
            index + 1,
            disk.sectors / 2048,
            disk.description
        );
    }
    if available.is_empty() {
        console.write_str("No eligible target disks. Returning to Wisp.\n");
        debugcon::write_str("[installer] no eligible targets; no writes\n");
        bs.stall(3_000_000);
        return;
    }
    console.write_str("\nDisk number (Enter/Esc cancels): ");
    debugcon::write_str("[installer] target prompt\n");
    let Some(answer) = read_line(st, bs, console) else {
        return;
    };
    let Some(index) = answer
        .trim()
        .parse::<usize>()
        .ok()
        .and_then(|n| n.checked_sub(1))
    else {
        debugcon::write_str("[installer] cancelled; no writes\n");
        return;
    };
    let Some(target) = available.get(index) else {
        console.write_str("Unknown disk number. Installation cancelled.\n");
        debugcon::write_str("[installer] invalid target; no writes\n");
        return;
    };
    console.set_fg(gfx::ERROR);
    let _ = writeln!(
        console,
        "\nERASE ALL DATA on disk {}: {} MiB\n{}",
        index + 1,
        target.sectors / 2048,
        target.description
    );
    console.write_str("Creates a 64 MiB boot partition and a separate WISP save partition.\n");
    let confirmation = format!("ERASE {}", index + 1);
    let _ = write!(console, "Type {confirmation} exactly to install: ");
    console.set_fg(gfx::FOREGROUND);
    debugcon::write_str("[installer] erase confirmation prompt\n");
    if read_line(st, bs, console).as_deref() != Some(confirmation.as_str()) {
        console.write_str("Cancelled. Nothing was written.\n");
        debugcon::write_str("[installer] cancelled; no writes\n");
        return;
    }
    console.write_str("\nCopying Wisp and apps, then verifying the installed data...\n");
    let id = installation_id(st, bs, target);
    match install(bs, image, target, apps, &efi, id) {
        Ok(()) => {
            console.set_fg(gfx::ACCENT);
            console.write_str("\nInstallation complete. All written sectors verified.\n");
            console.set_fg(gfx::FOREGROUND);
            console.write_str("Remove the live media and select the installed disk in firmware.\n\n[R] Reboot    [T/Enter] Continue this live session: ");
            debugcon::write_str("[installer] installation verified\n");
            if read_line(st, bs, console).is_some_and(|line| line.eq_ignore_ascii_case("r")) {
                st.runtime_services()
                    .reset(ResetType::COLD, Status::SUCCESS, None);
            }
        }
        Err(message) => {
            console.set_fg(gfx::ERROR);
            let _ = writeln!(console, "Installation failed: {message}\nThe target may contain a partial installation. Boot the live media to retry.");
            console.set_fg(gfx::FOREGROUND);
            let _ = debugcon::write_fmt(format_args!("[installer] failed: {message}\n"));
            console.write_str("Press Enter to continue this live session: ");
            let _ = read_line(st, bs, console);
        }
    }
}

fn install(
    bs: &BootServices,
    image: Handle,
    target: &Target,
    apps: &[(String, String)],
    efi: &[u8],
    id: [u8; 16],
) -> Result<(), &'static str> {
    let mut files = Vec::with_capacity(apps.len() + 1);
    files.push(InstallFile {
        name: "BOOTX64.EFI",
        data: efi,
    });
    for (name, data) in apps {
        files.push(InstallFile {
            name,
            data: data.as_bytes(),
        });
    }
    let source = boot_path(bs, image).ok_or("boot media identity changed")?;
    let current = device_path(bs, target.handle).ok_or("target identity unavailable")?;
    if current != target.path || is_source(&source, &current) {
        return Err("target is the boot media or its identity changed");
    }
    let mut block = bs
        .open_protocol_exclusive::<BlockIO>(target.handle)
        .map_err(|_| "cannot open the target disk")?;
    let media = block.media();
    if media.media_id() != target.media_id
        || media.last_block().checked_add(1) != Some(target.sectors)
        || !media.is_media_present()
        || media.is_read_only()
        || media.is_logical_partition()
        || media.block_size() != 512
        || media.io_align() > 4096
        || (media.io_align() > 1 && !media.io_align().is_power_of_two())
    {
        return Err("target media changed; refusing to write");
    }
    let mut buffer = IoBuffer([0; 512]);
    disk_layout::install(target.sectors, &files, id, |lba, data| {
        buffer.0.copy_from_slice(data);
        block
            .write_blocks(target.media_id, lba, &buffer.0)
            .map_err(|_| "disk write failed")
    })?;
    block
        .flush_blocks()
        .map_err(|_| "disk cache flush failed")?;
    disk_layout::verify(target.sectors, &files, id, |lba, out| {
        block
            .read_blocks(target.media_id, lba, &mut buffer.0)
            .map_err(|_| "disk verification read failed")?;
        out.copy_from_slice(&buffer.0);
        Ok(())
    })?;
    Ok(())
}
