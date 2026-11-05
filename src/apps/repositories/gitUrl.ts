import type { Repository } from "../../api";

export function repositoryWebUrl(repository?: Repository): string | null {
  if (!repository || repository.local) return null;
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

/// 仓库展示名：SSH/HTTPS URL 取 owner/name，本地仓库给可读文案。
export function repositoryLabel(repository: Repository | undefined): string {
  if (!repository) return "";
  if (repository.local) return "本地目录";
  const withoutGit = repository.url.trim().replace(/\/+$/, "").replace(/\.git$/i, "");
  const scpLike = /^git@[^:]+:(.+)$/.exec(withoutGit);
  const https = /^https?:\/\/[^/]+\/(.+)$/.exec(withoutGit);
  return (scpLike?.[1] ?? https?.[1] ?? withoutGit).replace(/^\/+/, "");
}

export function skillWebUrl(repository: Repository | undefined, path: string): string | null {
  const base = repositoryWebUrl(repository);
  if (!base) return null;
  const parts = path === "." ? [] : path.split("/");
  if (parts.some((part) => !part || part === "." || part === ".." || part.includes("\\"))) return null;
  const reference = (repository?.reference || "HEAD").split("/").map(encodeURIComponent).join("/");
  return `${base}/tree/${reference}${parts.length ? `/${parts.map(encodeURIComponent).join("/")}` : ""}`;
}
