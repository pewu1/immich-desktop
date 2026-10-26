//! Native-only menus and lifecycle. Remote Immich gets no reset or launcher capability.
use crate::{cache, immich};
use std::sync::atomic::Ordering;
use tauri::{
    menu::{Menu, MenuItem, PredefinedMenuItem, Submenu},
    AppHandle, Manager, WebviewUrl, WebviewWindow, WebviewWindowBuilder,
};
use tauri_plugin_window_state::AppHandleExt;

pub fn reset_pending(app: &AppHandle) -> bool {
    immich::config_path(app).is_ok_and(|p| p.with_file_name("reset-pending.json").exists())
}

pub fn startup(app: AppHandle) {
    if reset_pending(&app) {
        let _ = show_reset(&app);
    } else if immich::read_settings(&app).is_ok_and(|s| !s.server.is_empty()) {
        tauri::async_runtime::spawn(async move {
            let state = app.state::<immich::DesktopState>();
            if immich::open_configured(app.clone(), &state).await.is_err() {
                if let Some(setup) = app.get_webview_window("main") {
                    let _ =
                        setup.set_title("Immich Desktop — Setup (saved connection unavailable)");
                    let _ = setup.show();
                }
            }
        });
    } else if let Some(setup) = app.get_webview_window("main") {
        let _ = setup.show();
    }
}

pub fn install_menu(app: &tauri::App) -> tauri::Result<()> {
    #[cfg(debug_assertions)]
    app.add_capability(
        serde_json::json!({
            "identifier": "local-lab", "local": true, "windows": ["lab"],
            "permissions": ["allow-drag-test-file"]
        })
        .to_string(),
    )?;
    let about = MenuItem::with_id(app, "about", "About Immich Desktop", true, None::<&str>)?;
    let reset = MenuItem::with_id(
        app,
        "reset",
        "Reset configuration and login…",
        true,
        None::<&str>,
    )?;
    let application = Submenu::with_items(
        app,
        "Immich Desktop",
        true,
        &[
            &about,
            &PredefinedMenuItem::separator(app)?,
            &reset,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::quit(app, None)?,
        ],
    )?;
    // Preserve standard webview editing shortcuts (login, search, clipboard).
    let edit = Submenu::with_items(
        app,
        "Edit",
        true,
        &[
            &PredefinedMenuItem::undo(app, None)?,
            &PredefinedMenuItem::redo(app, None)?,
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::cut(app, None)?,
            &PredefinedMenuItem::copy(app, None)?,
            &PredefinedMenuItem::paste(app, None)?,
            &PredefinedMenuItem::select_all(app, None)?,
        ],
    )?;
    let menu = Menu::with_items(app, &[&application, &edit])?;
    #[cfg(debug_assertions)]
    menu.append(&Submenu::with_items(
        app,
        "Development",
        true,
        &[&MenuItem::with_id(
            app,
            "lab",
            "Native drag lab",
            true,
            None::<&str>,
        )?],
    )?)?;
    app.set_menu(menu)?;
    app.on_menu_event(|app, event| {
        let app = app.clone();
        let action = event.id().as_ref().to_owned();
        // Window creation must not block WebView2's main-thread event handler.
        std::thread::spawn(move || {
            let result = match action.as_str() {
                "about" => show_about(&app),
                "reset" => show_reset(&app),
                #[cfg(debug_assertions)]
                "lab" => show_local(&app, "lab", "Native drag lab", "lab.html", 800.0, 640.0),
                _ => Ok(()),
            };
            if let Err(error) = result {
                eprintln!("Cannot open application menu window: {error}");
            }
        });
    });
    Ok(())
}

fn show_local(
    app: &AppHandle,
    label: &str,
    title: &str,
    page: &str,
    width: f64,
    height: f64,
) -> tauri::Result<()> {
    if let Some(window) = app.get_webview_window(label) {
        window.show()?;
        return window.set_focus();
    }
    WebviewWindowBuilder::new(app, label, WebviewUrl::App(page.into()))
        .title(title)
        .inner_size(width, height)
        .disable_drag_drop_handler()
        .build()?;
    Ok(())
}

fn show_reset(app: &AppHandle) -> tauri::Result<()> {
    show_local(
        app,
        "reset",
        "Reset Immich Desktop",
        "reset.html",
        520.0,
        340.0,
    )
}

fn show_about(app: &AppHandle) -> tauri::Result<()> {
    if let Some(window) = app.get_webview_window("about") {
        window.show()?;
        return window.set_focus();
    }
    WebviewWindowBuilder::new(app, "about", WebviewUrl::App("about.html".into()))
        .title("About Immich Desktop")
        .inner_size(560.0, 600.0)
        .disable_drag_drop_handler()
        .on_navigation(|url| {
            let external = matches!(
                url.as_str(),
                "https://github.com/pewu1/ImmichDesktop"
                    | "https://www.gnu.org/licenses/gpl-3.0.html"
            );
            if external {
                // Only these constant, menu-owned links can launch the system browser.
                #[cfg(target_os = "macos")]
                let _ = std::process::Command::new("open").arg(url.as_str()).spawn();
                #[cfg(windows)]
                let _ = std::process::Command::new("rundll32.exe")
                    .args(["url.dll,FileProtocolHandler", url.as_str()])
                    .spawn();
            }
            !external
                && matches!(url.scheme(), "tauri" | "http" | "https")
                && matches!(url.host_str(), Some("localhost" | "tauri.localhost"))
        })
        .build()?;
    Ok(())
}

#[tauri::command]
pub async fn reset_configuration(
    window: WebviewWindow,
    app: AppHandle,
    state: tauri::State<'_, immich::DesktopState>,
) -> Result<(), String> {
    if window.label() != "reset" {
        return Err("Reset is available only in the local reset window".into());
    }
    if state.resetting.swap(true, Ordering::AcqRel) {
        return Err("Reset is already in progress".into());
    }
    let result = reset(&app, &state).await.and_then(|()| {
        // Tauri may spawn the replacement before this process exits. Release the
        // cache lock only after reset and all native readers finish.
        fs2::FileExt::unlock(&state._cache_lock)
            .map_err(|_| "Cannot release cache for restart".into())
    });
    if result.is_err() {
        state.resetting.store(false, Ordering::Release);
        return result;
    }
    // A fresh process also removes the old origin's runtime capability.
    app.restart();
}

async fn save_window_layout(app: &AppHandle) {
    let (tx, rx) = tokio::sync::oneshot::channel();
    let save_app = app.clone();
    // Plugin state capture queries native windows while holding its cache
    // lock. Run on the UI thread so resize/move handlers cannot deadlock it.
    if app
        .run_on_main_thread(move || {
            let _ = tx.send(save_app.save_window_state(crate::WINDOW_STATE_FLAGS));
        })
        .is_ok()
    {
        if let Ok(Err(error)) = rx.await {
            eprintln!("Cannot save window layout before reset: {error}");
        }
    }
}

async fn reset(app: &AppHandle, state: &immich::DesktopState) -> Result<(), String> {
    if cache::has_active_transfers() {
        return Err("Wait for the active file transfer to finish before resetting".into());
    }
    let path = immich::config_path(app)?;
    let marker = path.with_file_name("reset-pending.json");
    let settings = if marker.exists() {
        serde_json::from_slice::<immich::Settings>(
            &std::fs::read(&marker).map_err(|_| "Cannot read pending reset")?,
        )
        .map_err(|_| "Invalid pending reset")?
    } else {
        let settings = immich::read_settings(app).unwrap_or_default();
        std::fs::create_dir_all(path.parent().unwrap())
            .map_err(|_| "Cannot create reset marker")?;
        let partial = marker.with_extension("partial");
        let mut file = std::fs::File::create(&partial).map_err(|_| "Cannot save reset marker")?;
        use std::io::Write;
        file.write_all(&serde_json::to_vec(&settings).unwrap())
            .map_err(|_| "Cannot save reset marker")?;
        file.sync_all().map_err(|_| "Cannot flush reset marker")?;
        drop(file);
        std::fs::rename(partial, &marker).map_err(|_| "Cannot publish reset marker")?;
        settings
    };
    // Stop scripts/service workers from immediately recreating login state.
    state
        .profile
        .lock()
        .map_err(|_| "Profile unavailable")?
        .take();
    if let Some(remote) = app.get_webview_window("immich") {
        // Destroy bypasses CloseRequested. Capture fullscreen and geometry
        // while the window still exists; the plugin also retries saving on exit.
        save_window_layout(app).await;
        remote
            .destroy()
            .map_err(|_| "Cannot close Immich before reset")?;
    }
    let store = app
        .get_webview_window("main")
        .ok_or("Setup webview unavailable")?;
    clear_login(&store).await?;
    if !settings.server.is_empty() {
        let server = crate::policy::server_url(&settings.server)?;
        match immich::credential(&server)?.delete_credential() {
            Ok(()) | Err(keyring::Error::NoEntry) => {}
            Err(_) => {
                return Err("Cannot remove API key from OS credential store; retry reset".into())
            }
        }
    }
    state.cache.clear()?;
    match std::fs::remove_file(&path) {
        Ok(()) => {}
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
        Err(_) => return Err("Cannot remove saved configuration".into()),
    }
    std::fs::remove_file(marker).map_err(|_| "Cannot complete reset")?;
    Ok(())
}

async fn clear_login(window: &WebviewWindow) -> Result<(), String> {
    let (tx, rx) = tokio::sync::oneshot::channel::<Result<(), String>>();
    window
        .with_webview(move |webview| {
            #[cfg(target_os = "macos")]
            unsafe {
                extern "C" {
                    fn immich_clear_login(
                        view: *mut std::ffi::c_void,
                        context: *mut std::ffi::c_void,
                        done: extern "C" fn(*mut std::ffi::c_void),
                    );
                }
                extern "C" fn done(context: *mut std::ffi::c_void) {
                    let tx = unsafe {
                        Box::from_raw(
                            context.cast::<tokio::sync::oneshot::Sender<Result<(), String>>>(),
                        )
                    };
                    let _ = tx.send(Ok(()));
                }
                immich_clear_login(webview.inner(), Box::into_raw(Box::new(tx)).cast(), done);
            }
            #[cfg(windows)]
            {
                use webview2_com::{
                    ClearBrowsingDataCompletedHandler,
                    Microsoft::Web::WebView2::Win32::{ICoreWebView2Profile2, ICoreWebView2_13},
                };
                use windows::core::Interface;
                // Match Tauri's WebView2 API, but wait for its real completion callback.
                let sender = std::sync::Arc::new(std::sync::Mutex::new(Some(tx)));
                let completion = sender.clone();
                let result = (|| unsafe {
                    webview
                        .controller()
                        .CoreWebView2()?
                        .cast::<ICoreWebView2_13>()?
                        .Profile()?
                        .cast::<ICoreWebView2Profile2>()?
                        .ClearBrowsingDataAll(&ClearBrowsingDataCompletedHandler::create(Box::new(
                            move |status| {
                                if let Some(tx) = completion.lock().unwrap().take() {
                                    let _ =
                                        tx.send(status.ok().map_err(|_| {
                                            "WebView2 could not clear login data".into()
                                        }));
                                }
                                Ok(())
                            },
                        )))
                })();
                if result.is_err() {
                    if let Some(tx) = sender.lock().unwrap().take() {
                        let _ = tx.send(Err(
                            "WebView2 reset unavailable; update the WebView2 runtime and retry"
                                .into(),
                        ));
                    }
                }
            }
            #[cfg(not(any(target_os = "macos", windows)))]
            {
                let _ = webview;
                let _ = tx.send(Err("Login reset is supported on Windows and macOS".into()));
            }
        })
        .map_err(|_| "Cannot schedule login reset")?;
    tokio::time::timeout(std::time::Duration::from_secs(60), rx)
        .await
        .map_err(|_| "Login reset timed out; retry from the reset menu")?
        .map_err(|_| "Login reset interrupted")?
}
