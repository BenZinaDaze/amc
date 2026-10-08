import { Glyph } from "../components/Glyph";
import ompIcon from "../assets/omp.svg";
import { Alerts } from "../components/Alerts";
import { PageHeading } from "../components/PageHeading";
import { AgentsUsagePanel } from "../apps/usage/AgentsUsagePanel";
import { useAgentsUsage } from "../apps/usage/useUsage";
import type { ClaudeCodeStatus, CodexStatus } from "../api";
import type { PageProps } from "./PageProps";

/// Agents 页：全 Agent 用量汇总 + 本机运行时只读检测。
export function AgentsPage({ state, loading, error, notice, setError, setNotice, reload, navigate, claudeCodeStatus, codexStatus }: PageProps & {
  claudeCodeStatus: ClaudeCodeStatus | null;
  codexStatus: CodexStatus | null;
}) {
  const usage = useAgentsUsage();
  // 刷新状态同时重拉运行时列表与用量汇总（定价 → 用量的互斥流程在 hook 内）。
  function refreshAll() {
    usage.refresh();
    void reload();
  }

  return <>
    <PageHeading title="Agents" description="管理代理环境与兼容性检测。" actions={
      <button className="button button-muted refresh-button" aria-label="刷新状态" onClick={refreshAll} disabled={usage.refreshBusy || usage.loading !== null}><Glyph name="refresh" size={16} />刷新状态</button>
    } />
    <Alerts error={error} notice={notice} onErrorClose={() => setError("")} onNoticeClose={() => setNotice("")} />
    {!loading && state && <>
      <AgentsUsagePanel stats={usage.stats} range={usage.selection.range} rangeLabel={usage.selection.label} activeChoice={usage.selection.choice} loading={usage.loading} error={usage.error} pricingNote={usage.pricingNote} onRangeChange={usage.changeRange} onRefresh={usage.refresh} />
      <section className="section-block">
        <div className="section-heading"><div><h2>已接入 Agent</h2><p>只读检测本机可用的 Agent 运行时，不修改其配置。</p></div></div>
        <div className="card-list">
          <article className="item-card">
            <div className="item-icon omp-icon-tile"><img src={ompIcon} alt="" /></div>
            <div className="item-content"><div className="item-title"><h3>OMP</h3><span className={`tag ${state.agent.installed ? "tag-good" : "tag-muted"}`}>{state.agent.installed ? "已检测到" : "未检测到"}</span></div><p>{state.agent.installed ? (state.agent.version || "已找到可执行文件，但无法读取版本") : "未找到可用的 omp 可执行文件"}</p></div>
            <div className="item-actions"><button className="button button-muted" onClick={() => navigate("omp")}>查看详情 <Glyph name="arrow" size={14} /></button></div>
          </article>
          <article className="item-card">
            <div className="item-icon claude-icon-tile"><Glyph name="claude" size={22} /></div>
            <div className="item-content"><div className="item-title"><h3>Claude Code</h3><span className={`tag ${claudeCodeStatus?.installed ? "tag-good" : "tag-muted"}`}>{claudeCodeStatus?.installed ? "已检测到" : "未检测到"}</span></div><p>{claudeCodeStatus ? (claudeCodeStatus.installed ? (claudeCodeStatus.version || "已找到可执行文件，但无法读取版本") : "未找到可用的 claude 可执行文件") : "正在检测 Claude Code CLI…"}</p></div>
            <div className="item-actions"><button className="button button-muted" onClick={() => navigate("claude")}>查看详情 <Glyph name="arrow" size={14} /></button></div>
          </article>
          <article className="item-card">
            <div className="item-icon codex-icon-tile"><Glyph name="codex" size={22} /></div>
            <div className="item-content"><div className="item-title"><h3>Codex CLI</h3><span className={`tag ${codexStatus?.installed ? "tag-good" : "tag-muted"}`}>{codexStatus?.installed ? "已检测到" : "未检测到"}</span></div><p>{codexStatus ? (codexStatus.installed ? (codexStatus.version || "已找到可执行文件，但无法读取版本") : "未找到可用的 codex 可执行文件") : "正在检测 Codex CLI…"}</p></div>
            <div className="item-actions"><button className="button button-muted" onClick={() => navigate("codex")}>查看详情 <Glyph name="arrow" size={14} /></button></div>
          </article>
        </div>
      </section>
    </>}
  </>;
}
