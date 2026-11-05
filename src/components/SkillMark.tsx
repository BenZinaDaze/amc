/// 技能确定性图标：后端只有 {name, description}，没有图标数据，
/// 这里按技能名哈希取色相，生成柔和渐变底 + 首字母 monogram。
/// 同名永远同色，技能在「已分发」和「发现技能」两个视图间视觉一致。
function hueFromName(name: string): number {
  let hash = 5381;
  for (let index = 0; index < name.length; index++) hash = ((hash << 5) + hash + name.charCodeAt(index)) >>> 0;
  return hash % 360;
}

export function SkillMark({ name }: { name: string }) {
  const hue = hueFromName(name);
  const letter = (name.trim()[0] || "S").toUpperCase();
  return (
    <span
      className="skill-mark"
      aria-hidden="true"
      style={{
        background: `linear-gradient(135deg, hsl(${hue} 78% 94%), hsl(${hue} 66% 86%))`,
        color: `hsl(${hue} 52% 38%)`,
      }}
    >
      {letter}
    </span>
  );
}
