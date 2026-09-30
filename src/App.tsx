import { useCallback, useEffect, useRef, useState, type FormEvent, type ReactNode } from "react";
import { openUrl } from "@tauri-apps/plugin-opener";
import ompIcon from "./assets/omp.svg";
import zaiIcon from "./assets/zai.svg";
import { version } from "../package.json";
import { api, type ClaudeCodeStatus, type McpServer, type Plan, type Repository, type RepositorySkill, type State, type SubscriptionKind, type SubscriptionStatus, type UsageRange, type UsageStats, type Workspace } from "./api";
import "./App.css";

type Page = "overview" | "agents" | "omp" | "claude" | "mcp" | "skills" | "repositories";
type McpMode = "stdio" | "http" | "sse";
type SkillTab = "installed" | "discover";
type DiscoveredSkill = RepositorySkill & { repositoryId: number };
type UsagePreset = "today" | "yesterday" | "24h" | "7d" | "14d" | "30d" | "thisMonth" | "lastMonth" | "1h" | "90d" | "all";
type UsageChoice = UsagePreset | "custom";
const usageRanges: { value: UsagePreset; label: string }[] = [
  { value: "today", label: "今天" },
  { value: "yesterday", label: "昨天" },
  { value: "24h", label: "近24小时" },
  { value: "7d", label: "近7天" },
  { value: "14d", label: "近14天" },
  { value: "30d", label: "近30天" },
  { value: "thisMonth", label: "本月" },
  { value: "lastMonth", label: "上月" },
];
const extraUsageRanges: { value: UsagePreset; label: string }[] = [
  { value: "1h", label: "近1小时" },
  { value: "90d", label: "近90天" },
  { value: "all", label: "全部" },
];

function localDateString(date: Date): string {
  return `${date.getFullYear().toString().padStart(4, "0")}-${String(date.getMonth() + 1).padStart(2, "0")}-${String(date.getDate()).padStart(2, "0")}`;
}

function localMidnight(value: string): Date | null {
  const parts = /^(\d{4})-(\d{2})-(\d{2})$/.exec(value);
  if (!parts) return null;
  const [, year, month, day] = parts.map(Number);
  const date = new Date(0);
  date.setFullYear(year, month - 1, day);
  date.setHours(0, 0, 0, 0);
  return date.getFullYear() === year && date.getMonth() === month - 1 && date.getDate() === day ? date : null;
}

function calendarRange(choice: UsagePreset): UsageRange | null {
  const now = new Date();
  const today = new Date(now.getFullYear(), now.getMonth(), now.getDate());
  const tomorrow = new Date(now.getFullYear(), now.getMonth(), now.getDate() + 1);
  const monthStart = new Date(now.getFullYear(), now.getMonth(), 1);
  let start: Date;
  let end: Date;
  switch (choice) {
    case "today": [start, end] = [today, tomorrow]; break;
    case "yesterday": [start, end] = [new Date(now.getFullYear(), now.getMonth(), now.getDate() - 1), today]; break;
    case "thisMonth": [start, end] = [monthStart, new Date(now.getFullYear(), now.getMonth() + 1, 1)]; break;
    case "lastMonth": [start, end] = [new Date(now.getFullYear(), now.getMonth() - 1, 1), monthStart]; break;
    default: return null;
  }
  return `custom:${start.getTime()}:${end.getTime()}`;
}

function dateFieldsForChoice(choice: UsageChoice, range: UsageRange): { start: string; end: string } {
  if (choice === "custom" && range.startsWith("custom:")) {
    const [, start, end] = range.split(":").map(Number);
    const lastDay = new Date(end);
    lastDay.setDate(lastDay.getDate() - 1);
    return { start: localDateString(new Date(start)), end: localDateString(lastDay) };
  }
  if (choice !== "custom") {
    const calendar = calendarRange(choice);
    if (calendar) return dateFieldsForChoice("custom", calendar);
  }
  if (choice === "all") return { start: "", end: localDateString(new Date()) };
  const now = new Date();
  const start = new Date(now);
  const rollingDays: Partial<Record<UsagePreset, number>> = { "24h": 1, "7d": 7, "14d": 14, "30d": 30, "90d": 90 };
  start.setDate(start.getDate() - (rollingDays[choice as UsagePreset] ?? 0));
  if (choice === "1h") start.setHours(start.getHours() - 1);
  return { start: localDateString(start), end: localDateString(now) };
}



function errorText(error: unknown): string {
  return error instanceof Error ? error.message : String(error);
}

function repositoryWebUrl(repository?: Repository): string | null {
  if (!repository || repository.localPath) return null;
  const value = repository.url.trim();
  const location = /^git@github\.com:/i.test(value)
    ? value.slice("git@github.com:".length)
    : /^ssh:\/\/git@github\.com\//i.test(value)
      ? value.slice("ssh://git@github.com/".length)
      : (() => {
          try {
            const url = new URL(value);
            return url.protocol === "https:" && url.hostname === "github.com" && !url.port && !url.username && !url.password && !url.search && !url.hash
              ? url.pathname.replace(/^\//, "")
              : null;
          } catch { return null; }
        })();
  if (!location) return null;
  const parts = location.replace(/\/$/, "").replace(/\.git$/i, "").split("/");
  if (parts.length !== 2 || parts.some((part) => !/^[a-z0-9_.-]+$/i.test(part) || part === "." || part === "..")) return null;
  return `https://github.com/${parts[0]}/${parts[1]}`;
}

function skillWebUrl(repository: Repository | undefined, path: string): string | null {
  const base = repositoryWebUrl(repository);
  if (!base) return null;
  const parts = path === "." ? [] : path.split("/");
  if (parts.some((part) => !part || part === "." || part === ".." || part.includes("\\"))) return null;
  const reference = (repository?.reference || "HEAD").split("/").map(encodeURIComponent).join("/");
  return `${base}/tree/${reference}${parts.length ? `/${parts.map(encodeURIComponent).join("/")}` : ""}`;
}

function Glyph({ name, size = 20 }: { name: string; size?: number }) {
  const paths: Record<string, ReactNode> = {
    grid: <><rect x="3" y="3" width="7" height="7" rx="1.5" /><rect x="14" y="3" width="7" height="7" rx="1.5" /><rect x="3" y="14" width="7" height="7" rx="1.5" /><rect x="14" y="14" width="7" height="7" rx="1.5" /></>,
    agents: <><rect x="4" y="4" width="16" height="16" rx="4" /><path d="M9 10h.01M15 10h.01M9 15c1.7 1.5 4.3 1.5 6 0M12 1v3M8 1h8" /></>,
    claude: <path d="M12 2.5v19M2.5 12h19M5.3 5.3l13.4 13.4M18.7 5.3 5.3 18.7" />,
    plug: <><path d="M8 3v5m8-5v5M7 8h10v3a5 5 0 0 1-10 0V8Zm5 8v5m-4 0h8" /></>,
    spark: <><path d="m12 2 1.8 6.2L20 10l-6.2 1.8L12 18l-1.8-6.2L4 10l6.2-1.8L12 2ZM19 17l.7 1.3L21 19l-1.3.7L19 21l-.7-1.3L17 19l1.3-.7L19 17Z" /></>,
    repo: <><rect x="3" y="3" width="18" height="18" rx="3" /><path d="M8 3v18M12 8h5m-5 4h5" /></>,
    arrow: <path d="m9 18 6-6-6-6" />,
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

const navigation: { id: Page; title: string; icon: string; group?: boolean; child?: boolean }[] = [
  { id: "overview", title: "概览", icon: "grid" },
  { id: "agents", title: "Agents", icon: "agents", group: true },
  { id: "omp", title: "OMP", icon: "omp", child: true },
  { id: "claude", title: "Claude Code", icon: "claude", child: true },
  { id: "mcp", title: "MCP", icon: "plug", group: true },
  { id: "skills", title: "Skills", icon: "spark", group: true },
  { id: "repositories", title: "仓库", icon: "repo", group: true },
];

function App() {
  const [page, setPage] = useState<Page>("overview");
  const [workspace, setWorkspace] = useState<Workspace | null>(null);
  const [state, setState] = useState<State | null>(null);
  const [loading, setLoading] = useState(true);
  const [busy, setBusy] = useState("");
  const [error, setError] = useState("");
  const [notice, setNotice] = useState("");
  const [plan, setPlan] = useState<Plan | null>(null);
  const [mcpOpen, setMcpOpen] = useState(false);
  const [mcpOriginal, setMcpOriginal] = useState("");
  const [mcpName, setMcpName] = useState("");
  const [mcpMode, setMcpMode] = useState<McpMode>("stdio");
  const [mcpCommand, setMcpCommand] = useState("");
  const [mcpEnabled, setMcpEnabled] = useState(true);
  const [mcpArgs, setMcpArgs] = useState("");
  const [mcpUrl, setMcpUrl] = useState("");
  const [mcpExtra, setMcpExtra] = useState("{}");
  const [rawConfig, setRawConfig] = useState(false);
  const [mcpRaw, setMcpRaw] = useState("{}");
  const [repoUrl, setRepoUrl] = useState("");
  const [repoRef, setRepoRef] = useState("");
  const [repositorySkills, setRepositorySkills] = useState<Record<number, RepositorySkill[]>>({});
  const [repositorySkillErrors, setRepositorySkillErrors] = useState<Record<number, string>>({});
  const [repositoryRemoval, setRepositoryRemoval] = useState<Repository | null>(null);
  const [repositoryScanLoading, setRepositoryScanLoading] = useState(false);
  const [skillTab, setSkillTab] = useState<SkillTab>("installed");
  const [discoverSearchOpen, setDiscoverSearchOpen] = useState(false);
  const [discoverSearch, setDiscoverSearch] = useState("");
  const [usageSelection, setUsageSelection] = useState<{ range: UsageRange; label: string; choice: UsageChoice }>({ range: "24h", label: "近24小时", choice: "24h" });
  const [usage, setUsage] = useState<UsageStats | null>(null);
  const [usageLoading, setUsageLoading] = useState<"sync" | "read" | null>(null);
  const [usageError, setUsageError] = useState("");
  const usageLoadId = useRef(0);
  const usageRangeRef = useRef<UsageRange>("24h");
  const [agentsUsage, setAgentsUsage] = useState<UsageStats | null>(null);
  const [agentsUsageLoading, setAgentsUsageLoading] = useState<"sync" | "read" | null>(null);
  const [agentsUsageError, setAgentsUsageError] = useState("");
  const [agentsPricingNote, setAgentsPricingNote] = useState("");
  const [agentsUsageSelection, setAgentsUsageSelection] = useState<{ range: UsageRange; label: string; choice: UsageChoice }>({ range: "24h", label: "近24小时", choice: "24h" });
  const agentsUsageLoadId = useRef(0);
  const agentsUsageRangeRef = useRef<UsageRange>("24h");
  const [claudeUsage, setClaudeUsage] = useState<UsageStats | null>(null);
  const [claudeUsageLoading, setClaudeUsageLoading] = useState<"sync" | "read" | null>(null);
  const [claudeUsageError, setClaudeUsageError] = useState("");
  const claudeUsageLoadId = useRef(0);
  const claudeUsageRangeRef = useRef<UsageRange>("24h");
  const [claudeUsageSelection, setClaudeUsageSelection] = useState<{ range: UsageRange; label: string; choice: UsageChoice }>({ range: "24h", label: "近24小时", choice: "24h" });
  const pendingClaudeUsageSync = useRef<Promise<UsageStats> | null>(null);
  const [claudeCodeStatus, setClaudeCodeStatus] = useState<ClaudeCodeStatus | null>(null);
  const [subscriptions, setSubscriptions] = useState<SubscriptionStatus[] | null>(null);
  const [subscriptionsLoading, setSubscriptionsLoading] = useState(false);
  const [subscriptionsError, setSubscriptionsError] = useState("");
  const subscriptionsLoadId = useRef(0);
  const subscriptionsFetchedAt = useRef(0);
  const pendingUsageSync = useRef<Promise<UsageStats> | null>(null);
  const [planForm, setPlanForm] = useState<{ mode: "add" } | { mode: "edit"; status: SubscriptionStatus } | null>(null);
  const [cardMenu, setCardMenu] = useState<{ x: number; y: number; status: SubscriptionStatus; confirming: boolean } | null>(null);

  const loadSubscriptions = useCallback(async (force: boolean) => {
    if (!force && Date.now() - subscriptionsFetchedAt.current < 60_000) return;
    const request = ++subscriptionsLoadId.current;
    subscriptionsFetchedAt.current = Date.now();
    setSubscriptionsLoading(true);
    setSubscriptionsError("");
    try {
      const next = await api.fetchSubscriptions();
      if (request === subscriptionsLoadId.current) setSubscriptions(next);
    } catch (reason) {
      if (request === subscriptionsLoadId.current) setSubscriptionsError(`读取订阅配额失败：${errorText(reason)}`);
    } finally {
      if (request === subscriptionsLoadId.current) setSubscriptionsLoading(false);
    }
  }, []);

  const removeSubscription = useCallback(async (id: string) => {
    setCardMenu(null);
    try {
      await api.removeSubscriptionPlan(id);
      await loadSubscriptions(true);
    } catch (reason) {
      setSubscriptionsError(`删除订阅套餐失败：${errorText(reason)}`);
    }
  }, [loadSubscriptions]);

  // Any press, key or scroll outside the card menu dismisses it; menu items
  // stop the mousedown propagation so they still fire.
  useEffect(() => {
    if (!cardMenu) return;
    const close = () => setCardMenu(null);
    const onKeyDown = (event: KeyboardEvent) => { if (event.key === "Escape") close(); };
    window.addEventListener("mousedown", close);
    window.addEventListener("keydown", onKeyDown);
    window.addEventListener("resize", close);
    window.addEventListener("scroll", close, true);
    return () => {
      window.removeEventListener("mousedown", close);
      window.removeEventListener("keydown", onKeyDown);
      window.removeEventListener("resize", close);
      window.removeEventListener("scroll", close, true);
    };
  }, [cardMenu]);

  const loadId = useRef(0);
  const repositoryLoadId = useRef(0);

  const loadUsage = useCallback(async (mode: "sync" | "read", range: UsageRange) => {
    const request = ++usageLoadId.current;
    setUsage(null);
    setUsageError("");
    const pending = mode === "read" ? pendingUsageSync.current : null;
    setUsageLoading(pending ? "sync" : mode);
    let activeOperation: "sync" | "read" = mode;
    let sync: Promise<UsageStats> | null = null;
    try {
      if (mode === "sync") {
        sync = api.syncAgentUsage("omp", range);
        pendingUsageSync.current = sync;
      } else if (pending) {
        activeOperation = "sync";
        await pending;
        if (request !== usageLoadId.current) return;
        activeOperation = "read";
        setUsageLoading("read");
      }
      if (request !== usageLoadId.current) return;
      const stats = mode === "sync" ? await sync! : await api.getAgentUsage("omp", range);
      if (request === usageLoadId.current) setUsage(stats);
    } catch (reason) {
      if (request === usageLoadId.current) setUsageError(`${activeOperation === "sync" ? "OMP 用量同步" : "OMP 用量读取"}失败：${errorText(reason)}`);
    } finally {
      if (sync && pendingUsageSync.current === sync) pendingUsageSync.current = null;
      if (request === usageLoadId.current) setUsageLoading(null);
    }
  }, []);

  useEffect(() => {
    if (page === "omp") void loadUsage("sync", usageRangeRef.current);
    return () => { ++usageLoadId.current; pendingUsageSync.current = null; };
  }, [page, loadUsage]);

  useEffect(() => {
    if (page === "overview") void loadSubscriptions(false);
  }, [page, loadSubscriptions]);

  const loadAgentsUsage = useCallback(async (mode: "sync" | "read", range: UsageRange) => {
    const request = ++agentsUsageLoadId.current;
    setAgentsUsage(null);
    setAgentsUsageError("");
    setAgentsUsageLoading(mode);
    try {
      const stats = mode === "sync" ? await api.syncAgentsUsage(range) : await api.getAgentsUsage(range);
      if (request === agentsUsageLoadId.current) setAgentsUsage(stats);
    } catch (reason) {
      if (request === agentsUsageLoadId.current) setAgentsUsageError(`${mode === "sync" ? "Agent 用量同步" : "Agent 用量读取"}失败：${errorText(reason)}`);
    } finally {
      if (request === agentsUsageLoadId.current) setAgentsUsageLoading(null);
    }
  }, []);

  useEffect(() => {
    if (page === "agents" || page === "claude") {
      api.getClaudeCodeStatus().then(setClaudeCodeStatus).catch(() => setClaudeCodeStatus({ installed: false, version: "" }));
    }
    if (page === "agents") void loadAgentsUsage("sync", agentsUsageRangeRef.current);
  }, [page, loadAgentsUsage]);

  function changeAgentsUsageRange(range: UsageRange, label: string, choice: UsageChoice) {
    const changed = range !== agentsUsageRangeRef.current;
    agentsUsageRangeRef.current = range;
    setAgentsUsageSelection({ range, label, choice });
    if (changed) void loadAgentsUsage("read", range);
  }

  // 刷新状态会先从远程仓库根目录拉取定价配置，再同步所有 Agent 用量；
  // 定价拉取失败不阻塞统计，只提示已改用本地缓存定价。整段流程从入口
  // 就是互斥的：定价请求在途时按钮保持禁用，避免重复点击叠加命令。
  const [agentsRefreshBusy, setAgentsRefreshBusy] = useState(false);

  function refreshAgentsUsage() {
    if (agentsRefreshBusy || agentsUsageLoading !== null) return;
    setAgentsRefreshBusy(true);
    void (async () => {
      setAgentsPricingNote("");
      try {
        setAgentsPricingNote(await api.refreshPricing());
      } catch (reason) {
        setAgentsPricingNote(`定价配置刷新失败：${errorText(reason)}，已使用本地缓存定价。`);
      }
      try {
        await loadAgentsUsage("sync", agentsUsageRangeRef.current);
      } finally {
        setAgentsRefreshBusy(false);
      }
    })();
  }

  function changeUsageRange(range: UsageRange, label: string, choice: UsageChoice) {
    const changed = range !== usageRangeRef.current;
    usageRangeRef.current = range;
    setUsageSelection({ range, label, choice });
    if (changed) void loadUsage("read", range);
  }

  const loadClaudeUsage = useCallback(async (mode: "sync" | "read", range: UsageRange) => {
    const request = ++claudeUsageLoadId.current;
    setClaudeUsage(null);
    setClaudeUsageError("");
    const pending = mode === "read" ? pendingClaudeUsageSync.current : null;
    setClaudeUsageLoading(pending ? "sync" : mode);
    let activeOperation: "sync" | "read" = mode;
    let sync: Promise<UsageStats> | null = null;
    try {
      if (mode === "sync") {
        sync = api.syncAgentUsage("claude-code", range);
        pendingClaudeUsageSync.current = sync;
      } else if (pending) {
        activeOperation = "sync";
        await pending;
        if (request !== claudeUsageLoadId.current) return;
        activeOperation = "read";
        setClaudeUsageLoading("read");
      }
      if (request !== claudeUsageLoadId.current) return;
      const stats = mode === "sync" ? await sync! : await api.getAgentUsage("claude-code", range);
      if (request === claudeUsageLoadId.current) setClaudeUsage(stats);
    } catch (reason) {
      if (request === claudeUsageLoadId.current) setClaudeUsageError(`${activeOperation === "sync" ? "Claude Code 用量同步" : "Claude Code 用量读取"}失败：${errorText(reason)}`);
    } finally {
      if (sync && pendingClaudeUsageSync.current === sync) pendingClaudeUsageSync.current = null;
      if (request === claudeUsageLoadId.current) setClaudeUsageLoading(null);
    }
  }, []);

  useEffect(() => {
    if (page === "claude") void loadClaudeUsage("sync", claudeUsageRangeRef.current);
    return () => { ++claudeUsageLoadId.current; pendingClaudeUsageSync.current = null; };
  }, [page, loadClaudeUsage]);

  function changeClaudeUsageRange(range: UsageRange, label: string, choice: UsageChoice) {
    const changed = range !== claudeUsageRangeRef.current;
    claudeUsageRangeRef.current = range;
    setClaudeUsageSelection({ range, label, choice });
    if (changed) void loadClaudeUsage("read", range);
  }

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
        setError(`无法读取自动发现的来源：${errorText(reason)}`);
      }
    } finally {
      if (request === loadId.current) setLoading(false);
    }
    return null;
  }, []);

  async function loadRepositorySkills(repositories: Repository[]) {
    const request = ++repositoryLoadId.current;
    setRepositoryScanLoading(true);
    const results = await Promise.allSettled(repositories.map(async (repository) => [repository.id, await api.listRepositorySkills(repository.id)] as const));
    if (request !== repositoryLoadId.current) return;
    const loaded: Record<number, RepositorySkill[]> = {};
    const failures: Record<number, string> = {};
    results.forEach((result, index) => {
      const repository = repositories[index];
      if (result.status === "fulfilled") loaded[result.value[0]] = result.value[1];
      else failures[repository.id] = errorText(result.reason);
    });
    setRepositorySkills(loaded);
    setRepositorySkillErrors(failures);
    setRepositoryScanLoading(false);
    const failureText = Object.entries(failures).map(([id, reason]) => `#${id} ${repositories.find((repository) => repository.id === Number(id))?.url || "仓库"}：${reason}`).join("；");
    if (failureText) setError(`部分仓库扫描失败：${failureText}`);
  }

  const reloadAndLoad = useCallback(async (selected: Workspace): Promise<State | null> => {
    const next = await reload(selected);
    if (next) await loadRepositorySkills(next.repositories);
    return next;
  }, [reload]);

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
        setError(`无法自动发现通用来源：${errorText(reason)}`);
      }
    });
    return () => { cancelled = true; };
  }, [reloadAndLoad]);
  useEffect(() => {
    if (!plan && !mcpOpen && !repositoryRemoval) return;
    const onKey = (event: KeyboardEvent) => { if (event.key === "Escape" && !busy) { setPlan(null); setMcpOpen(false); setRepositoryRemoval(null); } };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [plan, mcpOpen, repositoryRemoval, busy]);

  async function operation<T>(label: string, work: () => Promise<T>): Promise<T | undefined> {
    setBusy(label); setError(""); setNotice("");
    try { return await work(); }
    catch (reason) { setError(`${label}失败：${errorText(reason)}`); }
    finally { setBusy(""); }
    return undefined;
  }

  async function prepare(label: string, work: () => Promise<Plan>) {
    const result = await operation(label, work);
    if (result) setPlan(result);
  }

  function openRepository(url: string) {
    setError("");
    void openUrl(url).catch((reason) => setError(`无法打开仓库链接：${errorText(reason)}`));
  }

  async function apply() {
    if (!plan) return;
    const result = await operation("应用变更", () => api.applyPlan(plan.id));
    if (result) {
      setPlan(null);
      setNotice(result.message || "变更已应用");
      if (workspace) await reloadAndLoad(workspace);
    }
  }



  function openMcp(server?: McpServer) {
    setError("");
    const config = server?.config && typeof server.config === "object" && !Array.isArray(server.config) ? server.config as Record<string, unknown> : {};
    const mode: McpMode = config.type === "sse" ? "sse" : config.type === "http" || typeof config.url === "string" ? "http" : "stdio";
    setMcpOriginal(server?.name || ""); setMcpName(server?.name || ""); setMcpMode(mode);
    setMcpCommand(typeof config.command === "string" ? config.command : "");
    setMcpArgs(Array.isArray(config.args) ? config.args.map(String).join("\n") : "");
    setMcpUrl(typeof config.url === "string" ? config.url : "");
    const extras = { ...config }; delete extras.type; delete extras.command; delete extras.args; delete extras.url; delete extras.enabled;
    setMcpExtra(JSON.stringify(extras, null, 2)); setMcpRaw(JSON.stringify(config, null, 2));
    setMcpEnabled(config.enabled !== false);
    setRawConfig(Boolean(config.type && !["stdio", "http", "sse"].includes(String(config.type))));
    setMcpOpen(true);
  }

  function parseConfigObject(text: string): Record<string, unknown> {
    const value: unknown = JSON.parse(text);
    if (!value || typeof value !== "object" || Array.isArray(value)) throw new Error("配置必须是 JSON 对象");
    return value as Record<string, unknown>;
  }

  function structuredMcpConfig(requireConnection: boolean): Record<string, unknown> {
    const extras = parseConfigObject(mcpExtra);
    if (mcpMode === "stdio") {
      if (requireConnection && !mcpCommand.trim()) throw new Error("请输入可执行命令");
      return { ...extras, type: "stdio", command: mcpCommand.trim(), args: mcpArgs.split("\n").filter((arg) => arg.length > 0), enabled: mcpEnabled };
    }
    if (requireConnection && !mcpUrl.trim()) throw new Error("请输入服务 URL");
    return { ...extras, type: mcpMode, url: mcpUrl.trim(), enabled: mcpEnabled };
  }

  function switchMcpEditor() {
    try {
      if (!rawConfig) {
        setMcpRaw(JSON.stringify(structuredMcpConfig(false), null, 2));
      } else {
        const config = parseConfigObject(mcpRaw);
        if (config.type && !["stdio", "http", "sse"].includes(String(config.type))) throw new Error("此连接类型只能在完整 JSON 模式下编辑");
        const mode: McpMode = config.type === "sse" ? "sse" : config.type === "http" || typeof config.url === "string" ? "http" : "stdio";
        const extras = { ...config }; delete extras.type; delete extras.command; delete extras.args; delete extras.url; delete extras.enabled;
        setMcpMode(mode);
        setMcpCommand(typeof config.command === "string" ? config.command : "");
        setMcpArgs(Array.isArray(config.args) ? config.args.map(String).join("\n") : "");
        setMcpUrl(typeof config.url === "string" ? config.url : "");
        setMcpExtra(JSON.stringify(extras, null, 2));
        setMcpEnabled(config.enabled !== false);
      }
      setError("");
      setRawConfig(!rawConfig);
    } catch (reason) { setError(`无法切换编辑模式：${errorText(reason)}`); }
  }

  async function saveMcp(event: FormEvent) {
    event.preventDefault();
    const selected = selectedWorkspace;
    if (!selected) { setError("正在自动发现通用来源，请稍候。"); return; }
    const name = mcpName.trim();
    if (!name) { setError("请输入 MCP 服务名称。"); return; }
    if (mcpOriginal && mcpOriginal !== name) { setError("修改服务名称需先删除旧服务，再添加新服务。"); return; }
    if (state?.mcp.some((server) => server.name === name && server.name !== mcpOriginal)) { setError("同名 MCP 服务已存在。"); return; }
    let config: Record<string, unknown>;
    try { config = rawConfig ? parseConfigObject(mcpRaw) : structuredMcpConfig(true); }
    catch (reason) { setError(`配置无效：${errorText(reason)}`); return; }
    const result = await operation("预览 MCP 变更", () => api.planMcp(selected, name, config));
    if (result) {
      setMcpOpen(false);
      setPlan(result);
    }
  }

  async function addRepository(event: FormEvent) {
    event.preventDefault();
    const url = repoUrl.trim();
    if (!url) return;
    const added = await operation("添加仓库", () => api.addRepository(url, repoRef.trim()));
    if (added) {
      setRepoUrl("");
      setRepoRef("");
      setNotice(`已添加 ${added.url}`);
      if (selectedWorkspace) await reloadAndLoad(selectedWorkspace);
    }
  }

  async function checkUpdates(id: number) {
    const result = await operation("检查更新", () => api.checkUpdates(id));
    if (result) {
      setNotice(result.message || "已检查仓库更新");
      if (selectedWorkspace) await reloadAndLoad(selectedWorkspace);
    }
  }

  async function refreshAllRepositories() {
    const result = await operation("刷新所有仓库", () => api.checkAllUpdates());
    if (!result) return;
    setNotice(result.message || "已刷新所有仓库");
    if (selectedWorkspace) await reloadAndLoad(selectedWorkspace);
  }

  async function removeRepository() {
    if (!repositoryRemoval) return;
    const repository = repositoryRemoval;
    const result = await operation("移除仓库", () => api.removeRepository(repository.id));
    if (!result) return;
    setRepositoryRemoval(null);
    setRepositorySkills((current) => {
      const next = { ...current };
      delete next[repository.id];
      return next;
    });
    setRepositorySkillErrors((current) => {
      const next = { ...current };
      delete next[repository.id];
      return next;
    });
    setNotice(result.message || `已移除 ${repository.localPath || repository.url}`);
    if (selectedWorkspace) await reloadAndLoad(selectedWorkspace);
  }

  const selectedWorkspace = workspace ?? state?.workspace;
  const installations = state?.installations || [];
  const activeInstallations = installations.filter((item) => item.active);
  const installedSkills = activeInstallations.map((installation) => ({
    installation,
    skill: state?.skills.find((skill) => skill.path === installation.targetPath),
  }));
  const detectedOnlySkills = (state?.skills || []).filter((skill) => !activeInstallations.some((installation) => installation.targetPath === skill.path));
  const discoveredSkills: DiscoveredSkill[] = state?.repositories.flatMap((repository) => (repositorySkills[repository.id] || []).map((skill) => ({ ...skill, repositoryId: repository.id }))) || [];
  const normalizedDiscoverSearch = discoverSearch.trim().toLocaleLowerCase();
  const filteredDiscoveredSkills = normalizedDiscoverSearch ? discoveredSkills.filter((skill) => {
    const repository = state?.repositories.find((item) => item.id === skill.repositoryId);
    return [skill.name, skill.description, skill.path, repository?.url, repository?.localPath, repository?.reference].filter(Boolean).join(" ").toLocaleLowerCase().includes(normalizedDiscoverSearch);
  }) : discoveredSkills;
  const repositoryFailureCount = Object.keys(repositorySkillErrors).length;
  const repositoryRecords = repositoryRemoval ? state?.installations.filter((item) => item.repositoryId === repositoryRemoval.id) || [] : [];
  const activeRepositoryCount = repositoryRecords.filter((item) => item.active).length;
  const inactiveRepositoryCount = repositoryRecords.length - activeRepositoryCount;
  const isBusy = Boolean(busy);
  const contentHeader = {
    overview: ["概览", "自动发现通用来源，集中查看 MCP、Skills 与仓库状态。"],
    agents: ["Agents", "管理代理环境与兼容性检测。"],
    omp: ["OMP", ""],
    claude: ["Claude Code", ""],
    mcp: ["MCP 服务", "查看通用来源，在应用前审阅每一处配置变更。"],
    skills: ["Skills", "管理已安装技能，保留本地修改的控制权。"],
    repositories: ["技能仓库", "从已发现的 Git 来源同步可用技能。"],
  }[page];

  return (
    <div className="app-shell">
      <aside className="sidebar">
        <div className="brand"><span className="brand-mark"><img src="/amc.svg" alt="" /></span><div><strong>AMC</strong><small>Agent Management Center</small></div></div>
        <div className="sidebar-caption">通用来源</div>
        <nav aria-label="主导航" className="navigation">
          {navigation.map((item) => <div key={item.id} className={`nav-section${item.group ? " group" : ""}${item.child ? " nav-child" : ""}`}>
            <button className={`nav-item ${page === item.id ? "selected" : ""}`} onClick={() => setPage(item.id)} aria-current={page === item.id ? "page" : undefined}>{item.icon === "omp" ? <img className="omp-nav-icon" src={ompIcon} alt="" /> : <Glyph name={item.icon} size={19} />}<span>{item.title}</span>{item.id === "agents" && <Glyph name="arrow" size={15} />}</button>
          </div>)}
        </nav>
        <div className="sidebar-bottom"><div className="sidebar-orbit"><Glyph name="shield" size={15} /><span>写入前预览确认</span></div><div className="sidebar-version"><span>版本</span><strong>{version}</strong></div></div>
      </aside>
      <main className="main-area">
        <div className="content">
          <div className="page-heading"><div><h1>{contentHeader[0]}{page === "omp" && state && <span className={`omp-version-pill ${state.agent.installed && state.agent.version ? "available" : "missing"}`}>{state.agent.installed && state.agent.version ? state.agent.version.replace(/^omp\//i, "") : "未检测到"}</span>}{page === "claude" && claudeCodeStatus && <span className={`omp-version-pill ${claudeCodeStatus.installed && claudeCodeStatus.version ? "available" : "missing"}`}>{claudeCodeStatus.installed && claudeCodeStatus.version ? claudeCodeStatus.version.split(/\s+/)[0] : "未检测到"}</span>}</h1>{page !== "omp" && page !== "claude" && <p>{contentHeader[1]}</p>}</div><button className="button button-muted refresh-button" aria-label={page === "omp" ? "刷新 OMP 用量" : page === "claude" ? "刷新 Claude Code 用量" : "刷新状态"} onClick={() => { if (page === "omp") void loadUsage("sync", usageRangeRef.current); else if (page === "claude") void loadClaudeUsage("sync", claudeUsageRangeRef.current); else { if (page === "agents") refreshAgentsUsage(); if (selectedWorkspace) void reload(selectedWorkspace); } }} disabled={page === "omp" ? usageLoading !== null : page === "claude" ? claudeUsageLoading !== null : page === "agents" ? agentsRefreshBusy || agentsUsageLoading !== null : loading || isBusy}><Glyph name="refresh" size={16} />{page === "omp" || page === "claude" ? "刷新用量" : "刷新状态"}</button></div>
          {error && <div className="alert alert-error" role="alert"><Glyph name="warning" size={18} /><span>{error}</span><button aria-label="关闭错误提示" onClick={() => setError("")}><Glyph name="close" size={16} /></button></div>}
          {notice && <div className="alert alert-success" role="status"><Glyph name="check" size={18} /><span>{notice}</span><button aria-label="关闭成功提示" onClick={() => setNotice("")}><Glyph name="close" size={16} /></button></div>}
          {page !== "omp" && page !== "claude" && loading && <div className="loading-panel" role="status"><span className="spinner" />正在自动发现通用来源…</div>}
          {page !== "omp" && page !== "claude" && !loading && !state && <Empty icon="warning" title="尚无法读取通用来源" description="AMC 会自动扫描用户级与兼容来源，请刷新重试。" action="重新扫描" onClick={() => { if (selectedWorkspace) void reloadAndLoad(selectedWorkspace); }} />}
          {page === "omp" && <AgentUsagePanel agentLabel="OMP" stats={usage} range={usageSelection.range} rangeLabel={usageSelection.label} activeChoice={usageSelection.choice} loading={usageLoading} error={usageError} onRangeChange={changeUsageRange} onRefresh={() => void loadUsage("sync", usageRangeRef.current)} />}
          {page === "claude" && <AgentUsagePanel agentLabel="Claude Code" stats={claudeUsage} range={claudeUsageSelection.range} rangeLabel={claudeUsageSelection.label} activeChoice={claudeUsageSelection.choice} loading={claudeUsageLoading} error={claudeUsageError} onRangeChange={changeClaudeUsageRange} onRefresh={() => void loadClaudeUsage("sync", claudeUsageRangeRef.current)} />}
          {!loading && state && <>
            {page === "overview" && <>
              <div className="stat-grid"><button className="stat-card" onClick={() => setPage("mcp")}><span className="stat-icon purple"><Glyph name="plug" /></span><span className="stat-value">{state.mcp.length}</span><span className="stat-title">MCP 服务</span><small>查看配置来源 <Glyph name="arrow" size={13} /></small></button><button className="stat-card" onClick={() => setPage("skills")}><span className="stat-icon amber"><Glyph name="spark" /></span><span className="stat-value">{state.skills.length}</span><span className="stat-title">检测到的 Skills</span><small>查看技能来源 <Glyph name="arrow" size={13} /></small></button><button className="stat-card" onClick={() => setPage("repositories")}><span className="stat-icon mint"><Glyph name="repo" /></span><span className="stat-value">{state.repositories.length}</span><span className="stat-title">Git 仓库</span><small>发现更多技能 <Glyph name="arrow" size={13} /></small></button></div>
              <SubscriptionSection statuses={subscriptions} loading={subscriptionsLoading} error={subscriptionsError} onRefresh={() => void loadSubscriptions(true)} onAddPlan={() => setPlanForm({ mode: "add" })} onMenu={(status, x, y) => setCardMenu({ x, y, status, confirming: false })} />
            </>}
            {page === "agents" && <>
              <AgentsUsagePanel stats={agentsUsage} range={agentsUsageSelection.range} rangeLabel={agentsUsageSelection.label} activeChoice={agentsUsageSelection.choice} loading={agentsUsageLoading} error={agentsUsageError} pricingNote={agentsPricingNote} onRangeChange={changeAgentsUsageRange} onRefresh={refreshAgentsUsage} />
              <section className="section-block">
                <div className="section-heading"><div><h2>已接入 Agent</h2><p>只读检测本机可用的 Agent 运行时，不修改其配置。</p></div></div>
                <div className="card-list">
                  <article className="item-card">
                    <div className="item-icon omp-icon-tile"><img src={ompIcon} alt="" /></div>
                    <div className="item-content"><div className="item-title"><h3>OMP</h3><span className={`tag ${state.agent.installed ? "tag-good" : "tag-muted"}`}>{state.agent.installed ? "已检测到" : "未检测到"}</span></div><p>{state.agent.installed ? (state.agent.version || "已找到可执行文件，但无法读取版本") : "未找到可用的 omp 可执行文件"}</p></div>
                    <div className="item-actions"><button className="button button-muted" onClick={() => setPage("omp")}>查看详情 <Glyph name="arrow" size={14} /></button></div>
                  </article>
                  <article className="item-card">
                    <div className="item-icon claude-icon-tile"><Glyph name="claude" size={22} /></div>
                    <div className="item-content"><div className="item-title"><h3>Claude Code</h3><span className={`tag ${claudeCodeStatus?.installed ? "tag-good" : "tag-muted"}`}>{claudeCodeStatus?.installed ? "已检测到" : "未检测到"}</span></div><p>{claudeCodeStatus ? (claudeCodeStatus.installed ? (claudeCodeStatus.version || "已找到可执行文件，但无法读取版本") : "未找到可用的 claude 可执行文件") : "正在检测 Claude Code CLI…"}</p></div>
                    <div className="item-actions"><button className="button button-muted" onClick={() => setPage("claude")}>查看详情 <Glyph name="arrow" size={14} /></button></div>
                  </article>
                </div>
              </section>
            </>}
            {page === "mcp" && <>
              <div className="section-heading section-heading-top">
                <div><h2>服务列表 <span className="count">{state.mcp.length}</span></h2><p>通用来源仅供查看；只编辑 AMC 通用可写来源管理的服务。</p></div>
                <button className="button button-primary" onClick={() => openMcp()} disabled={isBusy}><Glyph name="plus" size={17} />添加服务</button>
              </div>
              {state.mcp.length ? (
                <div className="card-list">
                  {state.mcp.map((server) => <article className="item-card" key={`${server.source}:${server.name}`}>
                    <div className="item-icon purple"><Glyph name="plug" /></div>
                    <div className="item-content">
                      <div className="item-title">
                        <h3>{server.name}</h3>
                        <span className={`tag ${server.enabled ? "tag-good" : "tag-muted"}`}>{server.enabled ? "已启用" : "未启用"}</span>
                        <span className="tag tag-muted">{server.managed ? "通用可写来源" : "只读兼容来源"}</span>
                      </div>
                      <p>{server.source || "未知通用来源"}</p>
                    </div>
                    {server.managed && <div className="item-actions">
                      <button className="button button-muted" onClick={() => openMcp(server)} disabled={isBusy}>编辑</button>
                      <button className="button button-danger-ghost" onClick={() => { if (selectedWorkspace) void prepare("预览删除服务", () => api.planMcp(selectedWorkspace, server.name, null)); }} disabled={isBusy}>删除</button>
                    </div>}
                  </article>)}
                </div>
              ) : <Empty icon="plug" title="还没有 MCP 服务" description="添加 stdio、HTTP 或 SSE 服务，先预览配置再写入。" action="添加服务" onClick={() => openMcp()} />}
            </>}
            {page === "skills" && <>
              <div className="section-heading section-heading-top skills-heading">
                <div className="skills-context"><p>{skillTab === "installed" ? "管理已安装技能，预览更新、回滚或卸载。" : "从已添加的仓库发现技能，逐个选择安装。"}</p></div>
                <div className="section-actions">
                  <div className="skill-tabs" role="tablist" aria-label="Skills 视图">
                    <button className={`skill-tab ${skillTab === "installed" ? "active" : ""}`} role="tab" aria-selected={skillTab === "installed"} onClick={() => setSkillTab("installed")}>已安装 <span>{activeInstallations.length}</span></button>
                    <button className={`skill-tab ${skillTab === "discover" ? "active" : ""}`} role="tab" aria-selected={skillTab === "discover"} onClick={() => setSkillTab("discover")}>发现技能 <span>{discoveredSkills.length}</span></button>
                  </div>
                  {skillTab === "discover" && <>
                    <div className="skill-tool-buttons">
                      <button className="button button-primary" onClick={() => setPage("repositories")}><Glyph name="plus" size={16} />管理仓库</button>
                      <button className="button button-muted" onClick={() => void refreshAllRepositories()} disabled={isBusy || repositoryScanLoading}><Glyph name="refresh" size={16} />刷新所有仓库</button>
                      <button className="button button-muted skill-search-toggle" aria-label="搜索" title="搜索" aria-pressed={discoverSearchOpen} onClick={() => { setDiscoverSearchOpen((open) => !open); if (discoverSearchOpen) setDiscoverSearch(""); }}><Glyph name="search" size={18} /></button>
                    </div>
                  </>}
                </div>
              </div>
              {skillTab === "discover" && discoverSearchOpen && <div className="skill-search-row"><div className="skill-search"><Glyph name="search" size={16} /><input autoFocus value={discoverSearch} onChange={(event) => setDiscoverSearch(event.target.value)} placeholder="搜索名称、描述或来源" aria-label="搜索发现的 Skill" /><button type="button" className="icon-button skill-search-clear" aria-label="关闭搜索" onClick={() => { setDiscoverSearch(""); setDiscoverSearchOpen(false); }}><Glyph name="close" size={15} /></button></div></div>}
              {skillTab === "installed" ? <>
                {installedSkills.length ? <div className="card-list">
                  {installedSkills.map(({ installation, skill }) => {
                    return <article className="item-card" key={installation.id}>
                      <div className="item-icon amber"><Glyph name="spark" /></div>
                      <div className="item-content">
                        <div className="item-title"><h3>{skill?.name || installation.name}</h3><span className="tag tag-good">已安装</span>{installation.modified && <span className="tag tag-warn">本地已修改</span>}{installation.updateAvailable && <span className="tag tag-info">有更新</span>}</div>
                        <p>{skill?.description || "已安装技能，当前扫描中未找到对应文件。"}</p>
                      </div>
                      <div className="item-actions">
                        {installation.updateAvailable && <button className="button button-primary" disabled={isBusy} onClick={() => void prepare("预览技能更新", () => api.planSync(installation.id))}>更新</button>}
                        {installation.rollbackAvailable && <button className="button button-muted" disabled={isBusy} onClick={() => void prepare("预览技能回滚", () => api.rollbackSkill(installation.id))}>回滚</button>}
                        <button className="button button-danger-ghost" disabled={isBusy} onClick={() => void prepare("预览移除技能", () => api.planRemoveSkill(installation.id))}>移除</button>
                      </div>
                    </article>;
                  })}
                </div> : <Empty icon="spark" title="还没有已安装的技能" description="切换到“发现技能”，从已添加的仓库选择要安装的 Skill。" action="发现技能" onClick={() => setSkillTab("discover")} />}
                {detectedOnlySkills.length > 0 && <section className="section-block detected-skills">
                  <div className="section-heading"><div><h2>已检测到的其他 Skills <span className="count">{detectedOnlySkills.length}</span></h2><p>这些技能来自本地通用或兼容来源，但没有 AMC 安装记录；这里只读展示，不提供更新或卸载操作。</p></div></div>
                  <div className="card-list">
                    {detectedOnlySkills.map((skill) => <article className="item-card" key={`${skill.source}:${skill.path}`}>
                      <div className="item-icon amber"><Glyph name="spark" /></div>
                      <div className="item-content"><div className="item-title"><h3>{skill.name}</h3>{skill.shadowed && <span className="tag tag-warn">同名来源</span>}<span className="tag tag-muted">{skill.managed ? "通用检测" : "只读检测"}</span></div><p>{skill.description || "此技能未提供说明"}</p></div>
                    </article>)}
                  </div>
                </section>}
                {installations.some((item) => !item.active) && <section className="section-block">
                  <div className="section-heading"><h2>安装记录</h2></div>
                  <div className="card-list">
                    {installations.filter((item) => !item.active).map((item) => <article className="item-card" key={item.id}>
                      <div className="item-icon mint"><Glyph name="branch" /></div>
                      <div className="item-content"><div className="item-title"><h3>{item.name}</h3><span className="tag tag-muted">已移除</span></div><p>{item.targetPath} · {item.commit ? item.commit.slice(0, 8) : "—"}</p></div>
                      <div className="item-actions">{item.rollbackAvailable && <button className="button button-muted" disabled={isBusy} onClick={() => void prepare("预览技能回滚", () => api.rollbackSkill(item.id))}>回滚</button>}</div>
                    </article>)}
                  </div>
                </section>}
              </> : <>
                {repositoryScanLoading ? (
                  <div className="loading-panel"><span className="spinner" />正在扫描仓库技能…</div>
                ) : repositoryFailureCount > 0 ? (
                  <div className="plan-warnings"><p><Glyph name="warning" size={16} />{repositoryFailureCount} 个仓库扫描失败；成功扫描的技能仍会显示，可在“仓库”页面单独刷新。</p></div>
                ) : null}
                {filteredDiscoveredSkills.length ? (
                  <div className="skill-discovery-grid">
                    {filteredDiscoveredSkills.map((skill) => {
                      const repository = state.repositories.find((item) => item.id === skill.repositoryId);
                      const skillUrl = skillWebUrl(repository, skill.path);
                      const installed = activeInstallations.find((item) => item.repositoryId === skill.repositoryId && item.skillPath === skill.path);
                      const removed = installations.find((item) => !item.active && item.repositoryId === skill.repositoryId && item.skillPath === skill.path);
                      return <article className="item-card skill-discovery-card" key={`${skill.repositoryId}:${skill.path}`}>
                        <div className="skill-card-heading"><div className="item-icon amber"><Glyph name="spark" /></div><div className="item-title"><h3>{skill.name}</h3>{installed && <span className="tag tag-good">已安装</span>}{!installed && removed && <span className="tag tag-muted">已移除</span>}{installed?.modified && <span className="tag tag-warn">本地已修改</span>}{installed?.updateAvailable && <span className="tag tag-info">有更新</span>}</div></div>
                        <div className="item-content skill-card-body"><p>{skill.description || "此技能未提供说明"}</p></div>
                        <div className="item-actions">{skillUrl && <button type="button" className="icon-button skill-source-link" title="打开 GitHub 技能目录" aria-label={`打开 ${skill.name} 的 GitHub 技能目录`} onClick={() => openRepository(skillUrl)}><Glyph name="external" size={15} /></button>}{installed ? <>{installed.updateAvailable && <button className="button button-primary" disabled={isBusy} onClick={() => void prepare("预览技能更新", () => api.planSync(installed.id))}>更新</button>}{installed.rollbackAvailable && <button className="button button-muted" disabled={isBusy} onClick={() => void prepare("预览技能回滚", () => api.rollbackSkill(installed.id))}>回滚</button>}<button className="button button-danger-ghost" disabled={isBusy} onClick={() => void prepare("预览移除技能", () => api.planRemoveSkill(installed.id))}>移除</button></> : <>{removed?.rollbackAvailable && <button className="button button-muted" disabled={isBusy} onClick={() => void prepare("预览技能回滚", () => api.rollbackSkill(removed.id))}>回滚</button>}<button className="button button-primary skill-install-button" disabled={isBusy || !selectedWorkspace} onClick={() => { if (selectedWorkspace) void prepare("预览安装技能", () => api.planSkill(selectedWorkspace, skill.repositoryId, skill.path)); }}>{removed ? "重新安装" : "安装"}</button></>}</div>
                      </article>;
                    })}
                  </div>
                ) : !repositoryScanLoading ? (
                  <Empty icon="spark" title={normalizedDiscoverSearch ? "未找到匹配的技能" : "未发现可安装的技能"} description={normalizedDiscoverSearch ? `没有匹配“${discoverSearch.trim()}”的技能。` : repositoryFailureCount > 0 ? "没有成功扫描到技能；请刷新失败仓库后重试。" : state.repositories.length ? "仓库中需要包含带 name 和 description 的 SKILL.md。" : "先添加 Git 仓库，之后可以在这里发现并选择安装 Skill。"} />
                ) : null}
              </>}
            </>}
            {page === "repositories" && <>
              <section className="panel add-repo">
                <div className="panel-header">
                  <span className="panel-icon"><Glyph name="plus" size={20} /></span>
                  <div><h2>添加 Git 仓库</h2><p>支持 HTTPS、SSH 或本地绝对路径。不会运行仓库中的脚本。</p></div>
                </div>
                <form onSubmit={(event) => void addRepository(event)} className="repo-form">
                  <label>仓库地址<input required placeholder="https://github.com/owner/repo.git" value={repoUrl} onChange={(event) => setRepoUrl(event.target.value)} /></label>
                  <label>分支 / 标签（可选）<input placeholder="默认分支" value={repoRef} onChange={(event) => setRepoRef(event.target.value)} /></label>
                  <button type="submit" className="button button-primary" disabled={isBusy || !repoUrl.trim()}>{busy === "添加仓库" ? "正在添加…" : "添加并扫描"}</button>
                </form>
              </section>
              <div className="section-heading"><div><h2>已添加来源 <span className="count">{state.repositories.length}</span></h2><p>仓库技能会在加载和刷新后统一发现；可在这里更新来源或移除来源。</p></div></div>
              {state.repositories.length ? <div className="card-list">
                {state.repositories.map((repo) => {
                  const repositoryUrl = repositoryWebUrl(repo);
                  return <article className="item-card repo-card" key={repo.id}>
                    <div className="item-icon mint"><Glyph name={repo.localPath ? "folder" : "repo"} /></div>
                    <div className="item-content"><h3>{repo.localPath ? "本地 Git 仓库" : repo.url}</h3>{repo.localPath && <p className="path-line">{repo.localPath}</p>}<p>{repo.localPath ? "本地仓库工作树" : `引用：${repo.reference || "默认分支"}`} · {repositorySkills[repo.id]?.length ?? (repositoryScanLoading ? "扫描中" : 0)} 项技能 · {activeInstallations.filter((item) => item.repositoryId === repo.id).length} 项已安装</p></div>
                    <div className="item-actions">
                      {repositoryUrl && <button type="button" className="icon-button skill-source-link" title="打开 GitHub 仓库" aria-label={`打开 ${repo.url} 的 GitHub 仓库`} onClick={() => openRepository(repositoryUrl)}><Glyph name="external" size={15} /></button>}
                      <button className="button button-muted" disabled={isBusy || repositoryScanLoading} onClick={() => void checkUpdates(repo.id)}>检查更新</button>
                      <button className="button button-danger-ghost" disabled={isBusy} onClick={() => { setError(""); setRepositoryRemoval(repo); }}>{repo.localPath ? "移除来源" : "移除仓库"}</button>
                    </div>
                  </article>;
                })}
              </div> : <Empty icon="repo" title="还没有技能来源" description="输入 Git 地址以发现可安装的技能。" />}
            </>}
          </>}
        </div>
      </main>
      {repositoryRemoval && <div className="modal-backdrop" onMouseDown={(event) => { if (event.target === event.currentTarget && !isBusy) setRepositoryRemoval(null); }}>
        <div className="modal" role="dialog" aria-modal="true" aria-labelledby="remove-repo-title">
          <div className="modal-head">
            <div><div className="eyebrow">技能来源 / 移除确认</div><h2 id="remove-repo-title">{repositoryRemoval.localPath ? "移除本地 Git 来源" : "移除 Git 仓库"}</h2></div>
            <button className="icon-button" aria-label="关闭" disabled={isBusy} onClick={() => setRepositoryRemoval(null)}><Glyph name="close" size={20} /></button>
          </div>
          <div className="modal-body">
            {error && <div className="alert alert-error" role="alert"><Glyph name="warning" size={18} /><span>{error}</span></div>}
            <p className="repo-remove-url">{repositoryRemoval.localPath || repositoryRemoval.url}</p>
            {activeRepositoryCount > 0 ? <div className="plan-warnings"><p><Glyph name="warning" size={16} />此来源还有 {activeRepositoryCount} 项已安装技能。请先移除这些安装，再移除来源。</p></div> :
              <div className="plan-warnings"><p><Glyph name="warning" size={16} />将删除来源记录及 AMC 管理的{repositoryRemoval.localPath ? "本地 Git 缓存" : "Git 缓存"}{inactiveRepositoryCount > 0 ? `，并永久清理 ${inactiveRepositoryCount} 条已移除技能的安装历史与回滚备份` : ""}。{repositoryRemoval.localPath && "原始路径不会删除。"}此操作无法撤销。</p></div>}
            <div className="modal-actions">
              <button className="button button-muted" disabled={isBusy} onClick={() => setRepositoryRemoval(null)}>取消</button>
              <button className="button button-danger" disabled={isBusy || activeRepositoryCount > 0} onClick={() => void removeRepository()}>{isBusy ? "正在移除…" : repositoryRemoval.localPath ? "确认移除来源" : "确认移除仓库"}</button>
            </div>
          </div>
        </div>
      </div>}
      {mcpOpen && <div className="modal-backdrop" onMouseDown={(event) => { if (event.target === event.currentTarget && !isBusy) setMcpOpen(false); }}>
        <div className="modal" role="dialog" aria-modal="true" aria-labelledby="mcp-title">
          <div className="modal-head">
            <div><div className="eyebrow">MCP / 配置</div><h2 id="mcp-title">{mcpOriginal ? "编辑服务" : "添加 MCP 服务"}</h2></div>
            <button className="icon-button" aria-label="关闭" disabled={isBusy} onClick={() => setMcpOpen(false)}><Glyph name="close" size={20} /></button>
          </div>
          <form onSubmit={(event) => void saveMcp(event)} className="modal-body mcp-form">
            {error && <div className="alert alert-error" role="alert"><Glyph name="warning" size={18} /><span>{error}</span><button type="button" aria-label="关闭错误提示" onClick={() => setError("")}><Glyph name="close" size={16} /></button></div>}
            <label>服务名称<input value={mcpName} onChange={(event) => setMcpName(event.target.value)} placeholder="例如 filesystem" required readOnly={Boolean(mcpOriginal)} /></label>
            {!rawConfig && <label className="checkbox-row"><input type="checkbox" checked={mcpEnabled} onChange={(event) => setMcpEnabled(event.target.checked)} />启用此服务</label>}
            <label className="checkbox-row"><input type="checkbox" checked={rawConfig} onChange={switchMcpEditor} />直接编辑完整 JSON 配置</label>
            {rawConfig ? <label>配置对象<textarea className="code-editor" spellCheck={false} rows={12} value={mcpRaw} onChange={(event) => setMcpRaw(event.target.value)} /></label> : <>
              <div>
                <span className="field-label">连接方式</span>
                <div className="segmented">
                  <button type="button" className={mcpMode === "stdio" ? "active" : ""} onClick={() => setMcpMode("stdio")}>stdio</button>
                  <button type="button" className={mcpMode === "http" ? "active" : ""} onClick={() => setMcpMode("http")}>HTTP</button>
                  <button type="button" className={mcpMode === "sse" ? "active" : ""} onClick={() => setMcpMode("sse")}>SSE</button>
                </div>
              </div>
              {mcpMode === "stdio" ? <>
                <label>命令<input value={mcpCommand} onChange={(event) => setMcpCommand(event.target.value)} placeholder="例如 npx" required /></label>
                <label>参数（每行一项）<textarea rows={3} value={mcpArgs} onChange={(event) => setMcpArgs(event.target.value)} placeholder={"-y\n@modelcontextprotocol/server-filesystem"} /></label>
              </> : <label>服务 URL<input type="url" value={mcpUrl} onChange={(event) => setMcpUrl(event.target.value)} placeholder="https://example.com/mcp" required /></label>}
              <label>附加配置（JSON 对象，可选）<textarea className="code-editor" spellCheck={false} rows={5} value={mcpExtra} onChange={(event) => setMcpExtra(event.target.value)} placeholder='{"env": {"KEY": "value"}}' /><small>可填写 env、headers 等字段；不会执行服务命令。</small></label>
            </>}
            <div className="modal-actions">
              <button type="button" className="button button-muted" disabled={isBusy} onClick={() => setMcpOpen(false)}>取消</button>
              <button type="submit" className="button button-primary" disabled={isBusy}>{isBusy ? "正在生成…" : "生成预览"}</button>
            </div>
          </form>
        </div>
      </div>}
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
      {planForm && <PlanFormModal form={planForm} onClose={() => setPlanForm(null)} onSaved={() => { setPlanForm(null); void loadSubscriptions(true); }} />}
      {cardMenu && <div className="card-menu" role="menu" style={{ left: Math.max(8, Math.min(cardMenu.x, window.innerWidth - 156)), top: Math.max(8, Math.min(cardMenu.y, window.innerHeight - 98)) }}>
        <button role="menuitem" onMouseDown={(event) => { event.stopPropagation(); setCardMenu(null); setPlanForm({ mode: "edit", status: cardMenu.status }); }}>
          <Glyph name="edit" size={15} />编辑
        </button>
        {cardMenu.confirming
          ? <button role="menuitem" className="danger" onMouseDown={(event) => { event.stopPropagation(); void removeSubscription(cardMenu.status.id); }}><Glyph name="trash" size={15} />确认删除</button>
          : <button role="menuitem" className="danger" onMouseDown={(event) => { event.stopPropagation(); setCardMenu({ ...cardMenu, confirming: true }); }}><Glyph name="trash" size={15} />删除</button>}
      </div>}
    </div>
  );
}

const usageNumber = new Intl.NumberFormat("zh-CN");
const usageTokenNumber = { format: (tokens: number) => tokens >= 1_000_000
  ? `${(tokens / 1_000_000).toFixed(2)}M`
  : tokens >= 1_000 ? `${(tokens / 1_000).toFixed(2)}K` : usageNumber.format(tokens) };
const usagePercent = new Intl.NumberFormat("zh-CN", { style: "percent", maximumFractionDigits: 1 });
const usageCost = new Intl.NumberFormat("en-US", { style: "currency", currency: "USD", maximumFractionDigits: 4 });

function formatUsageCost(cost: number | null) {
  if (cost === null) return "—";
  return cost > 0 && cost < 0.0001 ? "< $0.0001" : usageCost.format(cost);
}

function formatUsageTime(timestamp: number, hourly: boolean) {
  return new Intl.DateTimeFormat("zh-CN", hourly
    ? { month: "numeric", day: "numeric", hour: "2-digit", minute: "2-digit" }
    : { year: "numeric", month: "numeric", day: "numeric" }).format(new Date(timestamp));
}

function formatResetCountdown(resetsAt: number): string {
  const minutes = Math.floor((resetsAt - Date.now()) / 60_000);
  if (minutes <= 0) return "即将重置";
  const days = Math.floor(minutes / 1440);
  const hours = Math.floor((minutes % 1440) / 60);
  const rest = minutes % 60;
  if (days > 0) return `${days} 天 ${hours} 小时后重置`;
  if (hours > 0) return `${hours} 小时 ${rest} 分后重置`;
  return `${minutes} 分后重置`;
}

function formatQuotaCount(kind: string, value: number): string {
  return kind === "tokens" ? usageTokenNumber.format(value) : usageNumber.format(value);
}

function metricIcon(id: string): string {
  if (id === "tokens" || id === "model") return "spark";
  if (id === "requests") return "agents";
  if (id === "search") return "search";
  if (id === "zread") return "code";
  return "plug";
}

function PlanFormModal({ form, onClose, onSaved }: {
  form: { mode: "add" } | { mode: "edit"; status: SubscriptionStatus };
  onClose: () => void;
  onSaved: () => void;
}) {
  const editing = form.mode === "edit" ? form.status : null;
  const [kinds, setKinds] = useState<SubscriptionKind[]>([]);
  const [kind, setKind] = useState(editing?.provider ?? "glm");
  const [name, setName] = useState(editing?.title ?? "");
  const [nameTouched, setNameTouched] = useState(editing !== null);
  const [platform, setPlatform] = useState(editing?.platform ?? "zai");
  const [key, setKey] = useState("");
  const [saving, setSaving] = useState(false);
  const [error, setError] = useState("");
  const selected = kinds.find((entry) => entry.id === kind);

  // Catalog from the backend drives the kind picker and the credential
  // fields; the name follows the picked kind until the user edits it.
  useEffect(() => {
    let cancelled = false;
    api.listSubscriptionKinds().then((list) => {
      if (cancelled || list.length === 0) return;
      setKinds(list);
      if (!editing) {
        setKind(list[0].id);
        setName((current) => current || list[0].title);
      }
    }).catch(() => { /* keep the glm default; add validates server-side */ });
    return () => { cancelled = true; };
  }, [editing]);
  const submit = async () => {
    if (saving || !name.trim() || (!editing && !key.trim())) return;
    setSaving(true);
    setError("");
    try {
      if (editing) await api.updateSubscriptionPlan(editing.id, name.trim(), platform, key.trim());
      else await api.addSubscriptionPlan(kind, name.trim(), platform, key.trim());
      onSaved();
    } catch (reason) {
      setError(errorText(reason));
    } finally {
      setSaving(false);
    }
  };
  return <div className="modal-backdrop" onMouseDown={(event) => { if (event.target === event.currentTarget) onClose(); }}>
    <div className="modal" role="dialog" aria-modal="true" aria-labelledby="plan-form-title">
      <div className="modal-head">
        <div><div className="eyebrow">订阅与配额</div><h2 id="plan-form-title">{editing ? "编辑订阅套餐" : "添加订阅套餐"}</h2></div>
        <button className="icon-button" aria-label="关闭" onClick={onClose}><Glyph name="close" size={20} /></button>
      </div>
      <form className="modal-body" onSubmit={(event) => { event.preventDefault(); void submit(); }}>
        <small className="form-hint">同一个订阅可以添加多份，用名称区分；凭据只保存在本机。</small>
        <div className="subscription-form">
          {!editing && kinds.length > 0 && <label>套餐类型
            <select value={kind} onChange={(event) => {
              const next = kinds.find((entry) => entry.id === event.target.value);
              setKind(event.target.value);
              if (next) {
                if (!nameTouched) setName(next.title);
                if (next.platforms[0]) setPlatform(next.platforms[0][0]);
              }
            }}>
              {kinds.map((entry) => <option key={entry.id} value={entry.id}>{entry.title}</option>)}
            </select>
          </label>}
          <label>名称
            <input value={name} maxLength={100} placeholder="例如：主力账号" autoFocus onChange={(event) => { setNameTouched(true); setName(event.target.value); }} />
          </label>
          {/* 平台与凭据字段随所选套餐类型切换 */}
          <label>平台
            <select value={platform} onChange={(event) => setPlatform(event.target.value)}>
              {(selected?.platforms ?? []).map(([value, label]) => <option key={value} value={value}>{label}</option>)}
            </select>
          </label>
          <label>{selected?.keyLabel ?? "凭据 Key"}
            <input type="password" value={key} autoComplete="off" placeholder={editing ? `留空则保留现有 Key（${editing.keyHint ?? "已保存"}）` : selected?.keyPlaceholder ?? "粘贴凭据 Key"} onChange={(event) => setKey(event.target.value)} />
          </label>
        </div>
        {error && <small className="subscription-form-error" role="alert">{error}</small>}
        <div className="modal-actions">
          <button className="button button-muted" type="button" onClick={onClose}>取消</button>
          <button className="button button-primary" type="submit" disabled={saving || !name.trim() || (!editing && !key.trim())}>{saving ? "保存中…" : editing ? "保存" : "添加"}</button>
        </div>
      </form>
    </div>
  </div>;
}

function SubscriptionCard({ status, onMenu }: { status: SubscriptionStatus; onMenu: (status: SubscriptionStatus, x: number, y: number) => void }) {
  return <article className="subscription-card" onContextMenu={(event) => { event.preventDefault(); onMenu(status, event.clientX, event.clientY); }}>
    <div className="subscription-head">
      <span className="subscription-icon"><img src={zaiIcon} alt="" /></span>
      <div className="subscription-title">
        <h3>{status.title}{status.plan && <span className="tag tag-muted">{status.plan}</span>}</h3>
      </div>
    </div>
    {status.error
      ? <div className="subscription-error" role="alert"><Glyph name="warning" size={16} /><span>{status.error}</span></div>
      : <>
        <div className="quota-list">
          {status.quotas.map((quota) => {
            const tone = quota.usedPercent >= 90 ? "danger" : quota.usedPercent >= 70 ? "warn" : "ok";
            const counts = quota.used !== null && quota.total !== null
              ? `${formatQuotaCount(quota.kind, quota.used)} / ${formatQuotaCount(quota.kind, quota.total)}`
              : null;
            return <div key={quota.label} className="quota-row">
              <div className="quota-top">
                <strong>{quota.label}</strong>
                <span className={`quota-state ${tone}`}>{quota.usedPercent.toFixed(1)}%{counts ? ` · ${counts}` : ""}{quota.resetsAt ? ` · ${formatResetCountdown(quota.resetsAt)}` : ""}</span>
              </div>
              <div className="quota-bar" role="img" aria-label={`${quota.label}已用 ${quota.usedPercent.toFixed(1)}%`}><span className={tone} style={{ width: `${Math.min(100, Math.max(2, quota.usedPercent))}%` }} /></div>
              {quota.details.length > 0 && <small className="quota-details">{quota.details.map((detail) => `${detail.name} ${usageNumber.format(detail.usage)}`).join(" · ")}</small>}
            </div>;
          })}
        </div>
        {status.metrics.length > 0 && <div className="metric-row">
          {status.metrics.map((metric) => <span key={metric.id + metric.label} className="metric-chip"><Glyph name={metricIcon(metric.id)} size={14} /><span>{metric.label}</span><strong>{formatQuotaCount(metric.id === "tokens" || metric.id === "model" ? "tokens" : "plain", metric.value)}</strong></span>)}
        </div>}
      </>}
  </article>;
}

function SubscriptionSection({ statuses, loading, error, onRefresh, onAddPlan, onMenu }: {
  statuses: SubscriptionStatus[] | null;
  loading: boolean;
  error: string;
  onRefresh: () => void;
  onAddPlan: () => void;
  onMenu: (status: SubscriptionStatus, x: number, y: number) => void;
}) {
  return <section className="section-block">
    <div className="section-heading">
      <div><h2>订阅与配额</h2><p>通过官方查询接口读取订阅余量与用量。</p></div>
      <div className="section-actions">
        <button className="button button-muted" onClick={onRefresh} disabled={loading}><Glyph name="refresh" size={16} />{loading ? "查询中…" : "刷新"}</button>
        <button className="button button-muted" onClick={onAddPlan}><Glyph name="plus" size={16} />添加套餐</button>
      </div>
    </div>
    {error && <div className="subscription-error" role="alert"><Glyph name="warning" size={16} /><span>{error}</span></div>}
    <div className="subscription-list">
      {(statuses || []).map((status) => <SubscriptionCard key={status.id} status={status} onMenu={onMenu} />)}
      {statuses && statuses.length === 0 && <Empty icon="spark" title="尚未添加订阅套餐" description="添加套餐并填写名称与 Key 后，即可在此查看对应订阅的配额窗口与用量统计；同一个订阅可添加多份，用名称区分。" action="添加订阅套餐" onClick={onAddPlan} />}
      {!statuses && !error && loading && <div className="loading-panel" role="status"><span className="spinner" />正在查询订阅配额…</div>}
    </div>
  </section>;
}

function UsageTrend({ trend, range }: { trend: UsageStats["trend"]; range: UsageRange }) {
  const [active, setActive] = useState<number | null>(null);
  const points = [...trend].sort((a, b) => a.bucket - b.bucket);
  if (!points.length) return <p className="omp-no-breakdown">此时间范围内没有趋势数据。</p>;
  const customBounds = range.startsWith("custom:") ? range.split(":").slice(1).map(Number) : null;
  const hourly = range === "1h" || range === "24h" || (customBounds !== null && customBounds[1] - customBounds[0] <= 48 * 60 * 60 * 1000);

  let max = 1;
  for (const point of points) max = Math.max(max, point.totalTokens);
  const axisLabel = usageTokenNumber.format(max);
  const chartLeft = Math.max(32, 12 + axisLabel.length * 7);
  const chartRight = 690;
  const x = (index: number) => chartLeft + index * (chartRight - chartLeft) / Math.max(1, points.length - 1);
  const y = (value: number) => 170 - value / max * 128;
  const line = points.map((point, index) => `${index ? "L" : "M"} ${x(index)} ${y(point.totalTokens)}`).join(" ");
  const area = `${line} L ${x(points.length - 1)} 170 L ${x(0)} 170 Z`;
  const labels = [...new Set([0, Math.floor((points.length - 1) / 2), points.length - 1])];
  return <div className="omp-chart">
    <svg viewBox="0 0 720 215" preserveAspectRatio="none" role="img" aria-label={`Token 用量趋势，共 ${points.length} 个时间段`} onMouseLeave={() => setActive(null)}>
      <title>Token 用量趋势</title>
      <desc>{points.map((point) => `${formatUsageTime(point.bucket, hourly)}：${usageTokenNumber.format(point.totalTokens)} Token，${usageNumber.format(point.requests)} 次请求`).join("；")}</desc>
      {[42, 106, 170].map((position) => <line key={position} className="omp-chart-grid" x1={chartLeft} y1={position} x2={chartRight} y2={position} />)}
      <text className="omp-chart-label" x={chartLeft - 6} y="47" textAnchor="end">{axisLabel}</text>
      <text className="omp-chart-label" x={chartLeft - 6} y="174" textAnchor="end">0</text>
      {points.length > 1 && <path className="omp-chart-area" d={area} />}
      <path className="omp-chart-line" d={line} />
      {points.map((point, index) => <circle key={`${point.bucket}:${index}`} className={`omp-chart-point${active === index ? " active" : ""}`} cx={x(index)} cy={y(point.totalTokens)} r={active === index ? 5 : 3.5} />)}
      {points.map((point, index) => {
        const left = index === 0 ? chartLeft : (x(index - 1) + x(index)) / 2;
        const right = index === points.length - 1 ? chartRight : (x(index) + x(index + 1)) / 2;
        return <rect key={point.bucket} className="omp-chart-hit" x={left} y="38" width={right - left} height="138"
          tabIndex={0} role="button"
          aria-label={`${formatUsageTime(point.bucket, hourly)}，${usageTokenNumber.format(point.totalTokens)} Token，${usageNumber.format(point.requests)} 次请求`}
          onMouseEnter={() => setActive(index)} onFocus={() => setActive(index)} onBlur={() => setActive(null)} />;
      })}
      {active !== null && points[active] && <g className="omp-chart-tooltip" transform={`translate(${Math.max(140, Math.min(580, x(active)))}, 0)`}>
        <rect x="-130" y="0" width="260" height="37" rx="6" />
        <text textAnchor="middle" y="14">{formatUsageTime(points[active].bucket, hourly)}</text>
        <text textAnchor="middle" y="29">{usageTokenNumber.format(points[active].totalTokens)} Token · {usageNumber.format(points[active].requests)} 次请求</text>
      </g>}
      {labels.map((index) => <text key={index} className="omp-chart-label" x={x(index)} y="203" textAnchor={index === 0 ? "start" : index === points.length - 1 ? "end" : "middle"}>{formatUsageTime(points[index].bucket, hourly)}</text>)}
    </svg>
  </div>;
}

function UsageRangePicker({ range, label, activeChoice, onApply }: {
  range: UsageRange;
  label: string;
  activeChoice: UsageChoice;
  onApply: (range: UsageRange, label: string, choice: UsageChoice) => void;
}) {
  const [open, setOpen] = useState(false);
  const [draftChoice, setDraftChoice] = useState<UsageChoice>(activeChoice);
  const [dates, setDates] = useState(() => dateFieldsForChoice(activeChoice, range));
  const container = useRef<HTMLDivElement>(null);
  const trigger = useRef<HTMLButtonElement>(null);
  const popup = useRef<HTMLDivElement>(null);

  useEffect(() => {
    if (!open) return;
    popup.current?.querySelector<HTMLButtonElement | HTMLInputElement>('button[aria-pressed="true"], input')?.focus();
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
          <label>开始日期<input type="date" value={dates.start} onChange={(event) => { setDates({ ...dates, start: event.target.value }); setDraftChoice("custom"); }} aria-invalid={draftChoice === "custom" && !!dateError} /></label>
          <span className="omp-range-arrow" aria-hidden="true">→</span>
          <label>结束日期<input type="date" value={dates.end} onChange={(event) => { setDates({ ...dates, end: event.target.value }); setDraftChoice("custom"); }} aria-invalid={draftChoice === "custom" && !!dateError} /></label>
        </div>
        {draftChoice === "custom" && dateError && <p className="omp-range-error" role="alert">{dateError}</p>}
        <div className="omp-range-actions"><button type="button" className="button button-primary" disabled={draftChoice === "custom" && !!dateError} onClick={apply}>应用</button></div>
      </div>
    </div>}
  </div>;
}

function AgentsUsagePanel({ stats, range, rangeLabel, activeChoice, loading, error, pricingNote, onRangeChange, onRefresh }: {
  stats: UsageStats | null;
  range: UsageRange;
  rangeLabel: string;
  activeChoice: UsageChoice;
  loading: "sync" | "read" | null;
  error: string;
  pricingNote: string;
  onRangeChange: (range: UsageRange, label: string, choice: UsageChoice) => void;
  onRefresh: () => void;
}) {
  return <section className="section-block agents-usage" aria-label="Agent 用量统计">
    <div className="omp-usage-toolbar">
      <div><h2>用量统计</h2><p>所有 Agent 的本机会话汇总 · {stats?.syncedAt ? `上次同步：${new Date(stats.syncedAt).toLocaleString("zh-CN")}` : "尚未同步"}</p></div>
      <div className="section-actions">
        <UsageRangePicker range={range} label={rangeLabel} activeChoice={activeChoice} onApply={onRangeChange} />
      </div>
    </div>
    {pricingNote && <p className="omp-pricing-note" role="status"><Glyph name="check" size={14} />{pricingNote}</p>}
    {loading && <div className="loading-panel" role="status"><span className="spinner" />{loading === "sync" ? "正在同步所有 Agent 本地用量…" : "正在读取此时间范围的用量…"}</div>}
    {!loading && error && <div className="alert alert-error" role="alert"><Glyph name="warning" size={18} /><span>{error}。当前没有可显示的最新数据。</span><button className="button button-muted" type="button" onClick={onRefresh}>重试同步</button></div>}
    {!loading && !error && stats && (stats.totalRequests === 0
      ? <Empty icon="grid" title="所选时间范围内没有用量" description="已完成同步，但所有 Agent 在此时间范围内都没有请求。可选择其他时间范围，或点击右上角「刷新状态」重新同步。" />
      : <>
        <div className="omp-summary" aria-label="Agent 用量汇总">
          <div className="omp-summary-card"><span>请求数</span><strong>{usageNumber.format(stats.totalRequests)}</strong><small>次请求</small></div>
          <div className="omp-summary-card"><span>总 Token</span><strong>{usageTokenNumber.format(stats.totalTokens)}</strong><small>输入 {usageTokenNumber.format(stats.inputTokens)} · 输出 {usageTokenNumber.format(stats.outputTokens)}</small></div>
          <div className="omp-summary-card"><span>{stats.unpricedRequests > 0 ? "已计价费用小计" : "估算费用"}</span><strong>{formatUsageCost(stats.totalCost)}</strong><small>USD{stats.unpricedRequests > 0 ? ` · ${usageNumber.format(stats.unpricedRequests)} 次未计价` : ""}</small></div>
        </div>
        <div className="section-heading model-breakdown-heading"><div><h2>模型明细 <span className="count">{stats.byModel.length}</span></h2><p>各模型的请求数、Token 总量与已计价费用小计；未计价请求不并入费用。</p></div></div>
        {stats.byModel.length ? <div className="omp-table-wrap"><table className="omp-table"><thead><tr><th scope="col">提供方 / 模型</th><th scope="col">请求数</th><th scope="col">总 Token</th><th scope="col">已计价费用 (USD)</th></tr></thead><tbody>{stats.byModel.map((model) => <tr key={`${model.provider}:${model.model}`}><th scope="row"><span>{model.provider || "未知提供方"}</span><strong>{model.model || "未知模型"}</strong></th><td>{usageNumber.format(model.requests)}</td><td>{usageTokenNumber.format(model.totalTokens)}</td><td>{formatUsageCost(model.cost)}{model.unpricedRequests > 0 && <small className="omp-unpriced">（{usageNumber.format(model.unpricedRequests)} 次未计价）</small>}</td></tr>)}</tbody></table></div> : null}
      </>)}
  </section>;
}

function AgentUsagePanel({ agentLabel, stats, range, rangeLabel, activeChoice, loading, error, onRangeChange, onRefresh }: {
  agentLabel: string;
  stats: UsageStats | null;
  range: UsageRange;
  rangeLabel: string;
  activeChoice: UsageChoice;
  loading: "sync" | "read" | null;
  error: string;
  onRangeChange: (range: UsageRange, label: string, choice: UsageChoice) => void;
  onRefresh: () => void;
}) {
  return <div className="omp-usage">
    <div className="omp-usage-toolbar">
      <div><h2>用量</h2><p>{agentLabel} 本机会话用量统计 · {stats?.syncedAt ? `上次同步：${new Date(stats.syncedAt).toLocaleString("zh-CN")}` : "尚未同步"}</p></div>
      <UsageRangePicker range={range} label={rangeLabel} activeChoice={activeChoice} onApply={onRangeChange} />
    </div>
    {loading && <div className="loading-panel" role="status"><span className="spinner" />{loading === "sync" ? `正在同步 ${agentLabel} 本地用量…` : "正在读取此时间范围的用量…"}</div>}
    {!loading && error && <div className="alert alert-error" role="alert"><Glyph name="warning" size={18} /><span>{error}。当前没有可显示的最新数据。</span><button className="button button-muted" type="button" onClick={onRefresh}>重试同步</button></div>}
    {!loading && !error && stats && (stats.totalRequests === 0
      ? <Empty icon="grid" title="所选时间范围内没有用量" description="已完成同步，但此时间范围内没有请求。可选择其他时间范围查看。" />
      : <>
        <div className="omp-summary" aria-label="用量汇总">
          <div className="omp-summary-card"><span>请求数</span><strong>{usageNumber.format(stats.totalRequests)}</strong><small>次请求</small></div>
          <div className="omp-summary-card"><span>总 Token</span><strong>{usageTokenNumber.format(stats.totalTokens)}</strong><small>输入 {usageTokenNumber.format(stats.inputTokens)} · 输出 {usageTokenNumber.format(stats.outputTokens)}</small></div>
          <div className="omp-summary-card"><span>缓存命中率</span><strong>{usagePercent.format(stats.cacheRate)}</strong><small>缓存读取 {usageTokenNumber.format(stats.cacheReadTokens)} Token</small></div>
          <div className="omp-summary-card"><span>{stats.unpricedRequests > 0 ? "已计价费用小计" : "估算费用"}</span><strong>{formatUsageCost(stats.totalCost)}</strong><small>USD{stats.unpricedRequests > 0 ? ` · ${usageNumber.format(stats.unpricedRequests)} 次未计价` : ""}</small></div>
        </div>
        <section className="omp-section">
          <div className="section-heading"><div><h2>Token 用量趋势</h2><p>按时间段统计 Token；悬停图表可查看各时间段请求数。</p></div></div>
          <UsageTrend trend={stats.trend} range={range} />
        </section>
        <section className="omp-section">
          <div className="section-heading"><div><h2>模型明细 <span className="count">{stats.byModel.length}</span></h2><p>非实际账单；未计价请求不并入费用。</p></div></div>
          {stats.byModel.length ? <div className="omp-table-wrap"><table className="omp-table"><thead><tr><th scope="col">提供方 / 模型</th><th scope="col">请求数</th><th scope="col">总 Token</th><th scope="col">缓存命中率</th><th scope="col">估算费用 (USD)</th></tr></thead><tbody>{stats.byModel.map((model) => <tr key={`${model.provider}:${model.model}`}><th scope="row"><span>{model.provider || "未知提供方"}</span><strong>{model.model || "未知模型"}</strong></th><td>{usageNumber.format(model.requests)}</td><td>{usageTokenNumber.format(model.totalTokens)}</td><td>{usagePercent.format(model.cacheRate)}</td><td>{formatUsageCost(model.cost)}{model.unpricedRequests > 0 && <small className="omp-unpriced">（{usageNumber.format(model.unpricedRequests)} 次未计价）</small>}</td></tr>)}</tbody></table></div> : <p className="omp-no-breakdown">此时间范围内没有可归属的模型明细。</p>}
        </section>
      </>)}
  </div>;
}

function Empty({ icon, title, description, action, onClick }: { icon: string; title: string; description: string; action?: string; onClick?: () => void }) {
  return <div className="empty-state"><span className="empty-icon"><Glyph name={icon} size={27} /></span><h3>{title}</h3><p>{description}</p>{action && onClick && <button className="button button-primary" onClick={onClick}>{action}<Glyph name="arrow" size={15} /></button>}</div>;
}

export default App;
