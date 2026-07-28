//! The comms surface between `Runtime` and whatever's driving it. Each
//! method here is the business logic that used to be inlined inside a
//! `#[tauri::command]` body, reaching into `ServiceContainer` directly.
//! `apps/workspace/src-tauri`'s commands now just parse their raw IPC
//! arguments into these methods' typed parameters and call them.
//!
//! Not moved here: `chat_send`'s per-token event emission (Tauri-specific —
//! see `Runtime::start_chat_turn`, which hands back the raw stream instead),
//! `resource_preflight_check` (a pure function of the static catalog +
//! hardware module, doesn't touch `self` at all), and
//! `system_check_status`/`system_check_continue` (operate on
//! `SystemCheckState`, which is deliberately constructed *before*
//! `Runtime::boot()` starts — outside this struct's own lifecycle by design).

use crate::common::{
    ConnectorKind, ConversationId, DimiError, MessageId, ModelInfo, PluginId, PluginManifest,
    PluginRecord, PluginSource, RepositoryConfig, RepositoryId, Result, Row, SqlValue,
    TelemetrySnapshot, TokenStream, Workspace, WorkspaceId, WorkspaceSpec, WorkspaceSummary,
};
use crate::kernel::events::topics;
use crate::kernel::Runtime;
use crate::pipelines::inference_pipeline::{run_chat_turn, ChatTurnDeps};
use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::Arc;

fn now_unix() -> i64 {
    std::time::SystemTime::now()
        .duration_since(std::time::UNIX_EPOCH)
        .unwrap_or_default()
        .as_secs() as i64
}

fn row_to_json(row: Row) -> serde_json::Value {
    let mut map = serde_json::Map::new();
    for (key, value) in row.0 {
        let json_value = match value {
            SqlValue::Null => serde_json::Value::Null,
            SqlValue::Integer(i) => serde_json::Value::from(i),
            SqlValue::Real(f) => serde_json::Value::from(f),
            SqlValue::Text(s) => serde_json::Value::from(s),
            SqlValue::Blob(b) => serde_json::Value::from(b),
        };
        map.insert(key, json_value);
    }
    serde_json::Value::Object(map)
}

fn unique_dest(dir: &Path, name: &std::ffi::OsStr) -> PathBuf {
    let candidate = dir.join(name);
    if !candidate.exists() {
        return candidate;
    }

    let name = Path::new(name);
    let stem = name
        .file_stem()
        .map(|s| s.to_string_lossy().into_owned())
        .unwrap_or_default();
    let extension = name.extension().map(|e| e.to_string_lossy().into_owned());

    for n in 1.. {
        let candidate_name = match &extension {
            Some(ext) => format!("{stem} ({n}).{ext}"),
            None => format!("{stem} ({n})"),
        };
        let candidate = dir.join(candidate_name);
        if !candidate.exists() {
            return candidate;
        }
    }
    unreachable!()
}

impl Runtime {
    pub async fn workspaces_create(&self, spec: WorkspaceSpec) -> Result<WorkspaceId> {
        self.container.workspace()?.create(spec).await
    }

    pub async fn workspaces_list(&self) -> Result<Vec<WorkspaceSummary>> {
        self.container.workspace()?.list().await
    }

    pub async fn workspaces_load(&self, id: WorkspaceId) -> Result<Workspace> {
        self.container.workspace()?.load(id).await
    }

    pub async fn workspaces_update(&self, id: WorkspaceId, spec: WorkspaceSpec) -> Result<()> {
        self.container.workspace()?.update(id, spec).await
    }

    pub async fn workspaces_delete(&self, id: WorkspaceId) -> Result<()> {
        self.container.workspace()?.delete(id).await
    }

    /// Runs `build_index` for a repository, tracking it in `indexing_repos`
    /// and publishing start/finish events around the run so the UI can show
    /// progress instead of the user having to guess. If this repository is
    /// already being indexed (e.g. the background pass from `add_folder`
    /// hasn't finished yet), this is a no-op that returns immediately rather
    /// than starting a second concurrent pass over the same files.
    async fn run_index_and_notify(&self, repository_id: RepositoryId, root: PathBuf) -> Result<()> {
        {
            let mut indexing = self.indexing_repos.lock().unwrap();
            if !indexing.insert(repository_id) {
                return Ok(());
            }
        }
        self.events.publish(
            topics::REPOSITORY_INDEXING_STARTED,
            serde_json::json!({ "repository_id": repository_id.to_string() }),
        );

        let result = self.import_pipeline.build_index(&root).await;

        self.indexing_repos.lock().unwrap().remove(&repository_id);

        match &result {
            Ok(()) => self.events.publish(
                topics::REPOSITORY_INDEXED,
                serde_json::json!({ "repository_id": repository_id.to_string() }),
            ),
            Err(e) => {
                tracing::warn!(%repository_id, path = %root.display(), error = %e, "index build failed");
                self.events.publish(
                    topics::REPOSITORY_INDEXING_FAILED,
                    serde_json::json!({ "repository_id": repository_id.to_string(), "error": e.to_string() }),
                );
            }
        }

        result
    }

    /// Repository ids currently being indexed, so the UI can seed its
    /// "indexing" state on mount instead of only reacting to events.
    pub fn repositories_indexing_status(&self) -> Vec<RepositoryId> {
        self.indexing_repos.lock().unwrap().iter().copied().collect()
    }

    pub async fn repositories_add_folder(self: &Arc<Self>, path: String) -> Result<RepositoryId> {
        let root = PathBuf::from(&path);
        let repository = RepositoryConfig {
            id: RepositoryId::new(),
            kind: ConnectorKind::Local,
            root: path,
            credentials: None,
            owning_plugin: None,
        };
        self.repositories.register(&repository).await?;

        let filesystem = self.container.filesystem()?;
        filesystem.watch(&root).await?;

        let runtime = self.clone();
        let repository_id = repository.id;
        tokio::spawn(async move {
            let _ = runtime.run_index_and_notify(repository_id, root).await;
        });

        Ok(repository.id)
    }

    pub async fn repositories_add_files(self: &Arc<Self>, paths: Vec<String>) -> Result<RepositoryId> {
        let id = RepositoryId::new();
        let short_id: String = id.to_string().chars().take(8).collect();
        let holding_dir = self
            .config
            .attached_files_dir()
            .join(id.to_string())
            .join(format!("Attached Files ({short_id})"));
        std::fs::create_dir_all(&holding_dir).map_err(DimiError::Io)?;

        for path in &paths {
            let source = PathBuf::from(path);
            let Some(name) = source.file_name() else {
                continue;
            };
            let dest = unique_dest(&holding_dir, name);
            std::os::unix::fs::symlink(&source, &dest)
                .map_err(|e| DimiError::Internal(e.to_string()))?;
        }

        let repository = RepositoryConfig {
            id,
            kind: ConnectorKind::Local,
            root: holding_dir.to_string_lossy().into_owned(),
            credentials: None,
            owning_plugin: None,
        };
        self.repositories.register(&repository).await?;

        let filesystem = self.container.filesystem()?;
        filesystem.watch(&holding_dir).await?;

        let runtime = self.clone();
        let repository_id = repository.id;
        tokio::spawn(async move {
            let _ = runtime.run_index_and_notify(repository_id, holding_dir).await;
        });

        Ok(repository.id)
    }

    pub async fn repositories_reindex(&self, repository_id: RepositoryId) -> Result<()> {
        let repository = self.repositories.get(repository_id).await?;
        let root = PathBuf::from(&repository.root);
        self.run_index_and_notify(repository_id, root).await
    }

    pub async fn repositories_get(&self, id: RepositoryId) -> Result<RepositoryConfig> {
        self.repositories.get(id).await
    }

    pub async fn documents_list(&self, repository_id: RepositoryId) -> Result<Vec<serde_json::Value>> {
        let knowledge = self.container.knowledge()?;
        let nodes = knowledge.list_indexed(repository_id).await?;

        Ok(nodes
            .into_iter()
            .filter(|n| !n.is_directory)
            .map(|n| {
                serde_json::json!({
                    "id": n.id,
                    "source_path": n.path,
                    "status": "indexed",
                    "imported_at": n.modified,
                    "indexed_at": n.modified,
                })
            })
            .collect())
    }

    pub async fn conversations_create(&self, workspace_ids: Vec<WorkspaceId>) -> Result<ConversationId> {
        let storage = self.container.storage()?;
        let id = ConversationId::new();
        let mut tx = storage.transaction().await?;

        if let Err(e) = tx
            .execute(
                "INSERT INTO conversations (id, title, created_at) VALUES (?1, NULL, ?2)",
                &[SqlValue::Text(id.to_string()), SqlValue::Integer(now_unix())],
            )
            .await
        {
            tx.rollback().await.ok();
            return Err(e);
        }

        for workspace_id in &workspace_ids {
            if let Err(e) = tx
                .execute(
                    "INSERT INTO conversation_workspaces (conversation_id, workspace_id) VALUES (?1, ?2)",
                    &[
                        SqlValue::Text(id.to_string()),
                        SqlValue::Text(workspace_id.to_string()),
                    ],
                )
                .await
            {
                tx.rollback().await.ok();
                return Err(e);
            }
        }

        tx.commit().await?;
        Ok(id)
    }

    // Turns a "general" conversation into one grounded in a library, without
    // losing its history — the model can point the user at this via
    // `list_libraries`, but the attach itself is a deliberate user action
    // from the UI, not something the model triggers on its own.
    pub async fn conversations_attach_workspace(
        &self,
        conversation_id: ConversationId,
        workspace_id: WorkspaceId,
    ) -> Result<()> {
        let storage = self.container.storage()?;
        storage
            .query(
                "INSERT OR IGNORE INTO conversation_workspaces (conversation_id, workspace_id) VALUES (?1, ?2)",
                &[
                    SqlValue::Text(conversation_id.to_string()),
                    SqlValue::Text(workspace_id.to_string()),
                ],
            )
            .await?;
        Ok(())
    }

    pub async fn conversations_list(&self) -> Result<Vec<serde_json::Value>> {
        let storage = self.container.storage()?;
        let conversations = storage
            .query(
                "SELECT id, title, created_at FROM conversations ORDER BY created_at DESC LIMIT 20",
                &[],
            )
            .await?;

        let attachment_rows = storage
            .query(
                "SELECT cw.conversation_id, w.id AS workspace_id, w.name AS workspace_name \
                 FROM conversation_workspaces cw JOIN workspaces w ON w.id = cw.workspace_id \
                 ORDER BY w.name",
                &[],
            )
            .await?;

        let mut attachments: HashMap<String, (Vec<String>, Vec<String>)> = HashMap::new();
        for row in attachment_rows {
            let Some(SqlValue::Text(conversation_id)) = row.0.get("conversation_id") else {
                continue;
            };
            let Some(SqlValue::Text(workspace_id)) = row.0.get("workspace_id") else {
                continue;
            };
            let Some(SqlValue::Text(workspace_name)) = row.0.get("workspace_name") else {
                continue;
            };
            let entry = attachments.entry(conversation_id.clone()).or_default();
            entry.0.push(workspace_id.clone());
            entry.1.push(workspace_name.clone());
        }

        Ok(conversations
            .into_iter()
            .filter_map(|row| {
                #[allow(clippy::needless_borrowed_reference)]
                let Some(&SqlValue::Text(ref id)) = row.0.get("id") else {
                    return None;
                };
                let title = match row.0.get("title") {
                    Some(SqlValue::Text(s)) => serde_json::Value::from(s.clone()),
                    _ => serde_json::Value::Null,
                };
                let created_at = match row.0.get("created_at") {
                    Some(&SqlValue::Integer(i)) => serde_json::Value::from(i),
                    _ => serde_json::Value::Null,
                };
                let (workspace_ids, workspace_names) = attachments.get(id).cloned().unwrap_or_default();
                let mut map = serde_json::Map::new();
                map.insert("id".into(), serde_json::Value::from(id.clone()));
                map.insert("title".into(), title);
                map.insert("created_at".into(), created_at);
                map.insert("workspace_ids".into(), serde_json::Value::from(workspace_ids));
                map.insert("workspace_names".into(), serde_json::Value::from(workspace_names));
                Some(serde_json::Value::Object(map))
            })
            .collect())
    }

    pub async fn conversations_delete(&self, id: ConversationId) -> Result<()> {
        let storage = self.container.storage()?;
        storage
            .query(
                "DELETE FROM conversations WHERE id = ?1",
                &[SqlValue::Text(id.to_string())],
            )
            .await?;
        Ok(())
    }

    pub async fn messages_list(&self, conversation_id: ConversationId) -> Result<Vec<serde_json::Value>> {
        let storage = self.container.storage()?;
        let rows = storage
            .query(
                "SELECT role, content, sources, created_at FROM messages WHERE conversation_id = ?1 AND visible = 1 ORDER BY created_at ASC",
                &[SqlValue::Text(conversation_id.to_string())],
            )
            .await?;
        Ok(rows.into_iter().map(row_to_json).collect())
    }

    /// Persists the user's message and starts the chat-turn loop, handing
    /// back the raw stream. Per-token delivery to a UI (Tauri `emit`, or
    /// anything else) is the caller's concern, not the runtime's.
    pub async fn start_chat_turn(&self, conversation_id: ConversationId, message: String) -> Result<TokenStream> {
        let storage = self.container.storage()?;
        storage
            .query(
                "INSERT INTO messages (id, conversation_id, role, content, created_at) VALUES (?1, ?2, 'user', ?3, ?4)",
                &[
                    SqlValue::Text(MessageId::new().to_string()),
                    SqlValue::Text(conversation_id.to_string()),
                    SqlValue::Text(message.clone()),
                    SqlValue::Integer(now_unix()),
                ],
            )
            .await?;

        let deps = ChatTurnDeps {
            context: self.container.context()?,
            inference: self.container.inference()?,
            tools: self.container.tool()?,
            telemetry: self.container.telemetry()?,
            storage,
            events: self.events.clone(),
            scheduler: self.scheduler.clone(),
            confirmations: self.pending_confirmations.clone(),
        };
        Ok(run_chat_turn(deps, conversation_id, message))
    }

    /// Resolves a pending destructive-tool-call confirmation raised mid-turn
    /// by `drive_chat_turn`. Errors if there's no live waiter for `id`
    /// (already resolved, timed out, or from a previous app session) —
    /// mirrors the unknown-tool error shape from `ToolEngine::invoke`.
    pub fn respond_to_tool_confirmation(&self, id: uuid::Uuid, approved: bool) -> Result<()> {
        if self.pending_confirmations.resolve(id, approved) {
            Ok(())
        } else {
            Err(DimiError::NotFound(format!("pending confirmation: {id}")))
        }
    }

    pub async fn plugins_discover(&self) -> Result<Vec<PluginManifest>> {
        self.container.plugin_manager()?.discover().await
    }

    pub async fn plugins_list(&self) -> Result<Vec<PluginRecord>> {
        self.container.plugin_manager()?.list().await
    }

    pub async fn plugins_install(&self, path: PathBuf) -> Result<PluginId> {
        self.container
            .plugin_manager()?
            .install(PluginSource::Path(path))
            .await
    }

    pub async fn plugins_enable(&self, id: PluginId) -> Result<()> {
        self.container.plugin_manager()?.enable(id).await
    }

    pub async fn plugins_disable(&self, id: PluginId) -> Result<()> {
        self.container.plugin_manager()?.disable(id).await
    }

    pub async fn models_list_installed(&self) -> Result<Vec<ModelInfo>> {
        self.container.model_manager()?.list_installed().await
    }

    /// The catalog (today: Qwen3-0.6B and Qwen3-1.7B), each entry marked
    /// `Available` regardless of whether it's actually installed yet —
    /// callers cross-reference against `models_list_installed` for that.
    pub async fn models_list_available(&self) -> Result<Vec<ModelInfo>> {
        self.container.model_manager()?.list_available().await
    }

    /// Kicks off a background download; progress arrives via the
    /// `model:download:progress` event, keyed by `model_id`.
    pub async fn models_download(&self, model_id: &str) -> Result<()> {
        self.container.model_manager()?.download(model_id).await?;
        Ok(())
    }

    pub async fn models_active(&self) -> Result<ModelInfo> {
        self.container.model_manager()?.active().await
    }

    pub async fn health_snapshot(&self) -> Result<TelemetrySnapshot> {
        self.container.telemetry()?.snapshot().await
    }

    pub fn runtime_health(&self) -> HashMap<String, String> {
        self.lifecycle
            .all()
            .into_iter()
            .map(|(name, state)| (name, format!("{state:?}")))
            .collect()
    }

    pub async fn settings_get_memory_budget(&self) -> Result<String> {
        let storage = self.container.storage()?;
        match storage.get("config", "memory_budget").await? {
            Some(val) => Ok(String::from_utf8_lossy(&val).into_owned()),
            None => Ok("Auto".to_string()),
        }
    }

    pub async fn settings_set_memory_budget(&self, budget: String) -> Result<()> {
        let storage = self.container.storage()?;
        storage.put("config", "memory_budget", budget.as_bytes()).await?;
        Ok(())
    }
}
