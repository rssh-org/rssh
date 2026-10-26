import { detectPrompt } from "./prompt";

export interface CommandBlockRedactionRule {
  pattern: string;
  replacement: string;
}

export interface CommandBlockRedactionSettings {
  promptEnabled: boolean;
  promptReplacement: string;
  rules: ReadonlyArray<CommandBlockRedactionRule>;
}

export interface CompiledCommandBlockRedactionRule {
  regex: RegExp;
  replacement: string;
}

export interface CompiledCommandBlockRedaction {
  promptEnabled: boolean;
  promptReplacement: string;
  rules: ReadonlyArray<CompiledCommandBlockRedactionRule>;
}

export interface RedactionRuleMatch {
  start: number;
  end: number;
}

/** Rust regex accepts leading inline flags. Translate the common global form
 * to JavaScript flags. Unsupported Rust-only syntax fails closed at copy time
 * instead of silently returning unredacted content. */
function compileRule(
  rule: CommandBlockRedactionRule,
): CompiledCommandBlockRedactionRule {
  let source = rule.pattern;
  let flags = "g";
  const inline = source.match(/^\(\?([ims]+)\)/);
  if (inline) {
    source = source.slice(inline[0].length);
    flags += inline[1];
  }
  return { regex: new RegExp(source, flags), replacement: rule.replacement };
}

export function compileCommandBlockRedaction(
  settings: CommandBlockRedactionSettings,
): CompiledCommandBlockRedaction {
  return {
    promptEnabled: settings.promptEnabled,
    promptReplacement: settings.promptReplacement,
    // Compile every rule before touching content. One bad synced rule rejects
    // the complete copy operation; partial redaction is a data leak.
    rules: settings.rules.map(compileRule),
  };
}

export function redactionRuleMatches(
  text: string,
  rule: CompiledCommandBlockRedactionRule,
): RedactionRuleMatch[] {
  const matches: RedactionRuleMatch[] = [];
  rule.regex.lastIndex = 0;
  for (;;) {
    const match = rule.regex.exec(text);
    if (!match) return matches;
    if (match[0].length === 0) throw new Error("redact_zero_width_pattern");
    matches.push({ start: match.index, end: match.index + match[0].length });
  }
}

export function commandBlockPromptEnd(
  text: string,
  firstLine: boolean,
  redaction: CompiledCommandBlockRedaction,
): number | null {
  if (!firstLine || !redaction.promptEnabled) return null;
  return detectPrompt(text)?.end ?? null;
}

export function promptReplacementForTail(replacement: string, tail: string): string {
  if (replacement && tail && !/\s$/.test(replacement) && !/^\s/.test(tail)) {
    return `${replacement} `;
  }
  return replacement;
}

function applyRuleToText(
  text: string,
  rule: CompiledCommandBlockRedactionRule,
): string {
  const matches = redactionRuleMatches(text, rule);
  if (matches.length === 0) return text;

  const out: string[] = [];
  let cursor = 0;
  for (const match of matches) {
    out.push(text.slice(cursor, match.start), rule.replacement);
    cursor = match.end;
  }
  out.push(text.slice(cursor));
  return out.join("");
}

function applyRulesToText(
  text: string,
  rules: ReadonlyArray<CompiledCommandBlockRedactionRule>,
): string {
  return rules.reduce((current, rule) => applyRuleToText(current, rule), text);
}

function redactLine(
  text: string,
  firstLine: boolean,
  redaction: CompiledCommandBlockRedaction,
): string {
  const promptEnd = commandBlockPromptEnd(text, firstLine, redaction);
  if (promptEnd === null) return applyRulesToText(text, redaction.rules);

  const tail = applyRulesToText(text.slice(promptEnd), redaction.rules);
  return promptReplacementForTail(redaction.promptReplacement, tail) + tail;
}

function redactCommandBlockText(
  text: string,
  redaction: CompiledCommandBlockRedaction,
): string {
  return text
    .split("\n")
    .map((line, index) => redactLine(line, index === 0, redaction))
    .join("\n");
}

/** Redact each block independently so prompt recognition never crosses the
 * Enter-defined block boundary. Returned array preserves input ordering. */
export function redactCommandBlockTexts(
  blocks: ReadonlyArray<string>,
  settings: CommandBlockRedactionSettings,
): string[] {
  const redaction = compileCommandBlockRedaction(settings);
  return blocks.map((text) => redactCommandBlockText(text, redaction));
}
