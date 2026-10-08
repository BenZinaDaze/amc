import { Glyph } from "../../components/Glyph";
import { Empty } from "../../components/Empty";
import antigravityIcon from "../../assets/antigravity.svg";
import sub2apiIcon from "../../assets/sub2api.svg";
import deepseekIcon from "../../assets/deepseek.svg";
import zaiIcon from "../../assets/zai.svg";
import type { SubscriptionStatus } from "../../api";
import { usageNumber } from "../usage/format";
import { formatQuotaAmount, formatQuotaCount, formatResetCountdown, metricIcon } from "./format";

function SubscriptionCard({ status, onMenu }: { status: SubscriptionStatus; onMenu: (status: SubscriptionStatus, x: number, y: number) => void }) {
  return <article className="subscription-card" onContextMenu={(event) => { event.preventDefault(); onMenu(status, event.clientX, event.clientY); }}>
    <div className="subscription-head">
      <span className="subscription-icon"><img src={status.provider === "antigravity" ? antigravityIcon : status.provider === "sub2api" ? sub2apiIcon : status.provider === "deepseek" ? deepseekIcon : zaiIcon} alt="" /></span>
      <div className="subscription-title">
        <h3>{status.title}{status.plan && <span className="tag tag-muted">{status.plan}</span>}</h3>
      </div>
    </div>
    {status.pending
      ? <div className="subscription-pending" role="status"><span className="spinner" />正在查询配额…</div>
      : status.error
        ? <div className="subscription-error" role="alert"><Glyph name="warning" size={16} /><span>{status.error}</span></div>
        : <>
        <div className="quota-list">
          {status.quotas.map((quota) => {
            // 预付费余额不是用量：纯金额行，无百分比无计量条。
            if (quota.kind === "balance") {
              return <div key={quota.label} className="quota-row balance-row">
                <div className="quota-top">
                  <strong>{quota.label}</strong>
                  <span className="balance-value">{quota.used !== null ? formatQuotaAmount(quota.unit, quota.kind, quota.used) : "—"}</span>
                </div>
                {quota.details.length > 0 && <small className="quota-details">{quota.details.map((detail) => `${detail.name} ${usageNumber.format(detail.usage)}`).join(" · ")}</small>}
              </div>;
            }
            const tone = quota.usedPercent >= 90 ? "danger" : quota.usedPercent >= 70 ? "warn" : "ok";
            const counts = quota.used !== null && quota.total !== null
              ? `${formatQuotaAmount(quota.unit, quota.kind, quota.used)} / ${formatQuotaAmount(quota.unit, quota.kind, quota.total)}`
              : quota.used !== null
                ? formatQuotaAmount(quota.unit, quota.kind, quota.used)
                : null;
            return <div key={quota.label} className="quota-row">
              <div className="quota-top">
                <strong>{quota.label}</strong>
                <span className={`quota-state ${tone}`}>{quota.usedPercent.toFixed(1)}%{counts ? ` · ${counts}` : ""}{quota.resetsAt ? ` · ${formatResetCountdown(quota.resetsAt)}` : ""}</span>
              </div>
              <div className="quota-bar" role="img" aria-label={`${quota.label}已用 ${quota.usedPercent.toFixed(1)}%`}><span className={tone} style={{ width: `${Math.min(100, Math.max(2, quota.usedPercent))}%` }} /></div>
              {quota.details.length > 0 && <small className="quota-details">{quota.details.map((detail) => `${detail.name} ${usageNumber.format(detail.usage)}`).join(" · ")}</small>}
            </div>;
          })}
        </div>
        {status.metrics.length > 0 && <div className="metric-row">
          {status.metrics.map((metric) => <span key={metric.id + metric.label} className="metric-chip"><Glyph name={metricIcon(metric.id)} size={14} /><span>{metric.label}</span><strong>{formatQuotaCount(metric.id === "tokens" || metric.id === "model" ? "tokens" : "plain", metric.value)}</strong></span>)}
        </div>}
      </>}
  </article>;
}

export function SubscriptionSection({ statuses, loading, error, onRefresh, onAddPlan, onMenu }: {
  statuses: SubscriptionStatus[] | null;
  loading: boolean;
  error: string;
  onRefresh: () => void;
  onAddPlan: () => void;
  onMenu: (status: SubscriptionStatus, x: number, y: number) => void;
}) {
  return <section className="section-block">
    <div className="section-heading">
      <div><h2>订阅与配额</h2><p>通过官方查询接口读取订阅余量与用量。</p></div>
      <div className="section-actions">
        <button className="button button-muted" onClick={onRefresh} disabled={loading}><Glyph name="refresh" size={16} />{loading ? "查询中…" : "刷新"}</button>
        <button className="button button-muted" onClick={onAddPlan}><Glyph name="plus" size={16} />添加套餐</button>
      </div>
    </div>
    {error && <div className="subscription-error" role="alert"><Glyph name="warning" size={16} /><span>{error}</span></div>}
    <div className="subscription-list">
      {(statuses || []).map((status) => <SubscriptionCard key={status.id} status={status} onMenu={onMenu} />)}
      {statuses && statuses.length === 0 && <Empty icon="spark" title="尚未添加订阅套餐" description="添加套餐并填写名称与 Key 后，即可在此查看对应订阅的配额窗口与用量统计；同一个订阅可添加多份，用名称区分。" action="添加订阅套餐" onClick={onAddPlan} />}
      {!statuses && !error && loading && <div className="loading-panel" role="status"><span className="spinner" />正在查询订阅配额…</div>}
    </div>
  </section>;
}
