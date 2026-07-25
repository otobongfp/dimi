import { useEffect, useState } from "react";
import { AlertTriangle, RefreshCw } from "lucide-react";
import { dimi } from "@/lib/api";
import { bytesToHuman } from "@/lib/format";
import type { ModelInfo, ResourcePreflightBlockedPayload } from "@/types";

type Phase = "blocked" | "rechecking" | "loading";

export function LowMemoryScreen() {
  const [blocked, setBlocked] = useState<ResourcePreflightBlockedPayload | null>(null);
  const [phase, setPhase] = useState<Phase>("blocked");
  const [error, setError] = useState<string | null>(null);
  const [catalog, setCatalog] = useState<ModelInfo[]>([]);

  useEffect(() => {
    dimi.models.listAvailable().then(setCatalog).catch(() => setCatalog([]));
  }, []);

  useEffect(() => {
    const unlistenPromises = [
      dimi.events.on<ResourcePreflightBlockedPayload>("resource:preflight_blocked", (payload) => {
        setBlocked(payload);
        setPhase("blocked");
        setError(null);
      }),
      dimi.events.on("model:registered", () => setBlocked(null)),
    ];
    return () => {
      unlistenPromises.forEach((p) => p.then((unlisten) => unlisten()));
    };
  }, []);

  if (!blocked) return null;

  // The smallest catalog model that's actually smaller than the one that
  // just got blocked — not a hardcoded id, since "smaller" only means
  // something relative to whichever model triggered this screen.
  const blockedSize = catalog.find((m) => m.id === blocked.model_id)?.size_bytes ?? Infinity;
  const smallerModel = catalog
    .filter((m) => m.size_bytes < blockedSize)
    .sort((a, b) => a.size_bytes - b.size_bytes)[0];

  async function recheck(modelId: string) {
    setPhase("rechecking");
    setError(null);
    try {
      const result = await dimi.resourcePreflight.check(modelId);
      if (result.sufficient) {
        setPhase("loading");
        await dimi.resourcePreflight.loadModel({ modelId, force: false });
      } else {
        setBlocked({ ...result, model_id: modelId });
        setPhase("blocked");
      }
    } catch (e) {
      setError(String(e));
      setPhase("blocked");
    }
  }

  async function proceedAnyway() {
    if (!blocked) return;
    setPhase("loading");
    setError(null);
    try {
      await dimi.resourcePreflight.loadModel({ modelId: blocked.model_id, force: true });
    } catch (e) {
      setError(String(e));
      setPhase("blocked");
    }
  }

  const busy = phase !== "blocked";

  return (
    <div className="fixed inset-0 z-50 flex items-center justify-center bg-ink/40 p-4">
      <div className="w-full max-w-lg rounded-2xl border border-blush/60 bg-white p-8 shadow-lg">
        <div className="mx-auto flex h-14 w-14 items-center justify-center rounded-xl bg-blush text-terracotta shadow-sm">
          <AlertTriangle size={28} />
        </div>
        <h1 className="mt-5 text-center text-2xl font-extrabold text-ink">Not enough free memory</h1>
        <p className="mt-2 text-center text-sm text-ink-muted">
          Dimi needs about {bytesToHuman(blocked.required_bytes)} free to load this model safely, but only{" "}
          {bytesToHuman(blocked.available_bytes)} is available right now. Loading anyway risks freezing the
          whole machine, not just Dimi — so it's waiting for you to decide.
        </p>

        {blocked.top_consumers.length > 0 && (
          <div className="mt-6 rounded-xl bg-cream p-4">
            <p className="text-xs font-semibold uppercase tracking-wide text-ink-muted">
              Using the most memory right now
            </p>
            <ul className="mt-2 space-y-1.5">
              {blocked.top_consumers.map((p) => (
                <li key={p.pid} className="flex items-center justify-between text-sm text-ink">
                  <span className="truncate">{p.name}</span>
                  <span className="ml-3 shrink-0 tabular-nums text-ink-muted">{bytesToHuman(p.ram_bytes)}</span>
                </li>
              ))}
            </ul>
            <p className="mt-2 text-xs text-ink-muted">
              Close what you don't need in your OS, then recheck — Dimi won't close anything for you.
            </p>
          </div>
        )}

        {error && <p className="mt-4 text-center text-xs text-red-600">{error}</p>}

        <div className="mt-6 flex flex-col gap-2">
          <button
            type="button"
            disabled={busy}
            onClick={() => recheck(blocked.model_id)}
            className="flex items-center justify-center gap-2 rounded-xl bg-terracotta px-4 py-2.5 text-sm font-semibold text-white shadow-sm transition disabled:opacity-60"
          >
            <RefreshCw size={16} className={phase === "rechecking" ? "animate-spin" : ""} />
            {phase === "rechecking" ? "Rechecking…" : "Recheck"}
          </button>
          {smallerModel && (
            <button
              type="button"
              disabled={busy}
              onClick={() => recheck(smallerModel.id)}
              className="rounded-xl border border-blush/60 bg-white px-4 py-2.5 text-sm font-semibold text-ink transition disabled:opacity-60"
            >
              Try a smaller model instead ({smallerModel.name})
            </button>
          )}
          <button
            type="button"
            disabled={busy}
            onClick={proceedAnyway}
            className="rounded-xl px-4 py-2.5 text-sm font-medium text-ink-muted transition hover:text-ink disabled:opacity-60"
          >
            {phase === "loading" ? "Loading…" : "Proceed anyway"}
          </button>
        </div>
      </div>
    </div>
  );
}
