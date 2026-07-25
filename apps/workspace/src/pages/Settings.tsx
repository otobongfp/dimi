import { useEffect, useState } from "react";
import { Card, Badge } from "@/components/ui/Card";
import { dimi } from "@/lib/api";
import { bytesToHuman } from "@/lib/format";
import type { ModelInfo, TelemetrySnapshot, MemoryBudget, ModelDownloadProgressPayload } from "@/types";

const GB = 1024 * 1024 * 1024;

// Mirrors runtime/src/kernel/hardware.rs's parse_memory_budget — keep in sync.
const FIXED_BUDGETS: { value: Exclude<MemoryBudget, "Auto">; bytes: number }[] = [
  { value: "2GB", bytes: 2 * GB },
  { value: "3GB", bytes: 3 * GB },
  { value: "4GB", bytes: 4 * GB },
  { value: "6GB", bytes: 6 * GB },
  { value: "8GB", bytes: 8 * GB },
  { value: "12GB", bytes: 12 * GB },
];

// A fixed budget above this fraction of total RAM leaves too little for the
// OS and everything else running, risking the system-freeze scenario the
// resource-preflight screen exists to prevent.
const MAX_BUDGET_FRACTION_OF_RAM = 0.6;

export function Settings() {
  const [model, setModel] = useState<ModelInfo | null>(null);
  const [installedModels, setInstalledModels] = useState<ModelInfo[]>([]);
  const [catalogModels, setCatalogModels] = useState<ModelInfo[]>([]);
  const [downloadProgress, setDownloadProgress] = useState<Record<string, ModelDownloadProgressPayload>>({});
  const [switchingId, setSwitchingId] = useState<string | null>(null);
  const [switchError, setSwitchError] = useState<string | null>(null);
  const [telemetry, setTelemetry] = useState<TelemetrySnapshot | null>(null);
  const [health, setHealth] = useState<Record<string, string>>({});
  const [memoryBudget, setMemoryBudget] = useState<MemoryBudget>("Auto");
  const [totalRamBytes, setTotalRamBytes] = useState<number | null>(null);

  useEffect(() => {
    dimi.settings.getMemoryBudget().then(setMemoryBudget);
    dimi.systemCheck.status().then((s) => setTotalRamBytes(s.total_ram_bytes));
  }, []);

  // A budget saved on a different (bigger) machine, or before this cap
  // existed, could still exceed the safe fraction of this machine's RAM —
  // fall back to Auto rather than leaving an unsafe value active.
  useEffect(() => {
    if (totalRamBytes == null || memoryBudget === "Auto") return;
    const entry = FIXED_BUDGETS.find((b) => b.value === memoryBudget);
    if (entry && entry.bytes > totalRamBytes * MAX_BUDGET_FRACTION_OF_RAM) {
      setMemoryBudget("Auto");
      dimi.settings.setMemoryBudget("Auto");
    }
  }, [totalRamBytes, memoryBudget]);

  useEffect(() => {
    const unlistenPromise = dimi.events.on<ModelDownloadProgressPayload>(
      "model:download:progress",
      (payload) => {
        setDownloadProgress((prev) => ({ ...prev, [payload.model_id]: payload }));
        if (payload.status === "installed") refresh();
      },
    );
    return () => {
      unlistenPromise.then((unlisten) => unlisten());
    };
  }, []);

  async function handleBudgetChange(e: React.ChangeEvent<HTMLSelectElement>) {
    const newBudget = e.target.value as MemoryBudget;
    setMemoryBudget(newBudget);
    await dimi.settings.setMemoryBudget(newBudget);
  }

  async function handleDownload(modelId: string) {
    setDownloadProgress((prev) => ({ ...prev, [modelId]: { model_id: modelId, status: "downloading" } }));
    try {
      await dimi.models.download(modelId);
    } catch (e) {
      setDownloadProgress((prev) => ({
        ...prev,
        [modelId]: { model_id: modelId, status: "failed", error: String(e) },
      }));
    }
  }

  async function handleSwitch(modelId: string) {
    setSwitchingId(modelId);
    setSwitchError(null);
    try {
      await dimi.resourcePreflight.loadModel({ modelId, force: false });
      await refresh();
    } catch (e) {
      setSwitchError(String(e));
    } finally {
      setSwitchingId(null);
    }
  }

  async function refresh() {
    const [m, installed, catalog, t, h] = await Promise.all([
      dimi.models.active(),
      dimi.models.listInstalled().catch(() => []),
      dimi.models.listAvailable().catch(() => []),
      dimi.health.snapshot().catch(() => null),
      dimi.health.runtimeStatus().catch(() => ({})),
    ]);
    setModel(m);
    setInstalledModels(installed);
    setCatalogModels(catalog);
    setTelemetry(t);
    setHealth(h);
  }

  useEffect(() => {
    refresh();
    const interval = setInterval(refresh, 5000);
    return () => clearInterval(interval);
  }, []);

  return (
    <div className="p-8">
      <h1 className="text-3xl font-extrabold text-ink">Settings & Activity</h1>
      <p className="mt-1 text-ink-muted">Runtime configuration and live health, entirely on this device.</p>

      <div className="mt-8 grid grid-cols-1 gap-5 lg:grid-cols-2">
        <Card>
          <h2 className="text-sm font-semibold uppercase tracking-wide text-terracotta">Active Model</h2>
          {model ? (
            <div className="mt-3">
              <div className="text-lg font-bold text-ink">{model.name}</div>
              <div className="mt-1 text-sm text-ink-muted">
                {model.backend} · {bytesToHuman(model.size_bytes)}
              </div>
            </div>
          ) : (
            <p className="mt-3 text-sm text-ink-muted">
              No model loaded yet — Dimi downloads one automatically on first run.
            </p>
          )}
        </Card>

        <Card>
          <h2 className="text-sm font-semibold uppercase tracking-wide text-terracotta">Models</h2>
          <p className="mt-1 text-xs text-ink-muted">
            Download and switch between the two models Dimi supports out of the box.
          </p>
          {switchError && <p className="mt-3 text-xs text-red-600">{switchError}</p>}
          <ul className="mt-3 flex flex-col gap-3">
            {catalogModels.map((m) => {
              const isInstalled = installedModels.some((im) => im.id === m.id);
              const isActive = model?.id === m.id;
              const progress = downloadProgress[m.id];
              const isDownloading = progress && progress.status !== "installed" && progress.status !== "failed";
              const percent =
                progress?.bytes_completed != null && progress?.total_bytes
                  ? Math.round((progress.bytes_completed / progress.total_bytes) * 100)
                  : null;

              return (
                <li key={m.id} className="rounded-xl border border-blush/60 p-3">
                  <div className="flex items-center justify-between text-sm">
                    <span className="font-medium text-ink">{m.name}</span>
                    <span className="flex items-center gap-2">
                      <span className="text-ink-muted">{bytesToHuman(m.size_bytes)}</span>
                      {isActive && <Badge tone="success">Active</Badge>}
                    </span>
                  </div>

                  <div className="mt-2 flex items-center justify-between">
                    {isDownloading ? (
                      <span className="text-xs text-ink-muted">
                        {progress.status === "downloading"
                          ? percent != null
                            ? `Downloading… ${percent}%`
                            : "Downloading…"
                          : progress.status === "validating"
                            ? "Validating…"
                            : "Loading…"}
                      </span>
                    ) : progress?.status === "failed" ? (
                      <span className="text-xs text-red-600">{progress.error ?? "Download failed"}</span>
                    ) : (
                      <span />
                    )}

                    {!isInstalled && !isDownloading && (
                      <button
                        type="button"
                        onClick={() => handleDownload(m.id)}
                        className="rounded-lg bg-terracotta px-3 py-1.5 text-xs font-semibold text-white shadow-sm transition disabled:opacity-60"
                      >
                        {progress?.status === "failed" ? "Retry download" : "Download"}
                      </button>
                    )}

                    {isInstalled && !isActive && (
                      <button
                        type="button"
                        disabled={switchingId === m.id}
                        onClick={() => handleSwitch(m.id)}
                        className="rounded-lg border border-blush/60 bg-white px-3 py-1.5 text-xs font-semibold text-ink transition disabled:opacity-60"
                      >
                        {switchingId === m.id ? "Switching…" : "Switch to this model"}
                      </button>
                    )}
                  </div>
                </li>
              );
            })}
          </ul>
        </Card>

        <Card>
          <h2 className="text-sm font-semibold uppercase tracking-wide text-terracotta">System</h2>
          {telemetry ? (
            <div className="mt-3 grid grid-cols-2 gap-3 text-sm">
              <div>
                <div className="text-ink-muted">Memory</div>
                <div className="font-semibold text-ink">{bytesToHuman(telemetry.ram_bytes)}</div>
              </div>
              <div>
                <div className="text-ink-muted">CPU</div>
                <div className="font-semibold text-ink">{telemetry.cpu_percent.toFixed(1)}%</div>
              </div>
              <div>
                <div className="text-ink-muted">Temperature</div>
                <div
                  className={`font-semibold ${
                    (telemetry.cpu_temp_celsius ?? 0) >= 82 ? "text-terracotta" : "text-ink"
                  }`}
                  title="Above 82°C, Dimi trims the context window to reduce heat output before hitting a thermal limit."
                >
                  {telemetry.cpu_temp_celsius != null ? `${telemetry.cpu_temp_celsius.toFixed(0)}°C` : "—"}
                </div>
              </div>
              <div>
                <div className="text-ink-muted">Tokens/sec</div>
                <div className="font-semibold text-ink">{telemetry.tokens_per_sec?.toFixed(1) ?? "—"}</div>
              </div>
              <div>
                <div className="text-ink-muted">Queued jobs</div>
                <div className="font-semibold text-ink">{telemetry.job_queue_depth}</div>
              </div>
            </div>
          ) : (
            <p className="mt-3 text-sm text-ink-muted">Unavailable.</p>
          )}
        </Card>

        <Card>
          <h2 className="text-sm font-semibold uppercase tracking-wide text-terracotta">Configuration</h2>
          <div className="mt-3">
            <label htmlFor="memoryBudget" className="block text-sm font-medium text-ink">
              Memory Budget (Requires Reload)
            </label>
            <select
              id="memoryBudget"
              value={memoryBudget}
              onChange={handleBudgetChange}
              className="mt-1 block w-full rounded-md border-cream-dark bg-cream px-3 py-2 text-sm text-ink outline-none focus:ring-2 focus:ring-terracotta/20"
            >
              <option value="Auto">Auto (Dynamic)</option>
              {FIXED_BUDGETS.map(({ value, bytes }) => {
                const exceedsCap =
                  totalRamBytes != null && bytes > totalRamBytes * MAX_BUDGET_FRACTION_OF_RAM;
                return (
                  <option key={value} value={value} disabled={exceedsCap}>
                    {value.replace("GB", " GB")}
                    {exceedsCap ? " — too high for this machine" : ""}
                  </option>
                );
              })}
            </select>
            <p className="mt-2 text-xs text-ink-muted">
              Limits the maximum amount of RAM the active model and its context window can use. A lower budget prevents system freezes on machines with limited free memory.
              {totalRamBytes != null && (
                <>
                  {" "}
                  This machine has {bytesToHuman(totalRamBytes)}, so budgets above{" "}
                  {bytesToHuman(totalRamBytes * MAX_BUDGET_FRACTION_OF_RAM)} are disabled.
                </>
              )}
            </p>
          </div>
        </Card>

        <Card className="lg:col-span-2">
          <h2 className="text-sm font-semibold uppercase tracking-wide text-terracotta">Service Health</h2>
          <div className="mt-3 flex flex-wrap gap-2">
            {Object.entries(health).map(([name, state]) => (
              <Badge key={name} tone={state === "Ready" ? "success" : state === "Degraded" ? "muted" : "neutral"}>
                {name}: {state}
              </Badge>
            ))}
          </div>
        </Card>
      </div>
    </div>
  );
}
