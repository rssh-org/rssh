/**
 * Block content extraction — pure functions over xterm.js Buffer.
 *
 * 把命令块的可视内容抽成纯文本，供"复制为文本"和"复制为图片"两条
 * 路径共用基础数据。
 *
 * 核心规则：
 *  1. 多块按 `block.id` 升序输出（时间顺序）
 *  2. 块内按行号 `[start.line .. end.line]` 顺序遍历
 *  3. 软换行（line.isWrapped === true）合并为同一逻辑行——对 shell paste
 *     友好，可重复执行
 *  4. CJK 宽字符：width=2 cell 持有字符，width=0 continuation 跳过
 *  5. 空 cell 视为空格（终端右侧 padding）；逻辑行末尾 trimEnd 去掉
 *  6. ANSI 颜色/属性不进入文本输出（纯文本，shell 粘贴友好）
 */
import type { Terminal, IBufferLine } from "@xterm/xterm";
import type { CommandBlock } from "./command-blocks";
import type { Fold, FoldStore } from "./folds";
import { redactCommandBlockTexts, type CommandBlockRedactionSettings } from "./command-block-redaction";

type FoldInfo = Pick<Fold, "kind" | "count">;

export interface FoldLookup {
  getFold(blockId: number): FoldInfo | undefined;
}

export function formatFoldLabel(count: number): string {
  return `⋯ ${count} ${count === 1 ? "line" : "lines"}`;
}

/** Full folding disposes the end marker and leaves only the command row.
 *  For a prefix fold the live end marker (or cursor) still bounds the tail. */
export function visibleBlockEndLine(block: CommandBlock, cursorAbs: number, fold?: FoldInfo): number {
  if (fold?.kind === "full") return block.start.line;
  return block.end && !block.end.isDisposed ? block.end.line : cursorAbs;
}

/** 一个块的可视行号范围。end 缺失时 fallback 到 cursor 绝对行（块还在写）。 */
export interface BlockRange {
  id: number;
  startLine: number;
  endLine: number;
}

export function resolveBlockRanges(
  term: Terminal,
  blocks: ReadonlyArray<CommandBlock>,
): BlockRange[] {
  const buf = term.buffer.active;
  const cursorAbs = buf.baseY + buf.cursorY;
  const out: BlockRange[] = [];
  for (const b of blocks) {
    if (b.start.isDisposed) continue;
    const endLine = visibleBlockEndLine(b, cursorAbs);
    if (endLine < b.start.line) continue;
    out.push({ id: b.id, startLine: b.start.line, endLine });
  }
  out.sort((a, b) => a.id - b.id);
  return out;
}

export function resolveVisibleBlock(
  term: Terminal,
  block: CommandBlock,
  foldStore?: FoldLookup,
): { lines: IBufferLine[]; foldedLineCount: number } {
  const lines: IBufferLine[] = [];
  if (block.start.isDisposed) return { lines, foldedLineCount: 0 };
  const buf = term.buffer.active;
  const fold = foldStore?.getFold(block.id);
  const endLine = visibleBlockEndLine(block, buf.baseY + buf.cursorY, fold);
  for (let y = block.start.line; y <= endLine; y++) {
    const line = buf.getLine(y);
    if (line) lines.push(line);
  }
  return { lines, foldedLineCount: fold?.count ?? 0 };
}

function sortedLiveBlocks(blocks: ReadonlyArray<CommandBlock>): CommandBlock[] {
  return blocks.filter((block) => !block.start.isDisposed).sort((a, b) => a.id - b.id);
}

interface BlockText {
  text: string;
  foldedLineCount: number;
}

/** Snapshot visible text before asynchronous policy or clipboard access. */
export function snapshotBlockTextsForCopy(
  term: Terminal,
  blocks: ReadonlyArray<CommandBlock>,
  foldStore?: FoldLookup,
): BlockText[] {
  return sortedLiveBlocks(blocks).map((block) => {
    const { lines, foldedLineCount } = resolveVisibleBlock(term, block, foldStore);
    // The visible tail can start in the middle of a hidden soft-wrapped line.
    // Keep that new first line separate from the command.
    const logicalLines = foldedLineCount && lines.length
      ? [lines[0].translateToString().trimEnd(), ...linesToLogicalText(lines.slice(1))]
      : linesToLogicalText(lines);
    return { text: logicalLines.join("\n"), foldedLineCount };
  });
}

/** Apply policy to the terminal text before adding the fold annotation. */
export function formatBlockClipboardText(
  blocks: ReadonlyArray<BlockText>,
  redaction: CommandBlockRedactionSettings,
): string {
  const texts = redactCommandBlockTexts(blocks.map((block) => block.text), redaction);
  return texts.map((text, i) => {
    const count = blocks[i].foldedLineCount;
    if (!count) return text;
    const label = formatFoldLabel(count);
    return text.replace(/^[^\n]*/, (command) => command ? `${command} ${label}` : label);
  }).join("\n");
}

// The AI prefill includes folded output. xterm's private saved rows and
// public buffer views both implement translateToString and isWrapped.
type TextLine = Pick<IBufferLine, "translateToString" | "isWrapped">;

export function extractBlockTextForAi(
  term: Terminal,
  blocks: ReadonlyArray<CommandBlock>,
  foldStore?: Pick<FoldStore, "getFold">,
): string {
  return sortedLiveBlocks(blocks).map((block) => {
    const { lines } = resolveVisibleBlock(term, block, foldStore);
    const saved = foldStore?.getFold(block.id)?.savedLines as TextLine[] | undefined;
    const allLines = saved && lines.length ? [lines[0], ...saved, ...lines.slice(1)] : lines;
    return linesToLogicalText(allLines).join("\n");
  }).join("\n");
}

/** Soft-wrapped rows retain trailing spaces until their logical line is complete. */
export function linesToLogicalText(lines: ReadonlyArray<TextLine>): string[] {
  const result: string[] = [];
  for (const line of lines) {
    const raw = line.translateToString();
    if (line.isWrapped && result.length > 0) {
      result[result.length - 1] += raw;
    } else {
      result.push(raw);
    }
  }
  return result.map((line) => line.trimEnd());
}

/** 行号范围 → 逻辑行数组。保留为 buf-shaped 输入的便利封装（测试用）。 */
export function extractRangeLines(
  buf: { getLine(i: number): IBufferLine | undefined },
  startLine: number,
  endLine: number,
): string[] {
  const lines: IBufferLine[] = [];
  for (let y = startLine; y <= endLine; y++) {
    const line = buf.getLine(y);
    if (line) lines.push(line);
  }
  return linesToLogicalText(lines);
}
