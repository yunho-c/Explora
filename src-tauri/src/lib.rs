mod commands;
mod filesystem;
mod local_filesystem;
mod ssh;
mod ssh_targets;
#[cfg(test)]
mod ssh_test_server;

use commands::AppState;
use filesystem::LocationRole;
use local_filesystem::{LocalFilesystem, LocalRoot};
use ssh_targets::SshTargetStore;
use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
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

            let filesystem = LocalFilesystem::new(roots)?;
            let ssh_targets = SshTargetStore::new(
                paths.app_config_dir()?.join("ssh-targets.json"),
                paths.home_dir()?,
            )?;
            app.manage(AppState::new(filesystem, ssh_targets));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
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
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
