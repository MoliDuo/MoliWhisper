//! OS-specific parts: the key hook, pasting, the overlay window, permissions.
//!
//! Each platform module offers the same free functions; the rest of the app
//! only calls them through here.

#[cfg(target_os = "macos")]
mod macos;
#[cfg(target_os = "macos")]
pub use macos::*;

#[cfg(not(target_os = "macos"))]
mod other;
#[cfg(not(target_os = "macos"))]
pub use other::*;

use serde::Serialize;

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "snake_case")]
pub enum MicStatus {
    Granted,
    Denied,
    NotDetermined,
    Unknown,
}
