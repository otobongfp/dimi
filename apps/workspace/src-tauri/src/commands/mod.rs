use dimi_runtime::common::{
    ConversationId, ModelInfo, PluginId, PluginManifest, PluginRecord, RepositoryConfig,
    RepositoryId, TelemetrySnapshot, Workspace, WorkspaceId, WorkspaceSpec, WorkspaceSummary,
};
use dimi_runtime::kernel::hardware::ResourcePreflight;
use dimi_runtime::kernel::Runtime;
use dimi_runtime::services::model_manager::catalog;
use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::Arc;
use tauri::{AppHandle, Emitter, State};

type CmdResult<T> = Result<T, String>;

#[tauri::command]
pub async fn workspaces_create(
    runtime: State<'_, Arc<Runtime>>,
    name: String,
    repositories: Vec<String>,
    tools: Vec<String>,
    system_prompt: String,
) -> CmdResult<String> {
    let repositories = repositories
        .into_iter()
        .map(|s| s.parse::<RepositoryId>().map_err(|e| e.to_string()))
        .collect::<CmdResult<Vec<_>>>()?;
    let spec = WorkspaceSpec {
        name,
        repositories,
        tools,
        system_prompt,
        plugin: None,
    };
    runtime
        .workspaces_create(spec)
        .await
        .map(|id| id.to_string())
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn workspaces_list(runtime: State<'_, Arc<Runtime>>) -> CmdResult<Vec<WorkspaceSummary>> {
    runtime.workspaces_list().await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn workspaces_load(runtime: State<'_, Arc<Runtime>>, id: String) -> CmdResult<Workspace> {
    let id: WorkspaceId = id.parse().map_err(|e: uuid::Error| e.to_string())?;
    runtime.workspaces_load(id).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn workspaces_update(
    runtime: State<'_, Arc<Runtime>>,
    id: String,
    name: String,
    repositories: Vec<String>,
    tools: Vec<String>,
    system_prompt: String,
) -> CmdResult<()> {
    let id: WorkspaceId = id.parse().map_err(|e: uuid::Error| e.to_string())?;
    let repositories = repositories
        .into_iter()
        .map(|s| s.parse::<RepositoryId>().map_err(|e| e.to_string()))
        .collect::<CmdResult<Vec<_>>>()?;
    let spec = WorkspaceSpec {
        name,
        repositories,
        tools,
        system_prompt,
        plugin: None,
    };
    runtime
        .workspaces_update(id, spec)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn workspaces_delete(runtime: State<'_, Arc<Runtime>>, id: String) -> CmdResult<()> {
    let id: WorkspaceId = id.parse().map_err(|e: uuid::Error| e.to_string())?;
    runtime.workspaces_delete(id).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn repositories_add_folder(
    runtime: State<'_, Arc<Runtime>>,
    path: String,
) -> CmdResult<String> {
    runtime
        .inner()
        .clone()
        .repositories_add_folder(path)
        .await
        .map(|id| id.to_string())
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn repositories_reindex(
    runtime: State<'_, Arc<Runtime>>,
    repository_id: String,
) -> CmdResult<()> {
    let repository_id: RepositoryId = repository_id
        .parse()
        .map_err(|e: uuid::Error| e.to_string())?;
    runtime
        .repositories_reindex(repository_id)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn repositories_add_files(
    runtime: State<'_, Arc<Runtime>>,
    paths: Vec<String>,
) -> CmdResult<String> {
    runtime
        .inner()
        .clone()
        .repositories_add_files(paths)
        .await
        .map(|id| id.to_string())
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub fn repositories_indexing_status(runtime: State<'_, Arc<Runtime>>) -> CmdResult<Vec<String>> {
    Ok(runtime
        .repositories_indexing_status()
        .into_iter()
        .map(|id| id.to_string())
        .collect())
}

#[tauri::command]
pub async fn repositories_get(
    runtime: State<'_, Arc<Runtime>>,
    id: String,
) -> CmdResult<RepositoryConfig> {
    let id: RepositoryId = id.parse().map_err(|e: uuid::Error| e.to_string())?;
    runtime.repositories_get(id).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn documents_list(
    runtime: State<'_, Arc<Runtime>>,
    repository_id: String,
) -> CmdResult<Vec<serde_json::Value>> {
    let repository_id: RepositoryId = repository_id
        .parse()
        .map_err(|e: uuid::Error| e.to_string())?;
    runtime
        .documents_list(repository_id)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn conversations_create(
    runtime: State<'_, Arc<Runtime>>,
    workspace_ids: Vec<String>,
) -> CmdResult<String> {
    let workspace_ids = workspace_ids
        .into_iter()
        .map(|s| s.parse::<WorkspaceId>().map_err(|e| e.to_string()))
        .collect::<CmdResult<Vec<_>>>()?;
    runtime
        .conversations_create(workspace_ids)
        .await
        .map(|id| id.to_string())
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn conversations_list(
    runtime: State<'_, Arc<Runtime>>,
) -> CmdResult<Vec<serde_json::Value>> {
    runtime.conversations_list().await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn conversations_attach_workspace(
    runtime: State<'_, Arc<Runtime>>,
    conversation_id: String,
    workspace_id: String,
) -> CmdResult<()> {
    let conversation_id: ConversationId = conversation_id.parse().map_err(|e: uuid::Error| e.to_string())?;
    let workspace_id: WorkspaceId = workspace_id.parse().map_err(|e: uuid::Error| e.to_string())?;
    runtime
        .conversations_attach_workspace(conversation_id, workspace_id)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn conversations_delete(runtime: State<'_, Arc<Runtime>>, id: String) -> CmdResult<()> {
    let id: ConversationId = id.parse().map_err(|e: uuid::Error| e.to_string())?;
    runtime.conversations_delete(id).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn messages_list(
    runtime: State<'_, Arc<Runtime>>,
    conversation_id: String,
) -> CmdResult<Vec<serde_json::Value>> {
    let conversation_id: ConversationId = conversation_id
        .parse()
        .map_err(|e: uuid::Error| e.to_string())?;
    runtime
        .messages_list(conversation_id)
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn chat_send(
    app: AppHandle,
    runtime: State<'_, Arc<Runtime>>,
    conversation_id: String,
    message: String,
) -> CmdResult<String> {
    let conversation_id: ConversationId = conversation_id
        .parse()
        .map_err(|e: uuid::Error| e.to_string())?;

    let mut stream = runtime
        .start_chat_turn(conversation_id, message)
        .await
        .map_err(|e| e.to_string())?;

    let mut full_response = String::new();
    while let Some(piece) = tokio_stream::StreamExt::next(&mut stream).await {
        let piece = piece.map_err(|e| e.to_string())?;
        full_response.push_str(&piece);
        let _ = app.emit("inference_token", &piece);
    }

    Ok(full_response)
}

#[tauri::command]
pub async fn plugins_discover(runtime: State<'_, Arc<Runtime>>) -> CmdResult<Vec<PluginManifest>> {
    runtime.plugins_discover().await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn plugins_list(runtime: State<'_, Arc<Runtime>>) -> CmdResult<Vec<PluginRecord>> {
    runtime.plugins_list().await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn plugins_install(runtime: State<'_, Arc<Runtime>>, path: String) -> CmdResult<String> {
    runtime
        .plugins_install(PathBuf::from(path))
        .await
        .map(|id| id.0)
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn plugins_enable(runtime: State<'_, Arc<Runtime>>, id: String) -> CmdResult<()> {
    runtime
        .plugins_enable(PluginId::new(id))
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn plugins_disable(runtime: State<'_, Arc<Runtime>>, id: String) -> CmdResult<()> {
    runtime
        .plugins_disable(PluginId::new(id))
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn models_list_installed(runtime: State<'_, Arc<Runtime>>) -> CmdResult<Vec<ModelInfo>> {
    runtime.models_list_installed().await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn models_list_available(runtime: State<'_, Arc<Runtime>>) -> CmdResult<Vec<ModelInfo>> {
    runtime.models_list_available().await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn models_download(runtime: State<'_, Arc<Runtime>>, model_id: String) -> CmdResult<()> {
    runtime.models_download(&model_id).await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn models_active(runtime: State<'_, Arc<Runtime>>) -> CmdResult<ModelInfo> {
    runtime.models_active().await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn health_snapshot(runtime: State<'_, Arc<Runtime>>) -> CmdResult<TelemetrySnapshot> {
    runtime.health_snapshot().await.map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn runtime_health(
    runtime: State<'_, Arc<Runtime>>,
) -> CmdResult<HashMap<String, String>> {
    Ok(runtime.runtime_health())
}

#[tauri::command]
pub async fn settings_get_memory_budget(runtime: State<'_, Arc<Runtime>>) -> CmdResult<String> {
    runtime
        .settings_get_memory_budget()
        .await
        .map_err(|e| e.to_string())
}

#[tauri::command]
pub async fn settings_set_memory_budget(
    runtime: State<'_, Arc<Runtime>>,
    budget: String,
) -> CmdResult<()> {
    runtime
        .settings_set_memory_budget(budget)
        .await
        .map_err(|e| e.to_string())
}

// Pure function of the static catalog + hardware module — doesn't touch
// `Runtime` state at all, so it stays here rather than becoming a method.
fn catalog_entry(model_id: &str) -> CmdResult<dimi_runtime::services::model_manager::ModelCatalogEntry> {
    catalog()
        .into_iter()
        .find(|entry| entry.id == model_id)
        .ok_or_else(|| format!("unknown model id: {model_id}"))
}

#[tauri::command]
pub async fn resource_preflight_check(model_id: String) -> CmdResult<ResourcePreflight> {
    let entry = catalog_entry(&model_id)?;
    Ok(dimi_runtime::kernel::hardware::resource_preflight(
        entry.size_bytes,
        entry.kv_bytes_per_token,
    ))
}

#[tauri::command]
pub async fn resource_preflight_load_model(
    runtime: State<'_, Arc<Runtime>>,
    model_id: String,
    force: bool,
) -> CmdResult<()> {
    let entry = catalog_entry(&model_id)?;
    runtime.inner().clone().load_model(entry.id, force);
    Ok(())
}

#[derive(Debug, Clone, serde::Serialize)]
pub struct SystemCheckResult {
    pub ram_tier: String,
    pub total_ram_bytes: u64,
    pub cpu_count: usize,
    pub arch: String,
    pub requires_confirmation: bool,
}

pub fn compute_system_check() -> SystemCheckResult {
    use dimi_runtime::kernel::hardware::RamTier;
    let tier = dimi_runtime::kernel::hardware::ram_tier();
    SystemCheckResult {
        ram_tier: format!("{tier:?}"),
        total_ram_bytes: dimi_runtime::kernel::hardware::total_ram_bytes(),
        cpu_count: std::thread::available_parallelism()
            .map(|n| n.get())
            .unwrap_or(1),
        arch: std::env::consts::ARCH.to_string(),
        requires_confirmation: matches!(tier, RamTier::Low | RamTier::Mid),
    }
}

pub struct SystemCheckState {
    pub result: SystemCheckResult,
    pub continue_notify: Arc<tokio::sync::Notify>,
}

#[tauri::command]
pub fn system_check_status(state: State<'_, Arc<SystemCheckState>>) -> SystemCheckResult {
    state.result.clone()
}

#[tauri::command]
pub fn system_check_continue(state: State<'_, Arc<SystemCheckState>>) {
    state.continue_notify.notify_one();
}
