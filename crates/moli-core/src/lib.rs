//! Platform-independent core of MoliWhisper.
//!
//! Nothing in here depends on Tauri or on OS-specific APIs, so the whole crate
//! builds and tests on any platform with plain `cargo test`.

pub mod asr;
pub mod audio;
pub mod config;
pub mod hotkey;
pub(crate) mod net;
pub mod organize;
pub mod qwen;
pub mod session;
