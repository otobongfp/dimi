use crate::common::{not_implemented, DownloadHandle, ModelInfo, Result};
use async_trait::async_trait;

#[async_trait]
pub trait ModelManager: Send + Sync {
    async fn list_available(&self) -> Result<Vec<ModelInfo>>;
    async fn list_installed(&self) -> Result<Vec<ModelInfo>>;
    async fn download(&self, model_id: &str) -> Result<DownloadHandle>;
    async fn verify(&self, model_id: &str) -> Result<bool>;
    async fn register(&self, model_id: &str) -> Result<()>;
    async fn remove(&self, model_id: &str) -> Result<()>;
    async fn active(&self) -> Result<ModelInfo>;
    async fn set_active(&self, model_id: &str) -> Result<()>;
}

pub struct StubModelManager;

#[async_trait]
impl ModelManager for StubModelManager {
    async fn list_available(&self) -> Result<Vec<ModelInfo>> {
        not_implemented("ModelManager::list_available")
    }
    async fn list_installed(&self) -> Result<Vec<ModelInfo>> {
        not_implemented("ModelManager::list_installed")
    }
    async fn download(&self, _model_id: &str) -> Result<DownloadHandle> {
        not_implemented("ModelManager::download")
    }
    async fn verify(&self, _model_id: &str) -> Result<bool> {
        not_implemented("ModelManager::verify")
    }
    async fn register(&self, _model_id: &str) -> Result<()> {
        not_implemented("ModelManager::register")
    }
    async fn remove(&self, _model_id: &str) -> Result<()> {
        not_implemented("ModelManager::remove")
    }
    async fn active(&self) -> Result<ModelInfo> {
        not_implemented("ModelManager::active")
    }
    async fn set_active(&self, _model_id: &str) -> Result<()> {
        not_implemented("ModelManager::set_active")
    }
}

use crate::common::{DimiError, InferenceBackend, JobId, ModelStatus, Row, SqlValue};
use crate::kernel::events::{topics, EventBus};
use crate::kernel::hardware::{self, RamTier};
use crate::services::storage::StorageEngine;
use sha2::{Digest, Sha256};
use std::path::{Path, PathBuf};
use std::sync::Arc;

#[derive(Clone)]
pub struct ModelCatalogEntry {
    pub id: &'static str,
    pub name: &'static str,
    pub repo_owner: &'static str,
    pub repo_name: &'static str,
    pub filename: &'static str,
    pub size_bytes: u64,
    pub needs_no_think: bool,
    pub kv_bytes_per_token: u64,
}

pub const LOW_RAM_MODEL_ID: &str = "Qwen3-0.6B-Q4_K_M";
pub const DEFAULT_MODEL_ID: &str = "Qwen3-1.7B-Q4_K_M";

pub fn catalog() -> Vec<ModelCatalogEntry> {
    vec![
        ModelCatalogEntry {
            id: LOW_RAM_MODEL_ID,
            name: "Qwen3 0.6B (Q4_K_M)",
            repo_owner: "unsloth",
            repo_name: "Qwen3-0.6B-GGUF",
            filename: "Qwen3-0.6B-Q4_K_M.gguf",
            size_bytes: 396_705_472,
            needs_no_think: false,
            kv_bytes_per_token: 28_672,
        },
        ModelCatalogEntry {
            id: DEFAULT_MODEL_ID,
            name: "Qwen3 1.7B (Q4_K_M)",
            repo_owner: "unsloth",
            repo_name: "Qwen3-1.7B-GGUF",
            filename: "Qwen3-1.7B-Q4_K_M.gguf",
            // Same num_layers (28) / num_kv_heads (8) / head_dim (128) as
            // 0.6B — Qwen3 scales 0.6B->1.7B via hidden_size (1024->2048),
            // not depth, so the KV cache footprint per token is identical
            // even though the model itself is ~3x the parameters.
            size_bytes: 1_107_409_472,
            needs_no_think: false,
            kv_bytes_per_token: 28_672,
        },
    ]
}

pub fn recommended_model_id(tier: RamTier) -> &'static str {
    match tier {
        RamTier::Low => LOW_RAM_MODEL_ID,
        RamTier::Mid | RamTier::High => DEFAULT_MODEL_ID,
    }
}

fn format_bytes(bytes: u64) -> String {
    const GB: f64 = 1024.0 * 1024.0 * 1024.0;
    format!("{:.2} GB", bytes as f64 / GB)
}

const DISK_SPACE_MARGIN_BYTES: u64 = 512 * 1024 * 1024;

pub struct SqliteModelManager {
    storage: Arc<dyn StorageEngine>,
    models_dir: PathBuf,
    events: EventBus,
}

impl SqliteModelManager {
    pub fn new(storage: Arc<dyn StorageEngine>, models_dir: PathBuf, events: EventBus) -> Self {
        Self {
            storage,
            models_dir,
            events,
        }
    }

    pub fn model_path(&self, model_id: &str) -> PathBuf {
        let entry = catalog().into_iter().find(|e| e.id == model_id);
        if let Some(entry) = entry {
            self.models_dir.join(entry.filename)
        } else {
            self.models_dir.join(format!("{}.gguf", model_id))
        }
    }

    pub async fn register_local_file(&self, model_id: &str, name: &str) -> Result<()> {
        let path = self.model_path(model_id);
        if !path.exists() {
            return Err(DimiError::NotFound(format!(
                "model file not found: {}",
                path.display()
            )));
        }
        let sha256 = compute_sha256(&path)?;
        let size_bytes = std::fs::metadata(&path)?.len();
        self.storage
            .query(
                "INSERT INTO models (id, name, backend, sha256, status, size_bytes, installed_at) \
                 VALUES (?1, ?2, 'llama_cpp', ?3, 'installed', ?4, ?5) \
                 ON CONFLICT(id) DO UPDATE SET \
                    sha256 = excluded.sha256, status = 'installed', \
                    size_bytes = excluded.size_bytes, installed_at = excluded.installed_at",
                &[
                    SqlValue::Text(model_id.to_string()),
                    SqlValue::Text(name.to_string()),
                    SqlValue::Text(sha256),
                    SqlValue::Integer(size_bytes as i64),
                    SqlValue::Integer(now_unix()),
                ],
            )
            .await?;
        Ok(())
    }

    pub async fn scan_and_register_local_files(&self) -> Result<Vec<&'static str>> {
        std::fs::create_dir_all(&self.models_dir)?;
        let mut discovered = Vec::new();

        for entry in catalog() {
            let canonical = self.model_path(entry.id);
            if !canonical.exists() {
                continue;
            }

            let already_installed = !self
                .storage
                .query(
                    "SELECT id FROM models WHERE id = ?1 AND status = 'installed'",
                    &[SqlValue::Text(entry.id.to_string())],
                )
                .await?
                .is_empty();
            if !already_installed {
                let size = std::fs::metadata(&canonical)?.len();
                if size == entry.size_bytes {
                    self.register_local_file(entry.id, entry.name).await?;
                    discovered.push(entry.id);
                } else {
                    tracing::warn!("discovered model {} has wrong size ({} vs {}). Deleting it.", canonical.display(), size, entry.size_bytes);
                    let _ = std::fs::remove_file(&canonical);
                }
            }
        }
        Ok(discovered)
    }
}

struct DownloadProgressHandler {
    events: EventBus,
    model_id: &'static str,
}

impl hf_hub::progress::ProgressHandler for DownloadProgressHandler {
    fn on_progress(&self, event: &hf_hub::progress::ProgressEvent) {
        use hf_hub::progress::{DownloadEvent, ProgressEvent};
        if let ProgressEvent::Download(DownloadEvent::Progress { files }) = event {
            if let Some(f) = files.first() {
                self.events.publish(
                    topics::MODEL_DOWNLOAD_PROGRESS,
                    serde_json::json!({
                        "model_id": self.model_id,
                        "status": "downloading",
                        "bytes_completed": f.bytes_completed,
                        "total_bytes": f.total_bytes,
                    }),
                );
            }
        }
    }
}

async fn run_download(
    entry: &ModelCatalogEntry,
    models_dir: &Path,
    storage: &Arc<dyn StorageEngine>,
    events: &EventBus,
) -> Result<()> {
    std::fs::create_dir_all(models_dir)?;
    events.publish(
        topics::MODEL_DOWNLOAD_PROGRESS,
        serde_json::json!({
            "model_id": entry.id,
            "status": "downloading",
            "bytes_completed": 0,
            "total_bytes": entry.size_bytes,
        }),
    );

    let client = hf_hub::HFClient::new()
        .map_err(|e| DimiError::Internal(format!("failed to create HF client: {e}")))?;
    let repo = client.model(entry.repo_owner, entry.repo_name);
    let handler = Arc::new(DownloadProgressHandler {
        events: events.clone(),
        model_id: entry.id,
    });

    let downloaded = repo
        .download_file()
        .filename(entry.filename)
        .local_dir(models_dir.to_path_buf())
        .progress(handler)
        .send()
        .await
        .map_err(|e| DimiError::Internal(format!("model download failed: {e}")))?;

    let metadata = std::fs::metadata(&downloaded)?;
    if metadata.len() != entry.size_bytes {
        std::fs::remove_file(&downloaded)?;
        return Err(DimiError::Internal(format!(
            "downloaded file size mismatch: expected {} bytes, got {} bytes. File has been deleted to allow redownload.",
            entry.size_bytes,
            metadata.len()
        )));
    }

    events.publish(
        topics::MODEL_DOWNLOAD_PROGRESS,
        serde_json::json!({ "model_id": entry.id, "status": "validating" }),
    );
    storage
        .query(
            "UPDATE models SET status = 'validating' WHERE id = ?1",
            &[SqlValue::Text(entry.id.to_string())],
        )
        .await?;

    let sha256 = compute_sha256(&downloaded)?;
    let size_bytes = std::fs::metadata(&downloaded)?.len();
    storage
        .query(
            "UPDATE models SET sha256 = ?1, size_bytes = ?2, status = 'installed', installed_at = ?3 WHERE id = ?4",
            &[
                SqlValue::Text(sha256),
                SqlValue::Integer(size_bytes as i64),
                SqlValue::Integer(now_unix()),
                SqlValue::Text(entry.id.to_string()),
            ],
        )
        .await?;

    crate::kernel::audit::record(storage, "system", "model.installed", Some(entry.id))
        .await
        .ok();

    events.publish(
        topics::MODEL_DOWNLOAD_PROGRESS,
        serde_json::json!({ "model_id": entry.id, "status": "installed" }),
    );
    events.publish(
        topics::MODEL_REGISTERED,
        serde_json::json!({ "model_id": entry.id }),
    );
    Ok(())
}

fn compute_sha256(path: &Path) -> Result<String> {
    use std::io::Read;

    let mut file = std::fs::File::open(path)?;
    let mut hasher = Sha256::new();
    let mut buf = [0u8; 65536];
    loop {
        let n = file.read(&mut buf)?;
        if n == 0 {
            break;
        }
        hasher.update(&buf[..n]);
    }
    let digest = hasher.finalize();
    Ok(digest.iter().map(|b| format!("{b:02x}")).collect())
}

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

fn parse_status(s: &str) -> ModelStatus {
    match s {
        "available" => ModelStatus::Available,
        "downloading" => ModelStatus::Downloading,
        "validating" => ModelStatus::Validating,
        "removing" => ModelStatus::Removing,
        _ => ModelStatus::Installed,
    }
}

fn row_to_model_info(row: &Row) -> Result<ModelInfo> {
    let get_text = |col: &str| match row.0.get(col) {
        Some(SqlValue::Text(s)) => Some(s.clone()),
        _ => None,
    };
    let id = get_text("id").ok_or_else(|| DimiError::Internal("model row missing id".into()))?;
    let name = get_text("name").unwrap_or_else(|| id.clone());
    let sha256 = get_text("sha256").unwrap_or_default();
    let status = parse_status(&get_text("status").unwrap_or_default());
    let size_bytes = match row.0.get("size_bytes") {
        Some(SqlValue::Integer(i)) => *i as u64,
        _ => 0,
    };
    let installed_at = match row.0.get("installed_at") {
        Some(SqlValue::Integer(i)) => Some(*i),
        _ => None,
    };
    Ok(ModelInfo {
        id,
        name,
        backend: InferenceBackend::LlamaCpp,
        sha256,
        status,
        size_bytes,
        installed_at,
    })
}

#[async_trait]
impl ModelManager for SqliteModelManager {
    async fn list_available(&self) -> Result<Vec<ModelInfo>> {
        Ok(catalog()
            .into_iter()
            .map(|entry| ModelInfo {
                id: entry.id.to_string(),
                name: entry.name.to_string(),
                backend: InferenceBackend::LlamaCpp,
                sha256: String::new(),
                status: ModelStatus::Available,
                size_bytes: entry.size_bytes,
                installed_at: None,
            })
            .collect())
    }

    async fn list_installed(&self) -> Result<Vec<ModelInfo>> {
        let rows = self
            .storage
            .query(
                "SELECT id, name, backend, sha256, status, size_bytes, installed_at \
                 FROM models WHERE status = 'installed'",
                &[],
            )
            .await?;
        rows.iter().map(row_to_model_info).collect()
    }

    async fn download(&self, model_id: &str) -> Result<crate::common::DownloadHandle> {
        let entry = catalog()
            .into_iter()
            .find(|m| m.id == model_id)
            .ok_or_else(|| DimiError::NotFound(format!("unknown model: {model_id}")))?;

        let required = entry.size_bytes + DISK_SPACE_MARGIN_BYTES;
        let available = hardware::available_disk_bytes(&self.models_dir);
        if available < required {
            return Err(DimiError::Internal(format!(
                "not enough disk space to download {}: need {} free, only {} available",
                entry.name,
                format_bytes(required),
                format_bytes(available),
            )));
        }

        self.storage
            .query(
                "INSERT INTO models (id, name, backend, sha256, status, size_bytes, installed_at) \
                 VALUES (?1, ?2, 'llama_cpp', '', 'downloading', ?3, NULL) \
                 ON CONFLICT(id) DO UPDATE SET status = 'downloading'",
                &[
                    SqlValue::Text(entry.id.to_string()),
                    SqlValue::Text(entry.name.to_string()),
                    SqlValue::Integer(entry.size_bytes as i64),
                ],
            )
            .await?;

        let job_id = JobId::new();
        let storage = self.storage.clone();
        let events = self.events.clone();
        let models_dir = self.models_dir.clone();

        tokio::spawn(async move {
            if let Err(e) = run_download(&entry, &models_dir, &storage, &events).await {
                tracing::error!(model_id = entry.id, error = %e, "model download failed");
                let _ = storage
                    .query(
                        "UPDATE models SET status = 'available' WHERE id = ?1",
                        &[SqlValue::Text(entry.id.to_string())],
                    )
                    .await;
                events.publish(
                    topics::MODEL_DOWNLOAD_PROGRESS,
                    serde_json::json!({
                        "model_id": entry.id,
                        "status": "failed",
                        "error": e.to_string(),
                    }),
                );
            }
        });

        Ok(crate::common::DownloadHandle {
            model_id: model_id.to_string(),
            job_id,
        })
    }

    async fn verify(&self, model_id: &str) -> Result<bool> {
        let path = self.model_path(model_id);
        if !path.exists() {
            return Ok(false);
        }
        let current = compute_sha256(&path)?;
        let rows = self
            .storage
            .query(
                "SELECT sha256 FROM models WHERE id = ?1",
                &[SqlValue::Text(model_id.to_string())],
            )
            .await?;
        let Some(row) = rows.into_iter().next() else {
            return Ok(false);
        };
        let stored = match row.0.get("sha256") {
            Some(SqlValue::Text(s)) => s.clone(),
            _ => return Ok(false),
        };
        Ok(stored == current)
    }

    async fn register(&self, model_id: &str) -> Result<()> {
        self.storage
            .query(
                "UPDATE models SET status = 'installed' WHERE id = ?1",
                &[SqlValue::Text(model_id.to_string())],
            )
            .await?;
        crate::kernel::audit::record(&self.storage, "user", "model.installed", Some(model_id))
            .await
            .ok();
        Ok(())
    }

    async fn remove(&self, model_id: &str) -> Result<()> {
        self.storage
            .query(
                "DELETE FROM models WHERE id = ?1",
                &[SqlValue::Text(model_id.to_string())],
            )
            .await?;
        crate::kernel::audit::record(&self.storage, "user", "model.removed", Some(model_id))
            .await
            .ok();
        Ok(())
    }

    async fn active(&self) -> Result<ModelInfo> {
        let active_id = self
            .storage
            .get("config", "active_model_id")
            .await?
            .ok_or_else(|| DimiError::NotFound("no active model set".into()))?;
        let id = String::from_utf8(active_id).map_err(|e| DimiError::Internal(e.to_string()))?;
        let rows = self
            .storage
            .query(
                "SELECT id, name, backend, sha256, status, size_bytes, installed_at FROM models WHERE id = ?1",
                &[SqlValue::Text(id)],
            )
            .await?;
        let row = rows
            .into_iter()
            .next()
            .ok_or_else(|| DimiError::NotFound("active model not found".into()))?;
        row_to_model_info(&row)
    }

    async fn set_active(&self, model_id: &str) -> Result<()> {
        self.storage
            .put("config", "active_model_id", model_id.as_bytes())
            .await
    }
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::storage::SqliteStorageEngine;

    fn tempdir() -> PathBuf {
        let dir = std::env::temp_dir().join(format!("dimi-models-test-{}", uuid::Uuid::new_v4()));
        std::fs::create_dir_all(&dir).unwrap();
        dir
    }

    #[tokio::test]
    async fn scan_discovers_a_file_under_its_original_hf_filename() {
        let models_dir = tempdir();
        let storage: Arc<dyn StorageEngine> =
            Arc::new(SqliteStorageEngine::open_in_memory_for_test().unwrap());
        let manager = SqliteModelManager::new(storage, models_dir.clone(), EventBus::new());

        let entry = &catalog()[0];
        let file = std::fs::File::create(models_dir.join(entry.filename)).unwrap();
        file.set_len(entry.size_bytes).unwrap();

        let discovered = manager.scan_and_register_local_files().await.unwrap();
        assert_eq!(discovered, vec![entry.id]);

        assert!(manager.model_path(entry.id).exists());

        let installed = manager.list_installed().await.unwrap();
        assert!(installed.iter().any(|m| m.id == entry.id));

        let discovered_again = manager.scan_and_register_local_files().await.unwrap();
        assert!(discovered_again.is_empty());

        std::fs::remove_dir_all(&models_dir).ok();
    }

    #[tokio::test]
    async fn scan_finds_nothing_in_an_empty_folder() {
        let models_dir = tempdir();
        let storage: Arc<dyn StorageEngine> =
            Arc::new(SqliteStorageEngine::open_in_memory_for_test().unwrap());
        let manager = SqliteModelManager::new(storage, models_dir.clone(), EventBus::new());

        let discovered = manager.scan_and_register_local_files().await.unwrap();
        assert!(discovered.is_empty());
        assert!(manager.list_installed().await.unwrap().is_empty());

        std::fs::remove_dir_all(&models_dir).ok();
    }
}
