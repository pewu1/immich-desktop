use super::*;
use std::{net::TcpListener, thread, time::Duration};
const A: &str = "11111111-1111-4111-8111-111111111111";
const B: &str = "22222222-2222-4222-8222-222222222222";
fn events() -> Events {
    Channel::new(|_| Ok(()))
}
fn select(cache: &Cache, key: &str) -> Arc<Transfer> {
    cache
        .select(key.into(), "photo.png".into(), events(), false)
        .unwrap()
        .0
}
fn complete(cache: &Cache, transfer: &Transfer, bytes: &[u8]) -> PathBuf {
    std::fs::write(transfer.job.dir.join("original.partial"), bytes).unwrap();
    let ready = cache.publish(&transfer.job, bytes.len() as u64).unwrap();
    let path = ready.path.clone();
    transfer.job.finish(Ok(ready));
    path
}
#[test]
fn reset_evicts_retry_file_without_deleting_an_active_receivers_original() {
    let temp = tempfile::tempdir().unwrap();
    let cache = Cache::new(temp.path().into()).unwrap();
    let transfer = select(&cache, A);
    let path = complete(&cache, &transfer, b"original");
    cache.clear().unwrap();
    assert!(cache.cached_metadata(A).is_none());
    assert!(!temp.path().join("current.json").exists());
    assert!(path.is_file());
    drop(transfer);
    assert!(!path.exists());
    assert!(Cache::new(temp.path().into())
        .unwrap()
        .cached_metadata(A)
        .is_none());
}
#[test]
fn only_last_asset_is_reusable_and_readers_delay_deletion() {
    let temp = tempfile::tempdir().unwrap();
    let cache = Cache::new(temp.path().into()).unwrap();
    let a = select(&cache, A);
    let path = complete(&cache, &a, b"original A");
    let again = select(&cache, A);
    assert!(Arc::ptr_eq(&a.job, &again.job));
    let b = select(&cache, B);
    assert!(cache.cached_metadata(A).is_none());
    assert!(path.is_file());
    let mut bytes = [0; 10];
    assert_eq!(a.read_at(0, &mut bytes).unwrap(), 10);
    assert_eq!(&bytes, b"original A");
    drop(a);
    assert!(path.is_file());
    drop(again);
    assert!(!path.exists());
    let bpath = complete(&cache, &b, b"original B");
    drop(b);
    assert!(bpath.is_file());
    assert!(cache.cached_metadata(B).is_some());
}
#[test]
fn a_b_a_creates_distinct_generations_and_old_completion_cannot_replace_latest() {
    let temp = tempfile::tempdir().unwrap();
    let cache = Cache::new(temp.path().into()).unwrap();
    let a = select(&cache, A);
    let b = select(&cache, B);
    let latest = select(&cache, A);
    assert_ne!(a.job.dir, latest.job.dir);
    let latest_path = complete(&cache, &latest, b"latest");
    complete(&cache, &a, b"older");
    complete(&cache, &b, b"other");
    let manifest: Manifest =
        serde_json::from_slice(&std::fs::read(temp.path().join("current.json")).unwrap()).unwrap();
    assert_eq!(
        temp.path().join(manifest.generation).join("file/photo.png"),
        latest_path
    );
    drop(a);
    drop(b);
    assert!(latest_path.is_file());
}
#[test]
fn restart_keeps_one_original_and_removes_orphan_and_legacy_directories() {
    let temp = tempfile::tempdir().unwrap();
    let cache = Cache::new(temp.path().into()).unwrap();
    let a = select(&cache, A);
    let path = complete(&cache, &a, b"original");
    drop(a);
    drop(cache);
    let orphan = temp.path().join(B);
    std::fs::create_dir(&orphan).unwrap();
    let legacy = temp.path().join("a".repeat(64));
    std::fs::create_dir(&legacy).unwrap();
    let reopened = Cache::new(temp.path().into()).unwrap();
    assert!(path.is_file());
    assert!(!orphan.exists());
    assert!(!legacy.exists());
    assert!(reopened.cached_metadata(A).is_some());
    assert!(reopened
        .cached_metadata(&format!("different-account:{A}"))
        .is_none());
}
#[test]
fn reader_waits_for_completion_and_failed_job_can_be_retried() {
    let temp = tempfile::tempdir().unwrap();
    let cache = Cache::new(temp.path().into()).unwrap();
    let a = select(&cache, A);
    assert_eq!(a.readiness(), 0);
    let reader = a.clone();
    let (tx, rx) = std::sync::mpsc::channel();
    let thread = thread::spawn(move || {
        let mut bytes = [0; 8];
        tx.send(reader.read_at(0, &mut bytes).map(|_| bytes))
            .unwrap();
    });
    assert!(rx.recv_timeout(Duration::from_millis(30)).is_err());
    complete(&cache, &a, b"original");
    assert_eq!(a.readiness(), 1);
    assert_eq!(
        &rx.recv_timeout(Duration::from_secs(2)).unwrap().unwrap(),
        b"original"
    );
    thread.join().unwrap();
    let b = select(&cache, B);
    b.job.finish(Err("network failure".into()));
    assert_eq!(b.readiness(), -1);
    assert!(b.wait().is_err());
    let retry = select(&cache, B);
    assert_ne!(retry.job.dir, b.job.dir);
}
#[test]
fn copies_original_without_overwriting_receiver_files_or_using_partial_data() {
    let temp = tempfile::tempdir().unwrap();
    let cache = Cache::new(temp.path().join("cache")).unwrap();
    let a = select(&cache, A);
    complete(&cache, &a, b"original");
    let dest = temp.path().join("upload.png");
    a.copy_to(&dest).unwrap();
    assert_eq!(std::fs::read(&dest).unwrap(), b"original");
    assert!(a.copy_to(&dest).is_err());
    assert_eq!(std::fs::read(dest).unwrap(), b"original");
}
fn mock(responses: Vec<Vec<u8>>) -> (Url, thread::JoinHandle<()>) {
    let listener = TcpListener::bind("127.0.0.1:0").unwrap();
    let url = Url::parse(&format!("http://{}/", listener.local_addr().unwrap())).unwrap();
    listener.set_nonblocking(true).unwrap();
    let handle = thread::spawn(move || {
        for response in responses {
            let deadline = std::time::Instant::now() + Duration::from_secs(10);
            let mut stream = loop {
                match listener.accept() {
                    Ok((stream, _)) => break stream,
                    Err(e)
                        if e.kind() == std::io::ErrorKind::WouldBlock
                            && std::time::Instant::now() < deadline =>
                    {
                        thread::sleep(Duration::from_millis(5))
                    }
                    Err(e) => panic!("Missing mock request: {e}"),
                }
            };
            stream.set_nonblocking(false).unwrap();
            stream
                .set_read_timeout(Some(Duration::from_secs(5)))
                .unwrap();
            let mut request = Vec::new();
            let mut byte = [0];
            while !request.ends_with(b"\r\n\r\n") {
                stream.read_exact(&mut byte).unwrap();
                request.push(byte[0]);
                assert!(request.len() < 8192);
            }
            assert!(String::from_utf8(request)
                .unwrap()
                .to_lowercase()
                .contains("x-api-key: test-key"));
            stream.write_all(&response).unwrap();
        }
    });
    (url, handle)
}
fn response(status: u16, body: &[u8]) -> Vec<u8> {
    let mut response = format!(
        "HTTP/1.1 {status} Test\r\nContent-Length: {}\r\nConnection: close\r\n\r\n",
        body.len()
    )
    .into_bytes();
    response.extend_from_slice(body);
    response
}
fn client() -> Client {
    Client::builder()
        .no_proxy()
        .redirect(reqwest::redirect::Policy::none())
        .build()
        .unwrap()
}
#[tokio::test]
async fn original_download_streams_and_publishes_exact_bytes_once_for_retries() {
    let temp = tempfile::tempdir().unwrap();
    let cache = Cache::new(temp.path().into()).unwrap();
    let original = include_bytes!("../../fixtures/drag-test.png");
    let (url, server) = mock(vec![
        response(200, br#"{"originalFileName":"ready.json"}"#),
        response(200, original),
    ]);
    let client = client();
    let meta = metadata(&url, A, "test-key", &client).await.unwrap();
    let a = cache
        .start(
            format!("account:{A}"),
            meta.clone(),
            url.clone(),
            "test-key".into(),
            client.clone(),
            events(),
        )
        .unwrap();
    let again = cache
        .start(
            format!("account:{A}"),
            meta,
            url,
            "test-key".into(),
            client,
            events(),
        )
        .unwrap();
    assert!(Arc::ptr_eq(&a.job, &again.job));
    let ready = tokio::task::spawn_blocking(move || a.wait())
        .await
        .unwrap()
        .unwrap();
    server.join().unwrap();
    assert_eq!(ready.path.file_name().unwrap(), "ready.json");
    assert_eq!(std::fs::read(ready.path).unwrap(), original);
}
#[tokio::test]
async fn truncated_download_is_never_readable_and_partial_is_removed() {
    let temp = tempfile::tempdir().unwrap();
    let cache = Cache::new(temp.path().into()).unwrap();
    let (url, server) = mock(vec![
        b"HTTP/1.1 200 OK\r\nContent-Length: 100\r\nConnection: close\r\n\r\nshort".to_vec(),
    ]);
    let transfer = cache
        .start(
            format!("account:{A}"),
            Metadata {
                original_file_name: "photo.jpg".into(),
            },
            url,
            "test-key".into(),
            client(),
            events(),
        )
        .unwrap();
    let dir = transfer.job.dir.clone();
    assert!(tokio::task::spawn_blocking(move || transfer.wait())
        .await
        .unwrap()
        .is_err());
    server.join().unwrap();
    assert!(!dir.join("original.partial").exists());
    assert!(!temp.path().join("current.json").exists());
}
#[tokio::test]
async fn rejects_auth_errors_redirects_and_traversal_before_original_download() {
    for reply in [response(401, b""), response(403, b""), response(404, b""), b"HTTP/1.1 302 Found\r\nLocation: https://example.org/\r\nContent-Length: 0\r\nConnection: close\r\n\r\n".to_vec(), response(200, br#"{"originalFileName":"../../escape.jpg"}"#)] {
        let (url, server) = mock(vec![reply]); assert!(metadata(&url, A, "test-key", &client()).await.is_err()); server.join().unwrap();
    }
}
#[tokio::test]
async fn eviction_cancels_only_unneeded_downloads() {
    let temp = tempfile::tempdir().unwrap();
    let cache = Cache::new(temp.path().into()).unwrap();
    let a = select(&cache, A);
    let job = a.job.clone();
    let b = select(&cache, B);
    assert!(
        tokio::time::timeout(Duration::from_millis(20), job.cancelled())
            .await
            .is_err()
    );
    drop(a);
    tokio::time::timeout(Duration::from_secs(1), job.cancelled())
        .await
        .unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(20), b.job.cancelled())
            .await
            .is_err()
    );
}
