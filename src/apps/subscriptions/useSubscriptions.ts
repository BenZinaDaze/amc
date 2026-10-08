import { useCallback, useEffect, useRef, useState } from "react";
import { listen } from "@tauri-apps/api/event";
import { api, type SubscriptionStatus } from "../../api";
import { errorText } from "../../utils";

/// 订阅配额状态：监听器随 App 常驻（卸载页面也不丢事件），
/// 60 秒内的重复加载直接跳过。
export function useSubscriptions() {
  const [subscriptions, setSubscriptions] = useState<SubscriptionStatus[] | null>(null);
  const [loading, setLoading] = useState(false);
  const [error, setError] = useState("");
  const loadId = useRef(0);
  const fetchedAt = useRef(0);
  const listening = useRef<Promise<void> | null>(null);

  const load = useCallback(async (force: boolean) => {
    if (!force && Date.now() - fetchedAt.current < 60_000) return;
    const request = ++loadId.current;
    fetchedAt.current = Date.now();
    // listen() 是异步 IPC：首个请求必须等监听器注册完成，否则最早的事件
    // 可能落在注册前被丢掉。时间戳先于 await 同步落位，StrictMode 的二次
    // 调用仍被 60 秒守卫挡住，不会重复发起供应商查询。
    await listening.current;
    setLoading(true);
    setError("");
    try {
      const next = await api.fetchSubscriptions(request);
      if (request === loadId.current) setSubscriptions(next);
    } catch (reason) {
      if (request === loadId.current) setError(`读取订阅配额失败：${errorText(reason)}`);
    } finally {
      if (request === loadId.current) setLoading(false);
    }
  }, []);

  // 订阅卡片逐张流入：后端先发占位卡片（subscription-load），之后每查完一张
  // 发一次 subscription-status，前端按 nonce 丢弃过期请求的残留事件。刷新时
  // 旧数据原地保留、逐张更新；命令返回值兜底为权威列表。
  useEffect(() => {
    const unlistenLoad = listen<{ nonce: number; statuses: SubscriptionStatus[] }>("subscription-load", (event) => {
      if (event.payload.nonce !== loadId.current) return;
      setSubscriptions((prev) => {
        // 以存储顺序为基准对齐成员；已有数据的卡片保留旧值等待更新。
        if (!prev) return event.payload.statuses;
        return event.payload.statuses.map((placeholder) => prev.find((entry) => entry.id === placeholder.id) ?? placeholder);
      });
    });
    const unlistenStatus = listen<{ nonce: number; status: SubscriptionStatus }>("subscription-status", (event) => {
      if (event.payload.nonce !== loadId.current) return;
      const incoming = event.payload.status;
      setSubscriptions((prev) => {
        if (!prev) return [incoming];
        const index = prev.findIndex((entry) => entry.id === incoming.id);
        if (index < 0) return [...prev, incoming];
        const next = [...prev];
        next[index] = incoming;
        return next;
      });
    });
    listening.current = Promise.all([unlistenLoad, unlistenStatus]).then(() => {}, () => {});
    return () => {
      void unlistenLoad.then((unlisten) => unlisten());
      void unlistenStatus.then((unlisten) => unlisten());
    };
  }, []);

  const remove = useCallback(async (id: string) => {
    try {
      await api.removeSubscriptionPlan(id);
      await load(true);
    } catch (reason) {
      setError(`删除订阅套餐失败：${errorText(reason)}`);
    }
  }, [load]);

  return { subscriptions, loading, error, load, remove };
}
