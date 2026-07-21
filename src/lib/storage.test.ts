import { describe, expect, it } from "vitest";
import {
  formatStorageBytes,
  formatTransferSpeed,
  isKnowledgeImportable,
  isReadingImportable,
  storageProgress,
} from "./storage";

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

  it("formats current transfer speed", () => {
    expect(formatTransferSpeed(0)).toBe("0 B/s");
    expect(formatTransferSpeed(1024 * 1024)).toBe("1.00 MB/s");
    expect(formatTransferSpeed(null)).toBe("--");
  });

  it("recognizes every knowledge format accepted by the backend", () => {
    for (const name of [
      "notes.md",
      "data.json",
      "report.pdf",
      "report.DOCX",
      "slides.pptx",
      "table.xlsx",
    ]) {
      expect(isKnowledgeImportable({ name, media_type: null })).toBe(true);
    }
    expect(isKnowledgeImportable({
      name: "remote-file",
      media_type: " application/pdf; charset=binary ",
    })).toBe(true);
    expect(isKnowledgeImportable({
      name: "Google document",
      media_type: "application/vnd.google-apps.document",
    })).toBe(true);
    expect(isKnowledgeImportable({ name: "archive.zip", media_type: "application/zip" })).toBe(false);
  });

  it("recognizes EPUB books by MIME type or extension", () => {
    expect(isReadingImportable({ name: "book", media_type: "application/epub+zip" })).toBe(true);
    expect(isReadingImportable({ name: "book.EPUB", media_type: "application/octet-stream" })).toBe(true);
    expect(isReadingImportable({ name: "book.pdf", media_type: "application/pdf" })).toBe(false);
  });
});
