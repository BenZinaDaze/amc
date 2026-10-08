import { Glyph } from "../components/Glyph";
import { Alerts } from "../components/Alerts";
import { PageHeading } from "../components/PageHeading";
import { RepositoriesPanel } from "../apps/repositories/RepositoriesPanel";
import type { RepositorySkill } from "../api";
import type { PageProps } from "./PageProps";

export function RepositoriesPage({ state, loading, busy, isBusy, error, notice, setError, setNotice, operation, reload, openExternal, repositorySkills, repositoryScanLoading, forget }: PageProps & {
  repositorySkills: Record<number, RepositorySkill[]>;
  repositoryScanLoading: boolean;
  forget: (id: number) => void;
}) {
  return <>
    <PageHeading title="技能仓库" description="从已发现的 Git 来源同步可用技能。" actions={
      <button className="button button-muted refresh-button" aria-label="刷新状态" onClick={() => void reload()} disabled={loading || isBusy}><Glyph name="refresh" size={16} />刷新状态</button>
    } />
    <Alerts error={error} notice={notice} onErrorClose={() => setError("")} onNoticeClose={() => setNotice("")} />
    {!loading && state && <RepositoriesPanel state={state} skills={repositorySkills} scanLoading={repositoryScanLoading} forget={forget} busy={busy} isBusy={isBusy} error={error} setError={setError} setNotice={setNotice} operation={operation} reload={reload} openExternal={openExternal} />}
  </>;
}
