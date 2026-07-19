mod commands;
mod local_filesystem;

use commands::AppState;
use local_filesystem::{LocalFilesystem, LocalRoot, LocationRole};
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
            app.manage(AppState::new(filesystem));
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::list_local_locations,
            commands::list_local_directory,
            commands::cancel_local_listing,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
