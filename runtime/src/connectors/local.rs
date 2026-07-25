use crate::common::{ConnectorEntry, ConnectorKind, EntryMetadata, Result, WatchHandle};
use crate::connectors::Connector;
use crate::services::filesystem::FileSystemService;
use async_trait::async_trait;
use std::path::Path;
use std::sync::Arc;

pub struct LocalConnector {
    filesystem: Arc<dyn FileSystemService>,
}

impl LocalConnector {
    pub fn new(filesystem: Arc<dyn FileSystemService>) -> Self {
        Self { filesystem }
    }
}

#[async_trait]
impl Connector for LocalConnector {
    fn kind(&self) -> ConnectorKind {
        ConnectorKind::Local
    }

    async fn list(&self, path: &str) -> Result<Vec<ConnectorEntry>> {
        let entries = self.filesystem.list(Path::new(path)).await?;
        Ok(entries
            .into_iter()
            .map(|e| ConnectorEntry {
                path: e.path.to_string_lossy().into_owned(),
                is_dir: e.is_dir,
                size_bytes: e.size_bytes,
            })
            .collect())
    }

    async fn read(&self, path: &str) -> Result<Vec<u8>> {
        self.filesystem.read(Path::new(path)).await
    }

    async fn write(&self, path: &str, data: &[u8]) -> Result<()> {
        self.filesystem.write(Path::new(path), data).await
    }

    async fn watch(&self, path: &str) -> Result<WatchHandle> {
        self.filesystem.watch(Path::new(path)).await
    }

    async fn metadata(&self, path: &str) -> Result<EntryMetadata> {
        let meta = tokio::fs::metadata(path).await?;
        let mime_type = mime_guess_from_path(path);
        Ok(EntryMetadata {
            size_bytes: meta.len(),
            modified_at: meta
                .modified()
                .ok()
                .and_then(|t| t.duration_since(std::time::UNIX_EPOCH).ok())
                .map(|d| d.as_secs() as i64)
                .unwrap_or(0),
            mime_type,
        })
    }
}

fn mime_guess_from_path(path: &str) -> Option<String> {
    let ext = Path::new(path).extension()?.to_str()?.to_ascii_lowercase();
    Some(
        match ext.as_str() {
            "pdf" => "application/pdf",
            "docx" => "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
            "xlsx" | "xls" => "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
            "csv" => "text/csv",
            "txt" | "md" => "text/plain",
            "png" => "image/png",
            "jpg" | "jpeg" => "image/jpeg",
            _ => "application/octet-stream",
        }
        .to_string(),
    )
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::kernel::events::EventBus;
    use crate::services::filesystem::LocalFileSystemService;

    #[tokio::test]
    async fn round_trips_through_filesystem_service() {
        let fs: Arc<dyn FileSystemService> = Arc::new(LocalFileSystemService::new(EventBus::new()));
        let connector = LocalConnector::new(fs);

        let dir =
            std::env::temp_dir().join(format!("dimi-connector-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        let file_path = dir.join("doc.txt");
        let file_path_str = file_path.to_string_lossy().into_owned();

        connector
            .write(&file_path_str, b"hello repository")
            .await
            .unwrap();
        let data = connector.read(&file_path_str).await.unwrap();
        assert_eq!(data, b"hello repository");

        let meta = connector.metadata(&file_path_str).await.unwrap();
        assert_eq!(meta.size_bytes, "hello repository".len() as u64);
        assert_eq!(meta.mime_type.as_deref(), Some("text/plain"));

        std::fs::remove_dir_all(&dir).ok();
    }
}
