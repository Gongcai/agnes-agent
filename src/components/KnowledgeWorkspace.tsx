import React, { useEffect, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { open } from "@tauri-apps/plugin-dialog";
import {
  BookOpen,
  FilePlus2,
  Files,
  FolderPlus,
  LoaderCircle,
  Menu,
  Search,
} from "lucide-react";
import { useAgentStore } from "../store/useAgentStore";

interface Collection {
  id: string;
  name: string;
  scope: string;
  permission: string;
  document_count: number;
  updated_at: string;
}

interface DocumentRow {
  id: string;
  collection_id: string;
  title: string;
  media_type: string;
  status: string;
  current_version_id: string | null;
  chunk_count: number;
  updated_at: string;
}

interface SearchResult {
  document_id: string;
  document_version_id: string;
  chunk_id: string;
  title: string;
  ordinal: number;
  section_path: string | null;
  content: string;
}

interface KnowledgeVectorizationResult {
  indexed_now: number;
  model_ref: string;
}

interface KnowledgeWorkspaceProps {
  isSidebarOpen: boolean;
  onToggleSidebar: () => void;
}

export const KnowledgeWorkspace: React.FC<KnowledgeWorkspaceProps> = ({
  isSidebarOpen,
  onToggleSidebar,
}) => {
  const activeAgentId = useAgentStore((state) => state.activeAgentId);
  const [collections, setCollections] = useState<Collection[]>([]);
  const [selectedCollectionId, setSelectedCollectionId] = useState<string | null>(
    null,
  );
  const [documents, setDocuments] = useState<DocumentRow[]>([]);
  const [query, setQuery] = useState("");
  const [results, setResults] = useState<SearchResult[]>([]);
  const [indexStatus, setIndexStatus] = useState<string | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);

  const loadCollections = async () => {
    if (!activeAgentId) {
      setCollections([]);
      setSelectedCollectionId(null);
      return;
    }

    const next = await invoke<Collection[]>("list_knowledge_collections", {
      agentId: activeAgentId,
    });
    setCollections(next);
    setSelectedCollectionId((current) => {
      if (current && next.some((item) => item.id === current)) {
        return current;
      }
      return next[0]?.id ?? null;
    });
  };

  const loadDocuments = async (collectionId: string | null) => {
    if (!activeAgentId || !collectionId) {
      setDocuments([]);
      return;
    }

    const next = await invoke<DocumentRow[]>("list_knowledge_documents", {
      collectionId,
      agentId: activeAgentId,
    });
    setDocuments(next);
  };

  useEffect(() => {
    loadCollections().catch((reason) => setError(String(reason)));
  }, [activeAgentId]);

  useEffect(() => {
    loadDocuments(selectedCollectionId).catch((reason) => setError(String(reason)));
  }, [activeAgentId, selectedCollectionId]);

  const createCollection = async () => {
    if (!activeAgentId) return;

    const name = window.prompt("知识库名称");
    if (!name?.trim()) return;

    setLoading(true);
    setError(null);
    try {
      const id = await invoke<string>("create_knowledge_collection", {
        agentId: activeAgentId,
        name: name.trim(),
        scope: "agent_private",
      });
      await loadCollections();
      setSelectedCollectionId(id);
    } catch (reason) {
      setError(String(reason));
    } finally {
      setLoading(false);
    }
  };

  const importDocuments = async () => {
    if (!activeAgentId || !selectedCollectionId) return;

    const selected = await open({
      multiple: true,
      title: "导入本地文本",
      filters: [
        {
          name: "文本与 Markdown",
          extensions: ["md", "markdown", "txt", "rst", "log", "csv", "json"],
        },
      ],
    });
    const paths = Array.isArray(selected) ? selected : selected ? [selected] : [];
    if (paths.length === 0) return;

    setLoading(true);
    setError(null);
    setIndexStatus(null);
    try {
      for (const path of paths) {
        await invoke("import_local_knowledge_document", {
          collectionId: selectedCollectionId,
          agentId: activeAgentId,
          path,
        });
      }
      await loadCollections();
      await loadDocuments(selectedCollectionId);
    } catch (reason) {
      setError(String(reason));
    } finally {
      setLoading(false);
    }
  };

  const vectorize = async () => {
    if (!activeAgentId || !selectedCollectionId) return;

    setLoading(true);
    setError(null);
    try {
      const result = await invoke<KnowledgeVectorizationResult>(
        "vectorize_knowledge",
        {
          agentId: activeAgentId,
          collectionId: selectedCollectionId,
        },
      );
      setIndexStatus(
        result.indexed_now === 0
          ? `向量索引已是最新状态（${result.model_ref}）`
          : `已建立 ${result.indexed_now} 个向量（${result.model_ref}）`,
      );
    } catch (reason) {
      setError(String(reason));
    } finally {
      setLoading(false);
    }
  };

  const search = async () => {
    if (!activeAgentId || !query.trim()) {
      setResults([]);
      return;
    }

    setLoading(true);
    setError(null);
    try {
      const next = await invoke<SearchResult[]>("search_knowledge_hybrid", {
        agentId: activeAgentId,
        query: query.trim(),
        collectionId: selectedCollectionId,
        limit: 12,
      });
      setResults(next);
    } catch (reason) {
      setError(String(reason));
    } finally {
      setLoading(false);
    }
  };

  const selectedCollection = collections.find(
    (collection) => collection.id === selectedCollectionId,
  );

  return (
    <main className="flex h-full min-w-0 flex-1 flex-col bg-[#FAF9F5]">
      <header className="flex h-14 shrink-0 items-center justify-between border-b border-stone-200 bg-white/40 px-6 backdrop-blur-md">
        <div className="flex items-center gap-3">
          <button
            onClick={onToggleSidebar}
            className="rounded-lg p-1.5 text-stone-500 transition-colors hover:bg-stone-200/40 hover:text-stone-900"
            title={isSidebarOpen ? "收起侧边栏" : "展开侧边栏"}
          >
            <Menu className="h-4 w-4" />
          </button>
          <div className="h-4 w-px bg-stone-200" />
          <div className="flex items-center gap-2 text-sm font-semibold text-stone-800">
            <BookOpen className="h-4 w-4 text-[#8CA38A]" />
            知识库
          </div>
        </div>
        {loading && <LoaderCircle className="h-4 w-4 animate-spin text-stone-400" />}
      </header>

      <div className="flex min-h-0 flex-1">
        <section className="flex w-60 shrink-0 flex-col border-r border-stone-200 bg-white/30 p-3">
          <div className="mb-2 flex items-center justify-between px-1">
            <span className="text-[10px] font-bold uppercase tracking-wider text-stone-400">
              Collections
            </span>
            <button
              onClick={createCollection}
              className="rounded-md p-1 text-stone-500 hover:bg-stone-200/60 hover:text-stone-900"
              title="新建知识库"
            >
              <FolderPlus className="h-4 w-4" />
            </button>
          </div>
          <div className="space-y-1 overflow-y-auto">
            {collections.map((collection) => (
              <button
                key={collection.id}
                onClick={() => {
                  setSelectedCollectionId(collection.id);
                  setResults([]);
                  setIndexStatus(null);
                }}
                className={`flex w-full items-center gap-2 rounded-lg px-2 py-2 text-left text-xs ${
                  collection.id === selectedCollectionId
                    ? "bg-white font-semibold text-emerald-700 shadow-sm"
                    : "text-stone-600 hover:bg-stone-200/40"
                }`}
              >
                <BookOpen className="h-3.5 w-3.5 shrink-0" />
                <span className="min-w-0 flex-1 truncate">{collection.name}</span>
                <span className="text-[10px] text-stone-400">
                  {collection.document_count}
                </span>
              </button>
            ))}
            {collections.length === 0 && (
              <p className="px-2 py-4 text-center text-[11px] leading-relaxed text-stone-400">
                新建知识库后即可导入本地文档。
              </p>
            )}
          </div>
        </section>

        <section className="flex min-w-0 flex-1 flex-col p-6">
          <div className="mx-auto flex w-full max-w-4xl flex-col gap-5">
            <div className="flex items-center justify-between gap-4">
              <div>
                <h1 className="text-lg font-semibold text-stone-900">
                  {selectedCollection?.name ?? "本地知识库"}
                </h1>
                <p className="mt-1 text-xs text-stone-500">
                  本地检索；使用远程模型时，嵌入和召回内容会发送给该模型服务
                </p>
              </div>
              <div className="flex shrink-0 items-center gap-2">
                <button
                  disabled={!selectedCollectionId || loading}
                  onClick={vectorize}
                  className="rounded-xl border border-stone-200 bg-white px-3 py-2 text-xs font-semibold text-stone-700 shadow-sm transition-colors hover:bg-stone-50 disabled:cursor-not-allowed disabled:opacity-50"
                >
                  建立向量索引
                </button>
                <button
                  disabled={!selectedCollectionId || loading}
                  onClick={importDocuments}
                  className="flex items-center gap-2 rounded-xl border border-stone-200 bg-white px-3 py-2 text-xs font-semibold text-stone-700 shadow-sm transition-colors hover:bg-stone-50 disabled:cursor-not-allowed disabled:opacity-50"
                >
                  <FilePlus2 className="h-4 w-4 text-[#8CA38A]" />
                  导入文档
                </button>
              </div>
            </div>

            <form
              onSubmit={(event) => {
                event.preventDefault();
                void search();
              }}
              className="flex gap-2"
            >
              <label className="flex min-w-0 flex-1 items-center gap-2 rounded-xl border border-stone-200 bg-white px-3 shadow-sm">
                <Search className="h-4 w-4 text-stone-400" />
                <input
                  value={query}
                  onChange={(event) => setQuery(event.target.value)}
                  placeholder="检索当前知识库"
                  className="h-10 min-w-0 flex-1 bg-transparent text-sm outline-none placeholder:text-stone-400"
                />
              </label>
              <button
                type="submit"
                disabled={loading}
                className="rounded-xl bg-[#8CA38A] px-4 text-xs font-semibold text-white shadow-sm hover:bg-[#789176] disabled:cursor-not-allowed disabled:opacity-50"
              >
                检索
              </button>
            </form>

            {indexStatus && (
              <div className="rounded-xl border border-emerald-200 bg-emerald-50 px-3 py-2 text-xs text-emerald-700">
                {indexStatus}
              </div>
            )}
            {error && (
              <div className="rounded-xl border border-rose-200 bg-rose-50 px-3 py-2 text-xs text-rose-700">
                {error}
              </div>
            )}

            {results.length > 0 ? (
              <div className="space-y-2">
                {results.map((result) => (
                  <article
                    key={result.chunk_id}
                    className="rounded-xl border border-stone-200 bg-white p-4 shadow-sm"
                  >
                    <div className="mb-2 flex items-center gap-2 text-xs font-semibold text-stone-700">
                      <Files className="h-3.5 w-3.5 text-[#8CA38A]" />
                      {result.title}
                      {result.section_path && (
                        <span className="font-normal text-stone-400">
                          / {result.section_path}
                        </span>
                      )}
                    </div>
                    <p className="whitespace-pre-wrap text-xs leading-relaxed text-stone-600">
                      {result.content}
                    </p>
                  </article>
                ))}
              </div>
            ) : (
              <div className="rounded-2xl border border-dashed border-stone-200 px-6 py-12 text-center text-sm text-stone-400">
                {documents.length > 0
                  ? "输入关键词，检索当前知识库。"
                  : "导入 Markdown、文本、CSV 或 JSON 文件后，即可在本地检索。"}
              </div>
            )}

            {documents.length > 0 && results.length === 0 && (
              <div className="border-t border-stone-200 pt-4">
                <div className="mb-2 text-[10px] font-bold uppercase tracking-wider text-stone-400">
                  已索引文档
                </div>
                <div className="space-y-1">
                  {documents.map((document) => (
                    <div
                      key={document.id}
                      className="flex items-center gap-2 rounded-lg px-2 py-2 text-xs text-stone-600"
                    >
                      <Files className="h-3.5 w-3.5 text-stone-400" />
                      <span className="flex-1 truncate">{document.title}</span>
                      <span className="text-[10px] text-stone-400">
                        {document.chunk_count} chunks
                      </span>
                    </div>
                  ))}
                </div>
              </div>
            )}
          </div>
        </section>
      </div>
    </main>
  );
};
