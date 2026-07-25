use crate::common::{not_implemented, PluginId, Result, Workspace, WorkspaceId};
use crate::common::{WorkspaceSpec, WorkspaceSummary};
use async_trait::async_trait;

#[async_trait]
pub trait WorkspaceService: Send + Sync {
    async fn create(&self, spec: WorkspaceSpec) -> Result<WorkspaceId>;
    async fn load(&self, id: WorkspaceId) -> Result<Workspace>;
    async fn update(&self, id: WorkspaceId, spec: WorkspaceSpec) -> Result<()>;
    async fn delete(&self, id: WorkspaceId) -> Result<()>;
    async fn list(&self) -> Result<Vec<WorkspaceSummary>>;
    async fn from_plugin(&self, plugin: PluginId) -> Result<WorkspaceId>;
}

pub struct StubWorkspaceService;

#[async_trait]
impl WorkspaceService for StubWorkspaceService {
    async fn create(&self, _spec: WorkspaceSpec) -> Result<WorkspaceId> {
        not_implemented("WorkspaceService::create")
    }
    async fn load(&self, _id: WorkspaceId) -> Result<Workspace> {
        not_implemented("WorkspaceService::load")
    }
    async fn update(&self, _id: WorkspaceId, _spec: WorkspaceSpec) -> Result<()> {
        not_implemented("WorkspaceService::update")
    }
    async fn delete(&self, _id: WorkspaceId) -> Result<()> {
        not_implemented("WorkspaceService::delete")
    }
    async fn list(&self) -> Result<Vec<WorkspaceSummary>> {
        not_implemented("WorkspaceService::list")
    }
    async fn from_plugin(&self, _plugin: PluginId) -> Result<WorkspaceId> {
        not_implemented("WorkspaceService::from_plugin")
    }
}

use crate::common::{DimiError, RepositoryConfig, RepositoryId, SqlValue};
use crate::connectors::RepositoryStore;
use crate::services::plugin_manager::PluginManager;
use std::sync::Arc;

pub struct SqliteWorkspaceService {
    storage: Arc<dyn crate::services::storage::StorageEngine>,
    repositories: Arc<RepositoryStore>,
    plugin_manager: Arc<dyn PluginManager>,
}

impl SqliteWorkspaceService {
    pub fn new(
        storage: Arc<dyn crate::services::storage::StorageEngine>,
        repositories: Arc<RepositoryStore>,
        plugin_manager: Arc<dyn PluginManager>,
    ) -> Self {
        Self {
            storage,
            repositories,
            plugin_manager,
        }
    }

    async fn write_associations(
        &self,
        tx: &mut Box<dyn crate::services::storage::StorageTransaction>,
        id: WorkspaceId,
        spec: &WorkspaceSpec,
    ) -> Result<()> {
        for repo in &spec.repositories {
            tx.execute(
                "INSERT INTO workspace_repositories (workspace_id, repository_id) VALUES (?1, ?2)",
                &[
                    SqlValue::Text(id.to_string()),
                    SqlValue::Text(repo.to_string()),
                ],
            )
            .await?;
        }
        for tool in &spec.tools {
            tx.execute(
                "INSERT INTO workspace_tools (workspace_id, tool_name) VALUES (?1, ?2)",
                &[SqlValue::Text(id.to_string()), SqlValue::Text(tool.clone())],
            )
            .await?;
        }
        Ok(())
    }
}

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

#[async_trait]
impl WorkspaceService for SqliteWorkspaceService {
    async fn create(&self, spec: WorkspaceSpec) -> Result<WorkspaceId> {
        let id = WorkspaceId::new();
        let mut tx = self.storage.transaction().await?;

        let insert = tx
            .execute(
                "INSERT INTO workspaces (id, name, system_prompt, owning_plugin, created_at) \
                 VALUES (?1, ?2, ?3, ?4, ?5)",
                &[
                    SqlValue::Text(id.to_string()),
                    SqlValue::Text(spec.name.clone()),
                    SqlValue::Text(spec.system_prompt.clone()),
                    match &spec.plugin {
                        Some(p) => SqlValue::Text(p.to_string()),
                        None => SqlValue::Null,
                    },
                    SqlValue::Integer(now_unix()),
                ],
            )
            .await;
        if let Err(e) = insert {
            tx.rollback().await.ok();
            return Err(e);
        }

        if let Err(e) = self.write_associations(&mut tx, id, &spec).await {
            tx.rollback().await.ok();
            return Err(e);
        }

        tx.commit().await?;
        Ok(id)
    }

    async fn load(&self, id: WorkspaceId) -> Result<Workspace> {
        let rows = self
            .storage
            .query(
                "SELECT name, system_prompt, owning_plugin FROM workspaces WHERE id = ?1",
                &[SqlValue::Text(id.to_string())],
            )
            .await?;
        let row = rows
            .into_iter()
            .next()
            .ok_or_else(|| DimiError::NotFound(format!("workspace {id}")))?;
        let get_text = |col: &str| match row.0.get(col) {
            Some(SqlValue::Text(s)) => Some(s.clone()),
            _ => None,
        };
        let name = get_text("name").unwrap_or_default();
        let system_prompt = get_text("system_prompt").unwrap_or_default();
        let plugin: Option<PluginId> = get_text("owning_plugin").and_then(|s| s.parse().ok());

        let repo_rows = self
            .storage
            .query(
                "SELECT repository_id FROM workspace_repositories WHERE workspace_id = ?1",
                &[SqlValue::Text(id.to_string())],
            )
            .await?;
        let repositories: Vec<RepositoryId> = repo_rows
            .iter()
            .filter_map(|r| match r.0.get("repository_id") {
                Some(SqlValue::Text(s)) => s.parse().ok(),
                _ => None,
            })
            .collect();

        let tool_rows = self
            .storage
            .query(
                "SELECT tool_name FROM workspace_tools WHERE workspace_id = ?1",
                &[SqlValue::Text(id.to_string())],
            )
            .await?;
        let tools: Vec<String> = tool_rows
            .iter()
            .filter_map(|r| match r.0.get("tool_name") {
                Some(SqlValue::Text(s)) => Some(s.clone()),
                _ => None,
            })
            .collect();

        Ok(Workspace {
            id,
            name,
            repositories,
            tools,
            system_prompt,
            plugin,
        })
    }

    async fn update(&self, id: WorkspaceId, spec: WorkspaceSpec) -> Result<()> {
        let mut tx = self.storage.transaction().await?;

        let steps = async {
            tx.execute(
                "UPDATE workspaces SET name = ?2, system_prompt = ?3 WHERE id = ?1",
                &[
                    SqlValue::Text(id.to_string()),
                    SqlValue::Text(spec.name.clone()),
                    SqlValue::Text(spec.system_prompt.clone()),
                ],
            )
            .await?;
            tx.execute(
                "DELETE FROM workspace_repositories WHERE workspace_id = ?1",
                &[SqlValue::Text(id.to_string())],
            )
            .await?;
            tx.execute(
                "DELETE FROM workspace_tools WHERE workspace_id = ?1",
                &[SqlValue::Text(id.to_string())],
            )
            .await
        }
        .await;
        if let Err(e) = steps {
            tx.rollback().await.ok();
            return Err(e);
        }

        if let Err(e) = self.write_associations(&mut tx, id, &spec).await {
            tx.rollback().await.ok();
            return Err(e);
        }

        tx.commit().await
    }

    async fn delete(&self, id: WorkspaceId) -> Result<()> {
        self.storage
            .query(
                "DELETE FROM workspaces WHERE id = ?1",
                &[SqlValue::Text(id.to_string())],
            )
            .await?;
        Ok(())
    }

    async fn list(&self) -> Result<Vec<WorkspaceSummary>> {
        let rows = self
            .storage
            .query("SELECT id, name, owning_plugin FROM workspaces", &[])
            .await?;
        rows.iter()
            .map(|row| {
                let get_text = |col: &str| match row.0.get(col) {
                    Some(SqlValue::Text(s)) => Some(s.clone()),
                    _ => None,
                };
                let id: WorkspaceId = get_text("id")
                    .ok_or_else(|| DimiError::Internal("workspace row missing id".into()))?
                    .parse()
                    .map_err(|e| DimiError::Internal(format!("bad workspace id: {e}")))?;
                let name = get_text("name").unwrap_or_default();
                let plugin = get_text("owning_plugin").and_then(|s| s.parse().ok());
                Ok(WorkspaceSummary { id, name, plugin })
            })
            .collect()
    }

    async fn from_plugin(&self, plugin: PluginId) -> Result<WorkspaceId> {
        let records = self.plugin_manager.list().await?;
        let manifest = records
            .into_iter()
            .find(|r| r.id == plugin)
            .map(|r| r.manifest)
            .ok_or_else(|| DimiError::NotFound(format!("plugin {plugin}")))?;

        let mut repository_ids = Vec::new();
        for folder in &manifest.permissions.suggested_folders {
            let repo = RepositoryConfig {
                id: RepositoryId::new(),
                kind: crate::common::ConnectorKind::Local,
                root: folder.path.clone(),
                credentials: None,
                owning_plugin: Some(plugin.clone()),
            };
            self.repositories.register(&repo).await?;
            repository_ids.push(repo.id);
        }

        let tools = manifest.tools.iter().map(|t| t.id.clone()).collect();
        let system_prompt = manifest
            .description
            .clone()
            .unwrap_or_else(|| format!("{} assistant", manifest.display_name));

        self.create(WorkspaceSpec {
            name: manifest.display_name,
            repositories: repository_ids,
            tools,
            system_prompt,
            plugin: Some(plugin),
        })
        .await
    }
}
