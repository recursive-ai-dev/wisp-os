//! Compiles the kernel's hardware-independent modules for the host so they
//! can be exercised by ordinary `cargo test`. See `Cargo.toml`.

extern crate alloc;

pub mod ahci;

// The save format consumes the common disk interface. Its host transport
// retains the existing volatile-cache and fault-injection model.
pub mod storage {
    pub use crate::ahci::{Hba as Disk, SECTOR_SIZE};
}

#[path = "../../../src/storage_policy.rs"]
pub mod storage_policy;

#[path = "../../../src/wisp/mod.rs"]
pub mod wisp;

#[path = "../../../src/save_format.rs"]
pub mod save_format;

#[path = "../../../src/line_editor.rs"]
pub mod line_editor;

#[path = "../../../src/editor.rs"]
pub mod editor;

#[path = "../../../src/disk_layout.rs"]
pub mod disk_layout;

#[path = "../../../src/projects.rs"]
pub mod projects;

#[path = "../../../src/log.rs"]
pub mod log;

#[path = "../../../src/rtc.rs"]
pub mod rtc;

#[path = "../../../src/keymap.rs"]
pub mod keymap;
