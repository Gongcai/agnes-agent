export interface SearchableMemoryFile {
  path: string;
  description: string;
  aliases: string[];
}

const MEMORY_PATH_PATTERN = /^\/(?:profile|preferences)\.md$|^\/(?:topics|areas|people)\/[a-z0-9]+(?:-[a-z0-9]+)*\.md$/;

export function memoryFileMatchesQuery(file: SearchableMemoryFile, value: string): boolean {
  const query = value.trim().toLocaleLowerCase();
  if (!query) return true;
  return [file.path, file.description, ...file.aliases]
    .join("\n")
    .toLocaleLowerCase()
    .includes(query);
}

export function createMemoryFileTemplate(path: string): string {
  const normalized = path.trim();
  if (!MEMORY_PATH_PATTERN.test(normalized)) return "";
  const name = normalized.slice(normalized.lastIndexOf("/") + 1, -3);
  return [
    "---",
    `name: ${name}`,
    "description: 请说明本文件包含什么，以及应在何时读取",
    "sources: [user]",
    "aliases: []",
    "---",
    "- [stated] ",
    "",
  ].join("\n");
}
