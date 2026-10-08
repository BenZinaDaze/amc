import { useEffect, useState, type FormEvent } from "react";
import { Glyph } from "../../components/Glyph";
import { Empty } from "../../components/Empty";
import { api, type Repository, type RepositorySkill, type State } from "../../api";
import { repositoryWebUrl } from "./gitUrl";

/// 仓库页：添加来源表单 + 来源列表 + 移除确认弹窗。
/// 移除前若仍有已分发技能会被拦下（后端同样校验，这里提前提示）。
export function RepositoriesPanel({ state, skills, scanLoading, forget, busy, isBusy, error, setError, setNotice, operation, reload, openExternal }: {
  state: State;
  skills: Record<number, RepositorySkill[]>;
  scanLoading: boolean;
  forget: (id: number) => void;
  busy: string;
  isBusy: boolean;
  error: string;
  setError: (message: string) => void;
  setNotice: (message: string) => void;
  operation: <T>(label: string, work: () => Promise<T>) => Promise<T | undefined>;
  reload: () => Promise<void>;
  openExternal: (url: string) => void;
}) {
  const [repoUrl, setRepoUrl] = useState("");
  const [repoRef, setRepoRef] = useState("");
  const [repositoryRemoval, setRepositoryRemoval] = useState<Repository | null>(null);

  // Escape 关闭与背景点击一致：仅在非忙碌时生效。
  useEffect(() => {
    if (!repositoryRemoval) return;
    const onKey = (event: KeyboardEvent) => { if (event.key === "Escape" && !isBusy) setRepositoryRemoval(null); };
    window.addEventListener("keydown", onKey);
    return () => window.removeEventListener("keydown", onKey);
  }, [repositoryRemoval, isBusy]);

  async function addRepository(event: FormEvent) {
    event.preventDefault();
    const url = repoUrl.trim();
    if (!url) return;
    const added = await operation("添加仓库", () => api.addRepository(url, repoRef.trim()));
    if (added) {
      setRepoUrl("");
      setRepoRef("");
      setNotice(`已添加 ${added.url}`);
      await reload();
    }
  }

  async function checkUpdates(id: number) {
    const result = await operation("检查更新", () => api.checkUpdates(id));
    if (result) {
      setNotice(result.message || "已检查仓库更新");
      await reload();
    }
  }

  async function removeRepository() {
    if (!repositoryRemoval) return;
    const repository = repositoryRemoval;
    const result = await operation("移除仓库", () => api.removeRepository(repository.id));
    if (!result) return;
    setRepositoryRemoval(null);
    forget(repository.id);
    setNotice(result.message || `已移除 ${repository.localPath || repository.url}`);
    await reload();
  }

  const repositoryRecords = repositoryRemoval ? state.skills.filter((item) => item.repositoryId === repositoryRemoval.id) : [];

  return <>
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
          <div className="item-content"><h3>{repo.localPath ? "本地 Git 仓库" : repo.url}</h3>{repo.localPath && <p className="path-line">{repo.localPath}</p>}<p>{repo.localPath ? "本地仓库工作树" : `引用：${repo.reference || "默认分支"}`} · {skills[repo.id]?.length ?? (scanLoading ? "扫描中" : 0)} 项技能 · {state.skills.filter((item) => item.repositoryId === repo.id).length} 项已分发</p></div>
          <div className="item-actions">
            {repositoryUrl && <button type="button" className="icon-button skill-source-link" title="打开 GitHub 仓库" aria-label={`打开 ${repo.url} 的 GitHub 仓库`} onClick={() => openExternal(repositoryUrl)}><Glyph name="external" size={15} /></button>}
            <button className="button button-muted" disabled={isBusy || scanLoading} onClick={() => void checkUpdates(repo.id)}>检查更新</button>
            <button className="button button-danger-ghost" disabled={isBusy} onClick={() => { setError(""); setRepositoryRemoval(repo); }}>{repo.localPath ? "移除来源" : "移除仓库"}</button>
          </div>
        </article>;
      })}
    </div> : <Empty icon="repo" title="还没有技能来源" description="输入 Git 地址以发现可安装的技能。" />}
    {repositoryRemoval && <div className="modal-backdrop" onMouseDown={(event) => { if (event.target === event.currentTarget && !isBusy) setRepositoryRemoval(null); }}>
      <div className="modal" role="dialog" aria-modal="true" aria-labelledby="remove-repo-title">
        <div className="modal-head">
          <div><div className="eyebrow">技能来源 / 移除确认</div><h2 id="remove-repo-title">{repositoryRemoval.localPath ? "移除本地 Git 来源" : "移除 Git 仓库"}</h2></div>
          <button className="icon-button" aria-label="关闭" disabled={isBusy} onClick={() => setRepositoryRemoval(null)}><Glyph name="close" size={20} /></button>
        </div>
        <div className="modal-body">
          {error && <div className="alert alert-error" role="alert"><Glyph name="warning" size={18} /><span>{error}</span></div>}
          <p className="repo-remove-url">{repositoryRemoval.localPath || repositoryRemoval.url}</p>
          {repositoryRecords.length > 0 ? <div className="plan-warnings"><p><Glyph name="warning" size={16} />此来源还有 {repositoryRecords.length} 项已分发技能。请先移除这些技能，再移除来源。</p></div> :
            <div className="plan-warnings"><p><Glyph name="warning" size={16} />将删除来源记录及 AMC 管理的{repositoryRemoval.localPath ? "本地 Git 缓存" : "Git 缓存"}。{repositoryRemoval.localPath && "原始路径不会删除。"}此操作无法撤销。</p></div>}
          <div className="modal-actions">
            <button className="button button-muted" disabled={isBusy} onClick={() => setRepositoryRemoval(null)}>取消</button>
            <button className="button button-danger" disabled={isBusy || repositoryRecords.length > 0} onClick={() => void removeRepository()}>{isBusy ? "正在移除…" : repositoryRemoval.localPath ? "确认移除来源" : "确认移除仓库"}</button>
          </div>
        </div>
      </div>
    </div>}
  </>;
}
