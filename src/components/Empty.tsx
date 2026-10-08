import { Glyph } from "./Glyph";

/// 通用空状态卡片：图标 + 标题 + 描述，可选主按钮动作。
export function Empty({ icon, title, description, action, onClick }: { icon: string; title: string; description: string; action?: string; onClick?: () => void }) {
  return <div className="empty-state"><span className="empty-icon"><Glyph name={icon} size={27} /></span><h3>{title}</h3><p>{description}</p>{action && onClick && <button className="button button-primary" onClick={onClick}>{action}<Glyph name="arrow" size={15} /></button>}</div>;
}
