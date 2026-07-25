use crate::common::{not_implemented, DirEntry, Result, WatchHandle};
use async_trait::async_trait;
use std::path::Path;

#[async_trait]
pub trait FileSystemService: Send + Sync {
    async fn watch(&self, path: &Path) -> Result<WatchHandle>;
    async fn unwatch(&self, handle: WatchHandle) -> Result<()>;
    async fn read(&self, path: &Path) -> Result<Vec<u8>>;
    async fn write(&self, path: &Path, data: &[u8]) -> Result<()>;
    async fn r#move(&self, from: &Path, to: &Path) -> Result<()>;
    async fn copy(&self, from: &Path, to: &Path) -> Result<()>;
    async fn delete(&self, path: &Path) -> Result<()>;
    async fn list(&self, path: &Path) -> Result<Vec<DirEntry>>;
}

pub struct StubFileSystemService;

#[async_trait]
impl FileSystemService for StubFileSystemService {
    async fn watch(&self, _path: &Path) -> Result<WatchHandle> {
        not_implemented("FileSystemService::watch")
    }
    async fn unwatch(&self, _handle: WatchHandle) -> Result<()> {
        not_implemented("FileSystemService::unwatch")
    }
    async fn read(&self, _path: &Path) -> Result<Vec<u8>> {
        not_implemented("FileSystemService::read")
    }
    async fn write(&self, _path: &Path, _data: &[u8]) -> Result<()> {
        not_implemented("FileSystemService::write")
    }
    async fn r#move(&self, _from: &Path, _to: &Path) -> Result<()> {
        not_implemented("FileSystemService::move")
    }
    async fn copy(&self, _from: &Path, _to: &Path) -> Result<()> {
        not_implemented("FileSystemService::copy")
    }
    async fn delete(&self, _path: &Path) -> Result<()> {
        not_implemented("FileSystemService::delete")
    }
    async fn list(&self, _path: &Path) -> Result<Vec<DirEntry>> {
        not_implemented("FileSystemService::list")
    }
}

use crate::common::DimiError;
use crate::kernel::events::{topics, EventBus};
use notify::{Event as NotifyEvent, RecommendedWatcher, RecursiveMode, Watcher};
use std::collections::HashMap;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc as std_mpsc;
use std::sync::Mutex as StdMutex;

pub struct LocalFileSystemService {
    events: EventBus,
    watchers: StdMutex<HashMap<u64, RecommendedWatcher>>,
    next_handle: AtomicU64,
}

impl LocalFileSystemService {
    pub fn new(events: EventBus) -> Self {
        Self {
            events,
            watchers: StdMutex::new(HashMap::new()),
            next_handle: AtomicU64::new(1),
        }
    }
}

#[async_trait]
impl FileSystemService for LocalFileSystemService {
    async fn watch(&self, path: &Path) -> Result<WatchHandle> {
        let (tx, rx) = std_mpsc::channel::<notify::Result<NotifyEvent>>();
        let mut watcher = notify::recommended_watcher(tx)
            .map_err(|e| DimiError::Internal(format!("failed to create watcher: {e}")))?;
        watcher
            .watch(path, RecursiveMode::Recursive)
            .map_err(|e| DimiError::Internal(format!("failed to watch {}: {e}", path.display())))?;

        let events = self.events.clone();
        std::thread::spawn(move || {
            for res in rx {
                let Ok(event) = res else { continue };
                if !matches!(
                    event.kind,
                    notify::EventKind::Create(_) | notify::EventKind::Modify(_)
                ) {
                    continue;
                }
                for changed_path in event.paths {
                    if changed_path.components().any(|c| c.as_os_str() == ".dimi") {
                        continue;
                    }
                    events.publish(
                        topics::DOCUMENT_DETECTED,
                        serde_json::json!({ "path": changed_path.to_string_lossy() }),
                    );
                }
            }
        });

        let handle = self.next_handle.fetch_add(1, Ordering::SeqCst);
        self.watchers
            .lock()
            .expect("filesystem watcher registry lock poisoned")
            .insert(handle, watcher);
        Ok(WatchHandle(handle))
    }

    async fn unwatch(&self, handle: WatchHandle) -> Result<()> {
        self.watchers
            .lock()
            .expect("filesystem watcher registry lock poisoned")
            .remove(&handle.0);
        Ok(())
    }

    async fn read(&self, path: &Path) -> Result<Vec<u8>> {
        Ok(tokio::fs::read(path).await?)
    }

    async fn write(&self, path: &Path, data: &[u8]) -> Result<()> {
        if let Some(parent) = path.parent() {
            tokio::fs::create_dir_all(parent).await.ok();
        }
        Ok(tokio::fs::write(path, data).await?)
    }

    async fn r#move(&self, from: &Path, to: &Path) -> Result<()> {
        // Unlike `write`, `rename` doesn't create missing destination
        // directories on its own — without this, moving into a not-yet-
        // existing folder (the common "organize these into a new folder"
        // case) fails outright instead of creating the folder.
        if let Some(parent) = to.parent() {
            tokio::fs::create_dir_all(parent).await.ok();
        }
        Ok(tokio::fs::rename(from, to).await?)
    }

    async fn copy(&self, from: &Path, to: &Path) -> Result<()> {
        if let Some(parent) = to.parent() {
            tokio::fs::create_dir_all(parent).await.ok();
        }
        tokio::fs::copy(from, to).await?;
        Ok(())
    }

    async fn delete(&self, path: &Path) -> Result<()> {
        let metadata = tokio::fs::metadata(path).await?;
        if metadata.is_dir() {
            Ok(tokio::fs::remove_dir_all(path).await?)
        } else {
            Ok(tokio::fs::remove_file(path).await?)
        }
    }

    async fn list(&self, path: &Path) -> Result<Vec<DirEntry>> {
        let mut out = Vec::new();
        let mut read_dir = tokio::fs::read_dir(path).await?;
        while let Some(entry) = read_dir.next_entry().await? {
            let metadata = entry.metadata().await?;
            out.push(DirEntry {
                path: entry.path(),
                is_dir: metadata.is_dir(),
                size_bytes: metadata.len(),
            });
        }
        Ok(out)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn write_read_list_delete_roundtrip() {
        let dir = tempdir();
        let fs = LocalFileSystemService::new(EventBus::new());

        let file_path = dir.join("note.txt");
        fs.write(&file_path, b"hello dimi").await.unwrap();

        let contents = fs.read(&file_path).await.unwrap();
        assert_eq!(contents, b"hello dimi");

        let entries = fs.list(&dir).await.unwrap();
        assert!(entries.iter().any(|e| e.path == file_path && !e.is_dir));

        fs.delete(&file_path).await.unwrap();
        assert!(fs.read(&file_path).await.is_err());

        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn move_creates_a_destination_folder_that_does_not_exist_yet() {
        let dir = tempdir();
        let fs = LocalFileSystemService::new(EventBus::new());

        let source = dir.join("invoice.txt");
        fs.write(&source, b"total: 100").await.unwrap();

        let destination = dir.join("Invoices").join("2024").join("invoice.txt");
        fs.r#move(&source, &destination).await.unwrap();

        assert_eq!(fs.read(&destination).await.unwrap(), b"total: 100");
        assert!(fs.read(&source).await.is_err(), "the source should be gone after a move");

        std::fs::remove_dir_all(&dir).ok();
    }

    #[tokio::test]
    async fn copy_creates_a_destination_folder_that_does_not_exist_yet() {
        let dir = tempdir();
        let fs = LocalFileSystemService::new(EventBus::new());

        let source = dir.join("template.txt");
        fs.write(&source, b"draft").await.unwrap();

        let destination = dir.join("Archive").join("template.txt");
        fs.copy(&source, &destination).await.unwrap();

        assert_eq!(fs.read(&destination).await.unwrap(), b"draft");
        assert_eq!(fs.read(&source).await.unwrap(), b"draft", "the source should still exist after a copy");

        std::fs::remove_dir_all(&dir).ok();
    }

    fn tempdir() -> std::path::PathBuf {
        let dir = std::env::temp_dir().join(format!("dimi-fs-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }
}
