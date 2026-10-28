//! Compiles the kernel's hardware-independent modules for the host so they
//! can be exercised by ordinary `cargo test`. See `Cargo.toml`.

extern crate alloc;

pub mod ahci;

#[path = "../../../src/wisp/mod.rs"]
pub mod wisp;

#[path = "../../../src/save_format.rs"]
pub mod save_format;

#[path = "../../../src/line_editor.rs"]
pub mod line_editor;

#[path = "../../../src/disk_layout.rs"]
pub mod disk_layout;
