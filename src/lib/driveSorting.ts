export type DriveSortKey = "name" | "size" | "modified";
export type DriveSortDirection = "asc" | "desc";

export interface DriveSort {
  key: DriveSortKey;
  direction: DriveSortDirection;
}

interface SortableDriveItem {
  id: string;
  name: string;
  kind: string;
  size: number | null;
  modified_at: string | null;
}

const FILE_NAME_COLLATOR = new Intl.Collator("zh-CN", {
  numeric: true,
  sensitivity: "base",
});

export function remoteTimestampMillis(value: string | null): number | null {
  if (!value) return null;
  const numeric = Number(value);
  if (Number.isFinite(numeric)) {
    return Math.abs(numeric) >= 10_000_000_000 ? numeric : numeric * 1_000;
  }
  const parsed = Date.parse(value);
  return Number.isNaN(parsed) ? null : parsed;
}

function compareNullableNumbers(
  left: number | null,
  right: number | null,
  direction: DriveSortDirection,
): number {
  if (left === null || right === null) {
    if (left === right) return 0;
    return left === null ? 1 : -1;
  }
  const comparison = left - right;
  return direction === "asc" ? comparison : -comparison;
}

export function sortDriveItems<T extends SortableDriveItem>(items: T[], sort: DriveSort): T[] {
  return [...items].sort((left, right) => {
    const leftFolder = left.kind === "folder";
    const rightFolder = right.kind === "folder";
    if (leftFolder !== rightFolder) return leftFolder ? -1 : 1;

    let comparison = 0;
    if (sort.key === "name") {
      comparison = FILE_NAME_COLLATOR.compare(left.name, right.name);
      if (sort.direction === "desc") comparison = -comparison;
    } else if (sort.key === "size") {
      comparison = compareNullableNumbers(left.size, right.size, sort.direction);
    } else {
      comparison = compareNullableNumbers(
        remoteTimestampMillis(left.modified_at),
        remoteTimestampMillis(right.modified_at),
        sort.direction,
      );
    }

    if (comparison !== 0) return comparison;
    const nameComparison = FILE_NAME_COLLATOR.compare(left.name, right.name);
    return nameComparison || left.id.localeCompare(right.id);
  });
}
