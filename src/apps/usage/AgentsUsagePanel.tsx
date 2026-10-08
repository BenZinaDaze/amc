import { Empty } from "../../components/Empty";
import { Glyph } from "../../components/Glyph";
import type { UsageRange, UsageStats } from "../../api";
import { formatUsageCost, usageNumber, usageTokenNumber, type UsageChoice } from "./format";
import { ModelBreakdownTable } from "./ModelBreakdownTable";
import { UsageRangePicker } from "./UsageRangePicker";

/// Agents 汇总用量面板：所有 Agent 的本机会话合计。
export function AgentsUsagePanel({ stats, range, rangeLabel, activeChoice, loading, error, pricingNote, onRangeChange, onRefresh }: {
  stats: UsageStats | null;
  range: UsageRange;
  rangeLabel: string;
  activeChoice: UsageChoice;
  loading: "sync" | "read" | null;
  error: string;
  pricingNote: string;
  onRangeChange: (range: UsageRange, label: string, choice: UsageChoice) => void;
  onRefresh: () => void;
}) {
  return <section className="section-block agents-usage" aria-label="Agent 用量统计">
    <div className="omp-usage-toolbar">
      <div><h2>用量统计</h2><p>所有 Agent 的本机会话汇总 · {stats?.syncedAt ? `上次同步：${new Date(stats.syncedAt).toLocaleString("zh-CN")}` : "尚未同步"}</p></div>
      <div className="section-actions">
        <UsageRangePicker range={range} label={rangeLabel} activeChoice={activeChoice} onApply={onRangeChange} />
      </div>
    </div>
    {pricingNote && <p className="omp-pricing-note" role="status"><Glyph name="check" size={14} />{pricingNote}</p>}
    {loading && <div className="loading-panel" role="status"><span className="spinner" />{loading === "sync" ? "正在同步所有 Agent 本地用量…" : "正在读取此时间范围的用量…"}</div>}
    {!loading && error && <div className="alert alert-error" role="alert"><Glyph name="warning" size={18} /><span>{error}。当前没有可显示的最新数据。</span><button className="button button-muted" type="button" onClick={onRefresh}>重试同步</button></div>}
    {!loading && !error && stats && (stats.totalRequests === 0
      ? <Empty icon="grid" title="所选时间范围内没有用量" description="已完成同步，但所有 Agent 在此时间范围内都没有请求。可选择其他时间范围，或点击右上角「刷新状态」重新同步。" />
      : <>
        <div className="omp-summary" aria-label="Agent 用量汇总">
          <div className="omp-summary-card"><span>请求数</span><strong>{usageNumber.format(stats.totalRequests)}</strong><small>次请求</small></div>
          <div className="omp-summary-card"><span>总 Token</span><strong>{usageTokenNumber.format(stats.totalTokens)}</strong><small>输入 {usageTokenNumber.format(stats.inputTokens)} · 输出 {usageTokenNumber.format(stats.outputTokens)}</small></div>
          <div className="omp-summary-card"><span>{stats.unpricedRequests > 0 ? "已计价费用小计" : "估算费用"}</span><strong>{formatUsageCost(stats.totalCost)}</strong><small>USD{stats.unpricedRequests > 0 ? ` · ${usageNumber.format(stats.unpricedRequests)} 次未计价` : ""}</small></div>
        </div>
        <div className="section-heading model-breakdown-heading"><div><h2>模型明细 <span className="count">{stats.byModel.length}</span></h2><p>各模型的请求数、Token 总量与已计价费用小计；未计价请求不并入费用。</p></div></div>
        {stats.byModel.length ? <ModelBreakdownTable byModel={stats.byModel} showCacheRate={false} costLabel="已计价费用 (USD)" emptyFallback={null} /> : null}
      </>)}
  </section>;
}
