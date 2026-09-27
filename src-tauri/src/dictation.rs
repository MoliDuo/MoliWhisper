//! Wires the session controller to the app: login, microphone, delivery.

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use moli_core::asr::ConnectOptions;
use moli_core::asr::params::Overrides;
use moli_core::audio::{self, AudioInput};
use moli_core::session::{Controller, Env, Outcome, Timings, Update};
use tauri::{AppHandle, Manager, Runtime};

use crate::auth::{Auth, AuthStatus};
use crate::tray;

pub struct Dictation(pub Controller);

pub fn init<R: Runtime>(app: &AppHandle<R>) {
    let env = Arc::new(AppEnv { app: app.clone() });
    let (controller, actor) = Controller::new(env, Timings::default());
    tauri::async_runtime::spawn(actor);
    app.manage(Dictation(controller));
}

/// Records for `secs` seconds, then stops. A stand-in for the hotkey.
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
        Some(ConnectOptions::new(&creds, &Overrides::new()))
    }

    fn start_audio(&self) -> AudioInput {
        audio::capture::start()
    }

    fn deliver(&self, text: String) -> impl Future<Output = Result<(), String>> + Send + 'static {
        // Paste lands in M6; until then the log is the output.
        log::info!("recognized: {text}");
        async { Ok(()) }
    }

    fn session_rejected(&self) {
        self.app.state::<Auth>().mark_rejected();
        tray::refresh(&self.app);
    }

    fn update(&self, update: Update) {
        if let Update::Outcome(outcome) = update {
            match outcome {
                Outcome::NeedLogin => log::warn!("dictation needs a login first"),
                Outcome::Done { .. } | Outcome::Empty | Outcome::Cancelled => {}
                other => log::warn!("dictation failed: {other:?}"),
            }
        }
    }
}
