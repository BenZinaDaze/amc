/// 行级 diff 工具：以 git 统一 diff 的形式展示前后文本差异。
/// 算法为 Myers O(ND) 最短编辑脚本，先剪掉公共前后缀再处理中段。

export type DiffRowKind = "add" | "del" | "same";

export type DiffRow = {
  kind: DiffRowKind;
  text: string;
  /** 变更前行号，0 表示该行不存在于变更前 */
  beforeNo: number;
  /** 变更后行号，0 表示该行不存在于变更后 */
  afterNo: number;
};

export type DiffSegment =
  | { kind: "hunk"; header: string; rows: DiffRow[] }
  | { kind: "gap"; count: number; overflow?: boolean };

export type FileState = "new" | "deleted" | "modified" | "same";

export type LineDiff = {
  state: FileState;
  added: number;
  removed: number;
  segments: DiffSegment[];
};

/// 剪掉公共前后缀后超过该行数不再做精确 diff，按整段替换展示。
const MAX_EXACT_LINES = 2000;
/// 单个文件最多渲染的行数，超出部分折叠为提示。
const MAX_RENDER_ROWS = 1000;
/// 每个 hunk 保留的上下文行数，与 git diff -U3 一致。
const CONTEXT_LINES = 3;

function splitLines(text: string): string[] {
  if (!text) return [];
  const lines = text.split("\n");
  if (lines[lines.length - 1] === "") lines.pop(); // 尾部换行是结束符，不算一行内容
  return lines;
}

/// Myers 算法：返回中段行的增删序列（不含行号，行号由 assemble 统一编号）。
function myersEdits(a: string[], b: string[]): { kind: DiffRowKind; text: string }[] {
  const n = a.length;
  const m = b.length;
  if (n === 0) return b.map((text) => ({ kind: "add" as const, text }));
  if (m === 0) return a.map((text) => ({ kind: "del" as const, text }));
  const max = n + m;
  const off = max; // v 的下标 = k + off，覆盖 k ∈ [-max, max]
  let v = new Int32Array(2 * max + 1);
  const trace: Int32Array[] = [];
  let found = -1;
  for (let d = 0; d <= max && found < 0; d++) {
    trace.push(v.slice());
    for (let k = -d; k <= d; k += 2) {
      let x: number;
      if (k === -d || (k !== d && v[off + k - 1] < v[off + k + 1])) x = v[off + k + 1];
      else x = v[off + k - 1] + 1;
      let y = x - k;
      while (x < n && y < m && a[x] === b[y]) { x++; y++; }
      v[off + k] = x;
      if (x >= n && y >= m) { found = d; break; }
    }
  }
  if (found < 0) return [...a.map((text) => ({ kind: "del" as const, text })), ...b.map((text) => ({ kind: "add" as const, text }))];
  // 沿 trace 回溯编辑路径：same 为对角移动，add 为向下（来自 after），del 为向右（来自 before）
  const edits: { kind: DiffRowKind; text: string }[] = [];
  let x = n;
  let y = m;
  for (let d = found; d >= 0; d--) {
    const snapshot = trace[d];
    const k = x - y;
    const prevK = k === -d || (k !== d && snapshot[off + k - 1] < snapshot[off + k + 1]) ? k + 1 : k - 1;
    const prevX = snapshot[off + prevK];
    const prevY = prevX - prevK;
    while (x > prevX && y > prevY) { x--; y--; edits.push({ kind: "same", text: a[x] }); }
    if (d > 0) {
      if (x === prevX) { y--; edits.push({ kind: "add", text: b[y] }); }
      else { x--; edits.push({ kind: "del", text: a[x] }); }
    }
    x = prevX;
    y = prevY;
  }
  edits.reverse();
  return edits;
}

/// 将中段编辑序列与公共前后缀拼成完整行序列并编号。
function assembleRows(beforeLines: string[], prefix: number, endBefore: number, edits: { kind: DiffRowKind; text: string }[]): DiffRow[] {
  const rows: DiffRow[] = [];
  let beforeNo = 0;
  let afterNo = 0;
  const push = (kind: DiffRowKind, text: string) => {
    if (kind === "del") rows.push({ kind, text, beforeNo: ++beforeNo, afterNo: 0 });
    else if (kind === "add") rows.push({ kind, text, beforeNo: 0, afterNo: ++afterNo });
    else rows.push({ kind, text, beforeNo: ++beforeNo, afterNo: ++afterNo });
  };
  for (let i = 0; i < prefix; i++) push("same", beforeLines[i]);
  for (const edit of edits) push(edit.kind, edit.text);
  for (let i = endBefore; i < beforeLines.length; i++) push("same", beforeLines[i]);
  return rows;
}

function countLines(rows: DiffRow[], end: number, kinds: DiffRowKind[]): number {
  let total = 0;
  for (let i = 0; i < end; i++) if (kinds.includes(rows[i].kind)) total++;
  return total;
}

/// git 风格的 hunk 头：@@ -a,b +c,d @@
function hunkHeader(rows: DiffRow[], start: number, end: number): string {
  const beforeCount = countLines(rows, end, ["del", "same"]) - countLines(rows, start, ["del", "same"]);
  const afterCount = countLines(rows, end, ["add", "same"]) - countLines(rows, start, ["add", "same"]);
  const beforeStart = beforeCount === 0 ? countLines(rows, start, ["del", "same"]) : countLines(rows, start, ["del", "same"]) + 1;
  const afterStart = afterCount === 0 ? countLines(rows, start, ["add", "same"]) : countLines(rows, start, ["add", "same"]) + 1;
  const range = (startLine: number, count: number) => `${startLine}${count === 1 ? "" : `,${count}`}`;
  return `@@ -${range(beforeStart, beforeCount)} +${range(afterStart, afterCount)} @@`;
}

/// 将行序列切成 hunk（带 3 行上下文），相隔较远的 hunk 之间以 gap 折叠。
function buildSegments(rows: DiffRow[]): DiffSegment[] {
  if (rows.length > MAX_RENDER_ROWS) {
    return [
      { kind: "hunk", header: "", rows: rows.slice(0, MAX_RENDER_ROWS) },
      { kind: "gap", count: rows.length - MAX_RENDER_ROWS, overflow: true },
    ];
  }
  const changed: number[] = [];
  rows.forEach((row, index) => { if (row.kind !== "same") changed.push(index); });
  if (!changed.length) return [{ kind: "hunk", header: "", rows }];
  const hunks: [number, number][] = [];
  for (const index of changed) {
    const start = Math.max(0, index - CONTEXT_LINES);
    const end = Math.min(rows.length, index + CONTEXT_LINES + 1);
    const last = hunks[hunks.length - 1];
    if (last && start <= last[1]) last[1] = Math.max(last[1], end);
    else hunks.push([start, end]);
  }
  const segments: DiffSegment[] = [];
  let cursor = 0;
  for (const [start, end] of hunks) {
    if (start > cursor) segments.push({ kind: "gap", count: start - cursor });
    segments.push({ kind: "hunk", header: hunkHeader(rows, start, end), rows: rows.slice(start, end) });
    cursor = end;
  }
  if (rows.length > cursor) segments.push({ kind: "gap", count: rows.length - cursor });
  return segments;
}

/// 计算前后文本的行级 diff，供计划预览按 git diff 风格渲染。
export function diffLines(before: string, after: string): LineDiff {
  const beforeLines = splitLines(before);
  const afterLines = splitLines(after);
  if (beforeLines.length === 0 && afterLines.length === 0) return { state: "same", added: 0, removed: 0, segments: [] };
  // 公共前后缀先剪掉，让 Myers 只处理真正差异的中段
  let prefix = 0;
  while (prefix < beforeLines.length && prefix < afterLines.length && beforeLines[prefix] === afterLines[prefix]) prefix++;
  let endBefore = beforeLines.length;
  let endAfter = afterLines.length;
  while (endBefore > prefix && endAfter > prefix && beforeLines[endBefore - 1] === afterLines[endAfter - 1]) { endBefore--; endAfter--; }
  const middleBefore = beforeLines.slice(prefix, endBefore);
  const middleAfter = afterLines.slice(prefix, endAfter);
  const edits = middleBefore.length + middleAfter.length > MAX_EXACT_LINES
    ? [...middleBefore.map((text) => ({ kind: "del" as const, text })), ...middleAfter.map((text) => ({ kind: "add" as const, text }))]
    : myersEdits(middleBefore, middleAfter);
  const rows = assembleRows(beforeLines, prefix, endBefore, edits);
  const added = rows.filter((row) => row.kind === "add").length;
  const removed = rows.filter((row) => row.kind === "del").length;
  const state: FileState = added === 0 && removed === 0 ? "same" : beforeLines.length === 0 ? "new" : afterLines.length === 0 ? "deleted" : "modified";
  return { state, added, removed, segments: buildSegments(rows) };
}
