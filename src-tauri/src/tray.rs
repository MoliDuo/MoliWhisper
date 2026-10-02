use moli_core::config::BackendKind;
use tauri::image::Image;
use tauri::menu::{Menu, MenuBuilder, MenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Manager, Runtime};

use crate::auth::{Auth, AuthStatus};
use crate::hotkey::{self, HookStatus, HotkeyState};
use crate::settings::Settings;
use crate::updater::{Phase, UpdateState};
use crate::{login, platform, updater, windows};

pub const TRAY_ID: &str = "main";

pub fn create<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    TrayIconBuilder::with_id(TRAY_ID)
        .icon(Image::from_bytes(include_bytes!("../icons/tray.png"))?)
        .icon_as_template(true)
        .tooltip("MoliWhisper")
        .menu(&build_menu(app)?)
        .show_menu_on_left_click(true)
        .on_menu_event(|app, event| match event.id().as_ref() {
            "login" => {
                let fresh = matches!(app.state::<Auth>().status(), AuthStatus::Rejected);
                login::open(app, fresh);
            }
            "logout" => {
                tauri::async_runtime::spawn(login::logout(app.clone()));
            }
            "accessibility" => {
                platform::request_accessibility();
                platform::open_accessibility_settings();
            }
            "settings" => windows::show_settings(app),
            "update" => updater::check_now(app),
            "quit" => app.exit(0),
            _ => {}
        })
        .build(app)?;
    Ok(())
}

/// Rebuilds the menu after the login state changed.
pub fn refresh<R: Runtime>(app: &AppHandle<R>) {
    let Some(tray) = app.tray_by_id(TRAY_ID) else {
        return;
    };
    match build_menu(app) {
        Ok(menu) => {
            let _ = tray.set_menu(Some(menu));
        }
        Err(e) => log::error!("could not rebuild the tray menu: {e}"),
    }
}

fn build_menu<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<Menu<R>> {
    let (label, login_label, logged_in) = match app.state::<Settings>().get().backend {
        BackendKind::Ime => ("识别：豆包输入法（免登录）".to_string(), None, false),
        BackendKind::Qwen => ("识别：千问（阿里云）".to_string(), None, false),
        BackendKind::Web => web_status(app.state::<Auth>().status()),
    };

    let mut menu = MenuBuilder::new(app).item(&MenuItem::with_id(
        app,
        "status",
        label,
        false,
        None::<&str>,
    )?);
    if let Some(text) = login_label {
        menu = menu.text("login", text);
    }
    if logged_in {
        menu = menu.text("logout", "退出登录");
    }
    menu = menu.separator().item(&MenuItem::with_id(
        app,
        "usage",
        hotkey_line(app),
        false,
        None::<&str>,
    )?);
    if !platform::accessibility_trusted() {
        menu = menu.text("accessibility", "授予辅助功能权限…");
    }
    menu.separator()
        .text("settings", "设置…")
        .item(&update_item(app)?)
        .separator()
        .text("quit", "退出 MoliWhisper")
        .build()
}

/// The status line, the login item (if any) and whether to offer logging out.
fn web_status(status: AuthStatus) -> (String, Option<&'static str>, bool) {
    match status {
        AuthStatus::LoggedOut => ("未登录".to_string(), Some("登录豆包…"), false),
        AuthStatus::Active {
            days_left: Some(d),
            expiring: true,
        } => (format!("登录将在 {d} 天后过期"), Some("重新登录…"), true),
        AuthStatus::Active {
            days_left: Some(d), ..
        } => (format!("已登录（{d} 天后过期）"), None, true),
        AuthStatus::Active {
            days_left: None, ..
        } => ("已登录".to_string(), None, true),
        AuthStatus::Rejected => ("登录已失效".to_string(), Some("重新登录…"), true),
        AuthStatus::Expired => ("登录已过期".to_string(), Some("重新登录…"), true),
    }
}

/// "检查更新…", greyed out with what it is doing while a check runs.
fn update_item<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<MenuItem<R>> {
    let phase = app
        .try_state::<UpdateState>()
        .map_or(Phase::Idle, |s| s.phase());
    let (text, enabled) = match phase {
        Phase::Idle => ("检查更新…".to_string(), true),
        Phase::Checking => ("正在检查更新…".to_string(), false),
        Phase::Found(v) => (format!("发现新版本 {v}"), false),
        Phase::Downloading(v) => (format!("正在下载 {v}…"), false),
    };
    MenuItem::with_id(app, "update", text, enabled, None::<&str>)
}

fn hotkey_line<R: Runtime>(app: &AppHandle<R>) -> String {
    let config = app.state::<Settings>().get();
    let status = app
        .try_state::<HotkeyState>()
        .map_or(HookStatus::Starting, |h| h.0.status());
    match status {
        HookStatus::Running | HookStatus::Starting => hotkey::usage(&config.hotkey, config.mode),
        HookStatus::NeedsPermission => "热键需要辅助功能权限".into(),
        HookStatus::Unsupported(why) | HookStatus::Failed(why) => why,
    }
}
