//! Global hotkey: the platform hook feeds key events through the matcher and
//! the resulting actions drive the dictation controller.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Instant;

use moli_core::hotkey::{Action, Hotkey, KeyEvent, Matcher, Mode, Recorded};
use serde::Serialize;
use tauri::{AppHandle, Emitter, Manager, Runtime};
use tokio::sync::mpsc;

use crate::dictation::Dictation;
use crate::settings::Settings;
use crate::{platform, state_changed};

// Unused until the Windows hook lands (M8).
#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(tag = "kind", content = "detail", rename_all = "snake_case")]
pub enum HookStatus {
    Starting,
    NeedsPermission,
    Running,
    #[cfg_attr(target_os = "macos", allow(dead_code))]
    Unsupported(String),
    Failed(String),
}

#[cfg_attr(not(target_os = "macos"), allow(dead_code))]
enum Msg {
    Action(Action),
    Recorded(Recorded),
    Status,
}

/// Shared between the hook thread and the app.
pub struct Hook {
    matcher: Mutex<Matcher>,
    /// A session is connecting, recording or finalizing (Escape cancels it).
    active: AtomicBool,
    status: Mutex<HookStatus>,
    tx: mpsc::UnboundedSender<Msg>,
}

impl Hook {
    /// Runs on the hook thread for every key event. Returns whether to
    /// swallow the event; must not block.
    #[cfg_attr(not(target_os = "macos"), allow(dead_code))]
    pub fn handle(&self, ev: KeyEvent) -> bool {
        let active = self.active.load(Ordering::Relaxed);
        let d = self
            .matcher
            .lock()
            .unwrap()
            .handle(&ev, Instant::now(), active);
        if let Some(a) = d.action {
            let _ = self.tx.send(Msg::Action(a));
        }
        if let Some(r) = d.recorded {
            let _ = self.tx.send(Msg::Recorded(r));
        }
        d.swallow
    }

    pub fn set_status(&self, status: HookStatus) {
        let mut current = self.status.lock().unwrap();
        if *current != status {
            log::info!("key hook: {status:?}");
            *current = status;
            let _ = self.tx.send(Msg::Status);
        }
    }

    pub fn status(&self) -> HookStatus {
        self.status.lock().unwrap().clone()
    }

    pub fn set_active(&self, active: bool) {
        self.active.store(active, Ordering::Relaxed);
    }

    pub fn configure(&self, hotkey: Hotkey, mode: Mode) {
        self.matcher.lock().unwrap().set(hotkey, mode);
    }

    pub fn start_recording(&self) {
        self.matcher.lock().unwrap().start_recording();
    }

    pub fn stop_recording(&self) {
        self.matcher.lock().unwrap().stop_recording();
    }

    pub fn is_recording(&self) -> bool {
        self.matcher.lock().unwrap().is_recording()
    }
}

pub struct HotkeyState(pub Arc<Hook>);

pub fn init<R: Runtime>(app: &AppHandle<R>) {
    let config = app.state::<Settings>().get();
    let (tx, mut rx) = mpsc::unbounded_channel();
    let hook = Arc::new(Hook {
        matcher: Mutex::new(Matcher::new(config.hotkey, config.mode, platform::ESCAPE)),
        active: AtomicBool::new(false),
        status: Mutex::new(HookStatus::Starting),
        tx,
    });
    app.manage(HotkeyState(hook.clone()));
    if !platform::accessibility_trusted() {
        platform::request_accessibility();
    }
    platform::start_key_hook(hook.clone());

    let app = app.clone();
    tauri::async_runtime::spawn(async move {
        while let Some(msg) = rx.recv().await {
            match msg {
                Msg::Action(action) => {
                    log::debug!("hotkey action {action:?}");
                    let controller = &app.state::<Dictation>().0;
                    match action {
                        Action::Toggle => controller.toggle(),
                        Action::Start => controller.start(),
                        Action::Stop => controller.stop(),
                        Action::Cancel => controller.cancel(),
                    }
                }
                Msg::Recorded(recorded) => {
                    if let Recorded::Hotkey(hotkey) = recorded {
                        match app.state::<Settings>().update(|c| c.hotkey = hotkey) {
                            Ok(c) => {
                                hook.configure(c.hotkey, c.mode);
                                log::info!("hotkey set to {}", label(&c.hotkey));
                            }
                            Err(e) => log::error!("{e}"),
                        }
                    }
                    let _ = app.emit_to("settings", "hotkey-recorded", ());
                    state_changed(&app);
                }
                Msg::Status => state_changed(&app),
            }
        }
    });
}

pub fn label(hotkey: &Hotkey) -> String {
    hotkey.label(cfg!(target_os = "macos"), platform::key_label)
}

/// A one-line hint of how to dictate, for the tray and the settings page.
pub fn usage(hotkey: &Hotkey, mode: Mode) -> String {
    let key = label(hotkey);
    match (mode, hotkey) {
        (Mode::Toggle, Hotkey::Modifier { .. }) => format!("单击 {key} 开始 / 结束"),
        (Mode::Toggle, Hotkey::Combo { .. }) => format!("按 {key} 开始 / 结束"),
        (Mode::PushToTalk, _) => format!("按住 {key} 说话，松开结束"),
    }
}
