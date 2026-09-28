//! The Doubao IME client: anonymous recognition and the "organize" rewrite.

use std::time::Duration;

use moli_core::doubao::ime::{IME_VERSION_CODE, ImeClient};
use tauri::{AppHandle, Manager, Runtime};

use crate::settings::Settings;

/// How long to wait for the organized text before pasting the original.
pub const ORGANIZE_TIMEOUT: Duration = Duration::from_secs(5);

const VERSION_CHECK_TIMEOUT: Duration = Duration::from_secs(15);

pub struct Ime(pub ImeClient);

/// Builds the client with the saved device id, making one on first run.
pub fn init<R: Runtime>(app: &AppHandle<R>) -> Result<(), String> {
    let settings = app.state::<Settings>();
    let device_id = match settings.get().ime.device_id {
        Some(id) => id,
        None => {
            let id = ImeClient::new_device_id();
            settings.update(|c| c.ime.device_id = Some(id.clone()))?;
            id
        }
    };
    let client = ImeClient::new(device_id);
    // The service refused the old id and the client moved on: keep the new one.
    let handle = app.clone();
    client.on_new_device_id(move |id| {
        let id = id.to_string();
        if let Err(e) = handle
            .state::<Settings>()
            .update(|c| c.ime.device_id = Some(id))
        {
            log::warn!("saving the new IME device id: {e}");
        }
    });
    tauri::async_runtime::spawn(client.clone().warm());
    let checker = client.clone();
    tauri::async_runtime::spawn(async move {
        if let Some(release) = checker.newer_version(VERSION_CHECK_TIMEOUT).await {
            log::warn!(
                "Doubao IME {} ({}) is out; this app still presents itself as {IME_VERSION_CODE}",
                release.name,
                release.code
            );
        }
    });
    app.manage(Ime(client));
    Ok(())
}

/// Keeps a connection to the service open while the IME backend is in use,
/// so dictation starts without waiting for one.
pub fn keep_warm<R: Runtime>(app: &AppHandle<R>, on: bool) {
    app.state::<Ime>().0.set_keep_warm(on);
}
