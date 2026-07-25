import type { ReactNode } from "react";

export function Card({ children, className = "" }: { children: ReactNode; className?: string }) {
  return (
    <div className={`rounded-2xl border border-blush/60 bg-white p-6 shadow-sm ${className}`}>
      {children}
    </div>
  );
}

export function Badge({ children, tone = "neutral" }: { children: ReactNode; tone?: "neutral" | "success" | "muted" }) {
  const toneClasses =
    tone === "success"
      ? "bg-success-bg text-success-text"
      : tone === "muted"
        ? "bg-cream-dark text-ink-muted"
        : "bg-blush text-terracotta";
  return (
    <span className={`inline-flex items-center rounded-full px-3 py-1 text-xs font-semibold ${toneClasses}`}>
      {children}
    </span>
  );
}
