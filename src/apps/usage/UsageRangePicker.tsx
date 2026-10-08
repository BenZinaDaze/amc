import { useEffect, useRef, useState } from "react";
import { DateCalendar } from "../../components/DateCalendar";
import { localMidnight } from "../../utils";
import type { UsageRange } from "../../api";
import { calendarRange, dateFieldsForChoice, extraUsageRanges, usageRanges, type UsageChoice, type UsagePreset } from "./format";

export function UsageRangePicker({ range, label, activeChoice, onApply }: {
  range: UsageRange;
  label: string;
  activeChoice: UsageChoice;
  onApply: (range: UsageRange, label: string, choice: UsageChoice) => void;
}) {
  const [open, setOpen] = useState(false);
  const [draftChoice, setDraftChoice] = useState<UsageChoice>(activeChoice);
  const [dates, setDates] = useState(() => dateFieldsForChoice(activeChoice, range));
  const [editingDate, setEditingDate] = useState<"start" | "end" | null>(null);
  const container = useRef<HTMLDivElement>(null);
  const trigger = useRef<HTMLButtonElement>(null);
  const popup = useRef<HTMLDivElement>(null);
  const startField = useRef<HTMLButtonElement>(null);
  const endField = useRef<HTMLButtonElement>(null);

  useEffect(() => {
    if (!open) return;
    popup.current?.querySelector<HTMLButtonElement>('button[aria-pressed="true"], .omp-cal-day[tabindex="0"]')?.focus();
    function onPointerDown(event: PointerEvent) {
      if (event.target instanceof Node && !container.current?.contains(event.target)) setOpen(false);
    }
    function onKeyDown(event: KeyboardEvent) {
      if (event.key === "Escape") {
        event.preventDefault();
        setOpen(false);
        trigger.current?.focus();
      }
    }
    document.addEventListener("pointerdown", onPointerDown);
    document.addEventListener("keydown", onKeyDown);
    return () => {
      document.removeEventListener("pointerdown", onPointerDown);
      document.removeEventListener("keydown", onKeyDown);
    };
  }, [open]);

  function choosePreset(choice: UsagePreset) {
    setDraftChoice(choice);
    setDates(dateFieldsForChoice(choice, range));
  }

  const start = localMidnight(dates.start);
  const end = localMidnight(dates.end);
  const dateError = !start || !end ? "请选择有效的开始日期和结束日期。" : start.getTime() > end.getTime() ? "开始日期不能晚于结束日期。" : "";

  function apply() {
    let selectedRange: UsageRange;
    let selectedLabel: string;
    if (draftChoice === "custom") {
      if (dateError || !start || !end) return;
      const endExclusive = new Date(end);
      endExclusive.setDate(endExclusive.getDate() + 1);
      selectedRange = `custom:${start.getTime()}:${endExclusive.getTime()}`;
      selectedLabel = `${dates.start.replace(/-/g, "/")} → ${dates.end.replace(/-/g, "/")}`;
    } else {
      selectedRange = calendarRange(draftChoice) ?? draftChoice as UsageRange;
      selectedLabel = usageRanges.find(({ value }) => value === draftChoice)?.label
        ?? extraUsageRanges.find(({ value }) => value === draftChoice)!.label;
    }
    setOpen(false);
    trigger.current?.focus();
    onApply(selectedRange, selectedLabel, draftChoice);
  }

  return <div className="omp-range-picker" ref={container}>
    <button ref={trigger} type="button" className="omp-range-trigger" aria-label={`用量时间范围：${label}`} aria-haspopup="dialog" aria-expanded={open} aria-controls={open ? "omp-range-dialog" : undefined} onClick={() => {
      if (!open) {
        setDraftChoice(activeChoice);
        setDates(dateFieldsForChoice(activeChoice, range));
        setEditingDate(null);
      }
      setOpen(!open);
    }}>
      <span>{label}</span><span className="omp-range-chevron" aria-hidden="true" />
    </button>
    {open && <div ref={popup} id="omp-range-dialog" className="omp-range-popup" role="dialog" aria-label="选择用量时间范围">
      <div className="omp-range-presets" role="group" aria-label="常用时间范围">
        {usageRanges.map(({ value, label: optionLabel }) => <button key={value} type="button" aria-pressed={draftChoice === value} className={draftChoice === value ? "active" : ""} onClick={() => choosePreset(value)}>{optionLabel}</button>)}
      </div>
      <div className="omp-range-extras" role="group" aria-label="更多时间范围">
        {extraUsageRanges.map(({ value, label: optionLabel }) => <button key={value} type="button" aria-pressed={draftChoice === value} className={draftChoice === value ? "active" : ""} onClick={() => choosePreset(value)}>{optionLabel}</button>)}
      </div>
      <div className="omp-range-custom">
        <div className="omp-range-dates">
          <button ref={startField} type="button" className="omp-date-field" aria-haspopup="dialog" aria-expanded={editingDate === "start"} aria-invalid={draftChoice === "custom" && !!dateError} onClick={() => setEditingDate((current) => current === "start" ? null : "start")}>
            <small>开始日期</small><strong className={dates.start ? undefined : "unset"}>{dates.start ? dates.start.replace(/-/g, "/") : "—"}</strong>
          </button>
          <span className="omp-range-arrow" aria-hidden="true">→</span>
          <button ref={endField} type="button" className="omp-date-field" aria-haspopup="dialog" aria-expanded={editingDate === "end"} aria-invalid={draftChoice === "custom" && !!dateError} onClick={() => setEditingDate((current) => current === "end" ? null : "end")}>
            <small>结束日期</small><strong className={dates.end ? undefined : "unset"}>{dates.end ? dates.end.replace(/-/g, "/") : "—"}</strong>
          </button>
        </div>
        {editingDate !== null && <DateCalendar
          field={(editingDate === "start" ? startField : endField).current!}
          value={editingDate === "start" ? dates.start : dates.end}
          onPick={(iso) => {
            setDates(editingDate === "start" ? { ...dates, start: iso } : { ...dates, end: iso });
            setDraftChoice("custom");
            setEditingDate(null);
            (editingDate === "start" ? startField : endField).current?.focus();
          }}
          onClose={() => setEditingDate(null)} />}
        {draftChoice === "custom" && dateError && <p className="omp-range-error" role="alert">{dateError}</p>}
        <div className="omp-range-actions"><button type="button" className="button button-primary" disabled={draftChoice === "custom" && !!dateError} onClick={apply}>应用</button></div>
      </div>
    </div>}
  </div>;
}
