import { describe, expect, it } from "vitest";
import { formatStorageBytes, storageProgress } from "./storage";

describe("storage formatters", () => {
  it("formats byte counts without losing unavailable states", () => {
    expect(formatStorageBytes(null)).toBe("--");
    expect(formatStorageBytes(0)).toBe("0 B");
    expect(formatStorageBytes(1024)).toBe("1.00 KB");
    expect(formatStorageBytes(5 * 1024 * 1024)).toBe("5.00 MB");
  });

  it("bounds transfer progress", () => {
    expect(storageProgress(25, 100)).toBe(25);
    expect(storageProgress(120, 100)).toBe(100);
    expect(storageProgress(0, null)).toBeNull();
  });
});
