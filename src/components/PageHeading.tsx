import type { ReactNode } from "react";

/// 各页面统一的页头：标题（可带版本徽章）+ 描述 + 右侧动作区（通常是刷新按钮）。
export function PageHeading({ title, description, pill, actions }: { title: string; description?: string; pill?: ReactNode; actions?: ReactNode }) {
  return <div className="page-heading"><div><h1>{title}{pill}</h1>{description && <p>{description}</p>}</div>{actions}</div>;
}
