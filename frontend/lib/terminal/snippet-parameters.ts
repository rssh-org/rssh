export interface SnippetParameter {
  /** The exact text to replace in the command. */
  name: string;
  defaultValue: string | null;
}

export interface Snippet {
  name: string;
  command: string;
  parameters?: SnippetParameter[];
}

export interface SnippetPart {
  text: string;
  parameterName?: string;
}

export function snippetPreview(snippet: Snippet, values?: ReadonlyMap<string, string>): SnippetPart[] {
  const names = snippet.parameters?.map(parameter => parameter.name).filter(name => name.length > 0);
  if (!names?.length) return [{ text: snippet.command }];

  // At the same starting character the longest name wins. Escape every name so
  // shell punctuation is literal. Splitting once also keeps inserted values opaque.
  const alternatives = names.sort((a, b) => b.length - a.length)
    .map(name => name.replace(/[.*+?^${}()|[\]\\]/g, "\\$&"));
  const matches = new RegExp(`(${alternatives.join("|")})`, "gu");
  const parts: SnippetPart[] = [];
  for (const [index, text] of snippet.command.split(matches).entries()) {
    if (index % 2 === 1) parts.push({ text: values?.get(text) ?? text, parameterName: text });
    else if (text) parts.push({ text });
  }
  return parts;
}
