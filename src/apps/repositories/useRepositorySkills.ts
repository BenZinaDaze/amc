import { useCallback, useRef, useState } from "react";
import { api, type Repository, type RepositorySkill } from "../../api";
import { errorText } from "../../utils";

/// 仓库技能扫描：并行列出各仓库可用技能，请求编号丢弃过期响应；
/// 移除仓库时 forget 清掉对应缓存。
export function useRepositorySkills(setError: (message: string) => void) {
  const [skills, setSkills] = useState<Record<number, RepositorySkill[]>>({});
  const [errors, setErrors] = useState<Record<number, string>>({});
  const [scanLoading, setScanLoading] = useState(false);
  const loadId = useRef(0);

  const load = useCallback(async (repositories: Repository[]) => {
    const request = ++loadId.current;
    setScanLoading(true);
    const results = await Promise.allSettled(repositories.map(async (repository) => [repository.id, await api.listRepositorySkills(repository.id)] as const));
    if (request !== loadId.current) return;
    const loaded: Record<number, RepositorySkill[]> = {};
    const failures: Record<number, string> = {};
    results.forEach((result, index) => {
      const repository = repositories[index];
      if (result.status === "fulfilled") loaded[result.value[0]] = result.value[1];
      else failures[repository.id] = errorText(result.reason);
    });
    setSkills(loaded);
    setErrors(failures);
    setScanLoading(false);
    const failureText = Object.entries(failures).map(([id, reason]) => `#${id} ${repositories.find((repository) => repository.id === Number(id))?.url || "仓库"}：${reason}`).join("；");
    if (failureText) setError(`部分仓库扫描失败：${failureText}`);
  }, [setError]);

  const forget = useCallback((id: number) => {
    setSkills((current) => {
      const next = { ...current };
      delete next[id];
      return next;
    });
    setErrors((current) => {
      const next = { ...current };
      delete next[id];
      return next;
    });
  }, []);

  return { skills, errors, scanLoading, load, forget };
}
