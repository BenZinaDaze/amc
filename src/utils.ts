/// 与业务无关的小工具：错误文案 + 本地日期换算（DateCalendar 与用量
/// 时间范围选择共用）。

export function errorText(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

export function localDateString(date: Date): string {
  return `${date.getFullYear().toString().padStart(4, "0")}-${String(date.getMonth() + 1).padStart(2, "0")}-${String(date.getDate()).padStart(2, "0")}`;
}

export function localMidnight(value: string): Date | null {
  const parts = /^(\d{4})-(\d{2})-(\d{2})$/.exec(value);
  if (!parts) return null;
  const [, year, month, day] = parts.map(Number);
  const date = new Date(0);
  date.setFullYear(year, month - 1, day);
  date.setHours(0, 0, 0, 0);
  return date.getFullYear() === year && date.getMonth() === month - 1 && date.getDate() === day ? date : null;
}

export const calendarWeekdays = ["一", "二", "三", "四", "五", "六", "日"];

/// 某月按周一开头的 7 列网格展开，跨月空位补 null。
export function calendarMonthCells(year: number, month: number): (string | null)[] {
  const cells: (string | null)[] = Array((new Date(year, month, 1).getDay() + 6) % 7).fill(null);
  const days = new Date(year, month + 1, 0).getDate();
  for (let day = 1; day <= days; day++) cells.push(localDateString(new Date(year, month, day)));
  while (cells.length % 7) cells.push(null);
  return cells;
}

export function shiftDate(date: Date, days: number): Date {
  const shifted = new Date(date);
  shifted.setDate(shifted.getDate() + days);
  return shifted;
}
