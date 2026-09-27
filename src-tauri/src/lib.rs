mod tray;
mod windows;

use tauri::{Manager, RunEvent};
use tauri_plugin_log::{Target, TargetKind, TimezoneStrategy};

pub fn run() {
    let app = tauri::Builder::default()
        // Must come first: a second launch just opens the settings of the running one.
        .plugin(tauri_plugin_single_instance::init(|app, _argv, _cwd| {
            windows::show_settings(app);
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
            tray::create(app.handle())?;
            log::info!(
                "MoliWhisper {} started, data dir {:?}",
                app.package_info().version,
                app.path().app_data_dir().ok()
            );
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
