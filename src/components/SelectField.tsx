import { useEffect, useId, useRef, useState, type KeyboardEvent as ReactKeyboardEvent } from "react";
import { createPortal } from "react-dom";
import { Glyph } from "./Glyph";

/// 自定义下拉：替代原生 <select>，弹出层不再使用系统菜单样式。
/// 触发器沿用输入框视觉；菜单固定定位于触发器下方（视口放不下时
/// 上翻）。键盘交互沿用 listbox 惯例：方向键/Home/End 移动候选项
/// （焦点保留在触发器上，经 aria-activedescendant 暴露），Enter/
/// Space 确认，Escape 或点击外部关闭。
export function SelectField({ value, options, onChange, label, disabled }: {
  value: string;
  options: { value: string; label: string }[];
  onChange: (value: string) => void;
  label: string;
  disabled?: boolean;
}) {
  const [open, setOpen] = useState(false);
  const [active, setActive] = useState(() => Math.max(0, options.findIndex((option) => option.value === value)));
  const [position, setPosition] = useState<{ left: number; top: number; width: number } | null>(null);
  const triggerRef = useRef<HTMLButtonElement>(null);
  const popRef = useRef<HTMLDivElement>(null);
  const listId = useId();

  // 打开时对齐触发器；跟随滚动/缩放重新定位（与 DateCalendar 一致）。
  useEffect(() => {
    if (!open) return;
    function place() {
      const field = triggerRef.current;
      if (!field) return;
      const rect = field.getBoundingClientRect();
      const height = popRef.current?.offsetHeight ?? 160;
      const below = rect.bottom + 6 + height <= window.innerHeight;
      setPosition({
        left: Math.round(Math.min(Math.max(8, rect.left), window.innerWidth - rect.width - 8)),
        top: Math.round(below ? rect.bottom + 6 : Math.max(8, rect.top - height - 6)),
        width: Math.round(rect.width),
      });
    }
    place();
    window.addEventListener("resize", place);
    document.addEventListener("scroll", place, true);
    return () => {
      window.removeEventListener("resize", place);
      document.removeEventListener("scroll", place, true);
    };
  }, [open]);

  useEffect(() => {
    if (!open) return;
    function onKeyDown(event: KeyboardEvent) {
      if (event.key !== "Escape") return;
      event.preventDefault();
      event.stopPropagation();
      setOpen(false);
      // 仅当焦点仍在触发器上时回焦，避免从其他控件抢焦点。
      if (document.activeElement === triggerRef.current) triggerRef.current?.focus();
    }
    function onPointerDown(event: PointerEvent) {
      const target = event.target instanceof Node ? event.target : null;
      if (target && !popRef.current?.contains(target) && !triggerRef.current?.contains(target)) setOpen(false);
    }
    document.addEventListener("keydown", onKeyDown, true);
    document.addEventListener("pointerdown", onPointerDown, true);
    return () => {
      document.removeEventListener("keydown", onKeyDown, true);
      document.removeEventListener("pointerdown", onPointerDown, true);
    };
  }, [open]);

  // 外部值变化或重新打开时，让候选高亮回到当前选中项。
  useEffect(() => {
    const index = options.findIndex((option) => option.value === value);
    if (index >= 0) setActive(index);
  }, [value, open, options]);

  function choose(index: number) {
    const option = options[index];
    if (!option) return;
    onChange(option.value);
    setOpen(false);
    triggerRef.current?.focus();
  }

  function onKeyDown(event: ReactKeyboardEvent<HTMLButtonElement>) {
    const last = options.length - 1;
    const delta = event.key === "ArrowDown" ? 1 : event.key === "ArrowUp" ? -1 : 0;
    if (delta) {
      event.preventDefault();
      if (open) setActive((current) => (current + delta + options.length) % options.length);
      else { setOpen(true); setActive((current) => Math.max(0, Math.min(last, current + delta))); }
      return;
    }
    if (event.key === "Tab") { setOpen(false); return; } // Tab 离开：收起菜单，放行默认焦点移动
    if (!open) return; // Enter/Space 走原生 click 展开
    if (event.key === "Home") { event.preventDefault(); setActive(0); }
    else if (event.key === "End") { event.preventDefault(); setActive(last); }
    else if (event.key === "Enter" || event.key === " ") { event.preventDefault(); choose(active); }
  }

  const selectedLabel = options.find((option) => option.value === value)?.label ?? "";
  return <>
    <button ref={triggerRef} type="button" role="combobox" className={`select-field${open ? " open" : ""}`} disabled={disabled}
      aria-haspopup="listbox" aria-expanded={open} aria-label={label}
      aria-controls={open ? listId : undefined}
      // aria-activedescendant 仅在 combobox 等角色上有效（button 不支持），
      // 焦点留在触发器时屏幕阅读器靠它读出当前候选。
      aria-activedescendant={open ? `${listId}-${active}` : undefined}
      onClick={() => setOpen((current) => !current)} onKeyDown={onKeyDown}>
      <span className="select-field-value">{selectedLabel}</span>
      <span className="select-field-chevron" aria-hidden="true"><Glyph name="arrow" size={14} /></span>
    </button>
    {open && createPortal(<div ref={popRef} id={listId} role="listbox" aria-label={label} className="select-menu"
      style={position ? { left: position.left, top: position.top, minWidth: position.width } : { visibility: "hidden" }}>
      {options.map((option, index) => <div key={option.value} id={`${listId}-${index}`} role="option"
        aria-selected={option.value === value}
        className={`select-option${index === active ? " active" : ""}${option.value === value ? " selected" : ""}`}
        onMouseEnter={() => setActive(index)} onClick={() => choose(index)}>
        <span>{option.label}</span>
        {option.value === value && <Glyph name="check" size={14} />}
      </div>)}
    </div>, document.body)}
  </>;
}
