mod ffi;
mod keyhook;
mod keys;
mod overlay;
mod paste;
mod permissions;

pub use keyhook::start_key_hook;
pub use keys::{ESCAPE, key_label};
pub use overlay::{hide_overlay, prepare_overlay, show_overlay};
pub use paste::paste;
pub use permissions::{
    accessibility_trusted, microphone_status, open_accessibility_settings,
    open_microphone_settings, request_accessibility,
};
