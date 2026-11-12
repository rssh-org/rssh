import { describe, it, expect } from "vitest";
import { classifyShell, createShellProbe } from "./shell-probe.ts";

describe("classifyShell", () => {
  it("maps PowerShell editions", () => {
    expect(classifyShell("Desktop", "anything")).toBe("powershell");
    expect(classifyShell("Core", "12345")).toBe("powershell");
  });

  it("maps the unexpanded literal to cmd.exe", () => {
    expect(classifyShell("$PSEdition", "$$")).toBe("cmd");
  });

  it("maps empty edition + numeric PID to posix", () => {
    expect(classifyShell("", "12345")).toBe("posix");
  });

  it("returns null for unsupported or ambiguous signatures", () => {
    expect(classifyShell("", "$$")).toBeNull();
    expect(classifyShell("Foo", "bar")).toBeNull();
  });
});

describe("createShellProbe", () => {
  function evaluated(command: string, edition: string, dollar: string): string {
    return command.slice("echo ".length).replace("$PSEdition", () => edition).replace("$$", () => dollar);
  }

  it("uses the reserved session UUID as the shared shell-safe marker", () => {
    const id = crypto.randomUUID();
    const first = createShellProbe(id);
    const second = createShellProbe(crypto.randomUUID());
    expect(createShellProbe(id).command).toBe(first.command);
    expect(first.command).toContain(`RSSH_SHELL_${id.replaceAll("-", "")}`);
    expect(first.command).toMatch(/^echo RSSH_SHELL_[0-9a-f]{32}=\$PSEdition=\$\$=END$/);
    expect(second.command).not.toBe(first.command);
  });

  it.each([
    ["", "12345", "posix"],
    ["Desktop", "token", "powershell"],
    ["Core", "argument=value", "powershell"],
    ["$PSEdition", "$$", "cmd"],
  ] as const)("classifies a complete %s / %s response as %s", (edition, dollar, shell) => {
    const probe = createShellProbe(crypto.randomUUID());
    expect(probe.classify(`${evaluated(probe.command, edition, dollar)}\r\n`)).toEqual({ shell });
  });

  it.each(["", "user@host:~$ ", "PS C:\\> ", "C:\\> "])("ignores input echo after prompt %s", (prompt) => {
    const probe = createShellProbe(crypto.randomUUID());
    const echoed = `${prompt}${probe.command}\r\n`;
    expect(probe.classify(echoed)).toBeNull();
    expect(probe.classify(`${echoed}${evaluated(probe.command, "", "12345")}\r\n`)).toEqual({ shell: "posix" });
  });

  it("accepts real cmd output immediately without accepting its echoed command", () => {
    const probe = createShellProbe(crypto.randomUUID());
    const echo = `C:\\> ${probe.command}\r\n`;
    expect(probe.classify(echo)).toBeNull();
    expect(probe.classify(`${echo}${probe.command.slice(5)}\r\n`)).toEqual({ shell: "cmd" });
  });

  it("ignores old markers and unrelated output", () => {
    const old = createShellProbe(crypto.randomUUID());
    const current = createShellProbe(crypto.randomUUID());
    const noise = `Last login: yesterday\r\n${evaluated(old.command, "", "12345")}\r\nP=Core=token=E\r\n`;
    expect(current.classify(noise)).toBeNull();
    expect(current.classify(`${noise}${evaluated(current.command, "Core", "token")}\r\n`)).toEqual({ shell: "powershell" });
  });

  it.each(["\n", "\r\n"])("waits for the complete response ending in %j across split chunks", (ending) => {
    const probe = createShellProbe(crypto.randomUUID());
    const line = evaluated(probe.command, "", "12345") + ending;
    for (let length = 0; length < line.length; length++) {
      expect(probe.classify(line.slice(0, length))).toBeNull();
    }
    expect(probe.classify(line)).toEqual({ shell: "posix" });
  });

  it("requires the response to occupy a complete line", () => {
    const probe = createShellProbe(crypto.randomUUID());
    const line = evaluated(probe.command, "", "12345");
    expect(probe.classify(`noise ${line}\n`)).toBeNull();
    expect(probe.classify(`${line} trailing noise\n`)).toBeNull();
    expect(probe.classify(`banner\r\n${line}\r\nprompt> `)).toEqual({ shell: "posix" });
  });

  it("distinguishes a completed unsupported response from a pending probe", () => {
    const probe = createShellProbe(crypto.randomUUID());
    expect(probe.classify("")).toBeNull();
    expect(probe.classify(`${evaluated(probe.command, "", "")}\r\n`)).toEqual({ shell: null });
    expect(probe.classify(`${evaluated(probe.command, "Unknown", "value")}\n`)).toEqual({ shell: null });
  });
});
