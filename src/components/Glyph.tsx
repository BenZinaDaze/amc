import type { ReactNode } from "react";

/// 全局图标集：统一 24×24 描线风格，按名称取用。
export function Glyph({ name, size = 20 }: { name: string; size?: number }) {
  const paths: Record<string, ReactNode> = {
    grid: <><rect x="3" y="3" width="7" height="7" rx="1.5" /><rect x="14" y="3" width="7" height="7" rx="1.5" /><rect x="3" y="14" width="7" height="7" rx="1.5" /><rect x="14" y="14" width="7" height="7" rx="1.5" /></>,
    agents: <><rect x="4" y="4" width="16" height="16" rx="4" /><path d="M9 10h.01M15 10h.01M9 15c1.7 1.5 4.3 1.5 6 0M12 1v3M8 1h8" /></>,
    claude: <path d="M12 2.5v19M2.5 12h19M5.3 5.3l13.4 13.4M18.7 5.3 5.3 18.7" />,
    codex: <><path d="M5 6.5 11 12l-6 5.5" /><path d="M13 17.5h6" /></>,
    plug: <><path d="M8 3v5m8-5v5M7 8h10v3a5 5 0 0 1-10 0V8Zm5 8v5m-4 0h8" /></>,
    spark: <><path d="m12 2 1.8 6.2L20 10l-6.2 1.8L12 18l-1.8-6.2L4 10l6.2-1.8L12 2ZM19 17l.7 1.3L21 19l-1.3.7L19 21l-.7-1.3L17 19l1.3-.7L19 17Z" /></>,
    repo: <><rect x="3" y="3" width="18" height="18" rx="3" /><path d="M8 3v18M12 8h5m-5 4h5" /></>,
    arrow: <path d="m9 18 6-6-6-6" />,
    "arrow-left": <path d="m15 18-6-6 6-6" />,
    plus: <path d="M12 5v14M5 12h14" />,
    refresh: <><path d="M20 7v5h-5M4 17v-5h5" /><path d="M5.5 9a7 7 0 0 1 12.6-2L20 12M4 12l1.9 5a7 7 0 0 0 12.6-2" /></>,
    search: <path d="m21 21-4.4-4.4m2.4-5.1a7.5 7.5 0 1 1-15 0 7.5 7.5 0 0 1 15 0Z" />,
    close: <path d="M5 5 19 19M19 5 5 19" />,
    check: <path d="m4 12 5 5L20 6" />,
    folder: <path d="M3 7a2 2 0 0 1 2-2h5l2 2h7a2 2 0 0 1 2 2v10H3V7Z" />,
    shield: <><path d="M12 2 4 6v5c0 5 3 8 8 11 5-3 8-6 8-11V6l-8-4Z" /><path d="m9 12 2 2 4-4" /></>,
    code: <path d="m8 7-5 5 5 5m8-10 5 5-5 5m-3-13-2 18" />,
    branch: <><circle cx="7" cy="5" r="2" /><circle cx="17" cy="7" r="2" /><circle cx="17" cy="18" r="2" /><path d="M7 7v8a3 3 0 0 0 3 3h5M7 10a3 3 0 0 0 3-3h5" /></>,
    warning: <><path d="M12 3 2 21h20L12 3Z" /><path d="M12 9v5m0 3h.01" /></>,
    edit: <path d="M17 3a2.85 2.85 0 1 1 4 4L7.5 20.5 2 22l1.5-5.5Z" />,
    trash: <><path d="M3 6h18" /><path d="M19 6v14a2 2 0 0 1-2 2H7a2 2 0 0 1-2-2V6" /><path d="M8 6V4a2 2 0 0 1 2-2h4a2 2 0 0 1 2 2v2" /></>,
    external: <><path d="M14 4h6v6M20 4l-9 9" /><path d="M18 13v5a2 2 0 0 1-2 2H6a2 2 0 0 1-2-2V8a2 2 0 0 1 2-2h5" /></>,
  };
  return <svg width={size} height={size} viewBox="0 0 24 24" fill="none" stroke="currentColor" strokeWidth="1.7" strokeLinecap="round" strokeLinejoin="round" aria-hidden="true">{paths[name]}</svg>;
}
