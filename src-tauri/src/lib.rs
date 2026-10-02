mod auth;
mod commands;
mod dictation;
mod hotkey;
mod ime;
mod login;
mod overlay;
mod platform;
mod settings;
#[cfg(debug_assertions)]
mod test_audio;
mod tray;
mod updater;
mod windows;

use moli_core::config::BackendKind;
use tauri::{AppHandle, Emitter, Manager, RunEvent, Runtime};
use tauri_plugin_autostart::MacosLauncher;
use tauri_plugin_log::{Target, TargetKind, TimezoneStrategy};

/// Length of the test dictation from `--dictate`.
pub const TEST_DICTATION_SECS: u64 = 5;

pub fn run() {
    let app = tauri::Builder::default()
        // Must come first: a second launch hands its arguments to the running one.
        .plugin(tauri_plugin_single_instance::init(|app, argv, _cwd| {
            if !handle_args(app, &argv) {
                windows::show_settings(app);
            }
        }))
        .plugin(
            tauri_plugin_log::Builder::new()
                .targets([
                    Target::new(TargetKind::Stdout),
                    Target::new(TargetKind::LogDir {
                        file_name: Some("moliwhisper".into()),
                    }),
                ])
                .timezone_strategy(TimezoneStrategy::UseLocal)
                .level(log::LevelFilter::Info)
                .level_for("moli_core", log::LevelFilter::Debug)
                .build(),
        )
        .plugin(tauri_plugin_autostart::init(
            MacosLauncher::LaunchAgent,
            None,
        ))
        .plugin(tauri_plugin_dialog::init())
        .plugin(tauri_plugin_updater::Builder::new().build())
        .invoke_handler(tauri::generate_handler![
            commands::get_state,
            commands::set_mode,
            commands::set_restore_clipboard,
            commands::set_backend,
            commands::set_organize,
            commands::set_asr_server,
            commands::test_asr_server,
            commands::set_organizer,
            commands::test_organizer,
            commands::set_autostart,
            commands::set_overrides,
            commands::record_hotkey,
            commands::cancel_record_hotkey,
            commands::reset_hotkey,
            commands::open_accessibility_settings,
            commands::open_microphone_settings,
            commands::login,
            commands::logout,
            commands::check_for_updates,
        ])
        .setup(|app| {
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);
            let data_dir = app.path().app_data_dir()?;
            log::info!(
                "MoliWhisper {} started, data dir {data_dir:?}",
                app.package_info().version
            );
            app.manage(auth::Auth::load(&data_dir));
            app.manage(settings::Settings::load(&data_dir));
            updater::init(app.handle());
            ime::init(app.handle())?;
            let backend = app.state::<settings::Settings>().get().backend;
            ime::keep_warm(app.handle(), backend == BackendKind::Ime);
            overlay::create(app.handle())?;
            dictation::init(app.handle());
            hotkey::init(app.handle());
            tray::create(app.handle())?;
            handle_args(app.handle(), &std::env::args().collect::<Vec<_>>());
            Ok(())
        })
        .build(tauri::generate_context!())
        .expect("failed to build the Tauri app");

    app.run(|_app, event| {
        // A tray app keeps running with no windows open; only the tray's Quit exits.
        if let RunEvent::ExitRequested {
            code: None, api, ..
        } = event
        {
            api.prevent_exit();
        }
    });
}

/// Something the tray or the settings page shows changed.
pub fn state_changed<R: Runtime>(app: &AppHandle<R>) {
    tray::refresh(app);
    let _ = app.emit_to("settings", "settings-changed", ());
}

/// `--login` opens the login window, `--logout` forgets the session,
/// `--dictate` records for a few seconds, `--check-update` checks for a new
/// version. Returns whether any argument was acted on.
fn handle_args<R: Runtime>(app: &AppHandle<R>, argv: &[String]) -> bool {
    let mut handled = false;
    for arg in argv.iter().skip(1) {
        match arg.as_str() {
            "--login" => login::open(app, false),
            "--logout" => {
                tauri::async_runtime::spawn(login::logout(app.clone()));
            }
            "--dictate" => dictation::dictate_for(app, TEST_DICTATION_SECS),
            "--check-update" => updater::check_now(app),
            _ => continue,
        }
        handled = true;
    }
    handled
}
