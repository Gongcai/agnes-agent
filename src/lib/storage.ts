export function formatStorageBytes(value: number | null | undefined): string {
  if (value === null || value === undefined || !Number.isFinite(value) || value < 0) {
    return "--";
  }
  if (value < 1024) return `${Math.round(value)} B`;
  const units = ["KB", "MB", "GB", "TB", "PB"];
  let amount = value;
  let unit = -1;
  do {
    amount /= 1024;
    unit += 1;
  } while (amount >= 1024 && unit < units.length - 1);
  return `${amount >= 10 ? amount.toFixed(1) : amount.toFixed(2)} ${units[unit]}`;
}

export function storageProgress(transferred: number, total: number | null): number | null {
  if (total === null || total <= 0 || transferred < 0) return null;
  return Math.min(100, Math.max(0, (transferred / total) * 100));
}

export function formatTransferSpeed(bytesPerSecond: number | null | undefined): string {
  if (bytesPerSecond === null || bytesPerSecond === undefined || !Number.isFinite(bytesPerSecond) || bytesPerSecond < 0) {
    return "--";
  }
  return `${formatStorageBytes(bytesPerSecond)}/s`;
}

interface RemoteImportCandidate {
  name: string;
  media_type: string | null;
}

const KNOWLEDGE_MEDIA_TYPES = new Set([
  "text/markdown",
  "text/plain",
  "text/csv",
  "application/json",
  "application/pdf",
  "application/vnd.openxmlformats-officedocument.wordprocessingml.document",
  "application/vnd.openxmlformats-officedocument.presentationml.presentation",
  "application/vnd.openxmlformats-officedocument.spreadsheetml.sheet",
  "application/vnd.google-apps.document",
  "application/vnd.google-apps.spreadsheet",
  "application/vnd.google-apps.presentation",
  "application/vnd.google-apps.script",
  "application/vnd.google-apps.script+json",
]);

function normalizedMediaType(value: string | null): string {
  return value?.split(";", 1)[0].trim().toLowerCase() ?? "";
}

export function isKnowledgeImportable(item: RemoteImportCandidate): boolean {
  return KNOWLEDGE_MEDIA_TYPES.has(normalizedMediaType(item.media_type))
    || /\.(md|markdown|txt|rst|log|csv|json|pdf|docx|pptx|xlsx)$/i.test(item.name.trim());
}

export function isReadingImportable(item: RemoteImportCandidate): boolean {
  return normalizedMediaType(item.media_type) === "application/epub+zip"
    || /\.epub$/i.test(item.name.trim());
}
