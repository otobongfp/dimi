use crate::common::{
    not_implemented, PluginId, PluginManifest, PluginRecord, PluginSource, Result,
};
use async_trait::async_trait;

#[async_trait]
pub trait PluginManager: Send + Sync {
    async fn discover(&self) -> Result<Vec<PluginManifest>>;
    async fn install(&self, source: PluginSource) -> Result<PluginId>;
    async fn enable(&self, id: PluginId) -> Result<()>;
    async fn disable(&self, id: PluginId) -> Result<()>;
    async fn uninstall(&self, id: PluginId) -> Result<()>;
    async fn list(&self) -> Result<Vec<PluginRecord>>;
}

pub struct StubPluginManager;

#[async_trait]
impl PluginManager for StubPluginManager {
    async fn discover(&self) -> Result<Vec<PluginManifest>> {
        not_implemented("PluginManager::discover")
    }
    async fn install(&self, _source: PluginSource) -> Result<PluginId> {
        not_implemented("PluginManager::install")
    }
    async fn enable(&self, _id: PluginId) -> Result<()> {
        not_implemented("PluginManager::enable")
    }
    async fn disable(&self, _id: PluginId) -> Result<()> {
        not_implemented("PluginManager::disable")
    }
    async fn uninstall(&self, _id: PluginId) -> Result<()> {
        not_implemented("PluginManager::uninstall")
    }
    async fn list(&self) -> Result<Vec<PluginRecord>> {
        not_implemented("PluginManager::list")
    }
}

use crate::common::{DimiError, PluginState, SqlValue};
use crate::services::storage::StorageEngine;
use std::path::{Path, PathBuf};
use std::sync::Arc;

pub struct FilePluginManager {
    storage: Arc<dyn StorageEngine>,
    plugins_dir: PathBuf,
}

impl FilePluginManager {
    pub fn new(storage: Arc<dyn StorageEngine>, plugins_dir: PathBuf) -> Self {
        Self {
            storage,
            plugins_dir,
        }
    }

    fn manifest_dir(&self, id: &PluginId) -> PathBuf {
        self.plugins_dir.join(&id.0)
    }

    async fn read_manifest(dir: &Path) -> Result<PluginManifest> {
        let manifest_path = dir.join("plugin.yaml");
        let text = tokio::fs::read_to_string(&manifest_path)
            .await
            .map_err(|e| {
                DimiError::NotFound(format!(
                    "plugin manifest not found at {}: {e}",
                    manifest_path.display()
                ))
            })?;
        serde_yaml::from_str(&text)
            .map_err(|e| DimiError::InvalidArgument(format!("invalid plugin manifest: {e}")))
    }
}

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

fn parse_state(s: &str) -> PluginState {
    match s {
        "discovered" => PluginState::Discovered,
        "validated" => PluginState::Validated,
        "enabled" => PluginState::Enabled,
        "disabled" => PluginState::Disabled,
        "failed" => PluginState::Failed,
        "uninstalled" => PluginState::Uninstalled,
        _ => PluginState::Installed,
    }
}

#[async_trait]
impl PluginManager for FilePluginManager {
    async fn discover(&self) -> Result<Vec<PluginManifest>> {
        if !self.plugins_dir.exists() {
            return Ok(Vec::new());
        }
        let mut manifests = Vec::new();
        let mut entries = tokio::fs::read_dir(&self.plugins_dir).await?;
        while let Some(entry) = entries.next_entry().await? {
            if !entry.file_type().await.map(|t| t.is_dir()).unwrap_or(false) {
                continue;
            }
            match Self::read_manifest(&entry.path()).await {
                Ok(manifest) => manifests.push(manifest),
                Err(e) => {
                    tracing::warn!(path = %entry.path().display(), error = %e, "skipping invalid plugin manifest");
                }
            }
        }
        Ok(manifests)
    }

    async fn install(&self, source: PluginSource) -> Result<PluginId> {
        let PluginSource::Path(source_path) = source;
        let source_dir = if source_path.is_dir() {
            source_path
        } else {
            source_path
                .parent()
                .ok_or_else(|| {
                    DimiError::InvalidArgument("plugin source path has no parent directory".into())
                })?
                .to_path_buf()
        };
        let manifest = Self::read_manifest(&source_dir).await?;
        let id = PluginId::new(manifest.name.clone());

        let dest_dir = self.manifest_dir(&id);
        if dest_dir != source_dir {
            copy_dir_recursive(&source_dir, &dest_dir).await?;
        }

        self.storage
            .query(
                "INSERT INTO plugins (id, manifest_version, state, installed_at) VALUES (?1, ?2, 'installed', ?3) \
                 ON CONFLICT(id) DO UPDATE SET manifest_version = excluded.manifest_version, \
                    state = 'installed', installed_at = excluded.installed_at",
                &[
                    SqlValue::Text(id.0.clone()),
                    SqlValue::Text(manifest.version.clone()),
                    SqlValue::Integer(now_unix()),
                ],
            )
            .await?;
        crate::kernel::audit::record(&self.storage, "user", "plugin.installed", Some(&id.0))
            .await
            .ok();

        Ok(id)
    }

    async fn enable(&self, id: PluginId) -> Result<()> {
        self.storage
            .query(
                "UPDATE plugins SET state = 'enabled' WHERE id = ?1",
                &[SqlValue::Text(id.0.clone())],
            )
            .await?;
        crate::kernel::audit::record(&self.storage, "user", "plugin.enabled", Some(&id.0))
            .await
            .ok();
        Ok(())
    }

    async fn disable(&self, id: PluginId) -> Result<()> {
        self.storage
            .query(
                "UPDATE plugins SET state = 'disabled' WHERE id = ?1",
                &[SqlValue::Text(id.0.clone())],
            )
            .await?;
        crate::kernel::audit::record(&self.storage, "user", "plugin.disabled", Some(&id.0))
            .await
            .ok();
        Ok(())
    }

    async fn uninstall(&self, id: PluginId) -> Result<()> {
        self.storage
            .query(
                "DELETE FROM plugins WHERE id = ?1",
                &[SqlValue::Text(id.0.clone())],
            )
            .await?;
        crate::kernel::audit::record(&self.storage, "user", "plugin.uninstalled", Some(&id.0))
            .await
            .ok();
        let _ = id;
        Ok(())
    }

    async fn list(&self) -> Result<Vec<PluginRecord>> {
        let rows = self
            .storage
            .query("SELECT id, state FROM plugins", &[])
            .await?;
        let mut records = Vec::new();
        for row in rows {
            let Some(SqlValue::Text(id_str)) = row.0.get("id") else {
                continue;
            };
            let id = PluginId::new(id_str.clone());
            let state = match row.0.get("state") {
                Some(SqlValue::Text(s)) => parse_state(s),
                _ => PluginState::Installed,
            };
            match Self::read_manifest(&self.manifest_dir(&id)).await {
                Ok(manifest) => records.push(PluginRecord {
                    id,
                    manifest,
                    state,
                }),
                Err(e) => {
                    tracing::warn!(plugin = %id, error = %e, "installed plugin's manifest is missing/invalid");
                }
            }
        }
        Ok(records)
    }
}

fn copy_dir_recursive<'a>(
    from: &'a Path,
    to: &'a Path,
) -> std::pin::Pin<Box<dyn std::future::Future<Output = Result<()>> + Send + 'a>> {
    Box::pin(async move {
        tokio::fs::create_dir_all(to).await?;
        let mut entries = tokio::fs::read_dir(from).await?;
        while let Some(entry) = entries.next_entry().await? {
            let file_type = entry.file_type().await?;
            let dest = to.join(entry.file_name());
            if file_type.is_dir() {
                copy_dir_recursive(&entry.path(), &dest).await?;
            } else {
                tokio::fs::copy(entry.path(), dest).await?;
            }
        }
        Ok(())
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use crate::services::storage::SqliteStorageEngine;

    const TEST_PLUGIN_MANIFEST: &str =
        include_str!("../../tests/fixtures/dimi-test-plugin/plugin.yaml");

    fn temp_plugins_dir() -> PathBuf {
        std::env::temp_dir().join(format!("dimi-plugins-test-{}", uuid::Uuid::new_v4()))
    }

    async fn seed_source_plugin(plugins_root: &Path) -> PathBuf {
        let source_dir = plugins_root.join("source").join("dimi-test-plugin");
        tokio::fs::create_dir_all(&source_dir).await.unwrap();
        tokio::fs::write(source_dir.join("plugin.yaml"), TEST_PLUGIN_MANIFEST)
            .await
            .unwrap();
        source_dir
    }

    #[tokio::test]
    async fn full_lifecycle_install_enable_disable_uninstall() {
        let plugins_root = temp_plugins_dir();
        let installed_dir = plugins_root.join("installed");
        let source_dir = seed_source_plugin(&plugins_root).await;

        let storage: Arc<dyn StorageEngine> =
            Arc::new(SqliteStorageEngine::open_in_memory_for_test().unwrap());
        let manager = FilePluginManager::new(storage, installed_dir.clone());

        let source_manager = FilePluginManager::new(
            Arc::new(SqliteStorageEngine::open_in_memory_for_test().unwrap()),
            source_dir.parent().unwrap().to_path_buf(),
        );
        let discovered = source_manager.discover().await.unwrap();
        assert_eq!(discovered.len(), 1);
        assert_eq!(discovered[0].name, "dimi-test-plugin");
        assert!(
            discovered[0].tools.is_empty(),
            "test plugin declares no tools by design"
        );

        let id = manager
            .install(PluginSource::Path(source_dir))
            .await
            .unwrap();
        assert_eq!(id.0, "dimi-test-plugin");
        assert!(installed_dir.join("dimi-test-plugin/plugin.yaml").exists());

        let records = manager.list().await.unwrap();
        assert_eq!(records.len(), 1);
        assert_eq!(records[0].state, PluginState::Installed);

        manager.enable(id.clone()).await.unwrap();
        let records = manager.list().await.unwrap();
        assert_eq!(records[0].state, PluginState::Enabled);

        manager.disable(id.clone()).await.unwrap();
        let records = manager.list().await.unwrap();
        assert_eq!(records[0].state, PluginState::Disabled);

        manager.uninstall(id).await.unwrap();
        assert!(manager.list().await.unwrap().is_empty());

        tokio::fs::remove_dir_all(&plugins_root).await.ok();
    }
}
