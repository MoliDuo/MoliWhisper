//! Wires the session controller to the app: login, microphone, delivery.

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use moli_core::asr::Backend;
use moli_core::audio::{self, AudioInput};
use moli_core::config::{BackendKind, OrganizerKind};
use moli_core::doubao::web::ConnectOptions;
use moli_core::organize::{OpenAiOrganizer, Organizer};
use moli_core::selfhost;
use moli_core::session::{Controller, Env, Outcome, Phase, Timings, Update};
use tauri::{AppHandle, Manager, Runtime};

use crate::auth::{Auth, AuthStatus};
use crate::hotkey::HotkeyState;
use crate::ime::{Ime, OPENAI_ORGANIZE_TIMEOUT, ORGANIZE_TIMEOUT};
use crate::settings::Settings;
use crate::{login, overlay, platform, state_changed};

pub struct Dictation(pub Controller);

pub fn init<R: Runtime>(app: &AppHandle<R>) {
    let env = Arc::new(AppEnv { app: app.clone() });
    let backend = app.state::<Settings>().get().backend;
    let (controller, actor) = Controller::new(env, timings(backend));
    tauri::async_runtime::spawn(actor);
    app.manage(Dictation(controller));
}

pub fn timings(backend: BackendKind) -> Timings {
    match backend {
        BackendKind::Web => Timings::default(),
        BackendKind::Ime => Timings::ime(),
        BackendKind::SelfHosted => Timings::default(),
    }
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

/// The organizer the settings ask for and how long to wait for it; `None`
/// (the text is pasted as it is) when the provider is not set up.
fn organizer<R: Runtime>(
    app: &AppHandle<R>,
    config: &moli_core::config::OrganizerConfig,
) -> Option<(Organizer, Duration)> {
    match config.provider {
        OrganizerKind::DoubaoIme => Some((
            Organizer::DoubaoIme(app.state::<Ime>().0.clone()),
            ORGANIZE_TIMEOUT,
        )),
        OrganizerKind::Openai => match OpenAiOrganizer::new(&config.openai) {
            Some(o) => Some((Organizer::OpenAi(o), OPENAI_ORGANIZE_TIMEOUT)),
            None => {
                log::warn!("the OpenAI organizer needs a base URL and a model");
                None
            }
        },
    }
}

struct AppEnv<R: Runtime> {
    app: AppHandle<R>,
}

impl<R: Runtime> Env for AppEnv<R> {
    fn backend(&self) -> Option<Backend> {
        let config = self.app.state::<Settings>().get();
        match config.backend {
            BackendKind::Ime => return Some(Backend::Ime(self.app.state::<Ime>().0.clone())),
            BackendKind::SelfHosted => {
                let opts =
                    selfhost::ConnectOptions::new(&config.asr_server.url, &config.asr_server.token);
                if opts.is_none() {
                    log::warn!("the self-hosted ASR server URL is missing or invalid");
                }
                return opts.map(Backend::SelfHosted);
            }
            BackendKind::Web => {}
        }
        let auth = self.app.state::<Auth>();
        match auth.status() {
            AuthStatus::Active { .. } => {}
            AuthStatus::LoggedOut | AuthStatus::Rejected | AuthStatus::Expired => return None,
        }
        let creds = auth.credentials()?;
        Some(Backend::Web(ConnectOptions::new(
            &creds,
            &config.asr.param_overrides,
        )))
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
        let config = app.state::<Settings>().get();
        let organizer = config
            .organize
            .then(|| organizer(&app, &config.organizer))
            .flatten();
        async move {
            let text = match organizer {
                Some((organizer, timeout)) => {
                    overlay::organizing(&app);
                    organizer.organize(&text, timeout).await.unwrap_or(text)
                }
                None => text,
            };
            platform::paste(&app, text, config.restore_clipboard).await
        }
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
                Outcome::NeedLogin | Outcome::SessionRejected
                    if self.app.state::<Settings>().get().backend != BackendKind::Web =>
                {
                    log::warn!("dictation is not set up: {outcome:?}");
                }
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
