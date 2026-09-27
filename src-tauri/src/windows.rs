use tauri::{AppHandle, Manager, Runtime, WebviewUrl, WebviewWindowBuilder, WindowEvent};

use crate::hotkey::HotkeyState;

pub fn show_settings<R: Runtime>(app: &AppHandle<R>) {
    if let Some(window) = app.get_webview_window("settings") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
        return;
    }
    let built = WebviewWindowBuilder::new(app, "settings", WebviewUrl::App("settings.html".into()))
        .title("MoliWhisper 设置")
        .inner_size(560.0, 620.0)
        .min_inner_size(480.0, 420.0)
        .resizable(true)
        .center()
        .focused(true)
        .build();
    match built {
        Ok(window) => {
            let app = app.clone();
            window.on_window_event(move |event| {
                // A half-finished hotkey recording must not outlive the page.
                if let WindowEvent::Destroyed = event
                    && let Some(hotkey) = app.try_state::<HotkeyState>()
                {
                    hotkey.0.stop_recording();
                }
            });
            let _ = window.set_focus();
        }
        Err(e) => log::error!("could not open the settings window: {e}"),
    }
}
