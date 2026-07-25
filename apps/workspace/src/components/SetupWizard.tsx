import { useEffect, useState } from "react";
import { CheckCircle2, XCircle } from "lucide-react";
import { dimi } from "@/lib/api";

interface DownloadProgressPayload {
  model_id: string;
  status: "downloading" | "validating" | "installed" | "failed";
  error?: string;
}

interface SetupWizardProps {
  onReady: () => void;
}

export function SetupWizard({ onReady }: SetupWizardProps) {
  const [status, setStatus] = useState<DownloadProgressPayload["status"]>("downloading");
  const [errorMessage, setErrorMessage] = useState<string | null>(null);

  useEffect(() => {
    const unlistenPromises = [
      dimi.events.on<DownloadProgressPayload>("model:download:progress", (payload) => {
        setStatus(payload.status);
        if (payload.status === "failed") setErrorMessage(payload.error ?? "Unknown error");
      }),
      dimi.events.on("model:registered", () => {
        setStatus("installed");
        window.setTimeout(onReady, 900);
      }),
    ];
    return () => {
      unlistenPromises.forEach((p) => p.then((unlisten) => unlisten()));
    };
  }, [onReady]);

  const statusLabel =
    status === "failed"
      ? "Setup couldn't finish"
      : status === "installed"
        ? "Ready!"
        : status === "validating"
          ? "Verifying download…"
          : "Downloading the default AI model…";

  return (
    <div className="flex h-screen w-screen items-center justify-center bg-cream">
      <div className="w-full max-w-md rounded-2xl border border-blush/60 bg-white p-8 text-center shadow-sm">
        <div className="mx-auto flex h-14 w-14 items-center justify-center rounded-xl bg-blush text-3xl shadow-sm">
          🐙
        </div>
        <h1 className="mt-5 text-2xl font-extrabold text-ink">Setting up your system for local AI inference...</h1>

        <div className="mt-8">
          {status === "failed" ? (
            <div className="flex flex-col items-center gap-2 text-red-600">
              <XCircle size={28} />
              <p className="text-sm font-medium">{statusLabel}</p>
              <p className="text-xs text-ink-muted">{errorMessage}</p>
              <p className="mt-2 text-xs text-ink-muted">
                Check your internet connection and restart Dimi to try again.
              </p>
            </div>
          ) : status === "installed" ? (
            <div className="flex flex-col items-center gap-2 text-success-text">
              <CheckCircle2 size={28} />
              <p className="text-sm font-semibold">{statusLabel}</p>
            </div>
          ) : (
            <>
              <div className="h-2.5 w-full overflow-hidden rounded-full bg-cream-dark">
                <div
                  className={`h-full rounded-full bg-terracotta transition-all duration-300 ${status === "downloading" ? "w-full animate-pulse" : "w-full"}`}
                />
              </div>
              <p className="mt-3 text-sm font-medium text-ink">{statusLabel}</p>
            </>
          )}
        </div>
      </div>
    </div>
  );
}
