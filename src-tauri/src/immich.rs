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
    pub resetting: std::sync::atomic::AtomicBool,
    pub batches: Arc<crate::batch::Control>,
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
    pub server: String,
}

pub fn config_path(app: &AppHandle) -> Result<PathBuf, String> {
    Ok(app
        .path()
        .app_config_dir()
        .map_err(|_| "Configuration directory unavailable")?
        .join("settings.json"))
}

pub fn credential(server: &Url) -> Result<keyring::Entry, String> {
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
    read_settings(&app)
}

pub fn read_settings(app: &AppHandle) -> Result<Settings, String> {
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
    if state.resetting.load(std::sync::atomic::Ordering::Acquire)
        || crate::desktop::reset_pending(&app)
    {
        return Err("Finish resetting configuration before saving settings".into());
    }
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
    open_configured(app, &state).await
}

pub async fn open_configured(app: AppHandle, state: &DesktopState) -> Result<(), String> {
    if state.resetting.load(std::sync::atomic::Ordering::Acquire)
        || crate::desktop::reset_pending(&app)
    {
        return Err("Finish resetting configuration before opening Immich".into());
    }
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
            let settings = read_settings(&app)?;
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
            // Add once per process. Only media-transfer commands are granted to this exact origin.
            let capability = serde_json::json!({
                "identifier": "configured-immich", "local": false,
                "webviews": ["immich"],
                "remote": { "urls": [format!("{}/*", profile.server.origin().ascii_serialization())] },
                "permissions": ["allow-prepare-asset", "allow-drag-asset", "allow-drag-assets", "allow-cancel-drag"]
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
    let navigation_app = app.clone();
    // Build from this async command/startup worker. Creating a WebView2 window
    // synchronously inside a main-thread event handler can deadlock on Windows.
    let window =
        WebviewWindowBuilder::new(&app, "immich", WebviewUrl::External(profile.server.clone()))
            .title("Immich Desktop")
            .inner_size(1200.0, 850.0)
            .visible(false)
            .disable_drag_drop_handler()
            .initialization_script(script)
            // Registering a handler enables WKWebView downloads on macOS.
            // Keep the native destination: the system Downloads folder, with
            // filename collision handling supplied by the webview runtime.
            .on_download(|_, _| true)
            // HTTPS identity-provider redirects may load, but receive no native capability.
            .on_navigation(move |url| {
                if let Some(state) = navigation_app.try_state::<DesktopState>() {
                    state.batches.cancel_current();
                }
                url.origin() == allowed_origin || url.scheme() == "https"
            })
            .build()
            .map_err(|_| "Cannot open Immich webview")?;
    // The window-state plugin's ready handler is queued before this show, so
    // the initial default geometry never flashes before the saved state.
    window.show().map_err(|_| "Cannot show Immich webview")?;
    if let Some(setup) = app.get_webview_window("main") {
        setup.hide().map_err(|_| "Cannot hide setup")?;
    }
    Ok(())
}

fn remote(window: &WebviewWindow, state: &DesktopState) -> Result<Arc<Profile>, String> {
    if state.resetting.load(std::sync::atomic::Ordering::Acquire) {
        return Err("Configuration reset in progress".into());
    }
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
    gesture_id: String,
    preview_png: String,
    on_event: Events,
) -> Result<(), String> {
    let profile = remote(&window, &state)?;
    let id = policy::asset_id(&asset_id)?;
    let preview = Preview::parse(&preview_png)?;
    let ticket = state.batches.begin(&gesture_id)?;
    remote(&window, &state)?;
    let metadata = tokio::select! {
        biased;
        _ = ticket.cancelled() => return Err("Drag cancelled during metadata preparation".into()),
        metadata = describe(&profile, &id) => metadata?,
    };
    let (tx, rx) = tokio::sync::oneshot::channel();
    let main_thread = window.clone();
    window
        .run_on_main_thread(move || {
            // Check again when the OS drag actually starts, after scheduling.
            let permitted = ticket.check().is_ok()
                && !main_thread
                    .app_handle()
                    .state::<DesktopState>()
                    .resetting
                    .load(std::sync::atomic::Ordering::Acquire)
                && main_thread
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

#[tauri::command]
pub fn cancel_drag(
    window: WebviewWindow,
    state: State<'_, DesktopState>,
    gesture_id: String,
) -> Result<(), String> {
    remote(&window, &state)?;
    state.batches.cancel(&gesture_id)
}

async fn collect_group<T: Send + 'static>(
    mut tasks: tokio::task::JoinSet<Result<(usize, T), String>>,
    count: usize,
    ticket: &crate::batch::Ticket,
) -> Result<Vec<T>, String> {
    let mut ordered: Vec<Option<T>> = (0..count).map(|_| None).collect();
    loop {
        let result = tokio::select! {
            biased;
            _ = ticket.cancelled() => Err("Group drag cancelled".to_string()),
            result = tasks.join_next() => match result {
                None => break,
                Some(Ok(result)) => result,
                Some(Err(_)) => Err("Group preparation interrupted".into()),
            },
        };
        match result {
            Ok((index, value)) => ordered[index] = Some(value),
            Err(error) => {
                tasks.shutdown().await;
                return Err(error);
            }
        }
    }
    ordered
        .into_iter()
        .map(|value| value.ok_or_else(|| "Incomplete outgoing group".into()))
        .collect()
}

async fn prepare_group(
    profile: Arc<Profile>,
    ids: Vec<String>,
    ticket: &crate::batch::Ticket,
) -> Result<Vec<PathBuf>, String> {
    use crate::batch;
    let root = profile.cache.outgoing_root();
    ticket.check()?;
    let key = batch::key(&profile.namespace, &ids);
    if let Some(paths) = batch::existing(&root, &key, &ids) {
        return Ok(paths);
    }
    let staging = batch::Staging::new(&root)?;
    let mut originals = Vec::new();
    // Both metadata and original-file requests have bounded concurrency.
    for chunk in ids.chunks(4) {
        let mut tasks = tokio::task::JoinSet::new();
        for (index, id) in chunk.iter().enumerate() {
            let profile = profile.clone();
            let id = id.clone();
            tasks.spawn(async move {
                describe(&profile, &id)
                    .await
                    .map(|metadata| (index, metadata.original_file_name))
            });
        }
        originals.extend(collect_group(tasks, chunk.len(), ticket).await?);
    }
    let names = batch::names(&originals)?;
    let mut files = Vec::new();
    for (id_chunk, name_chunk) in ids.chunks(4).zip(names.chunks(4)) {
        let mut tasks = tokio::task::JoinSet::new();
        for (index, (id, name)) in id_chunk.iter().zip(name_chunk).enumerate() {
            let profile = profile.clone();
            let id = id.clone();
            let name = name.clone();
            let destination = staging.dir.join("files").join(&name);
            tasks.spawn(async move {
                let cached = profile
                    .cache
                    .cached_original(&format!("{}:{}", profile.namespace, id));
                let size = if let Some(ready) = cached {
                    cache::stage_original(&ready.path, &destination).await?;
                    ready.size
                } else {
                    cache::download_file(
                        &destination,
                        &profile.server,
                        &id,
                        &profile.key,
                        &profile.client,
                    )
                    .await?
                };
                Ok::<_, String>((index, batch::File { id, name, size }))
            });
        }
        files.extend(collect_group(tasks, id_chunk.len(), ticket).await?);
    }
    ticket.check()?;
    staging.commit(&root, &key, &files)
}

#[tauri::command]
pub async fn drag_assets(
    window: WebviewWindow,
    state: State<'_, DesktopState>,
    asset_ids: Vec<String>,
    gesture_id: String,
    preview_png: String,
    on_event: Events,
) -> Result<(), String> {
    let profile = remote(&window, &state)?;
    let ids = crate::batch::ids(asset_ids)?;
    let preview = Preview::parse(&preview_png)?;
    let mut ticket = state.batches.begin(&gesture_id)?;
    // Close the race between registering the activity and reset's initial check.
    remote(&window, &state)?;
    cache::emit(
        &on_event,
        "preparing",
        "Preparing selected originals; keep the mouse held",
    );
    let paths = prepare_group(profile.clone(), ids, &ticket).await?;
    ticket.check()?;
    let (tx, rx) = tokio::sync::oneshot::channel();
    let main_thread = window.clone();
    window
        .run_on_main_thread(move || {
            let result = (|| {
                ticket.check()?;
                remote(
                    &main_thread,
                    main_thread.app_handle().state::<DesktopState>().inner(),
                )?;
                if !native_drag::mouse_down() {
                    return Err("Mouse released before group drag started".into());
                }
                native_drag::start_files(
                    main_thread,
                    paths,
                    preview,
                    on_event,
                    ticket.activity.take().ok_or("Drag lease unavailable")?,
                )
            })();
            let _ = tx.send(result);
        })
        .map_err(|_| "Cannot schedule group drag")?;
    rx.await.map_err(|_| "Group drag interrupted")?
}

#[cfg(test)]
mod group_tests {
    use super::*;
    use crate::batch;
    use std::{
        io::{Read, Write},
        net::TcpListener,
        sync::atomic::{AtomicBool, AtomicUsize, Ordering},
        thread,
    };

    struct Server {
        url: Url,
        stop: Arc<AtomicBool>,
        requests: Arc<AtomicUsize>,
        peak: Arc<AtomicUsize>,
        thread: Option<thread::JoinHandle<()>>,
    }
    impl Server {
        fn new(fail_original: bool) -> Self {
            let listener = TcpListener::bind("127.0.0.1:0").unwrap();
            let url = Url::parse(&format!("http://{}/", listener.local_addr().unwrap())).unwrap();
            listener.set_nonblocking(true).unwrap();
            let stop = Arc::new(AtomicBool::new(false));
            let requests = Arc::new(AtomicUsize::new(0));
            let peak = Arc::new(AtomicUsize::new(0));
            let (worker_stop, worker_requests, worker_peak) =
                (stop.clone(), requests.clone(), peak.clone());
            let thread = thread::spawn(move || {
                let active = Arc::new(AtomicUsize::new(0));
                let mut workers = Vec::new();
                while !worker_stop.load(Ordering::Acquire) {
                    let (mut stream, _) = match listener.accept() {
                        Ok(connection) => connection,
                        Err(error) if error.kind() == std::io::ErrorKind::WouldBlock => {
                            thread::sleep(Duration::from_millis(2));
                            continue;
                        }
                        Err(error) => panic!("Test listener failed: {error}"),
                    };
                    let (active, requests, peak) =
                        (active.clone(), worker_requests.clone(), worker_peak.clone());
                    workers.push(thread::spawn(move || {
                        stream.set_nonblocking(false).unwrap();
                        stream.set_read_timeout(Some(Duration::from_secs(3))).unwrap();
                        let mut request = Vec::new(); let mut byte = [0];
                        while !request.ends_with(b"\r\n\r\n") {
                            if stream.read_exact(&mut byte).is_err() { return; }
                            request.push(byte[0]); assert!(request.len() < 8192);
                        }
                        let request = String::from_utf8(request).unwrap();
                        assert!(request.to_lowercase().contains("x-api-key: test-key"));
                        let path = request.split_whitespace().nth(1).unwrap();
                        let original = path.ends_with("/original");
                        requests.fetch_add(1, Ordering::AcqRel);
                        peak.fetch_max(active.fetch_add(1, Ordering::AcqRel) + 1, Ordering::AcqRel);
                        thread::sleep(Duration::from_millis(20));
                        let (status, body) = if original && fail_original { (403, Vec::new()) }
                            else if original { (200, path.as_bytes().to_vec()) }
                            else { (200, br#"{"originalFileName":"same.jpg"}"#.to_vec()) };
                        let header = format!("HTTP/1.1 {status} Test\r\nContent-Length: {}\r\nConnection: close\r\n\r\n", body.len());
                        let _ = stream.write_all(header.as_bytes()).and_then(|_| stream.write_all(&body));
                        active.fetch_sub(1, Ordering::AcqRel);
                    }));
                }
                for worker in workers {
                    worker.join().unwrap();
                }
            });
            Self {
                url,
                stop,
                requests,
                peak,
                thread: Some(thread),
            }
        }
    }
    impl Drop for Server {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Release);
            self.thread.take().unwrap().join().unwrap();
        }
    }
    fn profile(root: &std::path::Path, url: Url) -> Arc<Profile> {
        Arc::new(Profile {
            server: url,
            key: "test-key".into(),
            namespace: "account".into(),
            client: Client::builder()
                .no_proxy()
                .redirect(reqwest::redirect::Policy::none())
                .build()
                .unwrap(),
            cache: Cache::new(root.into()).unwrap(),
            metadata: tokio::sync::Mutex::new(HashMap::new()),
        })
    }
    #[tokio::test]
    async fn group_downloads_are_bounded_exact_and_reused_after_restart_until_reset() {
        let temp = tempfile::tempdir().unwrap();
        let server = Server::new(false);
        let profile = profile(temp.path(), server.url.clone());
        let ids: Vec<String> = (0..6).map(|_| uuid::Uuid::new_v4().to_string()).collect();
        let control = Arc::new(batch::Control::default());
        let ticket = control.begin(&uuid::Uuid::new_v4().to_string()).unwrap();
        let paths = prepare_group(profile.clone(), ids.clone(), &ticket)
            .await
            .unwrap();
        assert_eq!(paths.len(), 6);
        for (path, id) in paths.iter().zip(&ids) {
            assert_eq!(
                std::fs::read(path).unwrap(),
                format!("/api/assets/{id}/original").as_bytes()
            );
        }
        assert_eq!(paths[0].file_name().unwrap(), "same.jpg");
        assert_eq!(paths[1].file_name().unwrap(), "same (2).jpg");
        assert!(server.peak.load(Ordering::Acquire) <= 4);
        assert!(server.peak.load(Ordering::Acquire) > 1);
        let requests = server.requests.load(Ordering::Acquire);
        assert_eq!(requests, 12);
        let reopened = Cache::new(temp.path().into()).unwrap();
        assert!(paths.iter().all(|path| path.exists()));
        assert_eq!(
            prepare_group(profile.clone(), ids, &ticket).await.unwrap(),
            paths
        );
        assert_eq!(server.requests.load(Ordering::Acquire), requests);
        reopened.clear().unwrap();
        assert!(paths.iter().all(|path| !path.exists()));
    }
    #[tokio::test]
    async fn failed_group_never_publishes_partial_files() {
        let temp = tempfile::tempdir().unwrap();
        let server = Server::new(true);
        let profile = profile(temp.path(), server.url.clone());
        let control = Arc::new(batch::Control::default());
        let ticket = control.begin(&uuid::Uuid::new_v4().to_string()).unwrap();
        let ids = vec![
            uuid::Uuid::new_v4().to_string(),
            uuid::Uuid::new_v4().to_string(),
        ];
        assert!(prepare_group(profile.clone(), ids, &ticket).await.is_err());
        assert_eq!(
            std::fs::read_dir(profile.cache.outgoing_root())
                .unwrap()
                .count(),
            0
        );
    }
    #[tokio::test]
    async fn cancellation_stops_preparation_and_removes_staging() {
        let temp = tempfile::tempdir().unwrap();
        let server = Server::new(false);
        let profile = profile(temp.path(), server.url.clone());
        let control = Arc::new(batch::Control::default());
        let id = uuid::Uuid::new_v4().to_string();
        let ticket = control.begin(&id).unwrap();
        let ids = (0..8).map(|_| uuid::Uuid::new_v4().to_string()).collect();
        let cancel = async {
            tokio::time::sleep(Duration::from_millis(5)).await;
            control.cancel(&id).unwrap();
        };
        let (result, ()) = tokio::join!(prepare_group(profile.clone(), ids, &ticket), cancel);
        assert!(result.is_err());
        assert_eq!(
            std::fs::read_dir(profile.cache.outgoing_root())
                .unwrap()
                .count(),
            0
        );
        assert!(server.requests.load(Ordering::Acquire) <= 4);
    }
}
