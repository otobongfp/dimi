pub mod local;
pub mod sqlite;

use crate::common::{ConnectorEntry, ConnectorKind, EntryMetadata, Result, WatchHandle};
use async_trait::async_trait;

#[async_trait]
pub trait Connector: Send + Sync {
    fn kind(&self) -> ConnectorKind;
    async fn list(&self, path: &str) -> Result<Vec<ConnectorEntry>>;
    async fn read(&self, path: &str) -> Result<Vec<u8>>;
    async fn write(&self, path: &str, data: &[u8]) -> Result<()>;
    async fn watch(&self, path: &str) -> Result<WatchHandle>;
    async fn metadata(&self, path: &str) -> Result<EntryMetadata>;
}

use crate::common::{PluginId, RepositoryConfig, RepositoryId, SqlValue};
use crate::services::storage::StorageEngine;
use std::sync::Arc;

/// Fixed, well-known ID for the repository auto-registered at boot and
/// rooted at the user's home directory — file tools work against it without
/// requiring a library to be created first. Deterministic (not `new_v4()`)
/// so re-registering on every boot is an upsert onto the same row, not a
/// pile of duplicates.
pub fn computer_repository_id() -> RepositoryId {
    RepositoryId(uuid::Uuid::from_u128(1))
}

pub struct RepositoryStore {
    storage: Arc<dyn StorageEngine>,
}

impl RepositoryStore {
    pub fn new(storage: Arc<dyn StorageEngine>) -> Self {
        Self { storage }
    }

    pub async fn register(&self, config: &RepositoryConfig) -> Result<()> {
        self.storage
            .query(
                "INSERT INTO repositories (id, kind, root, owning_plugin, created_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5) \
                 ON CONFLICT(id) DO UPDATE SET kind = excluded.kind, root = excluded.root",
                &[
                    SqlValue::Text(config.id.to_string()),
                    SqlValue::Text(connector_kind_to_str(config.kind).to_string()),
                    SqlValue::Text(config.root.clone()),
                    match &config.owning_plugin {
                        Some(plugin) => SqlValue::Text(plugin.to_string()),
                        None => SqlValue::Null,
                    },
                    SqlValue::Integer(now_unix()),
                ],
            )
            .await?;
        Ok(())
    }

    pub async fn get(&self, id: RepositoryId) -> Result<RepositoryConfig> {
        let rows = self
            .storage
            .query(
                "SELECT id, kind, root, owning_plugin FROM repositories WHERE id = ?1",
                &[SqlValue::Text(id.to_string())],
            )
            .await?;
        let row = rows
            .into_iter()
            .next()
            .ok_or_else(|| crate::common::DimiError::NotFound(format!("repository {id}")))?;
        row_to_repository_config(&row)
    }

    pub async fn list(&self) -> Result<Vec<RepositoryConfig>> {
        let rows = self
            .storage
            .query(
                "SELECT id, kind, root, owning_plugin FROM repositories",
                &[],
            )
            .await?;
        rows.iter().map(row_to_repository_config).collect()
    }

    pub async fn find_by_path_prefix(
        &self,
        path: &std::path::Path,
    ) -> Result<Option<RepositoryConfig>> {
        let repos = self.list().await?;
        Ok(repos.into_iter().find(|r| path.starts_with(&r.root)))
    }
}

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

fn connector_kind_to_str(kind: ConnectorKind) -> &'static str {
    match kind {
        ConnectorKind::Local => "local",
        ConnectorKind::Sqlite => "sqlite",
        ConnectorKind::Git => "git",
        ConnectorKind::SharePoint => "sharepoint",
        ConnectorKind::GDrive => "gdrive",
        ConnectorKind::OneDrive => "onedrive",
        ConnectorKind::S3 => "s3",
        ConnectorKind::Enterprise => "enterprise",
    }
}

fn connector_kind_from_str(s: &str) -> Result<ConnectorKind> {
    Ok(match s {
        "local" => ConnectorKind::Local,
        "sqlite" => ConnectorKind::Sqlite,
        "git" => ConnectorKind::Git,
        "sharepoint" => ConnectorKind::SharePoint,
        "gdrive" => ConnectorKind::GDrive,
        "onedrive" => ConnectorKind::OneDrive,
        "s3" => ConnectorKind::S3,
        "enterprise" => ConnectorKind::Enterprise,
        other => {
            return Err(crate::common::DimiError::Internal(format!(
                "unknown connector kind in database: {other}"
            )))
        }
    })
}

fn row_to_repository_config(row: &crate::common::Row) -> Result<RepositoryConfig> {
    let get_text = |col: &str| match row.0.get(col) {
        Some(SqlValue::Text(s)) => Some(s.clone()),
        _ => None,
    };
    let id: RepositoryId = get_text("id")
        .ok_or_else(|| crate::common::DimiError::Internal("repository row missing id".into()))?
        .parse()
        .map_err(|e| crate::common::DimiError::Internal(format!("bad repository id: {e}")))?;
    let kind = connector_kind_from_str(&get_text("kind").ok_or_else(|| {
        crate::common::DimiError::Internal("repository row missing kind".into())
    })?)?;
    let root = get_text("root")
        .ok_or_else(|| crate::common::DimiError::Internal("repository row missing root".into()))?;
    let owning_plugin: Option<PluginId> = get_text("owning_plugin").and_then(|s| s.parse().ok());

    Ok(RepositoryConfig {
        id,
        kind,
        root,
        credentials: None,
        owning_plugin,
    })
}
