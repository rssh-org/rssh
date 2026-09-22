import { beforeEach, describe, expect, it, vi } from "vitest";
import * as files from "./file-access.ts";

const invokeMock = vi.hoisted(() => vi.fn());
vi.mock("@tauri-apps/api/core", () => ({ invoke: invokeMock }));

beforeEach(() => { invokeMock.mockReset(); });

describe("native file access contract", () => {
  it("keeps provider metadata separate from an opaque document id", async () => {
    const picked = {
      location: "content://com.example.documents/document/opaque-7",
      name: "报告: final%20.txt",
    };
    invokeMock.mockResolvedValue(picked);
    await expect(files.pickFile()).resolves.toEqual(picked);
    expect(invokeMock).toHaveBeenCalledExactlyOnceWith("sftp_pick_open_path");
  });

  it("preserves each selected location and its independent name", async () => {
    const picked = [
      { location: "content://com.example.documents/document/42", name: "one.txt" },
      { location: "content://com.example.documents/document/43", name: "two.txt" },
      { location: "content://com.example.documents/document/44", name: null },
    ];
    invokeMock.mockResolvedValue(picked);
    await expect(files.pickFiles()).resolves.toEqual(picked);
    expect(invokeMock).toHaveBeenCalledExactlyOnceWith("sftp_pick_open_files");
  });

  it("leaves missing metadata unknown instead of using the document id as a name", async () => {
    invokeMock.mockResolvedValue({ location: "content://docs/document/1234", name: null });
    expect((await files.pickFile())?.name).toBeNull();
  });

  it.each([false, true])("passes directory access intent and metadata through (%s)", async (write) => {
    const picked = { location: "file://docs/storage/selected", name: "备份" };
    invokeMock.mockResolvedValue(picked);
    await expect(files.pickDirectory(write)).resolves.toEqual(picked);
    expect(invokeMock).toHaveBeenCalledExactlyOnceWith("sftp_pick_folder", { write });
  });

  it("preserves cancellation without inventing a selection", async () => {
    invokeMock.mockResolvedValue(null);
    await expect(files.pickFile()).resolves.toBeNull();
    await expect(files.pickFiles()).resolves.toBeNull();
    await expect(files.pickDirectory(false)).resolves.toBeNull();
    await expect(files.pickSavePath("report.txt")).resolves.toBeNull();
  });

  it("propagates native errors rather than treating them as cancellation", async () => {
    invokeMock.mockRejectedValue("provider denied access");
    await expect(files.pickFile()).rejects.toBe("provider denied access");
    await expect(files.pickDirectory(false)).rejects.toBe("provider denied access");
  });

  it("uses the host to resolve children without concatenating a URI in the frontend", async () => {
    const root = "file://docs/storage/备份%20";
    const names = ["目录/a%2Fb.txt", "报告:final.txt"];
    const locations = ["file://docs/authorized/42", "file://docs/authorized/43"];
    invokeMock.mockResolvedValue(locations);
    await expect(files.resolvePaths(root, names, true)).resolves.toEqual(locations);
    expect(invokeMock).toHaveBeenCalledExactlyOnceWith("resolve_local_paths", {
      localRoot: root, relativePaths: names, write: true,
    });
  });

  it("keeps walk names and authorized file locations separate", async () => {
    const root = "file://docs/storage/selected";
    const entries = [{ rel_path: "dir/报告.txt", size: 32, local_path: "file://docs/authorized/42" }];
    invokeMock.mockResolvedValue(entries);
    await expect(files.walkDirectory(root)).resolves.toEqual(entries);
    expect(invokeMock).toHaveBeenCalledExactlyOnceWith("walk_local_dir", { localRoot: root });
  });

  it("keeps the selected save location opaque and preserves the suggested filename", async () => {
    invokeMock.mockResolvedValue("content://downloads/document/42");
    await expect(files.pickSavePath("report:final%20.txt"))
      .resolves.toBe("content://downloads/document/42");
    expect(invokeMock).toHaveBeenCalledExactlyOnceWith("sftp_pick_save_path", {
      defaultName: "report:final%20.txt",
    });
  });
});
