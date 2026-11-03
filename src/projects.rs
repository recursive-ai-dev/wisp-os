//! WISP source projects in the existing crash-atomic save store.
//! Boot-volume apps remain templates; `prog.<name>` takes precedence when
//! a saved project exists. No post-boot FAT writes are needed.

use alloc::format;
use alloc::rc::Rc;
use alloc::string::String;
use alloc::vec::Vec;

use crate::save_format::{SaveDisk, VALUE_MAX_LEN};
use crate::wisp::value::Value;

const PREFIX: &str = "prog.";
const MAX_NAME_BYTES: usize = 32 - PREFIX.len();
/// A serialized string has a one-byte tag and four-byte length prefix.
pub const MAX_SAVED_SOURCE_BYTES: usize = VALUE_MAX_LEN - 5;

/// A portable, case-insensitive project name, without its `.wisp` suffix.
pub fn normalize_name(input: &str) -> Result<String, &'static str> {
    let input = input.trim();
    let has_suffix = input
        .len()
        .checked_sub(5)
        .and_then(|start| input.get(start..))
        .is_some_and(|suffix| suffix.eq_ignore_ascii_case(".wisp"));
    let name = if has_suffix {
        &input[..input.len() - 5]
    } else {
        input
    };
    if name.is_empty() {
        return Err("expected a project name, e.g. 'hello'");
    }
    if name.len() > MAX_NAME_BYTES {
        return Err("project names can contain at most 27 characters");
    }
    if !name
        .bytes()
        .all(|b| b.is_ascii_alphanumeric() || b == b'_' || b == b'-')
    {
        return Err("use only letters, digits, '_' and '-' in project names");
    }
    Ok(name.to_ascii_lowercase())
}

/// Read a saved project first, then an app from the boot-time snapshot.
/// An unreadable mounted store is an error, so a broken save never silently
/// opens an older bundled source for the user to overwrite.
pub fn load_source(
    name: &str,
    apps: &[(String, String)],
    disk: Option<&SaveDisk>,
) -> Result<Option<String>, &'static str> {
    let name = normalize_name(name)?;
    if let Some(disk) = disk.filter(|disk| disk.is_mounted()) {
        match disk.load(&format!("{PREFIX}{name}"))? {
            Some(Value::Str(source)) => return Ok(Some((*source).clone())),
            Some(_) => return Err("saved project contains a non-string value"),
            None => {}
        }
    }
    let filename = format!("{name}.wisp");
    Ok(apps
        .iter()
        .find(|(file, _)| file.eq_ignore_ascii_case(&filename))
        .map(|(_, source)| source.clone()))
}

/// Save the entire source or return an error; never truncate it to fit.
pub fn save_source(name: &str, source: &str, disk: Option<&SaveDisk>) -> Result<(), &'static str> {
    let name = normalize_name(name)?;
    if source.len() > MAX_SAVED_SOURCE_BYTES {
        return Err("project exceeds the save store's 4042-byte source limit");
    }
    let disk = disk.ok_or("no save disk selected (use 'disks' and 'disk <n>')")?;
    disk.save(
        &format!("{PREFIX}{name}"),
        &Value::Str(Rc::new(String::from(source))),
    )
}

/// Saved project names, sorted, excluding unrelated application data.
pub fn list(disk: Option<&SaveDisk>) -> Result<Vec<String>, &'static str> {
    let Some(disk) = disk.filter(|disk| disk.is_mounted()) else {
        return Ok(Vec::new());
    };
    let mut names = Vec::new();
    for (key, _) in disk.keys()? {
        if let Some(name) = key.strip_prefix(PREFIX) {
            if normalize_name(name).is_ok_and(|normalized| normalized == name) {
                names.push(String::from(name));
            }
        }
    }
    names.sort();
    Ok(names)
}
