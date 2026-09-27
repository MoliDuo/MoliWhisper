//! Stand-ins until the Windows platform layer lands (M8): the app builds and
//! runs, but there is no global hotkey and no automatic paste yet.

use std::sync::Arc;

use tauri::{AppHandle, Runtime, WebviewWindow};

use super::MicStatus;
use crate::hotkey::{Hook, HookStatus};

/// VK_ESCAPE.
pub const ESCAPE: u32 = 0x1B;

pub fn key_label(code: u32) -> String {
    match code {
        0x30..=0x39 | 0x41..=0x5A => char::from_u32(code).unwrap_or('?').to_string(),
        0x70..=0x87 => format!("F{}", code - 0x6F),
        0x08 => "Backspace".into(),
        0x09 => "Tab".into(),
        0x0D => "Enter".into(),
        0x1B => "Esc".into(),
        0x20 => "Space".into(),
        0x21 => "Page Up".into(),
        0x22 => "Page Down".into(),
        0x23 => "End".into(),
        0x24 => "Home".into(),
        0x25 => "←".into(),
        0x26 => "↑".into(),
        0x27 => "→".into(),
        0x28 => "↓".into(),
        0x2D => "Insert".into(),
        0x2E => "Delete".into(),
        _ => format!("键 {code}"),
    }
}

pub fn start_key_hook(hook: Arc<Hook>) {
    hook.set_status(HookStatus::Unsupported("这个系统暂不支持全局热键".into()));
}

pub fn accessibility_trusted() -> bool {
    true
}

pub fn request_accessibility() {}

pub fn open_accessibility_settings() {}

pub fn open_microphone_settings() {}

pub fn microphone_status() -> MicStatus {
    MicStatus::Unknown
}

pub async fn paste<R: Runtime>(
    _app: &AppHandle<R>,
    _text: String,
    _restore: bool,
) -> Result<(), String> {
    Err("这个系统暂不支持自动粘贴".into())
}

pub fn prepare_overlay<R: Runtime>(_window: &WebviewWindow<R>) {}

pub fn show_overlay<R: Runtime>(window: &WebviewWindow<R>) {
    let _ = window.show();
}

pub fn hide_overlay<R: Runtime>(window: &WebviewWindow<R>) {
    let _ = window.hide();
}
