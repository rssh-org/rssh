import { describe, expect, it } from "vitest";
import { remoteUploadName } from "./sftp-name.ts";

describe("remoteUploadName", () => {
  it("recovers the display name from a SAF ExternalStorage content URI", () => {
    // ACTION_GET_CONTENT on a user-visible provider encodes the path.
    const uri =
      "content://com.android.externalstorage.documents/document/primary%3ADownload%2Freport.pdf";
    expect(remoteUploadName(uri)).toBe("report.pdf");
  });

  it("falls back to the document id for an opaque provider", () => {
    const uri = "content://com.android.providers.downloads.documents/document/msf%3A1234";
    // No display name available — the id is the best we can do (caller may
    // still override; this just must not be empty/garbage).
    expect(remoteUploadName(uri)).toBe("1234");
  });

  it("returns the basename for a plain filesystem path", () => {
    expect(remoteUploadName("/sdcard/Documents/notes.txt")).toBe("notes.txt");
    expect(remoteUploadName("C:\\Users\\me\\key.pem")).toBe("key.pem");
  });

  it("preserves spaces and colons in filesystem and document filenames", () => {
    expect(remoteUploadName("/tmp/ report: final ")).toBe(" report: final ");
    expect(remoteUploadName("file://docs/storage/Users/currentUser/Download/%20report%3A%20final%20"))
      .toBe(" report: final ");
  });

  it.each([
    ["/tmp/a%2Fb.txt", "a%2Fb.txt"],
    ["/tmp/report%20one.txt", "report%20one.txt"],
    ["C:\\Downloads\\report%20one.txt", "report%20one.txt"],
  ])("preserves literal percent escapes in a host filesystem path: %s", (path, name) => {
    expect(remoteUploadName(path)).toBe(name);
  });

  it("decodes Chinese and spaces in an OHOS document URI", () => {
    expect(remoteUploadName("file://docs/storage/Users/currentUser/Download/%E6%8A%A5%E5%91%8A%20one.txt"))
      .toBe("报告 one.txt");
  });

  it("still decodes Android document path separators before finding the name", () => {
    expect(remoteUploadName("content://com.android.externalstorage.documents/document/primary%3ADownload%2F%E6%8A%A5%E5%91%8A%20one.txt"))
      .toBe("报告 one.txt");
  });

  it("recovers a decoded basename from an iOS security-scoped file URL", () => {
    expect(remoteUploadName("file:///private/var/mobile/Containers/Shared/AppGroup/report%20x.pdf"))
      .toBe("report x.pdf");
  });

  it("returns empty when nothing usable remains (caller adds a fallback)", () => {
    expect(remoteUploadName("")).toBe("");
    expect(remoteUploadName("/")).toBe("");
  });

  it("does not throw on a malformed percent-escape", () => {
    expect(() => remoteUploadName("content://x/%E0%A4%A")).not.toThrow();
  });
});
