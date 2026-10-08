import { useState } from "react";
import type { UsageRange, UsageStats } from "../../api";
import { formatUsageTime, usageNumber, usageTokenNumber } from "./format";

export function UsageTrend({ trend, range }: { trend: UsageStats["trend"]; range: UsageRange }) {
  const [active, setActive] = useState<number | null>(null);
  const points = [...trend].sort((a, b) => a.bucket - b.bucket);
  if (!points.length) return <p className="omp-no-breakdown">此时间范围内没有趋势数据。</p>;
  const customBounds = range.startsWith("custom:") ? range.split(":").slice(1).map(Number) : null;
  const hourly = range === "1h" || range === "24h" || (customBounds !== null && customBounds[1] - customBounds[0] <= 48 * 60 * 60 * 1000);

  let max = 1;
  for (const point of points) max = Math.max(max, point.totalTokens);
  const axisLabel = usageTokenNumber.format(max);
  const chartLeft = Math.max(32, 12 + axisLabel.length * 7);
  const chartRight = 690;
  const x = (index: number) => chartLeft + index * (chartRight - chartLeft) / Math.max(1, points.length - 1);
  const y = (value: number) => 170 - value / max * 128;
  const line = points.map((point, index) => `${index ? "L" : "M"} ${x(index)} ${y(point.totalTokens)}`).join(" ");
  const area = `${line} L ${x(points.length - 1)} 170 L ${x(0)} 170 Z`;
  const labels = [...new Set([0, Math.floor((points.length - 1) / 2), points.length - 1])];
  return <div className="omp-chart">
    <svg viewBox="0 0 720 215" preserveAspectRatio="none" role="img" aria-label={`Token 用量趋势，共 ${points.length} 个时间段`} onMouseLeave={() => setActive(null)}>
      <title>Token 用量趋势</title>
      <desc>{points.map((point) => `${formatUsageTime(point.bucket, hourly)}：${usageTokenNumber.format(point.totalTokens)} Token，${usageNumber.format(point.requests)} 次请求`).join("；")}</desc>
      {[42, 106, 170].map((position) => <line key={position} className="omp-chart-grid" x1={chartLeft} y1={position} x2={chartRight} y2={position} />)}
      <text className="omp-chart-label" x={chartLeft - 6} y="47" textAnchor="end">{axisLabel}</text>
      <text className="omp-chart-label" x={chartLeft - 6} y="174" textAnchor="end">0</text>
      {points.length > 1 && <path className="omp-chart-area" d={area} />}
      <path className="omp-chart-line" d={line} />
      {points.map((point, index) => <circle key={`${point.bucket}:${index}`} className={`omp-chart-point${active === index ? " active" : ""}`} cx={x(index)} cy={y(point.totalTokens)} r={active === index ? 5 : 3.5} />)}
      {points.map((point, index) => {
        const left = index === 0 ? chartLeft : (x(index - 1) + x(index)) / 2;
        const right = index === points.length - 1 ? chartRight : (x(index) + x(index + 1)) / 2;
        return <rect key={point.bucket} className="omp-chart-hit" x={left} y="38" width={right - left} height="138"
          tabIndex={0} role="button"
          aria-label={`${formatUsageTime(point.bucket, hourly)}，${usageTokenNumber.format(point.totalTokens)} Token，${usageNumber.format(point.requests)} 次请求`}
          onMouseEnter={() => setActive(index)} onFocus={() => setActive(index)} onBlur={() => setActive(null)} />;
      })}
      {active !== null && points[active] && <g className="omp-chart-tooltip" transform={`translate(${Math.max(140, Math.min(580, x(active)))}, 0)`}>
        <rect x="-130" y="0" width="260" height="37" rx="6" />
        <text textAnchor="middle" y="14">{formatUsageTime(points[active].bucket, hourly)}</text>
        <text textAnchor="middle" y="29">{usageTokenNumber.format(points[active].totalTokens)} Token · {usageNumber.format(points[active].requests)} 次请求</text>
      </g>}
      {labels.map((index) => <text key={index} className="omp-chart-label" x={x(index)} y="203" textAnchor={index === 0 ? "start" : index === points.length - 1 ? "end" : "middle"}>{formatUsageTime(points[index].bucket, hourly)}</text>)}
    </svg>
  </div>;
}
