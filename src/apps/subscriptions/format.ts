import { usageNumber, usageTokenNumber } from "../usage/format";

export function formatResetCountdown(resetsAt: number): string {
  const minutes = Math.floor((resetsAt - Date.now()) / 60_000);
  if (minutes <= 0) return "即将重置";
  const days = Math.floor(minutes / 1440);
  const hours = Math.floor((minutes % 1440) / 60);
  const rest = minutes % 60;
  if (days > 0) return `${days} 天 ${hours} 小时后重置`;
  if (hours > 0) return `${hours} 小时 ${rest} 分后重置`;
  return `${minutes} 分后重置`;
}

export function formatQuotaCount(kind: string, value: number): string {
  return kind === "tokens" ? usageTokenNumber.format(value) : usageNumber.format(value);
}

// `unit: "usd"`/`"cny"` 的额度行存的是分,展示为美元/人民币。
const usageUsd = new Intl.NumberFormat("en-US", { style: "currency", currency: "USD" });
const usageCny = new Intl.NumberFormat("zh-CN", { style: "currency", currency: "CNY" });

export function formatQuotaAmount(unit: string | null, kind: string, value: number): string {
  if (unit === "usd") return usageUsd.format(value / 100);
  if (unit === "cny") return usageCny.format(value / 100);
  return formatQuotaCount(kind, value);
}

export function metricIcon(id: string): string {
  if (id === "tokens" || id === "model") return "spark";
  if (id === "requests") return "agents";
  if (id === "search") return "search";
  if (id === "zread") return "code";
  return "plug";
}
