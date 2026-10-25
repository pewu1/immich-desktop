use crate::{
    cache::{self, Cache, Events, Metadata},
    native_drag::{self, Preview},
    policy,
};
use reqwest::Client;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::collections::HashMap;
use std::{
    path::PathBuf,
    sync::{Arc, Mutex},
    time::Duration,
};
use tauri::{AppHandle, Manager, State, Url, WebviewUrl, WebviewWindow, WebviewWindowBuilder};

pub struct DesktopState {
    pub profile: Mutex<Option<Arc<Profile>>>,
    // Lock held for process lifetime. Two instances must never clean/use the same cache.
    pub _cache_lock: std::fs::File,
    pub cache: Arc<Cache>,
}

pub struct Profile {
    server: Url,
    key: String,
    client: Client,
    cache: Arc<Cache>,
    namespace: String,
    metadata: tokio::sync::Mutex<HashMap<String, Metadata>>,
}

#[derive(Serialize, Deserialize, Default)]
pub struct Settings {
    server: String,
}

fn config_path(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(app
        .path()
        .app_config_dir()
        .map_err(|_| "Configuration directory unavailable")?
        .join("settings.json"))
}

fn credential(server: &Url) -> Result<keyring::Entry, String> {
    let account = format!("{:x}", Sha256::digest(server.as_str().as_bytes()));
    keyring::Entry::new("app.immichdesktop.wrapper", &account)
        .map_err(|_| "OS credential store unavailable".into())
}

fn local(window: &WebviewWindow) -> Result<(), String> {
    if window.label() != "main" {
        return Err("Setup is available only in the local window".into());
    }
    Ok(())
}

#[tauri::command]
pub fn get_settings(window: WebviewWindow, app: AppHandle) -> Result<Settings, String> {
    local(&window)?;
    match std::fs::read(config_path(&app)?) {
        Ok(bytes) => serde_json::from_slice(&bytes).map_err(|_| "Invalid settings file".into()),
        Err(e) if e.kind() == std::io::ErrorKind::NotFound => Ok(Settings::default()),
        Err(_) => Err("Cannot read settings".into()),
    }
}

#[tauri::command]
pub fn save_settings(
    window: WebviewWindow,
    app: AppHandle,
    state: State<'_, DesktopState>,
    server: String,
    api_key: String,
) -> Result<(), String> {
    local(&window)?;
    let active = state.profile.lock().map_err(|_| "Profile unavailable")?;
    if active.is_some() {
        return Err("Restart the app before changing the server or API key".into());
    }
    let url = policy::server_url(&server)?;
    let entry = credential(&url)?;
    if !api_key.is_empty() {
        if api_key.len() > 1024
            || !api_key.is_ascii()
            || api_key.bytes().any(|b| b.is_ascii_control())
        {
            return Err("Invalid API key format".into());
        }
        entry
            .set_password(&api_key)
            .map_err(|_| "Cannot save API key in the OS credential store")?;
    } else {
        entry
            .get_password()
            .map_err(|_| "Enter an API key; none is stored for this origin")?;
    }
    let path = config_path(&app)?;
    std::fs::create_dir_all(path.parent().unwrap())
        .map_err(|_| "Cannot create configuration directory")?;
    std::fs::write(
        path,
        serde_json::to_vec(&Settings { server: url.into() }).unwrap(),
    )
    .map_err(|_| "Cannot save settings")?;
    Ok(())
}

#[tauri::command]
pub async fn open_immich(
    window: WebviewWindow,
    app: AppHandle,
    state: State<'_, DesktopState>,
) -> Result<(), String> {
    local(&window)?;
    if let Some(existing) = app.get_webview_window("immich") {
        return existing
            .set_focus()
            .map_err(|_| "Cannot focus Immich".into());
    }
    let profile = {
        let mut active = state.profile.lock().map_err(|_| "Profile unavailable")?;
        if let Some(profile) = active.as_ref() {
            profile.clone()
        } else {
            let settings = get_settings(window, app.clone())?;
            let server = policy::server_url(&settings.server)?;
            let key = credential(&server)?
                .get_password()
                .map_err(|_| "API key unavailable; enter it in setup")?;
            let namespace = format!(
                "{:x}",
                Sha256::digest(format!("{}\0{}", server, key).as_bytes())
            );
            let cache = state.cache.clone();
            let client = Client::builder()
                .redirect(reqwest::redirect::Policy::none())
                .connect_timeout(Duration::from_secs(15))
                .timeout(Duration::from_secs(1800))
                .build()
                .map_err(|_| "Cannot initialize secure HTTP client")?;
            let profile = Arc::new(Profile {
                server,
                key,
                client,
                cache,
                namespace,
                metadata: tokio::sync::Mutex::new(HashMap::new()),
            });
            // Add once per process. Only these two app commands are granted to this exact origin.
            let capability = serde_json::json!({
                "identifier": "configured-immich", "local": false,
                "webviews": ["immich"],
                "remote": { "urls": [format!("{}/*", profile.server.origin().ascii_serialization())] },
                "permissions": ["allow-prepare-asset", "allow-drag-asset"]
            });
            app.add_capability(capability.to_string())
                .map_err(|_| "Cannot authorize the configured Immich origin")?;
            *active = Some(profile.clone());
            profile
        }
    };
    let origin = profile.server.origin().ascii_serialization();
    let script = format!(
        "(() => {{ const TRUSTED_ORIGIN = {}; {} }})();",
        serde_json::to_string(&origin).unwrap(),
        include_str!("../../ui/immich.js")
    );
    let allowed_origin = profile.server.origin();
    let (tx, rx) = tokio::sync::oneshot::channel();
    let creator = app.clone();
    app.run_on_main_thread(move || {
        let result = WebviewWindowBuilder::new(
            &creator,
            "immich",
            WebviewUrl::External(profile.server.clone()),
        )
        .title("Immich Desktop")
        .inner_size(1200.0, 850.0)
        .disable_drag_drop_handler()
        .initialization_script(script)
        // HTTPS identity-provider redirects may load, but receive no native capability.
        // External HTTP, file:, data: and custom-scheme top-level navigation are refused.
        .on_navigation(move |url| url.origin() == allowed_origin || url.scheme() == "https")
        .build()
        .map(|_| ())
        .map_err(|_| "Cannot open Immich webview".to_string());
        let _ = tx.send(result);
    })
    .map_err(|_| "Cannot schedule Immich window")?;
    rx.await.map_err(|_| "Immich window interrupted")?
}

fn remote(window: &WebviewWindow, state: &DesktopState) -> Result<Arc<Profile>, String> {
    let profile = state
        .profile
        .lock()
        .map_err(|_| "Profile unavailable")?
        .clone()
        .ok_or("Configure Immich first")?;
    let url = window.url().map_err(|_| "Window URL unavailable")?;
    if window.label() != "immich" || url.origin() != profile.server.origin() {
        return Err("Command denied outside the configured Immich origin".into());
    }
    Ok(profile)
}

#[tauri::command]
pub async fn prepare_asset(
    window: WebviewWindow,
    state: State<'_, DesktopState>,
    asset_id: String,
) -> Result<Metadata, String> {
    let profile = remote(&window, &state)?;
    describe(&profile, &asset_id).await
}

async fn describe(profile: &Profile, asset_id: &str) -> Result<Metadata, String> {
    let id = policy::asset_id(asset_id)?;
    if let Some(metadata) = profile
        .cache
        .cached_metadata(&format!("{}:{}", profile.namespace, id))
    {
        return Ok(metadata);
    }
    if let Some(metadata) = profile.metadata.lock().await.get(&id).cloned() {
        return Ok(metadata);
    }
    let metadata = cache::metadata(&profile.server, &id, &profile.key, &profile.client).await?;
    let mut entries = profile.metadata.lock().await;
    if entries.len() >= 64 {
        entries.clear();
    }
    entries.insert(id, metadata.clone());
    Ok(metadata)
}

#[tauri::command]
pub async fn drag_asset(
    window: WebviewWindow,
    state: State<'_, DesktopState>,
    asset_id: String,
    preview_png: String,
    on_event: Events,
) -> Result<(), String> {
    let profile = remote(&window, &state)?;
    let id = policy::asset_id(&asset_id)?;
    let preview = Preview::parse(&preview_png)?;
    let metadata = describe(&profile, &id).await?;
    let (tx, rx) = tokio::sync::oneshot::channel();
    let main_thread = window.clone();
    window
        .run_on_main_thread(move || {
            // Check again when the OS drag actually starts, after scheduling.
            let permitted = main_thread
                .url()
                .is_ok_and(|url| url.origin() == profile.server.origin());
            let result = if permitted && native_drag::mouse_down() {
                profile
                    .cache
                    .start(
                        format!("{}:{}", profile.namespace, id),
                        metadata,
                        profile.server.clone(),
                        profile.key.clone(),
                        profile.client.clone(),
                        on_event,
                    )
                    .and_then(|transfer| native_drag::start(main_thread, transfer, preview))
            } else {
                Err("Drag released or origin changed before the native session started".into())
            };
            let _ = tx.send(result);
        })
        .map_err(|_| "Cannot schedule native drag")?;
    rx.await.map_err(|_| "Native drag interrupted")?
}
