import { useState } from "react";
import { Glyph } from "../../components/Glyph";
import { Empty } from "../../components/Empty";
import { AgentMark } from "../../components/AgentMark";
import { api, type Plan, type RepositorySkill, type Skill, type SkillTarget, type State } from "../../api";
import { skillWebUrl } from "../repositories/gitUrl";

type SkillTab = "installed" | "discover";
type DiscoveredSkill = RepositorySkill & { repositoryId: number };

const skillTargetMeta: Record<SkillTarget, { label: string; path: string }> = {
  omp: { label: "OMP", path: "~/.agents/skills" },
  codex: { label: "Codex", path: "~/.agents/skills" },
  claude: { label: "Claude Code", path: "~/.claude/skills" },
};

/// Skills 页：已分发（图标即开关）与发现（从仓库安装）两个视图。
export function SkillsPanel({ state, repositorySkills, repositorySkillErrors, repositoryScanLoading, isBusy, prepare, setNotice, operation, reload, navigateRepositories, openExternal }: {
  state: State;
  repositorySkills: Record<number, RepositorySkill[]>;
  repositorySkillErrors: Record<number, string>;
  repositoryScanLoading: boolean;
  isBusy: boolean;
  prepare: (label: string, work: () => Promise<Plan>) => void;
  setNotice: (message: string) => void;
  operation: <T>(label: string, work: () => Promise<T>) => Promise<T | undefined>;
  reload: () => Promise<void>;
  navigateRepositories: () => void;
  openExternal: (url: string) => void;
}) {
  const [skillTab, setSkillTab] = useState<SkillTab>("installed");
  const [discoverSearchOpen, setDiscoverSearchOpen] = useState(false);
  const [discoverSearch, setDiscoverSearch] = useState("");

  async function refreshAllRepositories() {
    const result = await operation("刷新所有仓库", () => api.checkAllUpdates());
    if (!result) return;
    setNotice(result.message || "已刷新所有仓库");
    await reload();
  }

  async function toggleSkillTarget(skill: Skill, target: SkillTarget) {
    const enabled = target === "omp" ? !skill.omp : target === "codex" ? !skill.codex : !skill.claude;
    prepare(`${enabled ? "启用" : "停用"} ${skill.name}`, () => api.planSkillToggle(skill.name, target, enabled));
  }

  // Agent 图标开关只对检测到已安装的 Agent 显示。
  const installedAgents = state.installedAgents;
  const distributedSkills = state.skills;
  const detectedOnlySkills = state.detected;
  const discoveredSkills: DiscoveredSkill[] = state.repositories.flatMap((repository) => (repositorySkills[repository.id] || []).map((skill) => ({ ...skill, repositoryId: repository.id })));
  const normalizedDiscoverSearch = discoverSearch.trim().toLocaleLowerCase();
  const filteredDiscoveredSkills = normalizedDiscoverSearch ? discoveredSkills.filter((skill) => {
    const repository = state.repositories.find((item) => item.id === skill.repositoryId);
    return [skill.name, skill.description, skill.path, repository?.url, repository?.localPath, repository?.reference].filter(Boolean).join(" ").toLocaleLowerCase().includes(normalizedDiscoverSearch);
  }) : discoveredSkills;
  const repositoryFailureCount = Object.keys(repositorySkillErrors).length;

  return <>
    <div className="section-heading section-heading-top">
      <div><h2>{skillTab === "installed" ? "技能列表" : "发现技能"} <span className="count">{skillTab === "installed" ? distributedSkills.length : discoveredSkills.length}</span></h2><p>{skillTab === "installed" ? "分发技能写入用户级目录，图标即开关。" : "从已添加的仓库发现技能，逐个选择安装。"}</p></div>
      <div className="section-actions">
        <div className="skill-tabs" role="tablist" aria-label="Skills 视图">
          <button className={`skill-tab ${skillTab === "installed" ? "active" : ""}`} role="tab" aria-selected={skillTab === "installed"} onClick={() => setSkillTab("installed")}>已分发</button>
          <button className={`skill-tab ${skillTab === "discover" ? "active" : ""}`} role="tab" aria-selected={skillTab === "discover"} onClick={() => setSkillTab("discover")}>发现技能</button>
        </div>
        {skillTab === "discover" && <>
          <div className="skill-tool-buttons">
            <button className="button button-primary" onClick={navigateRepositories}><Glyph name="plus" size={16} />管理仓库</button>
            <button className="button button-muted" onClick={() => void refreshAllRepositories()} disabled={isBusy || repositoryScanLoading}><Glyph name="refresh" size={16} />刷新所有仓库</button>
            <button className="button button-muted skill-search-toggle" aria-label="搜索" title="搜索" aria-pressed={discoverSearchOpen} onClick={() => { setDiscoverSearchOpen((open) => !open); if (discoverSearchOpen) setDiscoverSearch(""); }}><Glyph name="search" size={18} /></button>
          </div>
        </>}
      </div>
    </div>
    {skillTab === "discover" && discoverSearchOpen && <div className="skill-search-row"><div className="skill-search"><Glyph name="search" size={16} /><input autoFocus value={discoverSearch} onChange={(event) => setDiscoverSearch(event.target.value)} placeholder="搜索名称、描述或来源" aria-label="搜索发现的 Skill" /><button type="button" className="icon-button skill-search-clear" aria-label="关闭搜索" onClick={() => { setDiscoverSearch(""); setDiscoverSearchOpen(false); }}><Glyph name="close" size={15} /></button></div></div>}
    {skillTab === "installed" ? <>
      {distributedSkills.length ? <div className="card-list">
        {distributedSkills.map((skill) => {
          return <article className="item-card" key={skill.id}>
            <div className="item-icon amber"><Glyph name="spark" /></div>
            <div className="item-content">
              <div className="item-title"><h3>{skill.name}</h3>{skill.updateAvailable && <span className="tag tag-info">有更新</span>}</div>
              <p className="item-desc" title={skill.description}>{skill.description || "此技能未提供说明"}</p>
            </div>
            <div className="item-actions">
              <div className="skill-targets">
                {(["omp", "codex", "claude"] as SkillTarget[]).filter((target) => installedAgents.includes(target) || (target === "omp" ? skill.omp : target === "codex" ? skill.codex : skill.claude)).map((target) => {
                  const enabled = target === "omp" ? skill.omp : target === "codex" ? skill.codex : skill.claude;
                  const detected = installedAgents.includes(target);
                  const note = target === "claude" ? "" : "（与另一 Agent 共用目录，停用写入其配置文件）";
                  return <button key={target} type="button" className={`mcp-agent-mark ${enabled && detected ? "on" : ""}`} disabled={isBusy} aria-pressed={enabled} aria-label={`${skillTargetMeta[target].label} ${enabled ? "停用" : "启用"}`} title={`${skillTargetMeta[target].label} · ${skillTargetMeta[target].path}${note}${detected ? "" : " · 未检测到安装"} · ${enabled ? "已启用，点击停用" : "未启用，点击启用"}`} onClick={() => void toggleSkillTarget(skill, target)}>
                    <AgentMark agent={target} active={enabled && detected} />
                  </button>;
                })}
              </div>
              {skill.updateAvailable && <button className="button button-primary" disabled={isBusy} onClick={() => prepare("预览技能更新", () => api.planSkill(skill.repositoryId, skill.skillPath))}>更新</button>}
              <button className="button button-danger-ghost" disabled={isBusy} onClick={() => prepare("预览移除技能", () => api.planSkillRemove(skill.name))}>移除</button>
            </div>
          </article>;
        })}
      </div> : <Empty icon="spark" title="还没有分发的技能" description="切换到“发现技能”，从已添加的仓库安装；安装会写入 ~/.agents/skills（OMP 与 Codex）与 ~/.claude/skills，图标即开关。" action="发现技能" onClick={() => setSkillTab("discover")} />}
      {detectedOnlySkills.length > 0 && <section className="section-block detected-skills">
        <div className="section-heading"><div><h2>已检测到的其他 Skills <span className="count">{detectedOnlySkills.length}</span></h2><p>这些技能来自本地通用或兼容来源，不属于 AMC 分发管理；这里只读展示，不提供更新或卸载操作。</p></div></div>
        <div className="card-list">
          {detectedOnlySkills.map((skill) => <article className="item-card" key={`${skill.source}:${skill.path}`}>
            <div className="item-icon amber"><Glyph name="spark" /></div>
            <div className="item-content"><div className="item-title"><h3>{skill.name}</h3>{skill.shadowed && <span className="tag tag-warn">同名来源</span>}<span className="tag tag-muted">{skill.source}</span></div><p>{skill.description || "此技能未提供说明"}</p></div>
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
            const installed = distributedSkills.find((item) => item.repositoryId === skill.repositoryId && item.skillPath === skill.path);
            return <article className="item-card skill-discovery-card" key={`${skill.repositoryId}:${skill.path}`}>
              <div className="skill-card-heading"><div className="item-icon amber"><Glyph name="spark" /></div><div className="item-title"><h3>{skill.name}</h3>{installed && <span className="tag tag-good">已分发</span>}{installed?.updateAvailable && <span className="tag tag-info">有更新</span>}</div></div>
              <div className="item-content skill-card-body"><p>{skill.description || "此技能未提供说明"}</p></div>
              <div className="item-actions">{skillUrl && <button type="button" className="icon-button skill-source-link" title="打开 GitHub 技能目录" aria-label={`打开 ${skill.name} 的 GitHub 技能目录`} onClick={() => openExternal(skillUrl)}><Glyph name="external" size={15} /></button>}{installed ? <>{installed.updateAvailable && <button className="button button-primary" disabled={isBusy} onClick={() => prepare("预览技能更新", () => api.planSkill(installed.repositoryId, installed.skillPath))}>更新</button>}<button className="button button-danger-ghost" disabled={isBusy} onClick={() => prepare("预览移除技能", () => api.planSkillRemove(installed.name))}>移除</button></> : <button className="button button-primary skill-install-button" disabled={isBusy} onClick={() => prepare("预览安装技能", () => api.planSkill(skill.repositoryId, skill.path))}>安装</button>}</div>
            </article>;
          })}
        </div>
      ) : !repositoryScanLoading ? (
        <Empty icon="spark" title={normalizedDiscoverSearch ? "未找到匹配的技能" : "未发现可安装的技能"} description={normalizedDiscoverSearch ? `没有匹配“${discoverSearch.trim()}”的技能。` : repositoryFailureCount > 0 ? "没有成功扫描到技能；请刷新失败仓库后重试。" : state.repositories.length ? "仓库中需要包含带 name 和 description 的 SKILL.md。" : "先添加 Git 仓库，之后可以在这里发现并选择安装 Skill。"} />
      ) : null}
    </>}
  </>;
}
