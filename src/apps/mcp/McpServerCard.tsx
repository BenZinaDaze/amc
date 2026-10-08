import { Glyph } from "../../components/Glyph";
import { AgentMark, agentMeta } from "../../components/AgentMark";
import type { McpAgent, McpServer } from "../../api";
import { mcpCardDetail } from "./mcpCardDetail";

export function McpServerCard({ server, installedAgents, isBusy, onEdit, onToggleAgent, onRemove }: {
  server: McpServer;
  installedAgents: McpAgent[];
  isBusy: boolean;
  onEdit: () => void;
  onToggleAgent: (agent: McpAgent) => void;
  onRemove: () => void;
}) {
  const detail = mcpCardDetail(server.config);
  return <article className="item-card">
    <div className="item-icon purple"><Glyph name="plug" /></div>
    <div className="item-content">
      <div className="item-title">
        <h3>{server.name}</h3>
        <span className="tag tag-muted">{String(server.config.type || "stdio")}</span>
      </div>
      {detail && <p className="item-desc" title={detail}>{detail}</p>}
    </div>
    <div className="item-actions">
      <div className="mcp-agents">
        {(["omp", "claude", "codex"] as McpAgent[]).filter((agent) => installedAgents.includes(agent) || server.agents.includes(agent)).map((agent) => {
          const enabled = server.agents.includes(agent);
          const detected = installedAgents.includes(agent);
          return <button key={agent} type="button" className={`mcp-agent-mark ${enabled && detected ? "on" : ""}`} disabled={isBusy} aria-pressed={enabled} aria-label={`${agentMeta[agent].label} ${enabled ? "停用" : "启用"}`} title={`${agentMeta[agent].label}${detected ? "" : " · 未检测到安装"} · ${enabled ? "已启用，点击停用" : "未启用，点击启用"}`} onClick={() => onToggleAgent(agent)}>
            <AgentMark agent={agent} active={enabled && detected} />
          </button>;
        })}
      </div>
      <button className="button button-muted" onClick={onEdit} disabled={isBusy}>编辑</button>
      <button className="button button-danger-ghost" onClick={onRemove} disabled={isBusy}>移除</button>
    </div>
  </article>;
}
