//! FAT32 access via the UEFi Simple File System protocol -- used exactly
//! once, at boot, to snapshot every `APPS/*.wisp` file into memory before
//! `exit_boot_services` makes Boot Services (and with it, this whole
//! protocol) permanently unusable. See `CHANGES.md` for why a real
//! block-device-backed driver for live post-exit filesystem access is
//! separate, explicitly out-of-scope future work.

use alloc::format;
use alloc::string::String;
use alloc::vec::Vec;

use uefi::proto::media::file::{Directory, File, FileAttribute, FileInfo, FileMode, RegularFile};
use uefi::table::boot::BootServices;
use uefi::{CString16, Error, Handle, Result, Status};

use crate::debugcon;

/// Largest single `.wisp` source admitted into the snapshot. Comfortably
/// above any real program (the bundled ones are a few KiB) and small
/// enough that one file can never take a meaningful share of the 8 MiB
/// kernel heap, which the parser and interpreter need far more of.
pub const MAX_APP_SOURCE_BYTES: usize = 256 * 1024;
/// Combined budget for every snapshotted source plus its filename,
/// checked before each allocation: the snapshot stays resident for the
/// whole boot, so this is headroom permanently taken from the heap.
pub const MAX_APPS_TOTAL_BYTES: usize = 1024 * 1024;
/// Most `.wisp` files listed or snapshotted; further entries are skipped
/// (and logged) rather than collected without bound.
pub const MAX_APP_COUNT: usize = 64;
/// Longest filename kept (bytes of UTF-8). FAT long names go up to 255
/// characters; nothing a user types at `run` needs more than this.
const MAX_APP_NAME_BYTES: usize = 64;

/// Turn a user-typed path (which may use `/` out of habit) into the
/// backslash-separated, null-terminated UCS-2 form
/// `EFI_FILE_PROTOCOL.Open()` expects. A single `Open()` call on the root
/// directory accepts a multi-component relative path directly (this is
/// exactly how firmware itself locates `EFI\BOOT\BOOTX64.EFI`), so callers
/// never need to walk path components by hand.
fn make_path(path: &str) -> Result<CString16> {
    let normalized: String = path.chars().map(|c| if c == '/' { '\\' } else { c }).collect();
    CString16::try_from(normalized.as_str()).map_err(|_| Error::new(Status::INVALID_PARAMETER, ()))
}

/// Read a whole regular file from the boot volume into a `String`. `path`
/// is relative to the volume root (e.g. `"APPS\\hello.wisp"` or, thanks to
/// `make_path`'s normalization, `"APPS/hello.wisp"`).
///
/// Fails cleanly (never panics, never allocates more than `max_bytes`) if
/// the path doesn't exist, names a directory instead of a file, is larger
/// than `max_bytes` (`BAD_BUFFER_SIZE`), cannot be allocated
/// (`OUT_OF_RESOURCES`), ends before its advertised size (`END_OF_FILE`
/// -- a truncated program is not silently run as if complete), or isn't
/// valid UTF-8 (`COMPROMISED_DATA`).
pub fn read_file_to_string(bs: &BootServices, image: Handle, path: &str, max_bytes: usize) -> Result<String> {
    String::from_utf8(read_file_to_bytes(bs, image, path, max_bytes)?)
        .map_err(|_| Error::new(Status::COMPROMISED_DATA, ()))
}

/// Read an original on-disk binary, rather than its relocated in-memory
/// PE image. Used by the live installer while firmware file access exists.
pub fn read_file_to_bytes(bs: &BootServices, image: Handle, path: &str, max_bytes: usize) -> Result<Vec<u8>> {
    let mut fs = bs.get_image_file_system(image)?;
    let mut root: Directory = fs.open_volume()?;
    let cpath = make_path(path)?;

    let handle = root.open(&cpath, FileMode::Read, FileAttribute::empty())?;
    let mut file: RegularFile = handle
        .into_regular_file()
        .ok_or_else(|| Error::new(Status::NOT_FOUND, ()))?;

    let info = file.get_boxed_info::<FileInfo>()?;
    // The advertised size is checked before it is used as an allocation
    // size: a file is admitted by budget, not by trust.
    let size = usize::try_from(info.file_size())
        .ok()
        .filter(|&n| n <= max_bytes)
        .ok_or_else(|| Error::new(Status::BAD_BUFFER_SIZE, ()))?;

    let mut buf: Vec<u8> = Vec::new();
    buf.try_reserve_exact(size)
        .map_err(|_| Error::new(Status::OUT_OF_RESOURCES, ()))?;
    buf.resize(size, 0);
    let mut total = 0usize;
    while total < size {
        let n = file.read(&mut buf[total..])?;
        if n == 0 {
            // Fewer bytes on disk than FileInfo reported.
            return Err(Error::new(Status::END_OF_FILE, ()));
        }
        total += n;
    }

    Ok(buf)
}

/// Walk `APPS/` on the boot volume and read every `*.wisp` file into
/// memory as `(filename, source)` pairs, while Boot Services is still
/// alive. Called exactly once, from `main.rs`, immediately before
/// `exit_boot_services` -- this is the *last* use of `bs` in the whole
/// program.
///
/// Never panics: any failure (missing `APPS` directory, an unreadable
/// entry, a non-UTF-8 file, a file over [`MAX_APP_SOURCE_BYTES`] or one
/// that would take the snapshot past [`MAX_APPS_TOTAL_BYTES`]) is logged
/// to the debug port with the filename and that one entry is skipped, so
/// a single bad file can't prevent every other program from still being
/// available to `run` -- or exhaust the heap before the shell exists. Returns an empty `Vec` (not an error)
/// if `APPS` itself can't be opened at all, since a shell with no
/// installed programs is still a working shell.
pub fn snapshot_apps(bs: &BootServices, image: Handle) -> Vec<(String, String)> {
    // List filenames first, then drop every directory/filesystem handle
    // before reading any file contents. `read_file_to_string` below opens
    // its own fresh `SimpleFileSystem`/`Directory` for each file -- if the
    // listing's own `fs`/`root`/`apps_dir` handles were still held open
    // while that happens, the protocol's second concurrent open attempt
    // fails outright (`ACCESS_DENIED`, observed and confirmed while
    // bringing this phase up; see `CHANGES.md`). Collecting names first
    // and reading files only after every listing handle has gone out of
    // scope avoids ever having two opens of the same protocol instance
    // alive at once.
    let names = list_wisp_filenames(bs, image);

    let mut apps = Vec::with_capacity(names.len());
    let mut used = 0usize;
    for name in names {
        let remaining = MAX_APPS_TOTAL_BYTES.saturating_sub(used + name.len());
        let limit = remaining.min(MAX_APP_SOURCE_BYTES);
        let path = format!("APPS/{name}");
        match read_file_to_string(bs, image, &path, limit) {
            Ok(source) => {
                used += name.len() + source.len();
                apps.push((name, source));
            }
            Err(e) => {
                let _ = debugcon::write_fmt(format_args!(
                    "\n[fs] snapshot_apps: skipping APPS/{name}: {:?}{}\n",
                    e.status(),
                    if e.status() == Status::BAD_BUFFER_SIZE {
                        " (larger than the per-file or remaining snapshot budget)"
                    } else {
                        ""
                    },
                ));
            }
        }
    }
    apps
}

/// List every `*.wisp` filename directly under `APPS` on the boot volume.
/// Every filesystem handle this opens is dropped before returning, by
/// construction (they're all local to this function), so the caller is
/// free to open the filesystem again immediately afterward.
fn list_wisp_filenames(bs: &BootServices, image: Handle) -> Vec<String> {
    let mut names = Vec::new();

    let mut fs = match bs.get_image_file_system(image) {
        Ok(fs) => fs,
        Err(e) => {
            log_snapshot_error("get_image_file_system", e.status());
            return names;
        }
    };
    let mut root: Directory = match fs.open_volume() {
        Ok(root) => root,
        Err(e) => {
            log_snapshot_error("open_volume", e.status());
            return names;
        }
    };
    let apps_path = match make_path("APPS") {
        Ok(p) => p,
        Err(e) => {
            log_snapshot_error("make_path(APPS)", e.status());
            return names;
        }
    };
    let apps_handle = match root.open(&apps_path, FileMode::Read, FileAttribute::empty()) {
        Ok(h) => h,
        Err(e) => {
            log_snapshot_error("open(APPS)", e.status());
            return names;
        }
    };
    let mut apps_dir = match apps_handle.into_directory() {
        Some(d) => d,
        None => {
            debugcon::write_str("\n[fs] snapshot_apps: APPS exists but is not a directory\n");
            return names;
        }
    };

    loop {
        let entry = match apps_dir.read_entry_boxed() {
            Ok(Some(entry)) => entry,
            Ok(None) => break,
            Err(e) => {
                log_snapshot_error("read_entry_boxed", e.status());
                break;
            }
        };
        if entry.is_directory() {
            continue;
        }
        let name = format!("{}", entry.file_name());
        if !name.to_ascii_lowercase().ends_with(".wisp") {
            continue;
        }
        if name.len() > MAX_APP_NAME_BYTES {
            let _ = debugcon::write_fmt(format_args!(
                "\n[fs] snapshot_apps: skipping a .wisp file with a {}-byte name (limit {MAX_APP_NAME_BYTES})\n",
                name.len(),
            ));
            continue;
        }
        if names.len() >= MAX_APP_COUNT {
            let _ = debugcon::write_fmt(format_args!(
                "\n[fs] snapshot_apps: more than {MAX_APP_COUNT} .wisp files; skipping {name} and any later ones\n",
            ));
            break;
        }
        names.push(name);
    }

    names
}

fn log_snapshot_error(step: &str, status: Status) {
    debugcon::write_str("\n[fs] snapshot_apps: ");
    debugcon::write_str(step);
    debugcon::write_str(" failed: ");
    let _ = debugcon::write_fmt(format_args!("{status:?}\n"));
}
