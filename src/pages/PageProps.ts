import type { Plan, State } from "../api";

export type Page = "overview" | "agents" | "omp" | "claude" | "codex" | "mcp" | "skills" | "repositories";

/// 页面与 App 壳之间的共享契约：页面负责自己的页头与告警条，
/// 全局状态（busy/error/notice/plan 预览）仍由壳统一持有。
/// state 在首载完成前为 null——页面渲染页头，内容区自行判空。
export type PageProps = {
  state: State | null;
  loading: boolean;
  busy: string;
  isBusy: boolean;
  error: string;
  notice: string;
  setError: (message: string) => void;
  setNotice: (message: string) => void;
  /// 带标签执行命令并弹出变更预览；返回是否成功生成计划。
  prepare: (label: string, work: () => Promise<Plan>) => Promise<boolean>;
  /// 带标签执行命令（busy/错误提示统一处理）。
  operation: <T>(label: string, work: () => Promise<T>) => Promise<T | undefined>;
  reload: () => Promise<void>;
  navigate: (page: Page) => void;
  openExternal: (url: string) => void;
};
