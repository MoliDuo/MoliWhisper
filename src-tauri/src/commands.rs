//! What the settings page can call.

use std::time::Duration;

use moli_core::hotkey::{Hotkey, Mode};
use moli_core::organize::{DEFAULT_PROMPT, Organizer};
use moli_core::qwen;
use serde::Serialize;
use tauri::{AppHandle, Manager, State};
use tauri_plugin_autostart::ManagerExt;

use crate::hotkey::{self, HookStatus, HotkeyState};
use crate::platform::{self, MicStatus};
use crate::settings::Settings;
use crate::updater::{Phase, UpdateState};
use crate::{state_changed, updater};

#[derive(Serialize)]
pub struct HotkeyView {
    label: String,
    usage: String,
    is_default: bool,
}

#[derive(Serialize)]
pub struct StateView {
    version: String,
    mac: bool,
    hotkey: HotkeyView,
    mode: Mode,
    recording_hotkey: bool,
    hook: HookStatus,
    restore_clipboard: bool,
    organize: bool,
    has_qwen_key: bool,
    has_deepseek_key: bool,
    deepseek_prompt: String,
    default_prompt: &'static str,
    autostart: bool,
    accessibility: bool,
    microphone: MicStatus,
    config_path: String,
    update: Phase,
}

#[tauri::command]
pub fn get_state(app: AppHandle, settings: State<Settings>, hook: State<HotkeyState>) -> StateView {
    let config = settings.get();
    StateView {
        version: app.package_info().version.to_string(),
        mac: cfg!(target_os = "macos"),
        hotkey: HotkeyView {
            label: hotkey::label(&config.hotkey),
            usage: hotkey::usage(&config.hotkey, config.mode),
            is_default: config.hotkey == Hotkey::default(),
        },
        mode: config.mode,
        recording_hotkey: hook.0.is_recording(),
        hook: hook.0.status(),
        restore_clipboard: config.restore_clipboard,
        organize: config.organize,
        has_qwen_key: !config.qwen.api_key.is_empty(),
        has_deepseek_key: !config.deepseek.api_key.is_empty(),
        deepseek_prompt: config.deepseek.prompt.clone().unwrap_or_default(),
        default_prompt: DEFAULT_PROMPT,
        autostart: app.autolaunch().is_enabled().unwrap_or(false),
        accessibility: platform::accessibility_trusted(),
        microphone: platform::microphone_status(),
        config_path: app
            .path()
            .app_data_dir()
            .map(|d| d.join(moli_core::config::FILE_NAME).display().to_string())
            .unwrap_or_default(),
        update: app.state::<UpdateState>().phase(),
    }
}

#[tauri::command]
pub fn set_mode(
    app: AppHandle,
    settings: State<Settings>,
    hook: State<HotkeyState>,
    mode: Mode,
) -> Result<(), String> {
    let c = settings.update(|c| c.mode = mode)?;
    hook.0.configure(c.hotkey, c.mode);
    state_changed(&app);
    Ok(())
}

#[tauri::command]
pub fn set_restore_clipboard(
    app: AppHandle,
    settings: State<Settings>,
    enabled: bool,
) -> Result<(), String> {
    settings.update(|c| c.restore_clipboard = enabled)?;
    state_changed(&app);
    Ok(())
}

#[tauri::command]
pub fn set_organize(
    app: AppHandle,
    settings: State<Settings>,
    enabled: bool,
) -> Result<(), String> {
    settings.update(|c| c.organize = enabled)?;
    state_changed(&app);
    Ok(())
}

/// Saves the Qwen key; `None` keeps the saved one.
#[tauri::command]
pub fn set_qwen(
    app: AppHandle,
    settings: State<Settings>,
    api_key: Option<String>,
) -> Result<(), String> {
    settings.update(|c| {
        if let Some(key) = api_key {
            c.qwen.api_key = key.trim().to_string();
        }
    })?;
    state_changed(&app);
    Ok(())
}

/// Opens and ends a task with the saved key; the error is for the user to read.
#[tauri::command]
pub async fn test_qwen(settings: State<'_, Settings>) -> Result<String, String> {
    let opts =
        qwen::ConnectOptions::new(&settings.get().qwen.api_key).ok_or("还没有填写 API Key")?;
    qwen::check(&opts)
        .await
        .map(|t| format!("连接成功（{} ms）", t.as_millis()))
        .map_err(|e| format!("连接失败：{e}"))
}

/// Saves the DeepSeek key (`None` keeps the saved one) and prompt (empty
/// means the built-in one).
#[tauri::command]
pub fn set_deepseek(
    app: AppHandle,
    settings: State<Settings>,
    api_key: Option<String>,
    prompt: String,
) -> Result<(), String> {
    settings.update(|c| {
        if let Some(key) = api_key {
            c.deepseek.api_key = key.trim().to_string();
        }
        c.deepseek.prompt = Some(prompt.trim().to_string()).filter(|p| !p.is_empty());
    })?;
    state_changed(&app);
    Ok(())
}

/// Sends a request with the saved key and prompt; the error is for the user to read.
#[tauri::command]
pub async fn test_deepseek(settings: State<'_, Settings>) -> Result<String, String> {
    let d = settings.get().deepseek;
    let organizer =
        Organizer::deepseek(&d.api_key, d.prompt.as_deref()).ok_or("还没有填写 API Key")?;
    organizer
        .check(Duration::from_secs(30))
        .await
        .map(|()| "连接成功".to_string())
        .map_err(|e| format!("连接失败：{e}"))
}

#[tauri::command]
pub fn set_autostart(app: AppHandle, enabled: bool) -> Result<(), String> {
    let manager = app.autolaunch();
    let result = if enabled {
        manager.enable()
    } else {
        manager.disable()
    };
    result.map_err(|e| format!("无法修改开机启动：{e}"))?;
    state_changed(&app);
    Ok(())
}

#[tauri::command]
pub fn record_hotkey(app: AppHandle, hook: State<HotkeyState>) {
    hook.0.start_recording();
    state_changed(&app);
}

#[tauri::command]
pub fn cancel_record_hotkey(app: AppHandle, hook: State<HotkeyState>) {
    hook.0.stop_recording();
    state_changed(&app);
}

#[tauri::command]
pub fn reset_hotkey(
    app: AppHandle,
    settings: State<Settings>,
    hook: State<HotkeyState>,
) -> Result<(), String> {
    hook.0.stop_recording();
    let c = settings.update(|c| c.hotkey = Hotkey::default())?;
    hook.0.configure(c.hotkey, c.mode);
    state_changed(&app);
    Ok(())
}

#[tauri::command]
pub fn open_accessibility_settings() {
    platform::request_accessibility();
    platform::open_accessibility_settings();
}

#[tauri::command]
pub fn open_microphone_settings() {
    platform::open_microphone_settings();
}

#[tauri::command]
pub fn check_for_updates(app: AppHandle) {
    updater::check_now(&app);
}
