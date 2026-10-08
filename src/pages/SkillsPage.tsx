import { Glyph } from "../components/Glyph";
import { Alerts } from "../components/Alerts";
import { PageHeading } from "../components/PageHeading";
import { SkillsPanel } from "../apps/skills/SkillsPanel";
import type { RepositorySkill } from "../api";
import type { PageProps } from "./PageProps";

export function SkillsPage({ state, loading, isBusy, error, notice, setError, setNotice, prepare, operation, reload, navigate, openExternal, repositorySkills, repositorySkillErrors, repositoryScanLoading }: PageProps & {
  repositorySkills: Record<number, RepositorySkill[]>;
  repositorySkillErrors: Record<number, string>;
  repositoryScanLoading: boolean;
}) {
  return <>
    <PageHeading title="Skills" description="管理已安装技能，保留本地修改的控制权。" actions={
      <button className="button button-muted refresh-button" aria-label="刷新状态" onClick={() => void reload()} disabled={loading || isBusy}><Glyph name="refresh" size={16} />刷新状态</button>
    } />
    <Alerts error={error} notice={notice} onErrorClose={() => setError("")} onNoticeClose={() => setNotice("")} />
    {!loading && state && <SkillsPanel state={state} repositorySkills={repositorySkills} repositorySkillErrors={repositorySkillErrors} repositoryScanLoading={repositoryScanLoading} isBusy={isBusy} prepare={prepare} setNotice={setNotice} operation={operation} reload={reload} navigateRepositories={() => navigate("repositories")} openExternal={openExternal} />}
  </>;
}
