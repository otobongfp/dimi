use crate::api::Watcher;
use async_trait::async_trait;
use notify::{Config, Event, RecommendedWatcher, RecursiveMode, Watcher as NotifyWatcher};
use std::path::Path;
use std::sync::Mutex;
use tokio::sync::mpsc;

const EVENT_CHANNEL_CAPACITY: usize = 100;

pub struct FsWatcher {
    inner: Mutex<Option<RecommendedWatcher>>,
    tx: mpsc::Sender<Event>,
    rx: Mutex<Option<mpsc::Receiver<Event>>>,
}

impl FsWatcher {
    pub fn new() -> Self {
        let (tx, rx) = mpsc::channel(EVENT_CHANNEL_CAPACITY);
        Self {
            inner: Mutex::new(None),
            tx,
            rx: Mutex::new(Some(rx)),
        }
    }

    /// Takes the filesystem-event receiver. Only yields `Some` once per
    /// instance; a caller that wants to react to changes must take this
    /// before (or after) calling `watch()`.
    pub fn take_events(&self) -> Option<mpsc::Receiver<Event>> {
        self.rx.lock().expect("fs watcher lock poisoned").take()
    }
}

impl Default for FsWatcher {
    fn default() -> Self {
        Self::new()
    }
}

#[async_trait]
impl Watcher for FsWatcher {
    async fn watch(&self, root: &Path) -> Result<(), crate::Error> {
        let tx = self.tx.clone();
        let mut watcher = RecommendedWatcher::new(
            move |res: Result<Event, notify::Error>| {
                if let Ok(event) = res {
                    let _ = tx.blocking_send(event);
                }
            },
            Config::default(),
        )
        .map_err(|e| crate::Error::Internal(e.to_string()))?;

        watcher
            .watch(root, RecursiveMode::Recursive)
            .map_err(|e| crate::Error::Internal(e.to_string()))?;

        // Storing the watcher (rather than letting it drop here) is what
        // keeps the OS-level subscription alive after this call returns.
        *self.inner.lock().expect("fs watcher lock poisoned") = Some(watcher);
        Ok(())
    }

    async fn stop(&self) -> Result<(), crate::Error> {
        // Dropping the watcher unregisters it with the OS.
        self.inner.lock().expect("fs watcher lock poisoned").take();
        Ok(())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn watch_delivers_events_for_a_new_file() {
        let root = std::env::temp_dir().join(format!("isfi-watcher-test-{}", std::process::id()));
        tokio::fs::create_dir_all(&root).await.unwrap();

        let watcher = FsWatcher::new();
        let mut events = watcher.take_events().unwrap();
        watcher.watch(&root).await.unwrap();

        tokio::fs::write(root.join("new.txt"), b"hello").await.unwrap();

        let event = tokio::time::timeout(std::time::Duration::from_secs(5), events.recv())
            .await
            .expect("timed out waiting for a filesystem event")
            .expect("event channel closed unexpectedly");
        assert!(!event.paths.is_empty());

        watcher.stop().await.unwrap();
        tokio::fs::remove_dir_all(&root).await.ok();
    }
}
