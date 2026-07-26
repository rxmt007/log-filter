mod commands;
mod dto;
mod problems;
mod state;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .manage(state::AppState::new())
        .invoke_handler(tauri::generate_handler![
            commands::open_file,
            commands::list_devices,
            commands::start_logcat,
            commands::pause_logcat,
            commands::resume_logcat,
            commands::stop_logcat,
            commands::clear_logcat,
            commands::get_status,
            problems::get_problems_status,
            problems::get_problem_groups,
            problems::get_problem_occurrences,
            problems::get_problem_detail,
            problems::release_problem_snapshot,
            commands::get_rows,
            commands::get_rows_checked,
            commands::map_source_line,
            commands::set_filter,
            commands::get_filtered_count,
            commands::search,
            commands::search_next,
            commands::toggle_bookmark,
            commands::list_bookmarks,
            commands::next_bookmark,
            commands::line_to_result_index,
            commands::get_minimap,
            commands::export_logs,
            commands::export_problem_logs,
            commands::cancel_export,
            commands::split_log_file,
            commands::get_config,
            commands::set_config
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
