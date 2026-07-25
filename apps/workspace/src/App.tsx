import { useEffect, useState } from "react";
import { Sidebar } from "@/components/layout/Sidebar";
import { TopBar } from "@/components/layout/TopBar";
import { SetupWizard } from "@/components/SetupWizard";
import { SystemCheckScreen } from "@/components/SystemCheckScreen";
import { LowMemoryScreen } from "@/components/LowMemoryScreen";
import { Home } from "@/pages/Home";
import { Chat } from "@/pages/Chat";
import { Documents } from "@/pages/Documents";
import { Libraries } from "@/pages/Libraries";
import { Plugins } from "@/pages/Plugins";
import { Settings } from "@/pages/Settings";
import { dimi } from "@/lib/api";

export type Page = "home" | "chat" | "documents" | "libraries" | "plugins" | "settings";

function BootScreen({ percent }: { percent: number }) {
  return (
    <div className="flex h-screen w-screen items-center justify-center bg-cream">
      <div className="w-full max-w-md rounded-2xl border border-blush/60 bg-white p-8 text-center shadow-sm">
        <div className="mx-auto flex h-14 w-14 items-center justify-center rounded-xl bg-blush text-3xl shadow-sm">
          🐙
        </div>
        <h1 className="mt-5 text-2xl font-extrabold text-ink">Starting Dimi</h1>
        <p className="mt-2 text-sm text-ink-muted">Waking up the local runtime…</p>
        <div className="mt-8">
          <div className="h-2.5 w-full overflow-hidden rounded-full bg-cream-dark">
            <div
              className="h-full rounded-full bg-terracotta transition-all duration-300"
              style={{ width: `${percent}%` }}
            />
          </div>
        </div>
      </div>
    </div>
  );
}

const BOOT_POLL_INTERVAL_MS = 500;
const BOOT_POLL_MAX_ATTEMPTS = 40;

function App() {
  const [page, setPage] = useState<Page>("home");
  const [activeWorkspaceId, setActiveWorkspaceId] = useState<string | null>(null);
  const [chatConversationId, setChatConversationId] = useState<string | null>(null);
  const [chatPresetWorkspaceId, setChatPresetWorkspaceId] = useState<string | null>(null);
  const [needsSetup, setNeedsSetup] = useState<boolean | null>(null);
  const [systemCheckDone, setSystemCheckDone] = useState(false);
  const [bootAttempt, setBootAttempt] = useState(0);

  useEffect(() => {
    if (!systemCheckDone) return;
    let cancelled = false;
    (async () => {
      for (let attempt = 0; attempt < BOOT_POLL_MAX_ATTEMPTS; attempt++) {
        try {
          const installed = await dimi.models.listInstalled();
          if (!cancelled) setNeedsSetup(installed.length === 0);
          return;
        } catch {
          if (!cancelled) setBootAttempt(attempt + 1);
          await new Promise((resolve) => setTimeout(resolve, BOOT_POLL_INTERVAL_MS));
        }
      }
      if (!cancelled) setNeedsSetup(false);
    })();
    return () => {
      cancelled = true;
    };
  }, [systemCheckDone]);

  if (!systemCheckDone) {
    return <SystemCheckScreen onDone={() => setSystemCheckDone(true)} />;
  }

  if (needsSetup === null) {
    return (
      <>
        <BootScreen percent={Math.min(100, Math.round((bootAttempt / BOOT_POLL_MAX_ATTEMPTS) * 100))} />
        <LowMemoryScreen />
      </>
    );
  }

  function openLibrary(workspaceId: string) {
    setActiveWorkspaceId(workspaceId);
    setPage("documents");
  }

  function navigate(nextPage: Page) {
    setChatConversationId(null);
    setChatPresetWorkspaceId(null);
    setPage(nextPage);
  }

  function openChatOnConversation(conversationId: string) {
    setChatConversationId(conversationId);
    setChatPresetWorkspaceId(null);
    setPage("chat");
  }

  function openChatForWorkspace(workspaceId: string) {
    setChatPresetWorkspaceId(workspaceId);
    setChatConversationId(null);
    setActiveWorkspaceId(workspaceId);
    setPage("chat");
  }

  if (!systemCheckDone) {
    return <SystemCheckScreen onDone={() => setSystemCheckDone(true)} />;
  }

  if (needsSetup === null) {
    return (
      <>
        <BootScreen percent={Math.min(100, Math.round((bootAttempt / BOOT_POLL_MAX_ATTEMPTS) * 100))} />
        <LowMemoryScreen />
      </>
    );
  }

  if (needsSetup) {
    return (
      <>
        <SetupWizard onReady={() => setNeedsSetup(false)} />
        <LowMemoryScreen />
      </>
    );
  }

  return (
    <>
      <div className="flex h-screen w-screen overflow-hidden bg-cream">
        <Sidebar active={page} onNavigate={navigate} onCreateLibrary={() => navigate("libraries")} />
        <div className="flex flex-1 flex-col overflow-hidden">
          <TopBar onOpenSettings={() => navigate("settings")} />
          <main className="flex-1 overflow-y-auto">
            {page === "home" && (
              <Home
                onNavigate={navigate}
                onOpenLibrary={openLibrary}
                onOpenConversation={openChatOnConversation}
              />
            )}
            {page === "chat" && (
              <Chat conversationId={chatConversationId} presetWorkspaceId={chatPresetWorkspaceId} />
            )}
            {page === "documents" && (
              <Documents workspaceId={activeWorkspaceId} onOpenChat={openChatForWorkspace} />
            )}
            {page === "libraries" && <Libraries onOpenLibrary={openLibrary} />}
            {page === "plugins" && <Plugins />}
            {page === "settings" && <Settings />}
          </main>
        </div>
      </div>
      <LowMemoryScreen />
    </>
  );
}

export default App;
