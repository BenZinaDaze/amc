import { useState } from "react";
import type { UsageRange, UsageStats } from "../../api";
import { formatUsageTime, usageNumber, usageTokenNumber } from "./format";

const MS_MIN = 60_000;
const MS_HOUR = 3_600_000;
const MS_DAY = 86_400_000;
const MS_WEEK = 7 * MS_DAY;

/// 桶宽与窗口两端须与后端 `usage/mod.rs` 的 UsageRange::parse 一致：
/// 后端只为有记录的时段建桶、不补零，前端按真实时间把桶放回等距
/// 槽位，空闲时段留空而不是被压缩成连续柱。窗口未知（"all"）或
/// 数据越界（时钟偏差）时回退为数据自身的时间跨度。
function trendGrid(range: UsageRange, first: number, last: number): { step: number; start: number; end: number } {
  let step: number;
  let start: number | null = null;
  let end: number | null = null;
  if (range.startsWith("custom:")) {
    const [, a, b] = range.split(":").map(Number);
    const duration = b - a;
    step = duration <= 48 * MS_HOUR ? MS_HOUR : duration <= 180 * MS_DAY ? MS_DAY : MS_WEEK;
    start = Math.floor(a / step) * step;
    end = Math.ceil(b / step) * step - step;
  } else if (range === "1h") {
    step = 5 * MS_MIN;
    start = Math.floor((Date.now() - MS_HOUR) / step) * step;
    end = Math.floor(Date.now() / step) * step;
  } else if (range === "24h") {
    step = MS_HOUR;
    start = Math.floor((Date.now() - 24 * MS_HOUR) / step) * step;
    end = Math.floor(Date.now() / step) * step;
  } else if (range === "7d" || range === "14d" || range === "30d" || range === "90d") {
    step = MS_DAY;
    const days = range === "7d" ? 7 : range === "14d" ? 14 : range === "30d" ? 30 : 90;
    start = Math.floor((Date.now() - days * MS_DAY) / step) * step;
    end = Math.floor(Date.now() / step) * step;
  } else {
    step = MS_WEEK; // "all" 及未知取值：窗口未知，按数据跨度
  }
  start = Math.min(start ?? first, first);
  end = Math.max(end ?? last, last);
  // 防御极端自定义区间撑爆槽位（>10 年按周也上千槽）：退回数据跨度。
  if ((end - start) / step > 600) { start = first; end = last; }
  return { step, start, end };
}

/// Token 用量趋势柱状图：桶按真实时间放回窗口槽位（24h / ≤48h 自定义
/// 区间为按小时一柱，1h 区间为 5 分钟一柱，更长区间为天/周），空闲
/// 时段留空。悬停/键盘聚焦柱区查看该时间段的请求数。
export function UsageTrend({ trend, range }: { trend: UsageStats["trend"]; range: UsageRange }) {
  const [active, setActive] = useState<number | null>(null);
  const points = [...trend].sort((a, b) => a.bucket - b.bucket);
  if (!points.length) return <p className="omp-no-breakdown">此时间范围内没有趋势数据。</p>;
  const customBounds = range.startsWith("custom:") ? range.split(":").slice(1).map(Number) : null;
  const hourly = range === "1h" || range === "24h" || (customBounds !== null && customBounds[1] - customBounds[0] <= 48 * 60 * 60 * 1000);

  const { step, start, end } = trendGrid(range, points[0].bucket, points[points.length - 1].bucket);
  const slotCount = Math.round((end - start) / step) + 1;

  let max = 1;
  for (const point of points) max = Math.max(max, point.totalTokens);
  const axisLabel = usageTokenNumber.format(max);
  const chartLeft = Math.max(32, 12 + axisLabel.length * 7);
  const chartRight = 690;
  const slot = (chartRight - chartLeft) / Math.max(1, slotCount);
  const barWidth = Math.max(2, Math.min(26, slot * 0.62));
  const x = (index: number) => chartLeft + slot * index + slot / 2;
  const y = (value: number) => 170 - value / max * 128;
  const slotOf = (bucket: number) => Math.round((bucket - start) / step);
  const labels = [...new Set([0, Math.floor((slotCount - 1) / 2), slotCount - 1])];
  const activePoint = active !== null ? points.find((point) => slotOf(point.bucket) === active) : undefined;
  return <div className="omp-chart">
    <svg viewBox="0 0 720 215" preserveAspectRatio="none" role="img" aria-label={`Token 用量趋势，共 ${slotCount} 个时间段`} onMouseLeave={() => setActive(null)}>
      <title>Token 用量趋势</title>
      <desc>{points.map((point) => `${formatUsageTime(point.bucket, hourly)}：${usageTokenNumber.format(point.totalTokens)} Token，${usageNumber.format(point.requests)} 次请求`).join("；")}</desc>
      {[42, 106, 170].map((position) => <line key={position} className="omp-chart-grid" x1={chartLeft} y1={position} x2={chartRight} y2={position} />)}
      <text className="omp-chart-label" x={chartLeft - 6} y="47" textAnchor="end">{axisLabel}</text>
      <text className="omp-chart-label" x={chartLeft - 6} y="174" textAnchor="end">0</text>
      {points.map((point, index) => {
        const top = y(point.totalTokens);
        return <rect key={`${point.bucket}:${index}`} className={`omp-chart-bar${activePoint === point ? " active" : ""}`}
          x={x(slotOf(point.bucket)) - barWidth / 2} y={top} width={barWidth} height={Math.max(1, 170 - top)} rx={Math.min(3, barWidth / 2)} />;
      })}
      {points.map((point) => {
        const slotIndex = slotOf(point.bucket);
        const left = chartLeft + slot * slotIndex;
        const right = left + slot;
        return <rect key={point.bucket} className="omp-chart-hit" x={left} y="38" width={right - left} height="138"
          tabIndex={0} role="button"
          aria-label={`${formatUsageTime(point.bucket, hourly)}，${usageTokenNumber.format(point.totalTokens)} Token，${usageNumber.format(point.requests)} 次请求`}
          onMouseEnter={() => setActive(slotIndex)} onFocus={() => setActive(slotIndex)} onBlur={() => setActive(null)} />;
      })}
      {activePoint && <g className="omp-chart-tooltip" transform={`translate(${Math.max(140, Math.min(580, x(slotOf(activePoint.bucket))))}, 0)`}>
        <rect x="-130" y="0" width="260" height="37" rx="6" />
        <text textAnchor="middle" y="14">{formatUsageTime(activePoint.bucket, hourly)}</text>
        <text textAnchor="middle" y="29">{usageTokenNumber.format(activePoint.totalTokens)} Token · {usageNumber.format(activePoint.requests)} 次请求</text>
      </g>}
      {labels.map((index) => <text key={index} className="omp-chart-label" x={x(index)} y="203" textAnchor={index === 0 ? "start" : index === slotCount - 1 ? "end" : "middle"}>{formatUsageTime(start + index * step, hourly)}</text>)}
    </svg>
  </div>;
}
