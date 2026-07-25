import { useEffect, useState } from "react";
import { Cpu, MemoryStick } from "lucide-react";
import { dimi } from "@/lib/api";
import { bytesToHuman } from "@/lib/format";
import type { SystemCheckResult } from "@/types";

const AUTO_CONTINUE_DELAY_MS = 900;

interface SystemCheckScreenProps {
  onDone: () => void;
}

export function SystemCheckScreen({ onDone }: SystemCheckScreenProps) {
  const [result, setResult] = useState<SystemCheckResult | null>(null);

  useEffect(() => {
    let cancelled = false;
    let autoTimer: ReturnType<typeof setTimeout> | undefined;

    dimi.systemCheck
      .status()
      .then((status) => {
        if (cancelled) return;
        setResult(status);
        if (!status.requires_confirmation) {
          autoTimer = setTimeout(() => {
            dimi.systemCheck.proceed().finally(() => {
              if (!cancelled) onDone();
            });
          }, AUTO_CONTINUE_DELAY_MS);
        }
      })
      .catch(() => {
        if (!cancelled) onDone();
      });

    return () => {
      cancelled = true;
      if (autoTimer) clearTimeout(autoTimer);
    };
  }, [onDone]);

  async function proceed() {
    await dimi.systemCheck.proceed();
    onDone();
  }

  return (
    <div className="flex h-screen w-screen items-center justify-center bg-cream">
      <div className="w-full max-w-md rounded-2xl border border-blush/60 bg-white p-8 text-center shadow-sm">
        <div className="mx-auto flex h-14 w-14 items-center justify-center rounded-xl bg-blush text-3xl shadow-sm">
          🐙
        </div>
        <h1 className="mt-5 text-2xl font-extrabold text-ink">Checking your system</h1>
        <p className="mt-2 text-sm text-ink-muted">
          Dimi picks a default model that fits this machine before doing anything else.
        </p>

        <div className="mt-6 space-y-2 rounded-xl bg-cream p-4 text-left">
          <div className="flex items-center justify-between text-sm">
            <span className="flex items-center gap-2 text-ink-muted">
              <MemoryStick size={16} /> Memory
            </span>
            <span className="font-medium text-ink">
              {result ? bytesToHuman(result.total_ram_bytes) : "Detecting…"}
            </span>
          </div>
          <div className="flex items-center justify-between text-sm">
            <span className="flex items-center gap-2 text-ink-muted">
              <Cpu size={16} /> CPU / Architecture
            </span>
            <span className="font-medium text-ink">
              {result ? `${result.cpu_count}-core ${result.arch}` : "Detecting…"}
            </span>
          </div>
        </div>

        {result?.requires_confirmation && (
          <div className="mt-6">
            <p className="text-sm text-ink-muted">
              This machine has {bytesToHuman(result.total_ram_bytes)} of memory
              {result.ram_tier === "Low" ? " — Dimi will use a smaller model to stay responsive" : ""}.
              Downloading and loading the model uses noticeable memory and network for a bit; close
              anything else you don't need running before continuing.
            </p>
            <button
              type="button"
              onClick={proceed}
              className="mt-4 w-full rounded-xl bg-terracotta px-4 py-2.5 text-sm font-semibold text-white shadow-sm transition"
            >
              Continue
            </button>
          </div>
        )}
      </div>
    </div>
  );
}
