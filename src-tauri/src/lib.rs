mod commands;
mod dto;
mod state;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(state::AppState::new())
        .invoke_handler(tauri::generate_handler![
            commands::open_file,
            commands::get_status,
            commands::get_rows
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
