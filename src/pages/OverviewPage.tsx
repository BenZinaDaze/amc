import { useEffect, useState } from "react";
import { Glyph } from "../components/Glyph";
import { PageHeading } from "../components/PageHeading";
import { Alerts } from "../components/Alerts";
import { PlanFormModal } from "../apps/subscriptions/PlanFormModal";
import { SubscriptionSection } from "../apps/subscriptions/SubscriptionSection";
import type { SubscriptionStatus } from "../api";
import type { PageProps } from "./PageProps";

/// 概览页：状态统计卡 + 订阅配额卡片流，套餐增删改在本页内完成。
export function OverviewPage({ state, loading, isBusy, error, notice, setError, setNotice, reload, navigate, subscriptions, subscriptionsLoading, subscriptionsError, loadSubscriptions, removeSubscription }: PageProps & {
  subscriptions: SubscriptionStatus[] | null;
  subscriptionsLoading: boolean;
  subscriptionsError: string;
  loadSubscriptions: (force: boolean) => Promise<void>;
  removeSubscription: (id: string) => Promise<void>;
}) {
  const [planForm, setPlanForm] = useState<{ mode: "add" } | { mode: "edit"; status: SubscriptionStatus } | null>(null);
  const [cardMenu, setCardMenu] = useState<{ x: number; y: number; status: SubscriptionStatus; confirming: boolean } | null>(null);

  useEffect(() => {
    void loadSubscriptions(false);
  }, [loadSubscriptions]);

  // Any press, key or scroll outside the card menu dismisses it; menu items
  // stop the mousedown propagation so they still fire.
  useEffect(() => {
    if (!cardMenu) return;
    const close = () => setCardMenu(null);
    const onKeyDown = (event: KeyboardEvent) => { if (event.key === "Escape") close(); };
    window.addEventListener("mousedown", close);
    window.addEventListener("keydown", onKeyDown);
    window.addEventListener("resize", close);
    window.addEventListener("scroll", close, true);
    return () => {
      window.removeEventListener("mousedown", close);
      window.removeEventListener("keydown", onKeyDown);
      window.removeEventListener("resize", close);
      window.removeEventListener("scroll", close, true);
    };
  }, [cardMenu]);
  return <>
    <PageHeading title="概览" description="集中管理 MCP、Skills 与仓库状态。" actions={
      <button className="button button-muted refresh-button" aria-label="刷新状态" onClick={() => void reload()} disabled={loading || isBusy}><Glyph name="refresh" size={16} />刷新状态</button>
    } />
    <Alerts error={error} notice={notice} onErrorClose={() => setError("")} onNoticeClose={() => setNotice("")} />
    {!loading && state && <>
      <div className="stat-grid"><button className="stat-card" onClick={() => navigate("mcp")}><span className="stat-icon purple"><Glyph name="plug" /></span><span className="stat-value">{state.mcp.length}</span><span className="stat-title">MCP 服务</span><small>查看配置来源 <Glyph name="arrow" size={13} /></small></button><button className="stat-card" onClick={() => navigate("skills")}><span className="stat-icon amber"><Glyph name="spark" /></span><span className="stat-value">{state.skills.length}</span><span className="stat-title">已分发 Skills</span><small>管理技能开关与来源 <Glyph name="arrow" size={13} /></small></button><button className="stat-card" onClick={() => navigate("repositories")}><span className="stat-icon mint"><Glyph name="repo" /></span><span className="stat-value">{state.repositories.length}</span><span className="stat-title">Git 仓库</span><small>发现更多技能 <Glyph name="arrow" size={13} /></small></button></div>
      <SubscriptionSection statuses={subscriptions} loading={subscriptionsLoading} error={subscriptionsError} onRefresh={() => void loadSubscriptions(true)} onAddPlan={() => setPlanForm({ mode: "add" })} onMenu={(status, x, y) => setCardMenu({ x, y, status, confirming: false })} />
      {planForm && <PlanFormModal form={planForm} onClose={() => setPlanForm(null)} onSaved={() => { setPlanForm(null); void loadSubscriptions(true); }} />}
      {cardMenu && <div className="card-menu" role="menu" style={{ left: Math.max(8, Math.min(cardMenu.x, window.innerWidth - 156)), top: Math.max(8, Math.min(cardMenu.y, window.innerHeight - 98)) }}>
        <button role="menuitem" onMouseDown={(event) => { event.stopPropagation(); setCardMenu(null); setPlanForm({ mode: "edit", status: cardMenu.status }); }}>
          <Glyph name="edit" size={15} />编辑
        </button>
        {cardMenu.confirming
          ? <button role="menuitem" className="danger" onMouseDown={(event) => { event.stopPropagation(); setCardMenu(null); void removeSubscription(cardMenu.status.id); }}><Glyph name="trash" size={15} />确认删除</button>
          : <button role="menuitem" className="danger" onMouseDown={(event) => { event.stopPropagation(); setCardMenu({ ...cardMenu, confirming: true }); }}><Glyph name="trash" size={15} />删除</button>}
      </div>}
    </>}
  </>;
}
