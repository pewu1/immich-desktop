//! Completed outgoing groups are retained independently of the one-file cache.
use crate::{cache, policy};
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};
use std::{
    collections::{HashSet, VecDeque},
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex,
    },
};

#[derive(Default)]
pub struct Control(Mutex<ControlState>);
#[derive(Default)]
struct ControlState {
    current: Option<(String, Arc<AtomicBool>)>,
    cancelled: VecDeque<String>,
}
pub struct Ticket {
    control: Arc<Control>,
    id: String,
    cancelled: Arc<AtomicBool>,
    pub activity: Option<cache::Activity>,
}
impl Control {
    pub fn cancel_current(&self) {
        if let Ok(state) = self.0.lock() {
            if let Some((_, cancelled)) = &state.current {
                cancelled.store(true, Ordering::Release);
            }
        }
    }
    pub fn begin(self: &Arc<Self>, id: &str) -> Result<Ticket, String> {
        let id = policy::asset_id(id)?;
        let mut state = self.0.lock().map_err(|_| "Drag state unavailable")?;
        if state.cancelled.contains(&id) {
            return Err("Drag already released".into());
        }
        if let Some((_, previous)) = state.current.take() {
            previous.store(true, Ordering::Release);
        }
        let cancelled = Arc::new(AtomicBool::new(false));
        state.current = Some((id.clone(), cancelled.clone()));
        Ok(Ticket {
            control: self.clone(),
            id,
            cancelled,
            activity: Some(cache::Activity::new()),
        })
    }
    pub fn cancel(&self, id: &str) -> Result<(), String> {
        let id = policy::asset_id(id)?;
        let mut state = self.0.lock().map_err(|_| "Drag state unavailable")?;
        if let Some((current, cancelled)) = &state.current {
            if current == &id {
                cancelled.store(true, Ordering::Release);
            }
        }
        // Cancellation can arrive before the async drag command is dispatched.
        if !state.cancelled.contains(&id) {
            state.cancelled.push_back(id);
        }
        if state.cancelled.len() > 256 {
            state.cancelled.pop_front();
        }
        Ok(())
    }
}
impl Ticket {
    pub fn check(&self) -> Result<(), String> {
        if self.cancelled.load(Ordering::Acquire) {
            Err("Drag released or cancelled".into())
        } else {
            Ok(())
        }
    }
    pub async fn cancelled(&self) {
        while self.check().is_ok() {
            tokio::time::sleep(std::time::Duration::from_millis(20)).await;
        }
    }
}
impl Drop for Ticket {
    fn drop(&mut self) {
        if let Ok(mut state) = self.control.0.lock() {
            if state.current.as_ref().is_some_and(|(id, _)| id == &self.id) {
                state.current = None;
            }
        }
    }
}

#[derive(Serialize, Deserialize)]
pub struct File {
    pub id: String,
    pub name: String,
    pub size: u64,
}
pub fn ids(input: Vec<String>) -> Result<Vec<String>, String> {
    if input.is_empty() || input.len() > 10_000 {
        return Err("Select between 1 and 10,000 assets".into());
    }
    let mut seen = HashSet::new();
    input
        .into_iter()
        .map(|id| policy::asset_id(&id))
        .collect::<Result<Vec<_>, _>>()
        .map(|ids| {
            ids.into_iter()
                .filter(|id| seen.insert(id.clone()))
                .collect()
        })
}
pub fn key(namespace: &str, ids: &[String]) -> String {
    let mut hash = Sha256::new();
    hash.update(namespace);
    hash.update([0]);
    for id in ids {
        hash.update(id);
        hash.update([0]);
    }
    format!("{:x}", hash.finalize())
}
pub fn names(originals: &[String]) -> Result<Vec<String>, String> {
    let mut reserved = HashSet::new();
    for name in originals {
        policy::filename(name)?;
        reserved.insert(name.to_lowercase());
    }
    let mut used = HashSet::new();
    let mut result = Vec::new();
    for name in originals {
        if used.insert(name.to_lowercase()) {
            result.push(name.clone());
            continue;
        }
        let (stem, extension) = name
            .rsplit_once('.')
            .filter(|(stem, _)| !stem.is_empty())
            .map(|(s, e)| (s, format!(".{e}")))
            .unwrap_or((name.as_str(), String::new()));
        for index in 2.. {
            let suffix = format!(" ({index}){extension}");
            let available = 240_usize
                .checked_sub(suffix.len())
                .filter(|n| *n > 0)
                .ok_or("Cannot disambiguate original filename")?;
            let mut end = stem.len().min(available);
            while !stem.is_char_boundary(end) {
                end -= 1;
            }
            let candidate = format!("{}{suffix}", &stem[..end]);
            if !reserved.contains(&candidate.to_lowercase())
                && used.insert(candidate.to_lowercase())
            {
                policy::filename(&candidate)?;
                result.push(candidate);
                break;
            }
        }
    }
    Ok(result)
}
pub fn existing(root: &Path, key: &str, ids: &[String]) -> Option<Vec<PathBuf>> {
    let dir = root.join(key);
    let files: Vec<File> =
        serde_json::from_slice(&std::fs::read(dir.join("files.json")).ok()?).ok()?;
    if files.len() != ids.len() {
        return None;
    }
    let mut unique = HashSet::new();
    files
        .into_iter()
        .zip(ids)
        .map(|(file, id)| {
            if file.id != *id
                || policy::filename(&file.name).is_err()
                || !unique.insert(file.name.to_lowercase())
            {
                return None;
            }
            let path = dir.join("files").join(file.name);
            let metadata = std::fs::metadata(&path).ok()?;
            (metadata.is_file() && file.size > 0 && metadata.len() == file.size).then_some(path)
        })
        .collect()
}
pub struct Staging {
    pub dir: PathBuf,
}
impl Staging {
    pub fn new(root: &Path) -> Result<Self, String> {
        let dir = root.join(format!(".partial-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(dir.join("files")).map_err(|_| "Cannot create outgoing group")?;
        Ok(Self { dir })
    }
    pub fn commit(self, root: &Path, key: &str, files: &[File]) -> Result<Vec<PathBuf>, String> {
        use std::io::Write;
        let mut manifest = std::fs::File::create(self.dir.join("files.json"))
            .map_err(|_| "Cannot create group manifest")?;
        manifest
            .write_all(&serde_json::to_vec(files).map_err(|_| "Cannot serialize group")?)
            .map_err(|_| "Cannot save group manifest")?;
        manifest
            .sync_all()
            .map_err(|_| "Cannot save group manifest")?;
        drop(manifest);
        let ids = files.iter().map(|file| file.id.clone()).collect::<Vec<_>>();
        if let Some(paths) = existing(root, key, &ids) {
            return Ok(paths);
        }
        std::fs::rename(&self.dir, root.join(key)).map_err(|_| "Cannot retain outgoing group")?;
        Ok(files
            .iter()
            .map(|file| root.join(key).join("files").join(&file.name))
            .collect())
    }
}
impl Drop for Staging {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.dir);
    }
}
pub fn clean_partial(root: &Path) -> Result<(), String> {
    if !root.exists() {
        return Ok(());
    }
    for entry in std::fs::read_dir(root).map_err(|_| "Cannot inspect outgoing groups")? {
        let entry = entry.map_err(|_| "Cannot inspect outgoing group")?;
        let name = entry.file_name().to_string_lossy().into_owned();
        if name
            .strip_prefix(".partial-")
            .is_some_and(|id| policy::asset_id(id).is_ok())
            && entry
                .file_type()
                .map_err(|_| "Cannot inspect staging")?
                .is_dir()
        {
            std::fs::remove_dir_all(entry.path())
                .map_err(|_| "Cannot remove interrupted outgoing group")?;
        }
    }
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;
    const A: &str = "11111111-1111-4111-8111-111111111111";
    const B: &str = "22222222-2222-4222-8222-222222222222";
    #[test]
    fn validates_all_ids_and_disambiguates_names_without_stealing_original_names() {
        assert!(ids(vec![]).is_err());
        assert!(ids(vec![A.into(), "../bad".into()]).is_err());
        assert_eq!(ids(vec![A.into(), A.into(), B.into()]).unwrap(), vec![A, B]);
        assert_eq!(
            names(&[
                "a.jpg".into(),
                "a.jpg".into(),
                "a (2).jpg".into(),
                "A.JPG".into()
            ])
            .unwrap(),
            vec!["a.jpg", "a (3).jpg", "a (2).jpg", "A (4).JPG"]
        );
        assert!(names(&["../bad.jpg".into()]).is_err());
    }
    #[test]
    fn cancellation_before_dispatch_and_repress_cannot_start_old_groups() {
        let control = Arc::new(Control::default());
        control.cancel(A).unwrap();
        assert!(control.begin(A).is_err());
        let old = control.begin(B).unwrap();
        let new_id = uuid::Uuid::new_v4().to_string();
        let new = control.begin(&new_id).unwrap();
        assert!(old.check().is_err());
        assert!(new.check().is_ok());
        drop(old);
        control.cancel(&new_id).unwrap();
        assert!(new.check().is_err());
    }
    #[test]
    fn completed_groups_survive_restart_cleanup_and_partial_groups_do_not() {
        let root = tempfile::tempdir().unwrap();
        let ids = vec![A.into(), B.into()];
        let group_key = key("account", &ids);
        let stage = Staging::new(root.path()).unwrap();
        let files = vec![
            File {
                id: A.into(),
                name: "one.jpg".into(),
                size: 3,
            },
            File {
                id: B.into(),
                name: "two.mp4".into(),
                size: 3,
            },
        ];
        for file in &files {
            std::fs::write(stage.dir.join("files").join(&file.name), b"123").unwrap();
        }
        let paths = stage.commit(root.path(), &group_key, &files).unwrap();
        let partial = Staging::new(root.path()).unwrap();
        clean_partial(root.path()).unwrap();
        assert!(!partial.dir.exists());
        assert_eq!(existing(root.path(), &group_key, &ids).unwrap(), paths);
        assert!(existing(root.path(), &key("other", &ids), &ids).is_none());
        std::fs::write(&paths[0], b"broken").unwrap();
        assert!(existing(root.path(), &group_key, &ids).is_none());
    }
}
