#![cfg_attr(not(debug_assertions), windows_subsystem = "windows")]

mod batch;
mod cache;
mod desktop;
#[cfg(target_os = "macos")]
mod downloads;
mod immich;
mod native_drag;
mod policy;

use tauri::Manager;
use tauri_plugin_window_state::StateFlags;
const WINDOW_STATE_FLAGS: StateFlags = StateFlags::SIZE
    .union(StateFlags::POSITION)
    .union(StateFlags::MAXIMIZED)
    .union(StateFlags::FULLSCREEN);

#[tauri::command]
async fn drag_test_file(
    window: tauri::WebviewWindow,
    state: tauri::State<'_, immich::DesktopState>,
    on_event: cache::Events,
    delay_ms: u64,
    fresh: bool,
) -> Result<(), String> {
    if window.label() != "lab" {
        return Err("Only the local drag lab can drag the fixture".into());
    }
    let cache = state.cache.clone();
    let preview = native_drag::Preview::fixture()?;
    let (tx, rx) = tokio::sync::oneshot::channel();
    let main_thread = window.clone();
    window
        .run_on_main_thread(move || {
            let result = if !main_thread
                .app_handle()
                .state::<immich::DesktopState>()
                .resetting
                .load(std::sync::atomic::Ordering::Acquire)
                && native_drag::mouse_down()
            {
                cache
                    .fixture(on_event, delay_ms.min(30_000), fresh)
                    .and_then(|transfer| native_drag::start(main_thread, transfer, preview))
            } else {
                Err("Mouse released before native drag started".into())
            };
            let _ = tx.send(result);
        })
        .map_err(|_| "Cannot schedule native drag")?;
    rx.await.map_err(|_| "Native drag interrupted")?
}

fn main() {
    tauri::Builder::default()
        .plugin(
            tauri_plugin_window_state::Builder::default()
                .with_filter(|label| label == "immich")
                .with_state_flags(WINDOW_STATE_FLAGS)
                .build(),
        )
        .on_window_event(|window, event| {
            if window.label() == "immich"
                && matches!(event, tauri::WindowEvent::CloseRequested { .. })
            {
                window
                    .app_handle()
                    .state::<immich::DesktopState>()
                    .batches
                    .cancel_current();
                window.app_handle().exit(0);
            }
        })
        .setup(|app| {
            let root = app.path().app_cache_dir()?;
            std::fs::create_dir_all(&root)?;
            #[cfg(unix)]
            {
                use std::os::unix::fs::PermissionsExt;
                std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700))?;
            }
            let lock = std::fs::OpenOptions::new()
                .create(true)
                .truncate(false)
                .read(true)
                .write(true)
                .open(root.join("desktop.lock"))?;
            fs2::FileExt::try_lock_exclusive(&lock).map_err(|_| {
                "Another Immich Desktop instance is using this cache; close it first"
            })?;
            app.manage(immich::DesktopState {
                profile: std::sync::Mutex::new(None),
                _cache_lock: lock,
                cache: cache::Cache::new(root.join("originals"))?,
                resetting: std::sync::atomic::AtomicBool::new(false),
                batches: std::sync::Arc::new(batch::Control::default()),
            });
            desktop::install_menu(app)?;
            desktop::startup(app.handle().clone());
            Ok(())
        })
        .invoke_handler(tauri::generate_handler![
            drag_test_file,
            immich::get_settings,
            immich::save_settings,
            immich::open_immich,
            immich::prepare_asset,
            immich::drag_asset,
            immich::drag_assets,
            immich::cancel_drag,
            desktop::reset_configuration
        ])
        .build(tauri::generate_context!())
        .expect("Unable to run Immich Desktop")
        .run(|app, event| {
            #[cfg(target_os = "macos")]
            if let tauri::RunEvent::Exit = event {
                let _ = downloads::close_all(app);
            }
            if let tauri::RunEvent::ExitRequested { api, .. } = event {
                if cache::defer_exit(app.clone()) {
                    api.prevent_exit();
                }
            }
        });
}
