use super::errors::Result;
use serde::{Deserialize, Serialize};
use std::collections::HashMap;
use std::fmt;
use std::path::PathBuf;
use uuid::Uuid;

macro_rules! id_type {
    ($name:ident) => {
        #[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
        pub struct $name(pub Uuid);

        impl $name {
            pub fn new() -> Self {
                Self(Uuid::new_v4())
            }
        }

        impl Default for $name {
            fn default() -> Self {
                Self::new()
            }
        }

        impl fmt::Display for $name {
            fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
                write!(f, "{}", self.0)
            }
        }

        impl std::str::FromStr for $name {
            type Err = uuid::Error;
            fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
                Ok(Self(Uuid::parse_str(s)?))
            }
        }
    };
}

id_type!(WorkspaceId);
id_type!(RepositoryId);
id_type!(DocumentId);
id_type!(ConversationId);
id_type!(MessageId);
id_type!(JobId);
id_type!(CredentialId);

#[derive(Debug, Clone, PartialEq, Eq, Hash, Serialize, Deserialize)]
pub struct PluginId(pub String);

impl PluginId {
    pub fn new(name: impl Into<String>) -> Self {
        Self(name.into())
    }
}

impl fmt::Display for PluginId {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}", self.0)
    }
}

impl std::str::FromStr for PluginId {
    type Err = std::convert::Infallible;
    fn from_str(s: &str) -> std::result::Result<Self, Self::Err> {
        Ok(Self(s.to_string()))
    }
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Chunk {
    pub id: String,
    pub document_id: DocumentId,
    pub ordinal: i64,
    pub text: String,
    pub token_count: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Embedding {
    pub chunk_id: String,
    pub vector: Vec<f32>,
}

pub type TokenStream = std::pin::Pin<Box<dyn futures_core::Stream<Item = Result<String>> + Send>>;

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SearchResult {
    pub chunk: Chunk,
    pub score: f32,
    pub document_id: DocumentId,
    pub source_path: String,
    pub repository: RepositoryId,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ParsedDocument {
    pub text: String,
    pub structure: serde_json::Value,
    pub metadata: serde_json::Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum DocumentStatus {
    Pending,
    Parsed,
    Indexed,
    Failed,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DirEntry {
    pub path: PathBuf,
    pub is_dir: bool,
    pub size_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct EntryMetadata {
    pub size_bytes: u64,
    pub modified_at: i64,
    pub mime_type: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct WatchHandle(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ConnectorKind {
    Local,
    Sqlite,
    Git,
    SharePoint,
    GDrive,
    OneDrive,
    S3,
    Enterprise,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ConnectorEntry {
    pub path: String,
    pub is_dir: bool,
    pub size_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct RepositoryConfig {
    pub id: RepositoryId,
    pub kind: ConnectorKind,
    pub root: String,
    pub credentials: Option<CredentialId>,
    pub owning_plugin: Option<PluginId>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct IndexedNode {
    pub id: String,
    pub path: String,
    pub is_directory: bool,
    pub parent_id: Option<String>,
    pub modified: i64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceSpec {
    pub name: String,
    pub repositories: Vec<RepositoryId>,
    pub tools: Vec<String>,
    pub system_prompt: String,
    pub plugin: Option<PluginId>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Workspace {
    pub id: WorkspaceId,
    pub name: String,
    pub repositories: Vec<RepositoryId>,
    pub tools: Vec<String>,
    pub system_prompt: String,
    pub plugin: Option<PluginId>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct WorkspaceSummary {
    pub id: WorkspaceId,
    pub name: String,
    pub plugin: Option<PluginId>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ModelStatus {
    Available,
    Downloading,
    Validating,
    Installed,
    Removing,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InferenceBackend {
    LlamaCpp,
    Mlx,
    Onnx,
    Candle,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ModelInfo {
    pub id: String,
    pub name: String,
    pub backend: InferenceBackend,
    pub sha256: String,
    pub status: ModelStatus,
    pub size_bytes: u64,
    pub installed_at: Option<i64>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct DownloadHandle {
    pub model_id: String,
    pub job_id: JobId,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct Token(pub i32);

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ChatMessage {
    pub role: String,
    pub content: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PromptContext {
    pub messages: Vec<ChatMessage>,
    pub tools: Vec<ToolSchema>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ContextRequest {
    pub query: String,
    pub conversation_id: ConversationId,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolSchema {
    pub name: String,
    pub description: String,
    pub parameters: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ToolResult {
    pub content: serde_json::Value,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum JobStatus {
    Queued,
    Running,
    Completed,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum ResourceClass {
    Cpu,
    Gpu,
    Io,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Job {
    pub kind: String,
    pub priority: i32,
    pub resource_class: ResourceClass,
    pub payload: serde_json::Value,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct TelemetrySnapshot {
    pub ram_bytes: u64,
    pub cpu_percent: f32,
    pub cpu_temp_celsius: Option<f32>,
    pub tokens_per_sec: Option<f32>,
    pub job_queue_depth: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SystemMemory {
    pub total_bytes: u64,
    pub available_bytes: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct ProcessMemory {
    pub pid: u32,
    pub name: String,
    pub ram_bytes: u64,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub enum SqlValue {
    Null,
    Integer(i64),
    Real(f64),
    Text(String),
    Blob(Vec<u8>),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct Row(pub HashMap<String, SqlValue>);

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum PluginState {
    Discovered,
    Validated,
    Installed,
    Enabled,
    Disabled,
    Failed,
    Uninstalled,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginManifest {
    pub name: String,
    #[serde(rename = "displayName")]
    pub display_name: String,
    pub version: String,
    pub author: Option<String>,
    pub description: Option<String>,
    pub icon: Option<String>,
    pub permissions: PluginPermissions,
    #[serde(default)]
    pub knowledge: Vec<PluginKnowledge>,
    #[serde(default)]
    pub tools: Vec<PluginTool>,
    #[serde(default)]
    pub commands: Vec<PluginCommand>,
    #[serde(default)]
    pub pages: Vec<PluginPage>,
    pub lifecycle: Option<PluginLifecycle>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginPermissions {
    pub capabilities: Vec<String>,
    #[serde(rename = "suggestedFolders", default)]
    pub suggested_folders: Vec<SuggestedFolder>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct SuggestedFolder {
    pub path: String,
    #[serde(default)]
    pub recommended: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginKnowledge {
    pub name: String,
    pub description: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginTool {
    pub id: String,
    pub entry: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginCommand {
    pub id: String,
    pub title: String,
    pub tool: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginPage {
    pub id: String,
    pub title: String,
    pub entry: String,
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginLifecycle {
    #[serde(rename = "onInstall")]
    pub on_install: Option<String>,
    #[serde(rename = "onEnable")]
    pub on_enable: Option<String>,
    #[serde(rename = "onDisable")]
    pub on_disable: Option<String>,
    #[serde(rename = "onUninstall")]
    pub on_uninstall: Option<String>,
}

#[derive(Debug, Clone)]
pub enum PluginSource {
    Path(PathBuf),
}

#[derive(Debug, Clone, Serialize, Deserialize)]
pub struct PluginRecord {
    pub id: PluginId,
    pub manifest: PluginManifest,
    pub state: PluginState,
}
