import { useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { open, save } from "@tauri-apps/plugin-dialog";
import {
  AlertCircle,
  ChevronRight,
  Cloud,
  Download,
  File,
  Folder,
  HardDrive,
  LoaderCircle,
  Plus,
  RefreshCw,
  Trash2,
} from "lucide-react";
import { formatStorageBytes, storageProgress } from "../lib/storage";

interface ProviderDescriptor {
  id: string;
  display_name: string;
  stability: "official" | "community" | "experimental";
  implementation_version: string;
  capabilities: {
    browse_files: boolean;
    read_files: boolean;
    object_storage: boolean;
  };
}

interface StorageAccount {
  id: string;
  provider_id: string;
  display_name: string;
  account_subject: string | null;
  auth_state: string;
  enabled: boolean;
  quota_used_bytes: number | null;
  quota_total_bytes: number | null;
  last_error_category: string | null;
  last_error_message: string | null;
  provider_installed: boolean;
  has_credential: boolean;
}

interface RemoteFileItem {
  id: string;
  parent_id: string | null;
  name: string;
  kind: "file" | "folder" | "shortcut";
  media_type: string | null;
  size: number | null;
  modified_at: string | null;
  revision: string | null;
  downloadable: boolean;
}

interface RemoteFilePage {
  items: RemoteFileItem[];
  next_page_token: string | null;
}

interface TransferJob {
  id: string;
  account_id: string;
  operation: string;
  display_name: string;
  status: string;
  bytes_transferred: number;
  bytes_total: number | null;
  error_message: string | null;
  updated_at: string;
}

interface FolderLevel {
  id: string | null;
  name: string;
}

type DriveView = "files" | "transfers";

const STATUS_LABELS: Record<string, string> = {
  disconnected: "未连接",
  authorizing: "授权中",
  connected: "已连接",
  auth_required: "需重新授权",
  error: "异常",
};

const OPERATION_LABELS: Record<string, string> = {
  file_download: "下载",
  knowledge_import: "导入知识库",
  reading_import: "导入书架",
  object_upload: "上传副本",
  object_download: "下载副本",
};

const TRANSFER_STATUS_LABELS: Record<string, string> = {
  queued: "等待中",
  running: "传输中",
  paused: "已暂停",
  completed: "已完成",
  failed: "失败",
  cancelled: "已取消",
};

function formatTimestamp(value: string | null): string {
  if (!value) return "--";
  const numeric = Number(value);
  const date = Number.isFinite(numeric) ? new Date(numeric * 1000) : new Date(value);
  return Number.isNaN(date.getTime()) ? value : date.toLocaleString();
}

export function DriveWorkspace() {
  const [catalog, setCatalog] = useState<ProviderDescriptor[]>([]);
  const [accounts, setAccounts] = useState<StorageAccount[]>([]);
  const [selectedAccountId, setSelectedAccountId] = useState<string | null>(null);
  const [files, setFiles] = useState<RemoteFileItem[]>([]);
  const [nextPageToken, setNextPageToken] = useState<string | null>(null);
  const [folderPath, setFolderPath] = useState<FolderLevel[]>([{ id: null, name: "根目录" }]);
  const [transfers, setTransfers] = useState<TransferJob[]>([]);
  const [view, setView] = useState<DriveView>("files");
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [authorizationMessage, setAuthorizationMessage] = useState<string | null>(null);
  const fileRequestId = useRef(0);

  const selectedAccount = accounts.find((account) => account.id === selectedAccountId) ?? null;
  const selectedProvider = catalog.find(
    (provider) => provider.id === selectedAccount?.provider_id,
  );
  const currentFolder = folderPath[folderPath.length - 1];
  const accountIssue = selectedAccount
    ? !selectedAccount.provider_installed
      ? "当前版本未安装此 Provider adapter"
      : !selectedAccount.enabled
        ? "此网盘账户已在本机停用"
      : !selectedAccount.has_credential
        ? "本机缺少该账户的授权凭证"
        : selectedAccount.last_error_message
    : null;

  const sortedFiles = useMemo(
    () => [...files].sort((left, right) => {
      const leftFolder = left.kind === "folder";
      const rightFolder = right.kind === "folder";
      if (leftFolder !== rightFolder) return leftFolder ? -1 : 1;
      return left.name.localeCompare(right.name, "zh-CN");
    }),
    [files],
  );

  const loadShell = async () => {
    setLoading(true);
    setError(null);
    try {
      const [nextCatalog, nextAccounts, nextTransfers] = await Promise.all([
        invoke<ProviderDescriptor[]>("list_storage_provider_catalog"),
        invoke<StorageAccount[]>("list_storage_accounts"),
        invoke<TransferJob[]>("list_storage_transfers", { accountId: null, limit: 100 }),
      ]);
      setCatalog(nextCatalog);
      setAccounts(nextAccounts);
      setTransfers(nextTransfers);
      setSelectedAccountId((current) =>
        current && nextAccounts.some((account) => account.id === current)
          ? current
          : nextAccounts[0]?.id ?? null,
      );
    } catch (reason) {
      setError(String(reason));
    } finally {
      setLoading(false);
    }
  };

  const loadFiles = async (append: boolean, pageToken: string | null = null) => {
    if (
      !selectedAccountId
      || !selectedAccount?.provider_installed
      || !selectedAccount.enabled
      || selectedAccount.auth_state !== "connected"
    ) {
      fileRequestId.current += 1;
      setFiles([]);
      setNextPageToken(null);
      setLoading(false);
      return;
    }
    const requestId = ++fileRequestId.current;
    setLoading(true);
    setError(null);
    try {
      const page = await invoke<RemoteFilePage>("list_storage_files", {
        accountId: selectedAccountId,
        parentId: currentFolder.id,
        pageToken,
        pageSize: 100,
      });
      if (requestId === fileRequestId.current) {
        setFiles((current) => append ? [...current, ...page.items] : page.items);
        setNextPageToken(page.next_page_token);
      }
    } catch (reason) {
      if (requestId === fileRequestId.current) setError(String(reason));
    } finally {
      if (requestId === fileRequestId.current) setLoading(false);
    }
  };

  useEffect(() => {
    void loadShell();
  }, []);

  useEffect(() => {
    setFolderPath([{ id: null, name: "根目录" }]);
    setFiles([]);
    setNextPageToken(null);
  }, [selectedAccountId]);

  useEffect(() => {
    if (view === "files") void loadFiles(false);
  }, [selectedAccountId, selectedAccount?.auth_state, selectedAccount?.enabled, selectedAccount?.provider_installed, currentFolder.id, view]);

  const refreshQuota = async () => {
    if (!selectedAccountId) return;
    setLoading(true);
    setError(null);
    try {
      await invoke("refresh_storage_quota", { accountId: selectedAccountId });
      await loadShell();
    } catch (reason) {
      setError(String(reason));
    } finally {
      setLoading(false);
    }
  };

  const connectGoogleDrive = async () => {
    try {
      const selected = await open({
        title: "选择 Google Desktop OAuth 客户端文件",
        directory: false,
        multiple: false,
        filters: [{ name: "Google OAuth Client", extensions: ["json"] }],
      });
      if (typeof selected !== "string") return;
      setLoading(true);
      setError(null);
      setAuthorizationMessage("请在系统浏览器中完成 Google 授权");
      const accountId = await invoke<string>("authorize_storage_provider", {
        providerId: "google_drive",
        input: { client_credentials_path: selected },
      });
      await loadShell();
      setSelectedAccountId(accountId);
    } catch (reason) {
      setError(String(reason));
    } finally {
      setAuthorizationMessage(null);
      setLoading(false);
    }
  };

  const removeAccount = async () => {
    if (!selectedAccount) return;
    if (!window.confirm(`移除网盘账户「${selectedAccount.display_name}」？本地传输记录会保留。`)) return;
    setLoading(true);
    setError(null);
    try {
      await invoke("remove_storage_account", { accountId: selectedAccount.id });
      await loadShell();
    } catch (reason) {
      setError(String(reason));
    } finally {
      setLoading(false);
    }
  };

  const downloadFile = async (item: RemoteFileItem) => {
    if (!selectedAccount || !item.downloadable) return;
    try {
      const destination = await save({
        title: "保存网盘文件",
        defaultPath: item.name,
      });
      if (typeof destination !== "string") return;
      setLoading(true);
      setError(null);
      await invoke("download_storage_file", {
        accountId: selectedAccount.id,
        fileId: item.id,
        expectedRevision: item.revision,
        destination,
      });
      await loadShell();
    } catch (reason) {
      setError(String(reason));
    } finally {
      setLoading(false);
    }
  };

  return (
    <main className="flex h-full min-w-0 flex-1 flex-col bg-[#FAF9F5]">
      <header className="flex h-14 shrink-0 items-center justify-between border-b border-stone-200 bg-white/40 px-5 backdrop-blur-md">
        <div className="flex items-center gap-2 text-sm font-semibold text-stone-800">
          <HardDrive className="h-4 w-4 text-[#8CA38A]" />
          网盘
        </div>
        <div className="flex items-center gap-1">
          {selectedAccount && (
            <button
              onClick={refreshQuota}
              disabled={
                loading
                || !selectedAccount.enabled
                || !selectedAccount.provider_installed
                || selectedAccount.auth_state !== "connected"
              }
              className="grid h-8 w-8 place-items-center rounded-md text-stone-500 hover:bg-stone-100 hover:text-stone-900 disabled:opacity-40"
              title="刷新账户状态与配额"
            >
              <RefreshCw className="h-4 w-4" />
            </button>
          )}
          {loading && <LoaderCircle className="h-4 w-4 animate-spin text-stone-400" />}
        </div>
      </header>

      {(error || authorizationMessage || accountIssue) && (
        <div className={`mx-5 mt-3 flex shrink-0 items-start gap-2 rounded-md border px-3 py-2 text-xs ${
          error || accountIssue
            ? "border-rose-200 bg-rose-50 text-rose-700"
            : "border-amber-200 bg-amber-50 text-amber-700"
        }`}>
          {authorizationMessage && !error && !accountIssue
            ? <LoaderCircle className="mt-0.5 h-3.5 w-3.5 shrink-0 animate-spin" />
            : <AlertCircle className="mt-0.5 h-3.5 w-3.5 shrink-0" />}
          <span>{error ?? authorizationMessage ?? accountIssue}</span>
        </div>
      )}

      <div className="flex min-h-0 flex-1">
        <aside className="flex w-64 shrink-0 flex-col border-r border-stone-200 bg-white/30 p-3">
          <div className="mb-2 px-2 text-[10px] font-bold uppercase tracking-wider text-stone-400">
            账户
          </div>
          <div className="space-y-1 overflow-y-auto">
            {accounts.map((account) => (
              <button
                key={account.id}
                onClick={() => setSelectedAccountId(account.id)}
                className={`flex w-full items-center gap-2 rounded-lg px-2.5 py-2 text-left ${
                  account.id === selectedAccountId
                    ? "bg-white text-emerald-700 shadow-sm ring-1 ring-stone-200"
                    : "text-stone-600 hover:bg-stone-100"
                }`}
              >
                <Cloud className="h-4 w-4 shrink-0" />
                <span className="min-w-0 flex-1">
                  <span className="block truncate text-xs font-semibold">{account.display_name}</span>
                  <span className="block truncate text-[10px] text-stone-400">
                    {account.account_subject ?? account.provider_id}
                  </span>
                </span>
                {account.auth_state !== "connected" && <AlertCircle className="h-3.5 w-3.5 shrink-0 text-amber-500" />}
              </button>
            ))}
            {accounts.length === 0 && (
              <div className="px-3 py-8 text-center text-xs text-stone-400">暂无已连接账户</div>
            )}
          </div>
          <button
            onClick={() => void connectGoogleDrive()}
            disabled={loading}
            className="mt-auto flex h-9 w-full items-center justify-center gap-2 rounded-md border border-stone-200 bg-white text-xs font-medium text-stone-600 hover:bg-stone-50 hover:text-stone-900 disabled:opacity-50"
          >
            <Plus className="h-3.5 w-3.5" />
            连接 Google Drive
          </button>
        </aside>

        <section className="flex min-w-0 flex-1 flex-col">
          {selectedAccount ? (
            <>
              <div className="flex shrink-0 items-center justify-between border-b border-stone-200 px-5 py-3">
                <div className="min-w-0">
                  <div className="flex items-center gap-2">
                    <h1 className="truncate text-sm font-semibold text-stone-900">{selectedAccount.display_name}</h1>
                    <span className={`rounded-md px-1.5 py-0.5 text-[10px] ${
                      selectedAccount.auth_state === "connected"
                        ? "bg-emerald-50 text-emerald-700"
                        : "bg-amber-50 text-amber-700"
                    }`}>
                      {STATUS_LABELS[selectedAccount.auth_state] ?? selectedAccount.auth_state}
                    </span>
                    {selectedProvider?.stability === "community" && (
                      <span className="rounded-md bg-stone-100 px-1.5 py-0.5 text-[10px] text-stone-500">社区适配</span>
                    )}
                  </div>
                  <div className="mt-1 text-[11px] text-stone-400">
                    {formatStorageBytes(selectedAccount.quota_used_bytes)} / {formatStorageBytes(selectedAccount.quota_total_bytes)}
                  </div>
                </div>
                <div className="flex items-center gap-2">
                  <div className="flex rounded-lg border border-stone-200 bg-white p-0.5">
                    {(["files", "transfers"] as DriveView[]).map((item) => (
                      <button
                        key={item}
                        onClick={() => setView(item)}
                        className={`h-7 rounded-md px-3 text-[11px] font-medium ${
                          view === item ? "bg-stone-100 text-stone-900" : "text-stone-500"
                        }`}
                      >
                        {item === "files" ? "文件" : "传输"}
                      </button>
                    ))}
                  </div>
                  <button
                    onClick={removeAccount}
                    className="grid h-8 w-8 place-items-center rounded-md text-stone-400 hover:bg-rose-50 hover:text-rose-600"
                    title="移除账户"
                  >
                    <Trash2 className="h-4 w-4" />
                  </button>
                </div>
              </div>

              {view === "files" ? (
                <div className="flex min-h-0 flex-1 flex-col px-5 py-4">
                  <div className="mb-3 flex h-8 items-center gap-1 overflow-x-auto text-xs text-stone-500">
                    {folderPath.map((folder, index) => (
                      <span key={`${folder.id ?? "root"}-${index}`} className="flex shrink-0 items-center gap-1">
                        {index > 0 && <ChevronRight className="h-3.5 w-3.5 text-stone-300" />}
                        <button
                          onClick={() => setFolderPath((path) => path.slice(0, index + 1))}
                          className="rounded-md px-1.5 py-1 hover:bg-stone-100 hover:text-stone-900"
                        >
                          {folder.name}
                        </button>
                      </span>
                    ))}
                  </div>
                  <div className="min-h-0 flex-1 overflow-auto border-y border-stone-200 bg-white/40">
                    {sortedFiles.map((item) => (
                      <div
                        key={item.id}
                        className="grid w-full grid-cols-[minmax(0,1fr)_74px_28px] items-center gap-3 border-b border-stone-100 px-3 py-2 text-left text-xs last:border-b-0 hover:bg-white sm:grid-cols-[minmax(0,1fr)_90px_130px_28px] sm:gap-4"
                      >
                        <button
                          onClick={() => {
                            if (item.kind === "folder") {
                              setFolderPath((path) => [...path, { id: item.id, name: item.name }]);
                            }
                          }}
                          disabled={item.kind !== "folder"}
                          className="flex min-w-0 items-center gap-2 text-left text-stone-700 disabled:cursor-default"
                        >
                          {item.kind === "folder" ? <Folder className="h-4 w-4 shrink-0 text-amber-500" /> : <File className="h-4 w-4 shrink-0 text-stone-400" />}
                          <span className="truncate">{item.name}</span>
                        </button>
                        <span className="text-stone-400">{item.kind === "folder" ? "--" : formatStorageBytes(item.size)}</span>
                        <span className="hidden truncate text-stone-400 sm:block">{formatTimestamp(item.modified_at)}</span>
                        {item.downloadable ? (
                          <button
                            onClick={() => void downloadFile(item)}
                            disabled={loading}
                            className="grid h-7 w-7 place-items-center rounded-md text-stone-400 hover:bg-stone-100 hover:text-stone-800 disabled:opacity-40"
                            title="下载到本地"
                          >
                            <Download className="h-3.5 w-3.5" />
                          </button>
                        ) : <span className="h-7 w-7" />}
                      </div>
                    ))}
                    {!loading && sortedFiles.length === 0 && (
                      <div className="grid h-full min-h-48 place-items-center text-xs text-stone-400">
                        {!selectedAccount.provider_installed
                          ? "Provider adapter 不可用"
                          : !selectedAccount.enabled
                            ? "账户已在本机停用"
                          : selectedAccount.auth_state === "connected"
                            ? "此目录为空"
                            : "账户需要完成授权"}
                      </div>
                    )}
                  </div>
                  {nextPageToken && (
                    <button
                      onClick={() => void loadFiles(true, nextPageToken)}
                      disabled={loading}
                      className="mt-3 self-center rounded-md border border-stone-200 bg-white px-3 py-1.5 text-xs text-stone-600 hover:bg-stone-50 disabled:opacity-50"
                    >
                      加载更多
                    </button>
                  )}
                </div>
              ) : (
                <div className="min-h-0 flex-1 overflow-auto px-5 py-4">
                  <div className="border-y border-stone-200 bg-white/40">
                    {transfers.filter((job) => job.account_id === selectedAccount.id).map((job) => {
                      const progress = storageProgress(job.bytes_transferred, job.bytes_total);
                      return (
                        <div key={job.id} className="grid grid-cols-[minmax(0,1fr)_90px] items-center gap-3 border-b border-stone-100 px-3 py-3 text-xs last:border-b-0 sm:grid-cols-[minmax(0,1fr)_100px_140px] sm:gap-4">
                          <div className="min-w-0">
                            <div className="truncate font-medium text-stone-700">{job.display_name}</div>
                            <div className="mt-1 text-[10px] text-stone-400">{OPERATION_LABELS[job.operation] ?? job.operation}</div>
                            {progress !== null && (
                              <div className="mt-2 h-1 overflow-hidden rounded bg-stone-100">
                                <div className="h-full bg-[#8CA38A]" style={{ width: `${progress}%` }} />
                              </div>
                            )}
                          </div>
                          <span className="text-stone-500">{TRANSFER_STATUS_LABELS[job.status] ?? job.status}</span>
                          <span className="col-span-2 truncate text-stone-400 sm:col-span-1">{job.error_message ?? formatTimestamp(job.updated_at)}</span>
                        </div>
                      );
                    })}
                    {transfers.every((job) => job.account_id !== selectedAccount.id) && (
                      <div className="grid min-h-48 place-items-center text-xs text-stone-400">暂无传输任务</div>
                    )}
                  </div>
                </div>
              )}
            </>
          ) : (
            <div className="grid flex-1 place-items-center text-sm text-stone-400">
              <div className="text-center">
                <Cloud className="mx-auto mb-3 h-8 w-8 text-stone-300" />
                <p>{authorizationMessage ?? "连接 Provider 后在此管理文件和传输任务"}</p>
              </div>
            </div>
          )}
        </section>
      </div>
    </main>
  );
}
