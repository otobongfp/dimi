import { useEffect, useRef, useState } from "react";
import { confirm } from "@tauri-apps/plugin-dialog";
import { openPath, revealItemInDir } from "@tauri-apps/plugin-opener";
import ReactMarkdown from "react-markdown";
import remarkGfm from "remark-gfm";
import type { Components } from "react-markdown";
import { Loader2, Send, Trash2, ChevronRight, FileText } from "lucide-react";
import { dimi } from "@/lib/api";
import { relativeTime } from "@/lib/format";
import type {
  ChatMessage,
  ConversationSummary,
  MessageRow,
  MessageSource,
  WorkspaceSummary,
} from "@/types";

const FILE_LINK_SCHEME = "dimi-file://";

function toChatMessage(row: MessageRow): ChatMessage {
  let sources: MessageSource[] | undefined;
  if (row.sources) {
    try {
      sources = JSON.parse(row.sources) as MessageSource[];
    } catch {
      sources = undefined;
    }
  }
  return { role: row.role, content: row.content, sources };
}

function linkifySources(
  content: string,
  sources: MessageSource[] | undefined,
): string {
  if (!sources || sources.length === 0) return content;
  const byName = new Map<string, string>();
  for (const s of sources) {
    if (!byName.has(s.name)) byName.set(s.name, s.path);
  }
  const names = [...byName.keys()].sort((a, b) => b.length - a.length);
  if (names.length === 0) return content;

  const pattern = new RegExp(
    `(?<!\\]\\()(${names.map((n) => n.replace(/[.*+?^${}()|[\]\\]/g, "\\$&")).join("|")})`,
    "g",
  );
  return content.replace(pattern, (match) => {
    const path = byName.get(match);
    if (!path) return match;
    return `[${match}](${FILE_LINK_SCHEME}${encodeURIComponent(path)})`;
  });
}

const markdownComponents: Components = {
  p({ children }) {
    return <p className="mb-2 last:mb-0">{children}</p>;
  },
  ul({ children }) {
    return (
      <ul className="mb-2 list-disc space-y-1 pl-5 last:mb-0">{children}</ul>
    );
  },
  ol({ children }) {
    return (
      <ol className="mb-2 list-decimal space-y-1 pl-5 last:mb-0">{children}</ol>
    );
  },
  code({ children, className }) {
    const isBlock = /language-/.test(className ?? "");
    return isBlock ? (
      <code className={className}>{children}</code>
    ) : (
      <code className="rounded bg-cream-dark px-1 py-0.5 text-[0.85em]">
        {children}
      </code>
    );
  },
  pre({ children }) {
    return (
      <pre className="mb-2 overflow-x-auto rounded-lg bg-ink/90 p-3 text-xs text-cream last:mb-0">
        {children}
      </pre>
    );
  },
  a({ href, children, ...props }) {
    if (href?.startsWith(FILE_LINK_SCHEME)) {
      const path = decodeURIComponent(href.slice(FILE_LINK_SCHEME.length));
      return (
        <button
          type="button"
          onClick={() =>
            openPath(path).catch((e) => console.error("failed to open file", e))
          }
          className="inline-flex items-center gap-1 rounded-md bg-blush/60 px-1.5 py-0.5 font-medium text-terracotta underline decoration-dotted hover:bg-blush"
          title={path}
        >
          <FileText size={12} />
          {children}
        </button>
      );
    }
    return (
      <a href={href} target="_blank" rel="noreferrer" {...props}>
        {children}
      </a>
    );
  },
};

function parseMessageParts(content: string) {
  if (!content) return { pre: "", think: null, post: "", thinkClosed: false };

  const thinkStart = content.indexOf("<think>");
  if (thinkStart === -1)
    return { pre: content, think: null, post: "", thinkClosed: false };

  const thinkEnd = content.indexOf("</think>");
  const pre = content.slice(0, thinkStart);

  if (thinkEnd === -1) {
    return {
      pre,
      think: content.slice(thinkStart + 7),
      post: "",
      thinkClosed: false,
    };
  }

  return {
    pre,
    think: content.slice(thinkStart + 7, thinkEnd),
    post: content.slice(thinkEnd + 8),
    thinkClosed: true,
  };
}

const STATUS_WORDS = [
  "Thinking",
  "Working",
  "Cogitating",
  "Churning",
  "Analyzing",
  "Elucidating",
  "Pondering",
  "Synthesizing",
  "Percolating",
  "Deliberating",
  "Mulling",
  "Reasoning",
  "Contemplating",
  "Ruminating",
  "Parsing",
  "Computing",
  "Processing",
  "Inferring",
  "Sifting",
  "Digesting",
  "Weighing",
  "Untangling",
  "Distilling",
  "Considering",
  "Formulating",
  "Assembling",
  "Crunching",
  "Deducing",
  "Evaluating",
  "Exploring",
  "Gathering",
  "Interpreting",
  "Investigating",
  "Mapping",
  "Noodling",
  "Organizing",
  "Reflecting",
  "Scanning",
  "Searching",
  "Simmering",
  "Unpacking",
  "Wrangling",
];

// Never repeats the same word twice in a row so the cadence can't be
// anticipated, but stays within a small fixed vocabulary.
function nextStatusWord(current: string | null) {
  const options = current
    ? STATUS_WORDS.filter((w) => w !== current)
    : STATUS_WORDS;
  return options[Math.floor(Math.random() * options.length)];
}

function StatusText() {
  const [word, setWord] = useState(() => nextStatusWord(null));
  const [typed, setTyped] = useState("");

  useEffect(() => {
    let cancelled = false;
    let timer: ReturnType<typeof setTimeout>;

    let i = 0;
    const typeNext = () => {
      if (cancelled) return;
      i += 1;
      setTyped(word.slice(0, i));
      timer =
        i < word.length
          ? setTimeout(typeNext, 35)
          : setTimeout(() => {
              if (!cancelled) setWord((prev) => nextStatusWord(prev));
            }, 3000);
    };
    setTyped("");
    timer = setTimeout(typeNext, 35);

    return () => {
      cancelled = true;
      clearTimeout(timer);
    };
  }, [word]);

  return (
    <span className="text-xs font-medium text-ink-muted" aria-live="polite">
      {typed}
    </span>
  );
}

function TypingWave() {
  return (
    <span
      className="inline-flex items-center gap-2"
      aria-label="Dimi is typing"
    >
      <StatusText />
      <span className="inline-flex h-3.5 items-center gap-0.5">
        {[0, 1, 2, 3].map((i) => (
          <span
            key={i}
            className="animate-wave-bar h-full w-1 origin-center rounded-full bg-terracotta/60"
            style={{ animationDelay: `${i * 0.12}s` }}
          />
        ))}
      </span>
    </span>
  );
}

function SourcesList({ sources }: { sources: MessageSource[] }) {
  const unique: MessageSource[] = [];
  const seen = new Set<string>();
  for (const s of sources) {
    if (seen.has(s.path)) continue;
    seen.add(s.path);
    unique.push(s);
  }
  if (unique.length === 0) return null;

  return (
    <details className="group mt-1.5 w-full max-w-[80%] text-ink-muted [&_summary::-webkit-details-marker]:hidden">
      <summary className="flex cursor-pointer items-center gap-1.5 text-xs font-semibold select-none hover:text-terracotta">
        <ChevronRight
          size={12}
          className="transition-transform group-open:rotate-90"
        />
        Sources ({unique.length})
      </summary>
      <ul className="mt-1.5 flex flex-col gap-0.5 border-l border-blush pl-3">
        {unique.map((s) => (
          <li key={s.path}>
            <button
              type="button"
              onClick={() =>
                revealItemInDir(s.path).catch((e) =>
                  console.error("failed to reveal file", e),
                )
              }
              className="flex w-full items-center gap-1.5 rounded-md px-1.5 py-1 text-left text-xs text-ink-muted transition-colors hover:bg-blush/40 hover:text-terracotta"
              title={`Show "${s.name}" in its folder`}
            >
              <FileText size={12} className="shrink-0" />
              <span className="truncate">{s.name}</span>
            </button>
          </li>
        ))}
      </ul>
    </details>
  );
}

interface ChatProps {
  conversationId?: string | null;
  presetWorkspaceId?: string | null;
}

export function Chat({
  conversationId = null,
  presetWorkspaceId = null,
}: ChatProps) {
  const [conversations, setConversations] = useState<ConversationSummary[]>([]);
  const [libraries, setLibraries] = useState<WorkspaceSummary[]>([]);
  const [activeConversationId, setActiveConversationId] = useState<
    string | null
  >(null);
  const [messages, setMessages] = useState<ChatMessage[]>([]);
  const [input, setInput] = useState("");
  const [sending, setSending] = useState(false);
  const [deletingId, setDeletingId] = useState<string | null>(null);
  const [error, setError] = useState<string | null>(null);

  const [picking, setPicking] = useState(false);
  const [selectedLibraryIds, setSelectedLibraryIds] = useState<string[]>([]);

  const [modelReady, setModelReady] = useState(false);

  const scrollRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    let cancelled = false;
    let timer: ReturnType<typeof setTimeout>;

    const poll = async () => {
      try {
        const status = await dimi.health.runtimeStatus();
        if (cancelled) return;
        if (status.inference === "Ready") {
          setModelReady(true);
          return;
        }
      } catch {
        // Runtime health isn't up yet either — keep polling.
      }
      timer = setTimeout(poll, 1000);
    };
    poll();

    return () => {
      cancelled = true;
      clearTimeout(timer);
    };
  }, []);

  async function refreshConversations() {
    const all = await dimi.conversations.list();
    setConversations(all);
    return all;
  }

  useEffect(() => {
    let cancelled = false;
    (async () => {
      const [all, libs] = await Promise.all([
        refreshConversations(),
        dimi.workspaces.list(),
      ]);
      if (cancelled) return;
      setLibraries(libs);

      if (conversationId) {
        await openConversation(conversationId);
        return;
      }
      if (presetWorkspaceId) {
        const existing = all.find((c) =>
          c.workspace_ids.includes(presetWorkspaceId),
        );
        if (existing) {
          await openConversation(existing.id);
        } else {
          await createConversation([presetWorkspaceId]);
        }
        return;
      }
      if (all.length > 0) {
        await openConversation(all[0].id);
      } else {
        startPicking([]);
      }
    })();
    return () => {
      cancelled = true;
    };
  }, [conversationId, presetWorkspaceId]);

  useEffect(() => {
    scrollRef.current?.scrollTo({
      top: scrollRef.current.scrollHeight,
      behavior: "smooth",
    });
  }, [messages]);

  async function openConversation(id: string) {
    setPicking(false);
    setActiveConversationId(id);
    const rows = await dimi.messages.list(id);
    setMessages(rows.map(toChatMessage));
  }

  function startPicking(preselect: string[]) {
    setSelectedLibraryIds(preselect);
    setPicking(true);
  }

  async function createConversation(workspaceIds: string[]) {
    setError(null);
    try {
      const id = await dimi.conversations.create(workspaceIds);
      await refreshConversations();
      await openConversation(id);
    } catch (e) {
      setError(String(e));
    }
  }

  function toggleLibrary(id: string) {
    setSelectedLibraryIds((prev) =>
      prev.includes(id) ? prev.filter((x) => x !== id) : [...prev, id],
    );
  }

  async function attachLibrary(workspaceId: string) {
    if (!activeConversationId) return;
    setError(null);
    try {
      await dimi.conversations.attachWorkspace(
        activeConversationId,
        workspaceId,
      );
      await refreshConversations();
    } catch (e) {
      setError(String(e));
    }
  }

  async function deleteConversation(id: string) {
    const confirmed = await confirm(
      "Delete this chat? Its message history can't be recovered.",
      {
        title: "Delete chat",
        kind: "warning",
      },
    );
    if (!confirmed) return;

    setDeletingId(id);
    setError(null);
    try {
      await dimi.conversations.delete(id);
      const remaining = await refreshConversations();
      if (activeConversationId === id) {
        if (remaining.length > 0) {
          await openConversation(remaining[0].id);
        } else {
          setActiveConversationId(null);
          setMessages([]);
          startPicking([]);
        }
      }
    } catch (e) {
      setError(String(e));
    } finally {
      setDeletingId(null);
    }
  }

  async function sendMessage() {
    console.log(">>> JS: sendMessage triggered", {
      activeConversationId,
      input,
      sending,
    });
    if (!activeConversationId || !input.trim() || sending || !modelReady)
      return;
    const userMessage = input.trim();
    setInput("");
    setError(null);
    setMessages((prev) => [
      ...prev,
      { role: "user", content: userMessage },
      { role: "assistant", content: "" },
    ]);
    setSending(true);

    let unlisten: (() => void) | undefined;
    try {
      unlisten = await dimi.events.onToken((token) => {
        setMessages((prev) => {
          const next = [...prev];
          const last = next[next.length - 1];
          next[next.length - 1] = { ...last, content: last.content + token };
          return next;
        });
      });

      console.log(">>> JS: Calling dimi.chat.send for:", userMessage);
      await dimi.chat.send({
        conversationId: activeConversationId,
        message: userMessage,
      });
      console.log(">>> JS: dimi.chat.send returned successfully");
      // The streamed tokens above don't carry the assistant message's
      // structured `sources` — reload from storage now that it's persisted.
      const rows = await dimi.messages.list(activeConversationId);
      setMessages(rows.map(toChatMessage));
      await refreshConversations();
    } catch (e) {
      setError(String(e));
      setMessages((prev) => {
        const next = [...prev];
        next[next.length - 1] = { role: "assistant", content: `⚠️ ${e}` };
        return next;
      });
    } finally {
      if (unlisten) unlisten();
      setSending(false);
    }
  }

  const activeConversation =
    conversations.find((c) => c.id === activeConversationId) ?? null;

  return (
    <div className="flex h-full">
      <div className="w-72 shrink-0 overflow-y-auto border-r border-blush/60 p-6">
        <div className="mb-4 flex items-center justify-between">
          <h2 className="text-xl font-extrabold text-ink">Conversations</h2>
        </div>
        <button
          type="button"
          onClick={() => startPicking([])}
          className="mb-3 w-full rounded-lg border border-dashed border-blush-dark py-2 text-sm font-semibold text-terracotta hover:bg-blush/30"
        >
          + New conversation
        </button>
        <div className="flex flex-col gap-2">
          {conversations.map((c) => (
            <div
              key={c.id}
              className={`group rounded-lg px-3 py-2.5 transition-colors ${
                c.id === activeConversationId
                  ? "bg-blush text-terracotta"
                  : "hover:bg-blush/40 text-ink"
              }`}
            >
              <div className="flex items-start justify-between gap-2">
                <button
                  type="button"
                  onClick={() => openConversation(c.id)}
                  className="min-w-0 flex-1 text-left"
                >
                  <div className="truncate text-sm font-semibold">
                    {c.title ??
                      (c.workspace_names.length > 0
                        ? c.workspace_names.join(", ")
                        : "New chat")}
                  </div>
                  <div className="text-xs text-ink-muted">
                    {relativeTime(c.created_at)}
                  </div>
                </button>
                <button
                  type="button"
                  aria-label="Delete chat"
                  disabled={deletingId === c.id}
                  onClick={() => deleteConversation(c.id)}
                  className="shrink-0 rounded-md p-1 text-ink-muted opacity-0 transition-colors hover:bg-blush-dark/40 hover:text-terracotta disabled:opacity-60 group-hover:opacity-100"
                >
                  {deletingId === c.id ? (
                    <Loader2 size={14} className="animate-spin" />
                  ) : (
                    <Trash2 size={14} />
                  )}
                </button>
              </div>
              {c.workspace_names.length > 0 && (
                <div className="mt-1.5 flex flex-wrap gap-1">
                  {c.workspace_names.map((name) => (
                    <span
                      key={name}
                      className="rounded-full bg-white/70 px-2 py-0.5 text-[10px] font-medium text-ink-muted"
                    >
                      {name}
                    </span>
                  ))}
                </div>
              )}
            </div>
          ))}
          {conversations.length === 0 && !picking && (
            <p className="px-1 text-sm text-ink-muted">No conversations yet.</p>
          )}
        </div>
      </div>

      <div className="flex flex-1 flex-col">
        {picking ? (
          <div className="flex-1 overflow-y-auto p-8">
            <div className="mx-auto max-w-xl">
              <h1 className="text-2xl font-extrabold text-ink">
                Start a new chat
              </h1>
              <p className="mt-1 text-sm text-ink-muted">
                Select the libraries this chat should be able to search. You can
                pick none for a general conversation, or several to draw on all
                of them at once.
              </p>

              <div className="mt-6 flex flex-col gap-2">
                {libraries.map((lib) => (
                  <label
                    key={lib.id}
                    className="flex cursor-pointer items-center gap-3 rounded-xl border border-blush/60 bg-white px-4 py-3 text-sm hover:bg-blush/20"
                  >
                    <input
                      type="checkbox"
                      checked={selectedLibraryIds.includes(lib.id)}
                      onChange={() => toggleLibrary(lib.id)}
                      className="h-4 w-4 accent-terracotta"
                    />
                    <span className="font-medium text-ink">{lib.name}</span>
                  </label>
                ))}
                {libraries.length === 0 && (
                  <p className="text-sm text-ink-muted">
                    No libraries yet — you can still start a general chat below.
                  </p>
                )}
              </div>

              {error && <p className="mt-4 text-sm text-red-600">{error}</p>}

              <div className="mt-6 flex items-center gap-3">
                <button
                  type="button"
                  onClick={() => createConversation(selectedLibraryIds)}
                  className="rounded-xl bg-terracotta px-5 py-2.5 text-sm font-semibold text-white shadow-sm transition-colors hover:bg-terracotta-dark"
                >
                  {selectedLibraryIds.length === 0
                    ? "Start general chat"
                    : `Start chat with ${selectedLibraryIds.length} ${selectedLibraryIds.length === 1 ? "library" : "libraries"}`}
                </button>
                {conversations.length > 0 && (
                  <button
                    type="button"
                    onClick={() => openConversation(conversations[0].id)}
                    className="text-sm font-medium text-ink-muted hover:text-ink"
                  >
                    Cancel
                  </button>
                )}
              </div>
            </div>
          </div>
        ) : (
          <>
            {(() => {
              const unattached = libraries.filter(
                (lib) => !activeConversation?.workspace_ids.includes(lib.id),
              );
              const attached = activeConversation?.workspace_names ?? [];
              if (attached.length === 0 && unattached.length === 0) return null;
              return (
                <div className="mx-auto flex w-full max-w-3xl flex-wrap items-center gap-2 px-8 pt-4">
                  {attached.map((name) => (
                    <span
                      key={name}
                      className="rounded-full bg-blush px-2.5 py-1 text-xs font-medium text-terracotta"
                    >
                      {name}
                    </span>
                  ))}
                  {unattached.length > 0 && (
                    <select
                      value=""
                      onChange={(e) => {
                        if (e.target.value) attachLibrary(e.target.value);
                      }}
                      className="rounded-full border border-dashed border-blush-dark bg-transparent px-2.5 py-1 text-xs font-medium text-ink-muted hover:bg-blush/30"
                    >
                      <option value="" disabled>
                        + Attach a library…
                      </option>
                      {unattached.map((lib) => (
                        <option key={lib.id} value={lib.id}>
                          {lib.name}
                        </option>
                      ))}
                    </select>
                  )}
                </div>
              );
            })()}
            <div ref={scrollRef} className="flex-1 overflow-y-auto p-8">
              <div className="mx-auto flex max-w-3xl flex-col gap-4">
                {messages.length === 0 && (
                  <p className="text-sm text-ink-muted">
                    {activeConversation &&
                    activeConversation.workspace_names.length > 0
                      ? `Ask ${activeConversation.workspace_names.join(", ")} a question about its documents.`
                      : "Ask a general question — no library is attached to this chat."}
                  </p>
                )}
                {messages.map((m, i) => {
                  const parts = parseMessageParts(m.content);
                  const isAssistant = m.role !== "user";
                  const isStreaming = sending && i === messages.length - 1;

                  const hasThought =
                    parts.think !== null &&
                    (isStreaming || parts.think.trim().length > 0);
                  const hasContent =
                    (parts.pre && parts.pre.trim().length > 0) ||
                    (parts.post && parts.post.trim().length > 0) ||
                    (isStreaming &&
                      (parts.think === null || parts.thinkClosed));

                  return (
                    <div
                      key={i}
                      className={`flex flex-col ${isAssistant ? "items-start" : "items-end"} mb-4`}
                    >
                      {hasThought && (
                        <details className="group mb-2 w-full max-w-[80%] rounded-lg bg-blush/20 p-3 text-ink-muted shadow-sm transition-all [&_summary::-webkit-details-marker]:hidden">
                          <summary className="flex cursor-pointer items-center gap-1.5 text-xs font-bold uppercase tracking-wider text-terracotta/70 select-none hover:text-terracotta">
                            <ChevronRight
                              size={14}
                              className="transition-transform group-open:rotate-90"
                            />
                            Thought process
                          </summary>
                          <div className="mt-2 text-[11px] leading-relaxed opacity-90 whitespace-pre-wrap">
                            {parts.think || "..."}
                          </div>
                        </details>
                      )}
                      {hasContent && (
                        <div
                          className={`max-w-[80%] rounded-2xl px-5 py-3 text-sm ${
                            m.role === "user"
                              ? "bg-blush text-ink"
                              : "border border-blush/60 bg-white text-ink shadow-sm"
                          }`}
                        >
                          <ReactMarkdown
                            remarkPlugins={[remarkGfm]}
                            components={markdownComponents}
                          >
                            {linkifySources(
                              `${parts.pre}${parts.post}`,
                              m.sources,
                            )}
                          </ReactMarkdown>
                          {!parts.pre && !parts.post && isStreaming && (
                            <TypingWave />
                          )}
                        </div>
                      )}
                      {isAssistant &&
                        !isStreaming &&
                        m.sources &&
                        m.sources.length > 0 && (
                          <SourcesList sources={m.sources} />
                        )}
                    </div>
                  );
                })}
              </div>
            </div>

            {error && <p className="px-8 text-sm text-red-600">{error}</p>}
            {!modelReady && (
              <p className="px-8 text-sm text-ink-muted">
                Model is still loading — chat will be ready in a moment.
              </p>
            )}

            <div className="border-t border-blush/60 p-6">
              <div className="mx-auto flex max-w-3xl items-end gap-3">
                <textarea
                  value={input}
                  onChange={(e) => setInput(e.target.value)}
                  onKeyDown={(e) => {
                    if (e.key === "Enter" && !e.shiftKey) {
                      e.preventDefault();
                      sendMessage();
                    }
                  }}
                  rows={1}
                  placeholder={
                    modelReady ? "Type your inquiry..." : "Model is loading…"
                  }
                  disabled={!modelReady}
                  className="flex-1 resize-none rounded-2xl border border-blush bg-cream px-4 py-3 text-sm text-ink placeholder:text-ink-muted focus:outline-none focus:ring-2 focus:ring-terracotta disabled:opacity-60"
                />
                <button
                  type="button"
                  onClick={sendMessage}
                  disabled={
                    sending ||
                    !input.trim() ||
                    !activeConversationId ||
                    !modelReady
                  }
                  className="flex items-center gap-2 rounded-xl bg-terracotta px-5 py-3 text-sm font-semibold text-white shadow-sm transition-colors hover:bg-terracotta-dark disabled:opacity-60"
                >
                  <Send size={16} />
                  Send
                </button>
              </div>
              <p className="mx-auto mt-2 max-w-3xl text-center text-xs text-ink-muted">
                Dimi runs entirely on this device. Local AI may occasionally be
                wrong — verify anything critical.
              </p>
            </div>
          </>
        )}
      </div>
    </div>
  );
}
