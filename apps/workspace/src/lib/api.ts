import { invoke } from "@tauri-apps/api/core";
import { listen, type UnlistenFn } from "@tauri-apps/api/event";
import type {
  ChatMessage as _ChatMessage,
  ConversationSummary,
  DocumentRow,
  MessageRow,
  ModelInfo,
  PluginManifest,
  PluginRecord,
  TelemetrySnapshot,
  Workspace,
  WorkspaceSummary,
  MemoryBudget,
  ResourcePreflight,
  RepositoryInfo,
  SystemCheckResult,
} from "@/types";

export const dimi = {
  workspaces: {
    create: (args: {
      name: string;
      repositories: string[];
      tools: string[];
      systemPrompt: string;
    }) => invoke<string>("workspaces_create", args),
    list: () => invoke<WorkspaceSummary[]>("workspaces_list"),
    load: (id: string) => invoke<Workspace>("workspaces_load", { id }),
    update: (args: {
      id: string;
      name: string;
      repositories: string[];
      tools: string[];
      systemPrompt: string;
    }) => invoke<void>("workspaces_update", args),
    delete: (id: string) => invoke<void>("workspaces_delete", { id }),
  },

  repositories: {
    addFolder: (path: string) =>
      invoke<string>("repositories_add_folder", { path }),
    addFiles: (paths: string[]) =>
      invoke<string>("repositories_add_files", { paths }),
    reindex: (repositoryId: string) =>
      invoke<void>("repositories_reindex", { repositoryId }),
    get: (id: string) => invoke<RepositoryInfo>("repositories_get", { id }),
    indexingStatus: () => invoke<string[]>("repositories_indexing_status"),
  },

  documents: {
    list: (repositoryId: string) =>
      invoke<DocumentRow[]>("documents_list", { repositoryId }),
  },

  conversations: {
    create: (workspaceIds: string[]) =>
      invoke<string>("conversations_create", { workspaceIds }),
    list: () => invoke<ConversationSummary[]>("conversations_list"),
    attachWorkspace: (conversationId: string, workspaceId: string) =>
      invoke<void>("conversations_attach_workspace", { conversationId, workspaceId }),
    delete: (id: string) => invoke<void>("conversations_delete", { id }),
  },

  messages: {
    list: (conversationId: string) =>
      invoke<MessageRow[]>("messages_list", { conversationId }),
  },

  chat: {
    send: (args: { conversationId: string; message: string }) =>
      invoke<string>("chat_send", args),
  },

  models: {
    listInstalled: () => invoke<ModelInfo[]>("models_list_installed"),
    listAvailable: () => invoke<ModelInfo[]>("models_list_available"),
    download: (modelId: string) => invoke<void>("models_download", { modelId }),
    active: () => invoke<ModelInfo | null>("models_active").catch(() => null),
  },

  plugins: {
    discover: () => invoke<PluginManifest[]>("plugins_discover"),
    list: () => invoke<PluginRecord[]>("plugins_list"),
    install: (path: string) => invoke<string>("plugins_install", { path }),
    enable: (id: string) => invoke<void>("plugins_enable", { id }),
    disable: (id: string) => invoke<void>("plugins_disable", { id }),
  },

  health: {
    snapshot: () => invoke<TelemetrySnapshot>("health_snapshot"),
    runtimeStatus: () => invoke<Record<string, string>>("runtime_health"),
  },

  settings: {
    getMemoryBudget: () =>
      invoke<MemoryBudget>("settings_get_memory_budget").catch(
        () => "Auto" as MemoryBudget,
      ),
    setMemoryBudget: (budget: MemoryBudget) =>
      invoke<void>("settings_set_memory_budget", { budget }),
  },

  resourcePreflight: {
    check: (modelId: string) =>
      invoke<ResourcePreflight>("resource_preflight_check", { modelId }),
    loadModel: (args: { modelId: string; force: boolean }) =>
      invoke<void>("resource_preflight_load_model", args),
  },

  systemCheck: {
    status: () => invoke<SystemCheckResult>("system_check_status"),
    proceed: () => invoke<void>("system_check_continue"),
  },

  events: {
    onToken: (cb: (token: string) => void): Promise<UnlistenFn> =>
      listen<string>("inference_token", (e) => cb(e.payload)),
    on: <T = unknown>(
      topic: string,
      cb: (payload: T) => void,
    ): Promise<UnlistenFn> => listen<T>(topic, (e) => cb(e.payload)),
  },
};

export type ChatMessage = _ChatMessage;
