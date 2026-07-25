import { useMemo, useState } from "react";
import { ChevronRight, ChevronDown, Folder, CheckCircle2, Clock, Loader2, XCircle } from "lucide-react";
import { Badge } from "@/components/ui/Card";
import type { DocumentRow } from "@/types";

const STATUS_ICON: Record<DocumentRow["status"], typeof CheckCircle2> = {
  indexed: CheckCircle2,
  pending: Clock,
  parsed: Loader2,
  failed: XCircle,
};

interface TreeNode {
  name: string;
  path: string;
  type: "folder" | "file";
  status?: DocumentRow["status"];
  indexing?: boolean;
  children: TreeNode[];
}

function splitPath(path: string): string[] {
  return path.split(/[\\/]+/).filter(Boolean);
}

function insert(root: TreeNode, segments: string[], status: DocumentRow["status"]) {
  let node = root;
  for (let i = 0; i < segments.length; i++) {
    const segment = segments[i];
    const isLeaf = i === segments.length - 1;
    const path = `${node.path}/${segment}`;
    let child = node.children.find((c) => c.name === segment && c.type === (isLeaf ? "file" : "folder"));
    if (!child) {
      child = { name: segment, path, type: isLeaf ? "file" : "folder", children: [] };
      node.children.push(child);
    }
    if (isLeaf) child.status = status;
    node = child;
  }
}

function sortTree(node: TreeNode) {
  node.children.sort((a, b) => {
    if (a.type !== b.type) return a.type === "folder" ? -1 : 1;
    return a.name.localeCompare(b.name);
  });
  node.children.forEach(sortTree);
}

export interface RepositoryWithDocuments {
  id: string;
  root: string;
  documents: DocumentRow[];
  indexing?: boolean;
}

interface FileTreeProps {
  repositories: RepositoryWithDocuments[];
}

function buildForest(repositories: RepositoryWithDocuments[]): TreeNode[] {
  return repositories.map((repo) => {
    const rootSegments = splitPath(repo.root);
    const rootName = rootSegments[rootSegments.length - 1] ?? repo.root;
    const root: TreeNode = {
      name: rootName,
      path: repo.id,
      type: "folder",
      indexing: repo.indexing,
      children: [],
    };

    for (const doc of repo.documents) {
      const relative = doc.source_path.startsWith(repo.root)
        ? doc.source_path.slice(repo.root.length)
        : doc.source_path;
      const segments = splitPath(relative);
      if (segments.length > 0) insert(root, segments, doc.status);
    }

    sortTree(root);
    return root;
  });
}

function FileRow({ node, depth }: { node: TreeNode; depth: number }) {
  const [collapsed, setCollapsed] = useState(false);
  const indent = { paddingLeft: `${depth * 1.25 + 0.75}rem` };

  if (node.type === "file") {
    const StatusIcon = node.status ? STATUS_ICON[node.status] : Clock;
    return (
      <div className="flex items-center justify-between gap-3 py-1.5 pr-4 text-sm" style={indent}>
        <span className="min-w-0 truncate text-ink">{node.name}</span>
        {node.status && (
          <Badge tone={node.status === "indexed" ? "success" : node.status === "failed" ? "neutral" : "muted"}>
            <StatusIcon size={12} className="mr-1 inline" />
            {node.status}
          </Badge>
        )}
      </div>
    );
  }

  return (
    <div>
      <button
        type="button"
        onClick={() => setCollapsed((c) => !c)}
        className="flex w-full items-center gap-1.5 py-1.5 pr-4 text-left text-sm font-semibold text-ink hover:bg-blush/20"
        style={indent}
      >
        {collapsed ? <ChevronRight size={14} /> : <ChevronDown size={14} />}
        <Folder size={14} className="text-terracotta" />
        <span className="truncate">{node.name}</span>
        <span className="ml-1 text-xs font-normal text-ink-muted">
          {node.children.length === 0 ? "empty" : `${node.children.length} item${node.children.length === 1 ? "" : "s"}`}
        </span>
        {node.indexing && (
          <Badge tone="muted">
            <Loader2 size={12} className="mr-1 inline animate-spin" />
            Indexing…
          </Badge>
        )}
      </button>
      {!collapsed && node.children.map((child) => <FileRow key={child.path} node={child} depth={depth + 1} />)}
    </div>
  );
}

export function FileTree({ repositories }: FileTreeProps) {
  const forest = useMemo(() => buildForest(repositories), [repositories]);

  const isEmpty = forest.every((root) => root.children.length === 0);
  if (repositories.length === 0 || isEmpty) {
    return (
      <div className="px-5 py-8 text-center text-sm text-ink-muted">
        No documents yet — import a folder to get started.
      </div>
    );
  }

  return (
    <div className="py-2">
      {forest.map((root) => (
        <FileRow key={root.path} node={root} depth={0} />
      ))}
    </div>
  );
}
