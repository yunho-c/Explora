mod commands;
mod file_operations;
mod filesystem;
mod local_filesystem;
mod platform_trash;
mod preferences;
mod preview;
mod ssh;
mod ssh_targets;
#[cfg(test)]
mod ssh_test_server;
mod volumes;

use commands::AppState;
use filesystem::LocationRole;
use local_filesystem::{LocalFilesystem, LocalRoot};
use preferences::PreferencesStore;
use serde::Serialize;
use ssh_targets::SshTargetStore;
use tauri::{Manager, WebviewWindow};
use tauri_plugin_decoration::WebviewWindowExt;
use volumes::VolumeManager;

#[derive(Debug, Serialize)]
#[serde(rename_all = "camelCase")]
struct WindowChromeError {
    code: &'static str,
    message: String,
}

impl WindowChromeError {
    fn new(code: &'static str, message: impl Into<String>) -> Self {
        Self {
            code,
            message: message.into(),
        }
    }
}

#[tauri::command]
fn activate_custom_titlebar(window: WebviewWindow) -> Result<(), WindowChromeError> {
    window.create_overlay_titlebar().map_err(|error| {
        WindowChromeError::new(
            "activationFailed",
            format!("failed to activate custom window decorations: {error}"),
        )
    })?;

    #[cfg(target_os = "macos")]
    window
        .set_traffic_lights_inset(14.0, 18.0)
        .map_err(|error| {
            WindowChromeError::new(
                "trafficLightsFailed",
                format!("failed to position native window controls: {error}"),
            )
        })?;

    Ok(())
}

#[tauri::command]
fn show_native_titlebar_fallback(window: WebviewWindow) -> Result<(), WindowChromeError> {
    // Showing the window is attempted even when restoration fails. A partially
    // styled but visible window is recoverable; an indefinitely hidden one is not.
    let restore_result = window.restore_native_titlebar();
    let show_result = window.show();

    if let Err(error) = show_result {
        return Err(WindowChromeError::new(
            "showFailed",
            format!("failed to reveal the native-titlebar fallback: {error}"),
        ));
    }

    restore_result.map(|_| ()).map_err(|error| {
        WindowChromeError::new(
            "restoreFailed",
            format!("failed to restore native window decorations: {error}"),
        )
    })
}

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_decoration::init())
        .setup(|app| {
            let paths = app.path();
            let mut roots = vec![LocalRoot {
                id: "home",
                name: "Home",
                role: LocationRole::Home,
                path: paths.home_dir()?,
            }];

            let videos_name = if cfg!(target_os = "macos") {
                "Movies"
            } else {
                "Videos"
            };
            for (id, name, role, path) in [
                (
                    "desktop",
                    "Desktop",
                    LocationRole::Desktop,
                    paths.desktop_dir(),
                ),
                (
                    "documents",
                    "Documents",
                    LocationRole::Documents,
                    paths.document_dir(),
                ),
                (
                    "downloads",
                    "Downloads",
                    LocationRole::Downloads,
                    paths.download_dir(),
                ),
                (
                    "pictures",
                    "Pictures",
                    LocationRole::Pictures,
                    paths.picture_dir(),
                ),
                ("music", "Music", LocationRole::Music, paths.audio_dir()),
                (
                    "videos",
                    videos_name,
                    LocationRole::Videos,
                    paths.video_dir(),
                ),
            ] {
                if let Ok(path) = path {
                    roots.push(LocalRoot {
                        id,
                        name,
                        role,
                        path,
                    });
                }
            }

            let filesystem = std::sync::Arc::new(LocalFilesystem::new(roots)?);
            let volumes = VolumeManager::start(filesystem.clone())?;
            let preferences =
                PreferencesStore::new(paths.app_config_dir()?.join("preferences.json"));
            let ssh_targets = SshTargetStore::new(
                paths.app_config_dir()?.join("ssh-targets.json"),
                paths.home_dir()?,
            )?;
            app.manage(AppState::new(filesystem, preferences, ssh_targets, volumes));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            activate_custom_titlebar,
            show_native_titlebar_fallback,
            commands::watch_volumes,
            commands::cancel_volume_watch,
            commands::get_user_preferences,
            commands::update_user_preferences,
            commands::list_locations,
            commands::list_ssh_targets,
            commands::create_ssh_target,
            commands::update_ssh_target,
            commands::delete_ssh_target,
            commands::connect_ssh_target,
            commands::respond_ssh_prompt,
            commands::cancel_ssh_connection,
            commands::disconnect_ssh_target,
            commands::list_directory,
            commands::cancel_listing,
            commands::start_file_operation,
            commands::respond_file_operation,
            commands::cancel_file_operation,
            commands::prepare_preview,
            commands::cancel_preview,
            commands::read_preview_resource,
            commands::discard_preview_resource,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}

#[cfg(test)]
mod window_chrome_tests {
    use super::WindowChromeError;

    #[test]
    fn window_chrome_errors_keep_a_stable_structured_shape() {
        let value = serde_json::to_value(WindowChromeError::new(
            "activationFailed",
            "custom decorations are unavailable",
        ))
        .expect("window chrome errors should serialize");

        assert_eq!(value["code"], "activationFailed");
        assert_eq!(value["message"], "custom decorations are unavailable");
    }
}
