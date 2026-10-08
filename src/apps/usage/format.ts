import type { UsageRange } from "../../api";
import { localDateString } from "../../utils";

export type UsagePreset = "today" | "yesterday" | "24h" | "7d" | "14d" | "30d" | "thisMonth" | "lastMonth" | "1h" | "90d" | "all";
export type UsageChoice = UsagePreset | "custom";

export const usageRanges: { value: UsagePreset; label: string }[] = [
  { value: "today", label: "今天" },
  { value: "yesterday", label: "昨天" },
  { value: "24h", label: "近24小时" },
  { value: "7d", label: "近7天" },
  { value: "14d", label: "近14天" },
  { value: "30d", label: "近30天" },
  { value: "thisMonth", label: "本月" },
  { value: "lastMonth", label: "上月" },
];

export const extraUsageRanges: { value: UsagePreset; label: string }[] = [
  { value: "1h", label: "近1小时" },
  { value: "90d", label: "近90天" },
  { value: "all", label: "全部" },
];

export function calendarRange(choice: UsagePreset): UsageRange | null {
  const now = new Date();
  const today = new Date(now.getFullYear(), now.getMonth(), now.getDate());
  const tomorrow = new Date(now.getFullYear(), now.getMonth(), now.getDate() + 1);
  const monthStart = new Date(now.getFullYear(), now.getMonth(), 1);
  let start: Date;
  let end: Date;
  switch (choice) {
    case "today": [start, end] = [today, tomorrow]; break;
    case "yesterday": [start, end] = [new Date(now.getFullYear(), now.getMonth(), now.getDate() - 1), today]; break;
    case "thisMonth": [start, end] = [monthStart, new Date(now.getFullYear(), now.getMonth() + 1, 1)]; break;
    case "lastMonth": [start, end] = [new Date(now.getFullYear(), now.getMonth() - 1, 1), monthStart]; break;
    default: return null;
  }
  return `custom:${start.getTime()}:${end.getTime()}`;
}

export function dateFieldsForChoice(choice: UsageChoice, range: UsageRange): { start: string; end: string } {
  if (choice === "custom" && range.startsWith("custom:")) {
    const [, start, end] = range.split(":").map(Number);
    const lastDay = new Date(end);
    lastDay.setDate(lastDay.getDate() - 1);
    return { start: localDateString(new Date(start)), end: localDateString(lastDay) };
  }
  if (choice !== "custom") {
    const calendar = calendarRange(choice);
    if (calendar) return dateFieldsForChoice("custom", calendar);
  }
  if (choice === "all") return { start: "", end: localDateString(new Date()) };
  const now = new Date();
  const start = new Date(now);
  const rollingDays: Partial<Record<UsagePreset, number>> = { "24h": 1, "7d": 7, "14d": 14, "30d": 30, "90d": 90 };
  start.setDate(start.getDate() - (rollingDays[choice as UsagePreset] ?? 0));
  if (choice === "1h") start.setHours(start.getHours() - 1);
  return { start: localDateString(start), end: localDateString(now) };
}

export const usageNumber = new Intl.NumberFormat("zh-CN");
export const usageTokenNumber = { format: (tokens: number) => tokens >= 1_000_000_000
  ? `${(tokens / 1_000_000_000).toFixed(2)}B`
  : tokens >= 1_000_000 ? `${(tokens / 1_000_000).toFixed(2)}M`
  : tokens >= 1_000 ? `${(tokens / 1_000).toFixed(2)}K` : usageNumber.format(tokens) };
export const usagePercent = new Intl.NumberFormat("zh-CN", { style: "percent", maximumFractionDigits: 1 });
const usageCost = new Intl.NumberFormat("en-US", { style: "currency", currency: "USD", maximumFractionDigits: 4 });

export function formatUsageCost(cost: number | null) {
  if (cost === null) return "—";
  return cost > 0 && cost < 0.0001 ? "< $0.0001" : usageCost.format(cost);
}

export function formatUsageTime(timestamp: number, hourly: boolean) {
  return new Intl.DateTimeFormat("zh-CN", hourly
    ? { month: "numeric", day: "numeric", hour: "2-digit", minute: "2-digit" }
    : { year: "numeric", month: "numeric", day: "numeric" }).format(new Date(timestamp));
}
