import type { ReactNode } from "react";
import { Glyph } from "../components/Glyph";
import { Alerts } from "../components/Alerts";
import { PageHeading } from "../components/PageHeading";
import { AgentUsagePanel } from "../apps/usage/AgentUsagePanel";
import { useAgentUsage } from "../apps/usage/useUsage";
import type { PageProps } from "./PageProps";

/// OMP / Claude Code / Codex 三个详情页共用：参数化 Agent 标识与
/// 页头版本徽章，用量加载与时间范围切换由 useAgentUsage 承担。
export function AgentUsagePage({ agentId, name, title, refreshName, pill, error, notice, setError, setNotice }: {
  agentId: string;
  name: string;
  title: string;
  refreshName: string;
  pill?: ReactNode;
} & Pick<PageProps, "error" | "notice" | "setError" | "setNotice">) {
  const usage = useAgentUsage(agentId, name);
  return <>
    <PageHeading title={title} pill={pill} actions={
      <button className="button button-muted refresh-button" aria-label={`刷新 ${refreshName} 用量`} onClick={usage.refresh} disabled={usage.loading !== null}><Glyph name="refresh" size={16} />刷新用量</button>
    } />
    <Alerts error={error} notice={notice} onErrorClose={() => setError("")} onNoticeClose={() => setNotice("")} />
    <AgentUsagePanel agentLabel={refreshName} stats={usage.stats} range={usage.selection.range} rangeLabel={usage.selection.label} activeChoice={usage.selection.choice} loading={usage.loading} error={usage.error} onRangeChange={usage.changeRange} onRefresh={usage.refresh} />
  </>;
}
