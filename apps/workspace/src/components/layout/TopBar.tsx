import { Bell, Search, Settings } from "lucide-react";

interface TopBarProps {
  placeholder?: string;
  onOpenSettings: () => void;
}

export function TopBar({ placeholder = "Search workspace...", onOpenSettings }: TopBarProps) {
  return (
    <header className="flex items-center gap-4 border-b border-blush/60 bg-cream px-8 py-4">
      <div className="flex flex-1 items-center gap-2 rounded-full bg-blush/70 px-4 py-2.5 text-ink-muted">
        <Search size={18} strokeWidth={2} />
        <input
          type="text"
          placeholder={placeholder}
          className="w-full bg-transparent text-sm text-ink placeholder:text-ink-muted focus:outline-none"
        />
      </div>
      <button
        type="button"
        className="flex h-10 w-10 items-center justify-center rounded-full text-ink-muted transition-colors hover:bg-blush/60 hover:text-ink"
        aria-label="Notifications"
      >
        <Bell size={19} strokeWidth={2} />
      </button>
      <button
        type="button"
        onClick={onOpenSettings}
        className="flex h-10 w-10 items-center justify-center rounded-full text-ink-muted transition-colors hover:bg-blush/60 hover:text-ink"
        aria-label="Settings"
      >
        <Settings size={19} strokeWidth={2} />
      </button>
      <div className="flex h-10 w-10 items-center justify-center rounded-full bg-terracotta text-sm font-semibold text-white">
        You
      </div>
    </header>
  );
}
