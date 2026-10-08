import { useCallback, useEffect, useRef, useState } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import ompIcon from "./assets/omp.svg";
import { version } from "../package.json";
import { api, type ClaudeCodeStatus, type CodexStatus, type Plan, type State, type Workspace } from "./api";
import "./App.css";
import { Glyph } from "./components/Glyph";
import { Empty } from "./components/Empty";
import { useSubscriptions } from "./apps/subscriptions/useSubscriptions";
import { useRepositorySkills } from "./apps/repositories/useRepositorySkills";
import { AgentUsagePage } from "./pages/AgentUsagePage";
import { AgentsPage } from "./pages/AgentsPage";
import { McpPage } from "./pages/McpPage";
import { OverviewPage } from "./pages/OverviewPage";
import { RepositoriesPage } from "./pages/RepositoriesPage";
import { SkillsPage } from "./pages/SkillsPage";
import type { Page, PageProps } from "./pages/PageProps";
import { errorText } from "./utils";

const navigation: { id: Page; title: string; icon: string; group?: boolean; child?: boolean }[] = [
  { id: "overview", title: "概览", icon: "grid" },
  { id: "agents", title: "Agents", icon: "agents", group: true },
  { id: "omp", title: "OMP", icon: "omp", child: true },
  { id: "claude", title: "Claude Code", icon: "claude", child: true },
  { id: "codex", title: "Codex CLI", icon: "codex", child: true },
  { id: "mcp", title: "MCP", icon: "plug", group: true },
  { id: "skills", title: "Skills", icon: "spark", group: true },
  { id: "repositories", title: "仓库", icon: "repo", group: true },
];

/// 应用壳：侧边栏导航 + 路由 + 全局状态（workspace/state、busy、
/// error/notice、变更预览）。页面与业务模块见 pages/ 与 apps/。
function App() {
  const [page, setPage] = useState<Page>("overview");
  const [agentsNavOpen, setAgentsNavOpen] = useState(true);
  const [workspace, setWorkspace] = useState<Workspace | null>(null);
  const [state, setState] = useState<State | null>(null);
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState("");
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");
  const [plan, setPlan] = useState<Plan | null>(null);
  const [claudeCodeStatus, setClaudeCodeStatus] = useState<ClaudeCodeStatus | null>(null);
  const [codexStatus, setCodexStatus] = useState<CodexStatus | null>(null);
  const loadId = useRef(0);

  const subs = useSubscriptions();
  const repoSkills = useRepositorySkills(setError);

  const reload = useCallback(async (selected: Workspace): Promise<State | null> => {
    const request = ++loadId.current;
    setLoading(true);
    setError("");
    try {
      const next = await api.getState(selected);
      if (request === loadId.current) {
        setState(next);
        return next;
      }
    } catch (reason) {
      if (request === loadId.current) {
        setState(null);
        setError(`无法加载状态：${errorText(reason)}`);
      }
    } finally {
      if (request === loadId.current) setLoading(false);
    }
    return null;
  }, []);

  const reloadAndLoad = useCallback(async (selected: Workspace): Promise<State | null> => {
    const next = await reload(selected);
    if (next) await repoSkills.load(next.repositories);
    return next;
  }, [reload, repoSkills.load]);

  useEffect(() => {
    let cancelled = false;
    api.getDefaultWorkspace().then(async (next) => {
      if (cancelled) return;
      setWorkspace(next);
      await reloadAndLoad(next);
    }).catch((reason) => {
      if (!cancelled) {
        setState(null);
        setLoading(false);
        setError(`无法加载状态：${errorText(reason)}`);
      }
    });
    return () => { cancelled = true; };
  }, [reloadAndLoad]);

  useEffect(() => {
    if (page === "agents" || page === "claude" || page === "codex") {
      api.getClaudeCodeStatus().then(setClaudeCodeStatus).catch(() => setClaudeCodeStatus({ installed: false, version: "" }));
      api.getCodexStatus().then(setCodexStatus).catch(() => setCodexStatus({ installed: false, version: "" }));
    }
  }, [page]);

  // 屏蔽 WebView 默认右键菜单：与应用无关（刷新/检查等）。输入框、
  // 多行文本与可编辑区域保留系统菜单，粘贴仍可用。
  useEffect(() => {
    const suppress = (event: MouseEvent) => {
      const target = event.target as HTMLElement | null;
      if (target?.closest("input, textarea, [contenteditable]")) return;
      event.preventDefault();
    };
    document.addEventListener("contextmenu", suppress);
    return () => document.removeEventListener("contextmenu", suppress);
  }, []);

  useEffect(() => {
    if (!plan) return;
    const onKey = (event: KeyboardEvent) => { if (event.key === "Escape" && !busy) setPlan(null); };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [plan, busy]);

  async function operation<T>(label: string, work: () => Promise<T>): Promise<T | undefined> {
    setBusy(label); setError(""); setNotice("");
    try { return await work(); }
    catch (reason) { setError(`${label}失败：${errorText(reason)}`); }
    finally { setBusy(""); }
    return undefined;
  }

  async function prepare(label: string, work: () => Promise<Plan>): Promise<boolean> {
    const result = await operation(label, work);
    if (result) setPlan(result);
    return Boolean(result);
  }

  function openExternal(url: string) {
    setError("");
    void openUrl(url).catch((reason) => setError(`无法打开仓库链接：${errorText(reason)}`));
  }

  async function apply() {
    if (!plan) return;
    const result = await operation("应用变更", () => api.applyPlan(plan.id));
    if (result) {
      setPlan(null);
      setNotice(result.message || "变更已应用");
      if (selectedWorkspace) await reloadAndLoad(selectedWorkspace);
    }
  }

  const selectedWorkspace = workspace ?? state?.workspace;
  const isBusy = Boolean(busy);
  const shared: PageProps = {
    state,
    loading,
    busy,
    isBusy,
    error,
    notice,
    setError,
    setNotice,
    prepare,
    operation,
    reload: async () => { if (selectedWorkspace) await reloadAndLoad(selectedWorkspace); },
    navigate: setPage,
    openExternal,
  };
  const isUsagePage = page === "omp" || page === "claude" || page === "codex";

  return (
    <div className="app-shell">
      <aside className="sidebar">
        <div aria-hidden="true" className="titlebar-drag" data-tauri-drag-region />
        <div className="brand"><span className="brand-mark"><img src="/amc.svg" alt="" /></span><div><strong>AMC</strong><small>Agent Management Center</small></div></div>
        <div className="sidebar-caption">通用来源</div>
        <nav aria-label="主导航" className="navigation">
          {navigation.map((item) => <div key={item.id} className={`nav-section${item.group ? " group" : ""}${item.child ? " nav-child" : ""}${item.child && !agentsNavOpen ? " collapsed" : ""}`}>
            <button className={`nav-item ${page === item.id ? "selected" : ""}`} onClick={() => setPage(item.id)} aria-current={page === item.id ? "page" : undefined}>{item.icon === "omp" ? <img className="omp-nav-icon" src={ompIcon} alt="" /> : <Glyph name={item.icon} size={19} />}<span>{item.title}</span></button>
            {item.id === "agents" && <button type="button" className="nav-group-toggle" aria-expanded={agentsNavOpen} aria-label={agentsNavOpen ? "折叠 Agents 分组" : "展开 Agents 分组"} title={agentsNavOpen ? "折叠 Agents 分组" : "展开 Agents 分组"} onClick={() => setAgentsNavOpen((open) => !open)}><Glyph name="arrow" size={15} /></button>}
          </div>)}
        </nav>
        <div className="sidebar-bottom"><div className="sidebar-orbit"><Glyph name="shield" size={15} /><span>写入前预览确认</span></div><div className="sidebar-version"><span>版本</span><strong>{version}</strong></div></div>
      </aside>
      <main className="main-area">
        <div className="content">
          {page === "overview" && <OverviewPage {...shared} subscriptions={subs.subscriptions} subscriptionsLoading={subs.loading} subscriptionsError={subs.error} loadSubscriptions={subs.load} removeSubscription={subs.remove} />}
          {page === "agents" && <AgentsPage {...shared} claudeCodeStatus={claudeCodeStatus} codexStatus={codexStatus} />}
          {page === "omp" && <AgentUsagePage agentId="omp" name="OMP" title="OMP" refreshName="OMP" error={error} notice={notice} setError={setError} setNotice={setNotice} pill={state && <span className={`omp-version-pill ${state.agent.installed && state.agent.version ? "available" : "missing"}`}>{state.agent.installed && state.agent.version ? state.agent.version.replace(/^omp\//i, "") : "未检测到"}</span>} />}
          {page === "claude" && <AgentUsagePage agentId="claude-code" name="Claude Code" title="Claude Code" refreshName="Claude Code" error={error} notice={notice} setError={setError} setNotice={setNotice} pill={claudeCodeStatus && <span className={`omp-version-pill ${claudeCodeStatus.installed && claudeCodeStatus.version ? "available" : "missing"}`}>{claudeCodeStatus.installed && claudeCodeStatus.version ? claudeCodeStatus.version.split(/\s+/)[0] : "未检测到"}</span>} />}
          {page === "codex" && <AgentUsagePage agentId="codex" name="Codex" title="Codex CLI" refreshName="Codex" error={error} notice={notice} setError={setError} setNotice={setNotice} pill={codexStatus && <span className={`omp-version-pill ${codexStatus.installed && codexStatus.version ? "available" : "missing"}`}>{codexStatus.installed && codexStatus.version ? codexStatus.version.split(/\s+/).pop() : "未检测到"}</span>} />}
          {page === "mcp" && <McpPage {...shared} />}
          {page === "skills" && <SkillsPage {...shared} repositorySkills={repoSkills.skills} repositorySkillErrors={repoSkills.errors} repositoryScanLoading={repoSkills.scanLoading} />}
          {page === "repositories" && <RepositoriesPage {...shared} repositorySkills={repoSkills.skills} repositoryScanLoading={repoSkills.scanLoading} forget={repoSkills.forget} />}
          {!isUsagePage && loading && <div className="loading-panel" role="status"><span className="spinner" />正在加载状态…</div>}
          {!isUsagePage && !loading && !state && <Empty icon="warning" title="尚无法加载状态" description="AMC 无法读取本机数据目录或 Agent 配置，请刷新重试。" action="重新加载" onClick={() => { if (selectedWorkspace) void reloadAndLoad(selectedWorkspace); }} />}
        </div>
      </main>
      {plan && <div className="modal-backdrop" onMouseDown={(event) => { if (event.target === event.currentTarget && !isBusy) setPlan(null); }}>
        <div className="modal plan-modal" role="dialog" aria-modal="true" aria-labelledby="plan-title">
          <div className="modal-head">
            <div><div className="eyebrow">变更预览 / 请确认</div><h2 id="plan-title">{plan.summary}</h2></div>
            <button className="icon-button" aria-label="关闭" disabled={isBusy} onClick={() => setPlan(null)}><Glyph name="close" size={20} /></button>
          </div>
          <div className="modal-body">
            {error && <div className="alert alert-error" role="alert"><Glyph name="warning" size={18} /><span>{error}</span><button aria-label="关闭错误提示" onClick={() => setError("")}><Glyph name="close" size={16} /></button></div>}
            <p className="plan-description">请核对以下文件变更。确认前不会写入目标；应用失败时可取消并重新预览。</p>
            {plan.warnings.length > 0 && <div className="plan-warnings">
              {plan.warnings.map((warning, index) => <p key={index}><Glyph name="warning" size={16} />{warning}</p>)}
            </div>}
            {plan.changes.length ? <div className="diff-list">
              {plan.changes.map((change, index) => <div className="diff-card" key={`${change.path}:${index}`}>
                <div className="diff-path"><Glyph name="code" size={16} />{change.path}</div>
                <div className="diff-columns">
                  <div><span className="diff-label before-label">变更前</span><pre>{change.before || "（无）"}</pre></div>
                  <div><span className="diff-label after-label">变更后</span><pre>{change.after || "（删除）"}</pre></div>
                </div>
              </div>)}
            </div> : <div className="empty-diff">该计划没有文件差异；请确认摘要与警告。</div>}
            <div className="modal-actions">
              <button className="button button-muted" disabled={isBusy} onClick={() => setPlan(null)}>取消</button>
              <button className="button button-primary" disabled={isBusy} onClick={() => void apply()}>{isBusy ? "正在应用…" : "确认并应用"}</button>
            </div>
          </div>
        </div>
      </div>}
    </div>
  );
}

export default App;
