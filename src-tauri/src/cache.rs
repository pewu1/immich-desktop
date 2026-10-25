//! One reusable original on disk; evicted entries live only as long as their readers.
use crate::policy;
use reqwest::{Client, Response};
use serde::{Deserialize, Serialize};
use std::{
    fs::File,
    io::{Read, Seek, SeekFrom, Write},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicUsize, Ordering},
        Arc, Condvar, Mutex,
    },
};
use tauri::{ipc::Channel, Url};
use tokio::io::AsyncWriteExt;
static ACTIVE: AtomicUsize = AtomicUsize::new(0);
static EXIT_WHEN_DONE: Mutex<Option<tauri::AppHandle>> = Mutex::new(None);
pub fn has_active_transfers() -> bool {
    ACTIVE.load(Ordering::Acquire) != 0
}
pub fn defer_exit(app: tauri::AppHandle) -> bool {
    let mut pending = EXIT_WHEN_DONE.lock().unwrap();
    if ACTIVE.load(Ordering::Acquire) == 0 {
        return false;
    }
    *pending = Some(app);
    true
}

const MAX_BYTES: u64 = 20 * 1024 * 1024 * 1024;
#[derive(Clone, Deserialize, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct Metadata {
    pub original_file_name: String,
}
#[derive(Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DragEvent {
    pub stage: &'static str,
    pub message: String,
}
pub type Events = Channel<DragEvent>;
pub fn emit(events: &Events, stage: &'static str, message: impl Into<String>) {
    let _ = events.send(DragEvent {
        stage,
        message: message.into(),
    });
}
#[derive(Clone, Deserialize, Serialize)]
struct Manifest {
    key: String,
    generation: String,
    name: String,
    size: u64,
}
#[derive(Clone)]
pub struct ReadyFile {
    pub path: PathBuf,
    pub size: u64,
}

pub struct Job {
    key: String,
    pub name: String,
    dir: PathBuf,
    keep: AtomicBool,
    readers: AtomicUsize,
    result: Mutex<Option<Result<ReadyFile, String>>>,
    completed: Condvar,
    changed: tokio::sync::Notify,
    subscribers: Mutex<Vec<Events>>,
}
impl Drop for Job {
    fn drop(&mut self) {
        if !self.keep.load(Ordering::Acquire) {
            let _ = std::fs::remove_dir_all(&self.dir);
        }
    }
}
pub struct Transfer {
    job: Arc<Job>,
    pub events: Events,
}
impl Drop for Transfer {
    fn drop(&mut self) {
        self.job.readers.fetch_sub(1, Ordering::AcqRel);
        self.job.changed.notify_waiters();
        if ACTIVE.fetch_sub(1, Ordering::AcqRel) == 1 {
            if let Some(app) = EXIT_WHEN_DONE.lock().unwrap().take() {
                app.exit(0);
            }
        }
    }
}
impl Transfer {
    pub fn readiness(&self) -> i32 {
        match self.job.result.lock() {
            Ok(result) => match result.as_ref() {
                None => 0,
                Some(Ok(_)) => 1,
                Some(Err(_)) => -1,
            },
            Err(_) => -1,
        }
    }
    pub fn name(&self) -> &str {
        &self.job.name
    }
    pub fn wait(&self) -> Result<ReadyFile, String> {
        let mut result = self
            .job
            .result
            .lock()
            .map_err(|_| "Transfer state unavailable")?;
        while result.is_none() {
            result = self
                .job
                .completed
                .wait(result)
                .map_err(|_| "Transfer interrupted")?;
        }
        result.as_ref().unwrap().clone()
    }
    pub fn read_at(&self, offset: u64, buffer: &mut [u8]) -> Result<usize, String> {
        let ready = self.wait()?;
        let mut file = File::open(ready.path).map_err(io_error)?;
        file.seek(SeekFrom::Start(offset)).map_err(io_error)?;
        file.read(buffer).map_err(io_error)
    }
    pub fn copy_to(&self, destination: &Path) -> Result<(), String> {
        let ready = self.wait()?;
        let mut source = File::open(ready.path).map_err(io_error)?;
        let mut target = std::fs::OpenOptions::new()
            .write(true)
            .create_new(true)
            .open(destination)
            .map_err(io_error)?;
        let copied = std::io::copy(&mut source, &mut target).and_then(|n| {
            target.sync_all()?;
            Ok(n)
        });
        drop(target);
        if copied.as_ref().is_ok_and(|size| *size == ready.size) {
            Ok(())
        } else {
            let _ = std::fs::remove_file(destination);
            Err("Could not deliver the complete original to the receiver".into())
        }
    }
}
impl Job {
    fn finish(&self, result: Result<ReadyFile, String>) {
        let event = match &result {
            Ok(_) => ("ready", "Original downloaded".to_string()),
            Err(error) => ("error", error.clone()),
        };
        *self.result.lock().unwrap() = Some(result);
        self.completed.notify_all();
        for events in self.subscribers.lock().unwrap().drain(..) {
            emit(&events, event.0, event.1.clone());
        }
    }
    async fn cancelled(&self) {
        loop {
            let notified = self.changed.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            if !self.keep.load(Ordering::Acquire) && self.readers.load(Ordering::Acquire) == 0 {
                return;
            }
            notified.await;
        }
    }
}
pub struct Cache {
    root: PathBuf,
    current: Mutex<Option<Arc<Job>>>,
}
impl Cache {
    pub fn clear(&self) -> Result<(), String> {
        let mut current = self.current.lock().map_err(|_| "Cache state unavailable")?;
        match std::fs::remove_file(self.root.join("current.json")) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(io_error(e)),
        }
        if let Some(old) = current.take() {
            old.keep.store(false, Ordering::Release);
            old.changed.notify_waiters();
        }
        Ok(())
    }
    pub fn new(root: PathBuf) -> Result<Arc<Self>, String> {
        std::fs::create_dir_all(&root).map_err(io_error)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&root, std::fs::Permissions::from_mode(0o700))
                .map_err(io_error)?;
        }
        let manifest = std::fs::read(root.join("current.json"))
            .ok()
            .and_then(|bytes| serde_json::from_slice::<Manifest>(&bytes).ok())
            .filter(|m| {
                policy::asset_id(&m.generation).is_ok() && policy::filename(&m.name).is_ok()
            })
            .filter(|m| {
                std::fs::metadata(root.join(&m.generation).join("file").join(&m.name))
                    .is_ok_and(|f| f.is_file() && f.len() == m.size && m.size > 0)
            });
        for entry in std::fs::read_dir(&root).map_err(io_error)? {
            let entry = entry.map_err(io_error)?;
            let name = entry.file_name().to_string_lossy().into_owned();
            let legacy = name.len() == 64 && name.bytes().all(|c| c.is_ascii_hexdigit());
            if entry.file_type().map_err(io_error)?.is_dir()
                && (policy::asset_id(&name).is_ok() || legacy)
                && manifest
                    .as_ref()
                    .is_none_or(|m| entry.file_name().to_str() != Some(&m.generation))
            {
                std::fs::remove_dir_all(entry.path()).map_err(io_error)?;
            }
        }
        let current = manifest.map(|m| {
            Arc::new(Job {
                key: m.key,
                name: m.name.clone(),
                dir: root.join(&m.generation),
                keep: AtomicBool::new(true),
                readers: AtomicUsize::new(0),
                result: Mutex::new(Some(Ok(ReadyFile {
                    path: root.join(m.generation).join("file").join(m.name),
                    size: m.size,
                }))),
                completed: Condvar::new(),
                changed: tokio::sync::Notify::new(),
                subscribers: Mutex::new(vec![]),
            })
        });
        Ok(Arc::new(Self {
            root,
            current: Mutex::new(current),
        }))
    }
    // Called only by a drag. A generation prevents A -> B -> A deletion/publication races.
    fn select(
        &self,
        key: String,
        name: String,
        events: Events,
        fresh: bool,
    ) -> Result<(Arc<Transfer>, bool), String> {
        policy::filename(&name)?;
        let mut current = self.current.lock().map_err(|_| "Cache state unavailable")?;
        if !fresh {
            if let Some(job) = current.as_ref().filter(|j| j.key == key) {
                let result = job.result.lock().unwrap();
                if result.as_ref().is_none_or(|r| r.is_ok()) {
                    job.readers.fetch_add(1, Ordering::AcqRel);
                    ACTIVE.fetch_add(1, Ordering::AcqRel);
                    if result.is_none() {
                        job.subscribers.lock().unwrap().push(events.clone());
                    } else {
                        emit(&events, "ready", "Reusing the last original from disk");
                    }
                    return Ok((
                        Arc::new(Transfer {
                            job: job.clone(),
                            events,
                        }),
                        false,
                    ));
                }
            }
        }
        match std::fs::remove_file(self.root.join("current.json")) {
            Ok(()) => {}
            Err(e) if e.kind() == std::io::ErrorKind::NotFound => {}
            Err(e) => return Err(io_error(e)),
        }
        if let Some(old) = current.take() {
            old.keep.store(false, Ordering::Release);
            old.changed.notify_waiters();
        }
        let dir = self.root.join(uuid::Uuid::new_v4().to_string());
        std::fs::create_dir_all(dir.join("file")).map_err(io_error)?;
        let job = Arc::new(Job {
            key,
            name,
            dir,
            keep: AtomicBool::new(true),
            readers: AtomicUsize::new(1),
            result: Mutex::new(None),
            completed: Condvar::new(),
            changed: tokio::sync::Notify::new(),
            subscribers: Mutex::new(vec![events.clone()]),
        });
        *current = Some(job.clone());
        ACTIVE.fetch_add(1, Ordering::AcqRel);
        Ok((Arc::new(Transfer { job, events }), true))
    }
    pub fn cached_metadata(&self, key: &str) -> Option<Metadata> {
        let current = self.current.lock().ok()?;
        let job = current.as_ref().filter(|j| j.key == key)?;
        let ready = job.result.lock().ok()?;
        if ready.as_ref().is_some_and(|r| r.is_ok()) {
            Some(Metadata {
                original_file_name: job.name.clone(),
            })
        } else {
            None
        }
    }
    fn publish(&self, job: &Job, size: u64) -> Result<ReadyFile, String> {
        let path = job.dir.join("file").join(&job.name);
        std::fs::rename(job.dir.join("original.partial"), &path).map_err(io_error)?;
        let current = self.current.lock().map_err(|_| "Cache state unavailable")?;
        if current.as_ref().is_some_and(|j| j.dir == job.dir) {
            let manifest = Manifest {
                key: job.key.clone(),
                generation: job.dir.file_name().unwrap().to_str().unwrap().into(),
                name: job.name.clone(),
                size,
            };
            let mut file = File::create(self.root.join("current.partial")).map_err(io_error)?;
            file.write_all(&serde_json::to_vec(&manifest).unwrap())
                .map_err(io_error)?;
            file.sync_all().map_err(io_error)?;
            drop(file);
            std::fs::rename(
                self.root.join("current.partial"),
                self.root.join("current.json"),
            )
            .map_err(io_error)?;
        }
        Ok(ReadyFile { path, size })
    }
    pub fn start(
        self: &Arc<Self>,
        key: String,
        metadata: Metadata,
        server: Url,
        api_key: String,
        client: Client,
        events: Events,
    ) -> Result<Arc<Transfer>, String> {
        let id = key
            .rsplit(':')
            .next()
            .ok_or("Invalid cache identity")?
            .to_string();
        policy::asset_id(&id)?;
        let (transfer, new) = self.select(key, metadata.original_file_name, events, false)?;
        if new {
            let job = transfer.job.clone();
            let cache = self.clone();
            tauri::async_runtime::spawn(async move {
                let result = tokio::select! {
                    result = download(&job, &server, &id, &api_key, &client) => result.and_then(|size| cache.publish(&job, size)),
                    _ = job.cancelled() => Err("Superseded transfer cancelled".into()),
                };
                let _ = std::fs::remove_file(job.dir.join("original.partial"));
                job.finish(result);
            });
        }
        Ok(transfer)
    }
    pub fn fixture(
        self: &Arc<Self>,
        events: Events,
        delay_ms: u64,
        fresh: bool,
    ) -> Result<Arc<Transfer>, String> {
        let (transfer, new) = self.select(
            "lab:11111111-1111-4111-8111-111111111111".into(),
            "drag-test.png".into(),
            events,
            fresh,
        )?;
        if new {
            let job = transfer.job.clone();
            let cache = self.clone();
            tauri::async_runtime::spawn(async move {
                let result = tokio::select! {
                    result = async {
                        tokio::time::sleep(std::time::Duration::from_millis(delay_ms)).await;
                        let bytes = include_bytes!("../../fixtures/drag-test.png");
                        let mut file = tokio::fs::File::create(job.dir.join("original.partial")).await.map_err(io_error)?;
                        file.write_all(bytes).await.map_err(io_error)?; file.sync_all().await.map_err(io_error)?; drop(file);
                        cache.publish(&job, bytes.len() as u64)
                    } => result,
                    _ = job.cancelled() => Err("Superseded transfer cancelled".into()),
                };
                let _ = std::fs::remove_file(job.dir.join("original.partial"));
                job.finish(result);
            });
        }
        Ok(transfer)
    }
}

#[cfg(test)]
#[path = "cache_tests.rs"]
mod tests;
fn io_error(_: std::io::Error) -> String {
    "File I/O failed; check disk space and permissions".into()
}
pub fn checked(response: Response) -> Result<Response, String> {
    match response.status().as_u16() {
        200 => Ok(response),
        401 => Err("API key expired or invalid".into()),
        403 => Err("API key lacks permission for this asset".into()),
        404 => Err("Asset missing or deleted".into()),
        300..=399 => Err("API redirects are refused; configure the final HTTPS origin".into()),
        _ => Err("Immich returned an unsuccessful API response".into()),
    }
}
pub async fn metadata(
    server: &Url,
    id: &str,
    key: &str,
    client: &Client,
) -> Result<Metadata, String> {
    let id = policy::asset_id(id)?;
    let mut response = checked(
        client
            .get(server.join(&format!("api/assets/{id}")).unwrap())
            .header("x-api-key", key)
            .send()
            .await
            .map_err(|_| "Cannot reach Immich; check TLS and network connectivity")?,
    )?;
    let mut json = Vec::new();
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| "Metadata download interrupted")?
    {
        if json.len() + chunk.len() > 1024 * 1024 {
            return Err("Asset metadata exceeds limit".into());
        }
        json.extend_from_slice(&chunk);
    }
    let metadata: Metadata = serde_json::from_slice(&json).map_err(|_| "Invalid asset metadata")?;
    policy::filename(&metadata.original_file_name)?;
    Ok(metadata)
}
async fn download(
    job: &Job,
    server: &Url,
    id: &str,
    key: &str,
    client: &Client,
) -> Result<u64, String> {
    let mut response = checked(
        client
            .get(server.join(&format!("api/assets/{id}/original")).unwrap())
            .header("x-api-key", key)
            .send()
            .await
            .map_err(|_| "Cannot download original; check TLS and network connectivity")?,
    )?;
    let expected = response.content_length();
    if expected.is_some_and(|n| n > MAX_BYTES) {
        return Err("Original exceeds 20 GiB limit".into());
    }
    let mut file = tokio::fs::File::create(job.dir.join("original.partial"))
        .await
        .map_err(io_error)?;
    let mut size = 0;
    while let Some(chunk) = response
        .chunk()
        .await
        .map_err(|_| "Original download interrupted")?
    {
        size += chunk.len() as u64;
        if size > MAX_BYTES {
            return Err("Original exceeds 20 GiB limit".into());
        }
        file.write_all(&chunk).await.map_err(io_error)?;
    }
    if size == 0 || expected.is_some_and(|n| n != size) {
        return Err("Original download incomplete".into());
    }
    file.sync_all().await.map_err(io_error)?;
    Ok(size)
}
