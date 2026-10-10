import { Fragment, useMemo } from "react";
import { Glyph } from "../../components/Glyph";
import type { Plan } from "../../api";
import { diffLines, type FileState } from "./lineDiff";

export type PlanChange = Plan["changes"][number];

const stateTag: Record<FileState, { label: string; className: string } | null> = {
  new: { label: "新建文件", className: "tag tag-good" },
  deleted: { label: "删除文件", className: "tag tag-warn" },
  modified: null,
  same: null,
};

/// 单个文件的 git 风格统一 diff：红色为删除行、绿色为新增行，
/// 长文件按 hunk 展示并折叠未变更区段。
export function FileDiff({ change }: { change: PlanChange }) {
  const diff = useMemo(() => diffLines(change.before, change.after), [change.before, change.after]);
  const tag = stateTag[diff.state];
  return <div className="diff-card">
    <div className="diff-path">
      <Glyph name="code" size={16} />
      <span className="diff-path-text">{change.path}</span>
      {tag && <span className={tag.className}>{tag.label}</span>}
      <span className="diff-stats">
        {diff.added > 0 && <span className="diff-stat-add">+{diff.added}</span>}
        {diff.removed > 0 && <span className="diff-stat-del">-{diff.removed}</span>}
      </span>
    </div>
    {diff.state === "same" ? <div className="diff-gap">内容无变化</div> : <div className="diff-body">
      {diff.segments.map((segment, index) => segment.kind === "gap" ? (
        <div key={index} className="diff-gap">
          <span>{segment.overflow ? `其余 ${segment.count} 行未显示` : `··· 未变更 ${segment.count} 行 ···`}</span>
        </div>
      ) : (
        <Fragment key={index}>
          {segment.header && <div className="diff-row diff-hunk">{segment.header}</div>}
          {segment.rows.map((row, rowIndex) => (
            <div key={rowIndex} className={`diff-row diff-${row.kind}`}>
              <span className="diff-no">{row.beforeNo || ""}</span>
              <span className="diff-no">{row.afterNo || ""}</span>
              <span className="diff-sign">{row.kind === "add" ? "+" : row.kind === "del" ? "-" : ""}</span>
              <span className="diff-text">{row.text || " "}</span>
            </div>
          ))}
        </Fragment>
      ))}
    </div>}
  </div>;
}
