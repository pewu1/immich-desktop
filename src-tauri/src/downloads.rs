//! macOS archive forms target a new browsing context. Keep the original POST
//! inside WebKit, with the opener's configuration/session and no IPC grants.
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use tauri::{
    webview::NewWindowResponse, AppHandle, Manager, Url, WebviewUrl, WebviewWindowBuilder,
};

const PREFIX: &str = "archive-download-";

fn archive_url(server: &Url, url: &Url) -> bool {
    url.origin() == server.origin()
        && url.path() == "/api/download/archive"
        && url.username().is_empty()
        && url.password().is_none()
        && url.fragment().is_none()
}

pub fn attach<'a>(
    builder: WebviewWindowBuilder<'a, tauri::Wry, AppHandle>,
    app: AppHandle,
    server: Url,
) -> WebviewWindowBuilder<'a, tauri::Wry, AppHandle> {
    builder.on_new_window(move |url, features| {
        let trusted = app
            .get_webview_window("immich")
            .and_then(|window| window.url().ok())
            .is_some_and(|current| current.origin() == server.origin());
        if !trusted || !archive_url(&server, &url) {
            return NewWindowResponse::Deny;
        }
        let started = Arc::new(AtomicBool::new(false));
        let download_started = started.clone();
        let timeout_started = started.clone();
        let download_server = server.clone();
        let navigation_server = server.clone();
        let label = format!("{PREFIX}{}", uuid::Uuid::new_v4());
        // about:blank is intentional: loading `url` here would issue another
        // GET and lose the form body. WebKit supplies the pending POST after
        // this callback returns the new view with the supplied configuration.
        let child = WebviewWindowBuilder::new(
            &app,
            &label,
            WebviewUrl::External(Url::parse("about:blank").unwrap()),
        )
        .window_features(features)
        .visible(false)
        .focused(false)
        .disable_drag_drop_handler()
        .on_new_window(|_, _| NewWindowResponse::Deny)
        .on_navigation(move |target| {
            target.as_str() == "about:blank" || archive_url(&navigation_server, target)
        })
        .on_download(move |view, event| {
            match event {
                tauri::webview::DownloadEvent::Requested { url, .. } => {
                    if !archive_url(&download_server, &url) {
                        eprintln!("Archive download rejected an unexpected destination");
                        let _ = view.window().destroy();
                        return false;
                    }
                    download_started.store(true, Ordering::Release);
                }
                tauri::webview::DownloadEvent::Finished { success, .. } => {
                    if !success {
                        eprintln!("Immich archive download failed");
                    }
                    let _ = view.window().destroy();
                }
                _ => {}
            }
            true
        })
        .on_page_load(move |window, payload| {
            // An HTML error/login response isn't a download. Do not leave
            // its hidden browsing context running indefinitely.
            if payload.url().as_str() != "about:blank"
                && matches!(payload.event(), tauri::webview::PageLoadEvent::Finished)
                && !started.load(Ordering::Acquire)
            {
                eprintln!("Immich archive request returned a page instead of a download");
                let _ = window.destroy();
            }
        })
        .build();
        match child {
            Ok(window) => {
                let timeout_app = app.clone();
                tauri::async_runtime::spawn(async move {
                    tokio::time::sleep(std::time::Duration::from_secs(1800)).await;
                    // Bound abandoned requests, not the duration of a large
                    // archive once WebKit has begun writing it to Downloads.
                    if !timeout_started.load(Ordering::Acquire) {
                        if let Some(window) = timeout_app.get_webview_window(&label) {
                            eprintln!("Immich archive request timed out");
                            let _ = window.destroy();
                        }
                    }
                });
                NewWindowResponse::Create { window }
            }
            Err(_) => {
                eprintln!("Cannot create the Immich archive download window");
                NewWindowResponse::Deny
            }
        }
    })
}

pub fn close_all(app: &AppHandle) -> tauri::Result<()> {
    for (label, window) in app.webview_windows() {
        if label.starts_with(PREFIX) {
            window.destroy()?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn only_exact_archive_endpoint_on_configured_origin_is_allowed() {
        let server = Url::parse("https://photos.example/").unwrap();
        assert!(archive_url(
            &server,
            &server
                .join("api/download/archive?key=shared&edited=true")
                .unwrap()
        ));
        for target in [
            "https://other.example/api/download/archive",
            "https://photos.example:444/api/download/archive",
            "http://photos.example/api/download/archive",
            "https://photos.example/api/download/archive/",
            "https://photos.example/api/download/archive/extra",
            "https://photos.example/api/assets/id/original",
            "https://user@photos.example/api/download/archive",
            "https://photos.example/api/download/archive#fragment",
            "about:blank",
        ] {
            assert!(
                !archive_url(&server, &Url::parse(target).unwrap()),
                "{target}"
            );
        }
    }
}
