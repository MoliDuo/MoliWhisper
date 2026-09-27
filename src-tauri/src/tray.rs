use tauri::image::Image;
use tauri::menu::{Menu, MenuBuilder, MenuItem};
use tauri::tray::TrayIconBuilder;
use tauri::{AppHandle, Manager, Runtime};

use crate::auth::{Auth, AuthStatus};
use crate::{login, windows};

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
            "settings" => windows::show_settings(app),
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
    let status = app.state::<Auth>().status();
    let (label, login_label, logged_in) = match status {
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
    menu.separator()
        .text("settings", "设置…")
        .separator()
        .text("quit", "退出 MoliWhisper")
        .build()
}
