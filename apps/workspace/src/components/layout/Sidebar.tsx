import { Home, MessageSquare, LibraryBig, Puzzle, Settings, LifeBuoy } from "lucide-react";
import type { Page } from "@/App";

interface NavItem {
  page: Page;
  label: string;
  icon: typeof Home;
}

const NAV_ITEMS: NavItem[] = [
  { page: "home", label: "Home", icon: Home },
  { page: "chat", label: "Chat", icon: MessageSquare },
  { page: "libraries", label: "Libraries", icon: LibraryBig },
  { page: "plugins", label: "Plugins", icon: Puzzle },
];

interface SidebarProps {
  active: Page;
  onNavigate: (page: Page) => void;
  onCreateLibrary: () => void;
}

export function Sidebar({ active, onNavigate, onCreateLibrary }: SidebarProps) {
  return (
    <aside className="flex h-full w-[280px] shrink-0 flex-col bg-cream-dark px-4 py-6">
      <div className="flex items-center gap-3 px-2 pb-8">
        <div className="flex h-11 w-11 items-center justify-center rounded-xl bg-white text-2xl shadow-sm">
          🐙
        </div>
        <div>
          <div className="text-lg font-bold leading-tight text-ink">Dimi Workspace</div>
          <div className="text-xs text-ink-muted">Local Intelligence</div>
        </div>
      </div>

      <nav className="flex flex-col gap-1">
        {NAV_ITEMS.map(({ page, label, icon: Icon }) => {
          const isActive = active === page;
          return (
            <button
              key={page}
              type="button"
              onClick={() => onNavigate(page)}
              className={`flex items-center gap-3 rounded-lg border-l-4 px-3 py-2.5 text-left text-sm font-medium transition-colors ${
                isActive
                  ? "border-terracotta bg-blush text-terracotta"
                  : "border-transparent text-ink-muted hover:bg-blush/50 hover:text-ink"
              }`}
            >
              <Icon size={18} strokeWidth={2} />
              {label}
            </button>
          );
        })}
      </nav>

      <button
        type="button"
        onClick={onCreateLibrary}
        className="mt-6 rounded-lg bg-terracotta px-4 py-2.5 text-sm font-semibold text-white shadow-sm transition-colors hover:bg-terracotta-dark"
      >
        + Create Library
      </button>

      <div className="mt-auto flex flex-col gap-1 pt-6">
        <button
          type="button"
          onClick={() => onNavigate("settings")}
          className="flex items-center gap-3 rounded-lg px-3 py-2.5 text-left text-sm font-medium text-ink-muted transition-colors hover:bg-blush/50 hover:text-ink"
        >
          <Settings size={18} strokeWidth={2} />
          Settings
        </button>
        <button
          type="button"
          className="flex items-center gap-3 rounded-lg px-3 py-2.5 text-left text-sm font-medium text-ink-muted transition-colors hover:bg-blush/50 hover:text-ink"
        >
          <LifeBuoy size={18} strokeWidth={2} />
          Support
        </button>
      </div>
    </aside>
  );
}
