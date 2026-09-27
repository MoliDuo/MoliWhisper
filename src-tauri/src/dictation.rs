//! Wires the session controller to the app: login, microphone, delivery.

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use moli_core::asr::ConnectOptions;
use moli_core::audio::{self, AudioInput};
use moli_core::session::{Controller, Env, Outcome, Phase, Timings, Update};
use tauri::{AppHandle, Manager, Runtime};

use crate::auth::{Auth, AuthStatus};
use crate::hotkey::HotkeyState;
use crate::settings::Settings;
use crate::{login, overlay, platform, state_changed};

pub struct Dictation(pub Controller);

pub fn init<R: Runtime>(app: &AppHandle<R>) {
    let env = Arc::new(AppEnv { app: app.clone() });
    let (controller, actor) = Controller::new(env, Timings::default());
    tauri::async_runtime::spawn(actor);
    app.manage(Dictation(controller));
}

/// Records for `secs` seconds, then stops (`--dictate`, for testing).
pub fn dictate_for<R: Runtime>(app: &AppHandle<R>, secs: u64) {
    let controller = app.state::<Dictation>().0.clone();
    controller.start();
    tauri::async_runtime::spawn(async move {
        tokio::time::sleep(Duration::from_secs(secs)).await;
        controller.stop();
    });
}

struct AppEnv<R: Runtime> {
    app: AppHandle<R>,
}

impl<R: Runtime> Env for AppEnv<R> {
    fn connect_options(&self) -> Option<ConnectOptions> {
        let auth = self.app.state::<Auth>();
        match auth.status() {
            AuthStatus::Active { .. } => {}
            AuthStatus::LoggedOut | AuthStatus::Rejected | AuthStatus::Expired => return None,
        }
        let creds = auth.credentials()?;
        let overrides = self.app.state::<Settings>().get().asr.param_overrides;
        Some(ConnectOptions::new(&creds, &overrides))
    }

    fn start_audio(&self) -> AudioInput {
        #[cfg(debug_assertions)]
        if let Some(input) = crate::test_audio::from_env() {
            return input;
        }
        audio::capture::start()
    }

    fn deliver(&self, text: String) -> impl Future<Output = Result<(), String>> + Send + 'static {
        log::info!("recognized {} characters", text.chars().count());
        let app = self.app.clone();
        let restore = app.state::<Settings>().get().restore_clipboard;
        async move { platform::paste(&app, text, restore).await }
    }

    fn session_rejected(&self) {
        self.app.state::<Auth>().mark_rejected();
        state_changed(&self.app);
    }

    fn update(&self, update: Update) {
        if let Update::Phase(phase) = update
            && let Some(hotkey) = self.app.try_state::<HotkeyState>()
        {
            let active = matches!(
                phase,
                Phase::Connecting | Phase::Recording | Phase::Finalizing
            );
            hotkey.0.set_active(active);
        }
        if let Update::Outcome(outcome) = &update {
            match outcome {
                Outcome::Done { .. } | Outcome::Empty | Outcome::Cancelled => {}
                Outcome::NeedLogin | Outcome::SessionRejected => {
                    log::warn!("dictation needs a login: {outcome:?}");
                    let fresh = matches!(self.app.state::<Auth>().status(), AuthStatus::Rejected)
                        || *outcome == Outcome::SessionRejected;
                    login::open(&self.app, fresh);
                }
                other => log::warn!("dictation failed: {other:?}"),
            }
        }
        overlay::update(&self.app, &update);
    }
}
