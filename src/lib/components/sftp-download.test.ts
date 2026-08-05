import { readFileSync } from "node:fs";
import { join } from "node:path";
import { describe, expect, it } from "vitest";

const source = readFileSync(join(process.cwd(), "src/lib/components/SftpBrowser.svelte"), "utf8");

function functionSource(name: string, nextName: string): string {
  const start = source.indexOf(`async function ${name}`);
  const end = source.indexOf(`function ${nextName}`, start);
  if (start < 0 || end < 0) throw new Error(`missing ${name} function boundary`);
  return source.slice(start, end);
}

describe("SFTP context-menu download", () => {
  it("picks a target path and queues the file through the transfer store", () => {
    const downloadEntry = functionSource("downloadEntry", "confirmDelete");

    expect(downloadEntry).toContain('"sftp_pick_save_path"');
    expect(downloadEntry).toContain("transfers.startDownload");
    expect(downloadEntry).not.toContain('"sftp_save_file"');
  });
});
