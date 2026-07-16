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
