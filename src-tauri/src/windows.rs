use tauri::{AppHandle, Manager, Runtime, WebviewUrl, WebviewWindowBuilder};

pub fn show_settings<R: Runtime>(app: &AppHandle<R>) {
    if let Some(window) = app.get_webview_window("settings") {
        let _ = window.unminimize();
        let _ = window.show();
        let _ = window.set_focus();
        return;
    }
    let built = WebviewWindowBuilder::new(app, "settings", WebviewUrl::App("settings.html".into()))
        .title("MoliWhisper 设置")
        .inner_size(560.0, 480.0)
        .resizable(false)
        .center()
        .focused(true)
        .build();
    match built {
        Ok(window) => {
            let _ = window.set_focus();
        }
        Err(e) => log::error!("could not open the settings window: {e}"),
    }
}
