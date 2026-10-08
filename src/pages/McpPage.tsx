import { useState } from "react";
import { Glyph } from "../components/Glyph";
import { Alerts } from "../components/Alerts";
import { Empty } from "../components/Empty";
import { PageHeading } from "../components/PageHeading";
import { McpFormModal } from "../apps/mcp/McpFormModal";
import { McpServerCard } from "../apps/mcp/McpServerCard";
import { api, type McpServer } from "../api";
import type { PageProps } from "./PageProps";

export function McpPage({ state, loading, isBusy, error, notice, setError, setNotice, prepare, reload }: PageProps) {
  const [editing, setEditing] = useState<{ server?: McpServer } | null>(null);

  return <>
    <PageHeading title="MCP 服务" description="统一保存服务定义，按 Agent 开关写入或移除用户级配置。" actions={
      <button className="button button-muted refresh-button" aria-label="刷新状态" onClick={() => void reload()} disabled={loading || isBusy}><Glyph name="refresh" size={16} />刷新状态</button>
    } />
    <Alerts error={error} notice={notice} onErrorClose={() => setError("")} onNoticeClose={() => setNotice("")} />
    {!loading && state && <>
      <div className="section-heading section-heading-top">
        <div><h2>服务列表 <span className="count">{state.mcp.length}</span></h2><p>统一保存后投影到各 Agent 的用户级配置；开关即写入或移除对应文件条目。</p></div>
        <div className="section-actions">
          <button className="button button-primary" onClick={() => { setError(""); setEditing({}); }} disabled={isBusy}><Glyph name="plus" size={17} />添加服务</button>
        </div>
      </div>
      {state.mcp.length ? (
        <div className="card-list">
          {state.mcp.map((server) => <McpServerCard key={server.name} server={server} installedAgents={state.installedAgents} isBusy={isBusy}
            onEdit={() => { setError(""); setEditing({ server }); }}
            onToggleAgent={(agent) => {
              const enabled = !server.agents.includes(agent);
              void prepare(enabled ? `在 Agent 中启用 ${server.name}` : `在 Agent 中停用 ${server.name}`, () => api.planMcpToggle(server.name, agent, enabled));
            }}
            onRemove={() => void prepare("预览移除服务", () => api.planMcp(server.name, null, []))} />)}
        </div>
      ) : <Empty icon="plug" title="还没有 MCP 服务" description="添加 stdio、HTTP 或 SSE 服务，勾选要写入的 Agent，先预览再应用。AMC 只管理通过它保存的服务。" action="添加服务" onClick={() => { setError(""); setEditing({}); }} />}
      {editing && state && <McpFormModal server={editing.server} existingNames={state.mcp.map((server) => server.name)} installedAgents={state.installedAgents} isBusy={isBusy}
        error={error} setError={setError}
        onClose={() => setEditing(null)}
        onPreview={(name, config, agents) => prepare("预览 MCP 变更", () => api.planMcp(name, config, agents))} />}
    </>}
  </>;
}
