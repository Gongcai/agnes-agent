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
