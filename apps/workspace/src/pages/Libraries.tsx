import { useEffect, useState } from "react";
import { open, confirm } from "@tauri-apps/plugin-dialog";
import { Plus, LibraryBig, Loader2, Trash2, Pencil } from "lucide-react";
import { Card, Badge } from "@/components/ui/Card";
import { dimi } from "@/lib/api";
import type { RepositoryIndexingEventPayload, Workspace, WorkspaceSummary } from "@/types";

type NameModalState =
  | { mode: "create"; folderPath: string; defaultName: string }
  | { mode: "rename"; workspace: Workspace };

const DEFAULT_SYSTEM_PROMPT =
  "You are Dimi, a private local AI assistant. Answer questions using the documents in this workspace, citing sources when you do.";

const DEFAULT_TOOLS = [
  "file_search",
  "find_files",
  "calculator",
  "list_directory",
  "read_file",
  "write_file",
  "move_file",
];

interface LibrariesProps {
  onOpenLibrary: (workspaceId: string) => void;
}

export function Libraries({ onOpenLibrary }: LibrariesProps) {
  const [workspaces, setWorkspaces] = useState<Workspace[]>([]);
  const [loading, setLoading] = useState(true);
  const [deletingId, setDeletingId] = useState<string | null>(null);
  const [indexingRepoIds, setIndexingRepoIds] = useState<Set<string>>(new Set());
  const [error, setError] = useState<string | null>(null);
  const [nameModal, setNameModal] = useState<NameModalState | null>(null);
  const [nameInput, setNameInput] = useState("");
  const [savingName, setSavingName] = useState(false);

  async function refresh() {
    setLoading(true);
    try {
      const summaries: WorkspaceSummary[] = await dimi.workspaces.list();
      const full = await Promise.all(
        summaries.map((s) => dimi.workspaces.load(s.id)),
      );
      setWorkspaces(full);
    } catch (e) {
      setError(String(e));
    } finally {
      setLoading(false);
    }
  }

  useEffect(() => {
    refresh();
    dimi.repositories.indexingStatus().then((ids) => setIndexingRepoIds(new Set(ids)));

    const unlistenPromises = [
      dimi.events.on<RepositoryIndexingEventPayload>("repository:indexing_started", (payload) => {
        setIndexingRepoIds((prev) => new Set(prev).add(payload.repository_id));
      }),
      dimi.events.on<RepositoryIndexingEventPayload>("repository:indexed", (payload) => {
        setIndexingRepoIds((prev) => {
          const next = new Set(prev);
          next.delete(payload.repository_id);
          return next;
        });
      }),
      dimi.events.on<RepositoryIndexingEventPayload>("repository:indexing_failed", (payload) => {
        setIndexingRepoIds((prev) => {
          const next = new Set(prev);
          next.delete(payload.repository_id);
          return next;
        });
      }),
    ];
    return () => {
      unlistenPromises.forEach((p) => p.then((unlisten) => unlisten()));
    };
  }, []);

  async function pickFolderForNewLibrary() {
    setError(null);
    const selected = await open({ directory: true, multiple: false });
    if (!selected || typeof selected !== "string") return;

    const defaultName = selected.split(/[\\/]/).filter(Boolean).pop() ?? "New Library";
    setNameModal({ mode: "create", folderPath: selected, defaultName });
    setNameInput(defaultName);
  }

  function startRename(ws: Workspace) {
    setError(null);
    setNameModal({ mode: "rename", workspace: ws });
    setNameInput(ws.name);
  }

  async function confirmNameModal() {
    if (!nameModal) return;
    const name = nameInput.trim();
    if (!name) return;

    setSavingName(true);
    setError(null);
    try {
      if (nameModal.mode === "create") {
        const workspaceId = await dimi.workspaces.create({
          name,
          repositories: [],
          tools: DEFAULT_TOOLS,
          systemPrompt: DEFAULT_SYSTEM_PROMPT,
        });
        const repositoryId = await dimi.repositories.addFolder(nameModal.folderPath);
        await dimi.workspaces.update({
          id: workspaceId,
          name,
          repositories: [repositoryId],
          tools: DEFAULT_TOOLS,
          systemPrompt: DEFAULT_SYSTEM_PROMPT,
        });
      } else {
        const ws = nameModal.workspace;
        await dimi.workspaces.update({
          id: ws.id,
          name,
          repositories: ws.repositories,
          tools: ws.tools,
          systemPrompt: ws.system_prompt,
        });
      }
      setNameModal(null);
      await refresh();
    } catch (e) {
      setError(String(e));
    } finally {
      setSavingName(false);
    }
  }

  async function deleteLibrary(ws: WorkspaceSummary) {
    const confirmed = await confirm(
      `Delete "${ws.name}"? This removes the library from Dimi — the original folder and its files are untouched.`,
      { title: "Delete library", kind: "warning" },
    );
    if (!confirmed) return;

    setDeletingId(ws.id);
    setError(null);
    try {
      await dimi.workspaces.delete(ws.id);
      await refresh();
    } catch (e) {
      setError(String(e));
    } finally {
      setDeletingId(null);
    }
  }

  return (
    <div className="p-8">
      <h1 className="text-3xl font-extrabold text-ink">Knowledge Libraries</h1>
      <p className="mt-1 text-ink-muted">
        Manage your isolated intelligence nodes.
      </p>

      {error && <p className="mt-4 text-sm text-red-600">{error}</p>}

      <div className="mt-8 grid grid-cols-1 gap-5 sm:grid-cols-2 lg:grid-cols-3">
        <button
          type="button"
          onClick={pickFolderForNewLibrary}
          className="flex min-h-45 flex-col items-center justify-center gap-3 rounded-2xl border-2 border-dashed border-blush-dark text-terracotta transition-colors hover:bg-blush/30"
        >
          <span className="flex h-11 w-11 items-center justify-center rounded-full bg-blush">
            <Plus size={22} />
          </span>
          <span className="font-semibold">+ Create Library</span>
          <span className="max-w-[16rem] text-center text-xs text-ink-muted">
            Pick a folder — Dimi indexes it automatically
          </span>
        </button>

        {workspaces.map((ws) => {
          const isIndexing = ws.repositories.some((id) => indexingRepoIds.has(id));
          return (
          <div
            key={ws.id}
            role="button"
            tabIndex={0}
            onClick={() => onOpenLibrary(ws.id)}
            onKeyDown={(e) => {
              if (e.key === "Enter" || e.key === " ") onOpenLibrary(ws.id);
            }}
            className="cursor-pointer text-left"
          >
            <Card className="flex h-full flex-col gap-3 transition-shadow hover:shadow-md">
              <div className="flex items-start justify-between">
                <span className="flex h-10 w-10 items-center justify-center rounded-lg bg-blush text-terracotta">
                  <LibraryBig size={20} />
                </span>
                <div className="flex items-center gap-2">
                  {isIndexing ? (
                    <Badge tone="muted">
                      <Loader2 size={12} className="mr-1 inline animate-spin" />
                      Indexing…
                    </Badge>
                  ) : (
                    <Badge tone="success">Ready</Badge>
                  )}
                  <button
                    type="button"
                    aria-label={`Rename ${ws.name}`}
                    onClick={(e) => {
                      e.stopPropagation();
                      startRename(ws);
                    }}
                    className="rounded-md p-1 text-ink-muted transition-colors hover:bg-blush/40 hover:text-terracotta"
                  >
                    <Pencil size={16} />
                  </button>
                  <button
                    type="button"
                    aria-label={`Delete ${ws.name}`}
                    disabled={deletingId === ws.id}
                    onClick={(e) => {
                      e.stopPropagation();
                      deleteLibrary(ws);
                    }}
                    className="rounded-md p-1 text-ink-muted transition-colors hover:bg-blush/40 hover:text-terracotta disabled:opacity-60"
                  >
                    {deletingId === ws.id ? (
                      <Loader2 size={16} className="animate-spin" />
                    ) : (
                      <Trash2 size={16} />
                    )}
                  </button>
                </div>
              </div>
              <div className="text-lg font-bold text-ink">{ws.name}</div>
              <p className="line-clamp-3 flex-1 text-sm text-ink-muted">
                {ws.system_prompt}
              </p>
              <div className="flex items-center gap-1 border-t border-blush/60 pt-3 text-xs text-ink-muted">
                <LibraryBig size={14} />
                {ws.repositories.length}{" "}
                {ws.repositories.length === 1 ? "repository" : "repositories"}
              </div>
            </Card>
          </div>
          );
        })}

        {!loading && workspaces.length === 0 && (
          <p className="col-span-full text-sm text-ink-muted">
            No libraries yet — create one to point Dimi at your first folder of
            documents.
          </p>
        )}
      </div>

      {nameModal && (
        <div className="fixed inset-0 z-50 flex items-center justify-center bg-ink/40 p-4">
          <div className="w-full max-w-sm rounded-2xl border border-blush/60 bg-white p-6 shadow-lg">
            <h2 className="text-lg font-bold text-ink">
              {nameModal.mode === "create" ? "Name this library" : "Rename library"}
            </h2>
            <p className="mt-1 text-sm text-ink-muted">
              {nameModal.mode === "create"
                ? "Defaults to the folder name — call it whatever makes sense to you."
                : "This only changes the library's name, nothing else."}
            </p>
            <input
              type="text"
              autoFocus
              value={nameInput}
              onChange={(e) => setNameInput(e.target.value)}
              onKeyDown={(e) => {
                if (e.key === "Enter") confirmNameModal();
                if (e.key === "Escape") setNameModal(null);
              }}
              className="mt-4 w-full rounded-lg border border-blush bg-cream px-3 py-2 text-sm text-ink outline-none focus:ring-2 focus:ring-terracotta"
            />
            <div className="mt-5 flex justify-end gap-2">
              <button
                type="button"
                onClick={() => setNameModal(null)}
                disabled={savingName}
                className="rounded-lg px-4 py-2 text-sm font-medium text-ink-muted transition hover:text-ink disabled:opacity-60"
              >
                Cancel
              </button>
              <button
                type="button"
                onClick={confirmNameModal}
                disabled={savingName || !nameInput.trim()}
                className="flex items-center gap-2 rounded-lg bg-terracotta px-4 py-2 text-sm font-semibold text-white shadow-sm transition-colors hover:bg-terracotta-dark disabled:opacity-60"
              >
                {savingName && <Loader2 size={14} className="animate-spin" />}
                {nameModal.mode === "create" ? "Create" : "Save"}
              </button>
            </div>
          </div>
        </div>
      )}
    </div>
  );
}
