import { useEffect, useRef, useState, type KeyboardEvent as ReactKeyboardEvent } from "react";
import { Glyph } from "./Glyph";
import { calendarMonthCells, calendarWeekdays, localDateString, localMidnight, shiftDate } from "../utils";

/// 自绘日期下拉：单月网格，替代原生 <input type="date">。固定定位于
/// 触发字段下方（视口放不下时上翻），点击或键盘（方向键/Home/End/
/// PageUp/PageDown，加 Alt 翻年）选择日期，Escape 或点击外部关闭。
export function DateCalendar({ field, value, onPick, onClose }: { field: HTMLElement; value: string; onPick: (iso: string) => void; onClose: () => void }) {
  const initial = localMidnight(value) ?? new Date();
  const [view, setView] = useState(() => new Date(initial.getFullYear(), initial.getMonth(), 1));
  const [focusDay, setFocusDay] = useState(() => value || localDateString(new Date()));
  const [focusTick, setFocusTick] = useState(0);
  const [position, setPosition] = useState<{ left: number; top: number } | null>(null);
  const popRef = useRef<HTMLDivElement>(null);
  const gridRef = useRef<HTMLDivElement>(null);

  useEffect(() => {
    function place() {
      const rect = field.getBoundingClientRect();
      const height = popRef.current?.offsetHeight ?? 250;
      const width = popRef.current?.offsetWidth ?? 256;
      const below = rect.bottom + 6 + height <= window.innerHeight;
      setPosition({
        left: Math.round(Math.min(Math.max(8, rect.left), window.innerWidth - width - 8)),
        top: Math.round(below ? rect.bottom + 6 : Math.max(8, rect.top - height - 6)),
      });
    }
    place();
    window.addEventListener("resize", place);
    document.addEventListener("scroll", place, true);
    return () => {
      window.removeEventListener("resize", place);
      document.removeEventListener("scroll", place, true);
    };
  }, [field, view]);

  useEffect(() => {
    if (!focusTick) return;
    gridRef.current?.querySelector<HTMLButtonElement>(`button[data-date="${focusDay}"]`)?.focus();
  }, [focusTick]);

  useEffect(() => {
    function onKeyDown(event: KeyboardEvent) {
      if (event.key !== "Escape") return;
      event.preventDefault();
      event.stopPropagation();
      onClose();
    }
    function onPointerDown(event: PointerEvent) {
      const target = event.target instanceof Node ? event.target : null;
      if (target && !popRef.current?.contains(target) && !field.contains(target)) onClose();
    }
    document.addEventListener("keydown", onKeyDown, true);
    document.addEventListener("pointerdown", onPointerDown, true);
    return () => {
      document.removeEventListener("keydown", onKeyDown, true);
      document.removeEventListener("pointerdown", onPointerDown, true);
    };
  }, [field, onClose]);

  function moveFocus(day: Date) {
    setFocusDay(localDateString(day));
    setFocusTick((tick) => tick + 1);
    setView((current) => {
      const first = new Date(day.getFullYear(), day.getMonth(), 1);
      const anchor = new Date(current.getFullYear(), current.getMonth(), 1);
      return first.getTime() === anchor.getTime() ? current : first;
    });
  }

  function onKeyDown(event: ReactKeyboardEvent<HTMLDivElement>) {
    const base = localMidnight(focusDay);
    if (!base) return;
    let next: Date | null = null;
    switch (event.key) {
      case "ArrowLeft": next = shiftDate(base, -1); break;
      case "ArrowRight": next = shiftDate(base, 1); break;
      case "ArrowUp": next = shiftDate(base, -7); break;
      case "ArrowDown": next = shiftDate(base, 7); break;
      case "Home": next = shiftDate(base, -((base.getDay() + 6) % 7)); break;
      case "End": next = shiftDate(base, 6 - ((base.getDay() + 6) % 7)); break;
      case "PageUp": next = new Date(base.getFullYear(), base.getMonth() - (event.altKey ? 12 : 1), 1); break;
      case "PageDown": next = new Date(base.getFullYear(), base.getMonth() + (event.altKey ? 12 : 1), 1); break;
    }
    if (!next) return;
    event.preventDefault();
    moveFocus(next);
  }

  function shiftMonth(months: number) {
    const first = new Date(view.getFullYear(), view.getMonth() + months, 1);
    setView(first);
    setFocusDay(localDateString(first));
  }

  const cells = calendarMonthCells(view.getFullYear(), view.getMonth());
  const weeks: (string | null)[][] = [];
  for (let index = 0; index < cells.length; index += 7) weeks.push(cells.slice(index, index + 7));
  const today = localDateString(new Date());
  const title = `${view.getFullYear()}年${view.getMonth() + 1}月`;

  return <div ref={popRef} className="omp-date-pop" role="dialog" aria-label="选择日期"
    style={position ? { left: position.left, top: position.top } : { visibility: "hidden" }}>
    <div className="omp-cal-header">
      <button type="button" className="omp-cal-navbtn" aria-label="上一月" onClick={() => shiftMonth(-1)}><Glyph name="arrow-left" size={14} /></button>
      <div className="omp-cal-title" aria-hidden="true">{title}</div>
      <button type="button" className="omp-cal-navbtn" aria-label="下一月" onClick={() => shiftMonth(1)}><Glyph name="arrow" size={14} /></button>
    </div>
    <div ref={gridRef} role="grid" aria-label={`选择 ${title} 的日期`} className="omp-cal-grid" onKeyDown={onKeyDown}>
      <div role="row" className="omp-cal-week omp-cal-weekdays">
        {calendarWeekdays.map((weekday) => <span key={weekday} role="columnheader">{weekday}</span>)}
      </div>
      {weeks.map((week, weekIndex) => <div role="row" className="omp-cal-week" key={weekIndex}>
        {week.map((iso, dayIndex) => {
          if (iso === null) return <span role="gridcell" className="omp-cal-cell" key={dayIndex} />;
          const selected = iso === value;
          return <span role="gridcell" className="omp-cal-cell" key={dayIndex}>
            <button type="button" className={`omp-cal-day${iso === today ? " today" : ""}${selected ? " is-selected" : ""}`}
              data-date={iso} tabIndex={iso === focusDay ? 0 : -1} aria-selected={selected}
              onClick={() => onPick(iso)}>{Number(iso.slice(8))}</button>
          </span>;
        })}
      </div>)}
    </div>
  </div>;
}
