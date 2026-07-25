import { useEffect, useState } from "react";
import { Puzzle } from "lucide-react";
import { Card, Badge } from "@/components/ui/Card";
import { dimi } from "@/lib/api";
import type { PluginRecord } from "@/types";

export function Plugins() {
  const [plugins, setPlugins] = useState<PluginRecord[]>([]);
  const [error, setError] = useState<string | null>(null);
  const [busyId, setBusyId] = useState<string | null>(null);

  async function refresh() {
    try {
      setPlugins(await dimi.plugins.list());
    } catch (e) {
      setError(String(e));
    }
  }

  useEffect(() => {
    refresh();
  }, []);

  async function toggle(plugin: PluginRecord) {
    setBusyId(plugin.id);
    try {
      if (plugin.state === "enabled") {
        await dimi.plugins.disable(plugin.id);
      } else {
        await dimi.plugins.enable(plugin.id);
      }
      await refresh();
    } catch (e) {
      setError(String(e));
    } finally {
      setBusyId(null);
    }
  }

  return (
    <div className="p-8">
      <h1 className="text-3xl font-extrabold text-ink">Plugins</h1>
      <p className="mt-1 text-ink-muted">
        Plugins specialize a Workspace with domain folders, tools, and prompts — Dimi works fully without any
        installed.
      </p>

      {error && <p className="mt-4 text-sm text-red-600">{error}</p>}

      <div className="mt-8 grid grid-cols-1 gap-5 sm:grid-cols-2 lg:grid-cols-3">
        {plugins.map((p) => (
          <Card key={p.id} className="flex flex-col gap-3">
            <div className="flex items-start justify-between">
              <span className="flex h-10 w-10 items-center justify-center rounded-lg bg-blush text-terracotta">
                <Puzzle size={20} />
              </span>
              <Badge tone={p.state === "enabled" ? "success" : "muted"}>{p.state}</Badge>
            </div>
            <div>
              <div className="text-lg font-bold text-ink">{p.manifest.displayName}</div>
              <div className="text-xs text-ink-muted">v{p.manifest.version}</div>
            </div>
            <p className="flex-1 text-sm text-ink-muted">{p.manifest.description ?? "No description."}</p>
            <div className="flex flex-wrap gap-1.5">
              {p.manifest.permissions.capabilities.map((cap) => (
                <span key={cap} className="rounded-full bg-cream-dark px-2.5 py-1 text-[11px] text-ink-muted">
                  {cap}
                </span>
              ))}
            </div>
            <button
              type="button"
              onClick={() => toggle(p)}
              disabled={busyId === p.id}
              className="mt-1 rounded-lg border border-blush-dark py-2 text-sm font-semibold text-terracotta transition-colors hover:bg-blush/30 disabled:opacity-60"
            >
              {p.state === "enabled" ? "Disable" : "Enable"}
            </button>
          </Card>
        ))}

        {plugins.length === 0 && (
          <p className="col-span-full text-sm text-ink-muted">
            No plugins installed yet. Drop a plugin's directory (containing plugin.yaml) into the plugins folder
            and reopen this page.
          </p>
        )}
      </div>
    </div>
  );
}
