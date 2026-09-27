//! Platform-independent core of MoliWhisper.
//!
//! Nothing in here depends on Tauri or on OS-specific APIs, so the whole crate
//! builds and tests on any platform with plain `cargo test`.

pub mod asr;
pub mod creds;
pub mod store;
