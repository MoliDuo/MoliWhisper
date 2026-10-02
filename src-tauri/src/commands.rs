//! What the settings page can call.

use std::time::Duration;

use moli_core::config::{BackendKind, OrganizerKind};
use moli_core::doubao::web::params::Overrides;
use moli_core::hotkey::{Hotkey, Mode};
use moli_core::organize::{DEFAULT_PROMPT, OpenAiOrganizer};
use moli_core::qwen;
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
    qwen_model: String,
    qwen_url: String,
    has_qwen_key: bool,
    organizer: OrganizerKind,
    openai_base_url: String,
    openai_model: String,
    openai_prompt: String,
    default_prompt: &'static str,
    has_openai_key: bool,
    autostart: bool,
    overrides: String,
    auth: AuthView,
    accessibility: bool,
    microphone: MicStatus,
    config_path: String,
    update: Phase,
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
        qwen_model: config.qwen.model.clone(),
        qwen_url: config.qwen.url.clone(),
        has_qwen_key: !config.qwen.api_key.is_empty(),
        organizer: config.organizer.provider,
        openai_base_url: config.organizer.openai.base_url.clone(),
        openai_model: config.organizer.openai.model.clone(),
        openai_prompt: config.organizer.openai.prompt.clone().unwrap_or_default(),
        default_prompt: DEFAULT_PROMPT,
        has_openai_key: !config.organizer.openai.api_key.is_empty(),
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
        update: app.state::<UpdateState>().phase(),
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

/// Saves the Qwen settings. A `None` key keeps the saved one; blank model
/// and URL mean the defaults.
#[tauri::command]
pub fn set_qwen(
    app: AppHandle,
    settings: State<Settings>,
    api_key: Option<String>,
    model: String,
    url: String,
) -> Result<(), String> {
    settings.update(|c| {
        if let Some(key) = api_key {
            c.qwen.api_key = key.trim().to_string();
        }
        c.qwen.model = model.trim().to_string();
        c.qwen.url = url.trim().to_string();
    })?;
    state_changed(&app);
    Ok(())
}

/// Opens and ends a task with the saved settings; the error is for the user to read.
#[tauri::command]
pub async fn test_qwen(settings: State<'_, Settings>) -> Result<String, String> {
    let q = settings.get().qwen;
    let opts = qwen::ConnectOptions::new(&q.url, &q.api_key, &q.model)
        .ok_or("还没有填写 API Key，或地址不是 ws:// / wss:// 地址")?;
    qwen::check(&opts)
        .await
        .map(|t| format!("连接成功（{} ms）", t.as_millis()))
        .map_err(|e| format!("连接失败：{e}"))
}

/// Saves the organizer. `None` for the key keeps the saved one; an empty
/// prompt means the built-in one.
#[tauri::command]
pub fn set_organizer(
    app: AppHandle,
    settings: State<Settings>,
    provider: OrganizerKind,
    base_url: String,
    model: String,
    prompt: String,
    api_key: Option<String>,
) -> Result<(), String> {
    settings.update(|c| {
        c.organizer.provider = provider;
        let openai = &mut c.organizer.openai;
        openai.base_url = base_url.trim().to_string();
        openai.model = model.trim().to_string();
        openai.prompt = Some(prompt.trim().to_string()).filter(|p| !p.is_empty());
        if let Some(key) = api_key {
            openai.api_key = key.trim().to_string();
        }
    })?;
    state_changed(&app);
    Ok(())
}

/// Runs a sample sentence through the saved OpenAI-compatible model.
#[tauri::command]
pub async fn test_organizer(settings: State<'_, Settings>) -> Result<String, String> {
    let config = settings.get();
    let organizer =
        OpenAiOrganizer::new(&config.organizer.openai).ok_or("还需要填写 Base URL 和模型名")?;
    organizer
        .organize(
            "嗯那个我想说就是明天下午的会议呢可能要往后推一个小时吧",
            Duration::from_secs(30),
        )
        .await
        .ok_or_else(|| "没有得到结果：请检查地址、密钥和模型名（详见日志）".to_string())
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
