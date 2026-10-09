//! Updates from GitHub Releases (MoliSpec 007).
//!
//! Every `vX.Y.Z` tag publishes a release with a signed `latest.json`
//! (release.yml). The app checks it in the background every hour and only
//! speaks up when there is a newer version; "检查更新…" in the tray or on the
//! settings page also reports "up to date" and errors. Nothing is downloaded
//! until the user agrees, and the app restarts into the new version.

use std::path::Path;
use std::sync::Mutex;
use std::time::Duration;

use serde::Serialize;
use tauri::{AppHandle, Manager, Runtime};
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};
use tauri_plugin_updater::{Update, UpdaterExt};
use tokio::sync::oneshot;

use crate::state_changed;

/// The first background check waits for the app to settle.
const FIRST_CHECK: Duration = Duration::from_secs(10);
const CHECK_EVERY: Duration = Duration::from_secs(60 * 60);

#[derive(Clone, Debug, Default, PartialEq, Serialize)]
#[serde(tag = "kind", content = "version", rename_all = "snake_case")]
pub enum Phase {
    #[default]
    Idle,
    Checking,
    /// Asking whether to install this version.
    Found(String),
    Downloading(String),
}

#[derive(Default)]
pub struct UpdateState {
    phase: Mutex<Phase>,
    /// A version the user put off; background checks don't ask about it again.
    later: Mutex<Option<String>>,
}

impl UpdateState {
    pub fn phase(&self) -> Phase {
        self.phase.lock().unwrap().clone()
    }
}

#[derive(Clone, Copy, PartialEq)]
enum Trigger {
    /// The hourly check: quiet unless there is an update.
    Background,
    /// The user asked: say something whatever the outcome.
    Manual,
}

pub fn init<R: Runtime>(app: &AppHandle<R>) {
    app.manage(UpdateState::default());
    // Local debug builds carry the base version; they would always be behind.
    if cfg!(debug_assertions) {
        return;
    }
    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(FIRST_CHECK).await;
        loop {
            check(&app, Trigger::Background).await;
            tokio::time::sleep(CHECK_EVERY).await;
        }
    });
}

/// "检查更新…" from the tray or the settings page.
pub fn check_now<R: Runtime>(app: &AppHandle<R>) {
    let app = app.clone();
    tauri::async_runtime::spawn(async move { check(&app, Trigger::Manual).await });
}

async fn check<R: Runtime>(app: &AppHandle<R>, trigger: Trigger) {
    let manual = trigger == Trigger::Manual;
    if let Some(why) = blocking_reason() {
        if manual {
            alert(app, MessageDialogKind::Warning, "无法更新", why).await;
        }
        return;
    }
    // One check at a time; a click while one runs is a no-op.
    if !set_phase_from(app, Phase::Idle, Phase::Checking) {
        return;
    }

    let found = match app.updater() {
        Ok(updater) => updater.check().await,
        Err(e) => Err(e),
    };
    let current = app.package_info().version.to_string();
    match found {
        Ok(Some(update)) => {
            let state = app.state::<UpdateState>();
            if !manual && state.later.lock().unwrap().as_deref() == Some(&update.version) {
                set_phase(app, Phase::Idle);
                return;
            }
            offer(app, update, &current).await;
        }
        Ok(None) => {
            set_phase(app, Phase::Idle);
            if manual {
                let text = format!("Moli Whisper {current} 已是最新版本。");
                alert(app, MessageDialogKind::Info, "已是最新版本", &text).await;
            }
        }
        Err(e) => {
            set_phase(app, Phase::Idle);
            log::warn!("update check failed: {e}");
            if manual {
                let text = format!("现在连不上更新源，稍后再试。\n\n{e}");
                alert(app, MessageDialogKind::Error, "检查更新失败", &text).await;
            }
        }
    }
}

/// Asks, then downloads, installs and restarts.
async fn offer<R: Runtime>(app: &AppHandle<R>, update: Update, current: &str) {
    let version = update.version.clone();
    log::info!("update available: {current} -> {version}");
    set_phase(app, Phase::Found(version.clone()));
    let text = format!(
        "Moli Whisper {version} 已发布，当前是 {current}。\n\n安装后会自动重启，辅助功能和麦克风授权保持不变。"
    );
    let install = ask(app, "发现新版本", &text, "立即更新", "稍后").await;
    if !install {
        *app.state::<UpdateState>().later.lock().unwrap() = Some(version);
        set_phase(app, Phase::Idle);
        return;
    }

    set_phase(app, Phase::Downloading(version.clone()));
    match update.download_and_install(|_, _| {}, || {}).await {
        Ok(()) => {
            log::info!("installed {version}, restarting");
            app.request_restart();
        }
        Err(e) => {
            log::error!("update to {version} failed: {e}");
            set_phase(app, Phase::Idle);
            let text = format!("下载或安装 {version} 时出错：{e}");
            alert(app, MessageDialogKind::Error, "更新失败", &text).await;
        }
    }
}

/// Why this copy can't replace itself: it runs from the disk image, or from
/// the read-only copy macOS makes of a quarantined app (App Translocation).
fn blocking_reason() -> Option<&'static str> {
    let exe = std::env::current_exe().ok()?;
    blocking_reason_for(&exe)
}

fn blocking_reason_for(exe: &Path) -> Option<&'static str> {
    if exe.starts_with("/Volumes") {
        return Some(
            "Moli Whisper 正在从磁盘映像运行，无法更新。请先把应用移到「应用程序」文件夹，再从那里打开。",
        );
    }
    if exe.to_string_lossy().contains("/AppTranslocation/") {
        return Some(
            "macOS 把 Moli Whisper 放在只读的临时位置运行，无法更新。请先把应用移到「应用程序」文件夹，再从那里打开。",
        );
    }
    None
}

fn set_phase<R: Runtime>(app: &AppHandle<R>, phase: Phase) {
    *app.state::<UpdateState>().phase.lock().unwrap() = phase;
    state_changed(app);
}

/// Moves `from` → `to`; false if the phase was something else.
fn set_phase_from<R: Runtime>(app: &AppHandle<R>, from: Phase, to: Phase) -> bool {
    {
        let state = app.state::<UpdateState>();
        let mut phase = state.phase.lock().unwrap();
        if *phase != from {
            return false;
        }
        *phase = to;
    }
    state_changed(app);
    true
}

async fn ask<R: Runtime>(
    app: &AppHandle<R>,
    title: &str,
    text: &str,
    ok: &str,
    cancel: &str,
) -> bool {
    let (tx, rx) = oneshot::channel();
    app.dialog()
        .message(text)
        .title(title)
        .buttons(MessageDialogButtons::OkCancelCustom(
            ok.into(),
            cancel.into(),
        ))
        .show(move |yes| {
            let _ = tx.send(yes);
        });
    rx.await.unwrap_or(false)
}

async fn alert<R: Runtime>(app: &AppHandle<R>, kind: MessageDialogKind, title: &str, text: &str) {
    let (tx, rx) = oneshot::channel();
    app.dialog()
        .message(text)
        .title(title)
        .kind(kind)
        .buttons(MessageDialogButtons::OkCustom("好".into()))
        .show(move |_| {
            let _ = tx.send(());
        });
    let _ = rx.await;
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn refuses_to_update_in_place_from_a_disk_image_or_translocation() {
        let dmg = Path::new("/Volumes/MoliWhisper/MoliWhisper.app/Contents/MacOS/moliwhisper");
        assert!(blocking_reason_for(dmg).is_some());
        let translocated = Path::new(
            "/private/var/folders/x/T/AppTranslocation/ABC/d/MoliWhisper.app/Contents/MacOS/moliwhisper",
        );
        assert!(blocking_reason_for(translocated).is_some());
        let installed = Path::new("/Applications/MoliWhisper.app/Contents/MacOS/moliwhisper");
        assert!(blocking_reason_for(installed).is_none());
    }
}
