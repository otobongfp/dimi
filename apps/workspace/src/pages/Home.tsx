import { useEffect, useState } from "react";
import {
  FolderInput,
  MessageSquareText,
  LibraryBig,
  ChevronRight,
  Cpu,
  Gauge,
} from "lucide-react";
import { Card, Badge } from "@/components/ui/Card";
import { dimi } from "@/lib/api";
import { greeting, relativeTime, bytesToHuman } from "@/lib/format";
import type { Page } from "@/App";
import type {
  ConversationSummary,
  ModelInfo,
  TelemetrySnapshot,
  WorkspaceSummary,
} from "@/types";

interface HomeProps {
  onNavigate: (page: Page) => void;
  onOpenLibrary: (workspaceId: string) => void;
  onOpenConversation: (conversationId: string) => void;
}

export function Home({
  onNavigate,
  onOpenLibrary,
  onOpenConversation,
}: HomeProps) {
  const [conversations, setConversations] = useState<ConversationSummary[]>([]);
  const [libraries, setLibraries] = useState<WorkspaceSummary[]>([]);
  const [model, setModel] = useState<ModelInfo | null>(null);
  const [telemetry, setTelemetry] = useState<TelemetrySnapshot | null>(null);
  const [online, setOnline] = useState(false);

  useEffect(() => {
    (async () => {
      const [convos, libs, activeModel, snapshot] = await Promise.all([
        dimi.conversations.list().catch(() => []),
        dimi.workspaces.list().catch(() => []),
        dimi.models.active(),
        dimi.health.snapshot().catch(() => null),
      ]);
      setConversations(convos.slice(0, 2));
      setLibraries(libs.slice(0, 2));
      setModel(activeModel);
      setTelemetry(snapshot);
      setOnline(true);
    })();
  }, []);

  const actions = [
    {
      icon: FolderInput,
      title: "Import Documents",
      description: "Add PDFs or text files to your knowledge base.",
      onClick: () => onNavigate("libraries"),
    },
    {
      icon: MessageSquareText,
      title: "Continue Chat",
      description: "Pick up where you left off with your last inquiry.",
      onClick: () => onNavigate("chat"),
    },
    {
      icon: LibraryBig,
      title: "Create Library",
      description: "Set up a new workspace for a fresh research topic.",
      onClick: () => onNavigate("libraries"),
    },
  ];

  return (
    <div className="p-8">
      <h1 className="text-4xl font-extrabold text-ink">{greeting()}.</h1>
      <div className="mt-3">
        <Badge tone={online ? "success" : "muted"}>
          {online ? "Dimi is running. All data is local." : "Connecting…"}
        </Badge>
      </div>

      <h2 className="mt-10 text-xl font-bold text-ink">Next Actions</h2>
      <div className="mt-4 grid grid-cols-1 gap-5 sm:grid-cols-3">
        {actions.map(({ icon: Icon, title, description, onClick }) => (
          <button
            key={title}
            type="button"
            onClick={onClick}
            className="text-left"
          >
            <Card className="flex h-full flex-col gap-3 transition-shadow hover:shadow-md">
              <span className="flex h-11 w-11 items-center justify-center rounded-xl bg-blush text-terracotta">
                <Icon size={22} />
              </span>
              <div className="text-lg font-bold text-ink">{title}</div>
              <p className="text-sm text-ink-muted">{description}</p>
            </Card>
          </button>
        ))}
      </div>

      <div className="mt-10 flex items-center justify-between">
        <h2 className="text-xl font-bold text-ink">Recent Activity</h2>
        <button
          type="button"
          onClick={() => onNavigate("libraries")}
          className="flex items-center text-sm font-semibold text-terracotta hover:underline"
        >
          View all libraries <ChevronRight size={16} />
        </button>
      </div>

      <div className="mt-4 grid grid-cols-1 gap-6 lg:grid-cols-2">
        <div>
          <h3 className="mb-2 text-xs font-semibold uppercase tracking-wide text-ink-muted">
            Recent Chats
          </h3>
          <div className="flex flex-col gap-2">
            {conversations.map((c) => (
              <button
                key={c.id}
                type="button"
                onClick={() => onOpenConversation(c.id)}
                className="flex items-center justify-between rounded-xl bg-blush/40 px-4 py-3 text-left hover:bg-blush/60"
              >
                <div>
                  <div className="font-semibold text-ink">
                    {c.title ??
                      (c.workspace_names.length > 0
                        ? c.workspace_names.join(", ")
                        : "General chat")}
                  </div>
                  <div className="text-xs text-ink-muted">
                    {relativeTime(c.created_at)}
                  </div>
                </div>
                <ChevronRight size={18} className="text-ink-muted" />
              </button>
            ))}
            {conversations.length === 0 && (
              <p className="text-sm text-ink-muted">No conversations yet.</p>
            )}
          </div>
        </div>

        <div>
          <h3 className="mb-2 text-xs font-semibold uppercase tracking-wide text-ink-muted">
            Recent Libraries
          </h3>
          <div className="flex flex-col gap-2">
            {libraries.map((l) => (
              <button
                key={l.id}
                type="button"
                onClick={() => onOpenLibrary(l.id)}
                className="flex items-center justify-between rounded-xl bg-blush/40 px-4 py-3 text-left hover:bg-blush/60"
              >
                <div className="font-semibold text-ink">{l.name}</div>
                <ChevronRight size={18} className="text-ink-muted" />
              </button>
            ))}
            {libraries.length === 0 && (
              <p className="text-sm text-ink-muted">No libraries yet.</p>
            )}
          </div>
        </div>
      </div>

      <Card className="mt-10 flex flex-wrap items-center gap-8">
        <div className="flex items-center gap-2 text-sm">
          <Cpu size={16} className="text-terracotta" />
          <span className="text-ink-muted">Model:</span>
          <span className="font-semibold text-ink">
            {model?.name ?? "none loaded"}
          </span>
        </div>
        {telemetry && (
          <div className="flex items-center gap-2 text-sm">
            <Gauge size={16} className="text-terracotta" />
            <span className="text-ink-muted">RAM:</span>
            <span className="font-semibold text-ink">
              {bytesToHuman(telemetry.ram_bytes)}
            </span>
            <span className="text-ink-muted">· CPU:</span>
            <span className="font-semibold text-ink">
              {telemetry.cpu_percent.toFixed(0)}%
            </span>
          </div>
        )}
        <span className="ml-auto text-xs text-ink-muted">
          Local inference · zero network egress
        </span>
      </Card>
    </div>
  );
}
