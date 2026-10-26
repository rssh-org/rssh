import { describe, expect, it } from "vitest";
import { detectPrompt } from "./prompt.ts";

function promptOf(line: string): string | null {
  const match = detectPrompt(line);
  return match ? line.slice(0, match.end) : null;
}

describe("detectPrompt", () => {
  it.each([
    ["alice@host:~/src$ git status", "alice@host:~/src$"],
    ["(venv) root@server:/opt/app# id", "(venv) root@server:/opt/app#"],
    ["alice@host ~/src> pwd", "alice@host ~/src>"],
    ["[root@server /var/log]# tail app.log", "[root@server /var/log]#"],
    ["MacBook-Pro:project alice$ make", "MacBook-Pro:project alice$"],
    ["PS C:\\Users\\Alice> Get-ChildItem", "PS C:\\Users\\Alice>"],
    ["C:\\Users\\Alice>dir", "C:\\Users\\Alice>"],
    ["\\\\server\\share> dir", "\\\\server\\share>"],
    ["alice  ~/src  cargo test", "alice  ~/src "],
    ["❯ git status", "❯"],
    ["~/src ❯ git status", "~/src ❯"],
    ["bash-5.2$ printf ok", "bash-5.2$"],
    ["$ echo ok", "$"],
  ])("recognizes %s", (line, expected) => {
    expect(promptOf(line)).toBe(expected);
  });

  it("stops at the first Starship marker when the command contains another", () => {
    expect(promptOf("❯ git log --format='%s ➜ %d'")).toBe("❯");
    expect(promptOf("~/src ❯ printf '➜'")).toBe("~/src ❯");
  });

  it.each([
    "build > output.txt",
    "ERROR: request failed",
    "normal command output",
    "100% complete",
    "",
  ])("does not guess on ordinary output: %s", (line) => {
    expect(detectPrompt(line)).toBeNull();
  });
});
