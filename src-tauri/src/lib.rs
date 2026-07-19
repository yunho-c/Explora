mod commands;
mod local_filesystem;

use commands::AppState;
use local_filesystem::{LocalFilesystem, LocalRoot};
use tauri::Manager;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .setup(|app| {
            let paths = app.path();
            let mut roots = vec![LocalRoot {
                id: "home",
                name: "Home",
                path: paths.home_dir()?,
            }];

            for (id, name, path) in [
                ("desktop", "Desktop", paths.desktop_dir()),
                ("documents", "Documents", paths.document_dir()),
                ("downloads", "Downloads", paths.download_dir()),
            ] {
                if let Ok(path) = path {
                    roots.push(LocalRoot { id, name, path });
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
