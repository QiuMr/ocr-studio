use tauri::Manager;

pub mod api;
pub mod capture;
pub mod commands;
pub mod config;
/// 模板匹配。文件名 `match.rs` 与 Rust 关键字冲突，模块名对外暴露为 `matcher`。
#[path = "match.rs"]
pub mod matcher;
pub mod ocr;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    tracing_subscriber::fmt()
        .with_max_level(tracing::Level::INFO)
        .init();

    tauri::Builder::default()
        .plugin(tauri_plugin_opener::init())
        .setup(|app| {
            let state = api::start_for_tauri(app.handle());
            app.manage(state);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            commands::ocr_recognize,
            commands::get_health,
            commands::get_config,
            commands::save_config,
            commands::set_level,
            commands::set_device,
            commands::server_status,
            commands::server_start,
            commands::server_stop,
            commands::models_root,
            commands::find_image,
            commands::screen_preview,
            commands::list_windows,
        ])
        .run(tauri::generate_context!())
        .expect("error while running tauri application");
}
