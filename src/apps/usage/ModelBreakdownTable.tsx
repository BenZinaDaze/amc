import { useState, type ReactNode } from "react";
import type { UsageStats } from "../../api";
import { formatUsageCost, usageNumber, usagePercent, usageTokenNumber } from "./format";

type ModelSortKey = "requests" | "totalTokens" | "cacheRate" | "cost";

// 模型明细表（总览与单 Agent 页共用）：点击表头按列排序，点一下降序、
// 再点一下升序；未计价费用（null）不参与比较，恒排在最后。
export function ModelBreakdownTable({ byModel, showCacheRate, costLabel, emptyFallback }: {
  byModel: UsageStats["byModel"];
  showCacheRate: boolean;
  costLabel: string;
  emptyFallback: ReactNode;
}) {
  const [sort, setSort] = useState<{ key: ModelSortKey; dir: "desc" | "asc" } | null>(null);
  const sorted = sort
    ? [...byModel].sort((left, right) => {
        const a = left[sort.key];
        const b = right[sort.key];
        if (a === null && b === null) return 0;
        if (a === null) return 1;
        if (b === null) return -1;
        return (a - b) * (sort.dir === "desc" ? -1 : 1);
      })
    : byModel;
  const columns: { key: ModelSortKey; label: string }[] = [
    { key: "requests", label: "请求数" },
    { key: "totalTokens", label: "总 Token" },
    ...(showCacheRate ? [{ key: "cacheRate" as ModelSortKey, label: "缓存命中率" }] : []),
    { key: "cost", label: costLabel },
  ];
  if (!byModel.length) return emptyFallback;
  return <div className="omp-table-wrap"><table className="omp-table"><thead><tr><th scope="col">模型</th>{columns.map((column) => {
    const active = sort?.key === column.key;
    const dir = active ? sort?.dir ?? null : null;
    return <th key={column.key} scope="col" aria-sort={dir ? (dir === "desc" ? "descending" : "ascending") : undefined}><button type="button" className="omp-sort-btn" onClick={() => setSort((current) => current?.key === column.key ? { key: column.key, dir: current.dir === "desc" ? "asc" : "desc" } : { key: column.key, dir: "desc" })}>{column.label}{dir && <span className="omp-sort-arrow" aria-hidden="true">{dir === "desc" ? "↓" : "↑"}</span>}</button></th>;
  })}</tr></thead><tbody>{sorted.map((model) => <tr key={model.model}><th scope="row"><strong>{model.model || "未知模型"}</strong></th><td>{usageNumber.format(model.requests)}</td><td>{usageTokenNumber.format(model.totalTokens)}</td>{showCacheRate && <td>{usagePercent.format(model.cacheRate)}</td>}<td>{formatUsageCost(model.cost)}{model.unpricedRequests > 0 && <small className="omp-unpriced">（{usageNumber.format(model.unpricedRequests)} 次未计价）</small>}</td></tr>)}</tbody></table></div>;
}
