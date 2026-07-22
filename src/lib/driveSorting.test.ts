import { describe, expect, it } from "vitest";
import { remoteTimestampMillis, sortDriveItems, type DriveSort } from "./driveSorting";

const items = [
  { id: "file-10", name: "文件10.txt", kind: "file", size: 10, modified_at: "2000" },
  { id: "folder-2", name: "目录2", kind: "folder", size: null, modified_at: "3000" },
  { id: "file-2", name: "文件2.txt", kind: "file", size: 20, modified_at: null },
  { id: "folder-10", name: "目录10", kind: "folder", size: null, modified_at: "1000" },
];

function names(sort: DriveSort): string[] {
  return sortDriveItems(items, sort).map((item) => item.name);
}

describe("drive file sorting", () => {
  it("keeps folders first and uses natural name ordering", () => {
    expect(names({ key: "name", direction: "asc" })).toEqual([
      "目录2",
      "目录10",
      "文件2.txt",
      "文件10.txt",
    ]);
    expect(names({ key: "name", direction: "desc" })).toEqual([
      "目录10",
      "目录2",
      "文件10.txt",
      "文件2.txt",
    ]);
  });

  it("sorts numeric values while keeping missing values last", () => {
    expect(names({ key: "size", direction: "desc" })).toEqual([
      "目录2",
      "目录10",
      "文件2.txt",
      "文件10.txt",
    ]);
    expect(names({ key: "modified", direction: "asc" })).toEqual([
      "目录10",
      "目录2",
      "文件10.txt",
      "文件2.txt",
    ]);
  });

  it("normalizes second, millisecond, and ISO timestamps", () => {
    expect(remoteTimestampMillis("1753171200")).toBe(1_753_171_200_000);
    expect(remoteTimestampMillis("1753171200000")).toBe(1_753_171_200_000);
    expect(remoteTimestampMillis("2025-07-22T00:00:00Z")).toBe(Date.parse("2025-07-22T00:00:00Z"));
    expect(remoteTimestampMillis("invalid")).toBeNull();
  });
});
