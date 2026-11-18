mod api;
mod audio;
mod export;
mod models;
mod queue;
mod watcher;
mod whisper;

use api::AppBackend;
use queue::QueueManager;
use watcher::FolderWatcher;

#[cfg_attr(mobile, tauri::mobile_entry_point)]
pub fn run() {
    let queue = QueueManager::new(None);
    queue.start_worker();
    let watcher = FolderWatcher::new(queue.clone());
    let port = api::resolve_api_port();

    let backend = AppBackend {
        queue: queue.clone(),
        watcher,
        port,
    };
    let backend_for_setup = backend.clone();

    tauri::Builder::default()
        .manage(backend)
        .plugin(tauri_plugin_opener::init())
        .plugin(tauri_plugin_dialog::init())
        .setup(move |app| {
            queue.attach_app_handle(app.handle().clone());
            api::start_http_server(backend_for_setup);
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            api::get_app_state,
            api::get_api_port,
            api::enqueue_files,
            api::set_watch_folder,
            api::stop_watch_folder,
            api::retry_item,
            api::force_retranscribe,
            api::remove_item,
            api::clear_completed,
            api::clear_errors,
            api::read_export_text,
        ])
        .run(tauri::generate_context!())
        .expect("error while running Audio Transcription Studio");
}
