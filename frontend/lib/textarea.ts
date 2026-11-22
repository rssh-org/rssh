// Textarea values and selections use LF even when the source uses CRLF or CR.
export function textareaValue(text: string): string {
  return text.replace(/\r\n?/g, "\n");
}

export function sourceOffset(text: string, displayOffset: number): number {
  let offset = displayOffset;
  for (let crlf = text.indexOf("\r\n"); crlf >= 0 && crlf < offset; crlf = text.indexOf("\r\n", crlf + 2)) {
    offset++;
  }
  return offset;
}

export function editedText(previous: string, value: string): string {
  if (!previous.includes("\r")) return value;
  const display = textareaValue(previous);
  if (display === value) return previous;

  // Splice the edit into the source, preserving untouched line endings when the user edits elsewhere.
  let start = 0;
  while (start < display.length && start < value.length && display[start] === value[start]) start++;
  let oldEnd = display.length;
  let newEnd = value.length;
  while (oldEnd > start && newEnd > start && display[oldEnd - 1] === value[newEnd - 1]) {
    oldEnd--;
    newEnd--;
  }
  return previous.slice(0, sourceOffset(previous, start)) + value.slice(start, newEnd) +
    previous.slice(sourceOffset(previous, oldEnd));
}
