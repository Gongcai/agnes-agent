export interface SearchableMemory {
  name: string;
  keywords: string[];
  content: string;
}

export function parseMemoryKeywords(value: string): string[] {
  return value
    .split(/[,，]/)
    .map((keyword) => keyword.trim())
    .filter((keyword, index, values) => keyword.length > 0 && values.indexOf(keyword) === index);
}

export function memoryMatchesQuery(memory: SearchableMemory, value: string): boolean {
  const query = value.trim().toLocaleLowerCase();
  if (!query) return true;
  return [memory.name, memory.content, ...memory.keywords]
    .join("\n")
    .toLocaleLowerCase()
    .includes(query);
}

export function formatMemoryTime(value: string): string {
  const timestamp = Number(value);
  if (!Number.isFinite(timestamp) || timestamp <= 0) return value;
  return new Date(timestamp * 1000).toLocaleString("zh-CN", { hour12: false });
}

export function memoryEmbeddingProgress(indexed: number, total: number): number {
  if (!Number.isFinite(total) || total <= 0) return 0;
  const ratio = Number.isFinite(indexed) ? indexed / total : 0;
  return Math.round(Math.min(1, Math.max(0, ratio)) * 100);
}

export function embeddingModelName(modelRef: string | null): string {
  if (!modelRef) return "未配置嵌入模型";
  const separator = modelRef.indexOf("/");
  return separator >= 0 ? modelRef.slice(separator + 1) : modelRef;
}
