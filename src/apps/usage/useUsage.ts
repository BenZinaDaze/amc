import { useCallback, useEffect, useRef, useState } from "react";
import { api, type UsageRange, type UsageStats } from "../../api";
import { errorText } from "../../utils";
import type { UsageChoice } from "./format";

export type UsageSelection = { range: UsageRange; label: string; choice: UsageChoice };
const defaultSelection: UsageSelection = { range: "24h", label: "近24小时", choice: "24h" };

/// 单 Agent 用量（OMP / Claude Code / Codex 页共用）：
/// 进入页面先 sync 全量再展示；切换时间范围只读；请求编号丢弃过期
/// 响应；在途的 sync 会被后续 read 等待而不是重复发起。
export function useAgentUsage(agentId: string, name: string) {
  const [stats, setStats] = useState<UsageStats | null>(null);
  const [loading, setLoading] = useState<"sync" | "read" | null>(null);
  const [error, setError] = useState("");
  const [selection, setSelection] = useState<UsageSelection>(defaultSelection);
  const loadId = useRef(0);
  const rangeRef = useRef<UsageRange>("24h");
  const pendingSync = useRef<Promise<UsageStats> | null>(null);

  const load = useCallback(async (mode: "sync" | "read", range: UsageRange) => {
    const request = ++loadId.current;
    setStats(null);
    setError("");
    const pending = mode === "read" ? pendingSync.current : null;
    setLoading(pending ? "sync" : mode);
    let activeOperation: "sync" | "read" = mode;
    let sync: Promise<UsageStats> | null = null;
    try {
      if (mode === "sync") {
        sync = api.syncAgentUsage(agentId, range);
        pendingSync.current = sync;
      } else if (pending) {
        activeOperation = "sync";
        await pending;
        if (request !== loadId.current) return;
        activeOperation = "read";
        setLoading("read");
      }
      if (request !== loadId.current) return;
      const result = mode === "sync" ? await sync! : await api.getAgentUsage(agentId, range);
      if (request === loadId.current) setStats(result);
    } catch (reason) {
      if (request === loadId.current) setError(`${activeOperation === "sync" ? `${name} 用量同步` : `${name} 用量读取`}失败：${errorText(reason)}`);
    } finally {
      if (sync && pendingSync.current === sync) pendingSync.current = null;
      if (request === loadId.current) setLoading(null);
    }
  }, [agentId, name]);

  useEffect(() => {
    void load("sync", rangeRef.current);
    return () => { ++loadId.current; pendingSync.current = null; };
  }, [load]);

  const changeRange = useCallback((range: UsageRange, label: string, choice: UsageChoice) => {
    const changed = range !== rangeRef.current;
    rangeRef.current = range;
    setSelection({ range, label, choice });
    if (changed) void load("read", range);
  }, [load]);

  const refresh = useCallback(() => { void load("sync", rangeRef.current); }, [load]);

  return { stats, loading, error, selection, changeRange, refresh };
}

/// 所有 Agent 的汇总用量（Agents 页）：进入页面 sync；「刷新状态」
/// 会先拉取远程定价配置再同步，定价失败不阻塞统计。
export function useAgentsUsage() {
  const [stats, setStats] = useState<UsageStats | null>(null);
  const [loading, setLoading] = useState<"sync" | "read" | null>(null);
  const [error, setError] = useState("");
  const [pricingNote, setPricingNote] = useState("");
  const [selection, setSelection] = useState<UsageSelection>(defaultSelection);
  const [refreshBusy, setRefreshBusy] = useState(false);
  const loadId = useRef(0);
  const rangeRef = useRef<UsageRange>("24h");

  const load = useCallback(async (mode: "sync" | "read", range: UsageRange) => {
    const request = ++loadId.current;
    setStats(null);
    setError("");
    setLoading(mode);
    try {
      const result = mode === "sync" ? await api.syncAgentsUsage(range) : await api.getAgentsUsage(range);
      if (request === loadId.current) setStats(result);
    } catch (reason) {
      if (request === loadId.current) setError(`${mode === "sync" ? "Agent 用量同步" : "Agent 用量读取"}失败：${errorText(reason)}`);
    } finally {
      if (request === loadId.current) setLoading(null);
    }
  }, []);

  useEffect(() => {
    void load("sync", rangeRef.current);
  }, [load]);

  const changeRange = useCallback((range: UsageRange, label: string, choice: UsageChoice) => {
    const changed = range !== rangeRef.current;
    rangeRef.current = range;
    setSelection({ range, label, choice });
    if (changed) void load("read", range);
  }, [load]);

  // 刷新状态会先从远程仓库根目录拉取定价配置，再同步所有 Agent 用量；
  // 定价拉取失败不阻塞统计，只提示已改用本地缓存定价。整段流程从入口
  // 就是互斥的：定价请求在途时按钮保持禁用，避免重复点击叠加命令。
  const refresh = useCallback(() => {
    if (refreshBusy || loading !== null) return;
    setRefreshBusy(true);
    void (async () => {
      setPricingNote("");
      try {
        setPricingNote(await api.refreshPricing());
      } catch (reason) {
        setPricingNote(`定价配置刷新失败：${errorText(reason)}，已使用本地缓存定价。`);
      }
      try {
        await load("sync", rangeRef.current);
      } finally {
        setRefreshBusy(false);
      }
    })();
  }, [refreshBusy, loading, load]);

  return { stats, loading, error, pricingNote, selection, refreshBusy, changeRange, refresh };
}
