//! What the settings page can call.

use moli_core::config::BackendKind;
use moli_core::doubao::web::params::Overrides;
use moli_core::hotkey::{Hotkey, Mode};
use serde::Serialize;
use tauri::{AppHandle, Manager, State};
use tauri_plugin_autostart::ManagerExt;

use crate::auth::{Auth, AuthStatus};
use crate::dictation::{self, Dictation};
use crate::hotkey::{self, HookStatus, HotkeyState};
use crate::platform::{self, MicStatus};
use crate::settings::Settings;
use crate::updater::{Phase, UpdateState};
use crate::{ime, login, state_changed, updater};

#[derive(Serialize)]
pub struct HotkeyView {
    label: String,
    usage: String,
    is_default: bool,
}

#[derive(Serialize)]
pub struct AuthView {
    label: String,
    logged_in: bool,
    /// Needs a (new) login to dictate.
    needs_login: bool,
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
    backend: BackendKind,
    organize: bool,
    autostart: bool,
    overrides: String,
    auth: AuthView,
    accessibility: bool,
    microphone: MicStatus,
    config_path: String,
}

#[tauri::command]
pub fn get_state(app: AppHandle, settings: State<Settings>, hook: State<HotkeyState>) -> StateView {
    let config = settings.get();
    let overrides = if config.asr.param_overrides.is_empty() {
        String::new()
    } else {
        serde_json::to_string_pretty(&config.asr.param_overrides).unwrap_or_default()
    };
    StateView {
        version: app.package_info().version.to_string(),
    update: Phase,
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
        backend: config.backend,
        organize: config.organize,
        autostart: app.autolaunch().is_enabled().unwrap_or(false),
        overrides,
        auth: auth_view(app.state::<Auth>().status()),
        accessibility: platform::accessibility_trusted(),
        microphone: platform::microphone_status(),
        config_path: app
            .path()
            .app_data_dir()
            .map(|d| d.join(moli_core::config::FILE_NAME).display().to_string())
            .unwrap_or_default(),
    }
}

fn auth_view(status: AuthStatus) -> AuthView {
    let (label, logged_in, needs_login) = match status {
        AuthStatus::LoggedOut => ("未登录".to_string(), false, true),
        AuthStatus::Active {
            days_left: Some(d), ..
        } => (format!("已登录，{d} 天后过期"), true, false),
        AuthStatus::Active {
            days_left: None, ..
        } => ("已登录".to_string(), true, false),
        AuthStatus::Rejected => ("登录已失效".to_string(), true, true),
        AuthStatus::Expired => ("登录已过期".to_string(), true, true),
    };
    AuthView {
        label,
        logged_in,
        needs_login,
    }
        update: app.state::<UpdateState>().phase(),
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
pub fn set_backend(
    app: AppHandle,
    settings: State<Settings>,
    backend: BackendKind,
) -> Result<(), String> {
    settings.update(|c| c.backend = backend)?;
    app.state::<Dictation>()
        .0
        .set_timings(dictation::timings(backend));
    ime::keep_warm(&app, backend == BackendKind::Ime);
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

/// Takes the text of the advanced box: a JSON object of strings or nulls,
/// or nothing at all.
#[tauri::command]
pub fn set_overrides(
    app: AppHandle,
    settings: State<Settings>,
    text: String,
) -> Result<(), String> {
    let overrides: Overrides = if text.trim().is_empty() {
        Overrides::new()
    } else {
        serde_json::from_str(&text)
            .map_err(|e| format!("格式不对：需要一个 JSON 对象，值为字符串或 null（{e}）"))?
    };
    settings.update(|c| c.asr.param_overrides = overrides)?;
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

// Async so the window is not built on the main thread (deadlocks on Windows).
#[tauri::command]
pub async fn login(app: AppHandle) {
    let fresh = matches!(
        app.state::<Auth>().status(),
        AuthStatus::Rejected | AuthStatus::Expired
    );
    login::open(&app, fresh);
}

#[tauri::command]
pub async fn logout(app: AppHandle) {
    login::logout(app).await;
}

#[tauri::command]
pub fn check_for_updates(app: AppHandle) {
    updater::check_now(&app);
}
