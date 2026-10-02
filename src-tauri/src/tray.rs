use tauri::image::Image;
use tauri::menu::{Menu, MenuBuilder, MenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Manager, Runtime};

use crate::hotkey::{self, HookStatus, HotkeyState};
use crate::settings::Settings;
use crate::updater::{Phase, UpdateState};
use crate::{platform, updater, windows};

pub const TRAY_ID: &str = "main";

pub fn create<R: Runtime>(app: &AppHandle<R>) -> tauri::Result<()> {
    TrayIconBuilder::with_id(TRAY_ID)
        .icon(Image::from_bytes(include_bytes!("../icons/tray.png"))?)
        .icon_as_template(true)
        .tooltip("MoliWhisper")
        .menu(&build_menu(app)?)
        .show_menu_on_left_click(true)
        .on_menu_event(|app, event| match event.id().as_ref() {
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

/// Rebuilds the menu after a setting changed.
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
    let label = if app.state::<Settings>().get().qwen.api_key.is_empty() {
        "未填写千问 API Key"
    } else {
        "识别：千问（阿里云）"
    };

    let mut menu = MenuBuilder::new(app).item(&MenuItem::with_id(
        app,
        "status",
        label,
        false,
        None::<&str>,
    )?);
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
