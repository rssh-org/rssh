import { describe, expect, it } from "vitest";
import { redactCommandBlockTexts } from "./command-block-redaction.ts";

describe("redactCommandBlockTexts", () => {
  it("recognizes and replaces the first logical-line prompt in every block", () => {
    const redacted = redactCommandBlockTexts([
      "alice@prod:~/app$ ssh 10.0.0.1\nalice@prod is output",
      "PS C:\\Users\\Alice>Get-ChildItem\nready",
    ], {
      promptEnabled: true,
      promptReplacement: "anonymous@rssh",
      rules: [],
    });

    expect(redacted).toEqual([
      "anonymous@rssh ssh 10.0.0.1\nalice@prod is output",
      "anonymous@rssh Get-ChildItem\nready",
    ]);
  });

  it("applies rules to commands and output while keeping replacements literal", () => {
    const [redacted] = redactCommandBlockTexts([
      "$ curl 10.0.0.1/token\npeer=10.2.3.4 token=secret",
    ], {
      promptEnabled: true,
      promptReplacement: "anonymous@rssh",
      rules: [
        { pattern: String.raw`\b10\.\d{1,3}\.\d{1,3}\.\d{1,3}\b`, replacement: "<IP>" },
        { pattern: "(secret)", replacement: "$&-$1-literal" },
      ],
    });

    expect(redacted).toBe(
      "anonymous@rssh curl <IP>/token\npeer=<IP> token=$&-$1-literal",
    );
  });

  it("runs normal rules over the prompt when prompt redaction is disabled", () => {
    const [redacted] = redactCommandBlockTexts(["alice@prod:~$ id"], {
      promptEnabled: false,
      promptReplacement: "anonymous@rssh",
      rules: [{ pattern: "prod", replacement: "hidden" }],
    });

    expect(redacted).toBe("alice@hidden:~$ id");
  });

  it("fails closed before returning text when any rule is invalid", () => {
    expect(() => redactCommandBlockTexts(["$ id", "$ pwd"], {
      promptEnabled: true,
      promptReplacement: "anonymous@rssh",
      rules: [{ pattern: "(?P<name>x)", replacement: "<X>" }],
    })).toThrow();
  });

  it("rejects a zero-width pattern instead of looping", () => {
    expect(() => redactCommandBlockTexts(["$ id"], {
      promptEnabled: true,
      promptReplacement: "anonymous@rssh",
      rules: [{ pattern: "x*", replacement: "<X>" }],
    })).toThrow(/redact_zero_width_pattern/);
  });
});
