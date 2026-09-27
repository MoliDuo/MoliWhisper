//! Global hotkey logic shared by the platform hooks.

pub mod matcher;
pub mod spec;

pub use matcher::{Action, Decision, Key, KeyEvent, Matcher, Recorded};
pub use spec::{Hotkey, ModKey, Mode, Mods};
