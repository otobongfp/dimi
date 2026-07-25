import { useCallback, useEffect, useState } from "react";
import { open } from "@tauri-apps/plugin-dialog";
import { FolderInput, Files, MessageSquareText, RefreshCw } from "lucide-react";
import { Card } from "@/components/ui/Card";
import { FileTree, type RepositoryWithDocuments } from "@/components/FileTree";
import { dimi } from "@/lib/api";
import type { RepositoryIndexingEventPayload, RepositoryIndexingFailedPayload, Workspace } from "@/types";

interface DocumentsProps {
  workspaceId: string | null;
  onOpenChat: (workspaceId: string) => void;
}

export function Documents({ workspaceId, onOpenChat }: DocumentsProps) {
  const [workspace, setWorkspace] = useState<Workspace | null>(null);
  const [repositories, setRepositories] = useState<RepositoryWithDocuments[]>([]);
  const [attaching, setAttaching] = useState<"folder" | "files" | null>(null);
  const [reindexing, setReindexing] = useState(false);
  const [indexingRepoIds, setIndexingRepoIds] = useState<Set<string>>(new Set());
  const [error, setError] = useState<string | null>(null);

  const refresh = useCallback(async () => {
    if (!workspaceId) return;
    try {
      const ws = await dimi.workspaces.load(workspaceId);
      setWorkspace(ws);
      const grouped = await Promise.all(
        ws.repositories.map(async (repositoryId) => {
          const [info, documents] = await Promise.all([
            dimi.repositories.get(repositoryId),
            dimi.documents.list(repositoryId),
          ]);
          return { id: info.id, root: info.root, documents };
        }),
      );
      setRepositories(grouped);
    } catch (e) {
      setError(String(e));
    }
  }, [workspaceId]);

  useEffect(() => {
    refresh();
  }, [refresh]);

  useEffect(() => {
    if (!workspaceId) return;
    refresh();
    dimi.repositories.indexingStatus().then((ids) => setIndexingRepoIds(new Set(ids)));

    const unlistenPromises = [
      dimi.events.on("document:parsed", refresh),
      dimi.events.on<RepositoryIndexingEventPayload>("repository:indexing_started", (payload) => {
        setIndexingRepoIds((prev) => new Set(prev).add(payload.repository_id));
      }),
      dimi.events.on<RepositoryIndexingEventPayload>("repository:indexed", (payload) => {
        setIndexingRepoIds((prev) => {
          const next = new Set(prev);
          next.delete(payload.repository_id);
          return next;
        });
        refresh();
      }),
      dimi.events.on<RepositoryIndexingFailedPayload>("repository:indexing_failed", (payload) => {
        setIndexingRepoIds((prev) => {
          const next = new Set(prev);
          next.delete(payload.repository_id);
          return next;
        });
        setError(payload.error);
        refresh();
      }),
    ];
    return () => {
      unlistenPromises.forEach((p) => p.then((unlisten) => unlisten()));
    };
  }, [refresh, workspaceId]);

  async function attachToWorkspace(repositoryId: string) {
    if (!workspace) return;
    await dimi.workspaces.update({
      id: workspace.id,
      name: workspace.name,
      repositories: [...workspace.repositories, repositoryId],
      tools: workspace.tools,
      systemPrompt: workspace.system_prompt,
    });
    await refresh();
  }

  async function attachFolder() {
    if (!workspace) return;
    const selected = await open({ directory: true, multiple: false });
    if (!selected || typeof selected !== "string") return;

    setAttaching("folder");
    setError(null);
    try {
      const repositoryId = await dimi.repositories.addFolder(selected);
      await attachToWorkspace(repositoryId);
    } catch (e) {
      setError(String(e));
    } finally {
      setAttaching(null);
    }
  }

  async function attachFiles() {
    if (!workspace) return;
    const selected = await open({ directory: false, multiple: true });
    if (!selected) return;
    const paths = Array.isArray(selected) ? selected : [selected];
    if (paths.length === 0) return;

    setAttaching("files");
    setError(null);
    try {
      const repositoryId = await dimi.repositories.addFiles(paths);
      await attachToWorkspace(repositoryId);
    } catch (e) {
      setError(String(e));
    } finally {
      setAttaching(null);
    }
  }

  async function reindexAll() {
    if (!workspace || workspace.repositories.length === 0) return;
    setReindexing(true);
    setError(null);
    try {
      for (const repositoryId of workspace.repositories) {
        await dimi.repositories.reindex(repositoryId);
      }
      await refresh();
    } catch (e) {
      setError(String(e));
    } finally {
      setReindexing(false);
    }
  }

  const anyRepoIndexing = (workspace?.repositories ?? []).some((id) => indexingRepoIds.has(id));

  if (!workspaceId) {
    return (
      <div className="p-8">
        <h1 className="text-3xl font-extrabold text-ink">Documents</h1>
        <p className="mt-2 text-ink-muted">Pick a library first to see its documents.</p>
      </div>
    );
  }

  const allDocuments = repositories.flatMap((r) => r.documents);
  const indexedCount = allDocuments.filter((d) => d.status === "indexed").length;

  return (
    <div className="p-8">
      <div className="flex items-start justify-between">
        <div>
          <h1 className="text-3xl font-extrabold text-ink">Documents</h1>
          <p className="mt-1 text-ink-muted">
            Manage local knowledge files for <span className="font-semibold">{workspace?.name}</span>.
          </p>
        </div>
        <div className="flex items-center gap-3">
          <button
            type="button"
            onClick={() => onOpenChat(workspaceId)}
            className="flex items-center gap-2 rounded-lg border border-blush-dark px-4 py-2.5 text-sm font-semibold text-terracotta transition-colors hover:bg-blush/30"
          >
            <MessageSquareText size={16} />
            Open Chat
          </button>
          <button
            type="button"
            onClick={reindexAll}
            disabled={reindexing || anyRepoIndexing || (workspace?.repositories.length ?? 0) === 0}
            title={
              anyRepoIndexing && !reindexing
                ? "Already indexing in the background — wait for it to finish before reindexing again"
                : "Re-run indexing for every repository in this library — use this if search/chat isn't finding content that should be indexed"
            }
            className="flex items-center gap-2 rounded-lg border border-blush-dark px-4 py-2.5 text-sm font-semibold text-terracotta transition-colors hover:bg-blush/30 disabled:opacity-60"
          >
            <RefreshCw size={16} className={reindexing || anyRepoIndexing ? "animate-spin" : undefined} />
            {reindexing ? "Reindexing…" : anyRepoIndexing ? "Indexing…" : "Reindex"}
          </button>
          <button
            type="button"
            onClick={attachFiles}
            disabled={attaching !== null}
            className="flex items-center gap-2 rounded-lg border border-blush-dark px-4 py-2.5 text-sm font-semibold text-terracotta transition-colors hover:bg-blush/30 disabled:opacity-60"
          >
            <Files size={16} />
            {attaching === "files" ? "Attaching…" : "Attach Files"}
          </button>
          <button
            type="button"
            onClick={attachFolder}
            disabled={attaching !== null}
            className="flex items-center gap-2 rounded-lg bg-terracotta px-4 py-2.5 text-sm font-semibold text-white shadow-sm transition-colors hover:bg-terracotta-dark disabled:opacity-60"
          >
            <FolderInput size={16} />
            {attaching === "folder" ? "Attaching…" : "Attach Folder"}
          </button>
        </div>
      </div>

      {error && <p className="mt-4 text-sm text-red-600">{error}</p>}

      <div className="mt-6 grid grid-cols-1 gap-5 sm:grid-cols-3">
        <Card>
          <div className="text-xs font-semibold uppercase tracking-wide text-terracotta">Total Indexed</div>
          <div className="mt-2 text-3xl font-extrabold text-ink">{indexedCount}</div>
        </Card>
        <Card>
          <div className="text-xs font-semibold uppercase tracking-wide text-terracotta">Total Documents</div>
          <div className="mt-2 text-3xl font-extrabold text-ink">{allDocuments.length}</div>
        </Card>
        <Card>
          <div className="text-xs font-semibold uppercase tracking-wide text-terracotta">Repositories</div>
          <div className="mt-2 text-3xl font-extrabold text-ink">{workspace?.repositories.length ?? 0}</div>
        </Card>
      </div>

      <Card className="mt-6 overflow-hidden p-0">
        <FileTree
          repositories={repositories.map((r) => ({ ...r, indexing: indexingRepoIds.has(r.id) }))}
        />
      </Card>
    </div>
  );
}
