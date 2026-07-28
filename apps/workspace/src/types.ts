export interface WorkspaceSummary {
  id: string;
  name: string;
  plugin: string | null;
}

export interface Workspace {
  id: string;
  name: string;
  repositories: string[];
  tools: string[];
  system_prompt: string;
  plugin: string | null;
}

export type ModelStatus = "available" | "downloading" | "validating" | "installed" | "removing";
export type InferenceBackend = "llama_cpp" | "mlx" | "onnx" | "candle";

export interface ModelInfo {
  id: string;
  name: string;
  backend: InferenceBackend;
  sha256: string;
  status: ModelStatus;
  size_bytes: number;
  installed_at: number | null;
}

export interface TelemetrySnapshot {
  ram_bytes: number;
  cpu_percent: number;
  cpu_temp_celsius: number | null;
  tokens_per_sec: number | null;
  job_queue_depth: number;
}

export interface ProcessMemory {
  pid: number;
  name: string;
  ram_bytes: number;
}

export interface ResourcePreflight {
  available_bytes: number;
  required_bytes: number;
  sufficient: boolean;
  top_consumers: ProcessMemory[];
}

export interface ResourcePreflightBlockedPayload extends ResourcePreflight {
  model_id: string;
}

export interface ModelDownloadProgressPayload {
  model_id: string;
  status: "downloading" | "validating" | "installed" | "loading" | "failed";
  bytes_completed?: number;
  total_bytes?: number;
  error?: string;
}

export interface RepositoryInfo {
  id: string;
  root: string;
}

export interface RepositoryIndexingEventPayload {
  repository_id: string;
}

export interface RepositoryIndexingFailedPayload extends RepositoryIndexingEventPayload {
  error: string;
}

export type DocumentStatus = "pending" | "parsed" | "indexed" | "failed";

export interface DocumentRow {
  id: string;
  source_path: string;
  status: DocumentStatus;
  imported_at: number;
  indexed_at: number | null;
}

export interface ConversationSummary {
  id: string;
  title: string | null;
  created_at: number;
  workspace_ids: string[];
  workspace_names: string[];
}

export type PluginState =
  | "discovered"
  | "validated"
  | "installed"
  | "enabled"
  | "disabled"
  | "failed"
  | "uninstalled";

export interface SuggestedFolder {
  path: string;
  recommended: boolean;
}

export interface PluginManifest {
  name: string;
  displayName: string;
  version: string;
  author: string | null;
  description: string | null;
  icon: string | null;
  permissions: {
    capabilities: string[];
    suggestedFolders: SuggestedFolder[];
  };
  knowledge: { name: string; description: string | null }[];
  tools: { id: string; entry: string }[];
  commands: { id: string; title: string; tool: string | null }[];
  pages: { id: string; title: string; entry: string }[];
}

export interface PluginRecord {
  id: string;
  manifest: PluginManifest;
  state: PluginState;
}

export interface MessageSource {
  name: string;
  path: string;
}

export interface PlainChatMessage {
  role: "system" | "user" | "assistant";
  content: string;
  sources?: MessageSource[];
}

export interface ToolConfirmationMessage {
  role: "tool";
  kind: "tool_confirmation";
  confirmationId: string;
  tool: string;
  summary: string;
  resolution: { approved: boolean } | null;
  expired?: boolean;
}

export type ChatMessage = PlainChatMessage | ToolConfirmationMessage;

export interface MessageRow {
  role: "system" | "user" | "assistant" | "tool";
  content: string;
  sources: string | null;
  created_at: number;
}

export type MemoryBudget = "Auto" | "2GB" | "3GB" | "4GB" | "6GB" | "8GB" | "12GB";

export interface SystemCheckResult {
  ram_tier: "Low" | "Mid" | "High";
  total_ram_bytes: number;
  cpu_count: number;
  arch: string;
  requires_confirmation: boolean;
}
