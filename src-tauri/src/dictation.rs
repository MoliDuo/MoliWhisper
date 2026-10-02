//! Wires the session controller to the app: API key, microphone, delivery.

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use moli_core::audio::{self, AudioInput};
use moli_core::organize::Organizer;
use moli_core::qwen;
use moli_core::session::{Controller, Env, Outcome, Phase, Timings, Update};
use tauri::{AppHandle, Manager, Runtime};

use crate::hotkey::HotkeyState;
use crate::settings::Settings;
use crate::{overlay, platform, windows};

/// Longer than this and the text is pasted as it was recognized.
const ORGANIZE_TIMEOUT: Duration = Duration::from_secs(15);

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
    fn connect_options(&self) -> Option<qwen::ConnectOptions> {
        qwen::ConnectOptions::new(&self.app.state::<Settings>().get().qwen.api_key)
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
            .then(|| {
                Organizer::deepseek(&config.deepseek.api_key, config.deepseek.prompt.as_deref())
            })
            .flatten();
        async move {
            let text = match organizer {
                Some(organizer) => {
                    overlay::organizing(&app);
                    organizer
                        .organize(&text, ORGANIZE_TIMEOUT)
                        .await
                        .unwrap_or(text)
                }
                None => text,
            };
            platform::paste(&app, text, config.restore_clipboard).await
        }
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
                Outcome::NoKey | Outcome::KeyRejected => {
                    log::warn!("dictation needs a Qwen API key: {outcome:?}");
                    windows::show_settings(&self.app);
                }
                other => log::warn!("dictation failed: {other:?}"),
            }
        }
        overlay::update(&self.app, &update);
    }
}
