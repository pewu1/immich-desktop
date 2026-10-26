//! Physical WebKit/Tauri integration check. Uses an isolated browser session,
//! a loopback server, and uniquely named files in the real Downloads folder.
//! Run: cargo run --manifest-path src-tauri/Cargo.toml --example archive_download_probe --offline
#[cfg(target_os = "macos")]
#[path = "../src/downloads.rs"]
mod downloads;

#[cfg(target_os = "macos")]
fn main() {
    use base64::Engine;
    use std::{
        io::{Read, Write},
        net::TcpListener,
        sync::{
            atomic::{AtomicUsize, Ordering},
            Arc,
        },
        time::{Duration, Instant},
    };
    use tauri::{Manager, WebviewUrl, WebviewWindowBuilder};

    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let server: tauri::Url = format!("http://{}/", listener.local_addr().unwrap())
        .parse()
        .unwrap();
    let name = format!("immich-archive-probe-{}", uuid::Uuid::new_v4());
    let filename = format!("{name}.zip");
    let archive = base64::engine::general_purpose::STANDARD.decode("UEsDBBQAAAAAAHV0Wl0NWB2pHAAAABwAAAAJAAAAaGVsbG8udHh0YXV0aGVudGljYXRlZCBhcmNoaXZlIGJ5dGVzClBLAQIUAxQAAAAAAHV0Wl0NWB2pHAAAABwAAAAJAAAAAAAAAAAAAACAAQAAAABoZWxsby50eHRQSwUGAAAAAAEAAQA3AAAAQwAAAAAA").unwrap();
    let posts = Arc::new(AtomicUsize::new(0));
    let failures = Arc::new(AtomicUsize::new(0));
    let server_failures = failures.clone();
    let server_posts = posts.clone();
    let server_archive = archive.clone();
    let server_filename = filename.clone();
    std::thread::spawn(move || {
        for stream in listener.incoming() {
            let mut stream = stream.unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut request = Vec::new();
            let header_end = loop {
                let mut buffer = [0u8; 4096];
                let size = stream.read(&mut buffer).unwrap();
                assert!(size > 0, "unexpected EOF in request headers");
                request.extend_from_slice(&buffer[..size]);
                if let Some(end) = request.windows(4).position(|w| w == b"\r\n\r\n") {
                    break end + 4;
                }
            };
            let headers = String::from_utf8(request[..header_end].to_vec()).unwrap();
            let lower = headers.to_ascii_lowercase();
            let length: usize = lower
                .lines()
                .find_map(|line| {
                    line.strip_prefix("content-length:")
                        .map(|n| n.trim().parse().unwrap())
                })
                .unwrap_or(0);
            while request.len() < header_end + length {
                let mut buffer = [0u8; 4096];
                let size = stream.read(&mut buffer).unwrap();
                assert!(size > 0, "unexpected EOF in POST body");
                request.extend_from_slice(&buffer[..size]);
            }
            let (content_type, extra, body) = if headers
                .starts_with("POST /api/download/archive?fixture=1 ")
            {
                assert!(
                    lower.contains("cookie: archive_probe=session-ok"),
                    "opener cookie was lost"
                );
                let body = std::str::from_utf8(&request[header_end..]).unwrap();
                let fields: std::collections::HashMap<_, _> =
                    tauri::Url::parse(&format!("http://fixture/?{body}"))
                        .unwrap()
                        .query_pairs()
                        .into_owned()
                        .collect();
                assert_eq!(
                    fields.get("assetIds").unwrap(),
                    "11111111-1111-4111-8111-111111111111,22222222-2222-4222-8222-222222222222"
                );
                assert_eq!(fields.get("archiveName").unwrap(), "Album 東京");
                assert_eq!(fields.get("edited").unwrap(), "true");
                server_posts.fetch_add(1, Ordering::SeqCst);
                (
                    "application/zip",
                    format!("Content-Disposition: attachment; filename=\"{server_filename}\"\r\n"),
                    server_archive.clone(),
                )
            } else if headers.starts_with("POST /api/download/archive?failure=1 ") {
                server_failures.fetch_add(1, Ordering::SeqCst);
                (
                    "text/html",
                    String::new(),
                    b"<!doctype html><p>Fixture authentication failure</p>".to_vec(),
                )
            } else {
                assert!(
                    headers.starts_with("GET / HTTP/"),
                    "unexpected request: {}",
                    headers.lines().next().unwrap()
                );
                ("text/html", "Set-Cookie: archive_probe=session-ok; Path=/; HttpOnly; SameSite=Lax\r\n".into(), r#"<!doctype html><title>Archive fixture</title><script>
                    function archive(failure = false) {
                        const form = document.createElement('form');
                        form.method = 'post'; form.target = '_blank';
                        form.action = '/api/download/archive?' + (failure ? 'failure=1' : 'fixture=1');
                        for (const [name, value] of Object.entries({assetIds: '11111111-1111-4111-8111-111111111111,22222222-2222-4222-8222-222222222222', archiveName: 'Album 東京', edited: 'true'})) {
                            const input = document.createElement('input'); input.type = 'hidden'; input.name = name; input.value = value; form.append(input);
                        }
                        document.body.append(form); form.submit(); form.remove();
                    }
                    </script>"#.as_bytes().to_vec())
            };
            let response = format!("HTTP/1.1 200 OK\r\nContent-Type: {content_type}; charset=utf-8\r\n{extra}Content-Length: {}\r\nConnection: close\r\n\r\n", body.len());
            stream.write_all(response.as_bytes()).unwrap();
            stream.write_all(&body).unwrap();
        }
    });
    let mut context = tauri::generate_context!();
    context.config_mut().identifier = "app.immichdesktop.archive-probe".into();
    context.config_mut().app.windows.clear();
    tauri::Builder::default().setup(move |app| {
        let downloads_dir = app.path().download_dir()?;
        let first = downloads_dir.join(&filename);
        let second = downloads_dir.join(format!("{name} (1).zip"));
        assert!(!first.exists() && !second.exists());
        let builder = WebviewWindowBuilder::new(app.handle(), "immich", WebviewUrl::External(server.clone()))
            .visible(false).incognito(true)
            .on_page_load(|window, payload| {
                if matches!(payload.event(), tauri::webview::PageLoadEvent::Finished) {
                    window.eval("archive()").unwrap();
                }
            });
        downloads::attach(builder, app.handle().clone(), server.clone()).build()?;
        let handle = app.handle().clone();
        std::thread::spawn(move || {
            let deadline = Instant::now() + Duration::from_secs(45);
            let mut step = 0;
            loop {
                std::thread::sleep(Duration::from_millis(100));
                let children = handle.webview_windows().keys().filter(|label| label.starts_with("archive-download-")).count();
                if step == 0 && std::fs::read(&first).ok().as_deref() == Some(&archive) && children == 0 {
                    assert_eq!(posts.load(Ordering::SeqCst), 1);
                    handle.get_webview_window("immich").unwrap().eval("archive()").unwrap();
                    step = 1;
                } else if step == 1 && std::fs::read(&second).ok().as_deref() == Some(&archive) && children == 0 {
                    assert_eq!(posts.load(Ordering::SeqCst), 2);
                    handle.get_webview_window("immich").unwrap().eval("archive(true); window.open('/unrelated')").unwrap();
                    step = 2;
                } else if step == 2 && failures.load(Ordering::SeqCst) == 1 && children == 0 {
                    assert_eq!(handle.get_webview_window("immich").unwrap().url().unwrap(), server);
                    assert_eq!(posts.load(Ordering::SeqCst), 2);
                    assert_eq!(handle.webview_windows().len(), 1);
                    downloads::close_all(&handle).unwrap();
                    std::fs::remove_file(&first).unwrap();
                    std::fs::remove_file(&second).unwrap();
                    println!("PASS: authenticated archive POST, Unicode form fields, exact ZIP bytes, collision suffix, failed response cleanup and denied unrelated popup");
                    handle.exit(0);
                    return;
                }
                if Instant::now() > deadline {
                    eprintln!("FAIL: archive probe timed out at step {step} ({children} hidden windows, {} successful POSTs)", posts.load(Ordering::SeqCst));
                    let _ = std::fs::remove_file(&first);
                    let _ = std::fs::remove_file(&second);
                    handle.exit(1);
                    return;
                }
            }
        });
        Ok(())
    }).run(context).expect("archive probe failed");
}

#[cfg(not(target_os = "macos"))]
fn main() {
    panic!("The archive download probe requires macOS");
}
