import { useEffect, useState } from "react";
import { Glyph } from "../components/Glyph";
import ompIcon from "../assets/omp.svg";
import { Alerts } from "../components/Alerts";
import { PageHeading } from "../components/PageHeading";
import { AgentsUsagePanel } from "../apps/usage/AgentsUsagePanel";
import { useAgentsUsage } from "../apps/usage/useUsage";
import { api, type AgentUpdateSource, type AppUpdate, type ClaudeCodeStatus, type CodexStatus } from "../api";
import { isNewerVersion, versionToken } from "../utils";
import type { PageProps } from "./PageProps";

/// 检测到运行时并读到版本后再查一次最新版本；版本串变化（刷新状态）
/// 时重查，查询失败静默——更新提示是纯增益信息，不打扰主流程。
function useAgentUpdate(source: AgentUpdateSource, version: string) {
  const [update, setUpdate] = useState<AppUpdate | null>(null);
  useEffect(() => {
    setUpdate(null);
    if (!version) return undefined;
    let cancelled = false;
    api.checkAgentUpdate(source)
      .then((info) => { if (!cancelled) setUpdate(info); })
      .catch(() => {});
    return () => { cancelled = true; };
  }, [source, version]);
  return update;
}

/// 有新版时的更新按钮：点击调用 Agent 自带的更新命令，完成后由重新
/// 检测驱动版本与标签刷新。
function UpdateTag({ update, current, updating, disabled, onUpdate }: { update: AppUpdate; current: string; updating: boolean; disabled: boolean; onUpdate: () => void }) {
  if (!isNewerVersion(update.latest, current)) return null;
  return <button type="button" className="tag tag-warn" disabled={disabled} title={updating ? "正在更新…" : `点击自动更新到 ${update.latest}`} onClick={onUpdate}>{updating ? "正在更新…" : `更新到 ${update.latest}`}</button>;
}

/// Agents 页：全 Agent 用量汇总 + 本机运行时检测；有新版时点击标签即
/// 调用各 Agent 自带的更新命令。
export function AgentsPage({ state, loading, error, notice, setError, setNotice, reload, navigate, operation, claudeCodeStatus, codexStatus, refreshStatuses }: PageProps & {
  claudeCodeStatus: ClaudeCodeStatus | null;
  codexStatus: CodexStatus | null;
  refreshStatuses: () => void;
}) {
  const usage = useAgentsUsage();
  const [updating, setUpdating] = useState<AgentUpdateSource | null>(null);
  // 刷新状态同时重拉运行时列表与用量汇总（定价 → 用量的互斥流程在 hook 内）。
  function refreshAll() {
    usage.refresh();
    void reload();
  }

  const ompVersion = state?.agent.installed ? versionToken(state.agent.version) : "";
  const claudeVersion = claudeCodeStatus?.installed ? versionToken(claudeCodeStatus.version) : "";
  const codexVersion = codexStatus?.installed ? versionToken(codexStatus.version) : "";
  const ompUpdate = useAgentUpdate("omp", ompVersion);
  const claudeUpdate = useAgentUpdate("claude-code", claudeVersion);
  const codexUpdate = useAgentUpdate("codex", codexVersion);

  /// 点击更新：走全局 busy/错误通道调用 Agent 自带的更新命令，成功后
  /// 重测版本（OMP 在 state.agent，Claude/Codex 走状态接口）驱动标签消失。
  async function runUpdate(source: AgentUpdateSource, label: string) {
    setUpdating(source);
    const message = await operation(`更新 ${label}`, () => api.updateAgent(source));
    setUpdating(null);
    if (message === undefined) return;
    setNotice(message || `${label} 已更新`);
    refreshStatuses();
    if (source === "omp") void reload();
  }

  return <>
    <PageHeading title="Agents" description="管理代理环境与兼容性检测。" actions={
      <button className="button button-muted refresh-button" aria-label="刷新状态" onClick={refreshAll} disabled={usage.refreshBusy || usage.loading !== null}><Glyph name="refresh" size={16} />刷新状态</button>
    } />
    <Alerts error={error} notice={notice} onErrorClose={() => setError("")} onNoticeClose={() => setNotice("")} />
    {!loading && state && <>
      <AgentsUsagePanel stats={usage.stats} range={usage.selection.range} rangeLabel={usage.selection.label} activeChoice={usage.selection.choice} loading={usage.loading} error={usage.error} pricingNote={usage.pricingNote} onRangeChange={usage.changeRange} onRefresh={usage.refresh} />
      <section className="section-block">
        <div className="section-heading"><div><h2>已接入 Agent</h2><p>检测本机可用的 Agent 运行时；更新只调用各 Agent 自带的更新命令，不改动其配置。</p></div></div>
        <div className="card-list">
          <article className="item-card">
            <div className="item-icon omp-icon-tile"><img src={ompIcon} alt="" /></div>
            <div className="item-content"><div className="item-title"><h3>OMP</h3><span className={`tag ${state.agent.installed ? "tag-good" : "tag-muted"}`}>{state.agent.installed ? "已安装" : "未安装"}</span>{ompUpdate && <UpdateTag update={ompUpdate} current={ompVersion} updating={updating === "omp"} disabled={updating !== null} onUpdate={() => void runUpdate("omp", "OMP")} />}</div><p>{state.agent.installed ? (state.agent.version || "已找到可执行文件，但无法读取版本") : "未找到可用的 omp 可执行文件"}</p></div>
            <div className="item-actions"><button className="button button-muted" onClick={() => navigate("omp")}>查看详情 <Glyph name="arrow" size={14} /></button></div>
          </article>
          <article className="item-card">
            <div className="item-icon claude-icon-tile"><Glyph name="claude" size={22} /></div>
            <div className="item-content"><div className="item-title"><h3>Claude Code</h3><span className={`tag ${claudeCodeStatus?.installed ? "tag-good" : "tag-muted"}`}>{claudeCodeStatus?.installed ? "已安装" : "未安装"}</span>{claudeUpdate && <UpdateTag update={claudeUpdate} current={claudeVersion} updating={updating === "claude-code"} disabled={updating !== null} onUpdate={() => void runUpdate("claude-code", "Claude Code")} />}</div><p>{claudeCodeStatus ? (claudeCodeStatus.installed ? (claudeCodeStatus.version || "已找到可执行文件，但无法读取版本") : "未找到可用的 claude 可执行文件") : "正在检测 Claude Code CLI…"}</p></div>
            <div className="item-actions"><button className="button button-muted" onClick={() => navigate("claude")}>查看详情 <Glyph name="arrow" size={14} /></button></div>
          </article>
          <article className="item-card">
            <div className="item-icon codex-icon-tile"><Glyph name="codex" size={22} /></div>
            <div className="item-content"><div className="item-title"><h3>Codex CLI</h3><span className={`tag ${codexStatus?.installed ? "tag-good" : "tag-muted"}`}>{codexStatus?.installed ? "已安装" : "未安装"}</span>{codexUpdate && <UpdateTag update={codexUpdate} current={codexVersion} updating={updating === "codex"} disabled={updating !== null} onUpdate={() => void runUpdate("codex", "Codex")} />}</div><p>{codexStatus ? (codexStatus.installed ? (codexStatus.version || "已找到可执行文件，但无法读取版本") : "未找到可用的 codex 可执行文件") : "正在检测 Codex CLI…"}</p></div>
            <div className="item-actions"><button className="button button-muted" onClick={() => navigate("codex")}>查看详情 <Glyph name="arrow" size={14} /></button></div>
          </article>
        </div>
      </section>
    </>}
  </>;
}
