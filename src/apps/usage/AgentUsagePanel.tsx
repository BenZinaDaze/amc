import { Empty } from "../../components/Empty";
import { Glyph } from "../../components/Glyph";
import type { UsageRange, UsageStats } from "../../api";
import { formatUsageCost, usageNumber, usagePercent, usageTokenNumber, type UsageChoice } from "./format";
import { ModelBreakdownTable } from "./ModelBreakdownTable";
import { UsageRangePicker } from "./UsageRangePicker";
import { UsageTrend } from "./UsageTrend";

/// 单 Agent 用量面板（OMP / Claude Code / Codex 页共用）。
export function AgentUsagePanel({ agentLabel, stats, range, rangeLabel, activeChoice, loading, error, onRangeChange, onRefresh }: {
  agentLabel: string;
  stats: UsageStats | null;
  range: UsageRange;
  rangeLabel: string;
  activeChoice: UsageChoice;
  loading: "sync" | "read" | null;
  error: string;
  onRangeChange: (range: UsageRange, label: string, choice: UsageChoice) => void;
  onRefresh: () => void;
}) {
  return <div className="omp-usage">
    <div className="omp-usage-toolbar">
      <div><h2>用量</h2><p>{agentLabel} 本机会话用量统计 · {stats?.syncedAt ? `上次同步：${new Date(stats.syncedAt).toLocaleString("zh-CN")}` : "尚未同步"}</p></div>
      <UsageRangePicker range={range} label={rangeLabel} activeChoice={activeChoice} onApply={onRangeChange} />
    </div>
    {loading && <div className="loading-panel" role="status"><span className="spinner" />{loading === "sync" ? `正在同步 ${agentLabel} 本地用量…` : "正在读取此时间范围的用量…"}</div>}
    {!loading && error && <div className="alert alert-error" role="alert"><Glyph name="warning" size={18} /><span>{error}。当前没有可显示的最新数据。</span><button className="button button-muted" type="button" onClick={onRefresh}>重试同步</button></div>}
    {!loading && !error && stats && (stats.totalRequests === 0
      ? <Empty icon="grid" title="所选时间范围内没有用量" description="已完成同步，但此时间范围内没有请求。可选择其他时间范围查看。" />
      : <>
        <div className="omp-summary" aria-label="用量汇总">
          <div className="omp-summary-card"><span>请求数</span><strong>{usageNumber.format(stats.totalRequests)}</strong><small>次请求</small></div>
          <div className="omp-summary-card"><span>总 Token</span><strong>{usageTokenNumber.format(stats.totalTokens)}</strong><small>输入 {usageTokenNumber.format(stats.inputTokens)} · 输出 {usageTokenNumber.format(stats.outputTokens)}</small></div>
          <div className="omp-summary-card"><span>缓存命中率</span><strong>{usagePercent.format(stats.cacheRate)}</strong><small>缓存读取 {usageTokenNumber.format(stats.cacheReadTokens)} Token</small></div>
          <div className="omp-summary-card"><span>{stats.unpricedRequests > 0 ? "已计价费用小计" : "估算费用"}</span><strong>{formatUsageCost(stats.totalCost)}</strong><small>USD{stats.unpricedRequests > 0 ? ` · ${usageNumber.format(stats.unpricedRequests)} 次未计价` : ""}</small></div>
        </div>
        <section className="omp-section">
          <div className="section-heading"><div><h2>Token 用量趋势</h2><p>按时间段统计 Token；悬停图表可查看各时间段请求数。</p></div></div>
          <UsageTrend trend={stats.trend} range={range} />
        </section>
        <section className="omp-section">
          <div className="section-heading"><div><h2>模型明细 <span className="count">{stats.byModel.length}</span></h2><p>非实际账单；未计价请求不并入费用。</p></div></div>
          {stats.byModel.length ? <ModelBreakdownTable byModel={stats.byModel} showCacheRate={true} costLabel="估算费用 (USD)" emptyFallback={<p className="omp-no-breakdown">此时间范围内没有可归属的模型明细。</p>} /> : null}
        </section>
      </>)}
  </div>;
}
