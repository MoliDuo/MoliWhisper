mod auth;
mod login;
mod tray;
mod windows;

use tauri::{AppHandle, Manager, RunEvent, Runtime};
use tauri_plugin_log::{Target, TargetKind, TimezoneStrategy};

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
        .setup(|app| {
            #[cfg(target_os = "macos")]
            app.set_activation_policy(tauri::ActivationPolicy::Accessory);
            let data_dir = app.path().app_data_dir()?;
            log::info!(
                "MoliWhisper {} started, data dir {data_dir:?}",
                app.package_info().version
            );
            app.manage(auth::Auth::load(&data_dir));
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

/// `--login` opens the login window, `--logout` forgets the session. Returns
/// whether any argument was acted on.
fn handle_args<R: Runtime>(app: &AppHandle<R>, argv: &[String]) -> bool {
    let mut handled = false;
    for arg in argv.iter().skip(1) {
        match arg.as_str() {
            "--login" => login::open(app, false),
            "--logout" => {
                tauri::async_runtime::spawn(login::logout(app.clone()));
            }
            _ => continue,
        }
        handled = true;
    }
    handled
}
