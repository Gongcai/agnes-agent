import { useEffect, useMemo, useRef, useState } from "react";
import { invoke } from "@tauri-apps/api/core";
import { downloadDir, join } from "@tauri-apps/api/path";
import { open, save } from "@tauri-apps/plugin-dialog";
import * as QRCode from "qrcode";
import {
  AlertCircle,
  ArrowDown,
  ArrowUp,
  ChevronRight,
  Cloud,
  Download,
  Eye,
  EyeOff,
  File,
  Folder,
  FolderDown,
  FolderInput,
  FolderOpen,
  HardDrive,
  LoaderCircle,
  MoreHorizontal,
  Plus,
  RefreshCw,
  Search,
  ShieldAlert,
  Trash2,
  Upload,
  ChevronsUpDown,
  X,
} from "lucide-react";
import {
  remoteTimestampMillis,
  sortDriveItems,
  type DriveSort,
  type DriveSortKey,
} from "../lib/driveSorting";
import {
  formatStorageBytes,
  formatTransferSpeed,
  isKnowledgeImportable,
  isReadingImportable,
  storageProgress,
} from "../lib/storage";
import { useAgentStore } from "../store/useAgentStore";
import { useConfirmDialog } from "./ConfirmDialog";

interface ProviderDescriptor {
  id: string;
  display_name: string;
  stability: "official" | "community" | "experimental";
  implementation_version: string;
  capabilities: {
    browse_files: boolean;
    search_files: boolean;
    read_files: boolean;
    write_files: boolean;
    delete_files: boolean;
    move_files: boolean;
    object_storage: boolean;
    user_authorization: boolean;
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

interface KnowledgeCollection {
  id: string;
  name: string;
  permission: string;
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

interface FileContextMenu {
  item: RemoteFileItem;
  x: number;
  y: number;
}

interface MoveDialogState {
  fileIds: string[];
  sourceFolderId: string | null;
  path: FolderLevel[];
}

type DriveView = "files" | "transfers";
type QuarkAuthorizationMode = "cookie" | "qr";
type TransferSpeedSample = { timestamp: number; bytes: number };

interface StorageAuthorizationChallenge {
  challenge_id: string;
  provider_id: string;
  kind: string;
  payload: { qr_url?: string };
  expires_at: string | null;
}

interface StorageAuthorizationProgress {
  status: string;
  account_id: string | null;
}

const STATUS_LABELS: Record<string, string> = {
  disconnected: "未连接",
  authorizing: "授权中",
  connected: "已连接",
  auth_required: "需重新授权",
  error: "异常",
};

const OPERATION_LABELS: Record<string, string> = {
  file_download: "下载",
  file_upload: "上传",
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
  const timestamp = remoteTimestampMillis(value);
  const date = timestamp === null ? new Date(Number.NaN) : new Date(timestamp);
  return Number.isNaN(date.getTime()) ? value : date.toLocaleString();
}

function SortableFileHeader({
  label,
  sortKey,
  currentSort,
  onSort,
  className = "",
}: {
  label: string;
  sortKey: DriveSortKey;
  currentSort: DriveSort;
  onSort: (key: DriveSortKey) => void;
  className?: string;
}) {
  const active = currentSort.key === sortKey;
  const nextDirection = active && currentSort.direction === "asc" ? "降序" : "升序";
  const SortIcon = !active
    ? ChevronsUpDown
    : currentSort.direction === "asc"
      ? ArrowUp
      : ArrowDown;
  return (
    <button
      type="button"
      onClick={() => onSort(sortKey)}
      className={`group/sort flex min-w-0 items-center gap-1 text-left transition-colors hover:text-stone-700 ${active ? "text-stone-600" : ""} ${className}`}
      aria-label={`${label}${active ? `，当前${currentSort.direction === "asc" ? "升序" : "降序"}` : ""}，点击按${nextDirection}排列`}
    >
      <span className="truncate">{label}</span>
      <SortIcon className={`h-3 w-3 shrink-0 ${active ? "opacity-100" : "opacity-50 transition-opacity group-hover/sort:opacity-100"}`} />
    </button>
  );
}

export function DriveWorkspace() {
  const confirmDelete = useConfirmDialog();
  const activeAgentId = useAgentStore((state) => state.activeAgentId);
  const [catalog, setCatalog] = useState<ProviderDescriptor[]>([]);
  const [accounts, setAccounts] = useState<StorageAccount[]>([]);
  const [selectedAccountId, setSelectedAccountId] = useState<string | null>(null);
  const [files, setFiles] = useState<RemoteFileItem[]>([]);
  const [nextPageToken, setNextPageToken] = useState<string | null>(null);
  const [folderPath, setFolderPath] = useState<FolderLevel[]>([{ id: null, name: "根目录" }]);
  const [transfers, setTransfers] = useState<TransferJob[]>([]);
  const [transferSpeeds, setTransferSpeeds] = useState<Record<string, number>>({});
  const [view, setView] = useState<DriveView>("files");
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState<string | null>(null);
  const [authorizationMessage, setAuthorizationMessage] = useState<string | null>(null);
  const [fileContextMenu, setFileContextMenu] = useState<FileContextMenu | null>(null);
  const [showQuarkAuthorization, setShowQuarkAuthorization] = useState(false);
  const [quarkAuthorizationMode, setQuarkAuthorizationMode] = useState<QuarkAuthorizationMode>("cookie");
  const [quarkCookie, setQuarkCookie] = useState("");
  const [quarkCookieJsonPath, setQuarkCookieJsonPath] = useState<string | null>(null);
  const [showQuarkCookie, setShowQuarkCookie] = useState(false);
  const [quarkQrChallengeId, setQuarkQrChallengeId] = useState<string | null>(null);
  const [quarkQrImage, setQuarkQrImage] = useState<string | null>(null);
  const [quarkQrStatus, setQuarkQrStatus] = useState<string | null>(null);
  const [quarkQrLoading, setQuarkQrLoading] = useState(false);
  const [selectedFileIds, setSelectedFileIds] = useState<Set<string>>(new Set());
  const [fileSort, setFileSort] = useState<DriveSort>({ key: "name", direction: "asc" });
  const [fileSearchQuery, setFileSearchQuery] = useState("");
  const [searchResults, setSearchResults] = useState<RemoteFileItem[]>([]);
  const [searchNextPageToken, setSearchNextPageToken] = useState<string | null>(null);
  const [searchLoading, setSearchLoading] = useState(false);
  const [knowledgeCollections, setKnowledgeCollections] = useState<KnowledgeCollection[]>([]);
  const [knowledgeImportItem, setKnowledgeImportItem] = useState<RemoteFileItem | null>(null);
  const [moveDialog, setMoveDialog] = useState<MoveDialogState | null>(null);
  const [moveFolders, setMoveFolders] = useState<RemoteFileItem[]>([]);
  const [moveNextPageToken, setMoveNextPageToken] = useState<string | null>(null);
  const [moveLoading, setMoveLoading] = useState(false);
  const [moveSubmitting, setMoveSubmitting] = useState(false);
  const fileRequestId = useRef(0);
  const searchRequestId = useRef(0);
  const moveRequestId = useRef(0);
  const transferSpeedSamples = useRef(new Map<string, TransferSpeedSample>());

  const selectedAccount = accounts.find((account) => account.id === selectedAccountId) ?? null;
  const selectedProvider = catalog.find(
    (provider) => provider.id === selectedAccount?.provider_id,
  );
  const currentFolder = folderPath[folderPath.length - 1];
  const moveTargetIsSource = moveDialog
    ? moveDialog.path[moveDialog.path.length - 1].id === moveDialog.sourceFolderId
    : false;
  const accountIssue = selectedAccount
    ? !selectedAccount.provider_installed
      ? "当前版本未安装此 Provider adapter"
      : !selectedAccount.enabled
        ? "此网盘账户已在本机停用"
      : !selectedAccount.has_credential
        ? "本机缺少该账户的授权凭证"
        : selectedAccount.last_error_message
    : null;

  const normalizedSearchQuery = fileSearchQuery.trim();
  const displayedFiles = normalizedSearchQuery ? searchResults : files;
  const sortedFiles = useMemo(
    () => sortDriveItems(displayedFiles, fileSort),
    [displayedFiles, fileSort],
  );
  const fileListLoading = loading || searchLoading;
  const visibleNextPageToken = normalizedSearchQuery ? searchNextPageToken : nextPageToken;
  const selectedFileCount = selectedFileIds.size;
  const allFilesSelected = sortedFiles.length > 0 && sortedFiles.every((item) => selectedFileIds.has(item.id));
  const changeFileSort = (key: DriveSortKey) => {
    setFileSort((current) => {
      if (current.key === key) {
        return { key, direction: current.direction === "asc" ? "desc" : "asc" };
      }
      return { key, direction: key === "name" ? "asc" : "desc" };
    });
  };

  const loadShell = async () => {
    setLoading(true);
    setError(null);
    try {
      const [nextCatalog, nextAccounts, nextTransfers] = await Promise.all([
        invoke<ProviderDescriptor[]>("list_storage_provider_catalog"),
        invoke<StorageAccount[]>("list_storage_accounts"),
        invoke<TransferJob[]>("list_storage_transfers", { accountId: null, limit: 100 }),
      ]);
      const fileProviderIds = new Set(
        nextCatalog
          .filter((provider) => provider.capabilities.browse_files)
          .map((provider) => provider.id),
      );
      const fileAccounts = nextAccounts.filter((account) =>
        fileProviderIds.has(account.provider_id),
      );
      setCatalog(nextCatalog);
      setAccounts(fileAccounts);
      setTransfers(nextTransfers);
      setSelectedAccountId((current) =>
        current && fileAccounts.some((account) => account.id === current)
          ? current
          : fileAccounts[0]?.id ?? null,
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

  const loadSearchFiles = async (
    query: string,
    append: boolean,
    pageToken: string | null = null,
  ) => {
    if (
      !selectedAccountId
      || !selectedProvider?.capabilities.search_files
      || !selectedAccount?.provider_installed
      || !selectedAccount.enabled
      || selectedAccount.auth_state !== "connected"
    ) {
      searchRequestId.current += 1;
      setSearchResults([]);
      setSearchNextPageToken(null);
      setSearchLoading(false);
      return;
    }
    const requestId = ++searchRequestId.current;
    setSearchLoading(true);
    setError(null);
    if (!append) {
      setSearchResults([]);
      setSearchNextPageToken(null);
    }
    try {
      const page = await invoke<RemoteFilePage>("search_storage_files", {
        accountId: selectedAccountId,
        query,
        pageToken,
        pageSize: 100,
      });
      if (requestId === searchRequestId.current) {
        setSearchResults((current) => append ? [...current, ...page.items] : page.items);
        setSearchNextPageToken(page.next_page_token);
      }
    } catch (reason) {
      if (requestId === searchRequestId.current) setError(String(reason));
    } finally {
      if (requestId === searchRequestId.current) setSearchLoading(false);
    }
  };

  useEffect(() => {
    void loadShell();
  }, []);

  useEffect(() => {
    if (!activeAgentId) {
      setKnowledgeCollections([]);
      return;
    }
    void invoke<KnowledgeCollection[]>("list_knowledge_collections", {
      agentId: activeAgentId,
    })
      .then(setKnowledgeCollections)
      .catch((reason) => setError(String(reason)));
  }, [activeAgentId]);

  useEffect(() => {
    setFolderPath([{ id: null, name: "根目录" }]);
    setFiles([]);
    setNextPageToken(null);
    setFileSearchQuery("");
    setSearchResults([]);
    setSearchNextPageToken(null);
    setSearchLoading(false);
    searchRequestId.current += 1;
    setSelectedFileIds(new Set());
    setMoveDialog(null);
    moveRequestId.current += 1;
  }, [selectedAccountId]);

  useEffect(() => {
    setSelectedFileIds(new Set());
  }, [currentFolder.id]);

  useEffect(() => {
    if (view === "files") void loadFiles(false);
  }, [selectedAccountId, selectedAccount?.auth_state, selectedAccount?.enabled, selectedAccount?.provider_installed, currentFolder.id, view]);

  useEffect(() => {
    searchRequestId.current += 1;
    setSelectedFileIds(new Set());
    setSearchResults([]);
    setSearchNextPageToken(null);
    if (!normalizedSearchQuery || view !== "files") {
      setSearchLoading(false);
      return;
    }
    setSearchLoading(true);
    const timer = window.setTimeout(() => {
      void loadSearchFiles(normalizedSearchQuery, false);
    }, 300);
    return () => window.clearTimeout(timer);
  }, [normalizedSearchQuery, selectedAccountId, selectedAccount?.auth_state, selectedAccount?.enabled, selectedAccount?.provider_installed, selectedProvider?.capabilities.search_files, view]);

  useEffect(() => {
    if (!fileContextMenu) return;
    const closeOnEscape = (event: KeyboardEvent) => {
      if (event.key === "Escape") setFileContextMenu(null);
    };
    window.addEventListener("keydown", closeOnEscape);
    return () => window.removeEventListener("keydown", closeOnEscape);
  }, [fileContextMenu]);

  useEffect(() => {
    let cancelled = false;
    let polling = false;
    const refreshTransfers = async () => {
      if (polling) return;
      polling = true;
      try {
        const next = await invoke<TransferJob[]>("list_storage_transfers", {
          accountId: null,
          limit: 100,
        });
        if (!cancelled) {
          const timestamp = Date.now();
          setTransferSpeeds((current) => {
            const speeds = { ...current };
            const activeIds = new Set(next.map((job) => job.id));
            for (const job of next) {
              const previous = transferSpeedSamples.current.get(job.id);
              if (previous && job.status === "running") {
                const elapsed = (timestamp - previous.timestamp) / 1000;
                const delta = job.bytes_transferred - previous.bytes;
                if (elapsed > 0 && delta >= 0) speeds[job.id] = delta / elapsed;
              } else if (job.status !== "running") {
                delete speeds[job.id];
              }
              transferSpeedSamples.current.set(job.id, {
                timestamp,
                bytes: job.bytes_transferred,
              });
            }
            for (const id of transferSpeedSamples.current.keys()) {
              if (!activeIds.has(id)) {
                transferSpeedSamples.current.delete(id);
                delete speeds[id];
              }
            }
            return speeds;
          });
          setTransfers(next);
        }
      } catch (reason) {
        if (!cancelled) setError(String(reason));
      } finally {
        polling = false;
      }
    };
    void refreshTransfers();
    const timer = window.setInterval(() => void refreshTransfers(), 750);
    return () => {
      cancelled = true;
      window.clearInterval(timer);
      transferSpeedSamples.current.clear();
    };
  }, []);

  useEffect(() => {
    if (!showQuarkAuthorization || quarkAuthorizationMode !== "qr" || !quarkQrChallengeId) return;
    let cancelled = false;
    let polling = false;
    const poll = async () => {
      if (polling) return;
      polling = true;
      try {
        const result = await invoke<StorageAuthorizationProgress>(
          "poll_storage_provider_authorization",
          { providerId: "quark_drive", challengeId: quarkQrChallengeId },
        );
        if (cancelled) return;
        if (result.status === "completed" && result.account_id) {
          setQuarkQrStatus("登录成功，正在加载账户");
          setQuarkQrChallengeId(null);
          await loadShell();
          if (!cancelled) {
            setSelectedAccountId(result.account_id);
            closeQuarkAuthorization();
          }
          return;
        }
        setQuarkQrStatus("等待扫码，扫码后请在手机上确认登录");
      } catch (reason) {
        if (!cancelled) {
          const message = String(reason);
          setQuarkQrStatus(`二维码登录检查失败：${message}`);
          if (message.includes("过期") || message.includes("不存在")) {
            setQuarkQrChallengeId(null);
            setQuarkQrImage(null);
          }
        }
      } finally {
        polling = false;
      }
    };
    void poll();
    const timer = window.setInterval(() => void poll(), 2000);
    return () => {
      cancelled = true;
      window.clearInterval(timer);
    };
  }, [showQuarkAuthorization, quarkAuthorizationMode, quarkQrChallengeId]);

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

  const connectQuarkDrive = async () => {
    const cookie = quarkCookie.trim();
    if (!cookie && !quarkCookieJsonPath) return;
    try {
      setLoading(true);
      setError(null);
      setAuthorizationMessage("正在验证夸克网盘 Cookie");
      const accountId = await invoke<string>("authorize_storage_provider", {
        providerId: "quark_drive",
        input: quarkCookieJsonPath ? { cookie_json_path: quarkCookieJsonPath } : { cookie },
      });
      setQuarkCookie("");
      setQuarkCookieJsonPath(null);
      setShowQuarkCookie(false);
      setShowQuarkAuthorization(false);
      await loadShell();
      setSelectedAccountId(accountId);
    } catch (reason) {
      setError(String(reason));
    } finally {
      setAuthorizationMessage(null);
      setLoading(false);
    }
  };

  const selectQuarkCookieJson = async () => {
    const selected = await open({
      title: "选择夸克网盘 Cookie JSON",
      directory: false,
      multiple: false,
      filters: [{ name: "Cookie JSON", extensions: ["json"] }],
    });
    if (typeof selected === "string") setQuarkCookieJsonPath(selected);
  };

  const startQuarkQrLogin = async () => {
    try {
      setQuarkQrLoading(true);
      setQuarkQrImage(null);
      setQuarkQrChallengeId(null);
      setQuarkQrStatus("正在获取二维码...");
      setError(null);
      const challenge = await invoke<StorageAuthorizationChallenge>(
        "begin_storage_provider_authorization",
        { providerId: "quark_drive", input: { method: "qr" } },
      );
      const qrUrl = challenge.payload.qr_url;
      if (!qrUrl) throw new Error("夸克未返回二维码地址");
      const image = await QRCode.toDataURL(qrUrl, {
        errorCorrectionLevel: "M",
        margin: 2,
        width: 240,
      });
      setQuarkQrImage(image);
      setQuarkQrChallengeId(challenge.challenge_id);
      setQuarkQrStatus("请使用夸克 App 扫描二维码");
    } catch (reason) {
      setQuarkQrStatus("二维码获取失败，请重试");
      setError(String(reason));
    } finally {
      setQuarkQrLoading(false);
    }
  };

  const closeQuarkAuthorization = () => {
    setQuarkCookie("");
    setQuarkCookieJsonPath(null);
    setQuarkQrChallengeId(null);
    setQuarkQrImage(null);
    setQuarkQrStatus(null);
    setQuarkAuthorizationMode("cookie");
    setShowQuarkAuthorization(false);
  };

  const connectProvider = (providerId: string) => {
    if (providerId === "google_drive") {
      void connectGoogleDrive();
      return;
    }
    if (providerId === "quark_drive") {
      setError(null);
      setQuarkAuthorizationMode("cookie");
      setQuarkQrChallengeId(null);
      setQuarkQrImage(null);
      setShowQuarkAuthorization(true);
    }
  };

  const removeAccount = async () => {
    if (!selectedAccount) return;
    if (!await confirmDelete({
      title: `移除网盘账户「${selectedAccount.display_name}」？`,
      description: "账户授权会从本机移除，本地传输记录仍会保留。",
      confirmLabel: "移除账户",
    })) return;
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
      const safeName = item.name.replace(/[\\/\u0000]/g, "_").trim() || "download";
      const defaultPath = await downloadDir()
        .then((directory) => join(directory, safeName))
        .catch(() => safeName);
      const destination = await save({
        title: "保存网盘文件",
        defaultPath,
      });
      if (typeof destination !== "string") return;
      setView("transfers");
      setLoading(true);
      setError(null);
      await invoke("download_storage_file", {
        accountId: selectedAccount.id,
        fileId: item.id,
        expectedRevision: item.revision,
        expectedSize: item.size,
        destination,
      });
      await loadShell();
    } catch (reason) {
      setError(String(reason));
    } finally {
      setLoading(false);
    }
  };

  const openKnowledgeImport = (item: RemoteFileItem) => {
    setFileContextMenu(null);
    if (!activeAgentId || !isKnowledgeImportable(item)) return;
    const writableCollections = knowledgeCollections.filter(
      (collection) => collection.permission === "write" || collection.permission === "manage",
    );
    if (writableCollections.length === 0) {
      setError("当前 Agent 没有可写知识库，请先在知识库页面创建集合");
      return;
    }
    setKnowledgeImportItem(item);
  };

  const importToKnowledge = async (collectionId: string) => {
    if (!selectedAccount || !activeAgentId || !knowledgeImportItem) return;
    const item = knowledgeImportItem;
    setKnowledgeImportItem(null);
    setView("transfers");
    setLoading(true);
    setError(null);
    try {
      await invoke("import_storage_knowledge_document", {
        accountId: selectedAccount.id,
        fileId: item.id,
        fileName: item.name,
        fileMediaType: item.media_type,
        expectedRevision: item.revision,
        expectedSize: item.size,
        collectionId,
        agentId: activeAgentId,
      });
      await loadShell();
    } catch (reason) {
      setError(String(reason));
    } finally {
      setLoading(false);
    }
  };

  const importToReading = async (item: RemoteFileItem) => {
    if (!selectedAccount || !activeAgentId || !isReadingImportable(item)) return;
    setFileContextMenu(null);
    setView("transfers");
    setLoading(true);
    setError(null);
    try {
      await invoke("import_storage_reading_book", {
        accountId: selectedAccount.id,
        fileId: item.id,
        fileName: item.name,
        fileMediaType: item.media_type,
        expectedRevision: item.revision,
        expectedSize: item.size,
        agentId: activeAgentId,
      });
      await loadShell();
    } catch (reason) {
      setError(String(reason));
    } finally {
      setLoading(false);
    }
  };

  const openFolder = (item: RemoteFileItem) => {
    if (item.kind !== "folder") return;
    if (normalizedSearchQuery) {
      setFileSearchQuery("");
      setFolderPath([{ id: null, name: "根目录" }, { id: item.id, name: item.name }]);
    } else {
      setFolderPath((path) => [...path, { id: item.id, name: item.name }]);
    }
  };

  const refreshVisibleFiles = async () => {
    if (normalizedSearchQuery) {
      await loadSearchFiles(normalizedSearchQuery, false);
    } else {
      await loadFiles(false);
    }
  };

  const downloadFolder = async (item: RemoteFileItem) => {
    if (!selectedAccount || item.kind !== "folder") return;
    try {
      const destinationDirectory = await open({
        title: "选择文件夹下载位置",
        directory: true,
        multiple: false,
      });
      if (typeof destinationDirectory !== "string") return;
      setView("transfers");
      setLoading(true);
      setError(null);
      await invoke("download_storage_folder", {
        accountId: selectedAccount.id,
        folderId: item.id,
        folderName: item.name,
        destinationDirectory,
      });
      await loadShell();
    } catch (reason) {
      setError(String(reason));
    } finally {
      setLoading(false);
    }
  };

  const uploadFiles = async () => {
    if (!selectedAccount || !selectedProvider?.capabilities.write_files) return;
    try {
      const selected = await open({
        title: "上传文件到当前目录",
        directory: false,
        multiple: true,
      });
      const paths = Array.isArray(selected) ? selected : selected ? [selected] : [];
      if (paths.length === 0) return;
      setView("transfers");
      setLoading(true);
      setError(null);
      for (const source of paths) {
        await invoke("upload_storage_file", {
          accountId: selectedAccount.id,
          parentId: currentFolder.id,
          source,
        });
      }
      await loadShell();
      await refreshVisibleFiles();
    } catch (reason) {
      setError(String(reason));
    } finally {
      setLoading(false);
    }
  };

  const setFileSelected = (fileId: string, selected: boolean) => {
    setSelectedFileIds((current) => {
      const next = new Set(current);
      if (selected) next.add(fileId);
      else next.delete(fileId);
      return next;
    });
  };

  const toggleAllFiles = (selected: boolean) => {
    setSelectedFileIds(selected ? new Set(sortedFiles.map((item) => item.id)) : new Set());
  };

  const loadMoveFolders = async (
    dialog: MoveDialogState,
    append: boolean,
    pageToken: string | null = null,
  ) => {
    if (!selectedAccount) return;
    const requestId = ++moveRequestId.current;
    const target = dialog.path[dialog.path.length - 1];
    setMoveLoading(true);
    setError(null);
    if (!append) {
      setMoveFolders([]);
      setMoveNextPageToken(null);
    }
    try {
      const page = await invoke<RemoteFilePage>("list_storage_files", {
        accountId: selectedAccount.id,
        parentId: target.id,
        pageToken,
        pageSize: 100,
      });
      if (requestId !== moveRequestId.current) return;
      const movingIds = new Set(dialog.fileIds);
      const folders = page.items.filter(
        (item) => item.kind === "folder" && !movingIds.has(item.id),
      );
      setMoveFolders((current) => append ? [...current, ...folders] : folders);
      setMoveNextPageToken(page.next_page_token);
    } catch (reason) {
      if (requestId === moveRequestId.current) setError(String(reason));
    } finally {
      if (requestId === moveRequestId.current) setMoveLoading(false);
    }
  };

  const openMoveDialog = (fileIds: string[]) => {
    if (!selectedProvider?.capabilities.move_files) return;
    const uniqueIds = [...new Set(fileIds.map((id) => id.trim()).filter(Boolean))];
    if (uniqueIds.length === 0) return;
    if (uniqueIds.length > 100) {
      setError("一次最多移动 100 个项目");
      return;
    }
    const dialog = {
      fileIds: uniqueIds,
      sourceFolderId: currentFolder.id,
      path: [{ id: null, name: "根目录" }],
    } satisfies MoveDialogState;
    setFileContextMenu(null);
    setMoveDialog(dialog);
    void loadMoveFolders(dialog, false);
  };

  const openMoveFolder = (folder: RemoteFileItem) => {
    if (!moveDialog || folder.kind !== "folder") return;
    const nextDialog = {
      ...moveDialog,
      path: [...moveDialog.path, { id: folder.id, name: folder.name }],
    };
    setMoveDialog(nextDialog);
    void loadMoveFolders(nextDialog, false);
  };

  const openMoveBreadcrumb = (index: number) => {
    if (!moveDialog) return;
    const nextDialog = { ...moveDialog, path: moveDialog.path.slice(0, index + 1) };
    setMoveDialog(nextDialog);
    void loadMoveFolders(nextDialog, false);
  };

  const closeMoveDialog = () => {
    if (moveSubmitting) return;
    moveRequestId.current += 1;
    setMoveDialog(null);
    setMoveFolders([]);
    setMoveNextPageToken(null);
    setMoveLoading(false);
  };

  const moveFiles = async () => {
    if (!selectedAccount || !moveDialog || !selectedProvider?.capabilities.move_files) return;
    const targetFolderId = moveDialog.path[moveDialog.path.length - 1].id;
    if (targetFolderId === moveDialog.sourceFolderId) return;
    try {
      setMoveSubmitting(true);
      setError(null);
      await invoke("move_storage_files", {
        accountId: selectedAccount.id,
        fileIds: moveDialog.fileIds,
        targetFolderId,
      });
      setMoveDialog(null);
      setSelectedFileIds(new Set());
      await loadShell();
      await refreshVisibleFiles();
    } catch (reason) {
      setError(String(reason));
    } finally {
      setMoveSubmitting(false);
    }
  };

  const moveFilesToTrash = async (fileIds: string[]) => {
    if (!selectedAccount || !selectedProvider?.capabilities.delete_files || fileIds.length === 0) return;
    const count = fileIds.length;
    if (!await confirmDelete({
      title: `将 ${count} 个项目移入回收站？`,
      description: "这些项目会从当前目录移除，可在网盘网页端恢复。",
      confirmLabel: "移入回收站",
    })) return;
    try {
      setLoading(true);
      setError(null);
      await invoke("trash_storage_files", {
        accountId: selectedAccount.id,
        fileIds,
      });
      setSelectedFileIds(new Set());
      setFileContextMenu(null);
      await loadShell();
      await refreshVisibleFiles();
    } catch (reason) {
      setError(String(reason));
    } finally {
      setLoading(false);
    }
  };

  const openFileContextMenu = (event: React.MouseEvent, item: RemoteFileItem) => {
    event.preventDefault();
    event.stopPropagation();
    if (!selectedFileIds.has(item.id)) setSelectedFileIds(new Set([item.id]));
    const menuWidth = 196;
    const menuHeight = item.kind === "folder" ? 180 : 300;
    setFileContextMenu({
      item,
      x: Math.max(8, Math.min(event.clientX, window.innerWidth - menuWidth - 8)),
      y: Math.max(8, Math.min(event.clientY, window.innerHeight - menuHeight - 8)),
    });
  };

  const contextFileIds = fileContextMenu
    ? selectedFileIds.has(fileContextMenu.item.id)
      ? [...selectedFileIds]
      : [fileContextMenu.item.id]
    : [];

  return (
    <main className="agnes-feature-workspace agnes-drive-workspace flex h-full min-w-0 flex-1 flex-col bg-[#FAF9F5]">
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
        <aside className="agnes-subnav flex w-64 shrink-0 flex-col border-r border-stone-200 bg-white/30 p-3">
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
          <div className="mt-auto space-y-2">
            {catalog.filter((provider) => provider.capabilities.user_authorization).map((provider) => (
              <button
                key={provider.id}
                onClick={() => connectProvider(provider.id)}
                disabled={loading}
                className="flex min-h-9 w-full items-center justify-center gap-2 rounded-md border border-stone-200 bg-white px-2 py-2 text-xs font-medium text-stone-600 hover:bg-stone-50 hover:text-stone-900 disabled:opacity-50"
              >
                <Plus className="h-3.5 w-3.5 shrink-0" />
                <span>连接/重新授权 {provider.display_name}</span>
              </button>
            ))}
          </div>
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
                  <div className="mb-3 flex min-h-8 flex-wrap items-center gap-2 text-xs text-stone-500">
                    <div className="mr-auto flex min-w-40 flex-1 items-center gap-1 overflow-x-auto">
                      {folderPath.map((folder, index) => (
                        <span key={`${folder.id ?? "root"}-${index}`} className="flex shrink-0 items-center gap-1">
                          {index > 0 && <ChevronRight className="h-3.5 w-3.5 text-stone-300" />}
                          <button
                            onClick={() => {
                              setFileSearchQuery("");
                              setFolderPath((path) => path.slice(0, index + 1));
                            }}
                            className="rounded-md px-1.5 py-1 hover:bg-stone-100 hover:text-stone-900"
                          >
                            {folder.name}
                          </button>
                        </span>
                      ))}
                    </div>
                    <div className="flex shrink-0 items-center gap-2">
                      <div className="relative w-40 sm:w-56">
                        <Search className="pointer-events-none absolute left-2.5 top-1/2 h-3.5 w-3.5 -translate-y-1/2 text-stone-400" />
                        <input
                          type="text"
                          inputMode="search"
                          value={fileSearchQuery}
                          onChange={(event) => setFileSearchQuery(event.target.value)}
                          onKeyDown={(event) => {
                            if (event.key === "Escape") setFileSearchQuery("");
                          }}
                          maxLength={200}
                          disabled={!selectedProvider?.capabilities.search_files}
                          placeholder="全盘搜索"
                          aria-label="全盘搜索文件"
                          className="h-8 w-full rounded-md border border-stone-200 bg-white pl-8 pr-8 text-xs text-stone-700 outline-none placeholder:text-stone-400 focus:border-stone-400 disabled:cursor-not-allowed disabled:opacity-50"
                        />
                        {searchLoading ? (
                          <LoaderCircle className="pointer-events-none absolute right-2.5 top-1/2 h-3.5 w-3.5 -translate-y-1/2 animate-spin text-stone-400" />
                        ) : fileSearchQuery ? (
                          <button
                            type="button"
                            onClick={() => setFileSearchQuery("")}
                            className="absolute right-1.5 top-1/2 grid h-5 w-5 -translate-y-1/2 place-items-center rounded text-stone-400 hover:bg-stone-100 hover:text-stone-700"
                            title="清除搜索"
                          >
                            <X className="h-3 w-3" />
                          </button>
                        ) : null}
                      </div>
                      <button
                        onClick={() => void uploadFiles()}
                        disabled={loading || !selectedProvider?.capabilities.write_files}
                        className="grid h-8 w-8 shrink-0 place-items-center rounded-md text-stone-500 hover:bg-white hover:text-stone-900 disabled:opacity-35"
                        title={selectedProvider?.capabilities.write_files ? "上传文件" : "当前 Provider 不支持上传"}
                      >
                        <Upload className="h-4 w-4" />
                      </button>
                    </div>
                  </div>
                  <div
                    className="min-h-0 flex-1 overflow-auto border-y border-stone-200 bg-white/40"
                    onContextMenu={(event) => event.preventDefault()}
                  >
                    <div className="sticky top-0 z-10 grid grid-cols-[28px_minmax(0,1fr)_74px_56px] items-center gap-3 border-b border-stone-200 bg-[#FAF9F5]/95 px-3 py-2 text-[11px] font-medium text-stone-400 backdrop-blur-sm sm:grid-cols-[28px_minmax(0,1fr)_90px_130px_56px] sm:gap-4">
                      <input
                        type="checkbox"
                        aria-label={normalizedSearchQuery ? "全选搜索结果" : "全选当前目录"}
                        checked={allFilesSelected}
                        ref={(element) => {
                          if (element) element.indeterminate = !allFilesSelected && selectedFileCount > 0;
                        }}
                        onChange={(event) => toggleAllFiles(event.target.checked)}
                        disabled={fileListLoading || sortedFiles.length === 0}
                        className="h-3.5 w-3.5 accent-emerald-700"
                      />
                      <div className="flex min-w-0 items-center gap-2">
                        <SortableFileHeader label="名称" sortKey="name" currentSort={fileSort} onSort={changeFileSort} />
                        {selectedFileCount > 0 && <span className="shrink-0 text-[10px] text-stone-400">已选 {selectedFileCount} 项</span>}
                      </div>
                      <SortableFileHeader label="大小" sortKey="size" currentSort={fileSort} onSort={changeFileSort} />
                      <SortableFileHeader label="修改时间" sortKey="modified" currentSort={fileSort} onSort={changeFileSort} className="hidden sm:flex" />
                      {selectedFileCount > 0 ? (
                        <div className="flex items-center justify-end">
                          {selectedProvider?.capabilities.move_files && (
                            <button
                              onClick={() => openMoveDialog([...selectedFileIds])}
                              disabled={loading}
                              className="grid h-7 w-7 place-items-center rounded-md text-stone-400 hover:bg-stone-100 hover:text-stone-800 disabled:opacity-40"
                              title="移动选中项目"
                            >
                              <FolderInput className="h-3.5 w-3.5" />
                            </button>
                          )}
                          {selectedProvider?.capabilities.delete_files && (
                            <button
                              onClick={() => void moveFilesToTrash([...selectedFileIds])}
                              disabled={loading}
                              className="grid h-7 w-7 place-items-center rounded-md text-stone-400 hover:bg-rose-50 hover:text-rose-600 disabled:opacity-40"
                              title="将选中项目移入回收站"
                            >
                              <Trash2 className="h-3.5 w-3.5" />
                            </button>
                          )}
                        </div>
                      ) : <span />}
                    </div>
                    {sortedFiles.map((item) => (
                      <div
                        key={item.id}
                        onContextMenu={(event) => openFileContextMenu(event, item)}
                        aria-selected={selectedFileIds.has(item.id)}
                        className="agnes-drive-file-row grid w-full grid-cols-[28px_minmax(0,1fr)_74px_56px] items-center gap-3 border-b border-stone-100 px-3 py-2 text-left text-xs last:border-b-0 sm:grid-cols-[28px_minmax(0,1fr)_90px_130px_56px] sm:gap-4"
                      >
                        <input
                          type="checkbox"
                          aria-label={`选择 ${item.name}`}
                          checked={selectedFileIds.has(item.id)}
                          onChange={(event) => setFileSelected(item.id, event.target.checked)}
                          onClick={(event) => event.stopPropagation()}
                          className="h-3.5 w-3.5 accent-emerald-700"
                        />
                        <button
                          onClick={() => {
                            openFolder(item);
                          }}
                          disabled={item.kind !== "folder"}
                          className="flex min-w-0 items-center gap-2 text-left text-stone-700 disabled:cursor-default"
                        >
                          {item.kind === "folder" ? <Folder className="h-4 w-4 shrink-0 text-amber-500" /> : <File className="h-4 w-4 shrink-0 text-stone-400" />}
                          <span className="truncate">{item.name}</span>
                        </button>
                        <span className="text-stone-400">{item.kind === "folder" ? "--" : formatStorageBytes(item.size)}</span>
                        <span className="hidden truncate text-stone-400 sm:block">{formatTimestamp(item.modified_at)}</span>
                        <div className="flex items-center justify-end">
                          {item.downloadable && (
                            <button
                              onClick={() => void downloadFile(item)}
                              disabled={loading}
                              className="grid h-7 w-7 place-items-center rounded-md text-stone-400 hover:bg-stone-100 hover:text-stone-800 disabled:opacity-40"
                              title="下载到本地"
                            >
                              <Download className="h-3.5 w-3.5" />
                            </button>
                          )}
                          <button
                            onClick={(event) => openFileContextMenu(event, item)}
                            disabled={loading}
                            className="grid h-7 w-7 place-items-center rounded-md text-stone-400 hover:bg-stone-100 hover:text-stone-800 disabled:opacity-40"
                            title="更多操作"
                          >
                            <MoreHorizontal className="h-3.5 w-3.5" />
                          </button>
                        </div>
                      </div>
                    ))}
                    {!fileListLoading && sortedFiles.length === 0 && (
                      <div className="grid h-full min-h-48 place-items-center text-xs text-stone-400">
                        {normalizedSearchQuery
                          ? "未找到匹配文件"
                          : !selectedAccount.provider_installed
                          ? "Provider adapter 不可用"
                          : !selectedAccount.enabled
                            ? "账户已在本机停用"
                          : selectedAccount.auth_state === "connected"
                            ? "此目录为空"
                            : "账户需要完成授权"}
                      </div>
                    )}
                  </div>
                  {visibleNextPageToken && (
                    <button
                      onClick={() => {
                        if (normalizedSearchQuery) {
                          void loadSearchFiles(normalizedSearchQuery, true, visibleNextPageToken);
                        } else {
                          void loadFiles(true, visibleNextPageToken);
                        }
                      }}
                      disabled={fileListLoading}
                      className="mt-3 self-center rounded-md border border-stone-200 bg-white px-3 py-1.5 text-xs text-stone-600 hover:bg-stone-50 disabled:opacity-50"
                    >
                      {normalizedSearchQuery ? "加载更多搜索结果" : "加载更多"}
                    </button>
                  )}
                </div>
              ) : (
                <div className="min-h-0 flex-1 overflow-auto px-5 py-4">
                  <div className="border-y border-stone-200 bg-white/40">
                    {transfers.filter((job) => job.account_id === selectedAccount.id).map((job) => {
                      const progress = storageProgress(job.bytes_transferred, job.bytes_total);
                      const speed = transferSpeeds[job.id];
                      return (
                        <div key={job.id} className="grid grid-cols-[minmax(0,1fr)_90px] items-center gap-3 border-b border-stone-100 px-3 py-3 text-xs last:border-b-0 sm:grid-cols-[minmax(0,1fr)_100px_140px] sm:gap-4">
                          <div className="min-w-0">
                            <div className="truncate font-medium text-stone-700">{job.display_name}</div>
                            <div className="mt-1 text-[10px] text-stone-400">{OPERATION_LABELS[job.operation] ?? job.operation}</div>
                            <div className="mt-2 flex items-center justify-between gap-3 text-[10px] text-stone-400">
                              <span className="truncate">
                                {formatStorageBytes(job.bytes_transferred)}
                                {job.bytes_total !== null ? ` / ${formatStorageBytes(job.bytes_total)}` : ""}
                              </span>
                              {progress !== null && <span className="shrink-0">{Math.round(progress)}%</span>}
                            </div>
                            {progress !== null ? (
                              <div
                                className="mt-1.5 h-1 overflow-hidden rounded bg-stone-100"
                                role="progressbar"
                                aria-label={`${job.display_name} 传输进度`}
                                aria-valuemin={0}
                                aria-valuemax={100}
                                aria-valuenow={Math.round(progress)}
                              >
                                <div
                                  className="h-full bg-[#D97757] transition-[width] duration-500 ease-out"
                                  style={{ width: progress > 0 ? `max(2px, ${progress}%)` : "0%" }}
                                />
                              </div>
                            ) : job.status === "running" ? (
                              <div
                                className="mt-1.5 h-1 overflow-hidden rounded bg-stone-100"
                                role="progressbar"
                                aria-label={`${job.display_name} 正在传输`}
                              >
                                <div className="h-full w-1/3 animate-pulse bg-[#D97757]" />
                              </div>
                            ) : null}
                          </div>
                          <div className="text-right text-stone-500">
                            <div>{TRANSFER_STATUS_LABELS[job.status] ?? job.status}</div>
                            {job.status === "running" && (
                              <div className="mt-1 text-[10px] text-stone-400">速度 {formatTransferSpeed(speed)}</div>
                            )}
                          </div>
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

      {fileContextMenu && (
        <>
          <div
            className="fixed inset-0 z-40"
            onClick={() => setFileContextMenu(null)}
            onContextMenu={(event) => {
              event.preventDefault();
              setFileContextMenu(null);
            }}
          />
          <div
            className="fixed z-50 w-52 overflow-hidden rounded-xl border border-stone-200 bg-white py-1 text-xs text-stone-700 shadow-2xl"
            style={{ left: fileContextMenu.x, top: fileContextMenu.y }}
          >
            {fileContextMenu.item.kind === "folder" ? (
              <>
                <button
                  onClick={() => {
                    openFolder(fileContextMenu.item);
                    setFileContextMenu(null);
                  }}
                  disabled={loading}
                  className="flex w-full items-center gap-2 px-3 py-2 text-left transition-colors hover:bg-stone-100 disabled:text-stone-300"
                >
                  <FolderOpen className="h-3.5 w-3.5 text-amber-500" />
                  打开文件夹
                </button>
                <button
                  onClick={() => {
                    const item = fileContextMenu.item;
                    setFileContextMenu(null);
                    void downloadFolder(item);
                  }}
                  disabled={loading}
                  className="flex w-full items-center gap-2 px-3 py-2 text-left transition-colors hover:bg-stone-100 disabled:text-stone-300"
                >
                  <FolderDown className="h-3.5 w-3.5 text-stone-500" />
                  批量下载
                </button>
              </>
            ) : (
              <>
                <button
                  onClick={() => {
                    const item = fileContextMenu.item;
                    setFileContextMenu(null);
                    void downloadFile(item);
                  }}
                  disabled={loading || !fileContextMenu.item.downloadable}
                  className="flex w-full items-center gap-2 px-3 py-2 text-left transition-colors hover:bg-stone-100 disabled:text-stone-300"
                >
                  <Download className="h-3.5 w-3.5" />
                  下载
                </button>
                {isKnowledgeImportable(fileContextMenu.item) && (
                  <button
                    onClick={() => openKnowledgeImport(fileContextMenu.item)}
                    disabled={loading || !activeAgentId || !fileContextMenu.item.downloadable}
                    className="flex w-full items-center gap-2 px-3 py-2 text-left transition-colors hover:bg-stone-100 disabled:text-stone-300"
                  >
                    <FolderOpen className="h-3.5 w-3.5 text-emerald-700" />
                    导入知识库
                  </button>
                )}
                {isReadingImportable(fileContextMenu.item) && (
                  <button
                    onClick={() => void importToReading(fileContextMenu.item)}
                    disabled={loading || !activeAgentId || !fileContextMenu.item.downloadable}
                    className="flex w-full items-center gap-2 px-3 py-2 text-left transition-colors hover:bg-stone-100 disabled:text-stone-300"
                  >
                    <FolderDown className="h-3.5 w-3.5 text-emerald-700" />
                    导入书架
                  </button>
                )}
              </>
            )}
            {(selectedProvider?.capabilities.move_files || selectedProvider?.capabilities.delete_files) && (
              <>
                <div className="my-1 border-t border-stone-100" />
                {selectedProvider?.capabilities.move_files && (
                  <button
                    onClick={() => openMoveDialog(contextFileIds)}
                    disabled={loading || contextFileIds.length === 0}
                    className="flex w-full items-center gap-2 px-3 py-2 text-left transition-colors hover:bg-stone-100 disabled:text-stone-300"
                  >
                    <FolderInput className="h-3.5 w-3.5" />
                    移动到{contextFileIds.length > 1 ? `（${contextFileIds.length} 项）` : ""}
                  </button>
                )}
                {selectedProvider?.capabilities.delete_files && (
                  <button
                    onClick={() => void moveFilesToTrash(contextFileIds)}
                    disabled={loading || contextFileIds.length === 0}
                    className="flex w-full items-center gap-2 px-3 py-2 text-left text-rose-700 transition-colors hover:bg-rose-50 disabled:text-stone-300"
                  >
                    <Trash2 className="h-3.5 w-3.5" />
                    移入回收站{contextFileIds.length > 1 ? `（${contextFileIds.length} 项）` : ""}
                  </button>
                )}
              </>
            )}
          </div>
        </>
      )}

      {moveDialog && (
        <div
          className="fixed inset-0 z-[60] grid place-items-center bg-black/20 p-4 backdrop-blur-[1px]"
          onClick={closeMoveDialog}
        >
          <div
            className="flex max-h-[min(620px,calc(100vh-32px))] w-full max-w-lg flex-col overflow-hidden rounded-lg border border-stone-200 bg-white shadow-2xl"
            onClick={(event) => event.stopPropagation()}
          >
            <div className="flex items-center justify-between border-b border-stone-200 px-4 py-3">
              <div className="min-w-0">
                <div className="text-sm font-semibold text-stone-800">
                  移动 {moveDialog.fileIds.length} 个项目
                </div>
                <div className="mt-0.5 truncate text-[11px] text-stone-400">
                  选择目标文件夹
                </div>
              </div>
              <button
                onClick={closeMoveDialog}
                disabled={moveSubmitting}
                className="grid h-7 w-7 place-items-center rounded-md text-stone-400 hover:bg-stone-100 hover:text-stone-700 disabled:opacity-40"
                title="关闭"
              >
                <X className="h-4 w-4" />
              </button>
            </div>

            <div className="flex min-h-10 shrink-0 items-center gap-1 overflow-x-auto border-b border-stone-100 px-4 text-xs">
              {moveDialog.path.map((level, index) => (
                <div key={`${level.id ?? "root"}-${index}`} className="flex shrink-0 items-center gap-1">
                  {index > 0 && <ChevronRight className="h-3 w-3 text-stone-300" />}
                  <button
                    onClick={() => openMoveBreadcrumb(index)}
                    disabled={moveLoading || moveSubmitting || index === moveDialog.path.length - 1}
                    className="max-w-40 truncate rounded px-1.5 py-1 text-stone-500 hover:bg-stone-100 hover:text-stone-800 disabled:text-stone-800"
                    title={level.name}
                  >
                    {level.name}
                  </button>
                </div>
              ))}
            </div>

            <div className="min-h-64 flex-1 overflow-y-auto p-2">
              {moveFolders.map((folder) => (
                <button
                  key={folder.id}
                  onClick={() => openMoveFolder(folder)}
                  disabled={moveLoading || moveSubmitting}
                  className="flex h-10 w-full items-center gap-2 rounded-md px-3 text-left text-xs text-stone-700 hover:bg-stone-100 disabled:opacity-50"
                >
                  <Folder className="h-4 w-4 shrink-0 text-amber-500" />
                  <span className="min-w-0 flex-1 truncate">{folder.name}</span>
                  <ChevronRight className="h-3.5 w-3.5 shrink-0 text-stone-300" />
                </button>
              ))}
              {moveLoading && moveFolders.length === 0 && (
                <div className="grid min-h-56 place-items-center">
                  <LoaderCircle className="h-5 w-5 animate-spin text-stone-400" />
                </div>
              )}
              {!moveLoading && moveFolders.length === 0 && (
                <div className="grid min-h-56 place-items-center text-xs text-stone-400">
                  此目录没有子文件夹
                </div>
              )}
              {moveNextPageToken && (
                <button
                  onClick={() => void loadMoveFolders(moveDialog, true, moveNextPageToken)}
                  disabled={moveLoading || moveSubmitting}
                  className="mt-2 h-8 w-full rounded-md border border-stone-200 text-xs text-stone-500 hover:bg-stone-50 disabled:opacity-40"
                >
                  {moveLoading ? "加载中..." : "加载更多"}
                </button>
              )}
            </div>

            <div className="flex shrink-0 items-center justify-between gap-3 border-t border-stone-200 px-4 py-3">
              <div className="min-w-0 truncate text-[11px] text-stone-400">
                {moveTargetIsSource
                  ? "项目已位于此目录"
                  : `目标：${moveDialog.path[moveDialog.path.length - 1].name}`}
              </div>
              <div className="flex shrink-0 items-center gap-2">
                <button
                  onClick={closeMoveDialog}
                  disabled={moveSubmitting}
                  className="h-8 rounded-md px-3 text-xs font-medium text-stone-500 hover:bg-stone-100 disabled:opacity-40"
                >
                  取消
                </button>
                <button
                  onClick={() => void moveFiles()}
                  disabled={moveLoading || moveSubmitting || moveTargetIsSource}
                  className="flex h-8 items-center gap-2 rounded-md bg-emerald-700 px-3 text-xs font-medium text-white hover:bg-emerald-800 disabled:opacity-40"
                >
                  {moveSubmitting && <LoaderCircle className="h-3.5 w-3.5 animate-spin" />}
                  移动到此处
                </button>
              </div>
            </div>
          </div>
        </div>
      )}

      {knowledgeImportItem && (
        <div
          className="fixed inset-0 z-[60] grid place-items-center bg-black/20 p-4 backdrop-blur-[1px]"
          onClick={() => setKnowledgeImportItem(null)}
        >
          <div
            className="w-full max-w-sm overflow-hidden rounded-lg border border-stone-200 bg-white shadow-2xl"
            onClick={(event) => event.stopPropagation()}
          >
            <div className="flex items-center justify-between border-b border-stone-200 px-4 py-3">
              <div className="min-w-0">
                <div className="text-sm font-semibold text-stone-800">选择知识库</div>
                <div className="mt-0.5 truncate text-[11px] text-stone-400">{knowledgeImportItem.name}</div>
              </div>
              <button
                onClick={() => setKnowledgeImportItem(null)}
                className="grid h-7 w-7 place-items-center rounded-md text-stone-400 hover:bg-stone-100 hover:text-stone-700"
                title="关闭"
              >
                <X className="h-4 w-4" />
              </button>
            </div>
            <div className="max-h-72 overflow-y-auto py-1">
              {knowledgeCollections
                .filter((collection) => collection.permission === "write" || collection.permission === "manage")
                .map((collection) => (
                  <button
                    key={collection.id}
                    onClick={() => void importToKnowledge(collection.id)}
                    className="flex w-full items-center gap-2 px-4 py-2.5 text-left text-xs text-stone-700 hover:bg-stone-50"
                  >
                    <FolderOpen className="h-3.5 w-3.5 text-emerald-700" />
                    <span className="truncate">{collection.name}</span>
                  </button>
                ))}
            </div>
          </div>
        </div>
      )}

      {showQuarkAuthorization && (
        <div className="fixed inset-0 z-[80] grid place-items-center bg-stone-950/30 p-4 backdrop-blur-sm">
          <div className="w-full max-w-lg rounded-lg border border-stone-200 bg-[#FAF9F5] shadow-2xl">
            <div className="flex items-center justify-between border-b border-stone-200 px-4 py-3">
              <div>
                <h2 className="text-sm font-semibold text-stone-900">连接夸克网盘</h2>
                <p className="mt-0.5 text-[11px] text-stone-500">
                  {quarkAuthorizationMode === "qr" ? "使用夸克 App 扫码登录" : "粘贴或导入 pan.quark.cn 登录 Cookie"}
                </p>
              </div>
              <button
                onClick={closeQuarkAuthorization}
                disabled={loading}
                className="grid h-8 w-8 place-items-center rounded-md text-stone-400 hover:bg-stone-100 hover:text-stone-700 disabled:opacity-50"
                title="关闭"
              >
                <X className="h-4 w-4" />
              </button>
            </div>
            <div className="space-y-4 p-4">
              <div className="flex rounded-md border border-stone-200 bg-white p-0.5">
                <button
                  onClick={() => setQuarkAuthorizationMode("cookie")}
                  disabled={loading}
                  className={`flex-1 rounded px-3 py-1.5 text-xs font-medium ${
                    quarkAuthorizationMode === "cookie" ? "bg-stone-100 text-stone-900" : "text-stone-500"
                  }`}
                >
                  Cookie / JSON
                </button>
                <button
                  onClick={() => {
                    setQuarkAuthorizationMode("qr");
                    if (!quarkQrChallengeId && !quarkQrImage && !quarkQrLoading) void startQuarkQrLogin();
                  }}
                  disabled={loading || quarkQrLoading}
                  className={`flex-1 rounded px-3 py-1.5 text-xs font-medium ${
                    quarkAuthorizationMode === "qr" ? "bg-stone-100 text-stone-900" : "text-stone-500"
                  }`}
                >
                  二维码登录
                </button>
              </div>
              <div className="flex gap-2 rounded-md border border-amber-200 bg-amber-50 px-3 py-2 text-[11px] leading-5 text-amber-800">
                <ShieldAlert className="mt-0.5 h-4 w-4 shrink-0" />
                <span>这是社区逆向 API 适配器，可能因夸克接口变更或风控失效。Cookie 仅保存在本机系统 Keyring。</span>
              </div>
              {quarkAuthorizationMode === "qr" ? (
                <div className="flex min-h-72 flex-col items-center justify-center gap-3">
                  {quarkQrImage ? (
                    <img src={quarkQrImage} alt="夸克网盘登录二维码" className="h-60 w-60 rounded-md bg-white p-2" />
                  ) : quarkQrStatus?.includes("失败") || quarkQrStatus?.includes("过期") ? (
                    <button
                      onClick={() => void startQuarkQrLogin()}
                      className="flex h-9 items-center gap-2 rounded-md border border-stone-200 bg-white px-3 text-xs font-medium text-stone-600 hover:bg-stone-50"
                    >
                      <RefreshCw className="h-3.5 w-3.5" />
                      重新获取二维码
                    </button>
                  ) : (
                    <LoaderCircle className="h-7 w-7 animate-spin text-stone-400" />
                  )}
                  <p className="text-center text-xs text-stone-500">{quarkQrStatus ?? "正在准备二维码"}</p>
                  <p className="text-center text-[11px] text-stone-400">二维码有效期约 5 分钟，过期后重新切换此页获取</p>
                </div>
              ) : (
                <>
                  <label className="block">
                    <span className="mb-1.5 block text-xs font-medium text-stone-700">Cookie 文本或 JSON</span>
                    <div className="flex items-center rounded-md border border-stone-200 bg-white focus-within:border-emerald-400 focus-within:ring-2 focus-within:ring-emerald-100">
                      <input
                        type={showQuarkCookie ? "text" : "password"}
                        value={quarkCookie}
                        onChange={(event) => {
                          setQuarkCookie(event.target.value);
                          setQuarkCookieJsonPath(null);
                        }}
                        onKeyDown={(event) => {
                          if (event.key === "Enter" && (quarkCookie.trim() || quarkCookieJsonPath) && !loading) void connectQuarkDrive();
                        }}
                        autoFocus
                        autoComplete="off"
                        spellCheck={false}
                        placeholder="__kps=...; __uid=...; ... 或粘贴 JSON"
                        className="h-10 min-w-0 flex-1 bg-transparent px-3 text-xs text-stone-800 outline-none placeholder:text-stone-300"
                      />
                      <button
                        type="button"
                        onClick={() => setShowQuarkCookie((visible) => !visible)}
                        className="grid h-9 w-9 shrink-0 place-items-center text-stone-400 hover:text-stone-700"
                        title={showQuarkCookie ? "隐藏 Cookie" : "显示 Cookie"}
                      >
                        {showQuarkCookie ? <EyeOff className="h-4 w-4" /> : <Eye className="h-4 w-4" />}
                      </button>
                    </div>
                  </label>
                  <div className="flex items-center gap-2">
                    <button
                      onClick={() => void selectQuarkCookieJson()}
                      disabled={loading}
                      className="flex h-8 items-center gap-2 rounded-md border border-stone-200 bg-white px-3 text-xs font-medium text-stone-600 hover:bg-stone-50 disabled:opacity-50"
                    >
                      <File className="h-3.5 w-3.5" />
                      选择 JSON 文件
                    </button>
                    {quarkCookieJsonPath && (
                      <span className="min-w-0 truncate text-[11px] text-stone-500" title={quarkCookieJsonPath}>
                        {quarkCookieJsonPath.split(/[\\/]/).pop()}
                      </span>
                    )}
                  </div>
                  <div className="text-[11px] leading-5 text-stone-500">
                    支持浏览器 Cookie 导出数组、键值对象，以及 QuarkPan 的 <code>cookies</code> / <code>cookie_string</code> 格式；必须包含 <code>__kps</code> 和 <code>__uid</code>。
                  </div>
                </>
              )}
            </div>
            <div className="flex justify-end gap-2 border-t border-stone-200 px-4 py-3">
              <button
                onClick={closeQuarkAuthorization}
                disabled={loading}
                className="h-8 rounded-md px-3 text-xs font-medium text-stone-500 hover:bg-stone-100 disabled:opacity-50"
              >
                取消
              </button>
              {quarkAuthorizationMode === "cookie" && (
                <button
                  onClick={() => void connectQuarkDrive()}
                  disabled={loading || (!quarkCookie.trim() && !quarkCookieJsonPath)}
                  className="flex h-8 items-center gap-2 rounded-md bg-emerald-700 px-3 text-xs font-medium text-white hover:bg-emerald-800 disabled:opacity-40"
                >
                  {loading && <LoaderCircle className="h-3.5 w-3.5 animate-spin" />}
                  连接
                </button>
              )}
            </div>
          </div>
        </div>
      )}
    </main>
  );
}
